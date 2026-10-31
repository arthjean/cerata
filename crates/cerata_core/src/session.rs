//! Per-workspace sessions (open tabs with their back and forward history).
//!
//! Ported from Writer `commands/workspace.rs:851-918` and `lib/session.ts`.
//! The file layout is Writer's: `{root: {tabs: [{location, back, forward}],
//! active_index}}`. Fixes: written atomically under a process lock, keyed
//! by the canonical root with trailing separators of either kind removed
//! (Writer trimmed only `/`, defect 5), and a corrupt file is set aside.

use crate::json_store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A tab location, kept opaque except for its kind (`file`, ...), as the
/// shell owns its other fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerializedLocation {
    pub kind: String,
    #[serde(flatten)]
    pub payload: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    pub location: SerializedLocation,
    #[serde(default)]
    pub back: Vec<SerializedLocation>,
    #[serde(default)]
    pub forward: Vec<SerializedLocation>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionData {
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    #[serde(default)]
    pub active_index: Option<usize>,
}

impl SessionData {
    /// The file of the active tab, if that tab shows a file.
    pub fn active_file(&self) -> Option<PathBuf> {
        let tab = self.tabs.get(self.active_index?)?;
        if tab.location.kind != "file" {
            return None;
        }
        tab.location
            .payload
            .get("path")
            .and_then(|path| path.as_str())
            .map(PathBuf::from)
    }
}

type Sessions = BTreeMap<String, SessionData>;

/// `sessions.json` in the app data directory.
#[derive(Debug, Clone)]
pub struct SessionStore {
    store: JsonStore,
}

impl SessionStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            store: JsonStore::new(data_dir.join("sessions.json")),
        }
    }

    pub fn load(&self, root: &Path) -> Result<Option<SessionData>, StoreError> {
        let sessions: Sessions = self.store.load()?;
        Ok(sessions.get(&session_key(root)).cloned())
    }

    /// Saves the session of `root`; an empty session removes the entry.
    pub fn save(&self, root: &Path, session: SessionData) -> Result<(), StoreError> {
        let key = session_key(root);
        self.store.update(|sessions: &mut Sessions| {
            if session.tabs.is_empty() && session.active_index.is_none() {
                sessions.remove(&key);
            } else {
                sessions.insert(key, session);
            }
        })
    }
}

/// Canonical root without trailing `/` or `\`.
pub fn session_key(root: &Path) -> String {
    let canonical = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let text = canonical.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        text.into_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn file_tab(path: &str) -> SessionTab {
        let mut payload = serde_json::Map::new();
        payload.insert("path".into(), serde_json::Value::String(path.into()));
        SessionTab {
            location: SerializedLocation {
                kind: "file".into(),
                payload,
            },
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    #[test]
    fn sessions_round_trip_per_workspace_and_empty_removes() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let store = SessionStore::new(data.path());
        let session = SessionData {
            tabs: vec![file_tab("a.md"), file_tab("b.md")],
            active_index: Some(1),
        };
        store.save(workspace.path(), session.clone()).unwrap();
        assert_eq!(store.load(workspace.path()).unwrap(), Some(session.clone()));
        assert_eq!(session.active_file(), Some(PathBuf::from("b.md")));
        store
            .save(workspace.path(), SessionData::default())
            .unwrap();
        assert_eq!(store.load(workspace.path()).unwrap(), None);
    }

    #[test]
    fn trailing_separators_of_both_kinds_share_one_key() {
        let workspace = TempDir::new().unwrap();
        let plain = session_key(workspace.path());
        let slash = format!("{}/", workspace.path().display());
        let backslash = format!("{}\\", workspace.path().display());
        assert_eq!(session_key(Path::new(&slash)), plain);
        if cfg!(windows) {
            assert_eq!(session_key(Path::new(&backslash)), plain);
        }
        assert!(!plain.ends_with(['/', '\\']));
    }

    #[test]
    fn corrupt_sessions_file_is_renamed_and_an_empty_session_is_used() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        std::fs::write(data.path().join("sessions.json"), "[[[").unwrap();
        let store = SessionStore::new(data.path());
        assert_eq!(store.load(workspace.path()).unwrap(), None);
        assert!(data.path().join("sessions.json.corrupt").exists());
    }

    #[test]
    fn writer_session_format_is_read_as_is() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let key = session_key(workspace.path());
        let json = serde_json::json!({
            key.clone(): {
                "tabs": [{"location": {"kind": "file", "path": "x.md", "scrollTop": 12}}],
                "active_index": 0
            }
        });
        std::fs::write(data.path().join("sessions.json"), json.to_string()).unwrap();
        let session = SessionStore::new(data.path())
            .load(workspace.path())
            .unwrap()
            .unwrap();
        assert_eq!(session.active_file(), Some(PathBuf::from("x.md")));
        assert_eq!(session.tabs[0].location.payload["scrollTop"], 12);
    }
}
