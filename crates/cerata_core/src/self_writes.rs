//! Remembers Cerata's own recent writes so the watcher can drop their
//! echoes without swallowing real external changes.
//!
//! Replaces Writer `watcher.rs:106-138` (`is_self_write`, `record_write`),
//! which ignored every event on a path for 2 s after a save, keyed by a
//! non-canonical path (defect 7). Here an event is an echo only if the file
//! on disk still holds exactly the bytes Cerata wrote: an external edit
//! made one millisecond after a save is still reported.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a recorded write can match an echo. Generous on purpose: the
/// content fingerprint, not the window, is what tells echoes apart.
pub const SELF_WRITE_TTL: Duration = Duration::from_secs(10);

/// Fingerprint of written bytes: length plus a 64-bit hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    len: u64,
    hash: u64,
}

impl Fingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self {
            len: bytes.len() as u64,
            hash: hasher.finish(),
        }
    }
}

/// Thread-safe log of recent writes, keyed by canonical path.
#[derive(Debug, Default)]
pub struct SelfWriteLog {
    entries: Mutex<HashMap<PathBuf, Vec<(Fingerprint, Instant)>>>,
}

impl SelfWriteLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that Cerata is about to write `bytes` to `path`.
    pub fn record(&self, path: &Path, bytes: &[u8]) {
        let now = Instant::now();
        let mut entries = self.entries.lock();
        entries.retain(|_, writes| {
            writes.retain(|(_, at)| now.duration_since(*at) < SELF_WRITE_TTL);
            !writes.is_empty()
        });
        entries
            .entry(canonical_key(path))
            .or_default()
            .push((Fingerprint::of(bytes), now));
    }

    /// `true` when the current content of `path` is one Cerata wrote within
    /// [`SELF_WRITE_TTL`]. A missing or unreadable file is never an echo.
    pub fn is_self_write(&self, path: &Path) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        self.matches(path, Fingerprint::of(&bytes))
    }

    /// [`Self::is_self_write`] with a fingerprint the caller already has.
    pub fn matches(&self, path: &Path, current: Fingerprint) -> bool {
        let now = Instant::now();
        self.entries
            .lock()
            .get(&canonical_key(path))
            .is_some_and(|writes| {
                writes.iter().any(|(print, at)| {
                    *print == current && now.duration_since(*at) < SELF_WRITE_TTL
                })
            })
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
    use tempfile::TempDir;

    #[test]
    fn own_write_is_an_echo_but_a_later_external_edit_is_not() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        let log = SelfWriteLog::new();
        log.record(&path, b"ours");
        std::fs::write(&path, "ours").unwrap();
        assert!(log.is_self_write(&path));
        // An external tool rewrites the file right after our save.
        std::fs::write(&path, "theirs").unwrap();
        assert!(!log.is_self_write(&path));
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
