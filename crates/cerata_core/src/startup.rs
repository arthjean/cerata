use crate::fs::{DirEntry, FileContent, FsError, WorkspaceFs};
use crate::ignore::WorkspaceIgnore;
use crate::library::{Library, LibraryNotice, LibraryRegistry, Marker, read_marker};
use crate::open_target::OpenTarget;
use crate::paths;
use crate::pins::PinStore;
use crate::recents::{RecentEntry, RecentsStore};
use crate::session::{SessionData, SessionStore};
use crate::settings::{ConfigValue, ConfigWarning, Settings, SettingsError};
use crate::watcher::WatchEnvelope;
use crate::workspace::{
    OpenOptions, WindowRegistry, WindowState, WorkspaceInfo, canonicalize_workspace_root,
    open_workspace_with,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

pub const RESTORE_WORKSPACE_KEY: &str = "window.restore-workspace";

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("the OS reports no data directory")]
    NoDataDir,
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

pub struct CoreApp {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub local_data_dir: PathBuf,
    pub windows: WindowRegistry,
    pub sessions: SessionStore,
    pub recents: RecentsStore,
    pub pins: PinStore,
    pub libraries: LibraryRegistry,
}

impl CoreApp {
    pub fn new(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            sessions: SessionStore::new(&data_dir),
            pins: PinStore::new(&data_dir),
            recents: RecentsStore::new(&data_dir),
            libraries: LibraryRegistry::new(&data_dir),
            windows: WindowRegistry::new(),
            config_dir,
            local_data_dir: data_dir.clone(),
            data_dir,
        }
    }

    pub fn with_local_data_dir(mut self, dir: PathBuf) -> Self {
        self.local_data_dir = dir;
        self
    }

    pub fn open_default() -> Result<Self, StartupError> {
        let config_dir = paths::global_config_dir().ok_or(SettingsError::NoConfigDir)?;
        let data_dir = paths::data_dir().ok_or(StartupError::NoDataDir)?;
        let local_data_dir = paths::local_data_dir().unwrap_or_else(|| data_dir.clone());
        Ok(Self::new(config_dir, data_dir).with_local_data_dir(local_data_dir))
    }

    pub fn create_window(&self) -> Result<(Arc<WindowState>, Vec<ConfigWarning>), StartupError> {
        let (settings, warnings) = Settings::open(&self.config_dir)?;
        Ok((self.windows.create(settings), warnings))
    }

    pub fn open_workspace(
        &self,
        window: &Arc<WindowState>,
        path: &Path,
        events: Sender<WatchEnvelope>,
    ) -> Result<(WorkspaceInfo, Option<LibraryNotice>), FsError> {
        let root = canonicalize_workspace_root(path)?;
        let (id, notice) = match self.libraries.resolve(&root) {
            Ok(opened) => (opened.library.id, opened.notice),
            Err(error) => match read_marker(&root) {
                Marker::Id(id) => {
                    log::warn!("library registry unavailable: {error}");
                    (id, None)
                }
                _ => {
                    return Err(FsError::Io {
                        path: root,
                        source: std::io::Error::other(format!(
                            "the library registry is unavailable: {error}"
                        )),
                    });
                }
            },
        };
        let options = OpenOptions {
            store_root: Some(self.local_data_dir.clone()),
            files: None,
        };
        let info = open_workspace_with(window, &root, &id, events, options)?;
        Ok((info, notice))
    }
}

#[derive(Debug)]
pub struct StartupBundle {
    pub settings: Vec<(&'static str, ConfigValue)>,
    pub settings_warnings: Vec<ConfigWarning>,
    pub libraries: Vec<Library>,
    pub recent_files: Vec<RecentEntry>,
    pub workspace: Option<WorkspaceInfo>,
    pub library_notice: Option<LibraryNotice>,
    pub entries: Vec<DirEntry>,
    pub session: Option<SessionData>,
    pub active_file: Option<FileContent>,
    pub standalone_file: Option<FileContent>,
}

pub fn startup(
    app: &CoreApp,
    target: Option<OpenTarget>,
    events: Sender<WatchEnvelope>,
) -> Result<(Arc<WindowState>, StartupBundle), StartupError> {
    let (window, settings_warnings) = app.create_window()?;
    let libraries = app.libraries.list().unwrap_or_else(|error| {
        log::warn!("libraries unavailable: {error}");
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
        None if restore => libraries
            .iter()
            .find(|library| library.exists())
            .map(|library| library.path.clone()),
        None => None,
    };

    let mut bundle = StartupBundle {
        settings: Vec::new(),
        settings_warnings,
        libraries,
        recent_files,
        workspace: None,
        library_notice: None,
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
            Ok((info, notice)) => {
                bundle.library_notice = notice;
                if let Some(runtime) = window.runtime() {
                    let ignore = WorkspaceIgnore::bootstrap(&runtime.root);
                    bundle.entries = runtime
                        .fs
                        .read_directory(&runtime.root, &ignore, None)
                        .unwrap_or_else(|error| {
                            log::warn!("cannot list {}: {error}", runtime.root.display());
                            Vec::new()
                        });
                    bundle.session = app.sessions.load(&info.library()).unwrap_or_else(|error| {
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
                if let Ok(libraries) = app.libraries.list() {
                    bundle.libraries = libraries;
                }
            }
            Err(error) => log::warn!("cannot open workspace {}: {error}", root.display()),
        }
    }

    bundle.settings = window.settings().merged();
    Ok((window, bundle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::is_library_id;
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
    fn one_call_returns_settings_libraries_listing_session_and_active_file() {
        let env = env();
        let root = TempDir::new().unwrap();
        let canonical = dunce::canonicalize(root.path()).unwrap();
        std::fs::write(canonical.join("a.md"), "# A").unwrap();
        std::fs::write(canonical.join("b.md"), "# B body").unwrap();
        env.app
            .recents
            .record_file(&canonical.join("a.md"))
            .unwrap();
        let library = env.app.libraries.resolve(&canonical).unwrap().library;
        env.app
            .sessions
            .save(
                &library.key(),
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
        let info = bundle.workspace.as_ref().unwrap();
        assert_eq!(info.root, canonical);
        assert_eq!(info.library_id, library.id);
        assert_eq!(bundle.library_notice, None);
        let names: Vec<_> = bundle.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.md", "b.md"]);
        assert_eq!(bundle.session.as_ref().unwrap().tabs.len(), 1);
        assert_eq!(bundle.active_file.as_ref().unwrap().content, "# B body");
        assert_eq!(window.snapshot().unwrap().root, canonical);
        assert_eq!(window.runtime().unwrap().library_id, library.id);
        assert_eq!(bundle.libraries[0].path, canonical);
    }

    #[test]
    fn opening_a_folder_registers_it_as_a_library() {
        let env = env();
        let root = TempDir::new().unwrap();
        let (sender, _events) = mpsc::channel();
        let (_window, bundle) = startup(
            &env.app,
            Some(OpenTarget::Workspace(root.path().to_path_buf())),
            sender,
        )
        .unwrap();
        let info = bundle.workspace.unwrap();
        assert!(is_library_id(&info.library_id));
        assert_eq!(read_marker(&info.root), Marker::Id(info.library_id.clone()));
        assert_eq!(env.app.libraries.list().unwrap()[0].id, info.library_id);
    }

    #[test]
    fn without_target_the_most_recent_existing_library_is_restored() {
        let env = env();
        let older = TempDir::new().unwrap();
        let newer = TempDir::new().unwrap();
        let gone = TempDir::new().unwrap();
        for dir in [&older, &newer, &gone] {
            env.app
                .libraries
                .resolve(&dunce::canonicalize(dir.path()).unwrap())
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let gone_path = gone.path().to_path_buf();
        drop(gone);
        assert!(!gone_path.exists());
        let (sender, _events) = mpsc::channel();
        let (_window, bundle) = startup(&env.app, None, sender).unwrap();
        assert_eq!(
            bundle.workspace.unwrap().root,
            dunce::canonicalize(newer.path()).unwrap()
        );
    }

    #[test]
    fn the_recent_workspaces_of_an_older_version_are_restored_after_migration() {
        let env = env();
        let root = TempDir::new().unwrap();
        std::fs::write(
            env.app.data_dir.join("recent_workspaces.json"),
            serde_json::to_string(&[root.path().display().to_string()]).unwrap(),
        )
        .unwrap();
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
        env.app
            .libraries
            .resolve(&dunce::canonicalize(root.path()).unwrap())
            .unwrap();
        let (sender, _events) = mpsc::channel();
        let (_window, bundle) = startup(&env.app, None, sender).unwrap();
        assert!(bundle.workspace.is_none());
        assert_eq!(bundle.libraries.len(), 1);
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
        assert!(env.app.libraries.list().unwrap().is_empty());
        assert!(!dir.path().join(".cerata").exists());
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
        let aside = std::fs::read_dir(&env.app.data_dir)
            .unwrap()
            .flatten()
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("sessions.json.corrupt-")
            });
        assert!(aside);
    }

    #[test]
    fn unreadable_sessions_file_opens_the_library_without_tabs() {
        let env = env();
        let sessions = env.app.data_dir.join("sessions.json");
        std::fs::create_dir(&sessions).unwrap();
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
        assert!(sessions.is_dir());
    }
}
