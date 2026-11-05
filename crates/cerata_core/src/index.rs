use crate::epoch::EpochToken;
use crate::ignore::{GitignoreChain, IgnoreBuilder, WorkspaceIgnore, global_gitignore_path};
use crate::markdown::is_markdown_path;
use crate::store::ContentHash;
use parking_lot::{Condvar, Mutex};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{FileType, Metadata};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexedFile {
    pub path: PathBuf,
    pub relative_path: String,
    pub name: String,
    pub modified_at: u64,
    pub size: u64,
    pub modified_ns: u64,
    pub hash: Option<ContentHash>,
    pub title: Option<String>,
}

impl IndexedFile {
    pub fn new(root: &Path, path: &Path, metadata: Option<&Metadata>) -> Self {
        let modified_ns = metadata.map_or(0, modified_ns);
        Self {
            path: path.to_path_buf(),
            relative_path: relative_slash_path(root, path),
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            modified_at: modified_ns / 1_000_000_000,
            size: metadata.map_or(0, Metadata::len),
            modified_ns,
            hash: None,
            title: None,
        }
    }
}

pub fn modified_ns(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkWarning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceIndex {
    pub root: PathBuf,
    pub files: Vec<IndexedFile>,
    pub other_files: Vec<PathBuf>,
    pub dirs_with_markdown: HashSet<PathBuf>,
    pub ignore: Arc<WorkspaceIgnore>,
    pub warnings: Vec<WalkWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("indexing canceled: the workspace changed")]
pub struct Canceled;

pub fn index_workspace(root: &Path, token: &EpochToken) -> Result<WorkspaceIndex, Canceled> {
    index_workspace_with_global(root, global_gitignore_path().as_deref(), token)
}

pub fn index_workspace_with_global(
    root: &Path,
    global_gitignore: Option<&Path>,
    token: &EpochToken,
) -> Result<WorkspaceIndex, Canceled> {
    let files = Mutex::new(Vec::new());
    let others = Mutex::new(Vec::new());
    let walked = walk(root, global_gitignore, token, &|entry: &WalkEntry| {
        if !entry.file_type.is_file() {
            return;
        }
        if is_markdown_path(&entry.path) {
            files
                .lock()
                .push(IndexedFile::new(root, &entry.path, entry.metadata.as_ref()));
        } else {
            others.lock().push(entry.path.clone());
        }
    })?;
    let mut files = files.into_inner();
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let dirs_with_markdown = dirs_with_markdown(&files, root);
    Ok(WorkspaceIndex {
        root: root.to_path_buf(),
        files,
        other_files: others.into_inner(),
        dirs_with_markdown,
        ignore: Arc::new(walked.ignore),
        warnings: walked.warnings,
    })
}

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

pub fn relative_slash_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

#[derive(Debug, Clone)]
pub struct LiveIndex {
    root: PathBuf,
    files: std::collections::BTreeMap<PathBuf, IndexedFile>,
    dir_counts: HashMap<PathBuf, usize>,
    other_files: BTreeSet<PathBuf>,
    names: HashMap<String, BTreeSet<PathBuf>>,
    names_complete: bool,
}

impl LiveIndex {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            files: Default::default(),
            dir_counts: Default::default(),
            other_files: Default::default(),
            names: Default::default(),
            names_complete: false,
        }
    }

    pub fn from_index(index: &WorkspaceIndex) -> Self {
        let mut live = Self::new(&index.root);
        for file in &index.files {
            live.upsert(file.clone());
        }
        for path in &index.other_files {
            live.add_other(path);
        }
        live.names_complete = true;
        live
    }

    pub fn names_complete(&self) -> bool {
        self.names_complete
    }

    pub fn get(&self, path: &Path) -> Option<&IndexedFile> {
        self.files.get(path)
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut IndexedFile> {
        self.files.get_mut(path)
    }

    pub fn titles_in(&self, dir: &Path) -> HashMap<String, Option<String>> {
        let prefix = match relative_slash_path(&self.root, dir) {
            relative if relative.is_empty() || dir == self.root => String::new(),
            relative => format!("{relative}/"),
        };
        self.files
            .range(dir.to_path_buf()..)
            .map(|(_, file)| file)
            .take_while(|file| file.relative_path.starts_with(&prefix))
            .filter(|file| file.hash.is_some() && !file.relative_path[prefix.len()..].contains('/'))
            .map(|file| (file.name.clone(), file.title.clone()))
            .collect()
    }

    pub fn title(&self, path: &Path) -> Option<&str> {
        self.files.get(path).and_then(|file| file.title.as_deref())
    }

    pub fn add_other(&mut self, path: &Path) {
        if self.other_files.insert(path.to_path_buf()) {
            self.add_name(path);
        }
    }

    pub fn set_other_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in std::mem::take(&mut self.other_files) {
            self.remove_name(&path);
        }
        for path in paths {
            self.add_other(&path);
        }
        self.names_complete = true;
    }

    pub fn find_by_name(&self, file_name: &str) -> Option<PathBuf> {
        self.names
            .get(&file_name.to_lowercase())?
            .iter()
            .min_by(|a, b| {
                a.components()
                    .count()
                    .cmp(&b.components().count())
                    .then_with(|| a.cmp(b))
            })
            .cloned()
    }

    fn add_name(&mut self, path: &Path) {
        if let Some(name) = path.file_name() {
            self.names
                .entry(name.to_string_lossy().to_lowercase())
                .or_default()
                .insert(path.to_path_buf());
        }
    }

    fn remove_name(&mut self, path: &Path) {
        let Some(name) = path.file_name() else { return };
        let key = name.to_string_lossy().to_lowercase();
        if let Some(paths) = self.names.get_mut(&key) {
            paths.remove(path);
            if paths.is_empty() {
                self.names.remove(&key);
            }
        }
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

    pub fn upsert(&mut self, file: IndexedFile) -> bool {
        let path = file.path.clone();
        let depth = folder_depth(&file.relative_path);
        let added = self.files.insert(path.clone(), file).is_none();
        if added {
            self.count_in_folders(&path, depth);
            self.add_name(&path);
        }
        added
    }

    pub fn from_files(root: &Path, files: Vec<IndexedFile>) -> Self {
        let mut live = Self::new(root);
        let mut counted = Vec::with_capacity(files.len());
        live.files = files
            .into_iter()
            .map(|file| {
                counted.push((file.path.clone(), folder_depth(&file.relative_path)));
                (file.path.clone(), file)
            })
            .collect();
        for (path, depth) in counted {
            live.count_in_folders(&path, depth);
            live.add_name(&path);
        }
        live
    }

    fn count_in_folders(&mut self, path: &Path, depth: usize) {
        for dir in path.ancestors().skip(1).take(depth) {
            match self.dir_counts.get_mut(dir) {
                Some(count) => *count += 1,
                None => {
                    self.dir_counts.insert(dir.to_path_buf(), 1);
                }
            }
        }
    }

    pub fn remove_subtree(&mut self, path: &Path) -> Vec<IndexedFile> {
        let others: Vec<PathBuf> = self
            .other_files
            .range(path.to_path_buf()..)
            .take_while(|key| key.starts_with(path))
            .cloned()
            .collect();
        for other in others {
            self.other_files.remove(&other);
            self.remove_name(&other);
        }
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
                let depth = folder_depth(&file.relative_path);
                for dir in key.ancestors().skip(1).take(depth) {
                    if let Some(count) = self.dir_counts.get_mut(dir) {
                        *count -= 1;
                        if *count == 0 {
                            self.dir_counts.remove(dir);
                        }
                    }
                }
                self.remove_name(&key);
                removed.push(file);
            }
        }
        removed
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let files: Vec<IndexedFile> = self.files.values().cloned().collect();
        fuzzy_search(query, &files, limit)
    }

    pub fn recent(&self, limit: usize, offset: usize) -> Vec<IndexedFile> {
        let mut files: Vec<&IndexedFile> = self.files.values().collect();
        files.sort_by(|a, b| {
            b.modified_at
                .cmp(&a.modified_at)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        files
            .into_iter()
            .skip(offset)
            .take(limit.clamp(1, 100))
            .filter(|file| is_markdown_path(&file.path) && file.path.is_file())
            .cloned()
            .collect()
    }

    pub fn touch(&mut self, path: &Path, modified_at: u64) {
        if let Some(file) = self.files.get_mut(path) {
            file.modified_at = modified_at;
        }
    }
}

pub trait MarkdownDirs {
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

fn folder_depth(relative_path: &str) -> usize {
    relative_path.matches('/').count() + 1
}

pub fn index_subtree(
    root: &Path,
    dir: &Path,
    ignore: &WorkspaceIgnore,
) -> (Vec<IndexedFile>, Vec<PathBuf>) {
    let mut found = Vec::new();
    let mut others = Vec::new();
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
                found.push(IndexedFile::new(
                    root,
                    &path,
                    entry.metadata().ok().as_ref(),
                ));
            } else if file_type.is_file() {
                others.push(path);
            }
        }
    }
    (found, others)
}

pub fn indexed_file(root: &Path, path: &Path) -> IndexedFile {
    IndexedFile::new(root, path, std::fs::metadata(path).ok().as_ref())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub filename: String,
    pub relative_path: String,
    pub score: u32,
    pub match_indices: Vec<u32>,
}

pub const DEFAULT_SEARCH_LIMIT: usize = 50;

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

pub fn find_file_by_name(root: &Path, file_name: &str) -> Option<PathBuf> {
    find_file_by_name_with_global(root, file_name, global_gitignore_path().as_deref())
}

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

pub(crate) struct WalkEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: OsString,
    pub(crate) file_type: FileType,
    pub(crate) metadata: Option<Metadata>,
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
            visit(&WalkEntry {
                path,
                name: entry.file_name(),
                file_type,
                metadata: entry.metadata().ok(),
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
    use std::time::{Duration, Instant};
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
        let results = fuzzy_search("plans", &index.files, 50);
        assert_eq!(results[0].relative_path, "plans.md");
        let results = fuzzy_search("deep\\road", &index.files, 50);
        assert_eq!(results[0].relative_path, "plans/deep/roadmap.md");
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
        let readable = fs::read_dir(&locked).is_ok();
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
    fn titles_in_lists_the_read_notes_of_one_folder_only() {
        let root = PathBuf::from("/w");
        let file = |rel: &str, title: Option<&str>| {
            let mut file = IndexedFile {
                path: rel.split('/').fold(root.clone(), |p, part| p.join(part)),
                relative_path: rel.to_owned(),
                name: rel.rsplit('/').next().unwrap().to_owned(),
                ..Default::default()
            };
            file.hash = title.map(|_| [0; 32]);
            file.title = title.map(str::to_owned);
            file
        };
        let mut live = LiveIndex::new(&root);
        live.upsert(file("top.md", Some("Top")));
        live.upsert(file("unread.md", None));
        live.upsert(file("a/inner.md", Some("Inner")));
        live.upsert(file("a/b/deep.md", Some("Deep")));
        live.upsert(file("a-b/side.md", Some("Side")));
        let root_titles = live.titles_in(&root);
        assert_eq!(root_titles.len(), 1);
        assert_eq!(root_titles["top.md"].as_deref(), Some("Top"));
        let a = live.titles_in(&root.join("a"));
        assert_eq!(a.len(), 1);
        assert_eq!(a["inner.md"].as_deref(), Some("Inner"));
        assert_eq!(live.titles_in(&root.join("a").join("b")).len(), 1);
    }

    #[test]
    fn live_index_tracks_folders_incrementally() {
        let root = PathBuf::from("/w");
        let file = |rel: &str| IndexedFile {
            path: root.join(rel),
            relative_path: rel.to_owned(),
            name: rel.rsplit('/').next().unwrap().to_owned(),
            modified_at: 0,
            ..Default::default()
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
        live.upsert(file("ab/three.md"));
        live.remove_subtree(&root.join("a"));
        assert_eq!(live.len(), 1);
        assert!(live.contains(&root.join("ab").join("three.md")));
    }

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
            ..Default::default()
        });

        for _ in 0..2 {
            let (found, _) = index_subtree(&root, &batch, &ignore);
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
            ..Default::default()
        };
        let mut live = LiveIndex::new(&root);
        for i in 0..50_000 {
            live.upsert(make(i));
        }
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

    fn indexed(root: &Path, name: &str, modified_at: u64) -> IndexedFile {
        let path = root.join(name);
        fs::write(&path, "x").unwrap();
        IndexedFile {
            relative_path: name.into(),
            name: name.into(),
            path,
            modified_at,
            ..Default::default()
        }
    }

    #[test]
    fn test_read_recent_files_sorts_by_index_mtime_and_pages() {
        let dir = TempDir::new().unwrap();
        let mut live = LiveIndex::new(dir.path());
        live.upsert(indexed(dir.path(), "old.md", 1));
        live.upsert(indexed(dir.path(), "new.md", 3));
        live.upsert(indexed(dir.path(), "middle.md", 2));
        let names = |files: Vec<IndexedFile>| files.into_iter().map(|f| f.name).collect::<Vec<_>>();
        assert_eq!(names(live.recent(2, 0)), ["new.md", "middle.md"]);
        assert_eq!(names(live.recent(2, 2)), ["old.md"]);
    }

    #[test]
    fn test_read_recent_files_cache_invalidates_on_mtime_update() {
        let dir = TempDir::new().unwrap();
        let mut live = LiveIndex::new(dir.path());
        live.upsert(indexed(dir.path(), "old.md", 1));
        live.upsert(indexed(dir.path(), "new.md", 3));
        assert_eq!(live.recent(1, 0)[0].name, "new.md");
        live.touch(&dir.path().join("old.md"), 5);
        assert_eq!(live.recent(1, 0)[0].name, "old.md");
    }

    #[test]
    fn recent_files_skip_missing_and_non_markdown_entries() {
        let dir = TempDir::new().unwrap();
        let mut live = LiveIndex::new(dir.path());
        live.upsert(indexed(dir.path(), "kept.md", 2));
        let gone = indexed(dir.path(), "gone.md", 3);
        fs::remove_file(&gone.path).unwrap();
        live.upsert(gone);
        live.upsert(indexed(dir.path(), "note.txt", 4));
        let names: Vec<_> = live.recent(100, 0).into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["kept.md"]);
    }
}
