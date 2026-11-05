use super::*;
use crate::epoch::EpochCounter;
use crate::store::{NoteChange, NoteRecord, StoreOpen};
use parking_lot::Mutex;
use std::path::Path;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(5);

struct Lib {
    _dir: TempDir,
    store: Store,
    token: EpochToken,
    _counter: EpochCounter,
    notices: Arc<Mutex<Vec<String>>>,
}

fn device() -> Device {
    Device {
        id: "0123456789abcdef0123456789abcdef".into(),
        name: "Test machine".into(),
    }
}

fn lib() -> Lib {
    let dir = TempDir::new().unwrap();
    let StoreOpen::Ready { store, .. } = Store::open(dir.path(), &|_: &Path| None) else {
        panic!("store unavailable");
    };
    let counter = EpochCounter::new();
    Lib {
        _dir: dir,
        store,
        token: counter.token(),
        _counter: counter,
        notices: Arc::new(Mutex::new(Vec::new())),
    }
}

impl Lib {
    fn history(&self, timing: Timing) -> History {
        let notices = self.notices.clone();
        History::start_with(
            self.store.clone(),
            self.token.clone(),
            device(),
            Arc::new(move |message| notices.lock().push(message)),
            timing,
        )
    }

    fn note(&self, path: &str, bytes: &[u8]) {
        self.store
            .apply(
                &self.token,
                vec![NoteChange::Upsert {
                    record: NoteRecord {
                        path: path.into(),
                        size: bytes.len() as u64,
                        mtime_ns: 1,
                        hash: Some(content_hash(bytes)),
                        title: None,
                        offline: false,
                    },
                    sketch: None,
                }],
            )
            .recv()
            .unwrap()
            .unwrap();
    }

    fn versions(&self, path: &str) -> Vec<VersionInfo> {
        versions_of_path(&self.store, path)
            .unwrap()
            .map(|(_, versions)| versions)
            .unwrap_or_default()
    }

    fn texts(&self, path: &str) -> Vec<String> {
        self.versions(path)
            .iter()
            .map(|v| String::from_utf8(content(&self.store, &v.hash).unwrap().unwrap()).unwrap())
            .collect()
    }
}

fn fast() -> Timing {
    Timing {
        save_pause: Duration::from_millis(300),
        batch_window: Duration::from_millis(50),
        ..Timing::default()
    }
}

fn wait_for(lib: &Lib, path: &str, count: usize) -> Vec<VersionInfo> {
    let deadline = Instant::now() + WAIT;
    loop {
        let versions = lib.versions(path);
        if versions.len() >= count || Instant::now() > deadline {
            return versions;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn saves_within_the_typing_pause_make_one_version_with_the_last_text() {
    let lib = lib();
    lib.note("a.md", b"zero");
    let history = lib.history(fast());
    for text in ["one", "two", "three"] {
        history.saved("a.md", text.as_bytes().to_vec());
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(lib.versions("a.md"), []);
    let versions = wait_for(&lib, "a.md", 1);
    assert_eq!(versions.len(), 1);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(lib.texts("a.md"), ["three"]);
    let version = &lib.versions("a.md")[0];
    assert_eq!(version.cause, Cause::Save);
    assert_eq!(version.device_name, "Test machine");
    assert_eq!(version.parent, None);
}

#[test]
fn closing_writes_the_pending_save_at_once() {
    let lib = lib();
    let history = lib.history(Timing {
        save_pause: Duration::from_secs(3600),
        batch_window: Duration::from_secs(3600),
        ..Timing::default()
    });
    history.saved("a.md", b"typed".to_vec());
    history.saved("b.md", b"other".to_vec());
    assert!(history.flush_and_wait(Some("a.md"), WAIT));
    assert_eq!(lib.texts("a.md"), ["typed"]);
    assert_eq!(lib.versions("b.md"), []);
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(lib.texts("b.md"), ["other"]);
    history.saved("b.md", b"last".to_vec());
    drop(history);
    assert_eq!(wait_for(&lib, "b.md", 2).len(), 2);
}

#[test]
fn identical_successive_contents_make_one_version() {
    let lib = lib();
    let history = lib.history(fast());
    history.record(Request::new("a.md", b"same".to_vec(), Cause::External));
    history.record(Request::new("a.md", b"same".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    history.saved("a.md", b"same".to_vec());
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(lib.versions("a.md").len(), 1);
    history.record(Request::new("a.md", b"other".to_vec(), Cause::Save));
    history.record(Request::new("a.md", b"same".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(lib.texts("a.md"), ["same", "other", "same"]);
}

#[test]
fn an_outside_change_is_a_version_whose_parent_is_the_known_head() {
    let lib = lib();
    let history = lib.history(fast());
    history.saved("a.md", b"mine".to_vec());
    history.record(Request::new("a.md", b"theirs".to_vec(), Cause::External));
    assert!(history.flush_and_wait(None, WAIT));
    let versions = lib.versions("a.md");
    assert_eq!(lib.texts("a.md"), ["theirs", "mine"]);
    assert_eq!(versions[0].cause, Cause::External);
    assert_eq!(versions[0].parent, Some(content_hash(b"mine")));
    assert_eq!(versions[1].seq, 1);
    assert_eq!(versions[0].seq, 2);
}

#[test]
fn a_first_version_only_while_the_note_has_none() {
    let lib = lib();
    let history = lib.history(fast());
    let mut first = Request::new("a.md", b"as found".to_vec(), Cause::Save);
    first.only_if_empty = true;
    history.record(first.clone());
    assert!(history.flush_and_wait(None, WAIT));
    first.bytes = b"found again".to_vec();
    history.record(first);
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(lib.texts("a.md"), ["as found"]);
}

#[test]
fn versions_are_deltas_in_bounded_chains_and_all_read_back() {
    let lib = lib();
    let history = lib.history(fast());
    let mut text = String::from("# Title\n\n");
    let mut expected = Vec::new();
    for n in 0..(2 * MAX_CHAIN + 5) {
        text.push_str(&format!("Paragraph {n} of a note that grows a little.\n"));
        expected.push(text.clone());
        history.record(Request::new("a.md", text.clone().into_bytes(), Cause::Save));
    }
    assert!(history.flush_and_wait(None, WAIT));
    let mut texts = lib.texts("a.md");
    texts.reverse();
    assert_eq!(texts, expected);
    let (full, deltas, longest) = lib
        .store
        .read(|c| {
            let full: i64 = c.query_row("SELECT COUNT(*) FROM blob WHERE codec = 1", [], |r| {
                r.get(0)
            })?;
            let deltas: i64 =
                c.query_row("SELECT COUNT(*) FROM blob WHERE codec = 2", [], |r| {
                    r.get(0)
                })?;
            let mut longest = 0;
            let mut statement = c.prepare("SELECT hash FROM blob")?;
            let hashes: Vec<Vec<u8>> = statement
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for hash in hashes {
                longest = longest.max(chain_length(c, &hash_of(hash).unwrap())?);
            }
            Ok((full, deltas, longest))
        })
        .unwrap();
    assert_eq!(
        full, 3,
        "a full blob first and after each {MAX_CHAIN} deltas"
    );
    assert_eq!(deltas as usize, expected.len() - 3);
    assert_eq!(longest, MAX_CHAIN);
}

#[test]
fn a_full_disk_keeps_the_note_and_says_so_once() {
    let lib = lib();
    let history = lib.history(fast());
    history.record(Request::new("a.md", b"fits".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    lib.store
        .write(&lib.token, |t| {
            let pages: i64 = t.pragma_query_value(None, "page_count", |r| r.get(0))?;
            t.pragma_update(None, "max_page_count", pages)?;
            Ok(())
        })
        .recv()
        .unwrap()
        .unwrap();
    let big: Vec<u8> = (0..200_000u32).flat_map(|n| n.to_le_bytes()).collect();
    for _ in 0..2 {
        history.record(Request::new("a.md", big.clone(), Cause::Save));
        assert!(history.flush_and_wait(None, WAIT));
    }
    assert_eq!(lib.versions("a.md").len(), 1);
    let notices = lib.notices.lock().clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with("Cerata couldn't save this note's history: ")
            && notices[0].contains("full"),
        "{notices:?}"
    );
}

#[test]
fn a_pending_save_follows_a_rename_of_cerata() {
    let lib = lib();
    let history = lib.history(Timing {
        save_pause: Duration::from_secs(3600),
        batch_window: Duration::from_millis(10),
        ..Timing::default()
    });
    history.saved("old.md", b"typed".to_vec());
    history.renamed("old.md", "new.md");
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(lib.versions("old.md"), []);
    assert_eq!(lib.texts("new.md"), ["typed"]);
}

#[test]
fn a_crash_between_a_write_and_its_version_is_found() {
    let lib = lib();
    let history = lib.history(fast());
    lib.note("a.md", b"one");
    history.record(Request::new("a.md", b"one".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(unversioned_heads(&lib.store).unwrap(), Vec::<String>::new());
    lib.note("a.md", b"two");
    assert_eq!(unversioned_heads(&lib.store).unwrap(), ["a.md"]);
}

#[test]
fn deleted_notes_with_versions_are_listed() {
    let lib = lib();
    let history = lib.history(fast());
    lib.note("gone.md", b"bye");
    lib.note("never.md", b"no history");
    history.record(Request::new("gone.md", b"bye".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    lib.store
        .apply(
            &lib.token,
            vec![
                NoteChange::Delete {
                    path: "gone.md".into(),
                },
                NoteChange::Delete {
                    path: "never.md".into(),
                },
            ],
        )
        .recv()
        .unwrap()
        .unwrap();
    let deleted = deleted_notes(&lib.store).unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].path, "gone.md");
    assert_eq!(deleted[0].versions, 1);
    assert_eq!(deleted[0].head, content_hash(b"bye"));
}

#[test]
fn a_pending_save_of_a_note_gone_since_joins_its_history() {
    let lib = lib();
    let history = lib.history(Timing {
        save_pause: Duration::from_secs(3600),
        batch_window: Duration::from_millis(10),
        ..Timing::default()
    });
    lib.note("gone.md", b"one");
    history.record(Request::new("gone.md", b"one".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    lib.note("gone.md", b"two");
    history.saved("gone.md", b"two".to_vec());
    lib.store
        .apply(
            &lib.token,
            vec![NoteChange::Delete {
                path: "gone.md".into(),
            }],
        )
        .recv()
        .unwrap()
        .unwrap();
    assert!(history.flush_and_wait(None, WAIT));
    assert!(lib.store.note("gone.md").unwrap().is_none(), "phantom note");
    let deleted = deleted_notes(&lib.store).unwrap();
    assert_eq!(deleted.len(), 1, "{deleted:?}");
    assert_eq!(deleted[0].versions, 2);
    assert_eq!(deleted[0].head, content_hash(b"two"));

    lib.note("old.md", b"three");
    history.saved("old.md", b"three".to_vec());
    lib.store
        .apply(
            &lib.token,
            vec![NoteChange::Rename {
                from: "old.md".into(),
                record: NoteRecord {
                    path: "new.md".into(),
                    size: 5,
                    mtime_ns: 2,
                    hash: Some(content_hash(b"three")),
                    title: None,
                    offline: false,
                },
                sketch: None,
            }],
        )
        .recv()
        .unwrap()
        .unwrap();
    assert!(history.flush_and_wait(None, WAIT));
    assert!(lib.store.note("old.md").unwrap().is_none(), "phantom note");
    assert_eq!(lib.texts("new.md"), ["three"]);
}

#[test]
fn pinning_is_kept() {
    let lib = lib();
    let history = lib.history(fast());
    history.record(Request::new("a.md", b"x".to_vec(), Cause::Save));
    assert!(history.flush_and_wait(None, WAIT));
    let v = lib.versions("a.md")[0].clone();
    set_pinned(&lib.store, &lib.token, v.note, v.seq, true)
        .recv()
        .unwrap()
        .unwrap();
    assert!(lib.versions("a.md")[0].pinned);
}

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

fn at(path: &str, text: &str, ts: i64) -> Request {
    let mut request = Request::new(path, text.as_bytes().to_vec(), Cause::Save);
    request.ts = ts;
    request
}

fn pass(lib: &Lib, now: i64) -> gc::Removed {
    let (sender, removed) = std::sync::mpsc::channel();
    lib.store
        .write(&lib.token, move |t| {
            let _ = sender.send(gc::retention_pass(t, now)?);
            Ok(())
        })
        .recv()
        .unwrap()
        .unwrap();
    removed.recv().unwrap()
}

#[test]
fn a_pass_applies_the_tiers_keeps_pins_and_reads_back_what_stays() {
    let lib = lib();
    let history = lib.history(fast());
    let old_day = (NOW - 90 * DAY).div_euclid(DAY) * DAY;
    let hour = (NOW - 2 * DAY).div_euclid(HOUR) * HOUR;
    let requests = [
        at("a.md", "v1 old morning", old_day + HOUR),
        at("a.md", "v2 old noon", old_day + 12 * HOUR),
        at("a.md", "v3 old evening", old_day + 20 * HOUR),
        at("a.md", "v4 hour start", hour + 60),
        at("a.md", "v5 hour end", hour + 3000),
        at("a.md", "v6 recent", NOW - 600),
        at("a.md", "v7 latest", NOW - 60),
    ];
    for request in requests {
        history.record(request);
    }
    assert!(history.flush_and_wait(None, WAIT));
    let pinned = lib
        .versions("a.md")
        .into_iter()
        .find(|v| v.seq == 1)
        .unwrap();
    set_pinned(&lib.store, &lib.token, pinned.note, pinned.seq, true)
        .recv()
        .unwrap()
        .unwrap();
    let removed = pass(&lib, NOW);
    assert_eq!(removed.versions, 2, "the old noon and the hour's start");
    assert_eq!(
        lib.texts("a.md"),
        [
            "v7 latest",
            "v6 recent",
            "v5 hour end",
            "v3 old evening",
            "v1 old morning"
        ]
    );
    assert_eq!(removed.blobs, 2);
    let left: i64 = lib
        .store
        .read(|c| c.query_row("SELECT COUNT(*) FROM blob", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(left, 5, "only the blobs of the kept versions remain");
    assert!(!lib.store.read(|c| gc::pass_due(c, NOW + HOUR)).unwrap());
    assert!(lib.store.read(|c| gc::pass_due(c, NOW + DAY)).unwrap());
}

#[test]
fn a_blob_a_note_still_shows_is_kept() {
    let lib = lib();
    let history = lib.history(fast());
    history.record(at("a.md", "kept as the head", NOW - 90 * DAY));
    history.record(at("a.md", "newer", NOW - 89 * DAY));
    assert!(history.flush_and_wait(None, WAIT));
    lib.note("a.md", b"kept as the head");
    let id = lib.versions("a.md")[0].note;
    let removed = history.purge(gc::Purge::Note(id)).recv().unwrap().unwrap();
    assert_eq!(
        removed,
        gc::Removed {
            versions: 2,
            blobs: 1
        }
    );
    assert_eq!(
        content(&lib.store, &content_hash(b"kept as the head"))
            .unwrap()
            .as_deref(),
        Some(&b"kept as the head"[..])
    );
}

fn chunks(stored: &[u8]) -> Vec<&[u8]> {
    stored
        .get(8..)
        .unwrap_or_default()
        .as_chunks::<32>()
        .0
        .iter()
        .map(|chunk| chunk.as_slice())
        .collect()
}

fn store_files(lib: &Lib) -> Vec<(String, Vec<u8>)> {
    let path = lib.store.path().to_path_buf();
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let name = std::path::PathBuf::from(name);
            (
                name.display().to_string(),
                std::fs::read(&name).unwrap_or_default(),
            )
        })
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn assert_absent(lib: &Lib, witness: &[u8], stored: &[u8]) {
    for (name, bytes) in store_files(lib) {
        assert!(!contains(&bytes, witness), "the witness is still in {name}");
        for chunk in chunks(stored) {
            assert!(
                !contains(&bytes, chunk),
                "bytes of the purged blob are still in {name}"
            );
        }
    }
}

fn witness_note(lib: &Lib, history: &History) -> (Vec<u8>, Vec<u8>, i64) {
    let witness = b"WITNESS-6c1f0e2b-only-in-the-purged-version-9a7d".to_vec();
    let mut secret = String::from("# Diary\n\n");
    for n in 0..200 {
        secret.push_str(&format!("Line {n} of a private entry.\n"));
    }
    secret.push_str(std::str::from_utf8(&witness).unwrap());
    secret.push('\n');
    history.record(at("a.md", &secret, NOW - 3 * DAY));
    history.record(at("a.md", "# Diary\n\nNothing to see.\n", NOW - DAY));
    assert!(history.flush_and_wait(None, WAIT));
    let stored: Vec<u8> = lib
        .store
        .read(|c| {
            c.query_row(
                "SELECT bytes FROM blob WHERE hash = ?1",
                [content_hash(secret.as_bytes()).to_vec()],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(chunks(&stored).len() > 3);
    let seen = store_files(lib)
        .iter()
        .any(|(_, bytes)| contains(bytes, chunks(&stored)[0]));
    assert!(seen, "the check must find the blob before the purge");
    let id = lib.versions("a.md")[0].note;
    (witness, stored, id)
}

#[test]
fn a_purge_leaves_no_byte_of_the_purged_content_in_the_store_files() {
    let lib = lib();
    let history = lib.history(fast());
    let note = lib._dir.path().join("a.md");
    std::fs::write(&note, "# Diary\n\nNothing to see.\n").unwrap();
    let (witness, stored, _) = witness_note(&lib, &history);
    let before = NOW - 2 * DAY;
    assert_eq!(history.purge_count(gc::Purge::Before(before)).unwrap(), 1);
    let removed = history
        .purge(gc::Purge::Before(before))
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(
        removed,
        gc::Removed {
            versions: 1,
            blobs: 1
        }
    );
    assert_eq!(lib.texts("a.md"), ["# Diary\n\nNothing to see.\n"]);
    assert_absent(&lib, &witness, &stored);
    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        "# Diary\n\nNothing to see.\n"
    );
    assert!(
        lib.store
            .read(|c| gc::meta_get(c, gc::VACUUM_OWED))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_purge_interrupted_before_its_vacuum_vacuums_at_the_next_opening() {
    let lib = lib();
    let history = lib.history(fast());
    let (witness, stored, id) = witness_note(&lib, &history);
    lib.store
        .write(&lib.token, move |t| {
            gc::purge_versions(t, gc::Purge::Note(id)).map(|_| ())
        })
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(lib.versions("a.md"), []);
    assert_eq!(
        lib.store
            .read(|c| gc::meta_get(c, gc::VACUUM_OWED))
            .unwrap(),
        Some(1)
    );
    drop(history);
    let history = lib.history(fast());
    assert!(history.flush_and_wait(None, WAIT));
    let deadline = Instant::now() + WAIT;
    while lib
        .store
        .read(|c| gc::meta_get(c, gc::VACUUM_OWED))
        .unwrap()
        .is_some()
    {
        assert!(Instant::now() < deadline, "the owed vacuum never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_absent(&lib, &witness, &stored);
}

#[test]
fn closing_the_library_runs_a_retention_pass() {
    let lib = lib();
    let history = lib.history(fast());
    let day = (unix_now() - 60 * DAY).div_euclid(DAY) * DAY;
    history.record(at("a.md", "early", day + HOUR));
    history.record(at("a.md", "late", day + 2 * HOUR));
    assert!(history.flush_and_wait(None, WAIT));
    assert_eq!(lib.versions("a.md").len(), 2);
    assert!(history.close(WAIT));
    let deadline = Instant::now() + WAIT;
    while lib.versions("a.md").len() != 1 {
        assert!(Instant::now() < deadline, "no pass at close");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(lib.texts("a.md"), ["late"]);
}

#[test]
fn no_merge_scenario_loses_a_side_in_the_history() {
    let scenarios: [(&str, &str, &str, &str); 6] = [
        ("disjoint", "a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n"),
        ("overlap", "a\nb\n", "a\nmine\n", "a\ntheirs\n"),
        ("same edit", "a\nb\n", "a\nboth\n", "a\nboth\n"),
        ("crlf and lf", "a\nb\n", "a\nb\nmine\n", "a\r\nb\r\n"),
        ("both append", "a\n", "a\nmine end\n", "a\ntheirs end\n"),
        ("quit in conflict", "x\n", "mine\n", "theirs\n"),
    ];
    let lib = lib();
    let history = lib.history(fast());
    for (name, base, ours, theirs) in scenarios {
        let path = format!("{}.md", name.replace(' ', "-"));
        history.record(Request::new(
            path.as_str(),
            base.as_bytes().to_vec(),
            Cause::Save,
        ));
        history.record(Request::new(
            path.as_str(),
            theirs.as_bytes().to_vec(),
            Cause::External,
        ));
        history.record(Request::new(
            path.as_str(),
            ours.as_bytes().to_vec(),
            Cause::Save,
        ));
        let merged = crate::merge::merge(base, ours, theirs);
        let result = if name == "quit in conflict" {
            ours.to_owned()
        } else {
            merged.text().unwrap_or_else(|| {
                merged.resolve(&vec![crate::merge::Choice::Both; merged.conflicts()])
            })
        };
        let cause = if name == "quit in conflict" {
            Cause::Save
        } else {
            Cause::Merge
        };
        history.record(Request::new(path.as_str(), result.into_bytes(), cause));
        assert!(history.flush_and_wait(None, WAIT));
        let texts = lib.texts(&path);
        for side in [base, ours, theirs] {
            assert!(
                texts.iter().any(|t| t == side),
                "{name}: {side:?} lost from {texts:?}"
            );
        }
    }
    if let Some(text) = crate::merge::merge("a\nb\n", "a\nboth\n", "a\nboth\n").text() {
        assert_eq!(
            text.matches("both").count(),
            1,
            "the same edit is kept once"
        );
    }
}
