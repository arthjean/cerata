use crate::json_store::{JsonStore, StoreError};
use crate::library::LibraryKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

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

    fn map_paths(&self, map: impl Fn(&str) -> Option<String>) -> Self {
        let convert = |location: &SerializedLocation| -> Option<SerializedLocation> {
            if location.kind != "file" {
                return Some(location.clone());
            }
            let path = location.payload.get("path")?.as_str()?;
            let mut converted = location.clone();
            converted
                .payload
                .insert("path".into(), serde_json::Value::String(map(path)?));
            Some(converted)
        };
        let mut tabs = Vec::new();
        let mut active_index = None;
        for (ix, tab) in self.tabs.iter().enumerate() {
            let Some(location) = convert(&tab.location) else {
                continue;
            };
            if Some(ix) == self.active_index {
                active_index = Some(tabs.len());
            }
            tabs.push(SessionTab {
                location,
                back: tab.back.iter().filter_map(convert).collect(),
                forward: tab.forward.iter().filter_map(convert).collect(),
            });
        }
        Self { tabs, active_index }
    }
}

pub fn to_relative(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rest
        .components()
        .map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub fn to_absolute(root: &Path, relative: &str) -> Option<PathBuf> {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    if relative.is_empty() || relative.starts_with(separators) {
        return None;
    }
    let mut path = root.to_path_buf();
    for part in relative.split(separators) {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        let component = Path::new(part);
        if !matches!(
            component.components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        ) {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

type Sessions = BTreeMap<String, SessionData>;

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

    pub fn load(&self, library: &LibraryKey) -> Result<Option<SessionData>, StoreError> {
        let mut sessions: Sessions = self.store.load()?;
        if !sessions.contains_key(&library.id) && sessions.contains_key(&legacy_key(&library.root))
        {
            sessions = self.store.update(|sessions: &mut Sessions| {
                migrate(sessions, library);
                sessions.clone()
            })?;
        }
        Ok(sessions
            .get(&library.id)
            .map(|session| absolute(session, &library.root)))
    }

    pub fn save(&self, library: &LibraryKey, session: SessionData) -> Result<(), StoreError> {
        let session = relative(&session, &library.root);
        self.store.update(|sessions: &mut Sessions| {
            migrate(sessions, library);
            if session.tabs.is_empty() && session.active_index.is_none() {
                sessions.remove(&library.id);
            } else {
                sessions.insert(library.id.clone(), session);
            }
        })
    }
}

fn absolute(session: &SessionData, root: &Path) -> SessionData {
    session.map_paths(|stored| match to_absolute(root, stored) {
        Some(path) => Some(path.to_string_lossy().into_owned()),
        None => {
            log::warn!("session: {stored:?} is not a path inside the library, rejected");
            None
        }
    })
}

fn relative(session: &SessionData, root: &Path) -> SessionData {
    session.map_paths(|path| {
        let relative = to_relative(root, Path::new(path));
        if relative.is_none() {
            log::info!("session: {path} is outside {}, dropped", root.display());
        }
        relative
    })
}

fn migrate(sessions: &mut Sessions, library: &LibraryKey) {
    let Some(legacy) = sessions.remove(&legacy_key(&library.root)) else {
        return;
    };
    if sessions.contains_key(&library.id) {
        return;
    }
    let converted = relative(&legacy, &library.root);
    log::info!(
        "session of {} moved to library {}",
        library.root.display(),
        library.id
    );
    sessions.insert(library.id.clone(), converted);
}

pub fn legacy_key(root: &Path) -> String {
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

    fn file_location(path: &str) -> SerializedLocation {
        let mut payload = serde_json::Map::new();
        payload.insert("path".into(), serde_json::Value::String(path.into()));
        SerializedLocation {
            kind: "file".into(),
            payload,
        }
    }

    fn file_tab(path: &str) -> SessionTab {
        SessionTab {
            location: file_location(path),
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    fn library(root: &Path) -> LibraryKey {
        LibraryKey {
            id: "0123456789abcdef0123456789abcdef".into(),
            root: dunce::canonicalize(root).unwrap(),
        }
    }

    fn abs(key: &LibraryKey, rel: &str) -> String {
        rel.split('/')
            .fold(key.root.clone(), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned()
    }

    fn stored(data: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(data.join("sessions.json")).unwrap()).unwrap()
    }

    #[test]
    fn sessions_round_trip_with_relative_paths_and_empty_removes() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let key = library(workspace.path());
        let store = SessionStore::new(data.path());
        let mut tab = file_tab(&abs(&key, "b.md"));
        tab.back.push(file_location(&abs(&key, "dir/a.md")));
        let session = SessionData {
            tabs: vec![file_tab(&abs(&key, "a.md")), tab],
            active_index: Some(1),
        };
        store.save(&key, session.clone()).unwrap();
        let json = stored(data.path());
        assert_eq!(json[&key.id]["tabs"][1]["location"]["path"], "b.md");
        assert_eq!(json[&key.id]["tabs"][1]["location"]["kind"], "file");
        assert_eq!(json[&key.id]["tabs"][1]["back"][0]["path"], "dir/a.md");
        assert_eq!(store.load(&key).unwrap(), Some(session.clone()));
        assert_eq!(
            session.active_file(),
            Some(PathBuf::from(abs(&key, "b.md")))
        );
        store.save(&key, SessionData::default()).unwrap();
        assert_eq!(store.load(&key).unwrap(), None);
    }

    #[test]
    fn a_moved_library_finds_its_session_at_the_new_root() {
        let data = TempDir::new().unwrap();
        let one = TempDir::new().unwrap();
        let two = TempDir::new().unwrap();
        let store = SessionStore::new(data.path());
        let before = library(one.path());
        store
            .save(
                &before,
                SessionData {
                    tabs: vec![file_tab(&abs(&before, "n.md"))],
                    active_index: Some(0),
                },
            )
            .unwrap();
        let after = LibraryKey {
            id: before.id.clone(),
            root: dunce::canonicalize(two.path()).unwrap(),
        };
        assert_eq!(
            store.load(&after).unwrap().unwrap().active_file(),
            Some(after.root.join("n.md"))
        );
    }

    #[test]
    fn other_kinds_are_kept_and_paths_outside_the_root_dropped_on_save() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let key = library(workspace.path());
        let store = SessionStore::new(data.path());
        let mut graph = file_tab("");
        graph.location = SerializedLocation {
            kind: "graph".into(),
            payload: serde_json::Map::new(),
        };
        let outside = other.path().join("x.md").to_string_lossy().into_owned();
        store
            .save(
                &key,
                SessionData {
                    tabs: vec![graph.clone(), file_tab(&outside)],
                    active_index: Some(0),
                },
            )
            .unwrap();
        let loaded = store.load(&key).unwrap().unwrap();
        assert_eq!(loaded.tabs, [graph]);
        assert_eq!(loaded.active_index, Some(0));
    }

    #[test]
    fn an_absolute_key_is_migrated_on_first_read() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let key = library(workspace.path());
        let outside = elsewhere.path().join("x.md").to_string_lossy().into_owned();
        let json = serde_json::json!({
            legacy_key(&key.root): {
                "tabs": [
                    {"location": {"kind": "file", "path": outside}},
                    {"location": {"kind": "file", "path": abs(&key, "x.md"), "scrollTop": 12},
                     "back": [{"kind": "file", "path": outside}]}
                ],
                "active_index": 1
            },
            "/unrelated/root": {"tabs": [], "active_index": null}
        });
        std::fs::write(data.path().join("sessions.json"), json.to_string()).unwrap();
        let session = SessionStore::new(data.path()).load(&key).unwrap().unwrap();
        assert_eq!(session.tabs.len(), 1);
        assert!(session.tabs[0].back.is_empty());
        assert_eq!(session.active_index, Some(0));
        assert_eq!(session.active_file(), Some(key.root.join("x.md")));
        assert_eq!(session.tabs[0].location.payload["scrollTop"], 12);
        let json = stored(data.path());
        assert!(json.get(legacy_key(&key.root)).is_none());
        assert_eq!(json[&key.id]["tabs"][0]["location"]["path"], "x.md");
        assert!(json.get("/unrelated/root").is_some(), "{json}");
    }

    #[test]
    fn hand_edited_paths_that_leave_the_root_are_rejected() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let key = library(workspace.path());
        let json = serde_json::json!({
            &key.id: {
                "tabs": [
                    {"location": {"kind": "file", "path": "../x.md"}},
                    {"location": {"kind": "file", "path": "a/../../y.md"}},
                    {"location": {"kind": "file", "path": "/etc/z.md"}},
                    {"location": {"kind": "file", "path": "ok/n.md"}}
                ],
                "active_index": 3
            }
        });
        std::fs::write(data.path().join("sessions.json"), json.to_string()).unwrap();
        let session = SessionStore::new(data.path()).load(&key).unwrap().unwrap();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(
            session.active_file(),
            Some(key.root.join("ok").join("n.md"))
        );
    }

    #[test]
    fn relative_round_trip_and_rejections() {
        let root = Path::new(if cfg!(windows) { r"C:\notes" } else { "/notes" });
        let inside = root.join("a").join("b.md");
        assert_eq!(to_relative(root, &inside).as_deref(), Some("a/b.md"));
        assert_eq!(to_absolute(root, "a/b.md"), Some(inside));
        assert_eq!(to_relative(root, root), None);
        assert_eq!(to_relative(root, Path::new("/elsewhere/x.md")), None);
        for bad in ["", "../x.md", "a/../x.md", "./x.md", "/x.md", "a//b.md"] {
            assert_eq!(to_absolute(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn corrupt_sessions_file_is_set_aside_and_an_empty_session_is_used() {
        let data = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        std::fs::write(data.path().join("sessions.json"), "[[[").unwrap();
        let store = SessionStore::new(data.path());
        assert_eq!(store.load(&library(workspace.path())).unwrap(), None);
        let aside = std::fs::read_dir(data.path()).unwrap().flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("sessions.json.corrupt-")
        });
        assert!(aside);
    }

    #[test]
    fn legacy_keys_trim_separators_of_both_kinds() {
        let workspace = TempDir::new().unwrap();
        let plain = legacy_key(workspace.path());
        let slash = format!("{}/", workspace.path().display());
        let backslash = format!("{}\\", workspace.path().display());
        assert_eq!(legacy_key(Path::new(&slash)), plain);
        if cfg!(windows) {
            assert_eq!(legacy_key(Path::new(&backslash)), plain);
        }
        assert!(!plain.ends_with(['/', '\\']));
    }
}
