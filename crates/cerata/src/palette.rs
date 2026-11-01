//! The command palette (`command-palette/index.tsx`, `use-fuzzy-search.ts`):
//! the window's commands filtered by substring, files from the index 50 ms
//! after typing stops (20 results, matched characters highlighted), the
//! global recents in a compact window, and a create-note intent.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cerata_core::index::SearchResult;
use cerata_core::workspace::WorkspaceRuntime;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription, Task, Window,
    actions, div, prelude::FluentBuilder as _, px,
};

use crate::editor::text_field::{FieldEvent, TextField};
use crate::editor::theme;

/// File search waits this long after the last keystroke.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(50);
pub const SEARCH_LIMIT: usize = 20;

actions!(palette, [SelectNext, SelectPrevious]);

pub fn bindings() -> Vec<KeyBinding> {
    let c = Some("Palette");
    vec![
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("up", SelectPrevious, c),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Search,
    CreateFile,
}

/// A command the palette offers (Writer's list, in order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ToggleSidebar,
    NewFile,
    OpenInCompactWindow,
    CloseTab,
    CloseAllTabs,
    OpenWorkspace,
    CloseWorkspace,
    ToggleTheme,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    Settings,
}

impl Command {
    pub const ALL: [Self; 12] = [
        Self::ToggleSidebar,
        Self::NewFile,
        Self::OpenInCompactWindow,
        Self::CloseTab,
        Self::CloseAllTabs,
        Self::OpenWorkspace,
        Self::CloseWorkspace,
        Self::ToggleTheme,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ResetZoom,
        Self::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "Toggle Sidebar",
            Self::NewFile => "Create New File",
            Self::OpenInCompactWindow => "Open File in Compact Window",
            Self::CloseTab => "Close Current Tab",
            Self::CloseAllTabs => "Close All Tabs",
            Self::OpenWorkspace => "Open Workspace",
            Self::CloseWorkspace => "Close Workspace",
            Self::ToggleTheme => "Toggle Dark Mode",
            Self::ZoomIn => "Zoom In",
            Self::ZoomOut => "Zoom Out",
            Self::ResetZoom => "Reset Zoom",
            Self::Settings => "Settings",
        }
    }
}

/// What the window looks like when the palette opens, for the commands'
/// availability (`index.tsx:129-236`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Availability {
    pub has_workspace: bool,
    pub compact: bool,
    pub has_active_file: bool,
    pub has_tabs: bool,
}

impl Command {
    pub fn available(self, a: Availability) -> bool {
        match self {
            Self::ToggleSidebar => a.has_workspace && !a.compact,
            Self::NewFile => a.has_workspace || (a.compact && a.has_active_file),
            Self::OpenInCompactWindow => a.has_workspace && a.has_active_file,
            Self::CloseTab | Self::CloseAllTabs => a.has_tabs && !a.compact,
            Self::CloseWorkspace => a.has_workspace,
            Self::Settings => !a.compact,
            Self::OpenWorkspace
            | Self::ToggleTheme
            | Self::ZoomIn
            | Self::ZoomOut
            | Self::ResetZoom => true,
        }
    }
}

/// Commands whose label contains `query`, ignoring case (Writer filters by
/// substring, not fuzzily).
pub fn filter_commands(available: &[Command], query: &str) -> Vec<Command> {
    let query = query.trim().to_lowercase();
    available
        .iter()
        .copied()
        .filter(|command| command.label().to_lowercase().contains(&query))
        .collect()
}

/// The note a create query names: trimmed, `.md` added unless present.
pub fn note_file_name(query: &str) -> Option<String> {
    let name = query.trim();
    if name.is_empty() {
        return None;
    }
    Some(if name.to_lowercase().ends_with(".md") {
        name.to_owned()
    } else {
        format!("{name}.md")
    })
}

/// Byte ranges of the characters at `indices` (char positions).
pub fn char_ranges(text: &str, indices: &[u32]) -> Vec<Range<usize>> {
    text.char_indices()
        .enumerate()
        .filter(|(ix, _)| indices.contains(&(*ix as u32)))
        .map(|(_, (byte, c))| byte..byte + c.len_utf8())
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Command(Command),
    File(PathBuf),
    Recent(PathBuf),
    Create(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaletteEvent {
    Chosen(Choice),
    Dismissed,
}

/// A global recent file, as the compact window shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentFile {
    pub path: PathBuf,
    pub label: String,
}

pub struct Palette {
    pub intent: Intent,
    pub field: Entity<TextField>,
    available: Vec<Command>,
    runtime: Option<Arc<WorkspaceRuntime>>,
    compact: bool,
    recents: Vec<RecentFile>,
    pub files: Vec<SearchResult>,
    pub selected: usize,
    search_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    _field_events: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle.clone()
    }
}

impl Palette {
    pub fn new(
        intent: Intent,
        available: Vec<Command>,
        runtime: Option<Arc<WorkspaceRuntime>>,
        compact: bool,
        recents: Vec<RecentFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match intent {
            Intent::Search => "Search...",
            Intent::CreateFile => "Create a new note...",
        };
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        let events =
            cx.subscribe_in(
                &field,
                window,
                |this, _, event: &FieldEvent, _, cx| match event {
                    FieldEvent::Changed => this.query_changed(cx),
                    FieldEvent::Submit { .. } => this.choose_selected(cx),
                    FieldEvent::Escape => cx.emit(PaletteEvent::Dismissed),
                },
            );
        let focus = field.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        Self {
            intent,
            field,
            available,
            runtime,
            compact,
            recents,
            files: Vec::new(),
            selected: 0,
            search_task: None,
            focus_handle: cx.focus_handle(),
            _field_events: events,
        }
    }

    pub fn query(&self, cx: &App) -> String {
        self.field.read(cx).text.clone()
    }

    pub fn indexing(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|r| !r.index_ready.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn searches_files(&self) -> bool {
        self.intent == Intent::Search && !self.compact && self.runtime.is_some()
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.selected = 0;
        self.search_task = None;
        let query = self.query(cx);
        if !self.searches_files() || query.trim().is_empty() {
            self.files.clear();
            cx.notify();
            return;
        }
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let results = cx
                .background_executor()
                .spawn(async move { runtime.index.lock().search(&query, SEARCH_LIMIT) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.files = results;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The items shown, in order: commands, files, recents, or the note
    /// to create.
    pub fn choices(&self, cx: &App) -> Vec<Choice> {
        let query = self.query(cx);
        if self.intent == Intent::CreateFile {
            return note_file_name(&query)
                .map(Choice::Create)
                .into_iter()
                .collect();
        }
        let mut choices: Vec<Choice> = if query.trim().is_empty() {
            self.available
                .iter()
                .copied()
                .map(Choice::Command)
                .collect()
        } else {
            filter_commands(&self.available, &query)
                .into_iter()
                .map(Choice::Command)
                .collect()
        };
        if !query.trim().is_empty() {
            if self.searches_files() {
                choices.extend(self.files.iter().map(|f| Choice::File(f.path.clone())));
            }
            if self.compact {
                let q = query.trim().to_lowercase();
                choices.extend(
                    self.recents
                        .iter()
                        .filter(|r| {
                            r.label.to_lowercase().contains(&q)
                                || r.path.to_string_lossy().to_lowercase().contains(&q)
                        })
                        .map(|r| Choice::Recent(r.path.clone())),
                );
            }
        }
        choices
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.choices(cx).len();
        if count == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        cx.notify();
    }

    pub fn choose(&mut self, choice: Choice, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Chosen(choice));
    }

    fn choose_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(choice) = self.choices(cx).get(self.selected).cloned() {
            self.choose(choice, cx);
        }
    }

    fn heading(&self, query: &str) -> &'static str {
        match self.intent {
            Intent::CreateFile => "Create note",
            Intent::Search if query.trim().is_empty() => "Suggested",
            Intent::Search if self.searches_files() && self.indexing() => "Results (indexing...)",
            Intent::Search => "Results",
        }
    }

    /// The text under the list when it says nothing else.
    pub fn empty_text(&self, cx: &App) -> Option<&'static str> {
        let query = self.query(cx);
        if self.intent == Intent::CreateFile && query.trim().is_empty() {
            return Some("Type a note name to create it.");
        }
        if !self.choices(cx).is_empty() {
            return None;
        }
        if self.searches_files() && self.indexing() && !query.trim().is_empty() {
            Some("Indexing workspace...")
        } else {
            Some("No results found.")
        }
    }

    /// Without a workspace the palette offers commands only, and says so.
    pub fn no_workspace_hint(&self) -> Option<&'static str> {
        (self.intent == Intent::Search && self.runtime.is_none() && !self.compact)
            .then_some("Open a folder to search its files.")
    }

    fn label_of(&self, choice: &Choice) -> (SharedString, Option<StyledText>) {
        match choice {
            Choice::Command(command) => (command.label().into(), None),
            Choice::File(path) => {
                let result = self.files.iter().find(|f| &f.path == path);
                let name =
                    result.map_or_else(|| crate::shell::file_name(path), |r| r.filename.clone());
                let detail = result.map(|r| {
                    let highlight = HighlightStyle {
                        color: Some(theme::accent()),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    };
                    let ranges = char_ranges(&r.relative_path, &r.match_indices);
                    StyledText::new(r.relative_path.clone())
                        .with_highlights(ranges.into_iter().map(|range| (range, highlight)))
                });
                (name.into(), detail)
            }
            Choice::Recent(path) => {
                let label = self
                    .recents
                    .iter()
                    .find(|r| &r.path == path)
                    .map_or_else(|| crate::shell::file_name(path), |r| r.label.clone());
                let parent = path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                (label.into(), Some(StyledText::new(parent)))
            }
            Choice::Create(name) => (format!("Create: {name}").into(), None),
        }
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);
        let choices = self.choices(cx);
        let mut list = div()
            .id("palette-list")
            .flex()
            .flex_col()
            .max_h(px(360.0))
            .overflow_y_scroll()
            .py(px(4.0));
        if !choices.is_empty() {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .child(self.heading(&query)),
            );
        }
        for (ix, choice) in choices.iter().enumerate() {
            let (label, detail) = self.label_of(choice);
            let choice = choice.clone();
            list = list.child(
                div()
                    .id(("palette-item", ix))
                    .role(gpui::Role::ListBoxOption)
                    .aria_label(label.clone())
                    .aria_selected(ix == self.selected)
                    .mx(px(4.0))
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .flex()
                    .flex_col()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme::panel()))
                    .hover(|s| s.bg(theme::panel()))
                    .child(div().text_size(px(13.0)).child(label))
                    .children(detail.map(|detail| {
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::muted())
                            .child(detail)
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(choice.clone(), cx))),
            );
        }
        if let Some(text) = self.empty_text(cx) {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(px(13.0))
                    .text_color(theme::muted())
                    .child(text),
            );
        }
        if let Some(hint) = self.no_workspace_hint() {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .child(hint),
            );
        }
        div()
            .id("palette")
            .role(gpui::Role::Dialog)
            .aria_label("Command palette")
            // Tab stays in the palette (a modal) instead of reaching the
            // controls behind it.
            .on_action(|_: &crate::a11y::FocusNext, _, cx| cx.stop_propagation())
            .on_action(|_: &crate::a11y::FocusPrevious, _, cx| cx.stop_propagation())
            .key_context("Palette")
            .track_focus(&self.focus_handle)
            .occlude()
            .w(px(560.0))
            .rounded(px(10.0))
            .bg(theme::overlay())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(self.field.clone()),
            )
            .child(list)
    }
}

/// Where a created note goes: the workspace root, else the active file's
/// folder (`createBaseDir`).
pub fn create_base(root: Option<&Path>, active: Option<&Path>) -> Option<PathBuf> {
    root.map(Path::to_path_buf)
        .or_else(|| active.and_then(Path::parent).map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(commands: &[Command]) -> Vec<&'static str> {
        commands.iter().map(|c| c.label()).collect()
    }

    #[test]
    fn the_command_list_is_writers_in_order() {
        assert_eq!(
            labels(&Command::ALL),
            [
                "Toggle Sidebar",
                "Create New File",
                "Open File in Compact Window",
                "Close Current Tab",
                "Close All Tabs",
                "Open Workspace",
                "Close Workspace",
                "Toggle Dark Mode",
                "Zoom In",
                "Zoom Out",
                "Reset Zoom",
                "Settings"
            ]
        );
    }

    #[test]
    fn commands_filter_by_case_insensitive_substring() {
        assert_eq!(
            labels(&filter_commands(&Command::ALL, "ZOOM")),
            ["Zoom In", "Zoom Out", "Reset Zoom"]
        );
        assert_eq!(
            labels(&filter_commands(&Command::ALL, " tab")),
            ["Close Current Tab", "Close All Tabs"]
        );
        assert!(filter_commands(&Command::ALL, "zz").is_empty());
    }

    #[test]
    fn availability_follows_the_window() {
        let none = Availability::default();
        let available: Vec<_> = Command::ALL
            .into_iter()
            .filter(|c| c.available(none))
            .collect();
        assert_eq!(
            labels(&available),
            [
                "Open Workspace",
                "Toggle Dark Mode",
                "Zoom In",
                "Zoom Out",
                "Reset Zoom",
                "Settings"
            ]
        );
        let compact = Availability {
            compact: true,
            has_active_file: true,
            has_tabs: true,
            ..none
        };
        assert!(Command::NewFile.available(compact));
        assert!(!Command::Settings.available(compact));
        assert!(!Command::CloseTab.available(compact));
        assert!(!Command::ToggleSidebar.available(compact));
    }

    #[test]
    fn a_create_query_names_a_markdown_file() {
        assert_eq!(note_file_name("  Ideas "), Some("Ideas.md".into()));
        assert_eq!(note_file_name("plan.MD"), Some("plan.MD".into()));
        assert_eq!(note_file_name("   "), None);
        assert_eq!(
            create_base(None, Some(Path::new("/x/a.md"))),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            create_base(Some(Path::new("/v")), Some(Path::new("/x/a.md"))),
            Some(PathBuf::from("/v"))
        );
    }

    #[test]
    fn matched_characters_map_to_byte_ranges() {
        assert_eq!(char_ranges("é/ab", &[0, 2]), [0..2, 3..4]);
    }
}
