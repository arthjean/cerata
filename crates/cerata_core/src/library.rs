//! Libraries (ADR 0009, US-076): folders of notes recorded in the registry
//! `libraries.json` of the app data folder, each with an opaque 128-bit id
//! written once in `{root}/.cerata/id`.
//!
//! The marker lets Cerata recognize a library that moved (its entry follows
//! the folder) and tell a copy from the original (the copy gets its own
//! id, the only case where a marker is rewritten). A folder Cerata cannot
//! write to keeps a registry entry recognized by its path only.
//!
//! On the first start of a version with libraries, every folder of
//! `recent_workspaces.json` that still exists becomes a library; that file
//! is left as it is.

use crate::fs::{FsError, WorkspaceFs, validate_file_name};
use crate::json_store::{JsonStore, StoreError};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const REGISTRY_FILE: &str = "libraries.json";
/// Writer's list of recent workspaces, read once for the migration.
pub const LEGACY_RECENTS_FILE: &str = "recent_workspaces.json";
/// Name the welcome form proposes for a first library.
pub const DEFAULT_LIBRARY_NAME: &str = "Cerata";
const REGISTRY_VERSION: u32 = 1;
/// A marker is 32 hex characters and a newline; anything much longer is
/// not one.
const MARKER_MAX_BYTES: u64 = 64;

/// A registry entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    pub last_opened_at: u64,
    #[serde(default)]
    pub is_default: bool,
}

impl Library {
    pub fn key(&self) -> LibraryKey {
        LibraryKey {
            id: self.id.clone(),
            root: self.path.clone(),
        }
    }

    /// `false` for a library whose folder is gone ("Not found").
    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }
}

/// What app state (sessions, pins) is keyed by: the library id, with paths
/// stored relative to `root`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryKey {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct Registry {
    version: u32,
    #[serde(default)]
    libraries: Vec<Library>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            libraries: Vec::new(),
        }
    }
}

/// What opening a folder told about its identity, for the window notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryNotice {
    /// The marker's library was registered at a path that no longer exists.
    Moved,
    /// The marker's library still exists elsewhere: this folder is a copy.
    Copy { original: String },
    /// No marker could be written.
    ReadOnly,
}

impl fmt::Display for LibraryNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Moved => f.write_str("Library moved: Cerata updated its location"),
            Self::Copy { original } => write!(
                f,
                "This folder is a copy of {original}. Cerata gave it its own identity."
            ),
            Self::ReadOnly => f.write_str(
                "Cerata can't write to this folder. It will be recognized by its path only.",
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedLibrary {
    pub library: Library,
    pub notice: Option<LibraryNotice>,
}

/// The content of `{root}/.cerata/id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    Absent,
    Id(String),
    /// Unreadable or malformed: ignored, never overwritten.
    Unusable,
}

pub fn read_marker(root: &Path) -> Marker {
    let path = paths::library_marker_path(root);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        // A file named `.cerata` holds no marker: Unix says `ENOTDIR` where
        // Windows says not found. Writing one then fails, without touching
        // that file, and the folder is known by its path.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Marker::Absent;
        }
        Err(error) => {
            log::warn!("library marker {} unreadable: {error}", path.display());
            return Marker::Unusable;
        }
    };
    if !meta.is_file() || meta.len() > MARKER_MAX_BYTES {
        log::warn!("library marker {} is not an id", path.display());
        return Marker::Unusable;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let id = text.trim_end_matches(['\n', '\r']).to_ascii_lowercase();
            if is_library_id(&id) {
                Marker::Id(id)
            } else {
                log::warn!("library marker {} is malformed", path.display());
                Marker::Unusable
            }
        }
        Err(error) => {
            log::warn!("library marker {} unreadable: {error}", path.display());
            Marker::Unusable
        }
    }
}

/// 32 lowercase hexadecimal characters.
pub fn is_library_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 16 random bytes from the OS, as 32 lowercase hex characters.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    if let Err(error) = getrandom::fill(&mut bytes) {
        // Never seen in practice; std's per-process random hasher keys
        // and the clock still make a collision implausible.
        log::error!("OS randomness unavailable ({error}); deriving a library id");
        use std::hash::{BuildHasher, Hasher};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for half in bytes.chunks_mut(8) {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(nanos);
            hasher.write_u32(std::process::id());
            half.copy_from_slice(&hasher.finish().to_le_bytes());
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes `{root}/.cerata/id` if it does not exist; never replaces it.
fn write_marker(root: &Path, id: &str) -> Result<(), FsError> {
    let fs = WorkspaceFs::new(root)?;
    // `AlreadyExists` here means a file named `.cerata`, not a marker.
    fs.ensure_directory(&root.join(paths::WORKSPACE_DIR))
        .map_err(|error| match error {
            FsError::AlreadyExists(path) => FsError::Io {
                path,
                source: io::Error::other("not a folder"),
            },
            other => other,
        })?;
    fs.write_new_bytes(
        &paths::library_marker_path(root),
        format!("{id}\n").as_bytes(),
    )?;
    Ok(())
}

/// Replaces the marker of a copied library, atomically.
fn rewrite_marker(root: &Path, id: &str) -> Result<(), FsError> {
    let fs = WorkspaceFs::new(root)?;
    fs.write_bytes(
        &paths::library_marker_path(root),
        format!("{id}\n").as_bytes(),
    )?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl Registry {
    fn index_of(&self, id: &str) -> Option<usize> {
        self.libraries.iter().position(|l| l.id == id)
    }

    fn index_at(&self, path: &Path) -> Option<usize> {
        self.libraries.iter().position(|l| l.path == path)
    }

    fn fresh_id(&self) -> String {
        loop {
            let id = new_id();
            if self.index_of(&id).is_none() {
                return id;
            }
        }
    }

    fn push(&mut self, id: String, path: &Path, name: String) {
        let now = now_secs();
        self.libraries.push(Library {
            id,
            path: path.to_path_buf(),
            name,
            created_at: now,
            last_opened_at: now,
            is_default: false,
        });
    }

    /// The entry of `root`, created with a fresh id when there is none.
    fn entry_at(&mut self, root: &Path) -> String {
        match self.index_at(root) {
            Some(ix) => self.libraries[ix].id.clone(),
            None => {
                let id = self.fresh_id();
                self.push(id.clone(), root, folder_name(root));
                id
            }
        }
    }

    fn resolve_marked(&mut self, root: &Path, id: String) -> (String, Option<LibraryNotice>) {
        let Some(ix) = self.index_of(&id) else {
            // Unknown id (a new registry, another machine): the marker wins
            // over a path-only entry.
            return match self.index_at(root) {
                Some(at) => {
                    self.libraries[at].id = id.clone();
                    (id, None)
                }
                None => {
                    self.push(id.clone(), root, folder_name(root));
                    (id, None)
                }
            };
        };
        let known = self.libraries[ix].path.clone();
        if known == root {
            return (id, None);
        }
        let original_alive =
            known.is_dir() && matches!(read_marker(&known), Marker::Id(ref other) if *other == id);
        if original_alive {
            let original = self.libraries[ix].name.clone();
            let copy = self.entry_at(root);
            return match rewrite_marker(root, &copy) {
                Ok(()) => (copy, Some(LibraryNotice::Copy { original })),
                Err(error) => {
                    log::warn!("copy of {original} keeps the original marker: {error}");
                    (copy, Some(LibraryNotice::ReadOnly))
                }
            };
        }
        self.libraries.retain(|l| l.path != root || l.id == id);
        if let Some(ix) = self.index_of(&id) {
            self.libraries[ix].path = root.to_path_buf();
        }
        (id, Some(LibraryNotice::Moved))
    }

    fn resolve_unmarked(&mut self, root: &Path, write: bool) -> (String, Option<LibraryNotice>) {
        let id = self.entry_at(root);
        if !write {
            return (id, None);
        }
        match write_marker(root, &id) {
            Ok(()) | Err(FsError::AlreadyExists(_)) => (id, None),
            Err(error) => {
                log::warn!("no library marker in {}: {error}", root.display());
                (id, Some(LibraryNotice::ReadOnly))
            }
        }
    }
}

/// Result of [`LibraryRegistry::locate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocateOutcome {
    Located(Library),
    /// The folder's marker names another library, or it has none.
    NotThisLibrary {
        name: String,
    },
    /// The id is no longer in the registry.
    Unknown,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// The name cannot be a folder name; the reason is shown as is.
    #[error("{0}")]
    InvalidName(String),
    /// A non-empty folder without a marker is there.
    #[error("{} exists", .0.display())]
    FolderExists(PathBuf),
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The reason `name` cannot name a library folder, capitalized for the
/// form, or `None` when it can.
pub fn name_problem(name: &str) -> Option<String> {
    match validate_file_name(name) {
        Ok(()) => None,
        Err(FsError::InvalidName { reason, .. }) => {
            let mut chars = reason.chars();
            Some(match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => reason,
            })
        }
        Err(other) => Some(other.to_string()),
    }
}

/// `libraries.json` in the app data folder.
#[derive(Debug, Clone)]
pub struct LibraryRegistry {
    store: JsonStore,
    legacy: PathBuf,
}

impl LibraryRegistry {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            store: JsonStore::new(data_dir.join(REGISTRY_FILE)),
            legacy: data_dir.join(LEGACY_RECENTS_FILE),
        }
    }

    /// Every library, most recently opened first.
    pub fn list(&self) -> Result<Vec<Library>, StoreError> {
        let mut libraries = self.read()?.libraries;
        libraries.sort_by(|a, b| {
            b.last_opened_at
                .cmp(&a.last_opened_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(libraries)
    }

    pub fn get(&self, id: &str) -> Result<Option<Library>, StoreError> {
        Ok(self.read()?.libraries.into_iter().find(|l| l.id == id))
    }

    pub fn default_library(&self) -> Result<Option<Library>, StoreError> {
        Ok(self.read()?.libraries.into_iter().find(|l| l.is_default))
    }

    /// The registered library whose folder holds `path` (the deepest one
    /// when libraries nest), if its folder exists.
    pub fn containing(&self, path: &Path) -> Result<Option<Library>, StoreError> {
        Ok(self
            .read()?
            .libraries
            .into_iter()
            .filter(|l| path.starts_with(&l.path) && l.exists())
            .max_by_key(|l| l.path.components().count()))
    }

    /// Makes `id` the only default library. `false` when it is unknown.
    pub fn set_default(&self, id: &str) -> Result<bool, StoreError> {
        self.update(|registry| {
            let known = registry.index_of(id).is_some();
            if known {
                for library in &mut registry.libraries {
                    library.is_default = library.id == id;
                }
            }
            known
        })
    }

    /// Forgets `id`. No file is touched: not the folder, its marker or its
    /// store. `false` when it is unknown.
    pub fn remove(&self, id: &str) -> Result<bool, StoreError> {
        self.update(|registry| {
            let before = registry.libraries.len();
            registry.libraries.retain(|l| l.id != id);
            registry.libraries.len() != before
        })
    }

    /// Identifies the folder `root` (canonical, existing) by its marker,
    /// writing one when it has none, and records the opening.
    pub fn resolve(&self, root: &Path) -> Result<OpenedLibrary, StoreError> {
        let marker = read_marker(root);
        self.update(|registry| {
            let (id, notice) = match marker {
                Marker::Id(id) => registry.resolve_marked(root, id),
                Marker::Absent => registry.resolve_unmarked(root, true),
                Marker::Unusable => registry.resolve_unmarked(root, false),
            };
            let now = now_secs();
            let ix = registry.index_of(&id);
            let library = match ix {
                Some(ix) => {
                    let entry = &mut registry.libraries[ix];
                    entry.last_opened_at = now.max(entry.last_opened_at);
                    entry.clone()
                }
                // Unreachable: every branch above registers `id`.
                None => Library {
                    id,
                    path: root.to_path_buf(),
                    name: folder_name(root),
                    created_at: now,
                    last_opened_at: now,
                    is_default: false,
                },
            };
            OpenedLibrary { library, notice }
        })
    }

    /// "Locate…": `folder` replaces the path of `id` when its marker holds
    /// that id; otherwise nothing changes.
    pub fn locate(&self, id: &str, folder: &Path) -> Result<LocateOutcome, StoreError> {
        let folder = dunce::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        let matches = read_marker(&folder) == Marker::Id(id.to_owned());
        self.update(|registry| {
            let Some(ix) = registry.index_of(id) else {
                return LocateOutcome::Unknown;
            };
            if !matches {
                return LocateOutcome::NotThisLibrary {
                    name: registry.libraries[ix].name.clone(),
                };
            }
            registry
                .libraries
                .retain(|l| l.path != folder || l.id == id);
            match registry.index_of(id) {
                Some(ix) => {
                    registry.libraries[ix].path = folder.clone();
                    LocateOutcome::Located(registry.libraries[ix].clone())
                }
                None => LocateOutcome::Unknown,
            }
        })
    }

    /// The welcome form's "Create": `{parent}/{name}` becomes the default
    /// library. A folder there that already has a marker is reused, an
    /// empty one is adopted, a non-empty one without a marker is refused
    /// with [`CreateError::FolderExists`]. On an error nothing is recorded,
    /// and a folder this call created is removed again.
    pub fn create(&self, parent: &Path, name: &str) -> Result<Library, CreateError> {
        if let Some(problem) = name_problem(name) {
            return Err(CreateError::InvalidName(problem));
        }
        let parent = dunce::canonicalize(parent).map_err(|e| FsError::io(parent, e))?;
        let path = parent.join(name);
        let created = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {
                if let Marker::Id(_) = read_marker(&path) {
                    return self.adopt(&path);
                }
                let empty = std::fs::read_dir(&path)
                    .map_err(|e| FsError::io(&path, e))?
                    .next()
                    .is_none();
                if !empty {
                    return Err(CreateError::FolderExists(path));
                }
                false
            }
            Ok(_) => return Err(FsError::AlreadyExists(path).into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&path).map_err(|e| FsError::io(&path, e))?;
                true
            }
            Err(error) => return Err(FsError::io(&path, error).into()),
        };
        let path = dunce::canonicalize(&path).map_err(|e| FsError::io(&path, e))?;
        let undo = |path: &Path| {
            if created {
                let _ = std::fs::remove_dir(path.join(paths::WORKSPACE_DIR));
                let _ = std::fs::remove_dir(path);
            }
        };
        let id = match self.read() {
            Ok(registry) => registry.fresh_id(),
            Err(error) => {
                undo(&path);
                return Err(error.into());
            }
        };
        if let Err(error) = write_marker(&path, &id) {
            let _ = std::fs::remove_file(paths::library_marker_path(&path));
            undo(&path);
            return Err(error.into());
        }
        let name = name.to_owned();
        let recorded = self.update(|registry| {
            registry.push(id.clone(), &path, name);
            for library in &mut registry.libraries {
                library.is_default = library.id == id;
            }
            registry
                .index_of(&id)
                .map(|ix| registry.libraries[ix].clone())
        });
        match recorded {
            Ok(Some(library)) => Ok(library),
            Ok(None) => Err(CreateError::Fs(FsError::NotFound(path))),
            Err(error) => Err(error.into()),
        }
    }

    /// "Open" after [`CreateError::FolderExists`], or "Create" on a folder
    /// that is already a library: `path` is identified and made default.
    pub fn adopt(&self, path: &Path) -> Result<Library, CreateError> {
        let root = dunce::canonicalize(path).map_err(|e| FsError::io(path, e))?;
        let opened = self.resolve(&root)?;
        self.set_default(&opened.library.id)?;
        Ok(Library {
            is_default: true,
            ..opened.library
        })
    }

    fn read(&self) -> Result<Registry, StoreError> {
        self.migrate()?;
        self.store.load()
    }

    fn update<R>(&self, change: impl FnOnce(&mut Registry) -> R) -> Result<R, StoreError> {
        self.migrate()?;
        self.store.update(change)
    }

    /// Without `libraries.json`, the folders of `recent_workspaces.json`
    /// that still exist become libraries, in their order. Runs once: the
    /// registry file exists afterwards, even empty.
    fn migrate(&self) -> Result<(), StoreError> {
        if std::fs::symlink_metadata(self.store.path()).is_ok() {
            return Ok(());
        }
        let recents: Vec<String> = match std::fs::read_to_string(&self.legacy) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::warn!("{} not migrated: {error}", self.legacy.display());
                Vec::new()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                log::warn!("{} not migrated: {error}", self.legacy.display());
                Vec::new()
            }
        };
        let now = now_secs();
        self.store.update(|registry: &mut Registry| {
            for (ix, path) in recents.iter().enumerate() {
                let path = PathBuf::from(path);
                if !path.is_dir() {
                    log::info!("{} is gone and not migrated", path.display());
                    continue;
                }
                let path = dunce::canonicalize(&path).unwrap_or(path);
                if registry.index_at(&path).is_some() {
                    continue;
                }
                let id = match read_marker(&path) {
                    Marker::Id(id) if registry.index_of(&id).is_none() => id,
                    _ => registry.fresh_id(),
                };
                registry.libraries.push(Library {
                    id,
                    name: folder_name(&path),
                    path,
                    created_at: now,
                    // Keeps the recents' order once sorted by last opening.
                    last_opened_at: now.saturating_sub(ix as u64),
                    is_default: false,
                });
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Env {
        data: TempDir,
        notes: TempDir,
        registry: LibraryRegistry,
    }

    fn env() -> Env {
        let data = TempDir::new().unwrap();
        let registry = LibraryRegistry::new(data.path());
        Env {
            data,
            notes: TempDir::new().unwrap(),
            registry,
        }
    }

    impl Env {
        fn folder(&self, name: &str) -> PathBuf {
            let path = self.notes.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            dunce::canonicalize(path).unwrap()
        }
    }

    fn marker_of(root: &Path) -> String {
        std::fs::read_to_string(paths::library_marker_path(root)).unwrap()
    }

    #[test]
    fn ids_are_32_lowercase_hex_characters_and_differ() {
        let a = new_id();
        let b = new_id();
        assert!(is_library_id(&a), "{a}");
        assert_ne!(a, b);
        assert!(!is_library_id("ABC"));
        assert!(!is_library_id(&"g".repeat(32)));
    }

    #[test]
    fn opening_a_folder_writes_its_marker_and_registers_it() {
        let env = env();
        let root = env.folder("Notes");
        let opened = env.registry.resolve(&root).unwrap();
        assert_eq!(opened.notice, None);
        let library = opened.library;
        assert!(is_library_id(&library.id));
        assert_eq!(library.path, root);
        assert_eq!(library.name, "Notes");
        assert_eq!(marker_of(&root), format!("{}\n", library.id));
        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(env.data.path().join(REGISTRY_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(json["version"], 1);
        let entry = &json["libraries"][0];
        for field in [
            "id",
            "path",
            "name",
            "created_at",
            "last_opened_at",
            "is_default",
        ] {
            assert!(entry.get(field).is_some(), "{field} missing: {entry}");
        }
    }

    #[test]
    fn reopening_keeps_the_marker_untouched() {
        let env = env();
        let root = env.folder("Notes");
        let first = env.registry.resolve(&root).unwrap().library;
        let marker = paths::library_marker_path(&root);
        let before = std::fs::metadata(&marker).unwrap().modified().unwrap();
        let second = env.registry.resolve(&root).unwrap();
        assert_eq!(second.library.id, first.id);
        assert_eq!(second.notice, None);
        assert_eq!(
            std::fs::metadata(&marker).unwrap().modified().unwrap(),
            before
        );
        assert_eq!(env.registry.list().unwrap().len(), 1);
    }

    #[test]
    fn a_moved_library_keeps_its_id_and_its_entry_follows() {
        let env = env();
        let root = env.folder("Notes");
        let id = env.registry.resolve(&root).unwrap().library.id;
        let moved = env.notes.path().join("Moved");
        std::fs::rename(&root, &moved).unwrap();
        let moved = dunce::canonicalize(moved).unwrap();
        let opened = env.registry.resolve(&moved).unwrap();
        assert_eq!(opened.library.id, id);
        assert_eq!(opened.notice, Some(LibraryNotice::Moved));
        assert_eq!(
            opened.notice.unwrap().to_string(),
            "Library moved: Cerata updated its location"
        );
        let all = env.registry.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, moved);
        assert_eq!(marker_of(&moved), format!("{id}\n"));
    }

    #[test]
    fn a_copied_library_gets_its_own_identity() {
        let env = env();
        let root = env.folder("Notes");
        let id = env.registry.resolve(&root).unwrap().library.id;
        let copy = env.folder("Notes 2");
        std::fs::create_dir_all(copy.join(".cerata")).unwrap();
        std::fs::copy(
            paths::library_marker_path(&root),
            paths::library_marker_path(&copy),
        )
        .unwrap();
        let opened = env.registry.resolve(&copy).unwrap();
        assert_ne!(opened.library.id, id);
        assert_eq!(
            opened.notice.as_ref().map(ToString::to_string).as_deref(),
            Some("This folder is a copy of Notes. Cerata gave it its own identity.")
        );
        assert_eq!(marker_of(&copy), format!("{}\n", opened.library.id));
        assert_eq!(marker_of(&root), format!("{id}\n"));
        assert_eq!(env.registry.list().unwrap().len(), 2);
        // The copy is now known as itself.
        assert_eq!(env.registry.resolve(&copy).unwrap().notice, None);
    }

    /// `.cerata` is a file here, so no marker can be written: the folder
    /// is recognized by its path, with the same id each time.
    #[test]
    fn a_folder_without_a_writable_marker_is_known_by_its_path() {
        let env = env();
        let root = env.folder("Locked");
        std::fs::write(root.join(".cerata"), "not a folder").unwrap();
        let first = env.registry.resolve(&root).unwrap();
        assert_eq!(
            first.notice.as_ref().map(ToString::to_string).as_deref(),
            Some("Cerata can't write to this folder. It will be recognized by its path only.")
        );
        let second = env.registry.resolve(&root).unwrap();
        assert_eq!(second.library.id, first.library.id);
        assert_eq!(
            std::fs::read_to_string(root.join(".cerata")).unwrap(),
            "not a folder"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_gets_no_marker() {
        use std::os::unix::fs::PermissionsExt;
        let env = env();
        let root = env.folder("ReadOnly");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let opened = env.registry.resolve(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(opened.notice, Some(LibraryNotice::ReadOnly));
        assert!(!paths::library_marker_path(&root).exists());
    }

    #[test]
    fn a_malformed_marker_is_ignored_and_never_overwritten() {
        let env = env();
        let root = env.folder("Odd");
        std::fs::create_dir_all(root.join(".cerata")).unwrap();
        std::fs::write(paths::library_marker_path(&root), "hello\n").unwrap();
        let first = env.registry.resolve(&root).unwrap();
        assert_eq!(first.notice, None);
        assert!(is_library_id(&first.library.id));
        assert_eq!(marker_of(&root), "hello\n");
        assert_eq!(
            env.registry.resolve(&root).unwrap().library.id,
            first.library.id
        );
    }

    #[test]
    fn recent_workspaces_migrate_once_and_missing_folders_are_skipped() {
        let env = env();
        let a = env.folder("A");
        let b = env.folder("B");
        let gone = env.notes.path().join("Gone");
        let recents = serde_json::to_string(&[
            b.display().to_string(),
            gone.display().to_string(),
            a.display().to_string(),
        ])
        .unwrap();
        let legacy = env.data.path().join(LEGACY_RECENTS_FILE);
        std::fs::write(&legacy, &recents).unwrap();
        let libraries = env.registry.list().unwrap();
        let paths: Vec<&Path> = libraries.iter().map(|l| l.path.as_path()).collect();
        assert_eq!(paths, [b.as_path(), a.as_path()]);
        assert!(libraries.iter().all(|l| is_library_id(&l.id)));
        // The original file stays, and no marker is written by the
        // migration itself.
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), recents);
        assert!(!paths::library_marker_path(&a).exists());
        // Opening a migrated folder writes the id it was given.
        let id_a = libraries.iter().find(|l| l.path == a).unwrap().id.clone();
        assert_eq!(env.registry.resolve(&a).unwrap().library.id, id_a);
        assert_eq!(marker_of(&a), format!("{id_a}\n"));
        // Once the registry exists, the recents are not read again.
        env.registry.remove(&id_a).unwrap();
        assert_eq!(env.registry.list().unwrap().len(), 1);
    }

    #[test]
    fn default_remove_and_containing() {
        let env = env();
        let a = env.registry.resolve(&env.folder("A")).unwrap().library;
        let b = env.registry.resolve(&env.folder("B")).unwrap().library;
        assert_eq!(env.registry.default_library().unwrap(), None);
        assert!(env.registry.set_default(&a.id).unwrap());
        assert!(env.registry.set_default(&b.id).unwrap());
        let default = env.registry.default_library().unwrap().unwrap();
        assert_eq!(default.id, b.id);
        assert_eq!(
            env.registry
                .list()
                .unwrap()
                .iter()
                .filter(|l| l.is_default)
                .count(),
            1
        );
        let inside = b.path.join("sub").join("x.md");
        assert_eq!(
            env.registry.containing(&inside).unwrap().map(|l| l.id),
            Some(b.id.clone())
        );
        assert!(env.registry.remove(&b.id).unwrap());
        assert!(!env.registry.remove(&b.id).unwrap());
        assert_eq!(env.registry.containing(&inside).unwrap(), None);
        // Removing touched no file.
        assert!(paths::library_marker_path(&b.path).exists());
    }

    /// Ports Writer `stores.test.ts:881` (recent workspaces, now the
    /// registry): removing one drops only it and keeps the others' order.
    #[test]
    fn removing_a_library_keeps_the_others_in_order() {
        let env = env();
        let names = ["A", "B", "C"];
        let ids: Vec<String> = names
            .iter()
            .map(|name| env.registry.resolve(&env.folder(name)).unwrap().library.id)
            .collect();
        // Opened C, then B, then A: A is the most recent.
        env.registry
            .update(|registry| {
                for (ix, library) in registry.libraries.iter_mut().enumerate() {
                    library.last_opened_at = 100 - ix as u64;
                }
            })
            .unwrap();
        let order = |env: &Env| -> Vec<String> {
            env.registry
                .list()
                .unwrap()
                .into_iter()
                .map(|l| l.name)
                .collect()
        };
        assert_eq!(order(&env), names);
        env.registry.remove(&ids[1]).unwrap();
        assert_eq!(order(&env), ["A", "C"]);
    }

    #[test]
    fn locate_accepts_only_the_folder_with_the_same_id() {
        let env = env();
        let root = env.folder("Notes");
        let library = env.registry.resolve(&root).unwrap().library;
        let moved = env.notes.path().join("Elsewhere");
        std::fs::rename(&root, &moved).unwrap();
        let stranger = env.folder("Stranger");
        assert_eq!(
            env.registry.locate(&library.id, &stranger).unwrap(),
            LocateOutcome::NotThisLibrary {
                name: "Notes".into()
            }
        );
        assert_eq!(env.registry.get(&library.id).unwrap().unwrap().path, root);
        let LocateOutcome::Located(found) = env.registry.locate(&library.id, &moved).unwrap()
        else {
            panic!("not located");
        };
        assert_eq!(found.path, dunce::canonicalize(&moved).unwrap());
        assert_eq!(
            env.registry
                .locate("0".repeat(32).as_str(), &moved)
                .unwrap(),
            LocateOutcome::Unknown
        );
    }

    #[test]
    fn create_makes_a_default_library_and_reuses_or_refuses_existing_folders() {
        let env = env();
        let parent = env.folder("home");
        let library = env.registry.create(&parent, "Cerata").unwrap();
        assert!(library.is_default);
        assert_eq!(library.name, "Cerata");
        assert_eq!(marker_of(&library.path), format!("{}\n", library.id));
        // Same place again: the existing library, no second entry.
        let again = env.registry.create(&parent, "Cerata").unwrap();
        assert_eq!(again.id, library.id);
        assert_eq!(env.registry.list().unwrap().len(), 1);
        // A non-empty folder without a marker is refused.
        let busy = parent.join("Busy");
        std::fs::create_dir(&busy).unwrap();
        std::fs::write(busy.join("a.md"), "x").unwrap();
        assert!(matches!(
            env.registry.create(&parent, "Busy"),
            Err(CreateError::FolderExists(_))
        ));
        assert!(!paths::library_marker_path(&busy).exists());
        // An empty one is adopted.
        std::fs::create_dir(parent.join("Empty")).unwrap();
        assert!(env.registry.create(&parent, "Empty").is_ok());
        // Invalid names are refused before touching the disk.
        assert!(matches!(
            env.registry.create(&parent, "a/b"),
            Err(CreateError::InvalidName(_))
        ));
        assert!(matches!(
            env.registry.create(&parent, " "),
            Err(CreateError::InvalidName(_))
        ));
    }

    #[test]
    fn a_failed_create_records_nothing() {
        let env = env();
        let missing = env.notes.path().join("no-such-parent");
        assert!(matches!(
            env.registry.create(&missing, "Cerata"),
            Err(CreateError::Fs(_))
        ));
        assert!(env.registry.list().unwrap().is_empty());
    }
}
