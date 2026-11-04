//! Reconciliation (US-084) and renames made outside Cerata (US-085).
//!
//! The watcher misses changes: Windows drops events when its buffer
//! overflows (notify 8.2.1 never reads `bytes_written`, `windows.rs:333`),
//! Linux may watch only the folders of open files, and nothing watches
//! while Cerata is closed. One routine catches up: it walks the library
//! with the index's ignore rules, compares each note's `(size, mtime_ns)`
//! with what the index knows, reads and hashes only the new or changed
//! files, and reports what changed with the watcher's own events. It runs
//! at the end of the first frame, when the window gets the focus back (at
//! most every 30 s), after any watcher error, and every 5 minutes unless a
//! pass ran in the last minute.
//!
//! A note that disappeared and one that appeared in the same pass with the
//! same BLAKE3 hash are one note renamed; failing that, a pair whose line
//! content is at least 50 % alike (git's rename score) is one too. The
//! store row keeps its id and takes the new path. Nothing on disk is moved
//! or written by a detection.

use crate::epoch::EpochToken;
use crate::fs::titles::title_from_bytes;
use crate::history::{Cause, History, Request, conflict_version, unversioned_heads};
use crate::ignore::WorkspaceIgnore;
use crate::index::{IndexedFile, LiveIndex, WalkEntry, modified_ns, walk};
use crate::markdown::is_markdown_path;
use crate::self_writes::SelfWriteLog;
use crate::similarity::Sketch;
use crate::store::{ContentHash, NoteChange, NoteRecord, Store, content_hash};
use crate::watcher::{FileChange, WatchEnvelope, WatchEvent};
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The focus brings a pass at most this often.
pub const FOCUS_INTERVAL: Duration = Duration::from_secs(30);
/// Period of the timer trigger.
pub const TIMER_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The timer skips its pass when one ran this recently.
pub const TIMER_SKIP: Duration = Duration::from_secs(60);
/// A file modified this close to the walk is checked again next pass, as
/// the sidebar does (`sidebar/mod.rs`, settled titles).
pub const SETTLE: Duration = Duration::from_secs(2);
/// A library opened from its store waits this long for the first frame
/// before its first pass starts anyway.
pub const FIRST_RENDER_FALLBACK: Duration = Duration::from_secs(2);
/// Least similarity of two notes for a rename (git's default, 50 %).
pub const SIMILARITY_THRESHOLD: f64 = 0.5;
/// Most similarity comparisons per pass (100 notes gone × 100 appeared).
pub const MAX_COMPARISONS: usize = 100 * 100;
/// Files read per step of the first pass of a library without a store.
const CONTENT_CHUNK: usize = 256;

/// Windows: the file's content is not on this device (cloud placeholder).
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// Windows: the file's data is offline storage.
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;

/// What started a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileTrigger {
    FirstRender,
    Focus,
    WatchError,
    Timer,
}

/// How a pass reads the disk, injectable so tests count every open.
pub trait FileAccess: Send + Sync {
    /// `true` for a file that must never be opened: its content is not on
    /// this device, and reading it would download it.
    fn is_offline(&self, path: &Path, metadata: &Metadata) -> bool;
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
}

/// The real file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsFiles;

impl FileAccess for OsFiles {
    fn is_offline(&self, _path: &Path, metadata: &Metadata) -> bool {
        is_offline_metadata(metadata)
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}

/// `true` when Windows marks the file `RECALL_ON_DATA_ACCESS` or
/// `OFFLINE`. The attributes come from the directory listing, so nothing
/// is opened to know it. Always `false` elsewhere.
pub fn is_offline_metadata(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_OFFLINE)
            != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// What one read of a note tells.
#[derive(Debug, Clone)]
pub struct NoteContent {
    pub hash: ContentHash,
    pub title: Option<String>,
    pub sketch: Sketch,
}

pub fn note_content(bytes: &[u8]) -> NoteContent {
    NoteContent {
        hash: content_hash(bytes),
        title: title_from_bytes(bytes),
        sketch: Sketch::of(bytes),
    }
}

/// The store's record of an index entry.
pub fn note_record(file: &IndexedFile, offline: bool) -> NoteRecord {
    NoteRecord {
        path: file.relative_path.clone(),
        size: file.size,
        mtime_ns: file.modified_ns,
        hash: file.hash,
        title: file.title.clone(),
        offline,
    }
}

/// `root` joined with a `/`-separated relative path, with the OS separator.
pub fn path_from_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

// Renames ---------------------------------------------------------------

/// A note that disappeared, with the hash last known.
#[derive(Debug, Clone)]
pub struct Gone {
    pub path: PathBuf,
    pub relative_path: String,
    pub hash: Option<ContentHash>,
}

/// A note that appeared, read.
#[derive(Debug, Clone)]
pub struct Appeared {
    pub path: PathBuf,
    pub relative_path: String,
    pub hash: ContentHash,
    pub sketch: Sketch,
}

/// Pairs `(gone, appeared)` of indices recognized as one renamed note.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RenameMatches {
    pub pairs: Vec<(usize, usize)>,
    /// Similarity comparisons made, at most [`MAX_COMPARISONS`].
    pub comparisons: usize,
}

/// Matches disappeared notes with appeared ones: same hash first, the
/// appeared ones in lexicographic order of their paths (so of two copies
/// of a renamed note the first inherits its identity); then, among those
/// left, the most similar pairs of the same extension scoring at least
/// [`SIMILARITY_THRESHOLD`], comparing gone notes whose sketch is in
/// `sketches`, at most [`MAX_COMPARISONS`] times.
pub fn match_renames(
    gone: &[Gone],
    appeared: &[Appeared],
    sketches: &HashMap<String, Sketch>,
) -> RenameMatches {
    let mut gone_order: Vec<usize> = (0..gone.len()).collect();
    gone_order.sort_by(|a, b| gone[*a].relative_path.cmp(&gone[*b].relative_path));
    let mut appeared_order: Vec<usize> = (0..appeared.len()).collect();
    appeared_order.sort_by(|a, b| appeared[*a].relative_path.cmp(&appeared[*b].relative_path));
    let mut gone_taken = vec![false; gone.len()];
    let mut appeared_taken = vec![false; appeared.len()];
    let mut matches = RenameMatches::default();

    let mut by_hash: HashMap<ContentHash, std::collections::VecDeque<usize>> = HashMap::new();
    for &g in &gone_order {
        if let Some(hash) = gone[g].hash {
            by_hash.entry(hash).or_default().push_back(g);
        }
    }
    for &a in &appeared_order {
        if let Some(g) = by_hash
            .get_mut(&appeared[a].hash)
            .and_then(std::collections::VecDeque::pop_front)
        {
            gone_taken[g] = true;
            appeared_taken[a] = true;
            matches.pairs.push((g, a));
        }
    }

    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    'outer: for &g in &gone_order {
        if gone_taken[g] {
            continue;
        }
        let Some(sketch) = sketches.get(&gone[g].relative_path) else {
            continue;
        };
        let ext = extension(&gone[g].path);
        for &a in &appeared_order {
            if appeared_taken[a] || extension(&appeared[a].path) != ext {
                continue;
            }
            if matches.comparisons == MAX_COMPARISONS {
                break 'outer;
            }
            matches.comparisons += 1;
            let score = sketch.similarity(&appeared[a].sketch);
            if score >= SIMILARITY_THRESHOLD {
                candidates.push((score, g, a));
            }
        }
    }
    candidates.sort_by(|x, y| {
        y.0.total_cmp(&x.0)
            .then_with(|| {
                appeared[x.2]
                    .relative_path
                    .cmp(&appeared[y.2].relative_path)
            })
            .then_with(|| gone[x.1].relative_path.cmp(&gone[y.1].relative_path))
    });
    for (_, g, a) in candidates {
        if !gone_taken[g] && !appeared_taken[a] {
            gone_taken[g] = true;
            appeared_taken[a] = true;
            matches.pairs.push((g, a));
        }
    }
    matches
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

// One pass --------------------------------------------------------------

/// Everything a pass reads and updates.
#[derive(Clone)]
pub struct ReconcileContext {
    /// Canonical library root.
    pub root: PathBuf,
    pub token: EpochToken,
    pub index: Arc<Mutex<LiveIndex>>,
    pub ignore: Arc<RwLock<Arc<WorkspaceIgnore>>>,
    /// Set once an index (from the store or a walk) is published.
    pub index_ready: Arc<AtomicBool>,
    pub store: Option<Store>,
    /// The library's history: what changed outside Cerata becomes a
    /// version (US-089).
    pub history: Option<History>,
    pub self_writes: Arc<SelfWriteLog>,
    pub files: Arc<dyn FileAccess>,
    pub global_gitignore: Option<PathBuf>,
    pub events: Sender<WatchEnvelope>,
}

impl ReconcileContext {
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

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassReport {
    /// The first pass of a library without an index: the walk published
    /// the index, then every note was read.
    pub initial: bool,
    pub notes: usize,
    /// Files opened.
    pub opened: usize,
    pub created: usize,
    pub modified: usize,
    pub removed: usize,
    pub renamed: usize,
    /// Versions recorded for changes made outside Cerata.
    pub versions: usize,
    pub duration: Duration,
}

/// Why a pass stopped without applying anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PassError {
    #[error("reconciliation canceled: the library changed")]
    Canceled,
    #[error("the library folder is gone or unreadable")]
    RootUnavailable,
}

/// State kept from one pass to the next.
#[derive(Debug, Default)]
pub struct PassState {
    /// Files modified too close to the last walk to be trusted.
    recheck: HashSet<PathBuf>,
    /// The notes written without their version were looked for (once per
    /// opening of the library).
    heads_checked: bool,
}

struct Seen {
    path: PathBuf,
    metadata: Option<Metadata>,
}

/// Runs one pass. `on_published` is called once the index is current
/// (after the walk on a first pass, else after the changes are applied).
pub fn run_pass(
    ctx: &ReconcileContext,
    state: &mut PassState,
    on_published: &mut dyn FnMut(),
) -> Result<PassReport, PassError> {
    let started = Instant::now();
    let known: HashMap<PathBuf, IndexedFile> = {
        let index = ctx.index.lock();
        index
            .files()
            .map(|file| (file.path.clone(), file.clone()))
            .collect()
    };
    let walk_started = SystemTime::now();
    let notes = Mutex::new(Vec::new());
    let others = Mutex::new(Vec::new());
    let walked = walk(
        &ctx.root,
        ctx.global_gitignore.as_deref(),
        &ctx.token,
        &|entry: &WalkEntry| {
            if !entry.file_type.is_file() {
                return;
            }
            if is_markdown_path(&entry.path) {
                notes.lock().push(Seen {
                    path: entry.path.clone(),
                    metadata: entry.metadata.clone(),
                });
            } else {
                others.lock().push(entry.path.clone());
            }
        },
    )
    .map_err(|_| PassError::Canceled)?;
    if !ctx.root.is_dir()
        || walked
            .warnings
            .iter()
            .any(|warning| warning.path == ctx.root)
    {
        return Err(PassError::RootUnavailable);
    }
    if ctx.token.is_stale() {
        return Err(PassError::Canceled);
    }
    let mut notes = notes.into_inner();
    notes.sort_by(|a, b| a.path.cmp(&b.path));
    let others = others.into_inner();
    let unreadable: Vec<PathBuf> = walked.warnings.iter().map(|w| w.path.clone()).collect();
    *ctx.ignore.write() = Arc::new(walked.ignore);

    if !ctx.index_ready.load(Ordering::SeqCst) {
        return first_pass(ctx, notes, others, started, on_published);
    }

    let settled_ns = nanos(walk_started).saturating_sub(nanos_of(SETTLE));
    let mut next_recheck = HashSet::new();
    let mut report = PassReport {
        notes: notes.len(),
        ..PassReport::default()
    };
    let mut seen_paths: HashSet<PathBuf> = HashSet::with_capacity(notes.len());
    // (entry, previous, offline): read when `offline` is false.
    let mut to_read: Vec<(IndexedFile, Option<IndexedFile>)> = Vec::new();
    let mut unread: Vec<(IndexedFile, Option<IndexedFile>)> = Vec::new();
    for seen in notes {
        seen_paths.insert(seen.path.clone());
        let mut entry = IndexedFile::new(&ctx.root, &seen.path, seen.metadata.as_ref());
        let offline = seen
            .metadata
            .as_ref()
            .is_some_and(|metadata| ctx.files.is_offline(&seen.path, metadata));
        if entry.modified_ns > settled_ns {
            next_recheck.insert(seen.path.clone());
        }
        let previous = known.get(&seen.path).cloned();
        if offline {
            if let Some(previous) = &previous {
                entry.hash = previous.hash;
                entry.title = previous.title.clone();
            }
            unread.push((entry, previous));
            continue;
        }
        let unchanged = previous.as_ref().is_some_and(|previous| {
            previous.size == entry.size
                && previous.modified_ns == entry.modified_ns
                && previous.hash.is_some()
                && !state.recheck.contains(&seen.path)
        });
        if !unchanged {
            to_read.push((entry, previous));
        }
    }
    state.recheck = next_recheck;

    let gone: Vec<Gone> = known
        .values()
        .filter(|file| !seen_paths.contains(&file.path))
        .filter(|file| !unreadable.iter().any(|dir| file.path.starts_with(dir)))
        .map(|file| Gone {
            path: file.path.clone(),
            relative_path: file.relative_path.clone(),
            hash: file.hash,
        })
        .collect();

    // Read what changed or appeared. A changed entry carries its content
    // when it was read, `None` for an offline file.
    let mut changed: Vec<(IndexedFile, IndexedFile, Option<NoteContent>)> = Vec::new();
    let mut appeared: Vec<Appeared> = Vec::new();
    let mut appeared_files: Vec<(IndexedFile, Option<NoteContent>, bool)> = Vec::new();
    // The bytes read, for the versions of what changed outside Cerata.
    let mut bytes_read: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (mut entry, previous) in to_read {
        if ctx.token.is_stale() {
            return Err(PassError::Canceled);
        }
        report.opened += 1;
        let content = match ctx.files.read(&entry.path) {
            Ok(bytes) => {
                let content = note_content(&bytes);
                if ctx.history.is_some() {
                    bytes_read.insert(entry.path.clone(), bytes);
                }
                content
            }
            Err(error) => {
                log::warn!("cannot read {}: {error}", entry.path.display());
                if previous.is_none() {
                    appeared_files.push((entry, None, false));
                }
                continue;
            }
        };
        entry.hash = Some(content.hash);
        entry.title = content.title.clone();
        match previous {
            Some(previous) => changed.push((entry, previous, Some(content))),
            None => {
                appeared.push(Appeared {
                    path: entry.path.clone(),
                    relative_path: entry.relative_path.clone(),
                    hash: content.hash,
                    sketch: content.sketch.clone(),
                });
                appeared_files.push((entry, Some(content), false));
            }
        }
    }
    for (entry, previous) in unread {
        match previous {
            None => appeared_files.push((entry, None, true)),
            Some(previous)
                if previous.size != entry.size || previous.modified_ns != entry.modified_ns =>
            {
                // Never read: hash and title stay the last known ones.
                changed.push((entry, previous, None));
            }
            Some(_) => {}
        }
    }

    // Renames: the store keeps the sketches of the notes gone.
    let matches = if gone.is_empty() || appeared.is_empty() {
        RenameMatches::default()
    } else {
        let mut wanted: Vec<String> = gone
            .iter()
            .filter(|gone| !appeared.iter().any(|a| Some(a.hash) == gone.hash))
            .map(|gone| gone.relative_path.clone())
            .collect();
        wanted.sort();
        wanted.truncate(100);
        let sketches = match &ctx.store {
            Some(store) => store.sketches(&wanted).unwrap_or_else(|error| {
                log::warn!("rename sketches unavailable: {error}");
                HashMap::new()
            }),
            None => HashMap::new(),
        };
        match_renames(&gone, &appeared, &sketches)
    };
    let renamed_from: HashMap<usize, usize> = matches.pairs.iter().map(|(g, a)| (*a, *g)).collect();
    let gone_renamed: HashSet<usize> = matches.pairs.iter().map(|(g, _)| *g).collect();

    if ctx.token.is_stale() {
        return Err(PassError::Canceled);
    }

    // Apply to the index, then the store, then tell.
    let mut events: Vec<WatchEvent> = Vec::new();
    let mut versions: Vec<Request> = Vec::new();
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut changes: Vec<NoteChange> = Vec::new();
    {
        let mut index = ctx.index.lock();
        for (entry, previous, content) in changed {
            let content_changed = entry.hash != previous.hash;
            let offline = content.is_none();
            changes.push(NoteChange::Upsert {
                record: note_record(&entry, offline),
                sketch: content
                    .filter(|_| content_changed)
                    .map(|content| content.sketch),
            });
            let echo = entry
                .hash
                .is_some_and(|hash| ctx.self_writes.matches(&entry.path, &hash));
            if content_changed && !echo && !offline {
                ctx.self_writes.superseded(&entry.path);
            }
            if content_changed
                && !echo
                && let Some(bytes) = bytes_read.remove(&entry.path)
            {
                versions.push(Request::new(
                    entry.relative_path.clone(),
                    bytes,
                    Cause::External,
                ));
            }
            if content_changed && previous.hash.is_some() && !echo {
                report.modified += 1;
                events.push(WatchEvent::FileChanged {
                    path: entry.path.clone(),
                    change: FileChange::Modified,
                });
            }
            index.upsert(entry);
        }
        // Appeared files, in `appeared` order for the read ones.
        let mut read_index = 0;
        for (entry, content, offline) in appeared_files {
            let pair = if content.is_some() {
                let pair = renamed_from.get(&read_index).copied();
                read_index += 1;
                pair
            } else {
                None
            };
            if !entry.path.is_file() {
                continue;
            }
            match pair {
                Some(g) if !gone[g].path.exists() => {
                    let from = &gone[g];
                    if from.hash != entry.hash
                        && let Some(bytes) = bytes_read.remove(&entry.path)
                    {
                        versions.push(Request::new(
                            entry.relative_path.clone(),
                            bytes,
                            Cause::Rename,
                        ));
                    }
                    index.remove_subtree(&from.path);
                    changes.push(NoteChange::Rename {
                        from: from.relative_path.clone(),
                        record: note_record(&entry, offline),
                        sketch: content.map(|content| content.sketch),
                    });
                    report.renamed += 1;
                    if !ctx.self_writes.is_own_move(&from.path, &entry.path) {
                        events.push(WatchEvent::Renamed {
                            from: from.path.clone(),
                            to: entry.path.clone(),
                        });
                    }
                    dirs.extend(from.path.parent().map(Path::to_path_buf));
                }
                _ => {
                    let echo = entry
                        .hash
                        .is_some_and(|hash| ctx.self_writes.matches(&entry.path, &hash));
                    if !echo
                        && let Some(bytes) = bytes_read
                            .remove(&entry.path)
                            .filter(|bytes| !bytes.is_empty())
                    {
                        // A sync service's conflict copy is a branch of its
                        // original (US-093).
                        let conflict = ctx.store.as_ref().and_then(|store| {
                            conflict_version(store, &ctx.root, &entry.path, bytes.clone())
                        });
                        versions.push(conflict.unwrap_or_else(|| {
                            Request::new(entry.relative_path.clone(), bytes, Cause::External)
                        }));
                    }
                    changes.push(NoteChange::Upsert {
                        record: note_record(&entry, offline),
                        sketch: content.map(|content| content.sketch),
                    });
                    report.created += 1;
                    events.push(WatchEvent::FileChanged {
                        path: entry.path.clone(),
                        change: FileChange::Created,
                    });
                }
            }
            dirs.extend(entry.path.parent().map(Path::to_path_buf));
            index.upsert(entry);
        }
        for (g, gone) in gone.iter().enumerate() {
            if gone_renamed.contains(&g) && !index.contains(&gone.path) {
                continue;
            }
            if gone.path.exists() {
                continue;
            }
            index.remove_subtree(&gone.path);
            changes.push(NoteChange::Delete {
                path: gone.relative_path.clone(),
            });
            report.removed += 1;
            events.push(WatchEvent::FileChanged {
                path: gone.path.clone(),
                change: FileChange::Removed,
            });
            dirs.extend(gone.path.parent().map(Path::to_path_buf));
        }
        index.set_other_files(others);
    }
    commit(ctx, changes)?;
    if let Some(history) = &ctx.history {
        if !state.heads_checked {
            state.heads_checked = true;
            versions.extend(unrecorded_writes(ctx, &versions));
        }
        report.versions = versions.len();
        for request in versions {
            history.record(request);
        }
    }
    for event in events {
        ctx.send(event);
    }
    for dir in dirs {
        ctx.send(WatchEvent::DirectoryChanged { path: dir });
    }
    on_published();
    report.duration = started.elapsed();
    Ok(report)
}

/// Notes whose last write has no version (a crash came between the two,
/// US-089): what the disk holds becomes an outside version, unless it is
/// already among `queued`.
fn unrecorded_writes(ctx: &ReconcileContext, queued: &[Request]) -> Vec<Request> {
    let Some(store) = &ctx.store else {
        return Vec::new();
    };
    let paths = unversioned_heads(store).unwrap_or_else(|error| {
        log::warn!("unversioned notes not checked: {error}");
        Vec::new()
    });
    let mut found = Vec::new();
    for relative in paths {
        if queued.iter().any(|request| request.path == relative) {
            continue;
        }
        let path = path_from_relative(&ctx.root, &relative);
        let offline =
            std::fs::metadata(&path).is_ok_and(|metadata| ctx.files.is_offline(&path, &metadata));
        if offline {
            continue;
        }
        match ctx.files.read(&path) {
            Ok(bytes) => found.push(Request::new(relative, bytes, Cause::External)),
            Err(error) => log::warn!("cannot read {}: {error}", path.display()),
        }
    }
    found
}

/// The first pass of a library the index knows nothing of: the walk is
/// published at once, as before the store, then every note is read in
/// chunks to fill titles, hashes and the store.
fn first_pass(
    ctx: &ReconcileContext,
    notes: Vec<Seen>,
    others: Vec<PathBuf>,
    started: Instant,
    on_published: &mut dyn FnMut(),
) -> Result<PassReport, PassError> {
    let mut live = LiveIndex::new(&ctx.root);
    let mut offline = HashSet::new();
    for seen in &notes {
        if seen
            .metadata
            .as_ref()
            .is_some_and(|metadata| ctx.files.is_offline(&seen.path, metadata))
        {
            offline.insert(seen.path.clone());
        }
        live.upsert(IndexedFile::new(
            &ctx.root,
            &seen.path,
            seen.metadata.as_ref(),
        ));
    }
    // A full walk: the name table is complete (US-087).
    live.set_other_files(others);
    let count = live.len();
    let rows: Vec<NoteChange> = live
        .files()
        .map(|file| NoteChange::Upsert {
            record: note_record(file, offline.contains(&file.path)),
            sketch: None,
        })
        .collect();
    if ctx.token.is_stale() {
        return Err(PassError::Canceled);
    }
    *ctx.index.lock() = live;
    ctx.index_ready.store(true, Ordering::SeqCst);
    ctx.send(WatchEvent::IndexComplete { file_count: count });
    commit(ctx, rows)?;
    on_published();

    let mut report = PassReport {
        initial: true,
        notes: count,
        ..PassReport::default()
    };
    let readable: Vec<&Seen> = notes
        .iter()
        .filter(|seen| !offline.contains(&seen.path))
        .collect();
    for chunk in readable.chunks(CONTENT_CHUNK) {
        if ctx.token.is_stale() {
            return Err(PassError::Canceled);
        }
        let mut read = Vec::with_capacity(chunk.len());
        for seen in chunk {
            report.opened += 1;
            match ctx.files.read(&seen.path) {
                Ok(bytes) => read.push((seen, note_content(&bytes))),
                Err(error) => log::warn!("cannot read {}: {error}", seen.path.display()),
            }
        }
        let mut changes = Vec::with_capacity(read.len());
        {
            let mut index = ctx.index.lock();
            for (seen, content) in read {
                let size = seen.metadata.as_ref().map_or(0, Metadata::len);
                let mtime = seen.metadata.as_ref().map_or(0, modified_ns);
                let Some(file) = index.get_mut(&seen.path) else {
                    continue;
                };
                // The watcher got there first with newer content.
                if file.size != size || file.modified_ns != mtime || file.hash.is_some() {
                    continue;
                }
                file.hash = Some(content.hash);
                file.title = content.title;
                changes.push(NoteChange::Upsert {
                    record: note_record(file, false),
                    sketch: Some(content.sketch),
                });
            }
        }
        commit(ctx, changes)?;
    }
    report.duration = started.elapsed();
    Ok(report)
}

/// Applies `changes` to the store, if there is one, and waits for them.
fn commit(ctx: &ReconcileContext, changes: Vec<NoteChange>) -> Result<(), PassError> {
    let Some(store) = &ctx.store else {
        return Ok(());
    };
    if changes.is_empty() {
        return Ok(());
    }
    match store.apply(&ctx.token, changes).recv() {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err(PassError::Canceled),
        Ok(Err(error)) => {
            log::warn!("store not updated: {error}");
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

fn nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, nanos_of)
}

fn nanos_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

// The triggers ----------------------------------------------------------

#[derive(Default)]
struct LoopState {
    requested: Option<ReconcileTrigger>,
    last_pass: Option<Instant>,
    last_focus: Option<Instant>,
    stopped: bool,
    passes: usize,
}

impl LoopState {
    fn first_pass_done(&self) -> bool {
        self.passes > 0
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<LoopState>,
    wake: Condvar,
}

/// The triggers of one library's passes. Clones share the same loop.
#[derive(Clone, Default)]
pub struct Reconciler {
    shared: Arc<Shared>,
}

impl Reconciler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks for a pass. A focus within [`FOCUS_INTERVAL`] of the last one
    /// is ignored, and so is a first frame once the first pass ran; returns
    /// `false` then.
    pub fn request(&self, trigger: ReconcileTrigger) -> bool {
        let mut state = self.shared.state.lock();
        if trigger == ReconcileTrigger::FirstRender && state.first_pass_done() {
            return false;
        }
        if trigger == ReconcileTrigger::Focus {
            let now = Instant::now();
            if state
                .last_focus
                .is_some_and(|last| now.duration_since(last) < FOCUS_INTERVAL)
            {
                return false;
            }
            state.last_focus = Some(now);
        }
        state.requested = Some(trigger);
        self.shared.wake.notify_all();
        true
    }

    /// Ends the loop at its next wake.
    pub fn stop(&self) {
        self.shared.state.lock().stopped = true;
        self.shared.wake.notify_all();
    }

    /// Passes run so far.
    pub fn passes(&self) -> usize {
        self.shared.state.lock().passes
    }

    /// Runs the passes on the calling thread until stopped or the token
    /// goes stale. With `wait_first_render`, the first pass waits for
    /// [`ReconcileTrigger::FirstRender`] (or [`FIRST_RENDER_FALLBACK`]);
    /// `on_first_pass` runs once the first pass published its index.
    pub fn run(
        &self,
        ctx: &ReconcileContext,
        wait_first_render: bool,
        on_first_pass: Box<dyn FnOnce() + Send>,
    ) {
        let mut pass_state = PassState::default();
        if wait_first_render {
            let deadline = Instant::now() + FIRST_RENDER_FALLBACK;
            let mut state = self.shared.state.lock();
            // Only the first frame (or the fallback) starts it: a focus
            // asked meanwhile is what this pass does anyway.
            while state.requested != Some(ReconcileTrigger::FirstRender)
                && !state.stopped
                && ctx.token.is_current()
            {
                if self
                    .shared
                    .wake
                    .wait_until(&mut state, deadline)
                    .timed_out()
                {
                    break;
                }
            }
            state.requested = None;
        }
        let mut on_first_pass = Some(on_first_pass);
        let mut published = || {
            if let Some(start) = on_first_pass.take() {
                start();
            }
        };
        self.pass(ctx, &mut pass_state, &mut published, None);
        // A first pass that failed still lets the watcher start.
        published();

        loop {
            let trigger = {
                let mut state = self.shared.state.lock();
                loop {
                    if state.stopped || ctx.token.is_stale() {
                        return;
                    }
                    if let Some(trigger) = state.requested.take() {
                        break trigger;
                    }
                    if self
                        .shared
                        .wake
                        .wait_for(&mut state, TIMER_INTERVAL)
                        .timed_out()
                    {
                        let recent = state
                            .last_pass
                            .is_some_and(|last| last.elapsed() < TIMER_SKIP);
                        if !recent {
                            break ReconcileTrigger::Timer;
                        }
                    }
                }
            };
            self.pass(ctx, &mut pass_state, &mut || {}, Some(trigger));
        }
    }

    fn pass(
        &self,
        ctx: &ReconcileContext,
        state: &mut PassState,
        on_published: &mut dyn FnMut(),
        trigger: Option<ReconcileTrigger>,
    ) {
        match run_pass(ctx, state, on_published) {
            Ok(report) => log::info!(
                "reconciled {} notes in {:?} ({trigger:?}): {} read, {} created, {} modified, \
                 {} removed, {} renamed",
                report.notes,
                report.duration,
                report.opened,
                report.created,
                report.modified,
                report.removed,
                report.renamed
            ),
            Err(error) => log::warn!("reconciliation stopped ({trigger:?}): {error}"),
        }
        let mut shared = self.shared.state.lock();
        shared.last_pass = Some(Instant::now());
        shared.passes += 1;
        if trigger.is_none() {
            // What was asked while the first pass ran, it did. Cleared under
            // the lock that counts the pass: a request made once the pass is
            // seen as done is kept for the next one.
            shared.requested = None;
        }
        self.shared.wake.notify_all();
    }

    /// Waits until at least `count` passes ran, up to `timeout`.
    pub fn wait_for_passes(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock();
        while state.passes < count {
            if self
                .shared
                .wake
                .wait_until(&mut state, deadline)
                .timed_out()
            {
                return state.passes >= count;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests;
