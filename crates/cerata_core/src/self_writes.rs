//! Remembers Cerata's own writes so the watcher can drop their echoes
//! without swallowing real external changes.
//!
//! Replaces Writer `watcher.rs:106-138` (`is_self_write`, `record_write`),
//! which ignored every event on a path for 2 s after a save, keyed by a
//! non-canonical path (defect 7). Here an event is an echo only if the file
//! on disk holds exactly the bytes Cerata wrote: their BLAKE3 hash is that
//! of a write in progress or of the last write of the path, with no time
//! limit (US-086). An external edit made one millisecond after a save is
//! still reported, and an echo delivered a minute late is still dropped.

use crate::index::modified_ns;
use crate::store::{ContentHash, content_hash};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// What Cerata last wrote to a path, and the file's size and modified time
/// read right after the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrittenFile {
    pub hash: ContentHash,
    pub size: u64,
    pub modified_ns: u64,
    pub written_at: SystemTime,
}

/// Moves Cerata made that are remembered, newest last.
const KEPT_MOVES: usize = 64;

/// Told of every completed write, with its bytes (index and store).
pub type WriteSink = Arc<dyn Fn(&Path, &WrittenFile, &[u8]) + Send + Sync>;

/// Thread-safe registry of writes, keyed by canonical path.
#[derive(Default)]
pub struct SelfWriteLog {
    in_flight: Mutex<HashMap<PathBuf, Vec<ContentHash>>>,
    written: Mutex<HashMap<PathBuf, WrittenFile>>,
    /// Renames and moves Cerata made, `(from, to)`, canonical.
    moves: Mutex<VecDeque<(PathBuf, PathBuf)>>,
    sink: RwLock<Option<WriteSink>>,
    /// Held by an autosave from its disk check to its result, and by a
    /// move of Cerata's, so a save never recreates the path a move just
    /// emptied.
    gate: Mutex<()>,
}

impl std::fmt::Debug for SelfWriteLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfWriteLog")
            .field("in_flight", &self.in_flight.lock().len())
            .field("written", &self.written.lock().len())
            .finish()
    }
}

impl SelfWriteLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every completed write is passed to `sink` from now on.
    pub fn set_sink(&self, sink: WriteSink) {
        *self.sink.write() = Some(sink);
    }

    /// Records that Cerata is about to write `bytes` to `path`.
    pub fn record(&self, path: &Path, bytes: &[u8]) -> ContentHash {
        let hash = content_hash(bytes);
        self.in_flight
            .lock()
            .entry(canonical_key(path))
            .or_default()
            .push(hash);
        hash
    }

    /// The write of `bytes` (recorded as `hash`) reached the disk: it
    /// becomes the path's last write, with the size and modified time read
    /// now, and the sink is told.
    pub fn written(&self, path: &Path, hash: ContentHash, bytes: &[u8]) {
        let key = canonical_key(path);
        self.forget_in_flight(&key, hash);
        let metadata = std::fs::metadata(&key).ok();
        let file = WrittenFile {
            hash,
            size: metadata
                .as_ref()
                .map_or(bytes.len() as u64, std::fs::Metadata::len),
            modified_ns: metadata.as_ref().map_or(0, modified_ns),
            written_at: SystemTime::now(),
        };
        self.written.lock().insert(key.clone(), file);
        let sink = self.sink.read().clone();
        if let Some(sink) = sink {
            sink(&key, &file, bytes);
        }
    }

    /// The write recorded as `hash` failed; nothing of it is on disk.
    pub fn abandoned(&self, path: &Path, hash: ContentHash) {
        self.forget_in_flight(&canonical_key(path), hash);
    }

    fn forget_in_flight(&self, key: &Path, hash: ContentHash) {
        let mut in_flight = self.in_flight.lock();
        if let Some(hashes) = in_flight.get_mut(key) {
            if let Some(index) = hashes.iter().position(|known| *known == hash) {
                hashes.remove(index);
            }
            if hashes.is_empty() {
                in_flight.remove(key);
            }
        }
    }

    /// `true` when the current content of `path` is one Cerata wrote. A
    /// missing or unreadable file is never an echo.
    pub fn is_self_write(&self, path: &Path) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        self.matches(path, &content_hash(&bytes))
    }

    /// [`Self::is_self_write`] with a hash the caller already has.
    pub fn matches(&self, path: &Path, hash: &ContentHash) -> bool {
        let key = canonical_key(path);
        if self
            .in_flight
            .lock()
            .get(&key)
            .is_some_and(|hashes| hashes.contains(hash))
        {
            return true;
        }
        self.written
            .lock()
            .get(&key)
            .is_some_and(|file| file.hash == *hash)
    }

    /// The file at `path` was seen holding content that is neither Cerata's
    /// last write nor one in progress: that write is no longer what the disk
    /// holds, so a later return to its bytes (an undo, a checkout) is an
    /// external change, not an echo. Writes in progress stay recorded.
    pub fn superseded(&self, path: &Path) {
        self.written.lock().remove(&canonical_key(path));
    }

    /// The last completed write of `path`.
    pub fn last_write(&self, path: &Path) -> Option<WrittenFile> {
        self.written.lock().get(&canonical_key(path)).copied()
    }

    /// Waits for the autosave being written, if any, and keeps the next
    /// one from starting until the guard drops.
    pub fn write_gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.gate.lock()
    }

    /// Records that Cerata is about to move `from` (a file or a folder) to
    /// `to`: the watcher and the reconciliation then know the rename is
    /// Cerata's own.
    pub fn record_move(&self, from: &Path, to: &Path) {
        let mut moves = self.moves.lock();
        moves.push_back((canonical_key(from), canonical_key(to)));
        while moves.len() > KEPT_MOVES {
            moves.pop_front();
        }
    }

    /// The move from `from` to `to` failed.
    pub fn forget_move(&self, from: &Path, to: &Path) {
        let key = (canonical_key(from), canonical_key(to));
        self.moves.lock().retain(|recorded| *recorded != key);
    }

    /// `true` when a note found at `to` after it left `from` was moved
    /// there by Cerata, itself or with its folder.
    pub fn is_own_move(&self, from: &Path, to: &Path) -> bool {
        let (from, to) = (canonical_key(from), canonical_key(to));
        self.moves.lock().iter().any(|(old, new)| {
            from.strip_prefix(old).is_ok_and(|rest| {
                let moved = if rest.as_os_str().is_empty() {
                    new.clone()
                } else {
                    new.join(rest)
                };
                moved == to
            })
        })
    }

    /// A file Cerata moved from `old` to `new` keeps its last write.
    pub fn renamed(&self, old: &Path, new: &Path) {
        let mut written = self.written.lock();
        let moved: Vec<PathBuf> = written
            .keys()
            .filter(|path| path.starts_with(old))
            .cloned()
            .collect();
        for from in moved {
            if let (Some(file), Ok(rest)) = (written.remove(&from), from.strip_prefix(old)) {
                let to = if rest.as_os_str().is_empty() {
                    new.to_path_buf()
                } else {
                    new.join(rest)
                };
                written.insert(canonical_key(&to), file);
            }
        }
    }
}

/// Canonical path even for a file that does not exist yet (canonical parent
/// plus file name), so a write through `a/../b.md` and an event on `b.md`
/// share a key.
pub fn canonical_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = dunce::canonicalize(path) {
        return canonical;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => dunce::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn own_write_is_an_echo_but_a_later_external_edit_is_not() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let log = SelfWriteLog::new();
        let hash = log.record(&path, b"ours");
        std::fs::write(&path, "ours").unwrap();
        assert!(log.is_self_write(&path));
        log.written(&path, hash, b"ours");
        assert!(log.is_self_write(&path));
        // An external tool rewrites the file right after our save.
        std::fs::write(&path, "theirs").unwrap();
        assert!(!log.is_self_write(&path));
    }

    // Writer `watcher.rs` test_self_write_detection and
    // self_write_entry_is_not_consumed_on_match: one save fans out into
    // several OS events, and each of them is an echo.
    #[test]
    fn a_recorded_write_matches_every_echo_of_it() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let log = SelfWriteLog::new();
        log.record(&path, b"ours");
        std::fs::write(&path, "ours").unwrap();
        for _ in 0..3 {
            assert!(log.is_self_write(&path));
        }
        assert!(!log.is_self_write(&dir.path().join("other.md")));
    }

    // Replaces Writer's self_write_expires_after_ttl (US-086): the last
    // write matches its echo however late it comes.
    #[test]
    fn a_late_echo_is_still_recognized() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "ours").unwrap();
        let log = SelfWriteLog::new();
        let hash = log.record(&path, b"ours");
        log.written(&path, hash, b"ours");
        // As if the write happened a minute ago, past Writer's window and
        // the former 10 s one.
        let key = canonical_key(&path);
        log.written.lock().get_mut(&key).unwrap().written_at =
            SystemTime::now() - Duration::from_secs(60);
        assert!(log.is_self_write(&path));
        let last = log.last_write(&path).unwrap();
        assert_eq!(last.size, 4);
        assert!(last.modified_ns > 0);
    }

    #[test]
    fn a_failed_write_is_forgotten_and_the_sink_sees_completed_ones() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let log = SelfWriteLog::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        log.set_sink(Arc::new(
            move |path: &Path, _: &WrittenFile, bytes: &[u8]| {
                sink_seen.lock().push((path.to_path_buf(), bytes.to_vec()));
            },
        ));
        let failed = log.record(&path, b"lost");
        log.abandoned(&path, failed);
        assert!(!log.matches(&path, &failed));
        std::fs::write(&path, "kept").unwrap();
        let hash = log.record(&path, b"kept");
        log.written(&path, hash, b"kept");
        assert_eq!(*seen.lock(), [(canonical_key(&path), b"kept".to_vec())]);
    }

    #[test]
    fn a_move_of_cerata_is_known_for_the_folder_and_its_notes() {
        let dir = TempDir::new().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("old")).unwrap();
        let log = SelfWriteLog::new();
        log.record_move(&root.join("old"), &root.join("new"));
        std::fs::rename(root.join("old"), root.join("new")).unwrap();
        assert!(log.is_own_move(
            &root.join("old").join("a.md"),
            &root.join("new").join("a.md")
        ));
        assert!(!log.is_own_move(&root.join("old").join("a.md"), &root.join("elsewhere.md")));
        log.forget_move(&root.join("old"), &root.join("new"));
        assert!(!log.is_own_move(&root.join("old"), &root.join("new")));
    }

    #[test]
    fn keys_are_canonical() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let path = dir.path().join("note.md");
        let roundabout = dir.path().join("sub").join("..").join("note.md");
        let log = SelfWriteLog::new();
        log.record(&roundabout, b"x");
        std::fs::write(&path, "x").unwrap();
        assert!(log.is_self_write(&path));
    }

    #[test]
    fn missing_file_is_never_an_echo() {
        let dir = TempDir::new().unwrap();
        let log = SelfWriteLog::new();
        log.record(&dir.path().join("gone.md"), b"");
        assert!(!log.is_self_write(&dir.path().join("gone.md")));
    }
}
