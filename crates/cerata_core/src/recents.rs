//! Recent files (30) and recent workspaces (10), app-wide.
//!
//! Ported from Writer `commands/recents.rs` (recent files, atomic, with the
//! migration from a plain path list) and `commands/workspace.rs:699-744`
//! (recent workspaces, which Writer wrote without lock or atomicity, defect
//! 6). Both now go through [`JsonStore`].

use crate::json_store::{JsonStore, StoreError};
use crate::markdown::is_markdown_path;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_RECENT_FILES: usize = 30;
pub const MAX_RECENT_WORKSPACES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    pub path: String,
    /// Seconds since the Unix epoch; 0 for entries migrated from Writer's
    /// older path-only format.
    pub opened_at: u64,
}

#[derive(Debug, Clone)]
pub struct RecentsStore {
    files: JsonStore,
    workspaces: JsonStore,
}

fn migrate_paths(data: &str) -> Option<Vec<RecentEntry>> {
    serde_json::from_str::<Vec<String>>(data).ok().map(|paths| {
        paths
            .into_iter()
            .map(|path| RecentEntry { path, opened_at: 0 })
            .collect()
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn canonical_string(path: &Path) -> String {
    dunce::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

impl RecentsStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            files: JsonStore::new(data_dir.join("recent_files.json")),
            workspaces: JsonStore::new(data_dir.join("recent_workspaces.json")),
        }
    }

    /// Most recent first.
    pub fn files(&self) -> Result<Vec<RecentEntry>, StoreError> {
        self.files.load_with(migrate_paths)
    }

    /// Records an opened Markdown file; other paths are ignored.
    pub fn record_file(&self, path: &Path) -> Result<(), StoreError> {
        if !path.is_file() || !is_markdown_path(path) {
            return Ok(());
        }
        let entry = RecentEntry {
            path: canonical_string(path),
            opened_at: now_secs(),
        };
        self.files
            .update_with(migrate_paths, |recents: &mut Vec<RecentEntry>| {
                recents.retain(|existing| existing.path != entry.path);
                recents.insert(0, entry);
                recents.truncate(MAX_RECENT_FILES);
            })
    }

    pub fn remove_file(&self, path: &str) -> Result<(), StoreError> {
        self.files
            .update_with(migrate_paths, |recents: &mut Vec<RecentEntry>| {
                recents.retain(|entry| entry.path != path);
            })
    }

    /// Most recent first.
    pub fn workspaces(&self) -> Result<Vec<String>, StoreError> {
        self.workspaces.load()
    }

    pub fn record_workspace(&self, root: &Path) -> Result<(), StoreError> {
        let root = canonical_string(root);
        self.workspaces.update(|recents: &mut Vec<String>| {
            recents.retain(|existing| *existing != root);
            recents.insert(0, root);
            recents.truncate(MAX_RECENT_WORKSPACES);
        })
    }

    pub fn remove_workspace(&self, root: &str) -> Result<(), StoreError> {
        self.workspaces.update(|recents: &mut Vec<String>| {
            recents.retain(|existing| existing != root);
        })
    }

    /// Paths of the recent files that still exist.
    pub fn existing_files(&self) -> Result<Vec<PathBuf>, StoreError> {
        Ok(self
            .files()?
            .into_iter()
            .map(|entry| PathBuf::from(entry.path))
            .filter(|path| path.is_file())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Ported from Writer `commands/recents.rs` tests.

    #[test]
    fn push_recent_dedupes_and_moves_to_front() {
        let data = TempDir::new().unwrap();
        let notes = TempDir::new().unwrap();
        let store = RecentsStore::new(data.path());
        let a = notes.path().join("a.md");
        let b = notes.path().join("b.md");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        store.record_file(&a).unwrap();
        store.record_file(&b).unwrap();
        store.record_file(&a).unwrap();
        let paths: Vec<String> = store.files().unwrap().into_iter().map(|e| e.path).collect();
        assert_eq!(paths, vec![canonical_string(&a), canonical_string(&b)]);
    }

    #[test]
    fn push_recent_caps_at_thirty() {
        let data = TempDir::new().unwrap();
        let notes = TempDir::new().unwrap();
        let store = RecentsStore::new(data.path());
        for i in 0..35 {
            let path = notes.path().join(format!("{i}.md"));
            std::fs::write(&path, "").unwrap();
            store.record_file(&path).unwrap();
        }
        let files = store.files().unwrap();
        assert_eq!(files.len(), MAX_RECENT_FILES);
        assert!(files[0].path.ends_with("34.md"));
    }

    #[test]
    fn legacy_path_list_is_migrated() {
        let data = TempDir::new().unwrap();
        std::fs::write(
            data.path().join("recent_files.json"),
            r#"["/a.md","/b.md"]"#,
        )
        .unwrap();
        let files = RecentsStore::new(data.path()).files().unwrap();
        assert_eq!(
            files,
            vec![
                RecentEntry {
                    path: "/a.md".into(),
                    opened_at: 0
                },
                RecentEntry {
                    path: "/b.md".into(),
                    opened_at: 0
                },
            ]
        );
    }

    #[test]
    fn non_markdown_files_are_not_recorded() {
        let data = TempDir::new().unwrap();
        let notes = TempDir::new().unwrap();
        let png = notes.path().join("image.png");
        std::fs::write(&png, "").unwrap();
        let store = RecentsStore::new(data.path());
        store.record_file(&png).unwrap();
        store.record_file(&notes.path().join("missing.md")).unwrap();
        assert!(store.files().unwrap().is_empty());
    }

    #[test]
    fn remove_recent_file_drops_the_entry() {
        let data = TempDir::new().unwrap();
        let notes = TempDir::new().unwrap();
        let a = notes.path().join("a.MARKDOWN");
        std::fs::write(&a, "").unwrap();
        let store = RecentsStore::new(data.path());
        store.record_file(&a).unwrap();
        store.remove_file(&canonical_string(&a)).unwrap();
        assert!(store.files().unwrap().is_empty());
    }

    #[test]
    fn recent_workspaces_cap_at_ten_most_recent_first() {
        let data = TempDir::new().unwrap();
        let roots: Vec<TempDir> = (0..12).map(|_| TempDir::new().unwrap()).collect();
        let store = RecentsStore::new(data.path());
        for root in &roots {
            store.record_workspace(root.path()).unwrap();
        }
        store.record_workspace(roots[5].path()).unwrap();
        let recents = store.workspaces().unwrap();
        assert_eq!(recents.len(), MAX_RECENT_WORKSPACES);
        assert_eq!(recents[0], canonical_string(roots[5].path()));
        assert_eq!(recents[1], canonical_string(roots[11].path()));
    }
}
