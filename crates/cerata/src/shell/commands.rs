//! The window's commands: the palette and what it runs, opening and
//! closing workspaces, the compact window's recents, zoom and theme.

use std::path::{Path, PathBuf};

use cerata_core::fs::titles::{extract_title, title_from_text};
use cerata_core::settings::ConfigValue;
use gpui::{AppContext as _, Context, Focusable as _, PathPromptOptions, Window};

use super::{WindowView, file_name};
use crate::palette::{
    Availability, Choice, Command, HistoryCommand, Intent, Palette, PaletteEvent, RecentFile,
};
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
            history_open: self.history_panel.is_some(),
        }
    }

    /// Global recent files, newest first, without `exclude`; labeled by
    /// title, else stem. No file is read here (US-083): a title comes from
    /// the open document, the library's index or the titles read last time;
    /// [`Self::refresh_recent_titles`] reads the others in the background.
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
                label: self
                    .known_title(&path)
                    .unwrap_or_else(|| stem(&file_name(&path)).to_owned()),
                path,
            })
            .collect()
    }

    /// A note's title without reading its file, if one is known.
    pub fn known_title(&self, path: &Path) -> Option<String> {
        if let Some(doc) = self
            .docs
            .get(path)
            .filter(|doc| doc.loading_since.is_none())
        {
            return title_from_text(&doc.disk_text);
        }
        if let Some(runtime) = self.core.runtime() {
            let index = runtime.index.lock();
            if let Some(file) = index.get(path).filter(|file| file.hash.is_some()) {
                return file.title.clone();
            }
        }
        self.recent_titles.get(path).cloned().flatten()
    }

    /// Reads the titles of `recents` off the UI thread, then relabels the
    /// recents shown (the compact picker, the palette).
    pub fn refresh_recent_titles(&mut self, recents: &[RecentFile], cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = recents.iter().map(|recent| recent.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let titles: Vec<(PathBuf, Option<String>)> = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| {
                            let title = extract_title(&path);
                            (path, title)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.recent_titles.extend(titles);
                let label = |this: &Self, recent: &RecentFile| {
                    this.known_title(&recent.path)
                        .unwrap_or_else(|| stem(&file_name(&recent.path)).to_owned())
                };
                if let Some(mut picker) = this.recents_picker.take() {
                    for recent in &mut picker {
                        recent.label = label(this, recent);
                    }
                    this.recents_picker = Some(picker);
                }
                if let Some(palette) = this.palette.clone() {
                    let labels: std::collections::HashMap<PathBuf, String> = palette
                        .read(cx)
                        .recents()
                        .iter()
                        .map(|recent| (recent.path.clone(), label(this, recent)))
                        .collect();
                    palette.update(cx, |palette, cx| palette.relabel_recents(&labels, cx));
                }
                cx.notify();
            });
        });
        self.recent_titles_task = Some(task);
    }

    pub fn open_palette(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        let availability = self.availability();
        let mut available: Vec<Command> = Command::ALL
            .into_iter()
            .filter(|command| command.available(availability))
            .collect();
        available.extend(
            HistoryCommand::ALL
                .into_iter()
                .filter(|command| command.available(availability))
                .map(Command::History),
        );
        available.extend(self.editor_commands(cx));
        let runtime = self.core.runtime();
        let compact = self.chrome == Chrome::Compact;
        let recents = if compact {
            self.global_recents(self.tabs.active_path(), cx)
        } else {
            Vec::new()
        };
        if intent == Intent::Libraries {
            self.refresh_libraries(cx);
        }
        self.refresh_recent_titles(&recents, cx);
        let libraries = self.welcome.libraries.clone();
        let palette = cx.new(|cx| {
            let mut palette =
                Palette::new(intent, available, runtime, compact, recents, window, cx);
            palette.libraries = libraries;
            palette
        });
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
            // Without an editor, `sync_active` changes nothing: the focus
            // would stay on the dropped palette field and no shortcut
            // would reach the shell.
            self.focus_page(window, cx);
            cx.notify();
        }
    }

    fn run_choice(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            Choice::Command(command) => self.run_command(command, window, cx),
            Choice::File(path) => self.open_file(&path, window, cx),
            Choice::Recent(path) => self.open_elsewhere(OpenRequest::File(path), window, cx),
            Choice::Create(name) => self.create_note(&name, window, cx),
            Choice::Library(id) => self.open_library(&id, window, cx),
            Choice::LibraryAction(id, action) => self.run_library_action(&id, action, window, cx),
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
            Command::SwitchLibrary => self.open_palette(Intent::Libraries, window, cx),
            Command::CloseWorkspace => self.close_workspace(window, cx),
            Command::ToggleTheme => self.cycle_theme(cx),
            Command::ZoomIn => self.set_zoom(zoom::zoom_in(Some(self.zoom())), cx),
            Command::ZoomOut => self.set_zoom(zoom::zoom_out(Some(self.zoom())), cx),
            Command::ResetZoom => self.set_zoom(zoom::DEFAULT, cx),
            Command::Settings => self.open_settings(window, cx),
            Command::InsertImage => {
                if let super::Page::Editor(editor) = self.page(cx) {
                    editor.update(cx, |editor, cx| editor.pick_images(cx));
                }
            }
            Command::ImageWidth(preset) => {
                if let super::Page::Editor(editor) = self.page(cx) {
                    editor.update(cx, |editor, cx| editor.set_image_width_preset(preset, cx));
                }
            }
            Command::Image(action) => {
                if let super::Page::Editor(editor) = self.page(cx) {
                    editor.update(cx, |editor, cx| {
                        editor.run_image_action_at_head(action, window, cx)
                    });
                }
            }
            Command::History(command) => self.run_history_command(command, window, cx),
            Command::Editor(id) => {
                if let super::Page::Editor(editor) = self.page(cx) {
                    let clock = crate::editor::actions::clock();
                    editor.update(cx, |editor, cx| {
                        editor.update_state(cx, |state| state.run_command(id, &clock));
                    });
                }
            }
        }
    }

    /// The active note's commands: "Insert image…" (US-062), the preset
    /// widths and the actions of the block image at its caret (US-068,
    /// US-069), then the table commands that apply at its caret (US-053).
    pub(super) fn editor_commands(&self, cx: &gpui::App) -> Vec<Command> {
        let super::Page::Editor(editor) = self.page(cx) else {
            return Vec::new();
        };
        let editor = editor.read(cx);
        let state = &editor.state;
        std::iter::once(Command::InsertImage)
            .chain(
                editor
                    .image_presets()
                    .into_iter()
                    .filter(|(_, enabled)| *enabled)
                    .map(|(preset, _)| Command::ImageWidth(preset)),
            )
            .chain(
                editor
                    .image_actions()
                    .into_iter()
                    .filter(|(_, enabled)| *enabled)
                    .map(|(action, _)| Command::Image(action)),
            )
            .chain(
                cerata_markdown::table_ops::IDS
                    .iter()
                    .filter_map(|id| cerata_markdown::commands::command(id))
                    .filter(|c| c.enabled(state))
                    .map(|c| Command::Editor(c.id)),
            )
            .collect()
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

    /// The welcome screen's "Open File…": see
    /// [`super::libraries::open_picked_file`].
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
            cx.update(|cx| super::libraries::open_picked_file(cx, file, origin));
        })
        .detach();
    }

    /// "Close Workspace": the session is saved and the window shows the
    /// welcome screen, with the libraries read again.
    pub fn close_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace_root().map(Path::to_path_buf) else {
            return;
        };
        self.before_close(cx);
        self.core.clear_workspace_if_current(&root);
        self.workspace = None;
        self.saver = None;
        for path in self.docs.paths().cloned().collect::<Vec<PathBuf>>() {
            self.forget_doc(&path);
        }
        self.tabs = crate::tabs::Tabs::new();
        self.refresh_libraries(cx);
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
