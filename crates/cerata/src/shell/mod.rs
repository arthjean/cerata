//! A window: its chrome (workspace or compact), tabs, open documents,
//! autosave, session and the reaction to changes on disk.

mod banner;
mod commands;
mod compact;
mod pages;
mod sidebar_footer;
mod status_bar;
mod tab_bar;
pub mod title_bar;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cerata_core::fs::text::SaveOptions;
use cerata_core::fs::{FileContent, FsError, WorkspaceFs};
use cerata_core::save::{AutoSaver, SaveSnapshot};
use cerata_core::session::SessionStore;
use cerata_core::startup::StartupBundle;
use cerata_core::watcher::{FileChange, WatchEnvelope, WatchEvent};
use cerata_core::workspace::{WindowState, WorkspaceInfo};
use cerata_markdown::EditorState;
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Size, Styled,
    Subscription, Task, Window, WindowBounds, WindowHandle, WindowOptions, actions, div, px, size,
};

use crate::documents::{Conflict, Doc, Documents, LOAD_GRACE, SaveBridge, SaveResult};
use crate::editor::{Editor, EditorConfig, theme};
use crate::routing::{Chrome, OpenRequest};
use crate::sidebar::{self, Sidebar, SidebarEvent};
use crate::tabs::{Location, Tabs, Undo};

/// Initial content size of a new window (`tauri.conf.json:14-31`).
pub const DEFAULT_SIZE: Size<f32> = Size {
    width: 1200.0,
    height: 800.0,
};

/// Smallest size the user can shrink a window to.
pub const MIN_SIZE: Size<f32> = Size {
    width: 800.0,
    height: 500.0,
};

/// The session is saved this long after the last change of the tabs
/// (`workspace-store.ts:439-458`).
pub const SESSION_DELAY: Duration = Duration::from_millis(500);

/// How long closing a window waits for its last writes.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

const RESTORE_OPEN_FILES_KEY: &str = "workspace.restore-open-files";

actions!(
    shell,
    [
        GoBack,
        GoForward,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9,
        ToggleSidebar,
        OpenPalette,
        OpenPaletteAnywhere,
        GoToFile,
        NewNote,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        OpenPreferences,
    ]
);

pub fn bindings() -> Vec<gpui::KeyBinding> {
    use gpui::KeyBinding;
    let c = Some("Shell");
    vec![
        KeyBinding::new("alt-left", GoBack, c),
        KeyBinding::new("alt-right", GoForward, c),
        KeyBinding::new(secondary!("t"), NewTab, c),
        KeyBinding::new(secondary!("w"), CloseTab, c),
        KeyBinding::new("ctrl-tab", NextTab, c),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, c),
        KeyBinding::new(secondary!("1"), ActivateTab1, c),
        KeyBinding::new(secondary!("2"), ActivateTab2, c),
        KeyBinding::new(secondary!("3"), ActivateTab3, c),
        KeyBinding::new(secondary!("4"), ActivateTab4, c),
        KeyBinding::new(secondary!("5"), ActivateTab5, c),
        KeyBinding::new(secondary!("6"), ActivateTab6, c),
        KeyBinding::new(secondary!("7"), ActivateTab7, c),
        KeyBinding::new(secondary!("8"), ActivateTab8, c),
        KeyBinding::new(secondary!("9"), ActivateTab9, c),
        KeyBinding::new(secondary!("\\"), ToggleSidebar, c),
        KeyBinding::new(secondary!("p"), OpenPalette, c),
        KeyBinding::new(secondary!("shift-p"), OpenPaletteAnywhere, c),
        KeyBinding::new(secondary!("o"), GoToFile, c),
        KeyBinding::new(secondary!("n"), NewNote, c),
        KeyBinding::new(secondary!("="), ZoomIn, c),
        KeyBinding::new(secondary!("+"), ZoomIn, c),
        KeyBinding::new(secondary!("shift-="), ZoomIn, c),
        KeyBinding::new(secondary!("-"), ZoomOut, c),
        KeyBinding::new(secondary!("shift--"), ZoomOut, c),
        KeyBinding::new(secondary!("0"), ResetZoom, c),
        KeyBinding::new(secondary!(","), OpenPreferences, c),
    ]
}

/// The shortcuts on digit and punctuation keys again, by position: Windows
/// names a key by its unshifted character, so on AZERTY `ctrl-0` arrives
/// as `ctrl-à` and the bindings above never fire. GPUI's key equivalents
/// map each one to the key at its US position (letters are left alone).
/// `-` is left out: it is an unshifted character on US, AZERTY and QWERTZ
/// alike, and AZERTY has no key at its US position, which GPUI logs.
pub fn positional_bindings(cx: &App) -> Vec<gpui::KeyBinding> {
    let predicate = gpui::KeyBindingContextPredicate::parse("Shell")
        .ok()
        .map(std::rc::Rc::new);
    let bind = |keys: &str, action: Box<dyn gpui::Action>| {
        gpui::KeyBinding::load(keys, action, predicate.clone(), true, None, cx).ok()
    };
    let digits: [Box<dyn gpui::Action>; 9] = [
        Box::new(ActivateTab1),
        Box::new(ActivateTab2),
        Box::new(ActivateTab3),
        Box::new(ActivateTab4),
        Box::new(ActivateTab5),
        Box::new(ActivateTab6),
        Box::new(ActivateTab7),
        Box::new(ActivateTab8),
        Box::new(ActivateTab9),
    ];
    let mut bindings: Vec<gpui::KeyBinding> = digits
        .into_iter()
        .zip(1..)
        .filter_map(|(action, n)| bind(&format!("{}{n}", secondary!("")), action))
        .collect();
    bindings.extend(
        [
            (
                secondary!("0"),
                Box::new(ResetZoom) as Box<dyn gpui::Action>,
            ),
            (secondary!("="), Box::new(ZoomIn)),
            (secondary!("\\"), Box::new(ToggleSidebar)),
            (secondary!(","), Box::new(OpenPreferences)),
        ]
        .into_iter()
        .filter_map(|(keys, action)| bind(keys, action)),
    );
    bindings
}

pub fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(
        None,
        size(px(DEFAULT_SIZE.width), px(DEFAULT_SIZE.height)),
        cx,
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(MIN_SIZE.width), px(MIN_SIZE.height))),
        app_id: Some("cerata".into()),
        titlebar: title_bar::options(),
        ..Default::default()
    }
}

/// Opens the window of a startup bundle. Everything the first frame shows
/// (settings, session, active file) is already in `bundle`.
pub fn open(
    cx: &mut App,
    state: Arc<WindowState>,
    bundle: StartupBundle,
    restore_session: bool,
) -> anyhow::Result<WindowHandle<WindowView>> {
    let config = EditorConfig::from_settings(&bundle.settings);
    let (_, translucent) = crate::theme::apply(state.settings(), cx.window_appearance());
    let mut options = window_options(cx);
    options.window_background = background_appearance(translucent);
    cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let mut view = WindowView::new(state, config, window, cx);
            view.hydrate(bundle, restore_session, window, cx);
            view
        })
    })
}

/// What a tab's page shows while its file loads.
pub enum Page {
    Editor(Entity<Editor>),
    Loading { spinner: bool },
    Launcher,
    Settings(Entity<crate::settings_panel::SettingsPanel>),
    Welcome,
}

pub struct WindowView {
    core: Arc<WindowState>,
    pub chrome: Chrome,
    pub workspace: Option<WorkspaceInfo>,
    pub tabs: Tabs,
    pub docs: Documents,
    config: EditorConfig,
    bridge: Arc<SaveBridge>,
    saver: Option<AutoSaver>,
    /// Rollbacks of navigations whose file is still loading.
    pending_undos: HashMap<PathBuf, Vec<Undo>>,
    active_editor: Option<EntityId>,
    pub notice: Option<SharedString>,
    session_task: Option<Task<()>>,
    sessions: SessionStore,
    focus_handle: FocusHandle,
    handle: Option<WindowHandle<WindowView>>,
    editor_subscriptions: HashMap<EntityId, Subscription>,
    pub sidebar: Option<Entity<Sidebar>>,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    resizing: bool,
    _sidebar_events: Option<Subscription>,
    /// The sidebar footer's Search field, and whether the pointer is on it.
    pub sidebar_filter: Entity<crate::editor::text_field::TextField>,
    sidebar_filter_hovered: bool,
    _sidebar_filter_events: Subscription,
    pub palette: Option<Entity<crate::palette::Palette>>,
    pub settings_panel: Option<Entity<crate::settings_panel::SettingsPanel>>,
    /// The compact window's open recents picker and its entries.
    pub recents_picker: Option<Vec<crate::palette::RecentFile>>,
    picker_focus: FocusHandle,
    /// A file tab's open context menu.
    pub tab_menu: Option<crate::menu::Menu<WindowView>>,
    /// The tab strip's horizontal scroll, read by its edge fades.
    pub tab_scroll: gpui::ScrollHandle,
    /// The status bar's counts for an editor at a revision.
    stats_cache: Option<((EntityId, u64), status_bar::Stats)>,
    _settings_events: Option<Subscription>,
    _palette_events: Option<Subscription>,
    /// The window background to set on the next frame (translucency).
    background: Option<gpui::WindowBackgroundAppearance>,
    _appearance: Subscription,
    _tasks: Vec<Task<()>>,
}

/// A translucent theme lets the desktop show through, blurred.
pub fn background_appearance(translucent: bool) -> gpui::WindowBackgroundAppearance {
    if translucent {
        gpui::WindowBackgroundAppearance::Blurred
    } else {
        gpui::WindowBackgroundAppearance::Opaque
    }
}

impl WindowView {
    fn new(
        core: Arc<WindowState>,
        config: EditorConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (bridge, mut wakeups) = SaveBridge::new();
        let saves = cx.spawn(async move |this, cx| {
            while wakeups.next().await.is_some() {
                if this.update(cx, |this, cx| this.drain_saves(cx)).is_err() {
                    return;
                }
            }
        });
        // "System" follows the OS appearance.
        let observed = cx.weak_entity();
        let appearance_changes = window.observe_window_appearance(move |_, cx| {
            let _ = observed.update(cx, |this, cx| this.settings_applied(cx));
        });
        let sidebar_filter =
            cx.new(|cx| crate::editor::text_field::TextField::new("Search", cx).bare(15.0));
        let sidebar_filter_events = cx.subscribe_in(&sidebar_filter, window, Self::on_filter_event);
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = this.update(cx, |view, _| view.before_close());
            true
        });
        Self {
            core,
            chrome: Chrome::Workspace,
            workspace: None,
            tabs: Tabs::new(),
            docs: Documents::default(),
            config,
            bridge,
            saver: None,
            pending_undos: HashMap::new(),
            active_editor: None,
            notice: None,
            session_task: None,
            sessions: crate::app::core(cx).sessions.clone(),
            focus_handle: cx.focus_handle(),
            handle: window.window_handle().downcast::<WindowView>(),
            editor_subscriptions: HashMap::new(),
            sidebar: None,
            sidebar_width: 300.0,
            sidebar_visible: true,
            resizing: false,
            _sidebar_events: None,
            sidebar_filter,
            sidebar_filter_hovered: false,
            _sidebar_filter_events: sidebar_filter_events,
            palette: None,
            _palette_events: None,
            settings_panel: None,
            recents_picker: None,
            picker_focus: cx.focus_handle(),
            tab_menu: None,
            tab_scroll: gpui::ScrollHandle::new(),
            stats_cache: None,
            _settings_events: None,
            background: None,
            _appearance: appearance_changes,
            _tasks: vec![saves],
        }
    }

    // ---- Sidebar ----

    fn read_sidebar_settings(&mut self) {
        let settings = self.core.settings();
        self.sidebar_width = settings
            .get(sidebar::WIDTH_KEY)
            .and_then(|v| v.as_f64())
            .map_or(300.0, |w| w as f32);
        self.sidebar_visible = settings
            .get(sidebar::VISIBLE_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
    }

    /// Creates the sidebar for the workspace, or tells it the workspace
    /// changed.
    fn sync_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.workspace.is_none() || self.chrome == Chrome::Compact {
            self.sidebar = None;
            self._sidebar_events = None;
            return;
        }
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(cx, |sidebar, cx| sidebar.workspace_changed(cx));
            self.sidebar_filter
                .update(cx, |field, cx| field.set_text("", cx));
            return;
        }
        let state = self.core.clone();
        let sidebar = cx.new(|cx| Sidebar::new(state, cx));
        self._sidebar_events = Some(cx.subscribe(&sidebar, |this, _, event, cx| {
            this.on_sidebar_event(event.clone(), cx)
        }));
        self.sidebar = Some(sidebar);
    }

    fn on_sidebar_event(&mut self, event: SidebarEvent, cx: &mut Context<Self>) {
        match event {
            SidebarEvent::Open(path) => self.with_window(cx, move |this, window, cx| {
                this.open_file(&path, window, cx)
            }),
            SidebarEvent::OpenInNewTab(path) => self.with_window(cx, move |this, window, cx| {
                this.open_file_in_new_tab(&path, window, cx)
            }),
            SidebarEvent::Renamed { old, new } => self.path_renamed(&old, &new, cx),
            SidebarEvent::Notice(notice) => self.show_notice(notice, cx),
            SidebarEvent::Delete(paths) => self.with_window(cx, move |this, window, cx| {
                this.request_delete(paths, window, cx)
            }),
            SidebarEvent::Duplicate(path) => self.with_window(cx, move |this, window, cx| {
                this.duplicate(&path, window, cx)
            }),
            SidebarEvent::SettingsChanged => cx.notify(),
        }
    }

    fn is_dirty(&self, path: &Path, cx: &App) -> bool {
        self.docs
            .get(path)
            .is_some_and(|doc| doc.editor.read(cx).state.is_dirty())
    }

    /// Unsaved documents at `path` or under it.
    fn dirty_under(&self, path: &Path, cx: &App) -> usize {
        self.docs
            .iter()
            .filter(|(p, doc)| p.starts_with(path) && doc.editor.read(cx).state.is_dirty())
            .count()
    }

    /// The confirmation a delete needs (`use-file-tree-context-menus.ts:118-252`):
    /// only when something unsaved is involved, and always for several items.
    pub fn delete_confirmation(&self, paths: &[PathBuf], cx: &App) -> Option<String> {
        if let [path] = paths {
            let name = file_name(path);
            if path.is_dir() {
                let n = self.dirty_under(path, cx);
                return (n > 0)
                    .then(|| format!("\"{name}\" contains {n} unsaved file(s). Delete anyway?"));
            }
            return self
                .is_dirty(path, cx)
                .then(|| format!("\"{name}\" has unsaved changes. Delete anyway?"));
        }
        let unsaved = paths.iter().filter(|p| self.dirty_under(p, cx) > 0).count();
        let n = paths.len();
        Some(if unsaved > 0 {
            format!("{unsaved} of {n} selected items have unsaved changes. Delete anyway?")
        } else {
            format!("Delete {n} items?")
        })
    }

    pub fn request_delete(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self.delete_confirmation(&paths, cx) else {
            self.delete_now(&paths, cx);
            return;
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &message,
            None,
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.delete_now(&paths, cx));
            }
        })
        .detach();
    }

    /// Moves `paths` to the trash; their tabs close and unsaved text goes.
    pub fn delete_now(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let Some(fs) = self.fs() else { return };
        let mut failures = Vec::new();
        for path in paths {
            match fs.delete(path) {
                Ok(()) => {
                    self.path_removed(path, cx);
                    if let Some(sidebar) = self.sidebar.clone() {
                        sidebar.update(cx, |sidebar, cx| sidebar.removed(path, cx));
                    }
                }
                Err(error) if paths.len() > 1 => {
                    failures.push(format!("Failed to delete \"{}\": {error}", file_name(path)))
                }
                Err(error) => failures.push(format!("Failed to delete: {error}")),
            }
        }
        if !failures.is_empty() {
            self.show_notice(failures.join("\n"), cx);
        }
    }

    /// "Duplicate": `<stem> copy.md`, with the unsaved text when the file
    /// has some, opened in a new tab (`duplicate-file.ts:26-73`).
    pub fn duplicate(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fs) = self.fs() else { return };
        let unsaved = self
            .docs
            .get(path)
            .filter(|doc| doc.loading_since.is_none())
            .and_then(|doc| {
                let state = &doc.editor.read(cx).state;
                state.is_dirty().then(|| state.doc().to_owned())
            });
        match fs.duplicate(path, unsaved.as_deref()) {
            Ok(copy) => {
                if let (Some(sidebar), Some(parent)) = (self.sidebar.clone(), copy.parent()) {
                    let parent = parent.to_path_buf();
                    sidebar.update(cx, |sidebar, cx| sidebar.refresh_dir(&parent, cx));
                }
                self.open_file_in_new_tab(&copy, window, cx);
            }
            Err(error) => self.show_notice(format!("Failed to duplicate: {error}"), cx),
        }
    }

    /// Runs `f` with this view's window, after the current update (events
    /// arrive without one).
    fn with_window(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(handle) = self.handle else { return };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |view, window, cx| f(view, window, cx));
        });
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.chrome == Chrome::Compact || self.workspace.is_none() {
            return;
        }
        self.sidebar_visible = !self.sidebar_visible;
        self.write_setting(
            sidebar::VISIBLE_KEY,
            cerata_core::settings::ConfigValue::Bool(self.sidebar_visible),
            cx,
        );
        cx.notify();
    }

    #[cfg(test)]
    pub fn settings(&self) -> &cerata_core::settings::Settings {
        self.core.settings()
    }

    /// The settings changed (here or in a config file): the editors take
    /// the new size, zoom and fonts, and every window takes the theme.
    pub fn settings_applied(&mut self, cx: &mut Context<Self>) {
        let config = EditorConfig::from_settings(&self.core.settings().merged());
        if config != self.config {
            self.config = config.clone();
            let editors: Vec<Entity<Editor>> = self
                .docs
                .iter()
                .map(|(_, doc)| doc.editor.clone())
                .collect();
            for editor in editors {
                editor.update(cx, |editor, cx| editor.set_config(config.clone(), cx));
            }
        }
        let (_, translucent) = crate::theme::apply(self.core.settings(), cx.window_appearance());
        self.background = Some(background_appearance(translucent));
        cx.refresh_windows();
    }

    /// Cmd+P, Cmd+O and Cmd+N need a folder: the workspace, or the compact
    /// window's file (`use-keyboard-shortcuts.ts:53-97`).
    pub fn has_root(&self) -> bool {
        self.workspace.is_some()
            || (self.chrome == Chrome::Compact && self.tabs.active_path().is_some())
    }

    /// A press on the sidebar's edge starts a resize.
    pub fn begin_resize(&mut self, cx: &mut Context<Self>) {
        self.resizing = true;
        cx.notify();
    }

    pub fn resize_sidebar(&mut self, x: f32, window: &Window, cx: &mut Context<Self>) {
        let window_width = f32::from(window.viewport_size().width);
        let width = sidebar::tree::clamp_width(x, window_width);
        if width != self.sidebar_width {
            self.sidebar_width = width;
            cx.notify();
        }
    }

    /// The release saves the width, if it changed.
    pub fn end_resize(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.resizing) {
            return;
        }
        let saved = self
            .core
            .settings()
            .get(sidebar::WIDTH_KEY)
            .and_then(|v| v.as_f64())
            .map(|w| w as f32);
        if saved != Some(self.sidebar_width) {
            self.write_setting(
                sidebar::WIDTH_KEY,
                cerata_core::settings::ConfigValue::Number(f64::from(self.sidebar_width)),
                cx,
            );
        }
        cx.notify();
    }

    /// Shows `value` at once and writes the global config in the background.
    pub fn write_setting(
        &self,
        key: &str,
        value: cerata_core::settings::ConfigValue,
        cx: &mut Context<Self>,
    ) {
        let pending =
            self.core
                .settings()
                .begin_set(key, value, cerata_core::settings::Layer::Global);
        self.commit_setting(key, pending, cx);
    }

    /// Removes `key` from the global config: its default applies once the
    /// file is written.
    pub fn reset_setting(&self, key: &str, cx: &mut Context<Self>) {
        let pending = self
            .core
            .settings()
            .begin_reset(key, cerata_core::settings::Layer::Global);
        self.commit_setting(key, pending, cx);
    }

    fn commit_setting(
        &self,
        key: &str,
        pending: Result<cerata_core::settings::PendingWrite, cerata_core::settings::SettingsError>,
        cx: &mut Context<Self>,
    ) {
        let pending = match pending {
            Ok(pending) => pending,
            Err(error) => {
                log::warn!("{key} not set: {error}");
                return;
            }
        };
        let key = key.to_owned();
        let write = cx
            .background_executor()
            .spawn(async move { pending.commit() });
        cx.spawn(async move |this, cx| {
            if let Err(error) = write.await {
                log::warn!("{key} not saved: {error}");
            }
            let _ = this.update(cx, |this, cx| this.setting_written(cx));
        })
        .detach();
    }

    /// A setting changed from this window: everything that reads one
    /// follows.
    pub fn setting_written(&mut self, cx: &mut Context<Self>) {
        self.settings_applied(cx);
        self.read_sidebar_settings();
        if let Some(sidebar) = self.sidebar.clone() {
            sidebar.update(cx, |sidebar, cx| sidebar.settings_changed(cx));
        }
        if let Some(panel) = self.settings_panel.clone() {
            panel.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// The settings tab's panel, created the first time it shows.
    fn ensure_settings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_panel.is_some()
            || !matches!(self.tabs.active().location, Location::Settings)
        {
            return;
        }
        let state = self.core.clone();
        let panel = cx.new(|cx| crate::settings_panel::SettingsPanel::new(state, window, cx));
        self._settings_events = Some(cx.subscribe(
            &panel,
            |this, _, event, cx| match event.clone() {
                crate::settings_panel::PanelEvent::Set(key, value) => {
                    this.write_setting(&key, value, cx);
                    this.setting_written(cx);
                }
                crate::settings_panel::PanelEvent::Reset(key) => this.reset_setting(&key, cx),
            },
        ));
        self.settings_panel = Some(panel);
    }

    pub fn core_id(&self) -> u64 {
        self.core.id()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace.as_ref().map(|info| info.root.as_path())
    }

    /// `true` for events of the workspace this window shows now.
    pub fn owns(&self, envelope: &WatchEnvelope) -> bool {
        self.core
            .snapshot()
            .is_some_and(|snapshot| envelope.is_for(&snapshot.root, snapshot.epoch))
    }

    fn hydrate(
        &mut self,
        bundle: StartupBundle,
        restore_session: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.read_sidebar_settings();
        if let Some(file) = bundle.standalone_file {
            self.chrome = Chrome::Compact;
            self.start_saver(cx);
            let path = file.path.clone();
            self.seed_doc(file, cx);
            self.tabs.replace_all(&path);
        } else if let Some(info) = bundle.workspace {
            self.workspace = Some(info);
            self.start_saver(cx);
            let restore = restore_session && self.restores_open_files();
            if let (true, Some(session)) = (restore, bundle.session.as_ref()) {
                self.tabs = Tabs::from_session(session);
                if let Some(file) = bundle.active_file {
                    self.seed_doc(file, cx);
                }
                self.drop_missing_paths();
                // The active tab shows first; the others load after it.
                let paths: Vec<PathBuf> = self
                    .tabs
                    .tabs()
                    .iter()
                    .filter_map(|tab| tab.location.path().map(Path::to_path_buf))
                    .collect();
                for path in paths {
                    self.ensure_doc(&path, None, cx);
                }
            }
        }
        self.sync_sidebar(cx);
        self.tabs_changed(window, cx);
    }

    fn restores_open_files(&self) -> bool {
        self.core
            .settings()
            .get(RESTORE_OPEN_FILES_KEY)
            .and_then(|value| value.as_bool())
            .unwrap_or(true)
    }

    /// Restored tabs and history entries whose file is gone are removed,
    /// and each removal is logged.
    fn drop_missing_paths(&mut self) {
        let missing: Vec<PathBuf> = self
            .tabs
            .referenced_paths()
            .filter(|path| !path.is_file())
            .map(Path::to_path_buf)
            .collect();
        for path in missing {
            log::info!(
                "session: {} no longer exists, its tab is removed",
                path.display()
            );
            self.tabs.remove_prefix(&path);
        }
    }

    // ---- File systems and autosave ----

    fn fs(&self) -> Option<WorkspaceFs> {
        if let Some(runtime) = self.core.runtime() {
            return Some(runtime.fs.clone());
        }
        let file = self.core.standalone_file()?;
        WorkspaceFs::new(file.parent()?).ok()
    }

    fn start_saver(&mut self, cx: &mut Context<Self>) {
        self.saver = None;
        let Some(fs) = self.fs() else { return };
        let self_writes = self
            .core
            .runtime()
            .map(|runtime| runtime.self_writes.clone());
        let options = SaveOptions::from_settings(self.core.settings());
        match AutoSaver::spawn(fs, self.bridge.clone(), options, self_writes) {
            Ok(saver) => self.saver = Some(saver),
            Err(error) => {
                log::error!("autosave unavailable: {error}");
                self.show_notice("Autosave is unavailable: changes are not saved", cx);
            }
        }
    }

    /// Applies the save results queued by the save threads.
    pub fn drain_saves(&mut self, cx: &mut Context<Self>) {
        for result in self.bridge.take_results() {
            self.on_save_result(result, cx);
        }
    }

    fn on_save_result(&mut self, result: SaveResult, cx: &mut Context<Self>) {
        match result {
            SaveResult::Saved {
                path,
                revision,
                disk_text,
                modified_at,
            } => {
                let Some(doc) = self.docs.get_mut(&path) else {
                    return;
                };
                doc.disk_text = disk_text;
                doc.modified_at = modified_at;
                doc.save_error = None;
                doc.editor
                    .update(cx, |editor, _| editor.state.mark_saved_at(revision));
                if std::mem::take(&mut doc.change_pending) {
                    self.check_disk(&path, cx);
                }
                self.refresh_sidebar_for(&path, cx);
            }
            SaveResult::Failed { path, error, .. } => {
                log::error!("save of {} failed: {error}", path.display());
                let Some(doc) = self.docs.get_mut(&path) else {
                    return;
                };
                // Said once per failure streak; the tab keeps a dot.
                let first = doc.save_error.is_none();
                doc.save_error = Some(error.clone());
                if first {
                    self.notice = Some(
                        format!(
                            "Cannot save {}: {error}. Your changes stay in the editor; the next edit tries again.",
                            file_name(&path)
                        )
                        .into(),
                    );
                }
            }
        }
        cx.notify();
    }

    fn is_saving(&self, path: &Path) -> bool {
        self.bridge.is_writing(path)
            || self
                .saver
                .as_ref()
                .is_some_and(|saver| saver.is_in_flight(path))
    }

    // ---- Documents ----

    fn new_editor(&mut self, path: &Path, text: String, cx: &mut Context<Self>) -> Entity<Editor> {
        let root = self.workspace_root().map(Path::to_path_buf);
        let config = self.config.clone();
        let path = path.to_path_buf();
        let editor = cx.new(|cx| {
            let mut editor = Editor::new(text, Some(path), config, cx);
            if let Some(root) = &root {
                editor.images.set_root(root);
            }
            editor
        });
        let subscription = cx.observe(&editor, |this, editor, cx| {
            this.on_editor_changed(&editor, cx)
        });
        self.editor_subscriptions
            .insert(editor.entity_id(), subscription);
        editor
    }

    /// A document already read (startup prefetch).
    fn seed_doc(&mut self, file: FileContent, cx: &mut Context<Self>) {
        let editor = self.new_editor(&file.path, file.content.clone(), cx);
        let mut doc = Doc::new(editor.clone(), false);
        doc.seen_revision = editor.read(cx).state.revision();
        doc.disk_text = file.content;
        doc.format = file.format;
        doc.modified_at = file.modified_at;
        self.docs.insert(file.path, doc);
    }

    /// Starts reading `path` unless it is open or already being read: two
    /// opens of the same file share one read. `undo` rolls the navigation
    /// back if the read fails.
    fn ensure_doc(&mut self, path: &Path, undo: Option<Undo>, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.get(path) {
            if doc.loading_since.is_some() {
                self.pending_undos
                    .entry(path.to_path_buf())
                    .or_default()
                    .extend(undo);
            }
            return;
        }
        let Some(fs) = self.fs() else {
            if let Some(undo) = undo {
                self.tabs.rollback(&undo);
            }
            return;
        };
        self.pending_undos
            .entry(path.to_path_buf())
            .or_default()
            .extend(undo);
        let editor = self.new_editor(path, String::new(), cx);
        editor.update(cx, |editor, _| editor.state = EditorState::loading());
        self.docs.insert(path.to_path_buf(), Doc::new(editor, true));
        let target = path.to_path_buf();
        let read = cx.spawn(async move |this, cx| {
            let reader = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move { fs.read_file(&reader) })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_load(&target, result, cx));
        });
        let spinner = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOAD_GRACE).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        self._tasks.push(read);
        self._tasks.push(spinner);
    }

    fn finish_load(
        &mut self,
        path: &Path,
        result: Result<FileContent, FsError>,
        cx: &mut Context<Self>,
    ) {
        let undos = self.pending_undos.remove(path).unwrap_or_default();
        match result {
            Ok(file) => {
                let Some(doc) = self.docs.get_mut(path) else {
                    return;
                };
                doc.loading_since = None;
                doc.disk_text = file.content.clone();
                doc.format = file.format;
                doc.modified_at = file.modified_at;
                let editor = doc.editor.clone();
                let revision = editor.update(cx, |editor, cx| {
                    editor.update_state(cx, |state| state.reload(file.content));
                    editor.state.revision()
                });
                if let Some(doc) = self.docs.get_mut(path) {
                    doc.seen_revision = revision;
                }
            }
            Err(error) => {
                log::warn!("cannot open {}: {error}", path.display());
                self.forget_doc(path);
                let mut rolled_back = false;
                for undo in &undos {
                    rolled_back |= self.tabs.rollback(undo);
                }
                if !rolled_back {
                    // A restored tab with no navigation to undo.
                    self.tabs.remove_prefix(path);
                }
                let name = file_name(path);
                self.show_notice(format!("Failed to open {name}"), cx);
            }
        }
        self.after_tabs_change(cx);
    }

    fn forget_doc(&mut self, path: &Path) {
        if let Some(doc) = self.docs.remove(path) {
            self.editor_subscriptions.remove(&doc.editor.entity_id());
        }
        self.bridge.clear(path);
        if let Some(saver) = &self.saver {
            saver.cancel(path);
        }
    }

    fn on_editor_changed(&mut self, editor: &Entity<Editor>, cx: &mut Context<Self>) {
        let id = editor.entity_id();
        let Some((path, doc)) = self
            .docs
            .iter()
            .find(|(_, doc)| doc.editor.entity_id() == id)
            .map(|(path, doc)| (path.clone(), doc))
        else {
            return;
        };
        let state = &editor.read(cx).state;
        let revision = state.revision();
        if revision == doc.seen_revision || doc.loading_since.is_some() {
            return;
        }
        let dirty = state.is_dirty();
        let snapshot = SaveSnapshot {
            revision,
            text: state.doc().to_owned(),
            format: doc.format,
        };
        let conflicted = doc.conflict.is_some();
        if let Some(doc) = self.docs.get_mut(&path) {
            doc.seen_revision = revision;
        }
        if dirty && !conflicted {
            self.bridge.set_dirty(&path, snapshot);
            if let Some(saver) = &self.saver {
                saver.schedule(&path);
            }
        } else if !dirty {
            self.bridge.clear(&path);
        }
        cx.notify();
    }

    /// Drops documents no tab references that hold nothing unsaved.
    fn prune(&mut self, cx: &App) {
        let unreferenced = self.docs.unreferenced(
            |path| self.tabs.references(path),
            |doc| doc.conflict.is_some() || doc.editor.read(cx).state.is_dirty(),
        );
        for path in unreferenced {
            self.forget_doc(&path);
        }
    }

    // ---- Tabs ----

    pub fn page(&self, cx: &App) -> Page {
        match &self.tabs.active().location {
            Location::Launcher if self.workspace.is_none() && self.chrome == Chrome::Workspace => {
                Page::Welcome
            }
            Location::Launcher => Page::Launcher,
            Location::Settings => match &self.settings_panel {
                Some(panel) => Page::Settings(panel.clone()),
                None => Page::Loading { spinner: false },
            },
            Location::File { path } => match self.docs.get(path) {
                Some(doc) if doc.loading_since.is_some() => Page::Loading {
                    spinner: doc.shows_spinner(std::time::Instant::now()),
                },
                Some(doc) => Page::Editor(doc.editor.clone()),
                None => {
                    let _ = cx;
                    Page::Loading { spinner: false }
                }
            },
        }
    }

    pub fn active_doc(&self) -> Option<&Doc> {
        self.docs.get(self.tabs.active_path()?)
    }

    /// Opens `path` as a click does (replaces a launcher or navigates).
    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let undo = self.tabs.open(path);
        if undo.is_some() {
            self.ensure_doc(path, undo, cx);
        }
        self.tabs_changed(window, cx);
    }

    pub fn open_file_in_new_tab(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let undo = self.tabs.open_in_new_tab(path);
        self.ensure_doc(path, Some(undo), cx);
        self.tabs_changed(window, cx);
    }

    pub fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.activate(index) {
            self.tabs_changed(window, cx);
        }
    }

    pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.close(index).is_some() {
            self.tabs_changed(window, cx);
        }
    }

    pub fn close_other_tabs(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.close_others(index);
        self.tabs_changed(window, cx);
    }

    pub fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.close_all();
        self.tabs_changed(window, cx);
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.open_launcher();
        self.tabs_changed(window, cx);
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(undo) = self.tabs.back() {
            self.navigated(undo, window, cx);
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(undo) = self.tabs.forward() {
            self.navigated(undo, window, cx);
        }
    }

    fn navigated(&mut self, undo: Undo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.tabs.active_path().map(Path::to_path_buf) {
            self.ensure_doc(&path, Some(undo), cx);
        }
        self.tabs_changed(window, cx);
    }

    /// A file or folder was renamed or moved inside Cerata.
    pub fn path_renamed(&mut self, old: &Path, new: &Path, cx: &mut Context<Self>) {
        self.tabs.rewrite_prefix(old, new);
        for (from, to) in self.docs.rewrite_prefix(old, new) {
            self.bridge.rename(&from, &to);
            if let Some(saver) = &self.saver {
                saver.cancel(&from);
                let dirty = self
                    .docs
                    .get(&to)
                    .is_some_and(|doc| doc.editor.read(cx).state.is_dirty());
                if dirty {
                    saver.schedule(&to);
                }
            }
            if let Some(doc) = self.docs.get(&to) {
                doc.editor
                    .update(cx, |editor, _| editor.path = Some(to.clone()));
            }
        }
        self.after_tabs_change(cx);
    }

    /// A file or folder was deleted inside Cerata.
    pub fn path_removed(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.tabs.remove_prefix(path);
        let gone: Vec<PathBuf> = self
            .docs
            .paths()
            .filter(|p| p.starts_with(path))
            .cloned()
            .collect();
        for path in gone {
            self.forget_doc(&path);
        }
        self.after_tabs_change(cx);
    }

    fn tabs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.after_tabs_change(cx);
        self.sync_active(window, cx);
    }

    fn after_tabs_change(&mut self, cx: &mut Context<Self>) {
        if let Some(sidebar) = self.sidebar.clone() {
            let active = self.tabs.active_path().map(Path::to_path_buf);
            sidebar.update(cx, |sidebar, cx| sidebar.set_active_path(active, cx));
        }
        self.prune(cx);
        self.schedule_session_save(cx);
        cx.notify();
    }

    /// The editor of the active tab gets the focus; the one it replaces is
    /// told it is inactive (its search closes).
    fn sync_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = match self.page(cx) {
            Page::Editor(editor) => Some(editor),
            _ => None,
        };
        let active_id = active.as_ref().map(Entity::entity_id);
        if active_id == self.active_editor {
            return;
        }
        if let Some(previous) = self.active_editor
            && let Some(doc) = self
                .docs
                .iter()
                .find(|(_, doc)| doc.editor.entity_id() == previous)
                .map(|(_, doc)| doc.editor.clone())
        {
            doc.update(cx, |editor, cx| editor.set_active(false, cx));
        }
        self.active_editor = active_id;
        match active {
            Some(editor) => {
                editor.update(cx, |editor, cx| editor.set_active(true, cx));
                window.focus(&editor.focus_handle(cx), cx);
            }
            None => window.focus(&self.focus_handle, cx),
        }
    }

    /// Writes the edits the autosave throttle still holds: dropping the
    /// saver or a document would lose them.
    pub fn flush_saves(&self) {
        if let Some(saver) = &self.saver
            && !saver.flush(FLUSH_TIMEOUT)
        {
            log::error!("saves still running when the window let go of them");
        }
    }

    /// The window is about to close (or the app to quit): pending edits
    /// reach the disk and the session is saved.
    pub fn before_close(&mut self) {
        self.flush_saves();
        self.save_session_now();
    }

    // ---- Session ----

    fn schedule_session_save(&mut self, cx: &mut Context<Self>) {
        if self.workspace.is_none() || self.chrome == Chrome::Compact {
            return;
        }
        self.session_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SESSION_DELAY).await;
            let _ = this.update(cx, |this, _| this.save_session_now());
        }));
    }

    pub fn save_session_now(&mut self) {
        self.session_task = None;
        let Some(root) = self.workspace_root() else {
            return;
        };
        if self.chrome == Chrome::Compact || !self.restores_open_files() {
            return;
        }
        if let Err(error) = self.sessions.save(root, self.tabs.to_session()) {
            log::warn!("session not saved: {error}");
        }
    }

    // ---- Workspace and chrome changes ----

    /// Opens `root` in this window (an empty workspace window), then
    /// `file` or the saved session.
    pub fn open_workspace(
        &mut self,
        root: &Path,
        file: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let core = crate::app::core(cx);
        self.flush_saves();
        let info = core.open_workspace(&self.core, root, crate::app::events(cx))?;
        self.chrome = Chrome::Workspace;
        self.workspace = Some(info.clone());
        self.docs = Documents::default();
        self.editor_subscriptions.clear();
        self.active_editor = None;
        self.start_saver(cx);
        self.sync_sidebar(cx);
        let session = if file.is_none() && self.restores_open_files() {
            core.sessions.load(&info.root).unwrap_or_else(|error| {
                log::warn!("session unavailable: {error}");
                None
            })
        } else {
            None
        };
        self.tabs = session.as_ref().map(Tabs::from_session).unwrap_or_default();
        self.drop_missing_paths();
        let paths: Vec<PathBuf> = self
            .tabs
            .tabs()
            .iter()
            .filter_map(|tab| tab.location.path().map(Path::to_path_buf))
            .collect();
        for path in paths {
            self.ensure_doc(&path, None, cx);
        }
        match file {
            Some(file) => self.open_file(file, window, cx),
            None => self.tabs_changed(window, cx),
        }
        Ok(())
    }

    /// Turns this rootless window into a compact window on `file`.
    pub fn make_compact(
        &mut self,
        file: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if !file.is_file() {
            anyhow::bail!("{} not found", file_name(file));
        }
        self.core.set_standalone_file(Some(file.to_path_buf()));
        self.chrome = Chrome::Compact;
        self.start_saver(cx);
        self.tabs.replace_all(file);
        self.ensure_doc(file, None, cx);
        self.tabs_changed(window, cx);
        Ok(())
    }

    // ---- Disk changes (US-025) ----

    pub fn on_watch_event(
        &mut self,
        event: WatchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = window;
        match event {
            WatchEvent::FileChanged { path, change } => {
                self.refresh_sidebar_for(&path, cx);
                if !self.docs.contains(&path) {
                    return;
                }
                match change {
                    FileChange::Removed => self.file_removed(&path, cx),
                    FileChange::Created | FileChange::Modified => {
                        if self.is_saving(&path) {
                            if let Some(doc) = self.docs.get_mut(&path) {
                                doc.change_pending = true;
                            }
                        } else {
                            self.check_disk(&path, cx);
                        }
                    }
                }
            }
            WatchEvent::RootRemoved => {
                self.show_notice("The workspace folder was moved or deleted", cx);
            }
            WatchEvent::WatchLimited { message } => self.show_notice(message, cx),
            WatchEvent::DirectoryChanged { path } => {
                if let Some(sidebar) = self.sidebar.clone() {
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.refresh_dir(&path, cx);
                        sidebar.refresh_sections(cx);
                    });
                }
            }
            WatchEvent::IgnoreRebuilt | WatchEvent::IndexComplete { .. } => {
                if let Some(sidebar) = self.sidebar.clone() {
                    sidebar.update(cx, |sidebar, cx| sidebar.refresh_all(cx));
                }
            }
            WatchEvent::SettingsChanged => {
                self.settings_applied(cx);
                self.read_sidebar_settings();
                if let Some(sidebar) = self.sidebar.clone() {
                    sidebar.update(cx, |sidebar, cx| sidebar.settings_changed(cx));
                }
                cx.notify();
            }
        }
    }

    /// A file's title or time changed: its folder and the sections follow.
    fn refresh_sidebar_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        if let Some(runtime) = self.core.runtime() {
            runtime
                .index
                .lock()
                .touch(path, cerata_core::fs::modified_time(path));
        }
        sidebar.update(cx, |sidebar, cx| {
            sidebar.refresh_dir(&parent, cx);
            sidebar.refresh_sections(cx);
        });
    }

    /// Compares the disk with the text last read or written, then reloads
    /// a clean document or raises a conflict on a dirty one.
    fn check_disk(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(fs) = self.fs() else { return };
        match fs.read_file(path) {
            Ok(file) => self.disk_changed(path, file, cx),
            Err(FsError::NotFound(_)) => self.file_removed(path, cx),
            Err(error) => log::warn!("cannot read {}: {error}", path.display()),
        }
    }

    fn disk_changed(&mut self, path: &Path, file: FileContent, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(path) else {
            return;
        };
        if doc.loading_since.is_some() || file.content == doc.disk_text {
            return;
        }
        let editor = doc.editor.clone();
        let dirty = editor.read(cx).state.is_dirty();
        if dirty {
            doc.conflict = Some(Conflict::Modified { disk: file.content });
            self.hold_saves(path);
        } else {
            doc.disk_text = file.content.clone();
            doc.format = file.format;
            doc.modified_at = file.modified_at;
            doc.conflict = None;
            let revision = editor.update(cx, |editor, cx| {
                editor.update_state(cx, |state| state.reload(file.content));
                editor.state.revision()
            });
            if let Some(doc) = self.docs.get_mut(path) {
                doc.seen_revision = revision;
            }
        }
        cx.notify();
    }

    fn file_removed(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(path) else {
            return;
        };
        if !doc.editor.read(cx).state.is_dirty() {
            // Nothing unsaved is at risk (Writer ignores deletions too).
            return;
        }
        doc.conflict = Some(Conflict::Deleted);
        self.hold_saves(path);
        cx.notify();
    }

    /// No autosave may write `path` until the user decides.
    fn hold_saves(&mut self, path: &Path) {
        self.bridge.clear(path);
        if let Some(saver) = &self.saver {
            saver.cancel(path);
        }
    }

    /// "Keep my version": the editor's text wins and is written.
    pub fn keep_mine(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_path().map(Path::to_path_buf) else {
            return;
        };
        let Some(doc) = self.docs.get_mut(&path) else {
            return;
        };
        if let Some(Conflict::Modified { disk }) = doc.conflict.take() {
            doc.disk_text = disk;
        }
        doc.compare = false;
        self.resume_saves(&path, cx);
    }

    /// "Recreate": writes the editor's text back to the deleted path.
    pub fn recreate(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_path().map(Path::to_path_buf) else {
            return;
        };
        if let Some(doc) = self.docs.get_mut(&path) {
            doc.conflict = None;
            doc.disk_text.clear();
        }
        self.resume_saves(&path, cx);
    }

    fn resume_saves(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(path) else {
            return;
        };
        let state = &doc.editor.read(cx).state;
        self.bridge.set_dirty(
            path,
            SaveSnapshot {
                revision: state.revision(),
                text: state.doc().to_owned(),
                format: doc.format,
            },
        );
        if let Some(saver) = &self.saver {
            saver.schedule(path);
        }
        cx.notify();
    }

    /// "Reload": the disk's text replaces the editor's.
    pub fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_path().map(Path::to_path_buf) else {
            return;
        };
        let Some(doc) = self.docs.get_mut(&path) else {
            return;
        };
        let Some(Conflict::Modified { disk }) = doc.conflict.take() else {
            return;
        };
        doc.compare = false;
        doc.disk_text = disk.clone();
        let editor = doc.editor.clone();
        let revision = editor.update(cx, |editor, cx| {
            editor.update_state(cx, |state| state.reload(disk));
            editor.state.revision()
        });
        if let Some(doc) = self.docs.get_mut(&path) {
            doc.seen_revision = revision;
        }
        cx.notify();
    }

    /// "Compare" shows or hides the disk and editor texts side by side.
    pub fn toggle_compare(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_path().map(Path::to_path_buf) else {
            return;
        };
        if let Some(doc) = self.docs.get_mut(&path) {
            doc.compare = !doc.compare;
        }
        cx.notify();
    }

    /// "Close" on a deleted file: its unsaved text is discarded.
    pub fn discard_deleted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_path().map(Path::to_path_buf) else {
            return;
        };
        self.forget_doc(&path);
        self.tabs.remove_prefix(&path);
        self.tabs_changed(window, cx);
    }

    // ---- Notices and drops ----

    pub fn show_notice(&mut self, notice: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        cx.notify();
    }

    fn on_drop(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let requests: Vec<OpenRequest> = paths
            .paths()
            .iter()
            .filter_map(|path| OpenRequest::from_drop(path))
            .collect();
        if requests.is_empty() {
            self.show_notice("Only folders and Markdown files can be opened", cx);
            return;
        }
        let origin = window.window_handle().downcast::<WindowView>();
        // Deferred: a route may update this very window.
        cx.defer(move |cx| {
            for request in requests {
                crate::app::open(cx, request, origin);
            }
        });
    }

    /// The window title (`window-title/index.tsx`).
    pub fn title(&self, cx: &App) -> String {
        match &self.tabs.active().location {
            Location::File { path } => {
                let name = file_name(path);
                let dirty = self
                    .docs
                    .get(path)
                    .is_some_and(|doc| doc.editor.read(cx).state.is_dirty());
                if dirty {
                    format!("{name} (unsaved) - Cerata")
                } else {
                    format!("{name} - Cerata")
                }
            }
            Location::Settings => "Settings - Cerata".into(),
            Location::Launcher if self.chrome == Chrome::Workspace && self.workspace.is_none() => {
                "Cerata".into()
            }
            Location::Launcher => "New Tab - Cerata".into(),
        }
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl Focusable for WindowView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A file that finished loading becomes the active editor here.
        if self.palette.is_none() && self.recents_picker.is_none() {
            self.sync_active(window, cx);
        }
        if let Some(background) = self.background.take() {
            window.set_background_appearance(background);
        }
        window.set_window_title(&self.title(cx));
        self.ensure_settings_panel(window, cx);
        let page = self.page(cx);
        let mut root = div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::text())
            .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    this.go_back(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    this.go_forward(window, cx);
                }
            }))
            .on_action(|_: &crate::a11y::FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &crate::a11y::FocusPrevious, window, cx| window.focus_prev(cx))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.on_drop(paths, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                if this.has_root() {
                    this.open_palette(crate::palette::Intent::Search, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPaletteAnywhere, window, cx| {
                this.open_palette(crate::palette::Intent::Search, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoToFile, window, cx| {
                if this.has_root() {
                    this.open_palette(crate::palette::Intent::Search, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewNote, window, cx| {
                if this.has_root() {
                    this.open_palette(crate::palette::Intent::CreateFile, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.set_zoom(crate::zoom::zoom_in(Some(this.zoom())), cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.set_zoom(crate::zoom::zoom_out(Some(this.zoom())), cx)
            }))
            .on_action(
                cx.listener(|this, _: &ResetZoom, _, cx| this.set_zoom(crate::zoom::DEFAULT, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenPreferences, window, cx| this.open_settings(window, cx)),
            )
            .on_action(|_: &crate::app_menu::Minimize, window, _| window.minimize_window())
            .on_action(|_: &crate::app_menu::ToggleFullScreen, window, _| {
                window.toggle_fullscreen()
            })
            .on_action(
                cx.listener(|this, _: &crate::app_menu::CloseWindow, window, _| {
                    this.before_close();
                    window.remove_window();
                }),
            );
        if self.chrome == Chrome::Workspace {
            root = root
                .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
                .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                    if this.workspace.is_some() {
                        this.new_tab(window, cx);
                    }
                }))
                .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                    if this.tabs.only_launchers() {
                        this.before_close();
                        window.remove_window();
                    } else {
                        let ix = this.tabs.active_index();
                        this.close_tab(ix, window, cx);
                    }
                }))
                .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                    if this.tabs.cycle(true) {
                        this.tabs_changed(window, cx);
                    }
                }))
                .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                    if this.tabs.cycle(false) {
                        this.tabs_changed(window, cx);
                    }
                }));
            root = register_tab_jumps(root, cx);
        }
        let mut main = div().flex().flex_col().flex_1().min_w_0().h_full();
        let header = match self.chrome {
            Chrome::Workspace if self.workspace.is_some() => {
                Some(tab_bar::render(self, cx).into_any_element())
            }
            Chrome::Compact => Some(compact::render(self, cx)),
            Chrome::Workspace => None,
        };
        let captions = title_bar::caption_buttons(window);
        if header.is_some() || captions.is_some() {
            let filler = header.is_none().then(|| div().flex_1());
            main = main.child(
                title_bar::row("title-bar")
                    .w_full()
                    .border_b_1()
                    .border_color(theme::border())
                    .children(header)
                    .children(filler)
                    .children(captions),
            );
        }
        let mut content = div().relative().flex_1().min_h_0().w_full();
        let mut comparing = None;
        if let Some(doc) = self.active_doc() {
            if let Some(conflict) = &doc.conflict {
                content = content.child(banner::render(conflict, doc.compare, cx));
            }
            if let (true, Some(Conflict::Modified { disk })) = (doc.compare, &doc.conflict) {
                let mine = doc.editor.read(cx).state.doc().to_owned();
                comparing = Some(banner::compare(disk, &mine));
            }
        }
        let editing = matches!(page, Page::Editor(_));
        content = match comparing {
            Some(compare) => content.child(compare),
            None => content.child(pages::render(page, cx)),
        };
        main = main.child(content);
        if editing && self.chrome == Chrome::Workspace {
            main = main.children(status_bar::render(self, cx));
        }

        let mut body = div().flex().flex_1().min_h_0().w_full();
        if let (Some(sidebar), true) = (self.sidebar.clone(), self.sidebar_visible) {
            body = body
                .child(
                    div()
                        .flex_none()
                        .w(px(self.sidebar_width))
                        .h_full()
                        .flex()
                        .flex_col()
                        .bg(theme::panel())
                        .child(title_bar::wordmark())
                        .child(div().flex_1().min_h_0().child(sidebar))
                        .child(sidebar_footer::render(self, window, cx)),
                )
                .child(
                    div()
                        .id("sidebar-resize")
                        .flex_none()
                        .w(px(8.0))
                        .ml(px(-4.0))
                        .mr(px(-4.0))
                        .h_full()
                        .flex()
                        .justify_center()
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                        .child(div().w(px(1.0)).h_full().bg(if self.resizing {
                            theme::accent()
                        } else {
                            theme::border()
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.begin_resize(cx);
                            }),
                        ),
                );
            root = root
                .on_mouse_move(
                    cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                        if this.resizing {
                            this.resize_sidebar(f32::from(event.position.x), window, cx);
                        }
                    }),
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.end_resize(cx)),
                );
        }
        body = body.child(main);
        root.child(body)
            .children(pages::notice(self.notice.clone(), cx))
            .children(compact::overlay(self, cx))
            .children(
                self.tab_menu
                    .as_mut()
                    .map(|menu| crate::menu::render(menu, window, cx)),
            )
            .children(self.palette.clone().map(|palette| {
                div()
                    .id("palette-overlay")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .flex()
                    .justify_center()
                    .pt(px(80.0))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                    )
                    .child(
                        div()
                            .id("palette-frame")
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .child(palette),
                    )
            }))
    }
}

impl WindowView {
    fn editor_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.active_doc().is_some_and(|doc| {
            doc.editor
                .read(cx)
                .focus_handle
                .contains_focused(window, cx)
        })
    }
}

fn register_tab_jumps(
    root: gpui::Stateful<gpui::Div>,
    cx: &mut Context<WindowView>,
) -> gpui::Stateful<gpui::Div> {
    macro_rules! jump {
        ($root:expr, $($action:ident => $ix:expr),*) => {
            $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                this.activate_tab($ix, window, cx)
            })))*
        };
    }
    jump!(root,
        ActivateTab1 => 0, ActivateTab2 => 1, ActivateTab3 => 2,
        ActivateTab4 => 3, ActivateTab5 => 4, ActivateTab6 => 5,
        ActivateTab7 => 6, ActivateTab8 => 7, ActivateTab9 => 8)
}
