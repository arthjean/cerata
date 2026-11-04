use super::*;
use crate::epoch::EpochCounter;
use crate::store::StoreOpen;
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Receiver};
use tempfile::TempDir;

/// Reads through the disk and counts every open; files named in
/// `offline` are cloud placeholders.
#[derive(Default)]
struct CountingFiles {
    opened: AtomicUsize,
    offline: Mutex<HashSet<String>>,
}

impl CountingFiles {
    fn opened(&self) -> usize {
        self.opened.load(Ordering::SeqCst)
    }
}

impl FileAccess for CountingFiles {
    fn is_offline(&self, path: &Path, _metadata: &Metadata) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        name.is_some_and(|name| self.offline.lock().contains(&name))
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        fs::read(path)
    }
}

struct Lib {
    _dir: TempDir,
    _stores: TempDir,
    root: PathBuf,
    counter: EpochCounter,
    files: Arc<CountingFiles>,
    store: Store,
    ctx: ReconcileContext,
    events: Receiver<WatchEnvelope>,
    state: PassState,
}

impl Lib {
    fn new(setup: impl FnOnce(&Path)) -> Self {
        let dir = TempDir::new().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        setup(&root);
        let stores = TempDir::new().unwrap();
        let StoreOpen::Ready { store, .. } = Store::open(stores.path(), &|_| None) else {
            panic!("no store");
        };
        let counter = EpochCounter::new();
        let files = Arc::new(CountingFiles::default());
        let (sender, events) = mpsc::channel();
        let ctx = ReconcileContext {
            root: root.clone(),
            token: counter.token(),
            index: Arc::new(Mutex::new(LiveIndex::new(&root))),
            ignore: Arc::new(RwLock::new(Arc::new(WorkspaceIgnore::bootstrap(&root)))),
            index_ready: Arc::new(AtomicBool::new(false)),
            store: Some(store.clone()),
            history: None,
            self_writes: Arc::new(SelfWriteLog::new()),
            files: files.clone(),
            global_gitignore: None,
            events: sender,
        };
        Self {
            _dir: dir,
            _stores: stores,
            root,
            counter,
            files,
            store,
            ctx,
            events,
            state: PassState::default(),
        }
    }

    /// A library whose first pass already ran, events drained.
    fn indexed(setup: impl FnOnce(&Path)) -> Self {
        let mut lib = Self::new(setup);
        lib.age_all();
        lib.pass().unwrap();
        lib.drain();
        lib
    }

    fn pass(&mut self) -> Result<PassReport, PassError> {
        run_pass(&self.ctx, &mut self.state, &mut || {})
    }

    fn drain(&self) -> Vec<WatchEvent> {
        self.events
            .try_iter()
            .map(|envelope| envelope.event)
            .collect()
    }

    fn path(&self, relative: &str) -> PathBuf {
        path_from_relative(&self.root, relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        age(&path);
    }

    fn age_all(&self) {
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                } else {
                    age(&entry.path());
                }
            }
        }
    }

    fn title(&self, relative: &str) -> Option<String> {
        self.ctx
            .index
            .lock()
            .title(&self.path(relative))
            .map(str::to_owned)
    }

    fn id(&self, relative: &str) -> Option<i64> {
        self.store.note(relative).unwrap().map(|note| note.id)
    }
}

/// Moves a file's modified time an hour back, past the settle window.
fn age(path: &Path) {
    let file = fs::File::options().write(true).open(path).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.set_modified(modified - Duration::from_secs(3600))
        .unwrap();
}

fn lines(count: usize, tag: &str) -> String {
    (0..count).map(|i| format!("{tag} line {i:02}\n")).collect()
}

fn note(title: &str, body: &str) -> String {
    format!("# {title}\n\n{body}")
}

#[test]
fn the_first_pass_publishes_the_walk_then_fills_titles_hashes_and_the_store() {
    let mut lib = Lib::new(|root| {
        fs::write(root.join("a.md"), "# Alpha\n").unwrap();
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("b.md"), "# Beta\n").unwrap();
        fs::write(root.join("docs").join("pic.png"), [0u8]).unwrap();
    });
    let report = lib.pass().unwrap();
    assert!(report.initial);
    assert_eq!(report.notes, 2);
    assert_eq!(lib.files.opened(), 2);
    assert!(lib.ctx.index_ready.load(Ordering::SeqCst));
    assert_eq!(lib.drain(), [WatchEvent::IndexComplete { file_count: 2 }]);
    assert_eq!(lib.title("docs/b.md").as_deref(), Some("Beta"));
    let stored = lib.store.note("docs/b.md").unwrap().unwrap().record;
    assert_eq!(stored.title.as_deref(), Some("Beta"));
    assert_eq!(stored.hash, Some(content_hash(b"# Beta\n")));
    assert_eq!(
        lib.ctx.index.lock().find_by_name("PIC.png"),
        Some(lib.path("docs/pic.png"))
    );
    // The walk listed every file: embeds use the table, not a walk.
    assert!(lib.ctx.index.lock().names_complete());
}

/// US-086: once an outside edit replaced Cerata's last write, a return to
/// those bytes (an undo, a checkout) is a change, not an echo.
#[test]
fn an_outside_return_to_our_last_write_is_not_taken_for_its_echo() {
    let mut lib = Lib::indexed(|root| fs::write(root.join("a.md"), "# Ours\n").unwrap());
    let path = lib.path("a.md");
    let hash = lib.ctx.self_writes.record(&path, b"# Ours\n");
    lib.ctx.self_writes.written(&path, hash, b"# Ours\n");
    let modified = WatchEvent::FileChanged {
        path: path.clone(),
        change: FileChange::Modified,
    };
    lib.write("a.md", "# Theirs, longer\n");
    lib.pass().unwrap();
    assert!(lib.drain().contains(&modified));
    lib.write("a.md", "# Ours\n");
    lib.pass().unwrap();
    assert!(lib.drain().contains(&modified), "the restore was swallowed");
    assert_eq!(lib.title("a.md").as_deref(), Some("Ours"));
}

#[test]
fn an_unchanged_library_is_reconciled_without_opening_a_file() {
    let mut lib = Lib::indexed(|root| {
        for i in 0..2_000 {
            fs::write(root.join(format!("n{i:04}.md")), format!("# Note {i}\n")).unwrap();
        }
    });
    let opened = lib.files.opened();
    let report = lib.pass().unwrap();
    assert_eq!(report.notes, 2_000);
    assert_eq!(report.opened, 0);
    assert_eq!(lib.files.opened(), opened, "a file was opened");
    assert_eq!(lib.drain(), []);
}

#[test]
fn changes_the_watcher_missed_are_caught_and_reported_like_its_own() {
    let mut lib = Lib::indexed(|root| {
        fs::write(root.join("kept.md"), note("Kept", "body\n")).unwrap();
        fs::write(root.join("edited.md"), note("Old title", "body\n")).unwrap();
        fs::write(
            root.join("deleted.md"),
            note("Deleted", "unique gone text\n"),
        )
        .unwrap();
    });
    lib.write("edited.md", &note("New title", "body\n"));
    lib.write("sub/added.md", &note("Added", "brand new\n"));
    fs::remove_file(lib.path("deleted.md")).unwrap();
    let report = lib.pass().unwrap();
    assert_eq!((report.modified, report.created, report.removed), (1, 1, 1));
    assert_eq!(
        report.opened, 2,
        "only the changed and the new note are read"
    );
    let events = lib.drain();
    for expected in [
        WatchEvent::FileChanged {
            path: lib.path("edited.md"),
            change: FileChange::Modified,
        },
        WatchEvent::FileChanged {
            path: lib.path("sub/added.md"),
            change: FileChange::Created,
        },
        WatchEvent::FileChanged {
            path: lib.path("deleted.md"),
            change: FileChange::Removed,
        },
        WatchEvent::DirectoryChanged {
            path: lib.root.clone(),
        },
        WatchEvent::DirectoryChanged {
            path: lib.path("sub"),
        },
    ] {
        assert!(
            events.contains(&expected),
            "{expected:?} missing from {events:?}"
        );
    }
    assert_eq!(lib.title("edited.md").as_deref(), Some("New title"));
    assert_eq!(
        lib.store
            .note("edited.md")
            .unwrap()
            .unwrap()
            .record
            .title
            .as_deref(),
        Some("New title")
    );
    assert_eq!(lib.store.note("deleted.md").unwrap(), None);
    assert_eq!(lib.store.deleted_at_path("deleted.md").unwrap().len(), 1);
}

#[test]
fn a_file_modified_within_the_settle_window_is_checked_again_next_pass() {
    let mut lib = Lib::indexed(|root| fs::write(root.join("a.md"), "# One\n").unwrap());
    // Rewritten now: its time is too recent to trust.
    let path = lib.path("a.md");
    fs::write(&path, "# Two\n").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    lib.pass().unwrap();
    assert_eq!(lib.title("a.md").as_deref(), Some("Two"));
    // Same size, same modified time: only the recheck finds it.
    fs::write(&path, "# Six\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    lib.drain();
    let report = lib.pass().unwrap();
    assert_eq!(report.opened, 1);
    assert_eq!(lib.title("a.md").as_deref(), Some("Six"));
    assert!(lib.drain().contains(&WatchEvent::FileChanged {
        path: path.clone(),
        change: FileChange::Modified
    }));
    // Settled now (old time): no longer read.
    age(&path);
    lib.pass().unwrap();
    let opened = lib.files.opened();
    lib.pass().unwrap();
    assert_eq!(lib.files.opened(), opened);
}

#[test]
fn an_offline_file_is_never_opened_and_keeps_its_known_title() {
    let mut lib = Lib::new(|root| {
        fs::write(root.join("cloud.md"), "# Cloud title\n").unwrap();
        fs::write(root.join("placeholder.md"), "# Never read\n").unwrap();
    });
    lib.age_all();
    lib.files.offline.lock().insert("placeholder.md".into());
    lib.pass().unwrap();
    assert_eq!(lib.files.opened(), 1, "only cloud.md is read");
    assert_eq!(lib.title("placeholder.md"), None);
    assert!(
        lib.store
            .note("placeholder.md")
            .unwrap()
            .unwrap()
            .record
            .offline
    );
    // The other file goes offline after its title was read, then changes.
    lib.files.offline.lock().insert("cloud.md".into());
    lib.write("cloud.md", "# Changed while offline, longer\n");
    let opened = lib.files.opened();
    lib.pass().unwrap();
    assert_eq!(lib.files.opened(), opened, "an offline file was opened");
    assert_eq!(lib.title("cloud.md").as_deref(), Some("Cloud title"));
    let stored = lib.store.note("cloud.md").unwrap().unwrap().record;
    assert!(stored.offline);
    assert_eq!(stored.title.as_deref(), Some("Cloud title"));
}

#[cfg(windows)]
#[test]
fn the_windows_offline_attributes_mark_a_placeholder() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.md");
    fs::write(&path, "x").unwrap();
    assert!(!is_offline_metadata(&fs::metadata(&path).unwrap()));
    let status = std::process::Command::new("attrib")
        .args(["+O", &path.to_string_lossy()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(is_offline_metadata(&fs::metadata(&path).unwrap()));
}

#[test]
fn a_vanished_root_stops_the_pass_without_marking_deletions() {
    let mut lib = Lib::indexed(|root| fs::write(root.join("a.md"), "# A\n").unwrap());
    let moved = lib.root.with_extension("moved");
    fs::rename(&lib.root, &moved).unwrap();
    assert_eq!(lib.pass(), Err(PassError::RootUnavailable));
    assert!(lib.ctx.index.lock().contains(&lib.path("a.md")));
    assert!(lib.store.note("a.md").unwrap().is_some());
    assert_eq!(lib.drain(), []);
    fs::rename(&moved, &lib.root).unwrap();
}

#[test]
fn a_stale_epoch_cancels_the_pass_and_applies_nothing() {
    let mut lib = Lib::indexed(|root| fs::write(root.join("a.md"), "# A\n").unwrap());
    lib.write("a.md", "# Changed\n");
    lib.counter.advance();
    assert_eq!(lib.pass(), Err(PassError::Canceled));
    assert_eq!(lib.title("a.md").as_deref(), Some("A"));
    assert_eq!(
        lib.store
            .note("a.md")
            .unwrap()
            .unwrap()
            .record
            .title
            .as_deref(),
        Some("A")
    );
    assert_eq!(lib.drain(), []);
}

#[test]
fn a_focus_after_missed_events_brings_the_change() {
    // No watcher at all: the backend that emits nothing.
    let lib = Lib::indexed(|root| fs::write(root.join("a.md"), "# Before\n").unwrap());
    let reconciler = Reconciler::new();
    let runner = reconciler.clone();
    let ctx = lib.ctx.clone();
    let started = Arc::new(AtomicBool::new(false));
    let started_flag = started.clone();
    let thread = std::thread::spawn(move || {
        runner.run(
            &ctx,
            true,
            Box::new(move || started_flag.store(true, Ordering::SeqCst)),
        );
    });
    assert!(reconciler.request(ReconcileTrigger::FirstRender));
    assert!(reconciler.wait_for_passes(1, Duration::from_secs(10)));
    assert!(started.load(Ordering::SeqCst), "the watcher hook ran");
    lib.drain();
    lib.write("a.md", "# After, longer\n");
    assert!(reconciler.request(ReconcileTrigger::Focus));
    assert!(reconciler.wait_for_passes(2, Duration::from_secs(10)));
    assert_eq!(lib.title("a.md").as_deref(), Some("After, longer"));
    assert!(lib.drain().contains(&WatchEvent::FileChanged {
        path: lib.path("a.md"),
        change: FileChange::Modified
    }));
    // A second focus within 30 s brings nothing.
    assert!(!reconciler.request(ReconcileTrigger::Focus));
    assert!(reconciler.request(ReconcileTrigger::WatchError));
    assert!(reconciler.wait_for_passes(3, Duration::from_secs(10)));
    reconciler.stop();
    thread.join().unwrap();
}

#[test]
fn an_exact_rename_keeps_the_note_id() {
    let mut lib = Lib::indexed(|root| {
        fs::write(root.join("old.md"), note("Same", "content\n")).unwrap();
    });
    let id = lib.id("old.md");
    fs::create_dir(lib.path("moved")).unwrap();
    fs::rename(lib.path("old.md"), lib.path("moved/new.md")).unwrap();
    let report = lib.pass().unwrap();
    assert_eq!((report.renamed, report.created, report.removed), (1, 0, 0));
    assert_eq!(lib.id("moved/new.md"), id);
    assert_eq!(lib.store.note("old.md").unwrap(), None);
    let events = lib.drain();
    assert!(events.contains(&WatchEvent::Renamed {
        from: lib.path("old.md"),
        to: lib.path("moved/new.md"),
    }));
    assert!(!events.iter().any(|event| matches!(
        event,
        WatchEvent::FileChanged {
            change: FileChange::Removed | FileChange::Created,
            ..
        }
    )));
}

#[test]
fn a_rename_with_sixty_percent_kept_keeps_its_id_and_forty_does_not() {
    let original = lines(10, "kept");
    let edited = |kept: usize| -> String {
        original
            .lines()
            .enumerate()
            .map(|(i, line)| {
                if i < kept {
                    format!("{line}\n")
                } else {
                    format!("{}\n", line.replace("kept", "edit"))
                }
            })
            .collect()
    };
    let mut lib = Lib::indexed(|root| {
        fs::write(root.join("sixty.md"), &original).unwrap();
        fs::write(root.join("forty.md"), original.replace("kept", "else")).unwrap();
    });
    let (sixty, forty) = (lib.id("sixty.md"), lib.id("forty.md"));
    fs::remove_file(lib.path("sixty.md")).unwrap();
    lib.write("sixty-renamed.md", &edited(6));
    fs::remove_file(lib.path("forty.md")).unwrap();
    lib.write("forty-renamed.md", &edited(4).replace("kept", "else"));
    let report = lib.pass().unwrap();
    assert_eq!(report.renamed, 1);
    assert_eq!(lib.id("sixty-renamed.md"), sixty);
    let reborn = lib.id("forty-renamed.md");
    assert!(reborn.is_some());
    assert_ne!(reborn, forty);
    assert_eq!(
        lib.store.deleted_at_path("forty.md").unwrap()[0].0,
        forty.unwrap()
    );
    let events = lib.drain();
    assert!(events.contains(&WatchEvent::FileChanged {
        path: lib.path("forty.md"),
        change: FileChange::Removed
    }));
    assert!(events.contains(&WatchEvent::FileChanged {
        path: lib.path("forty-renamed.md"),
        change: FileChange::Created
    }));
}

#[test]
fn of_two_copies_of_a_renamed_note_the_first_path_inherits_its_id() {
    let mut lib = Lib::indexed(|root| fs::write(root.join("a.md"), "# Copy me\n").unwrap());
    let id = lib.id("a.md");
    lib.write("b.md", "# Copy me\n");
    lib.write("c.md", "# Copy me\n");
    fs::remove_file(lib.path("a.md")).unwrap();
    lib.pass().unwrap();
    assert_eq!(lib.id("b.md"), id);
    assert!(lib.id("c.md").is_some());
    assert_ne!(lib.id("c.md"), id);
}

#[test]
fn a_detection_moves_and_writes_nothing() {
    let mut lib = Lib::indexed(|root| {
        fs::write(root.join("a.md"), "# A\n\n![](a-assets/pic.png)\n").unwrap();
        fs::create_dir(root.join("a-assets")).unwrap();
        fs::write(root.join("a-assets").join("pic.png"), [1u8, 2, 3]).unwrap();
    });
    fs::rename(lib.path("a.md"), lib.path("b.md")).unwrap();
    let listing = |root: &Path| {
        let mut names: Vec<String> = walkdir(root);
        names.sort();
        names
    };
    let before = listing(&lib.root);
    let bytes = fs::read(lib.path("b.md")).unwrap();
    lib.pass().unwrap();
    assert_eq!(listing(&lib.root), before);
    assert_eq!(fs::read(lib.path("b.md")).unwrap(), bytes);
    assert!(lib.path("a-assets/pic.png").is_file());
    assert!(!lib.path("b-assets").exists());
}

fn walkdir(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            found.push(crate::index::relative_slash_path(root, &path));
            if entry.file_type().unwrap().is_dir() {
                stack.push(path);
            }
        }
    }
    found
}

#[test]
fn similarity_comparisons_stop_at_the_cap() {
    let gone: Vec<Gone> = (0..101)
        .map(|i| Gone {
            path: PathBuf::from(format!("g{i:03}.md")),
            relative_path: format!("g{i:03}.md"),
            hash: None,
        })
        .collect();
    let sketches: HashMap<String, Sketch> = gone
        .iter()
        .map(|g| {
            (
                g.relative_path.clone(),
                Sketch::of(g.relative_path.as_bytes()),
            )
        })
        .collect();
    let appeared: Vec<Appeared> = (0..101)
        .map(|i| Appeared {
            path: PathBuf::from(format!("a{i:03}.md")),
            relative_path: format!("a{i:03}.md"),
            hash: content_hash(&[i as u8]),
            sketch: Sketch::of(format!("unrelated {i}").as_bytes()),
        })
        .collect();
    let matches = match_renames(&gone, &appeared, &sketches);
    assert_eq!(matches.comparisons, MAX_COMPARISONS);
    assert!(matches.pairs.is_empty());
    // Candidates of another extension are never compared.
    let images: Vec<Appeared> = appeared
        .iter()
        .map(|a| Appeared {
            path: a.path.with_extension("png"),
            ..a.clone()
        })
        .collect();
    assert_eq!(match_renames(&gone, &images, &sketches).comparisons, 0);
}
