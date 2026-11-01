//! The window's commands: the palette and what it runs, opening and
//! closing workspaces, the compact window's recents, zoom and theme.

use std::path::{Path, PathBuf};

use cerata_core::fs::titles::extract_title;
use cerata_core::settings::ConfigValue;
use gpui::{AppContext as _, Context, Focusable as _, PathPromptOptions, Window};

use super::{WindowView, file_name};
use crate::palette::{Availability, Choice, Command, Intent, Palette, PaletteEvent, RecentFile};
use crate::routing::{Chrome, OpenRequest};
use crate::sidebar::tree::stem;
use crate::zoom;

/// Global recents a compact window lists (`use-global-recent-files.ts:27`).
pub const COMPACT_RECENTS: usize = 30;

impl WindowView {
    pub fn availability(&self) -> Availability {
        Availability {
            has_workspace: self.workspace.is_some(),
            compact: self.chrome == Chrome::Compact,
            has_active_file: self.tabs.active_path().is_some(),
            has_tabs: !self.tabs.tabs().is_empty(),
        }
    }

    /// Global recent files, newest first, without `exclude`; labeled by
    /// title, else stem.
    pub fn global_recents(&self, exclude: Option<&Path>, cx: &gpui::App) -> Vec<RecentFile> {
        let core = crate::app::core(cx);
        core.recents
            .existing_files()
            .unwrap_or_else(|error| {
                log::warn!("recent files unavailable: {error}");
                Vec::new()
            })
            .into_iter()
            .filter(|path| Some(path.as_path()) != exclude)
            .take(COMPACT_RECENTS)
            .map(|path| RecentFile {
                label: extract_title(&path).unwrap_or_else(|| stem(&file_name(&path)).to_owned()),
                path,
            })
            .collect()
    }

    pub fn open_palette(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        let availability = self.availability();
        let available: Vec<Command> = Command::ALL
            .into_iter()
            .filter(|command| command.available(availability))
            .collect();
        let runtime = self.core.runtime();
        let compact = self.chrome == Chrome::Compact;
        let recents = if compact {
            self.global_recents(self.tabs.active_path(), cx)
        } else {
            Vec::new()
        };
        let palette =
            cx.new(|cx| Palette::new(intent, available, runtime, compact, recents, window, cx));
        self._palette_events = Some(cx.subscribe_in(
            &palette,
            window,
            |this, _, event, window, cx| match event.clone() {
                PaletteEvent::Dismissed => this.close_palette(window, cx),
                PaletteEvent::Chosen(choice) => {
                    this.close_palette(window, cx);
                    this.run_choice(choice, window, cx);
                }
            },
        ));
        self.palette = Some(palette);
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self._palette_events = None;
            self.active_editor = None;
            self.sync_active(window, cx);
            cx.notify();
        }
    }

    fn run_choice(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            Choice::Command(command) => self.run_command(command, window, cx),
            Choice::File(path) => self.open_file(&path, window, cx),
            Choice::Recent(path) => self.open_elsewhere(OpenRequest::File(path), window, cx),
            Choice::Create(name) => self.create_note(&name, window, cx),
        }
    }

    /// Routes `request` from this window, after the current update.
    pub fn open_elsewhere(
        &mut self,
        request: OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin = window.window_handle().downcast::<WindowView>();
        if self.chrome == Chrome::Compact
            && let OpenRequest::File(path) = &request
        {
            // A compact window navigates to the file it is asked for.
            let path = path.clone();
            if let Err(error) = self.make_compact(&path, window, cx) {
                self.show_notice(format!("Cannot open: {error}"), cx);
            }
            return;
        }
        cx.defer(move |cx| crate::app::open(cx, request, origin));
    }

    pub fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::ToggleSidebar => self.toggle_sidebar(cx),
            Command::NewFile => self.open_palette(Intent::CreateFile, window, cx),
            Command::OpenInCompactWindow => {
                if let Some(path) = self.tabs.active_path().map(Path::to_path_buf) {
                    let origin = None;
                    cx.defer(move |cx| crate::app::open(cx, OpenRequest::File(path), origin));
                }
            }
            Command::CloseTab => {
                let ix = self.tabs.active_index();
                self.close_tab(ix, window, cx);
            }
            Command::CloseAllTabs => self.close_all_tabs(window, cx),
            Command::OpenWorkspace => self.pick_workspace(window, cx),
            Command::CloseWorkspace => self.close_workspace(window, cx),
            Command::ToggleTheme => self.cycle_theme(cx),
            Command::ZoomIn => self.set_zoom(zoom::zoom_in(Some(self.zoom())), cx),
            Command::ZoomOut => self.set_zoom(zoom::zoom_out(Some(self.zoom())), cx),
            Command::ResetZoom => self.set_zoom(zoom::DEFAULT, cx),
            Command::Settings => self.open_settings(window, cx),
        }
    }

    /// "Create: name.md" in the workspace root (or the active file's
    /// folder), then opened.
    fn create_note(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fs) = self.fs() else { return };
        let Some(base) =
            crate::palette::create_base(self.workspace_root(), self.tabs.active_path())
        else {
            return;
        };
        match fs.create_file(&base.join(name)) {
            Ok(file) => {
                if self.chrome == Chrome::Compact {
                    if let Err(error) = self.make_compact(&file.path, window, cx) {
                        self.show_notice(format!("Cannot open: {error}"), cx);
                    }
                } else {
                    if let (Some(sidebar), Some(parent)) =
                        (self.sidebar.clone(), file.path.parent())
                    {
                        let parent = parent.to_path_buf();
                        sidebar.update(cx, |sidebar, cx| sidebar.refresh_dir(&parent, cx));
                    }
                    self.open_file(&file.path, window, cx);
                }
            }
            Err(error) => self.show_notice(format!("Failed to create file: {error}"), cx),
        }
    }

    /// "Open Workspace" and the welcome screen's "Add Folder".
    pub fn pick_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let handle = window.window_handle().downcast::<WindowView>();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(root) = paths.into_iter().next() else {
                return;
            };
            cx.update(|cx| open_picked_folder(cx, root, handle));
        })
        .detach();
    }

    /// The welcome screen's "Open File": its folder opens as the workspace,
    /// then the file.
    pub fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let origin = window.window_handle().downcast::<WindowView>();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            cx.update(|cx| open_picked_file(cx, file, origin));
        })
        .detach();
    }

    /// "Close Workspace": the session is saved and the window shows the
    /// welcome screen.
    pub fn close_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace_root().map(Path::to_path_buf) else {
            return;
        };
        self.before_close();
        self.core.clear_workspace_if_current(&root);
        self.workspace = None;
        self.saver = None;
        for path in self.docs.paths().cloned().collect::<Vec<PathBuf>>() {
            self.forget_doc(&path);
        }
        self.tabs = crate::tabs::Tabs::new();
        self.sync_sidebar(cx);
        self.tabs_changed(window, cx);
    }

    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome == Chrome::Compact {
            return;
        }
        self.ensure_settings_panel(window, cx);
        self.settings_open = true;
        if let Some(panel) = &self.settings_panel {
            window.focus(&panel.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Back to the app: the active tab gets the focus again.
    pub fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.settings_open) {
            return;
        }
        self.sync_active(window, cx);
        self.focus_page(window, cx);
        cx.notify();
    }

    // ---- Zoom and theme ----

    pub fn zoom(&self) -> f64 {
        zoom::normalize(
            self.core
                .settings()
                .get(zoom::ZOOM_KEY)
                .and_then(|v| v.as_f64()),
        )
    }

    /// Writes `editor.zoom` only (the font size is untouched) unless it is
    /// already the value.
    pub fn set_zoom(&mut self, value: f64, cx: &mut Context<Self>) {
        if (value - self.zoom()).abs() < f64::EPSILON {
            return;
        }
        self.write_setting(zoom::ZOOM_KEY, ConfigValue::Number(value), cx);
        self.settings_applied(cx);
    }

    /// "Toggle Dark Mode": system, then light, then dark.
    pub fn cycle_theme(&mut self, cx: &mut Context<Self>) {
        let next = crate::theme::Preference::parse(
            self.core
                .settings()
                .get(crate::theme::THEME_KEY)
                .and_then(|v| v.as_str().map(str::to_owned))
                .as_deref(),
        )
        .next();
        self.write_setting(
            crate::theme::THEME_KEY,
            ConfigValue::String(next.id().into()),
            cx,
        );
        self.settings_applied(cx);
    }
}

/// "Add Folder" once a folder is picked: it opens as the workspace.
pub fn open_picked_folder(
    cx: &mut gpui::App,
    root: PathBuf,
    origin: Option<gpui::WindowHandle<WindowView>>,
) {
    crate::app::open(cx, OpenRequest::Workspace { root, file: None }, origin);
}

/// "Open File" once a file is picked: its folder opens as the workspace,
/// then the file (`welcome/index.tsx:15-21`).
pub fn open_picked_file(
    cx: &mut gpui::App,
    file: PathBuf,
    origin: Option<gpui::WindowHandle<WindowView>>,
) {
    let Some(root) = file.parent().map(Path::to_path_buf) else {
        return;
    };
    crate::app::open(
        cx,
        OpenRequest::Workspace {
            root,
            file: Some(file),
        },
        origin,
    );
}
