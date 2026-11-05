use crate::epoch::EpochToken;
use crate::history::{Cause, History, Request, conflict_version};
use crate::ignore::{WorkspaceIgnore, is_gitignore_path};
use crate::index::{IndexedFile, LiveIndex, index_subtree, index_workspace};
use crate::markdown::is_markdown_path;
use crate::paths::workspace_config_path;
use crate::reconcile::{Appeared, FileAccess, Gone, match_renames, note_content, note_record};
use crate::self_writes::SelfWriteLog;
use crate::store::{NoteChange, Store};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub const BATCH_WINDOW: Duration = Duration::from_millis(300);

pub const ROOT_CHECK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    FileChanged { path: PathBuf, change: FileChange },
    DirectoryChanged { path: PathBuf },
    SettingsChanged,
    IgnoreRebuilt,
    RootRemoved,
    IndexComplete { file_count: usize },
    WatchLimited { message: String },
    Renamed { from: PathBuf, to: PathBuf },
    Notice { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Created,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEnvelope {
    pub root: PathBuf,
    pub epoch: u64,
    pub event: WatchEvent,
}

impl WatchEnvelope {
    pub fn is_for(&self, root: &Path, epoch: u64) -> bool {
        self.root == root && self.epoch == epoch
    }
}

#[derive(Clone)]
pub struct WatchContext {
    pub root: PathBuf,
    pub token: EpochToken,
    pub index: Arc<Mutex<LiveIndex>>,
    pub ignore: Arc<RwLock<Arc<WorkspaceIgnore>>>,
    pub self_writes: Arc<SelfWriteLog>,
    pub store: Option<Store>,
    pub history: Option<History>,
    pub files: Arc<dyn FileAccess>,
    pub reload_settings: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_error: Option<Arc<dyn Fn() + Send + Sync>>,
    pub events: Sender<WatchEnvelope>,
}

impl WatchContext {
    fn lost_events(&self) {
        if let Some(on_error) = &self.on_error {
            on_error();
        }
    }

    fn send(&self, event: WatchEvent) {
        if self.token.is_stale() {
            return;
        }
        let _ = self.events.send(WatchEnvelope {
            root: self.root.clone(),
            epoch: self.token.epoch(),
            event,
        });
    }
}

pub trait WatchBackend: Send {
    fn watch(&mut self, path: &Path, recursive: bool) -> notify::Result<()>;
    fn unwatch(&mut self, path: &Path) -> notify::Result<()>;
}

impl<W: Watcher + Send> WatchBackend for W {
    fn watch(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        Watcher::watch(self, path, mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::unwatch(self, path)
    }
}

pub struct WorkspaceWatcher {
    backend: Mutex<Box<dyn WatchBackend>>,
    limited: bool,
    watched_dirs: Mutex<HashSet<PathBuf>>,
}

impl WorkspaceWatcher {
    pub fn start(ctx: WatchContext) -> notify::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let backend = notify::recommended_watcher(move |result| {
            let _ = sender.send(result);
        })?;
        Self::start_with_backend(ctx, Box::new(backend), receiver)
    }

    pub fn start_with_backend(
        ctx: WatchContext,
        mut backend: Box<dyn WatchBackend>,
        receiver: Receiver<notify::Result<Event>>,
    ) -> notify::Result<Self> {
        let limited = match backend.watch(&ctx.root, true) {
            Ok(()) => false,
            Err(error) if is_watch_limit(&error) => {
                let _ = backend.unwatch(&ctx.root);
                log::warn!("file watch limit reached: {error}");
                ctx.send(WatchEvent::WatchLimited {
                    message: watch_limit_message(),
                });
                ctx.lost_events();
                true
            }
            Err(error) => return Err(error),
        };
        std::thread::Builder::new()
            .name("cerata-watcher".into())
            .spawn(move || run(&ctx, &receiver))
            .map_err(notify::Error::io)?;
        Ok(Self {
            backend: Mutex::new(backend),
            limited,
            watched_dirs: Mutex::new(HashSet::new()),
        })
    }

    pub fn is_limited(&self) -> bool {
        self.limited
    }

    pub fn set_open_files(&self, open_files: &[PathBuf]) {
        if !self.limited {
            return;
        }
        let wanted: HashSet<PathBuf> = open_files
            .iter()
            .filter_map(|file| file.parent().map(Path::to_path_buf))
            .collect();
        let mut watched = self.watched_dirs.lock();
        let mut backend = self.backend.lock();
        for dir in watched.difference(&wanted) {
            let _ = backend.unwatch(dir);
        }
        for dir in wanted.difference(&watched) {
            if let Err(error) = backend.watch(dir, false) {
                log::warn!("cannot watch {}: {error}", dir.display());
            }
        }
        *watched = wanted;
    }
}

pub fn is_watch_limit(error: &notify::Error) -> bool {
    match &error.kind {
        notify::ErrorKind::MaxFilesWatch => true,
        notify::ErrorKind::Io(io) => io.raw_os_error() == Some(28) && cfg!(target_os = "linux"),
        _ => false,
    }
}

pub fn watch_limit_message() -> String {
    "This workspace has more folders than Linux allows Cerata to watch, so \
     only the folders of open files are watched. To watch everything, raise \
     the limit: sudo sysctl fs.inotify.max_user_watches=524288 (add \
     fs.inotify.max_user_watches=524288 to /etc/sysctl.d/90-cerata.conf to \
     keep it after a reboot)."
        .to_owned()
}

pub(crate) fn collect_batch<T>(
    first: T,
    receiver: &Receiver<T>,
    window: Duration,
) -> (Vec<T>, bool) {
    let deadline = Instant::now() + window;
    let mut batch = vec![first];
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return (batch, true);
        };
        match receiver.recv_timeout(left) {
            Ok(item) => batch.push(item),
            Err(RecvTimeoutError::Timeout) => return (batch, true),
            Err(RecvTimeoutError::Disconnected) => return (batch, false),
        }
    }
}

fn run(ctx: &WatchContext, receiver: &Receiver<notify::Result<Event>>) {
    loop {
        let first = match receiver.recv_timeout(ROOT_CHECK_INTERVAL) {
            Ok(first) => first,
            Err(RecvTimeoutError::Timeout) => {
                if ctx.token.is_stale() {
                    return;
                }
                if !ctx.root.exists() {
                    ctx.send(WatchEvent::RootRemoved);
                    return;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let (batch, connected) = collect_batch(first, receiver, BATCH_WINDOW);
        if ctx.token.is_stale() {
            return;
        }
        if !process_batch(ctx, batch) || !connected {
            return;
        }
    }
}

#[derive(Default)]
struct PathChange {
    membership: bool,
    folder_hint: bool,
}

fn process_batch(ctx: &WatchContext, batch: Vec<notify::Result<Event>>) -> bool {
    let mut order: Vec<PathBuf> = Vec::new();
    let mut changes: std::collections::HashMap<PathBuf, PathChange> = Default::default();
    let mut lost = false;
    for result in batch {
        let event = match result {
            Ok(event) => event,
            Err(error) => {
                log::warn!("watch error: {error}");
                lost = true;
                continue;
            }
        };
        let membership = matches!(
            event.kind,
            EventKind::Create(_)
                | EventKind::Remove(_)
                | EventKind::Modify(notify::event::ModifyKind::Name(_))
        );
        let folder_hint = matches!(
            event.kind,
            EventKind::Create(notify::event::CreateKind::Folder)
                | EventKind::Remove(notify::event::RemoveKind::Folder)
        );
        if matches!(event.kind, EventKind::Access(_)) {
            continue;
        }
        for path in event.paths {
            let entry = changes.entry(path.clone()).or_insert_with(|| {
                order.push(path.clone());
                PathChange::default()
            });
            entry.membership |= membership;
            entry.folder_hint |= folder_hint;
        }
    }

    if !ctx.root.exists() {
        ctx.send(WatchEvent::RootRemoved);
        return false;
    }

    let ignore = ctx.ignore.read().clone();
    let config_path = workspace_config_path(&ctx.root);
    let mut dirs_changed: Vec<PathBuf> = Vec::new();
    let mut rebuild_ignore = false;
    let mut settings_changed = false;
    let mut notes = NoteEvents::default();

    for path in order {
        let Some(change) = changes.get(&path) else {
            continue;
        };
        if path == config_path {
            settings_changed = true;
            continue;
        }
        if is_gitignore_path(&path) {
            let parent_visible = path
                .parent()
                .is_some_and(|parent| parent == ctx.root || !ignore.excludes(parent, true));
            rebuild_ignore |= parent_visible;
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).ok();
        let exists = metadata.is_some();
        let is_dir = metadata
            .as_ref()
            .map_or(change.folder_hint, |meta| meta.is_dir());
        if !path.starts_with(&ctx.root) || ignore.excludes(&path, is_dir) {
            continue;
        }
        let parent = path.parent().map(Path::to_path_buf);

        if exists && is_dir {
            if change.membership {
                let (found, others) = index_subtree(&ctx.root, &path, &ignore);
                for path in others {
                    ctx.index.lock().add_other(&path);
                }
                for file in found {
                    if !ctx.index.lock().contains(&file.path) {
                        notes.seen(ctx, file, None);
                    }
                }
                dirs_changed.extend(parent);
            }
            dirs_changed.push(path);
        } else if exists && is_markdown_path(&path) {
            let file = IndexedFile::new(&ctx.root, &path, metadata.as_ref());
            if notes.seen(ctx, file, metadata.as_ref()) || change.membership {
                dirs_changed.extend(parent);
            }
        } else if exists {
            if change.membership {
                ctx.index.lock().add_other(&path);
                dirs_changed.extend(parent);
            }
        } else {
            let removed = ctx.index.lock().remove_subtree(&path);
            for file in &removed {
                notes.gone(file);
            }
            if change.membership || !removed.is_empty() {
                dirs_changed.extend(parent);
            }
        }
    }
    notes.finish(ctx);

    let mut seen = HashSet::new();
    for dir in dirs_changed {
        if seen.insert(dir.clone()) {
            ctx.send(WatchEvent::DirectoryChanged { path: dir });
        }
    }
    if settings_changed {
        if let Some(reload) = &ctx.reload_settings {
            reload();
        }
        ctx.send(WatchEvent::SettingsChanged);
    }
    if rebuild_ignore {
        spawn_rebuild(ctx.clone());
    }
    if lost {
        ctx.lost_events();
    }
    true
}

#[derive(Default)]
struct NoteEvents {
    gone: Vec<Gone>,
    appeared: Vec<Appeared>,
    appeared_bytes: Vec<Option<Vec<u8>>>,
    versions: Vec<Request>,
    unread: Vec<PathBuf>,
    changes: Vec<NoteChange>,
}

impl NoteEvents {
    fn seen(
        &mut self,
        ctx: &WatchContext,
        mut file: IndexedFile,
        metadata: Option<&std::fs::Metadata>,
    ) -> bool {
        let previous = ctx.index.lock().get(&file.path).cloned();
        let added = previous.is_none();
        let offline = metadata.is_some_and(|metadata| ctx.files.is_offline(&file.path, metadata));
        let read = if offline {
            None
        } else {
            match ctx.files.read(&file.path) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    log::warn!("cannot read {}: {error}", file.path.display());
                    None
                }
            }
        };
        let content = read.as_deref().map(note_content);
        let Some(content) = content else {
            if let Some(previous) = &previous {
                file.hash = previous.hash;
                file.title = previous.title.clone();
            }
            self.changes.push(NoteChange::Upsert {
                record: note_record(&file, offline),
                sketch: None,
            });
            let path = file.path.clone();
            ctx.index.lock().upsert(file);
            if added {
                self.unread.push(path);
            } else if !offline {
                ctx.send(WatchEvent::FileChanged {
                    path,
                    change: FileChange::Modified,
                });
            }
            return added;
        };
        let known = previous.as_ref().and_then(|previous| previous.hash) == Some(content.hash);
        let echo = ctx.self_writes.matches(&file.path, &content.hash);
        if !known && !echo {
            ctx.self_writes.superseded(&file.path);
        }
        file.hash = Some(content.hash);
        file.title = content.title.clone();
        self.changes.push(NoteChange::Upsert {
            record: note_record(&file, false),
            sketch: (!known).then(|| content.sketch.clone()),
        });
        let path = file.path.clone();
        let relative_path = file.relative_path.clone();
        ctx.index.lock().upsert(file);
        if added {
            self.appeared.push(Appeared {
                path,
                relative_path,
                hash: content.hash,
                sketch: content.sketch,
            });
            self.appeared_bytes.push(read.filter(|_| !echo));
        } else if !known && !echo {
            if let Some(bytes) = read {
                self.versions
                    .push(Request::new(relative_path, bytes, Cause::External));
            }
            ctx.send(WatchEvent::FileChanged {
                path,
                change: FileChange::Modified,
            });
        }
        added
    }

    fn gone(&mut self, file: &IndexedFile) {
        self.gone.push(Gone {
            path: file.path.clone(),
            relative_path: file.relative_path.clone(),
            hash: file.hash,
        });
    }

    fn finish(mut self, ctx: &WatchContext) {
        let matches = if self.gone.is_empty() || self.appeared.is_empty() {
            Default::default()
        } else {
            let mut wanted: Vec<String> = self
                .gone
                .iter()
                .map(|gone| gone.relative_path.clone())
                .collect();
            wanted.sort();
            wanted.truncate(100);
            let sketches = ctx
                .store
                .as_ref()
                .and_then(|store| store.sketches(&wanted).ok())
                .unwrap_or_default();
            match_renames(&self.gone, &self.appeared, &sketches)
        };
        let mut renamed_gone = std::collections::HashSet::new();
        let mut renamed_appeared = std::collections::HashSet::new();
        for (g, a) in &matches.pairs {
            renamed_gone.insert(*g);
            renamed_appeared.insert(*a);
            let (from, to) = (&self.gone[*g], &self.appeared[*a]);
            if from.hash != Some(to.hash)
                && let Some(bytes) = self.appeared_bytes.get_mut(*a).and_then(Option::take)
            {
                self.versions
                    .push(Request::new(to.relative_path.clone(), bytes, Cause::Rename));
            }
            if let Some(file) = ctx.index.lock().get(&to.path).cloned() {
                self.changes.push(NoteChange::Rename {
                    from: from.relative_path.clone(),
                    record: note_record(&file, false),
                    sketch: Some(to.sketch.clone()),
                });
            }
            if !ctx.self_writes.is_own_move(&from.path, &to.path) {
                ctx.send(WatchEvent::Renamed {
                    from: from.path.clone(),
                    to: to.path.clone(),
                });
            }
        }
        for (g, gone) in self.gone.iter().enumerate() {
            if renamed_gone.contains(&g) {
                continue;
            }
            self.changes.push(NoteChange::Delete {
                path: gone.relative_path.clone(),
            });
            ctx.send(WatchEvent::FileChanged {
                path: gone.path.clone(),
                change: FileChange::Removed,
            });
        }
        for (a, appeared) in self.appeared.iter().enumerate() {
            if renamed_appeared.contains(&a) {
                continue;
            }
            if let Some(bytes) = self
                .appeared_bytes
                .get_mut(a)
                .and_then(Option::take)
                .filter(|bytes| !bytes.is_empty())
            {
                let conflict = ctx.store.as_ref().and_then(|store| {
                    conflict_version(store, &ctx.root, &appeared.path, bytes.clone())
                });
                self.versions.push(conflict.unwrap_or_else(|| {
                    Request::new(appeared.relative_path.clone(), bytes, Cause::External)
                }));
            }
        }
        let created = self
            .appeared
            .iter()
            .enumerate()
            .filter(|(a, _)| !renamed_appeared.contains(a))
            .map(|(_, appeared)| appeared.path.clone())
            .chain(self.unread.drain(..));
        for path in created.collect::<Vec<_>>() {
            ctx.send(WatchEvent::FileChanged {
                path,
                change: FileChange::Created,
            });
        }
        if let Some(store) = &ctx.store
            && !self.changes.is_empty()
        {
            let _ = store.apply(&ctx.token, std::mem::take(&mut self.changes));
        }
        if let Some(history) = &ctx.history {
            for request in self.versions.drain(..) {
                history.record(request);
            }
        }
    }
}

fn spawn_rebuild(ctx: WatchContext) {
    let spawned = std::thread::Builder::new()
        .name("cerata-ignore-rebuild".into())
        .spawn(move || {
            let Ok(index) = index_workspace(&ctx.root, &ctx.token) else {
                return;
            };
            if ctx.token.is_stale() {
                return;
            }
            *ctx.ignore.write() = index.ignore.clone();
            let mut rebuilt = LiveIndex::from_index(&index);
            {
                let mut live = ctx.index.lock();
                for file in index.files.iter() {
                    if let (Some(old), Some(new)) =
                        (live.get(&file.path), rebuilt.get_mut(&file.path))
                        && old.size == new.size
                        && old.modified_ns == new.modified_ns
                    {
                        new.hash = old.hash;
                        new.title = old.title.clone();
                    }
                }
                *live = rebuilt;
            }
            ctx.send(WatchEvent::IgnoreRebuilt);
            ctx.send(WatchEvent::DirectoryChanged {
                path: ctx.root.clone(),
            });
        });
    if let Err(error) = spawned {
        log::error!("could not start the ignore rebuild: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::EpochCounter;
    use crate::index::index_workspace_with_global;
    use std::fs;
    use std::sync::mpsc;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(5);

    struct Harness {
        _dir: TempDir,
        root: PathBuf,
        counter: EpochCounter,
        ctx: WatchContext,
        events: Receiver<WatchEnvelope>,
        reloads: Arc<Mutex<usize>>,
        watcher: Option<WorkspaceWatcher>,
    }

    impl Harness {
        fn new(setup: impl FnOnce(&Path)) -> Self {
            let dir = TempDir::new().unwrap();
            let root = dunce::canonicalize(dir.path()).unwrap();
            setup(&root);
            let counter = EpochCounter::new();
            let index = index_workspace_with_global(&root, None, &counter.token()).unwrap();
            let mut live = LiveIndex::from_index(&index);
            for file in &index.files {
                if let (Ok(bytes), Some(entry)) = (fs::read(&file.path), live.get_mut(&file.path)) {
                    entry.hash = Some(crate::store::content_hash(&bytes));
                }
            }
            let (sender, events) = mpsc::channel();
            let reloads = Arc::new(Mutex::new(0));
            let reload_count = reloads.clone();
            let ctx = WatchContext {
                root: root.clone(),
                token: counter.token(),
                index: Arc::new(Mutex::new(live)),
                ignore: Arc::new(RwLock::new(index.ignore.clone())),
                self_writes: Arc::new(SelfWriteLog::new()),
                store: None,
                history: None,
                files: Arc::new(crate::reconcile::OsFiles),
                reload_settings: Some(Arc::new(move || *reload_count.lock() += 1)),
                on_error: None,
                events: sender,
            };
            Self {
                _dir: dir,
                root,
                counter,
                ctx,
                events,
                reloads,
                watcher: None,
            }
        }

        fn start(mut self) -> Self {
            self.watcher = Some(WorkspaceWatcher::start(self.ctx.clone()).unwrap());
            while self.events.recv_timeout(2 * BATCH_WINDOW).is_ok() {}
            self
        }

        fn expect(&self, what: &str, pred: impl Fn(&WatchEvent) -> bool) -> WatchEnvelope {
            let deadline = Instant::now() + WAIT;
            loop {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_else(|| panic!("timed out waiting for {what}"));
                let envelope = self
                    .events
                    .recv_timeout(left)
                    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
                if pred(&envelope.event) {
                    return envelope;
                }
            }
        }

        fn expect_silence_about(&self, path: &Path, window: Duration) {
            let deadline = Instant::now() + window;
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match self.events.recv_timeout(left) {
                    Ok(WatchEnvelope {
                        event: WatchEvent::FileChanged { path: changed, .. },
                        ..
                    }) if changed == path => panic!("unexpected event for {}", path.display()),
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        }
    }

    #[test]
    fn external_edit_is_reported_with_root_and_epoch() {
        let h = Harness::new(|root| fs::write(root.join("a.md"), "one").unwrap()).start();
        let path = h.root.join("a.md");
        fs::write(&path, "two").unwrap();
        let envelope = h.expect("modification", |e| {
            matches!(
                e,
                WatchEvent::FileChanged {
                    change: FileChange::Modified,
                    ..
                }
            )
        });
        assert!(envelope.is_for(&h.root, 0));
        assert_eq!(
            envelope.event,
            WatchEvent::FileChanged {
                path,
                change: FileChange::Modified
            }
        );
    }

    #[test]
    fn own_save_is_silent_but_an_external_edit_right_after_is_reported() {
        let h = Harness::new(|root| fs::write(root.join("a.md"), "one").unwrap()).start();
        let path = h.root.join("a.md");
        h.ctx.self_writes.record(&path, b"ours");
        crate::fs::atomic::atomic_write(&path, b"ours").unwrap();
        h.expect_silence_about(&path, Duration::from_millis(800));
        fs::write(&path, "theirs").unwrap();
        h.expect("external edit after a save", |e| {
            matches!(
                e,
                WatchEvent::FileChanged {
                    change: FileChange::Modified,
                    ..
                }
            )
        });
    }

    #[test]
    fn an_outside_return_to_our_last_write_is_reported() {
        let h = Harness::new(|root| fs::write(root.join("a.md"), "ours").unwrap()).start();
        let path = h.root.join("a.md");
        let hash = h.ctx.self_writes.record(&path, b"ours");
        h.ctx.self_writes.written(&path, hash, b"ours");
        let modified = |e: &WatchEvent| {
            matches!(
                e,
                WatchEvent::FileChanged {
                    change: FileChange::Modified,
                    ..
                }
            )
        };
        fs::write(&path, "theirs, longer").unwrap();
        h.expect("the outside edit", modified);
        fs::write(&path, "ours").unwrap();
        h.expect("the return to our last write", modified);
    }

    #[test]
    fn a_rename_outside_cerata_is_reported_as_one() {
        let h =
            Harness::new(|root| fs::write(root.join("old.md"), "# Kept title").unwrap()).start();
        fs::rename(h.root.join("old.md"), h.root.join("new.md")).unwrap();
        let renamed = h.expect("rename", |e| matches!(e, WatchEvent::Renamed { .. }));
        assert_eq!(
            renamed.event,
            WatchEvent::Renamed {
                from: h.root.join("old.md"),
                to: h.root.join("new.md"),
            }
        );
        let index = h.ctx.index.lock();
        assert!(!index.contains(&h.root.join("old.md")));
        assert_eq!(index.title(&h.root.join("new.md")), Some("Kept title"));
    }

    #[test]
    fn a_watch_error_asks_for_a_reconciliation() {
        struct Quiet;
        impl WatchBackend for Quiet {
            fn watch(&mut self, _: &Path, _: bool) -> notify::Result<()> {
                Ok(())
            }
            fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
                Ok(())
            }
        }
        let mut h = Harness::new(|_| {});
        let asked = Arc::new(Mutex::new(0));
        let counted = asked.clone();
        h.ctx.on_error = Some(Arc::new(move || *counted.lock() += 1));
        let (os_sender, os_receiver) = mpsc::channel();
        let _watcher =
            WorkspaceWatcher::start_with_backend(h.ctx.clone(), Box::new(Quiet), os_receiver)
                .unwrap();
        os_sender
            .send(Err(notify::Error::generic("buffer overflow")))
            .unwrap();
        let deadline = Instant::now() + WAIT;
        while *asked.lock() == 0 {
            assert!(Instant::now() < deadline, "no reconciliation asked");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn rename_moves_index_membership() {
        let h = Harness::new(|root| fs::write(root.join("old.md"), "x").unwrap()).start();
        fs::rename(h.root.join("old.md"), h.root.join("new.md")).unwrap();
        h.expect(
            "rename of the old name",
            |e| matches!(e, WatchEvent::Renamed { from, .. } if *from == h.root.join("old.md")),
        );
        h.expect(
            "parent listing change",
            |e| matches!(e, WatchEvent::DirectoryChanged { path } if *path == h.root),
        );
        let deadline = Instant::now() + WAIT;
        while !h.ctx.index.lock().contains(&h.root.join("new.md")) {
            assert!(Instant::now() < deadline, "new name never indexed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!h.ctx.index.lock().contains(&h.root.join("old.md")));
    }

    #[test]
    fn folder_moved_in_is_walked() {
        let h = Harness::new(|_| {}).start();
        let target = h.root.join("batch");
        let temp = h.root.join("staging-copy");
        fs::create_dir_all(temp.join("deep")).unwrap();
        fs::write(temp.join("one.md"), "x").unwrap();
        fs::write(temp.join("deep").join("two.md"), "x").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        fs::rename(&temp, &target).unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            let index = h.ctx.index.lock();
            if index.contains(&target.join("one.md"))
                && index.contains(&target.join("deep").join("two.md"))
            {
                assert!(!index.contains(&temp.join("one.md")));
                break;
            }
            drop(index);
            assert!(Instant::now() < deadline, "moved folder never indexed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn gitignore_change_rebuilds_matcher_and_index() {
        let h = Harness::new(|root| {
            fs::create_dir(root.join("drafts")).unwrap();
            fs::write(root.join("drafts").join("wip.md"), "x").unwrap();
        })
        .start();
        assert!(
            h.ctx
                .index
                .lock()
                .contains(&h.root.join("drafts").join("wip.md"))
        );
        fs::write(h.root.join(".gitignore"), "drafts/\n").unwrap();
        h.expect("ignore rebuild", |e| *e == WatchEvent::IgnoreRebuilt);
        assert!(
            !h.ctx
                .index
                .lock()
                .contains(&h.root.join("drafts").join("wip.md"))
        );
        assert!(h.ctx.ignore.read().excludes(&h.root.join("drafts"), true));
    }

    #[test]
    fn workspace_config_change_reloads_settings() {
        let h = Harness::new(|root| fs::create_dir(root.join(".cerata")).unwrap()).start();
        fs::write(workspace_config_path(&h.root), "editor.font-size = 18\n").unwrap();
        h.expect("settings change", |e| *e == WatchEvent::SettingsChanged);
        assert!(*h.reloads.lock() >= 1);
    }

    #[test]
    fn events_of_a_stale_epoch_are_dropped() {
        let h = Harness::new(|root| fs::write(root.join("a.md"), "one").unwrap()).start();
        h.counter.advance();
        let path = h.root.join("a.md");
        fs::write(&path, "two").unwrap();
        h.expect_silence_about(&path, Duration::from_millis(1000));
    }

    #[test]
    fn deleting_the_root_stops_the_watcher_with_a_notice() {
        let h = Harness::new(|root| fs::write(root.join("a.md"), "one").unwrap()).start();
        let moved = h.root.with_extension("moved");
        fs::rename(&h.root, &moved).unwrap();
        h.expect("root removal", |e| *e == WatchEvent::RootRemoved);
        fs::remove_dir_all(&moved).unwrap();
    }

    #[test]
    fn batches_group_events_within_the_window() {
        let (sender, receiver) = mpsc::channel();
        let feeder = std::thread::spawn(move || {
            for (delay, value) in [(0, 1), (100, 2), (100, 3), (1500, 4)] {
                std::thread::sleep(Duration::from_millis(delay));
                sender.send(value).unwrap();
            }
        });
        let first = receiver.recv().unwrap();
        let (batch, connected) = collect_batch(first, &receiver, Duration::from_secs(1));
        assert_eq!(batch, vec![1, 2, 3]);
        assert!(connected);
        let next = receiver.recv().unwrap();
        assert_eq!(next, 4);
        feeder.join().unwrap();
    }

    #[derive(Default)]
    struct FakeBackend {
        calls: Arc<Mutex<Vec<(String, PathBuf, bool)>>>,
    }

    impl WatchBackend for FakeBackend {
        fn watch(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
            self.calls
                .lock()
                .push(("watch".into(), path.to_path_buf(), recursive));
            if recursive {
                Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch))
            } else {
                Ok(())
            }
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.calls
                .lock()
                .push(("unwatch".into(), path.to_path_buf(), false));
            Ok(())
        }
    }

    #[test]
    fn watch_limit_degrades_to_open_file_folders_with_one_notice() {
        let h = Harness::new(|root| fs::create_dir(root.join("notes")).unwrap());
        let backend = FakeBackend::default();
        let calls = backend.calls.clone();
        let (_os_sender, os_receiver) = mpsc::channel();
        let watcher =
            WorkspaceWatcher::start_with_backend(h.ctx.clone(), Box::new(backend), os_receiver)
                .unwrap();
        assert!(watcher.is_limited());
        let notice = h.expect("limit notice", |e| {
            matches!(e, WatchEvent::WatchLimited { .. })
        });
        let WatchEvent::WatchLimited { message } = notice.event else {
            unreachable!()
        };
        assert!(message.contains("sudo sysctl fs.inotify.max_user_watches="));

        let note = h.root.join("notes").join("a.md");
        watcher.set_open_files(&[note.clone(), h.root.join("b.md")]);
        watcher.set_open_files(&[note]);
        let calls = calls.lock().clone();
        assert!(calls.contains(&("watch".into(), h.root.join("notes"), false)));
        assert!(calls.contains(&("watch".into(), h.root.clone(), false)));
        assert!(calls.contains(&("unwatch".into(), h.root.clone(), false)));
        assert!(
            h.events
                .try_iter()
                .all(|e| !matches!(e.event, WatchEvent::WatchLimited { .. }))
        );
    }
}
