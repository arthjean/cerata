//! The open documents of a window, keyed by path: loading, autosave and
//! the disk state each one is compared with when the file changes outside
//! Cerata.
//!
//! Ported from Writer `editor-store.ts` (document half: `openFiles`,
//! `ensureFileLoaded` with merged reads, the 40 ms grace before a spinner,
//! pruning of unreferenced clean files) and `lib/save.ts`, whose throttle
//! lives in [`cerata_core::save::AutoSaver`]. Every tab showing a file
//! shares its editor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cerata_core::fs::FsError;
use cerata_core::fs::text::TextFormat;
use cerata_core::save::{SaveSnapshot, SaveTarget};
use gpui::Entity;
use parking_lot::Mutex;

use crate::editor::Editor;
use crate::inbox::{Inbox, Wakeups};

/// A read shorter than this shows no spinner (`OPEN_FILE_GRACE_MS`).
pub const LOAD_GRACE: Duration = Duration::from_millis(40);

/// Why a document waits for the user before any reload or save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The file changed on disk while the editor had unsaved changes.
    Modified { disk: String },
    /// The file was deleted while the editor had unsaved changes.
    Deleted,
}

pub struct Doc {
    pub editor: Entity<Editor>,
    /// The text last read from or written to disk.
    pub disk_text: String,
    pub format: TextFormat,
    pub modified_at: u64,
    /// Set while the first read is in flight.
    pub loading_since: Option<Instant>,
    pub save_error: Option<String>,
    pub conflict: Option<Conflict>,
    /// "Compare" is showing the disk and editor texts side by side.
    pub compare: bool,
    /// The editor revision last seen, to notice edits.
    pub seen_revision: u64,
    /// A disk change arrived while a save was in flight: decided once the
    /// save completes.
    pub change_pending: bool,
}

impl Doc {
    pub fn new(editor: Entity<Editor>, loading: bool) -> Self {
        Self {
            editor,
            disk_text: String::new(),
            format: TextFormat::default(),
            modified_at: 0,
            loading_since: loading.then(Instant::now),
            save_error: None,
            conflict: None,
            compare: false,
            seen_revision: 0,
            change_pending: false,
        }
    }

    /// The spinner shows only once a read outlasts [`LOAD_GRACE`].
    pub fn shows_spinner(&self, now: Instant) -> bool {
        self.loading_since
            .is_some_and(|since| now.duration_since(since) >= LOAD_GRACE)
    }
}

/// A save result, sent from the save threads to the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveResult {
    Saved {
        path: PathBuf,
        revision: u64,
        disk_text: String,
        modified_at: u64,
    },
    Failed {
        path: PathBuf,
        revision: u64,
        error: String,
    },
}

/// The [`SaveTarget`] of a window: the dirty texts, copied in on every
/// edit, and the results queued for the window.
pub struct SaveBridge {
    dirty: Mutex<HashMap<PathBuf, SaveSnapshot>>,
    results: Arc<Inbox<SaveResult>>,
    in_flight: Mutex<HashMap<PathBuf, usize>>,
}

impl SaveBridge {
    pub fn new() -> (Arc<Self>, Wakeups) {
        let (results, wakeups) = Inbox::new();
        let bridge = Arc::new(Self {
            dirty: Mutex::new(HashMap::new()),
            results,
            in_flight: Mutex::new(HashMap::new()),
        });
        (bridge, wakeups)
    }

    /// The results since the last call, oldest first.
    pub fn take_results(&self) -> Vec<SaveResult> {
        self.results.take()
    }

    fn push(&self, result: SaveResult) {
        self.results.push(result);
    }

    /// The text of `path` to write next.
    pub fn set_dirty(&self, path: &Path, snapshot: SaveSnapshot) {
        self.dirty.lock().insert(path.to_path_buf(), snapshot);
    }

    /// `path` has nothing to write any more (clean, conflicted, closed).
    pub fn clear(&self, path: &Path) {
        self.dirty.lock().remove(path);
    }

    pub fn rename(&self, old: &Path, new: &Path) {
        let mut dirty = self.dirty.lock();
        if let Some(snapshot) = dirty.remove(old) {
            dirty.insert(new.to_path_buf(), snapshot);
        }
    }

    /// `true` from the moment a write of `path` reads its text until its
    /// result is sent.
    pub fn is_writing(&self, path: &Path) -> bool {
        self.in_flight.lock().get(path).is_some_and(|n| *n > 0)
    }

    fn done(&self, path: &Path) {
        let mut in_flight = self.in_flight.lock();
        if let Some(count) = in_flight.get_mut(path) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.remove(path);
            }
        }
    }
}

impl SaveTarget for SaveBridge {
    fn dirty_snapshot(&self, path: &Path) -> Option<SaveSnapshot> {
        let snapshot = self.dirty.lock().get(path).cloned()?;
        *self.in_flight.lock().entry(path.to_path_buf()).or_default() += 1;
        Some(snapshot)
    }

    fn saved(&self, path: &Path, revision: u64, disk_text: &str, modified_at: u64) {
        {
            let mut dirty = self.dirty.lock();
            if dirty.get(path).is_some_and(|s| s.revision == revision) {
                dirty.remove(path);
            }
        }
        self.done(path);
        self.push(SaveResult::Saved {
            path: path.to_path_buf(),
            revision,
            disk_text: disk_text.to_owned(),
            modified_at,
        });
    }

    fn save_failed(&self, path: &Path, revision: u64, error: &FsError) {
        self.done(path);
        self.push(SaveResult::Failed {
            path: path.to_path_buf(),
            revision,
            error: error.to_string(),
        });
    }
}

/// The open documents.
#[derive(Default)]
pub struct Documents {
    docs: HashMap<PathBuf, Doc>,
}

impl Documents {
    pub fn get(&self, path: &Path) -> Option<&Doc> {
        self.docs.get(path)
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut Doc> {
        self.docs.get_mut(path)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.docs.contains_key(path)
    }

    pub fn insert(&mut self, path: PathBuf, doc: Doc) {
        self.docs.insert(path, doc);
    }

    pub fn remove(&mut self, path: &Path) -> Option<Doc> {
        self.docs.remove(path)
    }

    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.docs.keys()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &Doc)> {
        self.docs.iter()
    }

    /// Re-keys documents at `old` or under it.
    pub fn rewrite_prefix(&mut self, old: &Path, new: &Path) -> Vec<(PathBuf, PathBuf)> {
        let moved: Vec<PathBuf> = self
            .docs
            .keys()
            .filter(|path| path.starts_with(old))
            .cloned()
            .collect();
        let mut renames = Vec::new();
        for from in moved {
            let Ok(rest) = from.strip_prefix(old) else {
                continue;
            };
            let to = if rest.as_os_str().is_empty() {
                new.to_path_buf()
            } else {
                new.join(rest)
            };
            if let Some(doc) = self.docs.remove(&from) {
                self.docs.insert(to.clone(), doc);
                renames.push((from, to));
            }
        }
        renames
    }

    /// Paths of documents to drop: no tab references them and `keep`
    /// (dirty, conflicted) does not hold them (Writer's pruning).
    pub fn unreferenced(
        &self,
        referenced: impl Fn(&Path) -> bool,
        keep: impl Fn(&Doc) -> bool,
    ) -> Vec<PathBuf> {
        self.docs
            .iter()
            .filter(|(path, doc)| !referenced(path) && !keep(doc))
            .map(|(path, _)| path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(revision: u64, text: &str) -> SaveSnapshot {
        SaveSnapshot {
            revision,
            text: text.into(),
            format: TextFormat::default(),
        }
    }

    #[test]
    fn a_newer_edit_stays_dirty_after_an_older_write_completes() {
        let (bridge, _) = SaveBridge::new();
        let path = PathBuf::from("/v/a.md");
        bridge.set_dirty(&path, snapshot(1, "first"));
        let taken = bridge.dirty_snapshot(&path).unwrap();
        assert!(bridge.is_writing(&path));
        bridge.set_dirty(&path, snapshot(2, "second"));
        bridge.saved(&path, taken.revision, "first", 10);
        assert!(!bridge.is_writing(&path));
        assert_eq!(bridge.dirty_snapshot(&path).unwrap().revision, 2);
        assert_eq!(
            bridge.take_results(),
            [SaveResult::Saved {
                path: path.clone(),
                revision: 1,
                disk_text: "first".into(),
                modified_at: 10
            }]
        );
        bridge.saved(&path, 2, "second", 11);
        assert!(bridge.dirty_snapshot(&path).is_none());
    }

    #[test]
    fn a_cleared_document_is_never_written() {
        let (bridge, _) = SaveBridge::new();
        let path = PathBuf::from("/v/a.md");
        bridge.set_dirty(&path, snapshot(1, "x"));
        bridge.clear(&path);
        assert!(bridge.dirty_snapshot(&path).is_none());
        assert!(!bridge.is_writing(&path));
    }

    #[test]
    fn a_rename_moves_the_pending_text() {
        let (bridge, _) = SaveBridge::new();
        bridge.set_dirty(Path::new("/v/old.md"), snapshot(3, "x"));
        bridge.rename(Path::new("/v/old.md"), Path::new("/v/new.md"));
        assert!(bridge.dirty_snapshot(Path::new("/v/old.md")).is_none());
        assert_eq!(
            bridge
                .dirty_snapshot(Path::new("/v/new.md"))
                .unwrap()
                .revision,
            3
        );
    }
}
