//! Workspace file operations, confined to the workspace root.
//!
//! Ported from Writer `commands/fs.rs` and `sidebar/duplicate-file.ts`, with
//! these fixes:
//! - every path is canonicalized and must stay inside the canonical root
//!   (Writer defect 9: no confinement at all);
//! - writes are atomic with fsync, keep symlinks, permissions, line endings
//!   and BOM (defect 8, see [`atomic`] and [`text`]);
//! - listings never read titles synchronously (defect 12, see [`titles`]);
//! - renames validate the new name (defect 15);
//! - one Markdown predicate (defect 2).

pub mod atomic;
pub mod text;
pub mod titles;

use crate::ignore::WorkspaceIgnore;
use crate::index::MarkdownDirs;
use crate::markdown::is_markdown_path;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use text::TextFormat;

/// Errors of workspace file operations. `Io` carries the OS error verbatim.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("not found: {}", .0.display())]
    NotFound(PathBuf),
    #[error("already exists: {}", .0.display())]
    AlreadyExists(PathBuf),
    #[error("outside the workspace: {}", .0.display())]
    OutsideWorkspace(PathBuf),
    #[error("invalid name {name:?}: {reason}")]
    InvalidName { name: String, reason: String },
    #[error("{} is not valid UTF-8 text", .0.display())]
    NotUtf8(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl FsError {
    fn io(path: &Path, source: io::Error) -> Self {
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path.to_path_buf()),
            io::ErrorKind::AlreadyExists => Self::AlreadyExists(path.to_path_buf()),
            _ => Self::Io {
                path: path.to_path_buf(),
                source,
            },
        }
    }
}

/// A sidebar row. `title` is filled later by [`titles::spawn_title_batches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_markdown: bool,
    pub modified_at: u64,
    /// Falls back to the modified time where the file system has no birth time.
    pub created_at: u64,
    pub title: Option<String>,
}

/// A file's editor text and the on-disk format to restore when saving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub path: PathBuf,
    pub content: String,
    pub format: TextFormat,
    pub modified_at: u64,
}

/// What [`WorkspaceFs::create_untitled`] creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

/// Content of a new Markdown file, as in Writer.
pub const NEW_FILE_CONTENT: &str = "# ";

/// File operations confined to one workspace root.
#[derive(Debug, Clone)]
pub struct WorkspaceFs {
    root: PathBuf,
}

impl WorkspaceFs {
    /// `root` must exist; it is canonicalized once here.
    pub fn new(root: &Path) -> Result<Self, FsError> {
        let root = dunce::canonicalize(root).map_err(|e| FsError::io(root, e))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical form of an existing `path`, refused when outside the root.
    pub fn confine_existing(&self, path: &Path) -> Result<PathBuf, FsError> {
        let canonical = dunce::canonicalize(path).map_err(|e| FsError::io(path, e))?;
        self.check_inside(&canonical, path)?;
        Ok(canonical)
    }

    /// Canonical form of a path that may not exist yet: its parent must
    /// exist inside the root and its file name must be valid.
    pub fn confine_new(&self, path: &Path) -> Result<PathBuf, FsError> {
        let name = path
            .file_name()
            .ok_or_else(|| FsError::InvalidName {
                name: path.display().to_string(),
                reason: "the path has no file name".into(),
            })?
            .to_string_lossy()
            .into_owned();
        validate_file_name(&name)?;
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let parent = self.confine_existing(parent)?;
        Ok(parent.join(name))
    }

    /// An existing entry itself, without following a final symlink: its
    /// parent is canonicalized and confined, its own name is kept. Deleting
    /// or renaming a link must act on the link, never on its target.
    fn confine_entry(&self, path: &Path) -> Result<PathBuf, FsError> {
        let Some(name) = path.file_name() else {
            return self.confine_existing(path);
        };
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let entry = self.confine_existing(parent)?.join(name);
        fs::symlink_metadata(&entry).map_err(|e| FsError::io(&entry, e))?;
        Ok(entry)
    }

    fn check_inside(&self, canonical: &Path, original: &Path) -> Result<(), FsError> {
        if canonical.starts_with(&self.root) {
            Ok(())
        } else {
            Err(FsError::OutsideWorkspace(original.to_path_buf()))
        }
    }

    /// Lists `dir` for the sidebar: visible folders first, then Markdown
    /// files, each group sorted case-insensitively. Folders are shown when
    /// they hold Markdown or are empty trees. `dirs_with_markdown` is the
    /// index's set once indexing has completed, `None` before.
    ///
    /// Titles are left `None`; read them with
    /// [`titles::spawn_title_batches`] so slow storage never blocks here.
    pub fn read_directory(
        &self,
        dir: &Path,
        ignore: &WorkspaceIgnore,
        dirs_with_markdown: Option<&dyn MarkdownDirs>,
    ) -> Result<Vec<DirEntry>, FsError> {
        let dir = self.confine_existing(dir)?;
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| FsError::io(&dir, e))? {
            let Ok(entry) = entry else { continue };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if ignore.excludes(&path, file_type.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                match directory_is_visible(&path, ignore, dirs_with_markdown) {
                    Ok(true) => dirs.push(dir_entry(name, path, true)),
                    Ok(false) => {}
                    Err(error) => log::warn!("hiding unreadable folder: {error}"),
                }
            } else if file_type.is_file() && is_markdown_path(&path) {
                files.push(dir_entry(name, path, false));
            }
        }
        dirs.sort_by_key(|entry| entry.name.to_lowercase());
        files.sort_by_key(|entry| entry.name.to_lowercase());
        dirs.extend(files);
        Ok(dirs)
    }

    /// Reads a text file: UTF-8 (with or without BOM), `\n` line endings.
    pub fn read_file(&self, path: &Path) -> Result<FileContent, FsError> {
        let path = self.confine_existing(path)?;
        let bytes = fs::read(&path).map_err(|e| FsError::io(&path, e))?;
        let (content, format) = text::decode(&bytes).map_err(|_| FsError::NotUtf8(path.clone()))?;
        Ok(FileContent {
            modified_at: modified_time(&path),
            path,
            content,
            format,
        })
    }

    /// Atomically writes editor text back in its original `format`.
    /// Returns the new modification time.
    pub fn write_file(
        &self,
        path: &Path,
        content: &str,
        format: TextFormat,
    ) -> Result<u64, FsError> {
        self.write_bytes(path, &text::encode(content, format))
    }

    /// Atomically writes raw bytes (see [`atomic::atomic_write`]).
    pub fn write_bytes(&self, path: &Path, bytes: &[u8]) -> Result<u64, FsError> {
        let path = match self.confine_existing(path) {
            Ok(path) => path,
            Err(FsError::NotFound(_)) => self.confine_new(path)?,
            Err(error) => return Err(error),
        };
        atomic::atomic_write(&path, bytes).map_err(|e| FsError::io(&path, e))?;
        Ok(modified_time(&path))
    }

    /// Creates a new Markdown file containing `# `; never overwrites.
    pub fn create_file(&self, path: &Path) -> Result<FileContent, FsError> {
        let path = self.confine_new(path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| FsError::io(&path, e))?;
        if let Err(error) = file.write_all(NEW_FILE_CONTENT.as_bytes()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(FsError::io(&path, error));
        }
        Ok(FileContent {
            modified_at: modified_time(&path),
            path,
            content: NEW_FILE_CONTENT.into(),
            format: TextFormat::default(),
        })
    }

    /// Creates a folder; never reuses an existing one.
    pub fn create_directory(&self, path: &Path) -> Result<DirEntry, FsError> {
        let path = self.confine_new(path)?;
        fs::create_dir(&path).map_err(|e| FsError::io(&path, e))?;
        let name = file_name_of(&path);
        Ok(dir_entry(name, path, true))
    }

    /// Creates `Untitled.md`, `Untitled 2.md`, ... (or `Untitled Folder`,
    /// `Untitled Folder 2`, ...) in `parent`, with no upper bound and
    /// without ever overwriting.
    pub fn create_untitled(&self, parent: &Path, kind: EntryKind) -> Result<DirEntry, FsError> {
        let parent = self.confine_existing(parent)?;
        if !parent.is_dir() {
            return Err(FsError::NotFound(parent));
        }
        let mut suffix: u64 = 1;
        loop {
            let base = match kind {
                EntryKind::File => "Untitled",
                EntryKind::Folder => "Untitled Folder",
            };
            let stem = if suffix == 1 {
                base.to_owned()
            } else {
                format!("{base} {suffix}")
            };
            let candidate = match kind {
                EntryKind::File => parent.join(format!("{stem}.md")),
                EntryKind::Folder => parent.join(stem),
            };
            let created = match kind {
                EntryKind::File => self
                    .create_file(&candidate)
                    .map(|file| dir_entry(file_name_of(&file.path), file.path, false)),
                EntryKind::Folder => self.create_directory(&candidate),
            };
            match created {
                Err(FsError::AlreadyExists(_)) => {
                    suffix = suffix.checked_add(1).ok_or_else(|| FsError::InvalidName {
                        name: base.into(),
                        reason: "untitled counter overflowed".into(),
                    })?;
                }
                other => return other,
            }
        }
    }

    /// Renames or moves `old` to `new`. Refuses to overwrite, refuses names
    /// with `/` (and `<>:"\|?*` on Windows). A case-only rename of the same
    /// file is allowed on case-insensitive file systems.
    pub fn rename(&self, old: &Path, new: &Path) -> Result<PathBuf, FsError> {
        let old = self.confine_entry(old)?;
        let new = self.confine_new(new)?;
        // Any entry at `new`, a dangling symlink included, blocks the rename,
        // unless both paths name the same regular entry (case-only rename on
        // a case-insensitive file system). Symlinks never take that exception:
        // a link and its target share a canonical path.
        if let Ok(existing) = fs::symlink_metadata(&new) {
            let is_link = |meta: &fs::Metadata| meta.file_type().is_symlink();
            let same_entry = !is_link(&existing)
                && !fs::symlink_metadata(&old).is_ok_and(|meta| is_link(&meta))
                && dunce::canonicalize(&new).ok() == dunce::canonicalize(&old).ok();
            if !same_entry {
                return Err(FsError::AlreadyExists(new));
            }
        }
        fs::rename(&old, &new).map_err(|e| FsError::io(&old, e))?;
        Ok(new)
    }

    /// Moves a file or folder to the OS trash.
    pub fn delete(&self, path: &Path) -> Result<(), FsError> {
        let path = self.confine_entry(path)?;
        if path == self.root {
            return Err(FsError::OutsideWorkspace(path));
        }
        trash::delete(&path).map_err(|e| FsError::Io {
            path: path.clone(),
            source: io::Error::other(e.to_string()),
        })
    }

    /// Duplicates `source` as `<stem> copy.<ext>`, then `<stem> copy 2.<ext>`,
    /// ... up to 999. `unsaved` replaces the disk content when the source is
    /// open with unsaved changes, so the copy matches what the user sees.
    pub fn duplicate(&self, source: &Path, unsaved: Option<&str>) -> Result<PathBuf, FsError> {
        let source_file = self.read_file(source)?;
        let content = unsaved.unwrap_or(&source_file.content);
        let parent = source_file
            .path
            .parent()
            .ok_or_else(|| FsError::NotFound(source_file.path.clone()))?;
        let stem = source_file
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = source_file
            .path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for n in 1..1000 {
            let suffix = if n == 1 {
                " copy".to_owned()
            } else {
                format!(" copy {n}")
            };
            let candidate = parent.join(format!("{stem}{suffix}{extension}"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    drop(file);
                    if let Err(error) = self.write_file(&candidate, content, source_file.format) {
                        // Leave no empty placeholder behind.
                        let _ = fs::remove_file(&candidate);
                        return Err(error);
                    }
                    return Ok(candidate);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(FsError::io(&candidate, error)),
            }
        }
        Err(FsError::AlreadyExists(source_file.path))
    }
}

/// Rejects names that cannot be a single file name on every supported OS.
pub fn validate_file_name(name: &str) -> Result<(), FsError> {
    let invalid = |reason: &str| {
        Err(FsError::InvalidName {
            name: name.to_owned(),
            reason: reason.to_owned(),
        })
    };
    if name.trim().is_empty() {
        return invalid("the name is empty");
    }
    if name == "." || name == ".." {
        return invalid("the name is reserved");
    }
    if name.contains('/') {
        return invalid("the name contains '/'");
    }
    if cfg!(windows) {
        if let Some(bad) = name
            .chars()
            .find(|c| r#"<>:"\|?*"#.contains(*c) || c.is_control())
        {
            return invalid(&format!(
                "the name contains {bad:?}, not allowed on Windows"
            ));
        }
        if name.ends_with(['.', ' ']) {
            return invalid("Windows names cannot end with a dot or a space");
        }
    }
    let single = Path::new(name).components().count() == 1
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        );
    if !single {
        return invalid("the name is not a single path component");
    }
    Ok(())
}

/// Folder visibility in the sidebar (Writer `directory_is_sidebar_visible`):
/// visible when it holds Markdown or when its tree holds no visible file at
/// all (a new folder stays available for authoring).
fn directory_is_visible(
    path: &Path,
    ignore: &WorkspaceIgnore,
    dirs_with_markdown: Option<&dyn MarkdownDirs>,
) -> Result<bool, FsError> {
    match dirs_with_markdown {
        Some(dirs) if dirs.holds_markdown(path) => Ok(true),
        Some(_) => tree_is_empty(path, ignore),
        None => Ok(matches!(
            classify(path, ignore)?,
            DirectoryContent::Markdown | DirectoryContent::Empty
        )),
    }
}

enum DirectoryContent {
    Markdown,
    Other,
    Empty,
}

fn classify(path: &Path, ignore: &WorkspaceIgnore) -> Result<DirectoryContent, FsError> {
    let mut content = DirectoryContent::Empty;
    for entry in fs::read_dir(path).map_err(|e| FsError::io(path, e))? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let child = entry.path();
        if ignore.excludes(&child, file_type.is_dir()) {
            continue;
        }
        if file_type.is_file() {
            if is_markdown_path(&child) {
                return Ok(DirectoryContent::Markdown);
            }
            content = DirectoryContent::Other;
        } else if file_type.is_dir() {
            match classify(&child, ignore)? {
                DirectoryContent::Markdown => return Ok(DirectoryContent::Markdown),
                DirectoryContent::Other => content = DirectoryContent::Other,
                DirectoryContent::Empty => {}
            }
        }
    }
    Ok(content)
}

fn tree_is_empty(path: &Path, ignore: &WorkspaceIgnore) -> Result<bool, FsError> {
    for entry in fs::read_dir(path).map_err(|e| FsError::io(path, e))? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let child = entry.path();
        if ignore.excludes(&child, file_type.is_dir()) {
            continue;
        }
        if file_type.is_file() || (file_type.is_dir() && !tree_is_empty(&child, ignore)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn dir_entry(name: String, path: PathBuf, is_dir: bool) -> DirEntry {
    DirEntry {
        is_markdown: !is_dir && is_markdown_path(&path),
        modified_at: modified_time(&path),
        created_at: created_time(&path),
        title: None,
        name,
        path,
        is_dir,
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn epoch_secs(time: io::Result<SystemTime>) -> Option<u64> {
    time.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// Modification time in seconds since the Unix epoch, 0 when unknown.
pub fn modified_time(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| epoch_secs(meta.modified()))
        .unwrap_or(0)
}

/// Creation time, falling back to the modification time.
pub fn created_time(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| epoch_secs(meta.created()).or_else(|| epoch_secs(meta.modified())))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::EpochCounter;
    use crate::index::index_workspace_with_global;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("hello.md"), "# Hello").unwrap();
        fs::write(dir.path().join("world.md"), "# World").unwrap();
        fs::write(dir.path().join("readme.txt"), "text").unwrap();
        fs::create_dir(dir.path().join("notes")).unwrap();
        fs::write(dir.path().join("notes").join("note.md"), "# Note").unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        fs::write(dir.path().join("empty").join("data.txt"), "data").unwrap();
        dir
    }

    fn ws(dir: &TempDir) -> WorkspaceFs {
        WorkspaceFs::new(dir.path()).unwrap()
    }

    fn ignore(fs: &WorkspaceFs) -> WorkspaceIgnore {
        WorkspaceIgnore::load_with_global(fs.root(), None)
    }

    // Ported from Writer `commands/fs.rs` tests.

    #[test]
    fn read_directory_sorts_dirs_first_without_blocking_on_titles() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let result = ws.read_directory(ws.root(), &ignore(&ws), None).unwrap();
        let names: Vec<_> = result.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["notes", "hello.md", "world.md"]);
        assert!(result[0].is_dir);
        // Titles now arrive through `titles::spawn_title_batches`.
        assert!(result.iter().all(|e| e.title.is_none()));
        assert_eq!(
            titles::extract_title(&result[1].path).as_deref(),
            Some("Hello")
        );
    }

    #[test]
    fn read_directory_includes_empty_folder_trees_but_filters_non_markdown_content() {
        let dir = setup_test_dir();
        fs::create_dir_all(dir.path().join("drafts").join("nested")).unwrap();
        fs::write(dir.path().join("drafts").join(".DS_Store"), "hidden").unwrap();
        let ws = ws(&dir);
        let result = ws.read_directory(ws.root(), &ignore(&ws), None).unwrap();
        assert_eq!(result.len(), 4);
        assert!(result.iter().any(|e| e.name == "drafts" && e.is_dir));
        assert!(!result.iter().any(|e| e.name == "empty"));
        assert!(result.iter().all(|e| e.is_dir || e.is_markdown));
    }

    #[test]
    fn read_directory_uses_index_when_ready() {
        let dir = setup_test_dir();
        fs::create_dir_all(dir.path().join("drafts").join("nested")).unwrap();
        let ws = ws(&dir);
        let index =
            index_workspace_with_global(ws.root(), None, &EpochCounter::new().token()).unwrap();
        let result = ws
            .read_directory(
                ws.root(),
                &index.ignore,
                Some(&index.dirs_with_markdown as &dyn MarkdownDirs),
            )
            .unwrap();
        assert_eq!(result.len(), 4);
        assert!(result.iter().any(|e| e.name == "drafts" && e.is_dir));
        assert!(!result.iter().any(|e| e.name == "empty"));
        assert!(result[0].is_dir);
    }

    #[test]
    fn directory_visibility_surfaces_read_errors() {
        let dir = TempDir::new().unwrap();
        let ignore = WorkspaceIgnore::bootstrap(dir.path());
        let missing = dir.path().join("missing");
        assert!(matches!(
            directory_is_visible(&missing, &ignore, None),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn read_file_returns_content() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let file = ws.read_file(&dir.path().join("hello.md")).unwrap();
        assert_eq!(file.content, "# Hello");
        assert!(file.modified_at > 0);
    }

    #[test]
    fn read_file_not_found() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        assert!(matches!(
            ws.read_file(&dir.path().join("missing.md")),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn write_file_atomic() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let path = dir.path().join("hello.md");
        ws.write_file(&path, "# Updated", TextFormat::default())
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Updated");
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".cerata-tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn create_file_already_exists() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        assert!(matches!(
            ws.create_file(&dir.path().join("hello.md")),
            Err(FsError::AlreadyExists(_))
        ));
        assert_eq!(
            fs::read_to_string(dir.path().join("hello.md")).unwrap(),
            "# Hello"
        );
    }

    #[test]
    fn create_directory_already_exists() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        assert!(matches!(
            ws.create_directory(&dir.path().join("notes")),
            Err(FsError::AlreadyExists(_))
        ));
    }

    #[test]
    fn create_untitled_retries_collisions_without_overwriting() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Untitled.md"), "keep").unwrap();
        let ws = ws(&dir);
        let entry = ws.create_untitled(ws.root(), EntryKind::File).unwrap();
        assert_eq!(entry.name, "Untitled 2.md");
        assert_eq!(
            fs::read_to_string(dir.path().join("Untitled.md")).unwrap(),
            "keep"
        );
        assert_eq!(fs::read_to_string(&entry.path).unwrap(), NEW_FILE_CONTENT);
    }

    #[test]
    fn create_untitled_has_no_999_name_ceiling() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Untitled.md"), "").unwrap();
        for n in 2..=1000 {
            fs::write(dir.path().join(format!("Untitled {n}.md")), "").unwrap();
        }
        let ws = ws(&dir);
        let entry = ws.create_untitled(ws.root(), EntryKind::File).unwrap();
        assert_eq!(entry.name, "Untitled 1001.md");
    }

    #[test]
    fn create_untitled_folder_retries_collisions() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("Untitled Folder")).unwrap();
        let ws = ws(&dir);
        let entry = ws.create_untitled(ws.root(), EntryKind::Folder).unwrap();
        assert_eq!(entry.name, "Untitled Folder 2");
        assert!(entry.is_dir);
    }

    #[test]
    fn create_untitled_rejects_parent_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let ws = ws(&dir);
        assert!(matches!(
            ws.create_untitled(outside.path(), EntryKind::File),
            Err(FsError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn delete_entry_moves_to_trash() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let path = dir.path().join("hello.md");
        if let Err(error) = ws.delete(&path) {
            // Headless CI sessions may have no trash; the file must survive.
            eprintln!("skipped: no trash available here: {error}");
            assert!(path.exists());
            return;
        }
        assert!(!path.exists());
    }

    #[test]
    fn rename_entry() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let new = ws
            .rename(
                &dir.path().join("hello.md"),
                &dir.path().join("greeting.md"),
            )
            .unwrap();
        assert!(new.ends_with("greeting.md"));
        assert!(!dir.path().join("hello.md").exists());
        assert!(dir.path().join("greeting.md").exists());
    }

    #[test]
    fn rename_entry_not_found() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        assert!(matches!(
            ws.rename(&dir.path().join("nope.md"), &dir.path().join("x.md")),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn rename_entry_already_exists() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        assert!(matches!(
            ws.rename(&dir.path().join("hello.md"), &dir.path().join("world.md")),
            Err(FsError::AlreadyExists(_))
        ));
        assert_eq!(
            fs::read_to_string(dir.path().join("world.md")).unwrap(),
            "# World"
        );
    }

    #[test]
    fn error_display_names_the_path() {
        let error = FsError::NotFound(PathBuf::from("a.md"));
        assert_eq!(error.to_string(), "not found: a.md");
    }

    // Cerata additions (fixed defects).

    #[test]
    fn rename_case_only_is_allowed() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        ws.rename(&dir.path().join("hello.md"), &dir.path().join("Hello.md"))
            .unwrap();
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"Hello.md".to_owned()), "{names:?}");
    }

    #[test]
    fn rename_rejects_forbidden_names() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let old = dir.path().join("hello.md");
        for bad in ["", "..", " "] {
            assert!(
                matches!(
                    ws.rename(&old, &dir.path().join(bad)),
                    Err(FsError::InvalidName { .. }) | Err(FsError::OutsideWorkspace(_))
                ),
                "{bad:?}"
            );
        }
        assert!(validate_file_name("a/b.md").is_err());
        if cfg!(windows) {
            for bad in [
                "a<b.md", "a>b.md", "a:b.md", "a\"b.md", "a|b.md", "a?b.md", "a*b.md",
            ] {
                assert!(validate_file_name(bad).is_err(), "{bad}");
            }
        } else {
            assert!(validate_file_name("a:b?.md").is_ok());
        }
        assert!(old.exists());
    }

    #[test]
    fn operations_outside_the_root_are_refused() {
        let dir = setup_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("victim.md"), "safe").unwrap();
        let ws = ws(&dir);
        let victim = outside.path().join("victim.md");
        let escape = dir
            .path()
            .join("..")
            .join(outside.path().file_name().unwrap())
            .join("victim.md");
        for path in [&victim, &escape] {
            assert!(matches!(
                ws.read_file(path),
                Err(FsError::OutsideWorkspace(_))
            ));
            assert!(matches!(
                ws.write_file(path, "pwned", TextFormat::default()),
                Err(FsError::OutsideWorkspace(_))
            ));
            assert!(matches!(ws.delete(path), Err(FsError::OutsideWorkspace(_))));
            assert!(matches!(
                ws.rename(&dir.path().join("hello.md"), path),
                Err(FsError::OutsideWorkspace(_))
            ));
        }
        assert!(matches!(
            ws.create_file(&outside.path().join("new.md")),
            Err(FsError::OutsideWorkspace(_))
        ));
        assert_eq!(fs::read_to_string(&victim).unwrap(), "safe");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_root_is_refused() {
        let dir = setup_test_dir();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("victim.md"), "safe").unwrap();
        std::os::unix::fs::symlink(outside.path().join("victim.md"), dir.path().join("link.md"))
            .unwrap();
        let ws = ws(&dir);
        assert!(matches!(
            ws.write_file(&dir.path().join("link.md"), "pwned", TextFormat::default()),
            Err(FsError::OutsideWorkspace(_))
        ));
    }

    /// Creates a file symlink; `false` where the account cannot (Windows
    /// without Developer Mode or elevation).
    fn file_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(target, link);
        if let Err(error) = &created {
            eprintln!("skipped: cannot create symlinks here: {error}");
        }
        created.is_ok()
    }

    #[test]
    fn rename_and_delete_act_on_a_symlink_never_on_its_target() {
        let dir = setup_test_dir();
        fs::create_dir(dir.path().join("archive")).unwrap();
        let target = dir.path().join("archive").join("real.md");
        fs::write(&target, "kept").unwrap();
        let link = dir.path().join("link.md");
        if !file_symlink(&target, &link) {
            return;
        }
        let ws = ws(&dir);

        let renamed = ws.rename(&link, &dir.path().join("moved.md")).unwrap();
        assert!(
            fs::symlink_metadata(&renamed)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept");

        match ws.delete(&renamed) {
            Ok(()) => assert!(fs::symlink_metadata(&renamed).is_err()),
            // Headless CI sessions may have no trash.
            Err(error) => eprintln!("delete skipped: no trash available here: {error}"),
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
    }

    #[test]
    fn rename_never_replaces_an_existing_link_or_its_target() {
        let dir = setup_test_dir();
        let target = dir.path().join("hello.md");
        let link = dir.path().join("link.md");
        if !file_symlink(&target, &link) {
            return;
        }
        let dangling = dir.path().join("dangling.md");
        assert!(file_symlink(&dir.path().join("gone.md"), &dangling));
        let ws = ws(&dir);
        let world = dir.path().join("world.md");
        for (old, new) in [(&link, &target), (&world, &link), (&world, &dangling)] {
            assert!(
                matches!(ws.rename(old, new), Err(FsError::AlreadyExists(_))),
                "{} -> {}",
                old.display(),
                new.display()
            );
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "# Hello");
        assert_eq!(fs::read_to_string(&world).unwrap(), "# World");
        assert!(
            fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn line_endings_and_bom_survive_a_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("win.md");
        fs::write(&path, b"\xEF\xBB\xBF# A\r\nb\r\n").unwrap();
        let ws = ws(&dir);
        let file = ws.read_file(&path).unwrap();
        assert_eq!(file.content, "# A\nb\n");
        ws.write_file(&path, "# A\nb\nc\n", file.format).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\xEF\xBB\xBF# A\r\nb\r\nc\r\n");
    }

    #[test]
    fn duplicate_names_copies_and_prefers_unsaved_text() {
        let dir = setup_test_dir();
        let ws = ws(&dir);
        let source = dir.path().join("hello.md");
        let first = ws.duplicate(&source, None).unwrap();
        assert!(first.ends_with("hello copy.md"));
        assert_eq!(fs::read_to_string(&first).unwrap(), "# Hello");
        let second = ws.duplicate(&source, Some("# Unsaved")).unwrap();
        assert!(second.ends_with("hello copy 2.md"));
        assert_eq!(fs::read_to_string(&second).unwrap(), "# Unsaved");
    }
}
