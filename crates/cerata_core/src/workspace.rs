//! Per-window workspace state, the window registry and opening a workspace.
//!
//! Ported from Writer `state.rs` (`transition_to_workspace`,
//! `with_workspace_snapshot`, `clear_workspace_if_current`, pending opens,
//! `find_by_workspace`, `find_by_standalone_file`) and
//! `commands/workspace.rs:419-603` (canonicalization, background bootstrap).
//! Windows are identified by a numeric id instead of Tauri labels. Every
//! asynchronous result is checked against a `{root, epoch}` snapshot; the
//! epoch makes the A, B, A sequence (ABA) distinguishable from staying on A.

use crate::epoch::{EpochCounter, EpochToken};
use crate::fs::{FsError, WorkspaceFs};
use crate::history::History;
use crate::history::device::Device;
use crate::ignore::{WorkspaceIgnore, global_gitignore_path};
use crate::index::{IndexedFile, LiveIndex, relative_slash_path};
use crate::library::LibraryKey;
use crate::markdown::is_markdown_path;
use crate::open_target::OpenTarget;
use crate::reconcile::{
    FileAccess, OsFiles, ReconcileContext, ReconcileTrigger, Reconciler, note_content, note_record,
    path_from_relative,
};
use crate::self_writes::{SelfWriteLog, WriteSink};
use crate::settings::Settings;
use crate::store::{NoteChange, Store, StoreOpen, StoredNote, library_store_dir};
use crate::watcher::{WatchContext, WatchEnvelope, WatchEvent, WorkspaceWatcher};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

/// How long a library change waits for its pending versions.
pub const HISTORY_FLUSH: std::time::Duration = std::time::Duration::from_secs(2);

/// `{root, epoch}` of a window's workspace at some instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub root: PathBuf,
    pub epoch: u64,
}

/// Everything that lives exactly as long as one open workspace.
pub struct WorkspaceRuntime {
    pub root: PathBuf,
    /// The library's id (ADR 0009), the key of its sessions and pins.
    pub library_id: String,
    pub token: EpochToken,
    pub fs: WorkspaceFs,
    pub index: Arc<Mutex<LiveIndex>>,
    pub ignore: Arc<RwLock<Arc<WorkspaceIgnore>>>,
    pub self_writes: Arc<SelfWriteLog>,
    pub index_ready: Arc<AtomicBool>,
    /// The library's store once opened; `None` before or without one.
    pub store: Arc<RwLock<Option<Store>>>,
    /// The library's history, with its store (EP-018).
    pub history: Arc<RwLock<Option<History>>>,
    /// Why the library has no store, once its opening failed (the history
    /// panel says it, US-091).
    pub store_unavailable: Arc<RwLock<Option<String>>>,
    /// The passes that catch up with changes the watcher missed.
    pub reconciler: Reconciler,
    watcher: Arc<Mutex<Option<WorkspaceWatcher>>>,
}

impl WorkspaceRuntime {
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            root: self.root.clone(),
            epoch: self.token.epoch(),
        }
    }

    pub fn library(&self) -> LibraryKey {
        LibraryKey {
            id: self.library_id.clone(),
            root: self.root.clone(),
        }
    }

    /// `true` once the watcher runs (it starts after the first index).
    pub fn is_watching(&self) -> bool {
        self.watcher.lock().is_some()
    }

    /// Asks for a reconciliation pass (see [`Reconciler::request`]).
    pub fn request_reconcile(&self, trigger: ReconcileTrigger) -> bool {
        self.reconciler.request(trigger)
    }

    /// `path` relative to the root, `/`-separated; `None` outside it.
    pub fn relative(&self, path: &Path) -> Option<String> {
        path.starts_with(&self.root)
            .then(|| relative_slash_path(&self.root, path))
    }

    /// Writes the pending save versions of `path` (every note with
    /// `None`) now: a tab closes, the library changes, the app quits. With
    /// `wait`, blocks until they are committed or the time is up.
    pub fn flush_history(&self, path: Option<&Path>, wait: Option<std::time::Duration>) -> bool {
        let Some(history) = self.history.read().clone() else {
            return true;
        };
        let relative = match path {
            Some(path) => match self.relative(path) {
                Some(relative) => Some(relative),
                None => return true,
            },
            None => None,
        };
        match wait {
            Some(timeout) => history.flush_and_wait(relative.as_deref(), timeout),
            None => {
                history.flush(relative.as_deref());
                true
            }
        }
    }

    /// The library closes: its pending versions are written (waited for
    /// up to `timeout`), then a retention pass runs off this thread
    /// (US-090).
    pub fn close_history(&self, timeout: std::time::Duration) -> bool {
        match self.history.read().clone() {
            Some(history) => history.close(timeout),
            None => true,
        }
    }

    /// Lookups by file name for embeds (US-087).
    pub fn names(&self) -> NameIndex {
        NameIndex {
            index: self.index.clone(),
        }
    }
}

/// Finds files by name in a library's live index.
#[derive(Clone)]
pub struct NameIndex {
    index: Arc<Mutex<LiveIndex>>,
}

impl NameIndex {
    /// A name table over `index` alone (tests, tools).
    pub fn from_index(index: LiveIndex) -> Self {
        Self {
            index: Arc::new(Mutex::new(index)),
        }
    }

    /// Changes the index behind the table.
    pub fn update(&self, change: impl FnOnce(&mut LiveIndex)) {
        change(&mut self.index.lock());
    }

    /// `None` while the index does not know every file yet (the caller
    /// walks the folder instead), else the file found, if any.
    pub fn find(&self, file_name: &str) -> Option<Option<PathBuf>> {
        let index = self.index.lock();
        index
            .names_complete()
            .then(|| index.find_by_name(file_name))
    }
}

/// State of one window.
pub struct WindowState {
    id: u64,
    epoch: EpochCounter,
    runtime: RwLock<Option<Arc<WorkspaceRuntime>>>,
    standalone_file: RwLock<Option<PathBuf>>,
    pending_open: Mutex<VecDeque<OpenTarget>>,
    settings: Settings,
}

impl WindowState {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn runtime(&self) -> Option<Arc<WorkspaceRuntime>> {
        self.runtime.read().clone()
    }

    pub fn snapshot(&self) -> Option<WorkspaceSnapshot> {
        let runtime = self.runtime.read();
        runtime.as_ref().map(|runtime| WorkspaceSnapshot {
            root: runtime.root.clone(),
            epoch: self.epoch.current(),
        })
    }

    /// Runs `publish` only if the window still shows the workspace of
    /// `snapshot` (same root and same epoch), holding the state lock so no
    /// transition can interleave. `None` when the result is stale.
    pub fn with_snapshot<T>(
        &self,
        snapshot: &WorkspaceSnapshot,
        publish: impl FnOnce() -> T,
    ) -> Option<T> {
        let runtime = self.runtime.read();
        let current = runtime.as_ref()?;
        if current.root != snapshot.root || self.epoch.current() != snapshot.epoch {
            return None;
        }
        Some(publish())
    }

    /// Replaces the workspace: starts a new epoch (which cancels every job
    /// of the previous one) and installs `runtime_for(token)`.
    fn transition(
        &self,
        build: impl FnOnce(EpochToken) -> Arc<WorkspaceRuntime>,
    ) -> Arc<WorkspaceRuntime> {
        // The library changes: its pending versions are written first,
        // while its epoch is still current (US-089).
        if let Some(old) = self.runtime() {
            old.close_history(HISTORY_FLUSH);
        }
        let mut slot = self.runtime.write();
        self.epoch.advance();
        *self.standalone_file.write() = None;
        let runtime = build(self.epoch.token());
        let old = slot.replace(runtime.clone());
        drop(slot);
        drop_off_thread(old);
        runtime
    }

    /// `true` while a reader such as [`WindowState::with_snapshot`] holds
    /// the workspace lock (no transition can start).
    #[cfg(test)]
    pub(crate) fn workspace_is_locked(&self) -> bool {
        self.runtime.try_write().is_none()
    }

    /// Closes the workspace if it is still `root`.
    pub fn clear_workspace_if_current(&self, root: &Path) -> bool {
        if let Some(old) = self.runtime().filter(|runtime| runtime.root == root) {
            old.close_history(HISTORY_FLUSH);
        }
        let mut slot = self.runtime.write();
        if slot.as_ref().map(|runtime| runtime.root.as_path()) != Some(root) {
            return false;
        }
        self.epoch.advance();
        let old = slot.take();
        drop(slot);
        self.settings.clear_workspace();
        drop_off_thread(old);
        true
    }

    pub fn set_standalone_file(&self, path: Option<PathBuf>) {
        *self.standalone_file.write() = path;
    }

    pub fn standalone_file(&self) -> Option<PathBuf> {
        self.standalone_file.read().clone()
    }

    /// Queues an open request that arrived before the window was ready.
    /// A request equal to the last queued one is dropped.
    pub fn push_pending_open(&self, target: OpenTarget) {
        let mut pending = self.pending_open.lock();
        if pending.back() != Some(&target) {
            pending.push_back(target);
        }
    }

    pub fn pop_pending_open(&self) -> Option<OpenTarget> {
        self.pending_open.lock().pop_front()
    }

    fn has_pending(&self, target: &OpenTarget) -> bool {
        self.pending_open.lock().contains(target)
    }
}

/// Watcher teardown can block (FSEvents, inotify): never on the UI thread.
fn drop_off_thread(runtime: Option<Arc<WorkspaceRuntime>>) {
    let Some(runtime) = runtime else { return };
    runtime.reconciler.stop();
    let watcher = runtime.watcher.lock().take();
    if watcher.is_none() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("cerata-watcher-drop".into())
        .spawn(move || drop(watcher));
    if let Err(error) = spawned {
        log::warn!("dropping a watcher inline: {error}");
    }
}

/// All windows of the process.
#[derive(Default)]
pub struct WindowRegistry {
    next_id: AtomicU64,
    windows: RwLock<HashMap<u64, Arc<WindowState>>>,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a window with its own settings handle.
    pub fn create(&self, settings: Settings) -> Arc<WindowState> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let window = Arc::new(WindowState {
            id,
            epoch: EpochCounter::new(),
            runtime: RwLock::new(None),
            standalone_file: RwLock::new(None),
            pending_open: Mutex::new(VecDeque::new()),
            settings,
        });
        self.windows.write().insert(id, window.clone());
        window
    }

    pub fn get(&self, id: u64) -> Option<Arc<WindowState>> {
        self.windows.read().get(&id).cloned()
    }

    pub fn remove(&self, id: u64) -> Option<Arc<WindowState>> {
        let removed = self.windows.write().remove(&id);
        if let Some(window) = &removed {
            window.epoch.advance();
            drop_off_thread(window.runtime.write().take());
        }
        removed
    }

    /// The window showing (or about to show) workspace `root`.
    pub fn find_by_workspace(&self, root: &Path) -> Option<u64> {
        let target = OpenTarget::Workspace(root.to_path_buf());
        self.windows
            .read()
            .values()
            .find(|window| {
                window
                    .snapshot()
                    .is_some_and(|snapshot| snapshot.root == root)
                    || window.has_pending(&target)
            })
            .map(|window| window.id)
    }

    /// The window showing (or about to show) standalone file `path`.
    pub fn find_by_standalone_file(&self, path: &Path) -> Option<u64> {
        let target = OpenTarget::File(path.to_path_buf());
        self.windows
            .read()
            .values()
            .find(|window| {
                window.standalone_file().as_deref() == Some(path) || window.has_pending(&target)
            })
            .map(|window| window.id)
    }
}

/// Result of [`open_workspace`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub root: PathBuf,
    pub name: String,
    pub epoch: u64,
    /// The library's id; `{root, epoch}` stays the runtime identity.
    pub library_id: String,
}

impl WorkspaceInfo {
    pub fn library(&self) -> LibraryKey {
        LibraryKey {
            id: self.library_id.clone(),
            root: self.root.clone(),
        }
    }
}

/// Canonical form of an existing folder.
pub fn canonicalize_workspace_root(path: &Path) -> Result<PathBuf, FsError> {
    if !path.is_dir() {
        return Err(FsError::NotFound(path.to_path_buf()));
    }
    dunce::canonicalize(path).map_err(|source| FsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// How a library opens beyond its folder.
#[derive(Clone, Default)]
pub struct OpenOptions {
    /// `local_data_dir()/cerata`, where the stores live
    /// ([`library_store_dir`]); `None` opens the library without a store.
    pub store_root: Option<PathBuf>,
    /// How notes are read; the OS by default ([`OsFiles`]).
    pub files: Option<Arc<dyn FileAccess>>,
}

/// [`open_workspace_with`] without a store.
pub fn open_workspace(
    window: &Arc<WindowState>,
    path: &Path,
    library_id: &str,
    events: Sender<WatchEnvelope>,
) -> Result<WorkspaceInfo, FsError> {
    open_workspace_with(window, path, library_id, events, OpenOptions::default())
}

/// Opens `path` as the workspace of `window`, as library `library_id`
/// (resolved by [`crate::library::LibraryRegistry::resolve`]).
///
/// Synchronously: canonicalizes the root, starts a new epoch, installs a
/// bootstrap ignore matcher and the workspace settings layer. In the
/// background (US-083, US-084): opens the library's store, publishes the
/// index it holds (titles included) and reports `IndexComplete` before any
/// walk, then runs the first reconciliation pass, after the first frame
/// when the store had an index; without one, that pass is the walk that
/// publishes the index. The watcher starts after it. Every event goes to
/// `events` tagged with `{root, epoch}`.
pub fn open_workspace_with(
    window: &Arc<WindowState>,
    path: &Path,
    library_id: &str,
    events: Sender<WatchEnvelope>,
    options: OpenOptions,
) -> Result<WorkspaceInfo, FsError> {
    let root = canonicalize_workspace_root(path)?;
    let fs = WorkspaceFs::new(&root)?;
    let runtime = window.transition(|token| {
        Arc::new(WorkspaceRuntime {
            root: root.clone(),
            library_id: library_id.to_owned(),
            token,
            fs,
            index: Arc::new(Mutex::new(LiveIndex::new(&root))),
            ignore: Arc::new(RwLock::new(Arc::new(WorkspaceIgnore::bootstrap(&root)))),
            self_writes: Arc::new(SelfWriteLog::new()),
            index_ready: Arc::new(AtomicBool::new(false)),
            store: Arc::new(RwLock::new(None)),
            history: Arc::new(RwLock::new(None)),
            store_unavailable: Arc::new(RwLock::new(None)),
            reconciler: Reconciler::new(),
            watcher: Arc::new(Mutex::new(None)),
        })
    });
    match window.settings.load_workspace(&root) {
        Ok(warnings) => {
            for warning in warnings {
                log::warn!("{warning}");
            }
        }
        Err(error) => log::warn!("workspace settings not loaded: {error}"),
    }

    let job = runtime.clone();
    let settings = window.settings.clone();
    let spawned = std::thread::Builder::new()
        .name("cerata-index".into())
        .spawn(move || bootstrap(&job, settings, events, options));
    if let Err(error) = spawned {
        log::error!("could not start indexing: {error}");
    }

    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    Ok(WorkspaceInfo {
        root,
        name,
        epoch: runtime.token.epoch(),
        library_id: library_id.to_owned(),
    })
}

/// The background part of [`open_workspace_with`]; it ends when the
/// library closes.
fn bootstrap(
    job: &Arc<WorkspaceRuntime>,
    settings: Settings,
    events: Sender<WatchEnvelope>,
    options: OpenOptions,
) {
    let send = |event: WatchEvent| {
        if job.token.is_current() {
            let _ = events.send(WatchEnvelope {
                root: job.root.clone(),
                epoch: job.token.epoch(),
                event,
            });
        }
    };
    let store = options
        .store_root
        .as_deref()
        .and_then(|store_root| open_store(job, store_root, &send));
    let mut from_store = false;
    if let Some(store) = &store {
        let started = std::time::Instant::now();
        match store.live_notes() {
            Ok(notes) if !notes.is_empty() => {
                let live = index_from_store(&job.root, &notes);
                let file_count = live.len();
                if job.token.is_stale() {
                    return;
                }
                *job.index.lock() = live;
                job.index_ready.store(true, Ordering::SeqCst);
                log::info!(
                    "read {file_count} notes from the store in {:?}",
                    started.elapsed()
                );
                send(WatchEvent::IndexComplete { file_count });
                from_store = true;
            }
            Ok(_) => {}
            Err(error) => log::warn!("library index not read from the store: {error}"),
        }
    }
    let history = store
        .as_ref()
        .zip(options.store_root.as_deref())
        .map(|(store, root)| {
            let events = events.clone();
            let (library_root, token) = (job.root.clone(), job.token.clone());
            History::start(
                store.clone(),
                job.token.clone(),
                Device::load_or_create(root),
                Arc::new(move |message| {
                    if token.is_current() {
                        let _ = events.send(WatchEnvelope {
                            root: library_root.clone(),
                            epoch: token.epoch(),
                            event: WatchEvent::Notice { message },
                        });
                    }
                }),
            )
        });
    *job.store.write() = store.clone();
    *job.history.write() = history.clone();
    job.self_writes
        .set_sink(write_sink(job, store.clone(), history.clone()));
    let files: Arc<dyn FileAccess> = options.files.unwrap_or_else(|| Arc::new(OsFiles));
    let ctx = ReconcileContext {
        root: job.root.clone(),
        token: job.token.clone(),
        index: job.index.clone(),
        ignore: job.ignore.clone(),
        index_ready: job.index_ready.clone(),
        store: store.clone(),
        history: history.clone(),
        self_writes: job.self_writes.clone(),
        files: files.clone(),
        global_gitignore: global_gitignore_path(),
        events: events.clone(),
    };
    let watcher_job = job.clone();
    let start_watcher = Box::new(move || {
        let reload_root = watcher_job.root.clone();
        let reconciler = watcher_job.reconciler.clone();
        let ctx = WatchContext {
            root: watcher_job.root.clone(),
            token: watcher_job.token.clone(),
            index: watcher_job.index.clone(),
            ignore: watcher_job.ignore.clone(),
            self_writes: watcher_job.self_writes.clone(),
            store,
            history,
            files,
            reload_settings: Some(Arc::new(move || {
                if let Err(error) = settings.load_workspace(&reload_root) {
                    log::warn!("workspace settings not reloaded: {error}");
                }
            })),
            on_error: Some(Arc::new(move || {
                reconciler.request(ReconcileTrigger::WatchError);
            })),
            events,
        };
        match WorkspaceWatcher::start(ctx) {
            Ok(watcher) if watcher_job.token.is_current() => {
                *watcher_job.watcher.lock() = Some(watcher);
            }
            Ok(_) => {}
            Err(error) => log::warn!("file watcher not started: {error}"),
        }
    });
    job.reconciler.run(&ctx, from_store, start_watcher);
}

/// Opens the library's store under `store_root`, telling its notice. No
/// store inside the library itself (ADR 0009).
fn open_store(
    job: &WorkspaceRuntime,
    store_root: &Path,
    send: &dyn Fn(WatchEvent),
) -> Option<Store> {
    let dir = library_store_dir(store_root, &job.library_id);
    let unavailable = |reason: String| *job.store_unavailable.write() = Some(reason);
    if dir.starts_with(&job.root) {
        log::warn!(
            "no store for {}: its store folder would be inside it",
            job.root.display()
        );
        unavailable(UNAVAILABLE.into());
        return None;
    }
    if let Some(service) = crate::cloud::synced_by(&dir) {
        unavailable(format!(
            "History is unavailable: Cerata keeps no history in a folder synced by {service}."
        ));
    }
    match Store::open(&dir, &crate::cloud::synced_by) {
        StoreOpen::Ready { store, notice } => {
            if let Some(notice) = notice {
                send(WatchEvent::Notice {
                    message: notice.message(),
                });
            }
            Some(store)
        }
        StoreOpen::Unavailable { notice } => {
            match &notice {
                Some(notice) => unavailable(notice.message()),
                None if job.store_unavailable.read().is_none() => unavailable(UNAVAILABLE.into()),
                None => {}
            }
            if let Some(notice) = notice {
                send(WatchEvent::Notice {
                    message: notice.message(),
                });
            }
            None
        }
    }
}

/// Why a library has no store, when nothing more precise is known.
const UNAVAILABLE: &str = "History is unavailable: this library's store could not be opened.";

/// The live index the store describes: no file is opened.
pub fn index_from_store(root: &Path, notes: &[StoredNote]) -> LiveIndex {
    let mut files = Vec::with_capacity(notes.len());
    for note in notes {
        let path = path_from_relative(root, &note.record.path);
        files.push(IndexedFile {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path,
            relative_path: note.record.path.clone(),
            modified_at: note.record.mtime_ns / 1_000_000_000,
            size: note.record.size,
            modified_ns: note.record.mtime_ns,
            hash: note.record.hash,
            title: note.record.title.clone(),
        });
    }
    LiveIndex::from_files(root, files)
}

/// Cerata's own writes of notes update the index and the store with the
/// content written (US-086): the next pass and the next event know it. A
/// save becomes a version after the typing pause (US-089).
fn write_sink(job: &WorkspaceRuntime, store: Option<Store>, history: Option<History>) -> WriteSink {
    let root = job.root.clone();
    let index = job.index.clone();
    let token = job.token.clone();
    Arc::new(move |path, written, bytes| {
        if !path.starts_with(&root) || !is_markdown_path(path) || token.is_stale() {
            return;
        }
        let content = note_content(bytes);
        let mut file = IndexedFile::new(&root, path, None);
        file.size = written.size;
        file.modified_ns = written.modified_ns;
        file.modified_at = written.modified_ns / 1_000_000_000;
        file.hash = Some(written.hash);
        file.title = content.title;
        let record = note_record(&file, false);
        let relative = file.relative_path.clone();
        index.lock().upsert(file);
        if let Some(store) = &store {
            let _ = store.apply(
                &token,
                vec![NoteChange::Upsert {
                    record,
                    sketch: Some(content.sketch),
                }],
            );
        }
        if let Some(history) = &history {
            history.saved(relative, bytes.to_vec());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    const TEST_LIBRARY: &str = "0123456789abcdef0123456789abcdef";

    fn settings() -> (TempDir, Settings) {
        let dir = TempDir::new().unwrap();
        let (settings, _) = Settings::open(dir.path()).unwrap();
        (dir, settings)
    }

    /// Reads through the disk and counts every file opened.
    #[derive(Default)]
    struct CountingFiles(std::sync::atomic::AtomicUsize);

    impl FileAccess for CountingFiles {
        fn is_offline(&self, _: &Path, _: &std::fs::Metadata) -> bool {
            false
        }

        fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::fs::read(path)
        }
    }

    fn aged(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        file.set_modified(modified - Duration::from_secs(3600))
            .unwrap();
    }

    /// Opens `root` with a store under `stores`, reading through `files`,
    /// and returns the first `IndexComplete` with the runtime.
    fn open_with_store(
        window: &Arc<WindowState>,
        root: &Path,
        stores: &Path,
        files: Arc<CountingFiles>,
    ) -> (Arc<WorkspaceRuntime>, mpsc::Receiver<WatchEnvelope>, usize) {
        let (sender, events) = mpsc::channel();
        let options = OpenOptions {
            store_root: Some(stores.to_path_buf()),
            files: Some(files),
        };
        open_workspace_with(window, root, TEST_LIBRARY, sender, options).unwrap();
        let runtime = window.runtime().unwrap();
        loop {
            let envelope = events.recv_timeout(Duration::from_secs(10)).unwrap();
            if let WatchEvent::IndexComplete { file_count } = envelope.event {
                return (runtime, events, file_count);
            }
        }
    }

    fn note_library() -> TempDir {
        let root = TempDir::new().unwrap();
        for (name, text) in [("a.md", "# Alpha\n"), ("b.md", "# Beta\n")] {
            let path = root.path().join(name);
            std::fs::write(&path, text).unwrap();
            aged(&path);
        }
        root
    }

    /// US-083: the first open fills the store at the end of its walk; the
    /// next one shows every title from the store before any walk, without
    /// opening a single file, and its reconciliation opens none either.
    #[test]
    fn a_reopened_library_shows_its_titles_from_the_store_without_opening_a_file() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();

        let first = Arc::new(CountingFiles::default());
        let (runtime, _events, count) =
            open_with_store(&window, &root, stores.path(), first.clone());
        assert_eq!(count, 2);
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        assert_eq!(
            first.0.load(Ordering::SeqCst),
            2,
            "the walk read each note once"
        );
        let store = runtime.store.read().clone().unwrap();
        assert_eq!(
            store.note("b.md").unwrap().unwrap().record.title.as_deref(),
            Some("Beta")
        );
        assert!(store.path().starts_with(stores.path()));
        assert!(!store.path().starts_with(&root));
        assert!(window.clear_workspace_if_current(&root));

        let again = Arc::new(CountingFiles::default());
        let (runtime, _events, count) =
            open_with_store(&window, &root, stores.path(), again.clone());
        assert_eq!(count, 2);
        // Titles are there with the index, before the first pass.
        assert_eq!(
            runtime.index.lock().title(&root.join("a.md")),
            Some("Alpha")
        );
        assert_eq!(runtime.index.lock().title(&root.join("b.md")), Some("Beta"));
        assert_eq!(runtime.reconciler.passes(), 0);
        assert!(runtime.request_reconcile(ReconcileTrigger::FirstRender));
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        assert_eq!(again.0.load(Ordering::SeqCst), 0, "a file was opened");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !runtime.is_watching() {
            assert!(
                std::time::Instant::now() < deadline,
                "watcher never started"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// US-083 and US-084: a note edited while Cerata was closed shows its
    /// stored title first, then the first reconciliation corrects it.
    #[test]
    fn a_title_changed_while_closed_is_corrected_by_the_first_pass() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files.clone());
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        assert!(window.clear_workspace_if_current(&root));

        let edited = root.join("a.md");
        std::fs::write(&edited, "# Alpha, renamed while closed\n").unwrap();
        aged(&edited);
        let (runtime, events, _) = open_with_store(&window, &root, stores.path(), files);
        assert_eq!(runtime.index.lock().title(&edited), Some("Alpha"));
        runtime.request_reconcile(ReconcileTrigger::FirstRender);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let envelope = events
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap();
            if envelope.event
                == (WatchEvent::FileChanged {
                    path: edited.clone(),
                    change: crate::watcher::FileChange::Modified,
                })
            {
                break;
            }
        }
        assert_eq!(
            runtime.index.lock().title(&edited),
            Some("Alpha, renamed while closed")
        );
        let store = runtime.store.read().clone().unwrap();
        let record = store.note("a.md").unwrap().unwrap().record;
        assert_eq!(record.title.as_deref(), Some("Alpha, renamed while closed"));
        assert_eq!(
            record.hash,
            Some(crate::store::content_hash(
                b"# Alpha, renamed while closed\n"
            ))
        );
    }

    /// US-086: an autosave updates the index and the store with what it
    /// wrote, so the next pass reads nothing.
    #[test]
    fn a_save_records_its_content_in_the_index_and_the_store() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files.clone());
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        let path = root.join("a.md");
        let bytes = b"# Saved by Cerata\n";
        let hash = runtime.self_writes.record(&path, bytes);
        runtime.fs.write_bytes(&path, bytes).unwrap();
        runtime.self_writes.written(&path, hash, bytes);
        assert_eq!(runtime.index.lock().title(&path), Some("Saved by Cerata"));
        let store = runtime.store.read().clone().unwrap();
        store.sync().unwrap();
        let record = store.note("a.md").unwrap().unwrap().record;
        assert_eq!(record.hash, Some(hash));
        assert_eq!(record.title.as_deref(), Some("Saved by Cerata"));
    }

    fn versions(store: &Store, path: &str) -> Vec<(crate::history::Cause, String)> {
        crate::history::versions_of_path(store, path)
            .unwrap()
            .map(|(_, versions)| versions)
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                let bytes = crate::history::content(store, &v.hash).unwrap().unwrap();
                (v.cause, String::from_utf8(bytes).unwrap())
            })
            .collect()
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// US-089: a write of Cerata's becomes a `save` version, at once when
    /// the library changes rather than after the typing pause, and the
    /// device that wrote it is named.
    #[test]
    fn a_save_becomes_a_version_when_the_library_changes() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let other = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files.clone());
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        let store = runtime.store.read().clone().unwrap();
        let path = root.join("a.md");
        let bytes = b"# Typed in Cerata\n";
        let hash = runtime.self_writes.record(&path, bytes);
        runtime.fs.write_bytes(&path, bytes).unwrap();
        runtime.self_writes.written(&path, hash, bytes);
        // Within the 30 s pause: no version yet.
        store.sync().unwrap();
        assert_eq!(versions(&store, "a.md"), []);
        let other_root = dunce::canonicalize(other.path()).unwrap();
        let (_, _, _) = open_with_store(&window, &other_root, stores.path(), files);
        assert_eq!(
            versions(&store, "a.md"),
            [(
                crate::history::Cause::Save,
                "# Typed in Cerata\n".to_owned()
            )]
        );
        let device = crate::history::versions_of_path(&store, "a.md")
            .unwrap()
            .unwrap()
            .1[0]
            .device_name
            .clone();
        assert_eq!(device, crate::history::device::host_name());
        assert!(
            stores
                .path()
                .join(crate::history::device::DEVICE_ID_FILE)
                .is_file()
        );
    }

    /// US-089: a change made outside Cerata, found by a pass, is an
    /// `external` version.
    #[test]
    fn an_outside_change_found_by_a_pass_is_a_version() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files);
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        let store = runtime.store.read().clone().unwrap();
        // Opening and indexing a library records nothing.
        assert_eq!(versions(&store, "a.md"), []);
        let path = root.join("b.md");
        std::fs::write(&path, "# Beta, edited by an agent\n").unwrap();
        aged(&path);
        assert!(runtime.request_reconcile(ReconcileTrigger::Focus));
        wait_until("an external version", || {
            !versions(&store, "b.md").is_empty()
        });
        assert_eq!(
            versions(&store, "b.md"),
            [(
                crate::history::Cause::External,
                "# Beta, edited by an agent\n".to_owned()
            )]
        );
    }

    /// US-093: a sync conflict copy found beside its original is a
    /// `conflict` version of the original, whose parent is the original's
    /// last version before the copy's modified time; the original's head
    /// stays its own text.
    #[test]
    fn a_conflict_copy_is_a_conflict_version_of_its_original() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files);
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        let store = runtime.store.read().clone().unwrap();
        let history = runtime.history.read().clone().unwrap();
        let now = crate::history::unix_now();
        for (text, ts) in [
            ("# Alpha, before the copy\n", now - 7200),
            ("# Alpha, after\n", now),
        ] {
            let mut request = crate::history::Request::new(
                "a.md".to_owned(),
                text.as_bytes().to_vec(),
                crate::history::Cause::Save,
            );
            request.ts = ts;
            history.record(request);
        }
        assert!(history.flush_and_wait(None, Duration::from_secs(5)));
        // The copy was written an hour ago, between the two versions.
        let copy = root.join("a.sync-conflict-20261103-101010-ABCDEFG.md");
        std::fs::write(&copy, "# Alpha, from the other device\n").unwrap();
        aged(&copy);
        assert!(runtime.request_reconcile(ReconcileTrigger::Focus));
        wait_until("the conflict version", || {
            versions(&store, "a.md").len() == 3
        });
        let (_, listed) = crate::history::versions_of_path(&store, "a.md")
            .unwrap()
            .unwrap();
        let conflict = &listed[0];
        assert_eq!(conflict.cause, crate::history::Cause::Conflict);
        assert_eq!(
            crate::history::content(&store, &conflict.hash)
                .unwrap()
                .unwrap(),
            b"# Alpha, from the other device\n"
        );
        assert_eq!(
            conflict.parent,
            Some(listed[2].hash),
            "the version before the copy"
        );
        // The copy stays a note of its own until it is reviewed.
        assert_eq!(
            versions(&store, "a.sync-conflict-20261103-101010-ABCDEFG.md"),
            []
        );
    }

    /// US-093: a copy whose original is missing is an ordinary note.
    #[test]
    fn a_conflict_copy_without_its_original_is_an_ordinary_note() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files);
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        let store = runtime.store.read().clone().unwrap();
        let name = "gone.sync-conflict-20261103-101010-ABCDEFG.md";
        std::fs::write(root.join(name), "# Orphan copy\n").unwrap();
        aged(&root.join(name));
        assert!(runtime.request_reconcile(ReconcileTrigger::Focus));
        wait_until("the copy as a note", || !versions(&store, name).is_empty());
        assert_eq!(
            versions(&store, name),
            [(
                crate::history::Cause::External,
                "# Orphan copy\n".to_owned()
            )]
        );
        assert_eq!(versions(&store, "gone.md"), []);
    }

    /// US-089: a crash between the write of a note and its version leaves
    /// the index ahead of the history; reopening the library records what
    /// the disk holds as an `external` version, and no note is lost.
    #[test]
    fn a_crash_between_a_write_and_its_version_is_caught_up_on_reopen() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let stores = TempDir::new().unwrap();
        let library = note_library();
        let root = dunce::canonicalize(library.path()).unwrap();
        let files = Arc::new(CountingFiles::default());
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files.clone());
        assert!(
            runtime
                .reconciler
                .wait_for_passes(1, Duration::from_secs(10))
        );
        let path = root.join("a.md");
        let first = b"# First, versioned\n";
        let hash = runtime.self_writes.record(&path, first);
        runtime.fs.write_bytes(&path, first).unwrap();
        runtime.self_writes.written(&path, hash, first);
        assert!(runtime.flush_history(None, Some(Duration::from_secs(5))));
        let store = runtime.store.read().clone().unwrap();
        // The crash: the `.md` and the index row are written, the version
        // never is.
        let second = b"# Second, written just before the crash\n";
        std::fs::write(&path, second).unwrap();
        aged(&path);
        let mut file = IndexedFile::new(&root, &path, std::fs::metadata(&path).ok().as_ref());
        file.hash = Some(crate::store::content_hash(second));
        store
            .apply(
                &runtime.token,
                vec![NoteChange::Upsert {
                    record: note_record(&file, false),
                    sketch: None,
                }],
            )
            .recv()
            .unwrap()
            .unwrap();
        assert!(window.clear_workspace_if_current(&root));
        drop(runtime);
        let (runtime, _events, _) = open_with_store(&window, &root, stores.path(), files);
        // A library opened from its store tells its index before its store.
        wait_until("the store", || runtime.store.read().is_some());
        let store = runtime.store.read().clone().unwrap();
        wait_until("the caught-up version", || {
            versions(&store, "a.md").len() == 2
        });
        assert_eq!(
            versions(&store, "a.md"),
            [
                (
                    crate::history::Cause::External,
                    "# Second, written just before the crash\n".to_owned()
                ),
                (
                    crate::history::Cause::Save,
                    "# First, versioned\n".to_owned()
                ),
            ]
        );
        assert_eq!(std::fs::read(&path).unwrap(), second);
    }

    // Ported from Writer `commands/workspace.rs` tests.

    #[test]
    fn canonicalize_rejects_missing_path() {
        let dir = TempDir::new().unwrap();
        assert!(matches!(
            canonicalize_workspace_root(&dir.path().join("missing")),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn canonicalize_rejects_non_directory() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.md");
        std::fs::write(&file, "").unwrap();
        assert!(matches!(
            canonicalize_workspace_root(&file),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn canonicalize_round_trips_existing_directory() {
        let dir = TempDir::new().unwrap();
        let canonical = canonicalize_workspace_root(dir.path()).unwrap();
        assert_eq!(canonicalize_workspace_root(&canonical).unwrap(), canonical);
    }

    // Ported from Writer `state.rs` tests.

    #[test]
    fn find_by_workspace_matches_pending_open_and_current_root() {
        let (_config, settings) = settings();
        let registry = WindowRegistry::new();
        let window = registry.create(settings.clone());
        let pending = PathBuf::from("/tmp/pending");
        window.push_pending_open(OpenTarget::Workspace(pending.clone()));
        assert_eq!(registry.find_by_workspace(&pending), Some(window.id()));

        let other = registry.create(settings);
        let root = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        let info = open_workspace(&other, root.path(), TEST_LIBRARY, sender).unwrap();
        assert_eq!(registry.find_by_workspace(&info.root), Some(other.id()));
        assert_eq!(registry.find_by_workspace(Path::new("/elsewhere")), None);
    }

    #[test]
    fn find_by_standalone_file_matches_current_and_pending() {
        let (_config, settings) = settings();
        let registry = WindowRegistry::new();
        let window = registry.create(settings);
        let file = PathBuf::from("/tmp/a.md");
        window.set_standalone_file(Some(file.clone()));
        assert_eq!(registry.find_by_standalone_file(&file), Some(window.id()));
        window.set_standalone_file(None);
        window.push_pending_open(OpenTarget::File(file.clone()));
        assert_eq!(registry.find_by_standalone_file(&file), Some(window.id()));
    }

    #[test]
    fn pending_opens_are_fifo_and_drop_consecutive_duplicates() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let a = OpenTarget::Workspace(PathBuf::from("/a"));
        let b = OpenTarget::File(PathBuf::from("/b.md"));
        window.push_pending_open(a.clone());
        window.push_pending_open(a.clone());
        window.push_pending_open(b.clone());
        assert_eq!(window.pop_pending_open(), Some(a));
        assert_eq!(window.pop_pending_open(), Some(b));
        assert_eq!(window.pop_pending_open(), None);
    }

    #[test]
    fn stale_results_are_rejected_including_aba() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        open_workspace(&window, a.path(), TEST_LIBRARY, sender.clone()).unwrap();
        let captured = window.snapshot().unwrap();
        assert_eq!(
            window.with_snapshot(&captured, || "published"),
            Some("published")
        );

        open_workspace(&window, b.path(), TEST_LIBRARY, sender.clone()).unwrap();
        assert_eq!(window.with_snapshot(&captured, || ()), None);
        // Back to A: same root, new epoch. A result captured during the first
        // visit must still be rejected.
        open_workspace(&window, a.path(), TEST_LIBRARY, sender).unwrap();
        assert_eq!(window.snapshot().unwrap().root, captured.root);
        assert_eq!(window.with_snapshot(&captured, || ()), None);
    }

    // Writer `state.rs` guarded_workspace_snapshot_rejects_stale_callbacks_and_holds_read_guard.
    #[test]
    fn a_publish_holds_the_state_lock_so_no_transition_interleaves() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let a = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        open_workspace(&window, a.path(), TEST_LIBRARY, sender).unwrap();
        let captured = window.snapshot().unwrap();
        let published = window.with_snapshot(&captured, || window.runtime.try_write().is_none());
        assert_eq!(published, Some(true));
    }

    // Writer `commands/fs.rs` create_sidebar_entry_rejects_stale_workspace_before_mutation.
    #[test]
    fn a_stale_snapshot_never_reaches_the_file_system() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings);
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        open_workspace(&window, a.path(), TEST_LIBRARY, sender.clone()).unwrap();
        let captured = window.snapshot().unwrap();
        let stale = window.runtime().unwrap();
        open_workspace(&window, b.path(), TEST_LIBRARY, sender).unwrap();
        let created = window.with_snapshot(&captured, || {
            stale
                .fs
                .create_untitled(&stale.root, crate::fs::EntryKind::File)
        });
        assert!(created.is_none());
        assert_eq!(std::fs::read_dir(a.path()).unwrap().count(), 0);
    }

    // Writer `state.rs` close_workspace_invalidates_the_snapshot_and_rejects_stale_roots.
    #[test]
    fn clear_workspace_only_when_still_current() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings.clone());
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        let info = open_workspace(&window, a.path(), TEST_LIBRARY, sender.clone()).unwrap();
        let captured = window.snapshot().unwrap();
        let closed = window.runtime().unwrap();
        assert!(!window.clear_workspace_if_current(Path::new("/not/it")));
        assert!(window.clear_workspace_if_current(&info.root));
        assert!(window.snapshot().is_none());
        assert!(window.runtime().is_none());
        assert!(closed.token.is_stale(), "jobs of the closed workspace stop");
        assert!(settings.workspace_path().is_none());
        assert_eq!(window.with_snapshot(&captured, || ()), None);

        let other = open_workspace(&window, b.path(), TEST_LIBRARY, sender).unwrap();
        assert!(!window.clear_workspace_if_current(&info.root));
        assert_eq!(window.snapshot().map(|s| s.root), Some(other.root));
    }

    // Writer `commands/workspace.rs` canonicalize_resolves_var_alias_on_macos.
    #[cfg(target_os = "macos")]
    #[test]
    fn canonicalize_resolves_var_alias_on_macos() {
        let dir = TempDir::new().unwrap();
        let canonical = canonicalize_workspace_root(dir.path()).unwrap();
        let raw = canonical.to_string_lossy().into_owned();
        // `/var` is an alias of `/private/var`: both spellings must give the
        // same root, or the sidebar and the index disagree on every path.
        if let Some(aliased) = raw.strip_prefix("/private") {
            assert!(Path::new(aliased).exists());
            assert_eq!(
                canonicalize_workspace_root(Path::new(aliased)).unwrap(),
                canonical
            );
        }
    }

    #[test]
    fn open_workspace_indexes_in_the_background_then_watches() {
        let (_config, settings) = settings();
        let window = WindowRegistry::new().create(settings.clone());
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("a.md"), "# A").unwrap();
        std::fs::create_dir(root.path().join(".cerata")).unwrap();
        std::fs::write(
            root.path().join(".cerata").join("config"),
            "editor.font-size = 20\n",
        )
        .unwrap();
        let (sender, events) = mpsc::channel();
        let info = open_workspace(&window, root.path(), TEST_LIBRARY, sender).unwrap();
        // The workspace layer is installed synchronously.
        assert_eq!(
            settings.get("editor.font-size").and_then(|v| v.as_f64()),
            Some(20.0)
        );

        let complete = events.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(complete.is_for(&info.root, info.epoch));
        assert_eq!(complete.event, WatchEvent::IndexComplete { file_count: 1 });
        let runtime = window.runtime().unwrap();
        assert!(runtime.index_ready.load(Ordering::SeqCst));
        assert!(runtime.index.lock().contains(&info.root.join("a.md")));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !runtime.is_watching() {
            assert!(
                std::time::Instant::now() < deadline,
                "watcher never started"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(info.root.join("b.md"), "# B").unwrap();
        let created = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(created.event, WatchEvent::FileChanged { .. }),
            "{created:?}"
        );

        // The settings file is watched too.
        std::fs::write(
            info.root.join(".cerata").join("config"),
            "editor.font-size = 22\n",
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while settings.get("editor.font-size").and_then(|v| v.as_f64()) != Some(22.0) {
            assert!(
                std::time::Instant::now() < deadline,
                "settings never reloaded"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
