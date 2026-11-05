use crate::json_store::{JsonStore, StoreError};
use crate::library::LibraryKey;
use crate::session::{legacy_key, to_absolute, to_relative};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Pins = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone)]
pub struct PinStore {
    store: JsonStore,
}

impl PinStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            store: JsonStore::new(data_dir.join("pins.json")),
        }
    }

    pub fn load(&self, library: &LibraryKey) -> Result<Vec<PathBuf>, StoreError> {
        let pins: Pins = self.store.load()?;
        if !pins.contains_key(&library.id) && pins.contains_key(&legacy_key(&library.root)) {
            return self.edit(library, |_| {});
        }
        Ok(absolute(
            &library.root,
            pins.get(&library.id).cloned().unwrap_or_default(),
        ))
    }

    pub fn toggle(&self, library: &LibraryKey, path: &Path) -> Result<Vec<PathBuf>, StoreError> {
        let root = &library.root;
        self.edit(library, |pins| {
            if let Some(ix) = pins.iter().position(|p| p == path) {
                pins.remove(ix);
            } else if path.starts_with(root) && path != root {
                pins.insert(0, path.to_path_buf());
            }
        })
    }

    pub fn rewrite_prefix(
        &self,
        library: &LibraryKey,
        old: &Path,
        new: &Path,
    ) -> Result<Vec<PathBuf>, StoreError> {
        self.edit(library, |pins| {
            for pin in pins.iter_mut() {
                if let Ok(rest) = pin.strip_prefix(old) {
                    *pin = if rest.as_os_str().is_empty() {
                        new.to_path_buf()
                    } else {
                        new.join(rest)
                    };
                }
            }
        })
    }

    pub fn remove_prefix(
        &self,
        library: &LibraryKey,
        prefix: &Path,
    ) -> Result<Vec<PathBuf>, StoreError> {
        self.edit(library, |pins| pins.retain(|pin| !pin.starts_with(prefix)))
    }

    fn edit(
        &self,
        library: &LibraryKey,
        f: impl FnOnce(&mut Vec<PathBuf>),
    ) -> Result<Vec<PathBuf>, StoreError> {
        let root = &library.root;
        let mut result = Vec::new();
        self.store.update(|pins: &mut Pins| {
            let legacy = pins.remove(&legacy_key(root));
            let stored = match (pins.remove(&library.id), legacy) {
                (Some(stored), _) => stored,
                (None, Some(legacy)) => {
                    log::info!("pins of {} moved to library {}", root.display(), library.id);
                    legacy
                        .iter()
                        .filter_map(|path| to_relative(root, Path::new(path)))
                        .collect()
                }
                (None, None) => Vec::new(),
            };
            let mut list = absolute(root, stored);
            f(&mut list);
            list = normalize(root, list);
            let relative: Vec<String> = list
                .iter()
                .filter_map(|path| to_relative(root, path))
                .collect();
            if !relative.is_empty() {
                pins.insert(library.id.clone(), relative);
            }
            result = list;
        })?;
        Ok(result)
    }
}

fn absolute(root: &Path, stored: Vec<String>) -> Vec<PathBuf> {
    let paths = stored
        .iter()
        .filter_map(|relative| {
            let path = to_absolute(root, relative);
            if path.is_none() {
                log::warn!("pins: {relative:?} is not a path inside the library, rejected");
            }
            path
        })
        .collect();
    normalize(root, paths)
}

fn normalize(root: &Path, pins: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::with_capacity(pins.len());
    for pin in pins {
        if pin.starts_with(root) && pin != root && !seen.contains(&pin) {
            seen.push(pin);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn key(root: &Path, id: char) -> LibraryKey {
        LibraryKey {
            id: id.to_string().repeat(32),
            root: dunce::canonicalize(root).unwrap(),
        }
    }

    #[test]
    fn pins_are_prepended_toggled_and_kept_per_library() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        let lib = key(root.path(), 'a');
        let a = lib.root.join("a.md");
        let b = lib.root.join("b.md");
        store.toggle(&lib, &a).unwrap();
        assert_eq!(store.toggle(&lib, &b).unwrap(), [b.clone(), a.clone()]);
        assert_eq!(store.toggle(&lib, &a).unwrap(), std::slice::from_ref(&b));
        assert!(store.load(&key(other.path(), 'b')).unwrap().is_empty());
        assert_eq!(store.load(&lib).unwrap(), [b]);
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(data.path().join("pins.json")).unwrap())
                .unwrap();
        assert_eq!(json[&lib.id], serde_json::json!(["b.md"]));
    }

    #[test]
    fn pins_outside_the_root_or_duplicated_are_dropped() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        let lib = key(root.path(), 'a');
        assert!(
            store
                .toggle(&lib, Path::new("/elsewhere/x.md"))
                .unwrap()
                .is_empty()
        );
        assert!(store.toggle(&lib, &lib.root).unwrap().is_empty());
    }

    #[test]
    fn pins_follow_renames_and_removals_by_whole_components() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        let lib = key(root.path(), 'a');
        let r = lib.root.clone();
        store.toggle(&lib, &r.join("dir/a.md")).unwrap();
        store.toggle(&lib, &r.join("dir-b/c.md")).unwrap();
        assert_eq!(
            store
                .rewrite_prefix(&lib, &r.join("dir"), &r.join("moved"))
                .unwrap(),
            [r.join("dir-b/c.md"), r.join("moved/a.md")]
        );
        assert_eq!(
            store.remove_prefix(&lib, &r.join("moved")).unwrap(),
            [r.join("dir-b/c.md")]
        );
    }

    #[test]
    fn a_moved_library_keeps_its_pins() {
        let data = TempDir::new().unwrap();
        let one = TempDir::new().unwrap();
        let two = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        let before = key(one.path(), 'a');
        store.toggle(&before, &before.root.join("n.md")).unwrap();
        let after = LibraryKey {
            id: before.id.clone(),
            root: dunce::canonicalize(two.path()).unwrap(),
        };
        assert_eq!(store.load(&after).unwrap(), [after.root.join("n.md")]);
    }

    #[test]
    fn absolute_pins_are_migrated_and_escaping_entries_rejected() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let lib = key(root.path(), 'a');
        let inside = lib.root.join("x.md").to_string_lossy().into_owned();
        let outside = elsewhere.path().join("y.md").to_string_lossy().into_owned();
        let json = serde_json::json!({
            legacy_key(&lib.root): [inside, outside],
            "/other/root": ["/other/root/z.md"],
            "b".repeat(32): ["../escape.md", "ok.md"]
        });
        std::fs::write(data.path().join("pins.json"), json.to_string()).unwrap();
        let store = PinStore::new(data.path());
        assert_eq!(store.load(&lib).unwrap(), [lib.root.join("x.md")]);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(data.path().join("pins.json")).unwrap())
                .unwrap();
        assert_eq!(saved[&lib.id], serde_json::json!(["x.md"]));
        assert!(saved.get(legacy_key(&lib.root)).is_none());
        assert!(saved.get("/other/root").is_some());
        let other = LibraryKey {
            id: "b".repeat(32),
            root: lib.root.clone(),
        };
        assert_eq!(store.load(&other).unwrap(), [lib.root.join("ok.md")]);
    }
}
