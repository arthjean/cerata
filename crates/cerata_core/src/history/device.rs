//! This device's identity in the history (ADR 0009 decision 6): 16 random
//! bytes drawn once and kept in `data_local_dir()/cerata/device-id`, never
//! in a roaming folder, and the OS host name as the device's name.

use crate::fs::atomic::{atomic_create, atomic_write};
use crate::library::{is_library_id, new_id};
use std::path::{Path, PathBuf};

/// File name of the device id, in `local_data_dir()`.
pub const DEVICE_ID_FILE: &str = "device-id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// 32 lowercase hexadecimal characters.
    pub id: String,
    pub name: String,
}

impl Device {
    /// Reads the device id in `local_data_dir`, drawing and writing it the
    /// first time. A malformed file is replaced. Without a writable folder
    /// the id lives for this run only.
    pub fn load_or_create(local_data_dir: &Path) -> Self {
        Self {
            id: load_or_create_id(&local_data_dir.join(DEVICE_ID_FILE)),
            name: host_name(),
        }
    }
}

fn load_or_create_id(path: &PathBuf) -> String {
    if let Ok(text) = std::fs::read_to_string(path) {
        let id = text.trim();
        if is_library_id(id) {
            return id.to_owned();
        }
        log::warn!(
            "{} is malformed: a new device id replaces it",
            path.display()
        );
        let id = new_id();
        if let Err(error) = atomic_write(path, format!("{id}\n").as_bytes()) {
            log::warn!("device id not written: {error}");
        }
        return id;
    }
    let id = new_id();
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!("device id not written: {error}");
        return id;
    }
    match atomic_create(path, format!("{id}\n").as_bytes()) {
        Ok(()) => id,
        // Another window wrote it first: its id wins.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read_to_string(path)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|id| is_library_id(id))
                .unwrap_or(id)
        }
        Err(error) => {
            log::warn!("device id not written: {error}");
            id
        }
    }
}

/// The OS host name, or "This device" when it is empty.
pub fn host_name() -> String {
    let name = gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_owned();
    if name.is_empty() {
        "This device".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_id_is_drawn_once_and_kept() {
        let dir = TempDir::new().unwrap();
        let first = Device::load_or_create(dir.path());
        assert!(is_library_id(&first.id));
        assert!(!first.name.is_empty());
        let again = Device::load_or_create(dir.path());
        assert_eq!(again.id, first.id);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(DEVICE_ID_FILE)).unwrap(),
            format!("{}\n", first.id)
        );
    }

    #[test]
    fn a_malformed_id_is_replaced() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(DEVICE_ID_FILE), "not an id").unwrap();
        let device = Device::load_or_create(dir.path());
        assert!(is_library_id(&device.id));
        assert_eq!(Device::load_or_create(dir.path()).id, device.id);
    }
}
