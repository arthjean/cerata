//! Workspace file index, fuzzy file search and lookup by file name.
//!
//! Ported from Writer `commands/search.rs` with these fixes:
//! - one ignore model: the walk builds and filters with the same
//!   [`WorkspaceIgnore`] the sidebar and watcher use (Writer defect 3);
//! - one Markdown predicate, `.md` and `.markdown` in any case (defect 2);
//! - indexing runs off the calling thread and is canceled by epoch
//!   ([`spawn_index`]), never synchronously on the UI thread (defect 4);
//! - relative paths are normalized to `/`, so the filename bonus works with
//!   Windows separators (Writer `search.rs:112` used `rfind('/')`, defect 5);
//! - an unreadable directory is skipped with a warning instead of silently.

use crate::epoch::EpochToken;
use crate::ignore::{GitignoreChain, IgnoreBuilder, WorkspaceIgnore, global_gitignore_path};
use crate::markdown::is_markdown_path;
use parking_lot::{Condvar, Mutex};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::FileType;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One Markdown file of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub path: PathBuf,
    /// Path relative to the workspace root, always `/`-separated.
    pub relative_path: String,
    pub name: String,
    /// Seconds since the Unix epoch, 0 when unknown.
    pub modified_at: u64,
}

/// A directory the walk could not read. Indexing continues without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkWarning {
    pub path: PathBuf,
    pub message: String,
}

/// Result of a complete index walk.
#[derive(Debug, Clone)]
pub struct WorkspaceIndex {
    pub root: PathBuf,
    pub files: Vec<IndexedFile>,
    /// The root and every ancestor directory of an indexed file.
    pub dirs_with_markdown: HashSet<PathBuf>,
    /// The matcher the walk filtered with, to share with sidebar and watcher.
    pub ignore: Arc<WorkspaceIgnore>,
    pub warnings: Vec<WalkWarning>,
}

/// The walk stopped because its epoch went stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("indexing canceled: the workspace changed")]
pub struct Canceled;

/// How a background indexing job ended.
#[derive(Debug)]
pub enum IndexReport {
    Completed {
        epoch: u64,
        file_count: usize,
        duration: Duration,
        index: WorkspaceIndex,
    },
    Canceled {
        epoch: u64,
    },
}

/// Indexes `root` on a background thread and reports through `on_done`.
///
/// The job is canceled as soon as `token` goes stale (a newer workspace
/// epoch started); it then reports [`IndexReport::Canceled`]. A completed
/// report carries the file count for the caller's "indexed N files" signal.
pub fn spawn_index(
    root: PathBuf,
    token: EpochToken,
    on_done: impl FnOnce(IndexReport) + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("cerata-index".into())
        .spawn(move || {
            let started = Instant::now();
            let epoch = token.epoch();
            let report = match index_workspace(&root, &token) {
                Ok(index) => IndexReport::Completed {
                    epoch,
                    file_count: index.files.len(),
                    duration: started.elapsed(),
                    index,
                },
                Err(Canceled) => IndexReport::Canceled { epoch },
            };
            on_done(report);
        })
}

/// Indexes the Markdown files of `root`, honoring the user's global gitignore.
pub fn index_workspace(root: &Path, token: &EpochToken) -> Result<WorkspaceIndex, Canceled> {
    index_workspace_with_global(root, global_gitignore_path().as_deref(), token)
}

/// [`index_workspace`] with an explicit global gitignore file.
pub fn index_workspace_with_global(
    root: &Path,
    global_gitignore: Option<&Path>,
    token: &EpochToken,
) -> Result<WorkspaceIndex, Canceled> {
    let files = Mutex::new(Vec::new());
    let walked = walk(root, global_gitignore, token, &|entry: &WalkEntry| {
        if !entry.file_type.is_file() || !is_markdown_path(&entry.path) {
            return;
        }
        let modified_at = entry.modified.map(epoch_seconds).unwrap_or(0);
        files.lock().push(IndexedFile {
            relative_path: relative_slash_path(root, &entry.path),
            name: entry.name.to_string_lossy().into_owned(),
            path: entry.path.clone(),
            modified_at,
        });
    })?;
    let mut files = files.into_inner();
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let dirs_with_markdown = dirs_with_markdown(&files, root);
    Ok(WorkspaceIndex {
        root: root.to_path_buf(),
        files,
        dirs_with_markdown,
        ignore: Arc::new(walked.ignore),
        warnings: walked.warnings,
    })
}

/// The root and every directory between it and an indexed file.
pub fn dirs_with_markdown(files: &[IndexedFile], root: &Path) -> HashSet<PathBuf> {
    let mut dirs = HashSet::with_capacity(files.len());
    for file in files {
        let mut current = file.path.parent();
        while let Some(dir) = current {
            if !dir.starts_with(root) || !dirs.insert(dir.to_path_buf()) {
                break;
            }
            current = dir.parent();
        }
    }
    dirs
}

/// `path` relative to `root`, joined with `/` whatever the OS separator.
pub fn relative_slash_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

fn epoch_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// The index kept up to date by the watcher. Every update costs the depth
/// of the changed path, never a copy of the whole index (Writer
/// `watcher.rs:327-374` cloned the full index on each event).
#[derive(Debug, Clone)]
pub struct LiveIndex {
    root: PathBuf,
    /// Sorted by path, so a folder's files form one contiguous range.
    files: std::collections::BTreeMap<PathBuf, IndexedFile>,
    /// Number of indexed files below each directory (root included).
    dir_counts: std::collections::HashMap<PathBuf, usize>,
}

impl LiveIndex {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            files: Default::default(),
            dir_counts: Default::default(),
        }
    }

    pub fn from_index(index: &WorkspaceIndex) -> Self {
        let mut live = Self::new(&index.root);
        for file in &index.files {
            live.upsert(file.clone());
        }
        live
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    pub fn files(&self) -> impl Iterator<Item = &IndexedFile> {
        self.files.values()
    }

    /// Inserts or updates one file. Returns `true` if it was not indexed.
    pub fn upsert(&mut self, file: IndexedFile) -> bool {
        let path = file.path.clone();
        let added = self.files.insert(path.clone(), file).is_none();
        if added {
            for dir in ancestors_within(&path, &self.root) {
                *self.dir_counts.entry(dir.to_path_buf()).or_insert(0) += 1;
            }
        }
        added
    }

    /// Removes `path` and, if it is a folder, every file below it. Returns
    /// the removed files.
    pub fn remove_subtree(&mut self, path: &Path) -> Vec<IndexedFile> {
        let doomed: Vec<PathBuf> = self
            .files
            .range(path.to_path_buf()..)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(path))
            .cloned()
            .collect();
        let mut removed = Vec::with_capacity(doomed.len());
        for key in doomed {
            if let Some(file) = self.files.remove(&key) {
                for dir in ancestors_within(&key, &self.root) {
                    if let Some(count) = self.dir_counts.get_mut(dir) {
                        *count -= 1;
                        if *count == 0 {
                            self.dir_counts.remove(dir);
                        }
                    }
                }
                removed.push(file);
            }
        }
        removed
    }

    /// Search over the live index (see [`fuzzy_search`]).
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let files: Vec<IndexedFile> = self.files.values().cloned().collect();
        fuzzy_search(query, &files, limit)
    }
}

/// Folder visibility source for sidebar listings once indexing is done.
pub trait MarkdownDirs {
    /// `true` if `dir` holds a Markdown file somewhere below it.
    fn holds_markdown(&self, dir: &Path) -> bool;
}

impl MarkdownDirs for HashSet<PathBuf> {
    fn holds_markdown(&self, dir: &Path) -> bool {
        self.contains(dir)
    }
}

impl MarkdownDirs for LiveIndex {
    fn holds_markdown(&self, dir: &Path) -> bool {
        self.dir_counts.contains_key(dir)
    }
}

fn ancestors_within<'p>(path: &'p Path, root: &'p Path) -> impl Iterator<Item = &'p Path> {
    path.ancestors()
        .skip(1)
        .take_while(move |dir| dir.starts_with(root))
}

/// Markdown files below `dir`, filtered by `ignore`, for a folder that
/// appeared after the initial walk (created or moved in).
pub fn index_subtree(root: &Path, dir: &Path, ignore: &WorkspaceIgnore) -> Vec<IndexedFile> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if ignore.excludes(&path, file_type.is_dir()) {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && is_markdown_path(&path) {
                found.push(indexed_file(root, &path));
            }
        }
    }
    found
}

/// Index entry for one existing file.
pub fn indexed_file(root: &Path, path: &Path) -> IndexedFile {
    let modified_at = std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .map(epoch_seconds)
        .unwrap_or(0);
    IndexedFile {
        path: path.to_path_buf(),
        relative_path: relative_slash_path(root, path),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        modified_at,
    }
}

/// A search hit, ranked by `score` (higher first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub filename: String,
    pub relative_path: String,
    pub score: u32,
    /// Character indices of the match in `relative_path`.
    pub match_indices: Vec<u32>,
}

/// Default number of results, as in Writer.
pub const DEFAULT_SEARCH_LIMIT: usize = 50;

/// Substring search over relative paths with Writer's scoring: a match in the
/// file name beats a match in a parent directory, an earlier match beats a
/// later one, a shorter path breaks ties. The query also matches with spaces
/// and hyphens swapped, and `\` is read as `/`.
pub fn fuzzy_search(query: &str, index: &[IndexedFile], limit: usize) -> Vec<SearchResult> {
    if query.is_empty() {
        return Vec::new();
    }
    let normalized = query.to_lowercase().replace('\\', "/");
    let mut needles = vec![normalized.clone()];
    for variant in [normalized.replace(' ', "-"), normalized.replace('-', " ")] {
        if !needles.contains(&variant) {
            needles.push(variant);
        }
    }

    let mut results: Vec<SearchResult> = index
        .iter()
        .filter_map(|file| score_file(file, &needles))
        .collect();
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    results.truncate(limit);
    results
}

fn score_file(file: &IndexedFile, needles: &[String]) -> Option<SearchResult> {
    let haystack = file.relative_path.to_lowercase();
    let (byte_start, needle) = needles
        .iter()
        .filter_map(|needle| haystack.find(needle.as_str()).map(|start| (start, needle)))
        .min_by_key(|(start, _)| *start)?;

    // Lowercasing can change byte lengths, so locate the file name in the
    // lowercased haystack rather than in the original path.
    let filename_start = haystack.rfind('/').map(|slash| slash + 1).unwrap_or(0);
    let in_filename = byte_start >= filename_start;

    let mut score: u32 = 0;
    if in_filename {
        score += 1_000_000;
    }
    score += 10_000u32.saturating_sub(u32::try_from(byte_start).unwrap_or(u32::MAX));
    let path_chars = u32::try_from(file.relative_path.chars().count()).unwrap_or(u32::MAX);
    score += 1_000u32.saturating_sub(path_chars.min(1_000));

    let char_start = haystack[..byte_start].chars().count();
    let char_len = needle.chars().count();
    let match_indices = (char_start..char_start + char_len)
        .filter_map(|index| u32::try_from(index).ok())
        .collect();

    Some(SearchResult {
        path: file.path.clone(),
        filename: file.name.clone(),
        relative_path: file.relative_path.clone(),
        score,
        match_indices,
    })
}

/// Finds a file of any type under `root` by case-insensitive name, for
/// Obsidian-style embeds (`![[image.png]]`). Uses the workspace ignore
/// model. On duplicates the shortest path wins (fewest components, then
/// lexicographic order), as in Obsidian.
pub fn find_file_by_name(root: &Path, file_name: &str) -> Option<PathBuf> {
    find_file_by_name_with_global(root, file_name, global_gitignore_path().as_deref())
}

/// [`find_file_by_name`] with an explicit global gitignore file.
pub fn find_file_by_name_with_global(
    root: &Path,
    file_name: &str,
    global_gitignore: Option<&Path>,
) -> Option<PathBuf> {
    let target = file_name.to_lowercase();
    let matches = Mutex::new(Vec::new());
    let never_canceled = crate::epoch::EpochCounter::new().token();
    let walked = walk(
        root,
        global_gitignore,
        &never_canceled,
        &|entry: &WalkEntry| {
            if entry.file_type.is_file() && entry.name.to_string_lossy().to_lowercase() == target {
                matches.lock().push(entry.path.clone());
            }
        },
    );
    if walked.is_err() {
        return None;
    }
    let mut matches = matches.into_inner();
    matches.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    matches.into_iter().next()
}

// Parallel walk -----------------------------------------------------------

/// A non-excluded file or symlink seen by the walk.
pub(crate) struct WalkEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: OsString,
    pub(crate) file_type: FileType,
    pub(crate) modified: Option<SystemTime>,
}

pub(crate) struct Walked {
    pub(crate) ignore: WorkspaceIgnore,
    pub(crate) warnings: Vec<WalkWarning>,
}

struct Job {
    dir: PathBuf,
    chain: GitignoreChain,
}

struct Queue {
    jobs: Vec<Job>,
    active: usize,
}

/// Walks `root` top-down on several threads, pruning excluded directories
/// with the rules discovered so far, and calls `visit` for every
/// non-excluded non-directory entry. Returns the complete ignore matcher.
pub(crate) fn walk(
    root: &Path,
    global_gitignore: Option<&Path>,
    token: &EpochToken,
    visit: &(dyn Fn(&WalkEntry) + Sync),
) -> Result<Walked, Canceled> {
    let builder = IgnoreBuilder::new(root, global_gitignore);
    let warnings = Mutex::new(Vec::new());
    let canceled = AtomicBool::new(false);
    let queue = Mutex::new(Queue {
        jobs: vec![Job {
            dir: root.to_path_buf(),
            chain: builder.chain_for_root(),
        }],
        active: 0,
    });
    let ready = Condvar::new();
    let threads = std::thread::available_parallelism()
        .map(|count| count.get().min(8))
        .unwrap_or(4);

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| worker(&builder, &queue, &ready, &canceled, &warnings, token, visit));
        }
    });

    if canceled.load(Ordering::SeqCst) || token.is_stale() {
        return Err(Canceled);
    }
    Ok(Walked {
        ignore: builder.finish(),
        warnings: warnings.into_inner(),
    })
}

fn worker(
    builder: &IgnoreBuilder,
    queue: &Mutex<Queue>,
    ready: &Condvar,
    canceled: &AtomicBool,
    warnings: &Mutex<Vec<WalkWarning>>,
    token: &EpochToken,
    visit: &(dyn Fn(&WalkEntry) + Sync),
) {
    loop {
        let job = {
            let mut state = queue.lock();
            loop {
                if canceled.load(Ordering::SeqCst) {
                    state.jobs.clear();
                }
                if let Some(job) = state.jobs.pop() {
                    state.active += 1;
                    break job;
                }
                if state.active == 0 {
                    ready.notify_all();
                    return;
                }
                ready.wait(&mut state);
            }
        };

        let children = if token.is_stale() {
            canceled.store(true, Ordering::SeqCst);
            Vec::new()
        } else {
            read_dir_job(builder, &job, warnings, visit)
        };

        let mut state = queue.lock();
        state.active -= 1;
        state.jobs.extend(children);
        ready.notify_all();
    }
}

fn read_dir_job(
    builder: &IgnoreBuilder,
    job: &Job,
    warnings: &Mutex<Vec<WalkWarning>>,
    visit: &(dyn Fn(&WalkEntry) + Sync),
) -> Vec<Job> {
    let entries = match std::fs::read_dir(&job.dir) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn!(
                "skipping unreadable directory {}: {error}",
                job.dir.display()
            );
            warnings.lock().push(WalkWarning {
                path: job.dir.clone(),
                message: error.to_string(),
            });
            return Vec::new();
        }
    };
    let mut children = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let is_dir = file_type.is_dir();
        if builder.excludes_with(&job.chain, &path, is_dir) {
            continue;
        }
        if is_dir {
            let chain = builder.enter(&job.chain, &path);
            children.push(Job { dir: path, chain });
        } else {
            let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
            visit(&WalkEntry {
                path,
                name: entry.file_name(),
                file_type,
                modified,
            });
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::EpochCounter;
    use std::fs;
    use tempfile::TempDir;

    fn token() -> EpochToken {
        EpochCounter::new().token()
    }

    fn index(root: &Path) -> WorkspaceIndex {
        index_workspace_with_global(root, None, &token()).unwrap()
    }

    fn setup_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("readme.md"), "# Readme").unwrap();
        fs::write(dir.path().join("notes.md"), "# Notes").unwrap();
        fs::write(dir.path().join("data.txt"), "not indexed").unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("docs").join("guide.md"), "# Guide").unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git").join("config.md"), "git").unwrap();
        dir
    }

    // Ported from Writer `commands/search.rs` tests.

    #[test]
    fn index_counts_md_files() {
        let dir = setup_workspace();
        assert_eq!(index(dir.path()).files.len(), 3);
    }

    #[test]
    fn index_ignores_hidden() {
        let dir = setup_workspace();
        let index = index(dir.path());
        assert!(!index.files.iter().any(|f| f.relative_path.contains(".git")));
    }

    #[test]
    fn index_builds_dirs_with_markdown() {
        let dir = setup_workspace();
        let root = dir.path().to_path_buf();
        let index = index(&root);
        assert!(index.dirs_with_markdown.contains(&root));
        assert!(index.dirs_with_markdown.contains(&root.join("docs")));
        assert!(!index.dirs_with_markdown.contains(&root.join(".git")));
    }

    #[test]
    fn canceled_token_short_circuits_walk() {
        let dir = setup_workspace();
        let result = index_workspace_with_global(dir.path(), None, &EpochToken::canceled());
        assert_eq!(result.err(), Some(Canceled));
    }

    #[test]
    fn fuzzy_search_ranks_by_relevance() {
        let dir = setup_workspace();
        let results = fuzzy_search("readme", &index(dir.path()).files, 50);
        assert_eq!(results[0].filename, "readme.md");
    }

    #[test]
    fn fuzzy_search_matches_space_separated_names() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("No prior experience.md"), "# Note").unwrap();
        let results = fuzzy_search("No prior experience", &index(dir.path()).files, 50);
        assert_eq!(results[0].filename, "No prior experience.md");
    }

    #[test]
    fn fuzzy_search_returns_match_indices() {
        let dir = setup_workspace();
        let results = fuzzy_search("guide", &index(dir.path()).files, 50);
        assert_eq!(results[0].match_indices, vec![5, 6, 7, 8, 9]);
    }

    #[test]
    fn fuzzy_search_empty_query() {
        let dir = setup_workspace();
        assert!(fuzzy_search("", &index(dir.path()).files, 50).is_empty());
    }

    #[test]
    fn fuzzy_search_respects_limit() {
        let dir = setup_workspace();
        assert!(fuzzy_search("md", &index(dir.path()).files, 1).len() <= 1);
    }

    #[test]
    fn find_file_by_name_case_insensitive() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets").join("Diagram.PNG"), [0u8]).unwrap();
        let found = find_file_by_name_with_global(dir.path(), "diagram.png", None).unwrap();
        assert_eq!(found, dir.path().join("assets").join("Diagram.PNG"));
    }

    #[test]
    fn find_file_by_name_prefers_shortest_path() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a").join("b")).unwrap();
        fs::write(dir.path().join("a").join("b").join("pic.png"), [0u8]).unwrap();
        fs::write(dir.path().join("pic.png"), [0u8]).unwrap();
        let found = find_file_by_name_with_global(dir.path(), "pic.png", None).unwrap();
        assert_eq!(found, dir.path().join("pic.png"));
    }

    #[test]
    fn find_file_by_name_missing_and_hidden() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git").join("hidden.png"), [0u8]).unwrap();
        assert!(find_file_by_name_with_global(dir.path(), "nope.png", None).is_none());
        assert!(find_file_by_name_with_global(dir.path(), "hidden.png", None).is_none());
    }

    // Cerata additions (fixed defects).

    #[test]
    fn index_accepts_both_extensions_in_any_case() {
        let dir = TempDir::new().unwrap();
        for name in ["a.md", "B.MD", "c.markdown", "d.MarkDown", "e.txt"] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        assert_eq!(index(dir.path()).files.len(), 4);
    }

    #[test]
    fn index_and_ignore_model_agree_on_every_path() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "drafts/\n*.tmp.md\n").unwrap();
        fs::create_dir_all(root.join("docs").join("private")).unwrap();
        fs::write(root.join("docs").join(".gitignore"), "private/\n").unwrap();
        fs::create_dir_all(root.join(".git").join("info")).unwrap();
        fs::write(
            root.join(".git").join("info").join("exclude"),
            "excluded.md\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("drafts")).unwrap();
        fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        let candidates = [
            root.join("keep.md"),
            root.join("x.tmp.md"),
            root.join("excluded.md"),
            root.join("drafts").join("wip.md"),
            root.join("docs").join("guide.md"),
            root.join("docs").join("private").join("secret.md"),
            root.join("node_modules").join("pkg").join("readme.md"),
            root.join(".obsidian").join("workspace.md"),
        ];
        for path in &candidates {
            fs::write(path, "x").unwrap();
        }
        let index = index(root);
        let indexed: HashSet<&Path> = index.files.iter().map(|f| f.path.as_path()).collect();
        for path in &candidates {
            assert_eq!(
                indexed.contains(path.as_path()),
                !index.ignore.excludes(path, false),
                "index and ignore model disagree on {}",
                path.display()
            );
        }
        let reloaded = WorkspaceIgnore::load_with_global(root, None);
        for path in &candidates {
            assert_eq!(
                index.ignore.excludes(path, false),
                reloaded.excludes(path, false)
            );
        }
        assert_eq!(indexed.len(), 2, "keep.md and docs/guide.md");
    }

    #[test]
    fn relative_paths_use_forward_slashes_and_search_bonus_survives() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("plans").join("deep")).unwrap();
        fs::write(
            dir.path().join("plans").join("deep").join("roadmap.md"),
            "x",
        )
        .unwrap();
        fs::write(dir.path().join("plans.md"), "x").unwrap();
        let index = index(dir.path());
        assert!(
            index
                .files
                .iter()
                .any(|f| f.relative_path == "plans/deep/roadmap.md")
        );
        // A filename match beats a directory match, with `/` or `\` queries.
        let results = fuzzy_search("plans", &index.files, 50);
        assert_eq!(results[0].relative_path, "plans.md");
        let results = fuzzy_search("deep\\road", &index.files, 50);
        assert_eq!(results[0].relative_path, "plans/deep/roadmap.md");
    }

    #[test]
    fn spawn_index_reports_completion_with_file_count() {
        let dir = setup_workspace();
        let counter = EpochCounter::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        let handle = spawn_index(dir.path().to_path_buf(), counter.token(), move |report| {
            assert_ne!(std::thread::current().id(), caller);
            sender.send(report).unwrap();
        })
        .unwrap();
        handle.join().unwrap();
        match receiver.recv().unwrap() {
            IndexReport::Completed {
                file_count, epoch, ..
            } => {
                assert_eq!(file_count, 3);
                assert_eq!(epoch, 0);
            }
            IndexReport::Canceled { .. } => panic!("expected completion"),
        }
    }

    #[test]
    fn spawn_index_is_canceled_by_a_newer_epoch() {
        let dir = setup_workspace();
        let counter = EpochCounter::new();
        let token = counter.token();
        counter.advance();
        let (sender, receiver) = std::sync::mpsc::channel();
        spawn_index(dir.path().to_path_buf(), token, move |report| {
            sender.send(report).unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
        assert!(matches!(
            receiver.recv().unwrap(),
            IndexReport::Canceled { epoch: 0 }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_skipped_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        let dir = setup_workspace();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("inside.md"), "x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok(); // root ignores permissions
        let index = index(dir.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            return;
        }
        assert_eq!(index.files.len(), 3);
        assert_eq!(index.warnings.len(), 1);
        assert_eq!(index.warnings[0].path, locked);
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_directory_is_skipped_with_a_warning() {
        let dir = setup_workspace();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("inside.md"), "x").unwrap();
        let path = locked.to_string_lossy().into_owned();
        let user = std::env::var("USERNAME").unwrap();
        let deny = std::process::Command::new("icacls")
            .args([&path, "/deny", &format!("{user}:(RX)")])
            .output()
            .unwrap();
        assert!(deny.status.success(), "{deny:?}");
        // Accounts with backup privileges (CI runners run as administrators)
        // still read the folder; the test needs a denial that holds.
        let readable = fs::read_dir(&locked).is_ok();
        let index = index(dir.path());
        let restore = std::process::Command::new("icacls")
            .args([&path, "/remove:d", &user])
            .output()
            .unwrap();
        assert!(restore.status.success(), "{restore:?}");
        if readable {
            eprintln!("skipped: this account reads folders despite a deny ACE");
            return;
        }
        assert_eq!(index.files.len(), 3);
        assert_eq!(index.warnings.len(), 1);
        assert_eq!(index.warnings[0].path, locked);
    }

    #[test]
    fn live_index_tracks_folders_incrementally() {
        let root = PathBuf::from("/w");
        let file = |rel: &str| IndexedFile {
            path: root.join(rel),
            relative_path: rel.to_owned(),
            name: rel.rsplit('/').next().unwrap().to_owned(),
            modified_at: 0,
        };
        let mut live = LiveIndex::new(&root);
        assert!(live.upsert(file("a/b/one.md")));
        assert!(live.upsert(file("a/two.md")));
        assert!(!live.upsert(file("a/two.md")));
        assert!(live.holds_markdown(&root));
        assert!(live.holds_markdown(&root.join("a").join("b")));
        let removed = live.remove_subtree(&root.join("a").join("b"));
        assert_eq!(removed.len(), 1);
        assert!(!live.holds_markdown(&root.join("a").join("b")));
        assert!(live.holds_markdown(&root.join("a")));
        // A sibling whose name extends the removed folder's is untouched.
        live.upsert(file("ab/three.md"));
        live.remove_subtree(&root.join("a"));
        assert_eq!(live.len(), 1);
        assert!(live.contains(&root.join("ab").join("three.md")));
    }

    // Writer `watcher.rs` add_subtree tests: a folder that appears is walked
    // for Markdown only, and walking it again adds nothing.
    #[test]
    fn subtree_walk_indexes_markdown_only_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let batch = root.join("batch");
        fs::create_dir_all(batch.join("deep")).unwrap();
        fs::write(batch.join("one.md"), "x").unwrap();
        fs::write(batch.join("deep").join("two.MARKDOWN"), "x").unwrap();
        fs::write(batch.join("image.png"), "x").unwrap();
        fs::write(batch.join("notes.txt"), "x").unwrap();
        let ignore = WorkspaceIgnore::load_with_global(&root, None);
        let mut live = LiveIndex::new(&root);
        live.upsert(IndexedFile {
            path: batch.join("one.md"),
            relative_path: "batch/one.md".into(),
            name: "one.md".into(),
            modified_at: 0,
        });

        for _ in 0..2 {
            let found = index_subtree(&root, &batch, &ignore);
            let mut names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, ["one.md", "two.MARKDOWN"]);
            for file in found {
                live.upsert(file);
            }
            assert_eq!(live.len(), 2);
        }
        assert!(live.contains(&batch.join("deep").join("two.MARKDOWN")));
        assert!(!live.contains(&batch.join("image.png")));
    }

    #[test]
    fn live_index_update_cost_does_not_grow_with_index_size() {
        let root = PathBuf::from("/w");
        let make = |i: usize| IndexedFile {
            path: root.join(format!("d{}", i % 100)).join(format!("n{i}.md")),
            relative_path: format!("d{}/n{i}.md", i % 100),
            name: format!("n{i}.md"),
            modified_at: 0,
        };
        let mut live = LiveIndex::new(&root);
        for i in 0..50_000 {
            live.upsert(make(i));
        }
        // 2,000 single-path updates. Copying a 50,000-entry index per event,
        // as Writer did, would take seconds even in release.
        let started = Instant::now();
        for i in 0..1_000 {
            live.remove_subtree(&make(i).path);
            live.upsert(make(i));
        }
        assert_eq!(live.len(), 50_000);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }
}
