//! Pinned files of each workspace, in `pins.json` of the app data folder.
//!
//! Writer kept them in a webview preference
//! (`workspace:<root>:sidebar-pinned-files`, `workspace-store.ts:343-421`);
//! Cerata writes nothing in a workspace besides `.cerata/config`, so they
//! live next to the sessions, keyed the same way. On load they are
//! normalized as in Writer: inside the root only, without duplicates.

use crate::json_store::{JsonStore, StoreError};
use crate::session::session_key;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Pins = BTreeMap<String, Vec<PathBuf>>;

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

    /// Pinned files of `root`, most recently pinned first.
    pub fn load(&self, root: &Path) -> Result<Vec<PathBuf>, StoreError> {
        let pins: Pins = self.store.load()?;
        Ok(normalize(
            root,
            pins.get(&session_key(root)).cloned().unwrap_or_default(),
        ))
    }

    /// Pins `path` (first) or unpins it. Returns the new list.
    pub fn toggle(&self, root: &Path, path: &Path) -> Result<Vec<PathBuf>, StoreError> {
        self.edit(root, |pins| {
            if let Some(ix) = pins.iter().position(|p| p == path) {
                pins.remove(ix);
            } else if path.starts_with(root) && path != root {
                pins.insert(0, path.to_path_buf());
            }
        })
    }

    /// A file or folder moved: pins at `old` or under it follow.
    pub fn rewrite_prefix(
        &self,
        root: &Path,
        old: &Path,
        new: &Path,
    ) -> Result<Vec<PathBuf>, StoreError> {
        self.edit(root, |pins| {
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

    /// A file or folder is gone: its pins and those under it are dropped.
    pub fn remove_prefix(&self, root: &Path, prefix: &Path) -> Result<Vec<PathBuf>, StoreError> {
        self.edit(root, |pins| pins.retain(|pin| !pin.starts_with(prefix)))
    }

    fn edit(
        &self,
        root: &Path,
        f: impl FnOnce(&mut Vec<PathBuf>),
    ) -> Result<Vec<PathBuf>, StoreError> {
        let key = session_key(root);
        let mut result = Vec::new();
        self.store.update(|pins: &mut Pins| {
            let mut list = normalize(root, pins.remove(&key).unwrap_or_default());
            f(&mut list);
            list = normalize(root, list);
            if !list.is_empty() {
                pins.insert(key.clone(), list.clone());
            }
            result = list;
        })?;
        Ok(result)
    }
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

    #[test]
    fn pins_are_prepended_toggled_and_kept_per_workspace() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        let a = root.path().join("a.md");
        let b = root.path().join("b.md");
        store.toggle(root.path(), &a).unwrap();
        assert_eq!(
            store.toggle(root.path(), &b).unwrap(),
            [b.clone(), a.clone()]
        );
        assert_eq!(
            store.toggle(root.path(), &a).unwrap(),
            std::slice::from_ref(&b)
        );
        assert!(store.load(other.path()).unwrap().is_empty());
        assert_eq!(store.load(root.path()).unwrap(), [b]);
    }

    #[test]
    fn pins_outside_the_root_or_duplicated_are_dropped() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        assert!(
            store
                .toggle(root.path(), Path::new("/elsewhere/x.md"))
                .unwrap()
                .is_empty()
        );
        assert!(store.toggle(root.path(), root.path()).unwrap().is_empty());
    }

    #[test]
    fn pins_follow_renames_and_removals_by_whole_components() {
        let data = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let store = PinStore::new(data.path());
        let r = root.path();
        store.toggle(r, &r.join("dir/a.md")).unwrap();
        store.toggle(r, &r.join("dir-b/c.md")).unwrap();
        assert_eq!(
            store
                .rewrite_prefix(r, &r.join("dir"), &r.join("moved"))
                .unwrap(),
            [r.join("dir-b/c.md"), r.join("moved/a.md")]
        );
        assert_eq!(
            store.remove_prefix(r, &r.join("moved")).unwrap(),
            [r.join("dir-b/c.md")]
        );
    }
}
