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
use crate::ignore::WorkspaceIgnore;
use crate::index::{IndexReport, LiveIndex, spawn_index};
use crate::open_target::OpenTarget;
use crate::self_writes::SelfWriteLog;
use crate::settings::Settings;
use crate::watcher::{WatchContext, WatchEnvelope, WatchEvent, WorkspaceWatcher};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

/// `{root, epoch}` of a window's workspace at some instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub root: PathBuf,
    pub epoch: u64,
}

/// Everything that lives exactly as long as one open workspace.
pub struct WorkspaceRuntime {
    pub root: PathBuf,
    pub token: EpochToken,
    pub fs: WorkspaceFs,
    pub index: Arc<Mutex<LiveIndex>>,
    pub ignore: Arc<RwLock<Arc<WorkspaceIgnore>>>,
    pub self_writes: Arc<SelfWriteLog>,
    pub index_ready: Arc<AtomicBool>,
    watcher: Arc<Mutex<Option<WorkspaceWatcher>>>,
}

impl WorkspaceRuntime {
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            root: self.root.clone(),
            epoch: self.token.epoch(),
        }
    }

    /// `true` once the watcher runs (it starts after the first index).
    pub fn is_watching(&self) -> bool {
        self.watcher.lock().is_some()
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
        let mut slot = self.runtime.write();
        self.epoch.advance();
        *self.standalone_file.write() = None;
        let runtime = build(self.epoch.token());
        let old = slot.replace(runtime.clone());
        drop(slot);
        drop_off_thread(old);
        runtime
    }

    /// Closes the workspace if it is still `root`.
    pub fn clear_workspace_if_current(&self, root: &Path) -> bool {
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

/// Opens `path` as the workspace of `window`.
///
/// Synchronously: canonicalizes the root, starts a new epoch, installs a
/// bootstrap ignore matcher and the workspace settings layer. In the
/// background: indexes the workspace, publishes the index if the epoch is
/// unchanged, reports `IndexComplete`, then starts the watcher, whose events
/// (and the index report) go to `events` tagged with `{root, epoch}`.
pub fn open_workspace(
    window: &Arc<WindowState>,
    path: &Path,
    events: Sender<WatchEnvelope>,
) -> Result<WorkspaceInfo, FsError> {
    let root = canonicalize_workspace_root(path)?;
    let fs = WorkspaceFs::new(&root)?;
    let runtime = window.transition(|token| {
        Arc::new(WorkspaceRuntime {
            root: root.clone(),
            token,
            fs,
            index: Arc::new(Mutex::new(LiveIndex::new(&root))),
            ignore: Arc::new(RwLock::new(Arc::new(WorkspaceIgnore::bootstrap(&root)))),
            self_writes: Arc::new(SelfWriteLog::new()),
            index_ready: Arc::new(AtomicBool::new(false)),
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
    let spawned = spawn_index(root.clone(), runtime.token.clone(), move |report| {
        let IndexReport::Completed {
            index,
            file_count,
            duration,
            ..
        } = report
        else {
            return;
        };
        if job.token.is_stale() {
            return;
        }
        log::info!("indexed {file_count} files in {duration:?}");
        *job.ignore.write() = index.ignore.clone();
        *job.index.lock() = LiveIndex::from_index(&index);
        job.index_ready.store(true, Ordering::SeqCst);
        let _ = events.send(WatchEnvelope {
            root: job.root.clone(),
            epoch: job.token.epoch(),
            event: WatchEvent::IndexComplete { file_count },
        });
        let reload_root = job.root.clone();
        let ctx = WatchContext {
            root: job.root.clone(),
            token: job.token.clone(),
            index: job.index.clone(),
            ignore: job.ignore.clone(),
            self_writes: job.self_writes.clone(),
            reload_settings: Some(Arc::new(move || {
                if let Err(error) = settings.load_workspace(&reload_root) {
                    log::warn!("workspace settings not reloaded: {error}");
                }
            })),
            events,
        };
        match WorkspaceWatcher::start(ctx) {
            Ok(watcher) if job.token.is_current() => *job.watcher.lock() = Some(watcher),
            Ok(_) => {}
            Err(error) => log::warn!("file watcher not started: {error}"),
        }
    });
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    fn settings() -> (TempDir, Settings) {
        let dir = TempDir::new().unwrap();
        let (settings, _) = Settings::open(dir.path()).unwrap();
        (dir, settings)
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
        let info = open_workspace(&other, root.path(), sender).unwrap();
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
        open_workspace(&window, a.path(), sender.clone()).unwrap();
        let captured = window.snapshot().unwrap();
        assert_eq!(
            window.with_snapshot(&captured, || "published"),
            Some("published")
        );

        open_workspace(&window, b.path(), sender.clone()).unwrap();
        assert_eq!(window.with_snapshot(&captured, || ()), None);
        // Back to A: same root, new epoch. A result captured during the first
        // visit must still be rejected.
        open_workspace(&window, a.path(), sender).unwrap();
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
        open_workspace(&window, a.path(), sender).unwrap();
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
        open_workspace(&window, a.path(), sender.clone()).unwrap();
        let captured = window.snapshot().unwrap();
        let stale = window.runtime().unwrap();
        open_workspace(&window, b.path(), sender).unwrap();
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
        let info = open_workspace(&window, a.path(), sender.clone()).unwrap();
        let captured = window.snapshot().unwrap();
        let closed = window.runtime().unwrap();
        assert!(!window.clear_workspace_if_current(Path::new("/not/it")));
        assert!(window.clear_workspace_if_current(&info.root));
        assert!(window.snapshot().is_none());
        assert!(window.runtime().is_none());
        assert!(closed.token.is_stale(), "jobs of the closed workspace stop");
        assert!(settings.workspace_path().is_none());
        assert_eq!(window.with_snapshot(&captured, || ()), None);

        let other = open_workspace(&window, b.path(), sender).unwrap();
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
        let info = open_workspace(&window, root.path(), sender).unwrap();
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
