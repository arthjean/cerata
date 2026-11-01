//! Autosave: at most one write per second per file, never two writes of the
//! same file at once, and a catch-up save when the text changed during a
//! write.
//!
//! Ported from Writer `src/lib/save.ts` (`scheduleSave`, `isSaveInFlight`,
//! `cancelSave`, throttle, follow-up save). The open documents live in the
//! editor; the saver reaches them through [`SaveTarget`] and identifies each
//! text by a revision number, so a write that finishes after a newer edit
//! can never mark the newer text as saved.

use crate::fs::text::{SaveOptions, TextFormat, encode, prepare_for_save};
use crate::fs::{FsError, WorkspaceFs};
use crate::self_writes::SelfWriteLog;
use parking_lot::{Condvar, Mutex};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Minimum delay between two writes of the same file (Writer `THROTTLE_MS`).
pub const SAVE_THROTTLE: Duration = Duration::from_secs(1);

/// Text of an open document with unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSnapshot {
    /// Increases on every edit of the document.
    pub revision: u64,
    pub text: String,
    pub format: TextFormat,
}

/// The editor side of autosave.
pub trait SaveTarget: Send + Sync + 'static {
    /// Current text of `path` if it is open and dirty, `None` otherwise.
    fn dirty_snapshot(&self, path: &Path) -> Option<SaveSnapshot>;
    /// `revision` reached the disk as `disk_text`. The target marks the
    /// document clean only if its revision is still `revision`; otherwise it
    /// stays dirty and the saver writes again.
    fn saved(&self, path: &Path, revision: u64, disk_text: &str, modified_at: u64);
    /// The write of `revision` failed; the document stays dirty and nothing
    /// partial is left on disk.
    fn save_failed(&self, path: &Path, revision: u64, error: &FsError);
    /// The text `path` holds on disk as far as the target knows (what it
    /// last read or wrote). An autosave writes only while the disk still
    /// holds it; `None` writes without checking.
    fn expected_disk(&self, path: &Path) -> Option<String>;
    /// The disk no longer holds [`Self::expected_disk`]: an external change
    /// the watcher has not reported yet. `revision` was not written and no
    /// later save is queued until the target schedules one.
    fn save_conflicted(&self, path: &Path, revision: u64);
}

#[derive(Debug, Default)]
struct Controller {
    last_save: Option<Instant>,
    in_flight: bool,
    pending: bool,
    due: Option<Instant>,
}

struct Shared {
    controllers: Mutex<HashMap<PathBuf, Controller>>,
    wake: Condvar,
    shutdown: Mutex<bool>,
    fs: WorkspaceFs,
    target: Arc<dyn SaveTarget>,
    options: Mutex<SaveOptions>,
    self_writes: Option<Arc<SelfWriteLog>>,
    throttle: Duration,
}

/// Background autosave for one workspace. Dropping it stops the timer
/// thread; writes already started complete.
pub struct AutoSaver {
    shared: Arc<Shared>,
}

impl AutoSaver {
    pub fn spawn(
        fs: WorkspaceFs,
        target: Arc<dyn SaveTarget>,
        options: SaveOptions,
        self_writes: Option<Arc<SelfWriteLog>>,
    ) -> std::io::Result<Self> {
        Self::spawn_with_throttle(fs, target, options, self_writes, SAVE_THROTTLE)
    }

    pub(crate) fn spawn_with_throttle(
        fs: WorkspaceFs,
        target: Arc<dyn SaveTarget>,
        options: SaveOptions,
        self_writes: Option<Arc<SelfWriteLog>>,
        throttle: Duration,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            controllers: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            shutdown: Mutex::new(false),
            fs,
            target,
            options: Mutex::new(options),
            self_writes,
            throttle,
        });
        let timer = shared.clone();
        std::thread::Builder::new()
            .name("cerata-autosave".into())
            .spawn(move || run_timer(&timer))?;
        Ok(Self { shared })
    }

    /// Requests a save of `path` (Writer `scheduleSave`): immediately if the
    /// last write is older than the throttle, otherwise when it expires, and
    /// after the current write if one is in flight.
    pub fn schedule(&self, path: &Path) {
        let mut controllers = self.shared.controllers.lock();
        let controller = controllers.entry(path.to_path_buf()).or_default();
        controller.pending = true;
        queue(controller, self.shared.throttle);
        self.shared.wake.notify_all();
    }

    /// Drops a pending save of `path` (Writer `cancelSave`). A write already
    /// in flight completes.
    pub fn cancel(&self, path: &Path) {
        let mut controllers = self.shared.controllers.lock();
        if let Some(controller) = controllers.get_mut(path) {
            controller.pending = false;
            controller.due = None;
        }
        cleanup(&mut controllers, path, self.shared.throttle);
    }

    pub fn is_in_flight(&self, path: &Path) -> bool {
        self.shared
            .controllers
            .lock()
            .get(path)
            .is_some_and(|controller| controller.in_flight)
    }

    /// Applies new `files.*` settings to the next writes.
    pub fn set_options(&self, options: SaveOptions) {
        *self.shared.options.lock() = options;
    }

    /// Writes every pending save now, throttle ignored, and waits for the
    /// writes in flight, up to `timeout`. Called before the saver or its
    /// documents go away (window close, workspace change, quit): dropping
    /// the saver forgets the saves the throttle still holds. `false` if
    /// writes were still running at the deadline.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut controllers = self.shared.controllers.lock();
        loop {
            let due: Vec<PathBuf> = controllers
                .iter()
                .filter(|(_, c)| !c.in_flight && c.due.is_some())
                .map(|(path, _)| path.clone())
                .collect();
            if !due.is_empty() && Instant::now() < deadline {
                let now = Instant::now();
                for path in &due {
                    if let Some(controller) = controllers.get_mut(path) {
                        controller.due = None;
                        controller.in_flight = true;
                        controller.pending = false;
                        controller.last_save = Some(now);
                    }
                }
                drop(controllers);
                // The window is going away: its text is written even over a
                // change the watcher has not reported, as Writer does.
                for path in due {
                    perform_save(&self.shared, path, false);
                }
                controllers = self.shared.controllers.lock();
                continue;
            }
            let busy = controllers.values().any(|c| c.in_flight || c.due.is_some());
            if !busy {
                return true;
            }
            if Instant::now() >= deadline
                || self
                    .shared
                    .wake
                    .wait_until(&mut controllers, deadline)
                    .timed_out()
            {
                return !controllers.values().any(|c| c.in_flight || c.due.is_some());
            }
        }
    }
}

impl Drop for AutoSaver {
    fn drop(&mut self) {
        *self.shared.shutdown.lock() = true;
        // Take the controllers lock so the timer is either waiting (and gets
        // the notification) or will see the flag before waiting again.
        let _controllers = self.shared.controllers.lock();
        self.shared.wake.notify_all();
    }
}

fn queue(controller: &mut Controller, throttle: Duration) {
    if controller.in_flight {
        return;
    }
    let now = Instant::now();
    let due = controller
        .last_save
        .map(|last| last + throttle)
        .filter(|due| *due > now)
        .unwrap_or(now);
    controller.due = Some(due);
}

/// Forgets an idle controller once its throttle window has passed. Writer
/// dropped it as soon as it was idle, which forgot the last write time and
/// let a save scheduled right after a write go to disk immediately.
fn cleanup(controllers: &mut HashMap<PathBuf, Controller>, path: &Path, throttle: Duration) {
    if controllers
        .get(path)
        .is_some_and(|c| is_forgettable(c, throttle))
    {
        controllers.remove(path);
    }
}

fn is_forgettable(controller: &Controller, throttle: Duration) -> bool {
    !controller.in_flight
        && !controller.pending
        && controller.due.is_none()
        && controller
            .last_save
            .is_none_or(|last| last.elapsed() >= throttle)
}

fn run_timer(shared: &Arc<Shared>) {
    let mut controllers = shared.controllers.lock();
    loop {
        if *shared.shutdown.lock() {
            return;
        }
        let now = Instant::now();
        controllers.retain(|_, c| !is_forgettable(c, shared.throttle));
        let due: Vec<PathBuf> = controllers
            .iter()
            .filter(|(_, c)| !c.in_flight && c.due.is_some_and(|due| due <= now))
            .map(|(path, _)| path.clone())
            .collect();
        for path in due {
            if let Some(controller) = controllers.get_mut(&path) {
                controller.due = None;
                controller.in_flight = true;
                controller.pending = false;
                controller.last_save = Some(now);
            }
            let worker = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("cerata-save".into())
                .spawn(move || perform_save(&worker, path, true));
            if let Err(error) = spawned {
                log::error!("could not start a save thread: {error}");
            }
        }
        let next = controllers
            .values()
            .filter(|c| !c.in_flight)
            .filter_map(|c| c.due)
            .min();
        match next {
            Some(deadline) => {
                shared.wake.wait_until(&mut controllers, deadline);
            }
            None => shared.wake.wait(&mut controllers),
        }
    }
}

/// Whether the disk no longer holds the text the target last read or
/// wrote. A read error other than a missing file lets the write go on and
/// report it.
fn disk_moved(shared: &Shared, path: &Path) -> bool {
    let Some(expected) = shared.target.expected_disk(path) else {
        return false;
    };
    match shared.fs.read_file(path) {
        Ok(file) => file.content != expected,
        Err(FsError::NotFound(_)) => true,
        Err(_) => false,
    }
}

/// Writes the dirty text of `path`. With `check`, a disk that changed since
/// the target last read or wrote it is reported instead of overwritten.
fn perform_save(shared: &Shared, path: PathBuf, check: bool) {
    let mut follow_up = false;
    if let Some(snapshot) = shared.target.dirty_snapshot(&path) {
        if check && disk_moved(shared, &path) {
            shared.target.save_conflicted(&path, snapshot.revision);
            finish_save(shared, &path, false);
            return;
        }
        let options = *shared.options.lock();
        let disk_text = prepare_for_save(&snapshot.text, options);
        let bytes = encode(&disk_text, snapshot.format);
        if let Some(log) = &shared.self_writes {
            log.record(&path, &bytes);
        }
        match shared.fs.write_bytes(&path, &bytes) {
            Ok(modified_at) => {
                shared
                    .target
                    .saved(&path, snapshot.revision, &disk_text, modified_at);
                follow_up = shared
                    .target
                    .dirty_snapshot(&path)
                    .is_some_and(|latest| latest.revision != snapshot.revision);
            }
            Err(error) => {
                log::error!("save failed: {error}");
                shared.target.save_failed(&path, snapshot.revision, &error);
            }
        }
    }
    finish_save(shared, &path, follow_up);
}

fn finish_save(shared: &Shared, path: &Path, follow_up: bool) {
    let mut controllers = shared.controllers.lock();
    if let Some(controller) = controllers.get_mut(path) {
        controller.in_flight = false;
        if controller.pending || follow_up {
            controller.pending = true;
            queue(controller, shared.throttle);
        }
    }
    cleanup(&mut controllers, path, shared.throttle);
    shared.wake.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use tempfile::TempDir;

    #[derive(Default)]
    struct Doc {
        revision: u64,
        text: String,
        saved_revision: u64,
        disk_text: String,
        errors: Vec<String>,
        expected: Option<String>,
        conflicts: Vec<u64>,
    }

    type Hook = Box<dyn Fn(&FakeEditor) + Send + Sync>;

    #[derive(Default)]
    struct FakeEditor {
        doc: Mutex<Doc>,
        writes: Mutex<Vec<(Instant, String)>>,
        write_starts: Mutex<Vec<Instant>>,
        on_saved_hook: Mutex<Option<Hook>>,
        saved_events: Mutex<Option<mpsc::Sender<u64>>>,
    }

    impl FakeEditor {
        fn edit(&self, text: &str) {
            let mut doc = self.doc.lock();
            doc.revision += 1;
            doc.text = text.to_owned();
        }
    }

    impl SaveTarget for FakeEditor {
        fn dirty_snapshot(&self, _path: &Path) -> Option<SaveSnapshot> {
            let doc = self.doc.lock();
            if doc.revision != doc.saved_revision {
                self.write_starts.lock().push(Instant::now());
            }
            (doc.revision != doc.saved_revision).then(|| SaveSnapshot {
                revision: doc.revision,
                text: doc.text.clone(),
                format: TextFormat::default(),
            })
        }

        fn saved(&self, _path: &Path, revision: u64, disk_text: &str, _modified_at: u64) {
            self.writes
                .lock()
                .push((Instant::now(), disk_text.to_owned()));
            if let Some(hook) = self.on_saved_hook.lock().take() {
                hook(self);
            }
            {
                let mut doc = self.doc.lock();
                doc.disk_text = disk_text.to_owned();
                if doc.revision == revision {
                    doc.saved_revision = revision;
                }
            }
            if let Some(sender) = &*self.saved_events.lock() {
                let _ = sender.send(revision);
            }
        }

        fn save_failed(&self, _path: &Path, _revision: u64, error: &FsError) {
            self.doc.lock().errors.push(error.to_string());
            if let Some(sender) = &*self.saved_events.lock() {
                let _ = sender.send(u64::MAX);
            }
        }

        fn expected_disk(&self, _path: &Path) -> Option<String> {
            self.doc.lock().expected.clone()
        }

        fn save_conflicted(&self, _path: &Path, revision: u64) {
            self.doc.lock().conflicts.push(revision);
            if let Some(sender) = &*self.saved_events.lock() {
                let _ = sender.send(CONFLICT);
            }
        }
    }

    const CONFLICT: u64 = u64::MAX - 1;

    const THROTTLE: Duration = Duration::from_millis(300);
    const WAIT: Duration = Duration::from_secs(5);

    fn setup() -> (
        TempDir,
        PathBuf,
        Arc<FakeEditor>,
        AutoSaver,
        mpsc::Receiver<u64>,
    ) {
        let dir = TempDir::new().unwrap();
        let path = dunce::canonicalize(dir.path()).unwrap().join("test.md");
        std::fs::write(&path, "initial").unwrap();
        let editor = Arc::new(FakeEditor::default());
        let (sender, receiver) = mpsc::channel();
        *editor.saved_events.lock() = Some(sender);
        let saver = AutoSaver::spawn_with_throttle(
            WorkspaceFs::new(dir.path()).unwrap(),
            editor.clone(),
            SaveOptions {
                trim_trailing_whitespace: false,
                insert_final_newline: false,
            },
            None,
            THROTTLE,
        )
        .unwrap();
        (dir, path, editor, saver, receiver)
    }

    /// A file rewritten on disk before the watcher reports it (sync client,
    /// `git checkout`) is never overwritten by an autosave: the target hears
    /// of the conflict, and a disk that matches again is written.
    #[test]
    fn an_unreported_external_change_is_reported_instead_of_overwritten() {
        let (_dir, path, editor, saver, saved) = setup();
        editor.doc.lock().expected = Some("initial".into());
        std::fs::write(&path, "theirs").unwrap();
        editor.edit("mine");
        saver.schedule(&path);
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), CONFLICT);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs");
        assert_eq!(editor.doc.lock().conflicts, [1]);

        std::fs::remove_file(&path).unwrap();
        saver.schedule(&path);
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), CONFLICT, "deleted");
        assert!(!path.exists(), "a deleted file is not recreated");

        std::fs::write(&path, "theirs").unwrap();
        editor.doc.lock().expected = Some("theirs".into());
        saver.schedule(&path);
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
    }

    /// Writer `save.test.ts`: "keeps newer edits dirty until a follow-up save
    /// completes".
    #[test]
    fn keeps_newer_edits_dirty_until_a_follow_up_save_completes() {
        let (_dir, path, editor, saver, saved) = setup();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (in_write_tx, in_write_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        // Hold the first write open (between disk write and completion)
        // and edit the document meanwhile.
        *editor.on_saved_hook.lock() = Some(Box::new(move |editor: &FakeEditor| {
            editor.edit("second draft");
            in_write_tx.send(()).unwrap();
            release_rx.lock().recv_timeout(WAIT).unwrap();
        }));

        editor.edit("first draft");
        saver.schedule(&path);
        in_write_rx.recv_timeout(WAIT).unwrap();
        assert!(saver.is_in_flight(&path));
        // More schedule calls while in flight never start a parallel write.
        saver.schedule(&path);
        std::thread::sleep(THROTTLE * 2);
        assert_eq!(editor.writes.lock().len(), 1);
        release_tx.send(()).unwrap();

        assert_eq!(saved.recv_timeout(WAIT).unwrap(), 1);
        {
            let doc = editor.doc.lock();
            assert_eq!(doc.disk_text, "first draft");
            assert_eq!(doc.text, "second draft");
            assert_ne!(doc.revision, doc.saved_revision, "newer edit stays dirty");
        }
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), 2);
        let doc = editor.doc.lock();
        assert_eq!(doc.saved_revision, doc.revision);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second draft");
    }

    /// An edit the throttle is still holding reaches the disk on flush,
    /// before the saver is dropped (window close, quit).
    #[test]
    fn flush_writes_the_save_the_throttle_holds() {
        let (_dir, path, editor, saver, saved) = setup();
        editor.edit("a");
        saver.schedule(&path);
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), 1);
        editor.edit("b");
        saver.schedule(&path);
        let started = Instant::now();
        assert!(saver.flush(WAIT));
        assert!(started.elapsed() < THROTTLE, "flush ignores the throttle");
        drop(saver);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b");
        let doc = editor.doc.lock();
        assert_eq!(doc.saved_revision, doc.revision);
    }

    #[test]
    fn at_most_one_write_per_throttle_window() {
        let (_dir, path, editor, saver, saved) = setup();
        editor.edit("a");
        saver.schedule(&path);
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), 1);
        for text in ["b", "c", "d"] {
            editor.edit(text);
            saver.schedule(&path);
        }
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), 4);
        let writes = editor.writes.lock();
        assert_eq!(writes.len(), 2, "the burst coalesced into one write");
        // Snapshots taken at write start (the follow-up probe after a
        // write also snapshots, so compare the first and the last start).
        let starts = editor.write_starts.lock();
        let (first, last) = (starts[0], starts[starts.len() - 1]);
        assert!(
            last.duration_since(first) >= THROTTLE,
            "{:?}",
            last.duration_since(first)
        );
        assert_eq!(writes[1].1, "d");
    }

    #[test]
    fn save_options_apply_before_the_write() {
        let (_dir, path, editor, saver, saved) = setup();
        saver.set_options(SaveOptions {
            trim_trailing_whitespace: true,
            insert_final_newline: true,
        });
        editor.edit("a  \nb\t");
        saver.schedule(&path);
        saved.recv_timeout(WAIT).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\n");
    }

    #[test]
    fn failed_save_keeps_the_document_dirty_and_exposes_the_os_error() {
        let (_dir, path, editor, saver, saved) = setup();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).unwrap();
        editor.edit("new");
        saver.schedule(&path);
        assert_eq!(saved.recv_timeout(WAIT).unwrap(), u64::MAX);
        {
            let doc = editor.doc.lock();
            assert_ne!(doc.saved_revision, doc.revision);
            assert!(doc.errors[0].contains("read-only"), "{:?}", doc.errors);
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "initial");
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
    }

    #[test]
    fn cancel_drops_a_pending_save() {
        let (_dir, path, editor, saver, saved) = setup();
        editor.edit("a");
        saver.schedule(&path);
        saved.recv_timeout(WAIT).unwrap();
        editor.edit("b");
        saver.schedule(&path);
        saver.cancel(&path);
        assert!(saved.recv_timeout(THROTTLE * 3).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a");
    }

    #[test]
    fn writes_are_recorded_for_the_watcher() {
        let dir = TempDir::new().unwrap();
        let path = dunce::canonicalize(dir.path()).unwrap().join("test.md");
        let editor = Arc::new(FakeEditor::default());
        let (sender, saved) = mpsc::channel();
        *editor.saved_events.lock() = Some(sender);
        let log = Arc::new(SelfWriteLog::new());
        let saver = AutoSaver::spawn_with_throttle(
            WorkspaceFs::new(dir.path()).unwrap(),
            editor.clone(),
            SaveOptions::default(),
            Some(log.clone()),
            THROTTLE,
        )
        .unwrap();
        editor.edit("x");
        saver.schedule(&path);
        saved.recv_timeout(WAIT).unwrap();
        assert!(log.is_self_write(&path));
    }
}
