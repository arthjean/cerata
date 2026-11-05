use crate::index::modified_ns;
use crate::store::{ContentHash, content_hash};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrittenFile {
    pub hash: ContentHash,
    pub size: u64,
    pub modified_ns: u64,
    pub written_at: SystemTime,
}

const KEPT_MOVES: usize = 64;

pub type WriteSink = Arc<dyn Fn(&Path, &WrittenFile, &[u8]) + Send + Sync>;

#[derive(Default)]
pub struct SelfWriteLog {
    in_flight: Mutex<HashMap<PathBuf, Vec<ContentHash>>>,
    written: Mutex<HashMap<PathBuf, WrittenFile>>,
    moves: Mutex<VecDeque<(PathBuf, PathBuf)>>,
    sink: RwLock<Option<WriteSink>>,
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

    pub fn set_sink(&self, sink: WriteSink) {
        *self.sink.write() = Some(sink);
    }

    pub fn record(&self, path: &Path, bytes: &[u8]) -> ContentHash {
        let hash = content_hash(bytes);
        self.in_flight
            .lock()
            .entry(canonical_key(path))
            .or_default()
            .push(hash);
        hash
    }

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

    pub fn is_self_write(&self, path: &Path) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        self.matches(path, &content_hash(&bytes))
    }

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

    pub fn superseded(&self, path: &Path) {
        self.written.lock().remove(&canonical_key(path));
    }

    pub fn last_write(&self, path: &Path) -> Option<WrittenFile> {
        self.written.lock().get(&canonical_key(path)).copied()
    }

    pub fn write_gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.gate.lock()
    }

    pub fn record_move(&self, from: &Path, to: &Path) {
        let mut moves = self.moves.lock();
        moves.push_back((canonical_key(from), canonical_key(to)));
        while moves.len() > KEPT_MOVES {
            moves.pop_front();
        }
    }

    pub fn forget_move(&self, from: &Path, to: &Path) {
        let key = (canonical_key(from), canonical_key(to));
        self.moves.lock().retain(|recorded| *recorded != key);
    }

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
        std::fs::write(&path, "theirs").unwrap();
        assert!(!log.is_self_write(&path));
    }

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

    #[test]
    fn a_late_echo_is_still_recognized() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "ours").unwrap();
        let log = SelfWriteLog::new();
        let hash = log.record(&path, b"ours");
        log.written(&path, hash, b"ours");
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
