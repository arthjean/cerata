//! Small JSON files in the app data directory (sessions, recents), read and
//! written under an inter-process lock and replaced atomically.
//!
//! Fixes Writer defect 6: sessions and recent workspaces were written with
//! a plain `fs::write`, without a lock, so two windows or two processes
//! could interleave and a crash could leave a truncated file. A corrupt
//! file is renamed to `*.corrupt` and replaced by an empty value instead of
//! being silently discarded.

use crate::fs::atomic::atomic_write;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

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

    /// Reads the value under the lock; missing means `T::default()`.
    pub fn load<T: DeserializeOwned + Default>(&self) -> Result<T, StoreError> {
        let _lock = self.lock()?;
        Ok(self.read_unlocked(|_| None))
    }

    /// Reads with a fallback decoder for older formats, under the lock.
    pub fn load_with<T: DeserializeOwned + Default>(
        &self,
        migrate: impl Fn(&str) -> Option<T>,
    ) -> Result<T, StoreError> {
        let _lock = self.lock()?;
        Ok(self.read_unlocked(migrate))
    }

    /// Read-modify-write under the lock, then an atomic replace.
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
        let mut value = self.read_unlocked(migrate);
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
    ) -> T {
        let data = match std::fs::read_to_string(&self.path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return T::default(),
            Err(error) => {
                log::warn!(
                    "cannot read {}: {error}; using an empty value",
                    self.path.display()
                );
                return T::default();
            }
        };
        match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(error) => match migrate(&data) {
                Some(value) => value,
                None => {
                    self.quarantine(&error);
                    T::default()
                }
            },
        }
    }

    fn quarantine(&self, error: &serde_json::Error) {
        let mut corrupt = self.path.clone().into_os_string();
        corrupt.push(".corrupt");
        let corrupt = PathBuf::from(corrupt);
        match std::fs::rename(&self.path, &corrupt) {
            Ok(()) => log::warn!(
                "{} is corrupt ({error}); moved it to {} and started empty",
                self.path.display(),
                corrupt.display()
            ),
            Err(rename_error) => log::warn!(
                "{} is corrupt ({error}) and could not be set aside: {rename_error}",
                self.path.display()
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

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
    fn corrupt_file_is_set_aside_and_replaced_by_an_empty_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = JsonStore::new(path.clone());
        let value: Vec<String> = store.load().unwrap();
        assert!(value.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sessions.json.corrupt")).unwrap(),
            "{ not json"
        );
        assert!(!path.exists());
    }
}
