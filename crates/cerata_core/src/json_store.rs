//! Small JSON files in the app data directory (sessions, pins, the library
//! registry), read and written under an inter-process lock and replaced
//! atomically.
//!
//! Fixes Writer defect 6: sessions and recent workspaces were written with
//! a plain `fs::write`, without a lock, so two windows or two processes
//! could interleave and a crash could leave a truncated file. A corrupt
//! file is set aside as `{name}.corrupt-{YYYYMMDD-HHMMSS}` and replaced by
//! an empty value instead of being silently discarded. Any other read
//! error fails the call (US-074): an unreadable file is never replaced by
//! an empty value.

use crate::fs::atomic::atomic_write;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: {source}", path.display())]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// One JSON file holding a `T`.
#[derive(Debug, Clone)]
pub struct JsonStore {
    path: PathBuf,
}

impl JsonStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the value under the lock: a missing file is `T::default()`, a
    /// corrupt one is set aside and read as `T::default()`, and any other
    /// read error is returned.
    pub fn load<T: DeserializeOwned + Default>(&self) -> Result<T, StoreError> {
        let _lock = self.lock()?;
        self.read_unlocked(|_| None)
    }

    /// Reads with a fallback decoder for older formats, under the lock.
    pub fn load_with<T: DeserializeOwned + Default>(
        &self,
        migrate: impl Fn(&str) -> Option<T>,
    ) -> Result<T, StoreError> {
        let _lock = self.lock()?;
        self.read_unlocked(migrate)
    }

    /// Read-modify-write under the lock, then an atomic replace. A read
    /// error returns before `change` runs, and nothing is written.
    pub fn update<T, R>(&self, change: impl FnOnce(&mut T) -> R) -> Result<R, StoreError>
    where
        T: DeserializeOwned + Serialize + Default,
    {
        self.update_with(|_| None, change)
    }

    pub fn update_with<T, R>(
        &self,
        migrate: impl Fn(&str) -> Option<T>,
        change: impl FnOnce(&mut T) -> R,
    ) -> Result<R, StoreError>
    where
        T: DeserializeOwned + Serialize + Default,
    {
        let _lock = self.lock()?;
        let mut value = self.read_unlocked(migrate)?;
        let result = change(&mut value);
        let data = serde_json::to_vec_pretty(&value).map_err(|source| StoreError::Encode {
            path: self.path.clone(),
            source,
        })?;
        atomic_write(&self.path, &data).map_err(|source| self.io(source))?;
        Ok(result)
    }

    fn read_unlocked<T: DeserializeOwned + Default>(
        &self,
        migrate: impl Fn(&str) -> Option<T>,
    ) -> Result<T, StoreError> {
        let data = match std::fs::read_to_string(&self.path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                // Not UTF-8: as unusable as invalid JSON.
                self.quarantine(&error)?;
                return Ok(T::default());
            }
            Err(error) => return Err(self.io(error)),
        };
        Ok(match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(error) => match migrate(&data) {
                Some(value) => value,
                None => {
                    self.quarantine(&error)?;
                    T::default()
                }
            },
        })
    }

    /// Moves the file to `{name}.corrupt-{YYYYMMDD-HHMMSS}` (UTC), with a
    /// `-2`, `-3`, ... suffix when that name is taken, so an earlier
    /// quarantine is never replaced. A file that cannot be set aside is an
    /// error: reading it as empty would let `update` write over it.
    fn quarantine(&self, error: &dyn std::fmt::Display) -> Result<(), StoreError> {
        let stamp = utc_stamp(SystemTime::now());
        let corrupt = (1..10_000)
            .map(|n| {
                let mut name = self.path.clone().into_os_string();
                name.push(format!(".corrupt-{stamp}"));
                if n > 1 {
                    name.push(format!("-{n}"));
                }
                PathBuf::from(name)
            })
            .find(|candidate| std::fs::symlink_metadata(candidate).is_err());
        let Some(corrupt) = corrupt else {
            return Err(self.io(io::Error::other(format!(
                "corrupt ({error}) and no quarantine name is free"
            ))));
        };
        match std::fs::rename(&self.path, &corrupt) {
            Ok(()) => {
                log::warn!(
                    "{} is corrupt ({error}); moved it to {} and started empty",
                    self.path.display(),
                    corrupt.display()
                );
                Ok(())
            }
            Err(rename_error) => Err(self.io(io::Error::new(
                rename_error.kind(),
                format!("corrupt ({error}) and could not be set aside: {rename_error}"),
            ))),
        }
    }

    /// Exclusive lock on `{path}.lock`, held until the guard drops. Blocks
    /// other Cerata processes and other threads alike.
    fn lock(&self) -> Result<File, StoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| self.io(source))?;
        }
        let mut lock_path = self.path.clone().into_os_string();
        lock_path.push(".lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(PathBuf::from(lock_path))
            .map_err(|source| self.io(source))?;
        file.lock().map_err(|source| self.io(source))?;
        Ok(file)
    }

    fn io(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

/// `YYYYMMDD-HHMMSS` in UTC (days to civil date: Howard Hinnant's
/// `civil_from_days`).
pub fn utc_stamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn quarantined(dir: &Path, name: &str) -> Vec<PathBuf> {
        let prefix = format!("{name}.corrupt-");
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn concurrent_updates_never_lose_a_write() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(JsonStore::new(dir.path().join("counter.json")));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        store.update(|count: &mut u64| *count += 1).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(store.load::<u64>().unwrap(), 200);
    }

    #[test]
    fn missing_file_reads_as_default_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pins.json");
        let store = JsonStore::new(path.clone());
        let value: Vec<String> = store.load().unwrap();
        assert!(value.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_file_is_set_aside_and_replaced_by_an_empty_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = JsonStore::new(path.clone());
        let value: Vec<String> = store.load().unwrap();
        assert!(value.is_empty());
        let aside = quarantined(dir.path(), "sessions.json");
        assert_eq!(aside.len(), 1);
        let name = aside[0].file_name().unwrap().to_string_lossy().into_owned();
        let stamp = name.trim_start_matches("sessions.json.corrupt-");
        assert_eq!(stamp.len(), 15, "{name}");
        assert_eq!(&stamp[8..9], "-", "{name}");
        assert!(
            stamp
                .chars()
                .filter(|c| *c != '-')
                .all(|c| c.is_ascii_digit()),
            "{name}"
        );
        assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "{ not json");
        assert!(!path.exists());
    }

    #[test]
    fn two_quarantines_in_a_row_keep_both_files() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pins.json");
        let store = JsonStore::new(path.clone());
        std::fs::write(&path, "first").unwrap();
        let _: Vec<String> = store.load().unwrap();
        std::fs::write(&path, "second").unwrap();
        let _: Vec<String> = store.load().unwrap();
        let aside = quarantined(dir.path(), "pins.json");
        assert_eq!(aside.len(), 2, "{aside:?}");
        let mut contents: Vec<String> = aside
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        contents.sort();
        assert_eq!(contents, ["first", "second"]);
    }

    /// A folder in place of the file stands for an unreadable file
    /// (permission denied on Windows, `EISDIR` on Unix).
    #[test]
    fn unreadable_file_fails_load_and_update_without_writing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("inside"), "kept").unwrap();
        let store = JsonStore::new(path.clone());
        assert!(matches!(
            store.load::<Vec<String>>(),
            Err(StoreError::Io { .. })
        ));
        let mut ran = false;
        let result = store.update(|value: &mut Vec<String>| {
            ran = true;
            value.push("x".into());
        });
        assert!(matches!(result, Err(StoreError::Io { .. })));
        assert!(!ran, "the closure must not run after a read error");
        assert!(path.is_dir());
        assert_eq!(
            std::fs::read_to_string(path.join("inside")).unwrap(),
            "kept"
        );
        assert!(quarantined(dir.path(), "sessions.json").is_empty());
    }

    /// A corrupt file that cannot be moved aside (held open without delete
    /// sharing on Windows, in a read-only folder on Unix) fails the call:
    /// `update` must not write over it.
    #[test]
    fn a_corrupt_file_that_cannot_be_set_aside_is_never_overwritten() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pins.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = JsonStore::new(path.clone());
        // The lock file exists before the folder is locked.
        drop(store.lock().unwrap());
        #[cfg(windows)]
        let _held = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 1;
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
                .unwrap()
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let loaded = store.load::<Vec<String>>();
        let mut ran = false;
        let updated = store.update(|value: &mut Vec<String>| {
            ran = true;
            value.push("x".into());
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(matches!(loaded, Err(StoreError::Io { .. })), "{loaded:?}");
        assert!(matches!(updated, Err(StoreError::Io { .. })), "{updated:?}");
        assert!(!ran, "the closure must not run");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn utc_stamps_are_civil_dates() {
        let at = |secs| utc_stamp(UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "19700101-000000");
        // 2026-11-03T14:05:09Z
        assert_eq!(at(1_793_714_709), "20261103-140509");
        // 2024-02-29T23:59:59Z
        assert_eq!(at(1_709_251_199), "20240229-235959");
    }
}
