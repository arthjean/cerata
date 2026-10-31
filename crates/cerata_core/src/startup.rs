//! Application services and the single startup call.
//!
//! Ported from Writer `commands/startup.rs` (`get_startup_state`) and
//! `commands/workspace.rs:603-684` (`build_restore_bundle`): one call returns
//! the settings, the recents, the root listing, the session and the active
//! file, so the first frame can render without further round trips.

use crate::fs::{DirEntry, FileContent, WorkspaceFs};
use crate::ignore::WorkspaceIgnore;
use crate::open_target::OpenTarget;
use crate::paths;
use crate::recents::{RecentEntry, RecentsStore};
use crate::session::{SessionData, SessionStore};
use crate::settings::{ConfigValue, ConfigWarning, Settings, SettingsError};
use crate::watcher::WatchEnvelope;
use crate::workspace::{WindowRegistry, WindowState, WorkspaceInfo, open_workspace};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

/// Setting that reopens the last workspace at launch (default on).
pub const RESTORE_WORKSPACE_KEY: &str = "window.restore-workspace";

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("the OS reports no data directory")]
    NoDataDir,
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

/// Process-wide services shared by every window.
pub struct CoreApp {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub windows: WindowRegistry,
    pub sessions: SessionStore,
    pub recents: RecentsStore,
}

impl CoreApp {
    pub fn new(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            sessions: SessionStore::new(&data_dir),
            recents: RecentsStore::new(&data_dir),
            windows: WindowRegistry::new(),
            config_dir,
            data_dir,
        }
    }

    /// Services on the OS config and data directories for Cerata.
    pub fn open_default() -> Result<Self, StartupError> {
        let config_dir = paths::global_config_dir().ok_or(SettingsError::NoConfigDir)?;
        let data_dir = paths::data_dir().ok_or(StartupError::NoDataDir)?;
        Ok(Self::new(config_dir, data_dir))
    }

    /// A new window with its own settings handle on the global config.
    pub fn create_window(&self) -> Result<(Arc<WindowState>, Vec<ConfigWarning>), StartupError> {
        let (settings, warnings) = Settings::open(&self.config_dir)?;
        Ok((self.windows.create(settings), warnings))
    }

    /// Opens a workspace in `window` and records it in the recents.
    pub fn open_workspace(
        &self,
        window: &Arc<WindowState>,
        path: &Path,
        events: Sender<WatchEnvelope>,
    ) -> Result<WorkspaceInfo, crate::fs::FsError> {
        let info = open_workspace(window, path, events)?;
        if let Err(error) = self.recents.record_workspace(&info.root) {
            log::warn!("recent workspaces not updated: {error}");
        }
        Ok(info)
    }
}

/// Everything the first frame of a window needs.
#[derive(Debug)]
pub struct StartupBundle {
    pub settings: Vec<(&'static str, ConfigValue)>,
    pub settings_warnings: Vec<ConfigWarning>,
    pub recent_workspaces: Vec<String>,
    pub recent_files: Vec<RecentEntry>,
    pub workspace: Option<WorkspaceInfo>,
    /// Root listing, before indexing completes (titles come later).
    pub entries: Vec<DirEntry>,
    pub session: Option<SessionData>,
    /// Content of the session's active file.
    pub active_file: Option<FileContent>,
    /// A Markdown file opened on its own, outside any workspace.
    pub standalone_file: Option<FileContent>,
}

/// The startup call. `target` comes from argv or the OS; without one, the
/// most recent workspace that still exists is restored when
/// `window.restore-workspace` is on. Failures of optional parts (recents,
/// session, active file) are logged and leave that part empty.
pub fn startup(
    app: &CoreApp,
    target: Option<OpenTarget>,
    events: Sender<WatchEnvelope>,
) -> Result<(Arc<WindowState>, StartupBundle), StartupError> {
    let (window, settings_warnings) = app.create_window()?;
    let recent_workspaces = app.recents.workspaces().unwrap_or_else(|error| {
        log::warn!("recent workspaces unavailable: {error}");
        Vec::new()
    });
    let recent_files = app.recents.files().unwrap_or_else(|error| {
        log::warn!("recent files unavailable: {error}");
        Vec::new()
    });

    let restore = window
        .settings()
        .get(RESTORE_WORKSPACE_KEY)
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let workspace_path = match &target {
        Some(OpenTarget::Workspace(root)) => Some(root.clone()),
        Some(OpenTarget::File(_)) => None,
        None if restore => recent_workspaces
            .iter()
            .map(PathBuf::from)
            .find(|root| root.is_dir()),
        None => None,
    };

    let mut bundle = StartupBundle {
        settings: Vec::new(),
        settings_warnings,
        recent_workspaces,
        recent_files,
        workspace: None,
        entries: Vec::new(),
        session: None,
        active_file: None,
        standalone_file: None,
    };

    if let Some(OpenTarget::File(file)) = &target {
        match WorkspaceFs::new(file.parent().unwrap_or(file)).and_then(|fs| fs.read_file(file)) {
            Ok(content) => {
                window.set_standalone_file(Some(content.path.clone()));
                if let Err(error) = app.recents.record_file(&content.path) {
                    log::warn!("recent files not updated: {error}");
                }
                bundle.standalone_file = Some(content);
            }
            Err(error) => log::warn!("cannot open {}: {error}", file.display()),
        }
    }

    if let Some(root) = workspace_path {
        match app.open_workspace(&window, &root, events) {
            Ok(info) => {
                if let Some(runtime) = window.runtime() {
                    let ignore = WorkspaceIgnore::bootstrap(&runtime.root);
                    bundle.entries = runtime
                        .fs
                        .read_directory(&runtime.root, &ignore, None)
                        .unwrap_or_else(|error| {
                            log::warn!("cannot list {}: {error}", runtime.root.display());
                            Vec::new()
                        });
                    bundle.session = app.sessions.load(&info.root).unwrap_or_else(|error| {
                        log::warn!("session unavailable: {error}");
                        None
                    });
                    bundle.active_file = bundle
                        .session
                        .as_ref()
                        .and_then(SessionData::active_file)
                        .and_then(|path| runtime.fs.read_file(&path).ok());
                }
                bundle.workspace = Some(info);
            }
            Err(error) => log::warn!("cannot open workspace {}: {error}", root.display()),
        }
    }

    // After the workspace layer, so the bundle shows the merged values.
    bundle.settings = window.settings().merged();
    Ok((window, bundle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SerializedLocation, SessionTab};
    use std::sync::mpsc;
    use tempfile::TempDir;

    struct Env {
        _config: TempDir,
        _data: TempDir,
        app: CoreApp,
    }

    fn env() -> Env {
        let config = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let app = CoreApp::new(config.path().to_path_buf(), data.path().to_path_buf());
        Env {
            _config: config,
            _data: data,
            app,
        }
    }

    fn file_tab(path: &Path) -> SessionTab {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "path".into(),
            serde_json::Value::String(path.display().to_string()),
        );
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
    fn one_call_returns_settings_recents_listing_session_and_active_file() {
        let env = env();
        let root = TempDir::new().unwrap();
        let canonical = dunce::canonicalize(root.path()).unwrap();
        std::fs::write(canonical.join("a.md"), "# A").unwrap();
        std::fs::write(canonical.join("b.md"), "# B body").unwrap();
        env.app
            .recents
            .record_file(&canonical.join("a.md"))
            .unwrap();
        env.app
            .sessions
            .save(
                &canonical,
                SessionData {
                    tabs: vec![file_tab(&canonical.join("b.md"))],
                    active_index: Some(0),
                },
            )
            .unwrap();
        let (sender, _events) = mpsc::channel();
        let (window, bundle) = startup(
            &env.app,
            Some(OpenTarget::Workspace(canonical.clone())),
            sender,
        )
        .unwrap();

        assert_eq!(bundle.settings.len(), 43);
        assert!(
            bundle
                .settings
                .iter()
                .any(|(key, _)| *key == "editor.font-size")
        );
        assert_eq!(bundle.recent_files.len(), 1);
        assert_eq!(bundle.workspace.as_ref().unwrap().root, canonical);
        let names: Vec<_> = bundle.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.md", "b.md"]);
        assert_eq!(bundle.session.as_ref().unwrap().tabs.len(), 1);
        assert_eq!(bundle.active_file.as_ref().unwrap().content, "# B body");
        assert_eq!(window.snapshot().unwrap().root, canonical);
        // The workspace is now the most recent one.
        assert_eq!(
            env.app.recents.workspaces().unwrap()[0],
            canonical.display().to_string()
        );
    }

    #[test]
    fn without_target_the_last_workspace_is_restored() {
        let env = env();
        let root = TempDir::new().unwrap();
        env.app.recents.record_workspace(root.path()).unwrap();
        let (sender, _events) = mpsc::channel();
        let (_window, bundle) = startup(&env.app, None, sender).unwrap();
        assert_eq!(
            bundle.workspace.unwrap().root,
            dunce::canonicalize(root.path()).unwrap()
        );
    }

    #[test]
    fn restore_can_be_disabled() {
        let env = env();
        std::fs::write(
            env.app.config_dir.join("config"),
            "window.restore-workspace = false\n",
        )
        .unwrap();
        let root = TempDir::new().unwrap();
        env.app.recents.record_workspace(root.path()).unwrap();
        let (sender, _events) = mpsc::channel();
        let (_window, bundle) = startup(&env.app, None, sender).unwrap();
        assert!(bundle.workspace.is_none());
    }

    #[test]
    fn standalone_file_opens_without_a_workspace() {
        let env = env();
        let dir = TempDir::new().unwrap();
        let file = dunce::canonicalize(dir.path())
            .unwrap()
            .join("solo.markdown");
        std::fs::write(&file, "solo").unwrap();
        let (sender, _events) = mpsc::channel();
        let (window, bundle) =
            startup(&env.app, Some(OpenTarget::File(file.clone())), sender).unwrap();
        assert!(bundle.workspace.is_none());
        assert_eq!(bundle.standalone_file.unwrap().content, "solo");
        assert_eq!(window.standalone_file(), Some(file));
    }

    #[test]
    fn corrupt_sessions_file_starts_with_an_empty_session() {
        let env = env();
        std::fs::write(env.app.data_dir.join("sessions.json"), "{oops").unwrap();
        let root = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        let (_window, bundle) = startup(
            &env.app,
            Some(OpenTarget::Workspace(root.path().to_path_buf())),
            sender,
        )
        .unwrap();
        assert!(bundle.workspace.is_some());
        assert!(bundle.session.is_none());
        assert!(env.app.data_dir.join("sessions.json.corrupt").exists());
    }
}
