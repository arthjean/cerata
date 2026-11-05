#[path = "support/corpus.rs"]
mod corpus;

use cerata_core::epoch::EpochCounter;
use cerata_core::index::index_workspace;
use cerata_core::reconcile::{
    FileAccess, OsFiles, PassState, ReconcileContext, is_offline_metadata, note_content, run_pass,
};
use cerata_core::self_writes::SelfWriteLog;
use cerata_core::store::{NoteChange, NoteRecord, Store, StoreOpen};
use cerata_core::workspace::index_from_store;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const WARMUPS: usize = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs = |rest: &[String]| rest.first().and_then(|r| r.parse().ok()).unwrap_or(5);
    let result = match args.as_slice() {
        [command, dir, count] if command == "generate" => count
            .parse()
            .map_err(|error| format!("invalid file count: {error}"))
            .and_then(|count| corpus::generate(Path::new(dir), count).map_err(|e| e.to_string())),
        [command, dir, rest @ ..] if command == "store" => store(Path::new(dir), runs(rest)),
        [command, dir, rest @ ..] if command == "reconcile" => {
            reconcile(Path::new(dir), runs(rest))
        }
        _ => Err(
            "usage: store_bench generate <dir> <files> | store <dir> [runs] | reconcile <dir> [runs]"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn open_store(dir: &Path) -> Result<Store, String> {
    match Store::open(dir, &|_| None) {
        StoreOpen::Ready { store, .. } => Ok(store),
        StoreOpen::Unavailable { notice } => Err(format!("no store: {notice:?}")),
    }
}

fn summary(mut timings: Vec<Duration>) -> String {
    timings.sort();
    let at = |rank: f64| {
        let index = ((rank * timings.len() as f64).ceil() as usize).clamp(1, timings.len()) - 1;
        timings[index]
    };
    format!(
        "median {:?} (min {:?}, max {:?}, p99 {:?}, {} runs)",
        at(0.5),
        timings[0],
        timings[timings.len() - 1],
        at(0.99),
        timings.len()
    )
}

fn store(root: &Path, runs: usize) -> Result<(), String> {
    let token = EpochCounter::new().token();
    let index = index_workspace(root, &token).map_err(|e| e.to_string())?;
    let changes: Vec<NoteChange> = index
        .files
        .iter()
        .map(|file| {
            let bytes = std::fs::read(&file.path).unwrap_or_default();
            let content = note_content(&bytes);
            NoteChange::Upsert {
                record: NoteRecord {
                    path: file.relative_path.clone(),
                    size: file.size,
                    mtime_ns: file.modified_ns,
                    hash: Some(content.hash),
                    title: content.title,
                    offline: false,
                },
                sketch: Some(content.sketch),
            }
        })
        .collect();
    let count = changes.len();
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let store = open_store(dir.path())?;
    let started = Instant::now();
    store
        .apply(&token, changes)
        .recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    println!("inserted {count} notes in {:?}", started.elapsed());
    drop(store);

    let mut opens = Vec::new();
    for run in 0..WARMUPS + runs {
        let started = Instant::now();
        let store = open_store(dir.path())?;
        let elapsed = started.elapsed();
        drop(store);
        if run >= WARMUPS {
            opens.push(elapsed);
        }
    }
    println!("open: {}", summary(opens));

    let store = open_store(dir.path())?;
    let mut reads = Vec::new();
    let mut titled = 0;
    for run in 0..WARMUPS + runs {
        let started = Instant::now();
        let notes = store.live_notes().map_err(|e| e.to_string())?;
        let elapsed = started.elapsed();
        titled = notes
            .iter()
            .filter(|note| note.record.title.is_some())
            .count();
        if run >= WARMUPS {
            reads.push(elapsed);
        }
    }
    println!("read {titled} titles: {}", summary(reads));

    let target = store
        .live_notes()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .ok_or("empty store")?;
    let mut updates = Vec::new();
    for i in 0..200u64 {
        let mut record = target.record.clone();
        record.mtime_ns += i + 1;
        record.title = Some(format!("Title {i}"));
        let started = Instant::now();
        store
            .apply(
                &token,
                vec![NoteChange::Upsert {
                    record,
                    sketch: None,
                }],
            )
            .recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        updates.push(started.elapsed());
    }
    println!("update one row: {}", summary(updates));
    drop(store);
    let size = |name: &str| std::fs::metadata(dir.path().join(name)).map_or(0, |m| m.len());
    println!(
        "file size after close: store.sqlite {} bytes, -wal {} bytes",
        size("store.sqlite"),
        size("store.sqlite-wal")
    );
    Ok(())
}

#[derive(Default)]
struct Counting(AtomicUsize);

impl FileAccess for Counting {
    fn is_offline(&self, _: &Path, metadata: &std::fs::Metadata) -> bool {
        is_offline_metadata(metadata)
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        OsFiles.read(path)
    }
}

fn reconcile(root: &Path, runs: usize) -> Result<(), String> {
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let store = open_store(dir.path())?;
    let counter = EpochCounter::new();
    let files = Arc::new(Counting::default());
    let (events, _received) = std::sync::mpsc::channel();
    let ctx = ReconcileContext {
        root: root.clone(),
        token: counter.token(),
        index: Arc::new(parking_lot::Mutex::new(cerata_core::index::LiveIndex::new(
            &root,
        ))),
        ignore: Arc::new(parking_lot::RwLock::new(Arc::new(
            cerata_core::ignore::WorkspaceIgnore::bootstrap(&root),
        ))),
        index_ready: Arc::new(AtomicBool::new(false)),
        store: Some(store.clone()),
        history: None,
        self_writes: Arc::new(SelfWriteLog::new()),
        files: files.clone(),
        global_gitignore: cerata_core::ignore::global_gitignore_path(),
        events,
    };
    let mut state = PassState::default();
    let first = run_pass(&ctx, &mut state, &mut || {}).map_err(|e| e.to_string())?;
    println!(
        "first pass (no store): {} notes, {} opened, {:?}",
        first.notes, first.opened, first.duration
    );

    let mut passes = Vec::new();
    let mut opened = 0;
    for run in 0..WARMUPS + runs {
        let before = files.0.load(Ordering::SeqCst);
        let report = run_pass(&ctx, &mut state, &mut || {}).map_err(|e| e.to_string())?;
        if run >= WARMUPS {
            passes.push(report.duration);
            opened += files.0.load(Ordering::SeqCst) - before;
        }
    }
    println!(
        "unchanged pass: {} ({opened} files opened in the timed runs)",
        summary(passes)
    );

    let mut loads = Vec::new();
    let mut count = 0;
    for run in 0..WARMUPS + runs {
        let started = Instant::now();
        let notes = store.live_notes().map_err(|e| e.to_string())?;
        let index = index_from_store(&root, &notes);
        let elapsed = started.elapsed();
        count = index.len();
        if run >= WARMUPS {
            loads.push(elapsed);
        }
    }
    println!(
        "index from the store ({count} notes, titles included): {}",
        summary(loads)
    );
    Ok(())
}
