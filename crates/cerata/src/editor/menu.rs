//! The editor's context menu, drawn in the app (`editor-context-menu.ts:20-143`,
//! `editor-body-menu.ts:10-70`): Cut, Copy, Paste in plain text, the
//! Format, Paragraph and Insert submenus from the command registry with
//! their shortcuts, a Table submenu with the caret in a drawn table
//! (US-053), Select all, and Open or Copy link over a link.

use cerata_markdown::NodeKind;
use cerata_markdown::commands::command;
use gpui::{
    AnyElement, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred, div, px,
};

use super::actions::current_os;
use super::{Editor, theme};
use crate::icons::{Icon, icon};

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    OpenLink(String),
    CopyLink(String),
    Command(&'static str),
    /// "Image…": the file picker, which a pure state command cannot open.
    InsertImage,
    /// A preset width of the image at the caret (US-068), computed on the
    /// view's column.
    ImageWidth(super::resize::Preset),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Action {
        label: SharedString,
        shortcut: Option<SharedString>,
        enabled: bool,
        action: MenuAction,
    },
    Separator,
    Submenu {
        label: SharedString,
        items: Vec<MenuItem>,
    },
}

/// `FORMAT_SUBMENU`, `PARAGRAPH_SUBMENU` and `INSERT_SUBMENU`; `None` is a
/// separator, a label overrides the registry's.
/// A command id and its label override, or `None` for a separator.
type SubmenuEntry = Option<(&'static str, Option<&'static str>)>;

const SUBMENUS: &[(&str, &[SubmenuEntry])] = &[
    (
        "Format",
        &[
            Some(("format.bold", None)),
            Some(("format.italic", None)),
            Some(("format.strikethrough", None)),
            Some(("format.code", None)),
            None,
            Some(("format.link", Some("Insert link\u{2026}"))),
            None,
            Some(("format.clearFormatting", None)),
        ],
    ),
    (
        "Paragraph",
        &[
            Some(("format.heading1", None)),
            Some(("format.heading2", None)),
            Some(("format.heading3", None)),
            Some(("format.heading4", None)),
            Some(("format.heading5", None)),
            Some(("format.heading6", None)),
            Some(("format.paragraph", None)),
            None,
            Some(("format.bulletList", None)),
            Some(("format.numberedList", None)),
            Some(("format.taskList", None)),
            None,
            Some(("format.blockquote", None)),
            Some(("format.codeBlock", None)),
        ],
    ),
    (
        "Insert",
        &[
            Some(("format.link", None)),
            Some(("insert.table", None)),
            Some(("insert.horizontalRule", None)),
            None,
            Some(("insert.today", None)),
            Some(("insert.now", None)),
        ],
    ),
];

/// The Table submenu (US-053): rows, columns, alignment, sorting, then the
/// whole table; `None` is a separator.
const TABLE_SUBMENU: &[Option<&str>] = &[
    Some("table.rowAbove"),
    Some("table.rowBelow"),
    Some("table.rowDuplicate"),
    Some("table.rowUp"),
    Some("table.rowDown"),
    Some("table.rowDelete"),
    None,
    Some("table.columnLeft"),
    Some("table.columnRight"),
    Some("table.columnDuplicate"),
    Some("table.columnMoveLeft"),
    Some("table.columnMoveRight"),
    Some("table.columnDelete"),
    None,
    Some("table.alignLeft"),
    Some("table.alignCenter"),
    Some("table.alignRight"),
    Some("table.alignNone"),
    None,
    Some("table.sortAscending"),
    Some("table.sortDescending"),
    None,
    Some("table.format"),
    Some("table.editSource"),
    None,
    Some("table.delete"),
];

/// The Table submenu for `state`, when its caret is in a drawn table: each
/// command enabled where it applies.
fn table_submenu(state: &cerata_markdown::EditorState) -> Option<MenuItem> {
    cerata_markdown::cells::grid_at(state, state.selection().main().head)?;
    let items = TABLE_SUBMENU
        .iter()
        .map(|entry| match entry.and_then(command) {
            None => MenuItem::Separator,
            Some(c) => MenuItem::Action {
                label: SharedString::new_static(c.label),
                shortcut: None,
                enabled: c.enabled(state),
                action: MenuAction::Command(c.id),
            },
        })
        .collect();
    Some(MenuItem::Submenu {
        label: SharedString::new_static("Table"),
        items,
    })
}

/// The body menu's items. `link` is the destination under the pointer;
/// `state` gives the Table submenu, `presets` the Image width submenu of the
/// block image at the caret (US-068), each preset with whether it applies.
pub fn body_items(
    has_clipboard_text: bool,
    link: Option<String>,
    state: &cerata_markdown::EditorState,
    presets: &[(super::resize::Preset, bool)],
) -> Vec<MenuItem> {
    let os = current_os();
    let item = |label: &str, enabled: bool, action: MenuAction| MenuItem::Action {
        label: SharedString::from(label.to_owned()),
        shortcut: None,
        enabled,
        action,
    };
    let mut items = Vec::new();
    if !presets.is_empty() {
        items.push(MenuItem::Submenu {
            label: SharedString::new_static("Image width"),
            items: presets
                .iter()
                .map(|(preset, enabled)| {
                    item(preset.label(), *enabled, MenuAction::ImageWidth(*preset))
                })
                .collect(),
        });
        items.push(MenuItem::Separator);
    }
    items.extend([
        item("Cut", true, MenuAction::Cut),
        item("Copy", true, MenuAction::Copy),
        item("Paste", has_clipboard_text, MenuAction::Paste),
        MenuItem::Separator,
    ]);
    for (label, layout) in SUBMENUS {
        let mut sub = layout
            .iter()
            .map(|entry| match entry {
                None => MenuItem::Separator,
                Some((id, label)) => {
                    let registered = command(id);
                    MenuItem::Action {
                        label: SharedString::from(
                            label
                                .or(registered.map(|c| c.label))
                                .unwrap_or(id)
                                .to_owned(),
                        ),
                        shortcut: registered
                            .and_then(|c| c.shortcut(os))
                            .map(|chord| SharedString::from(chord.label())),
                        enabled: registered.is_some(),
                        action: MenuAction::Command(id),
                    }
                }
            })
            .collect::<Vec<_>>();
        if *label == "Insert" {
            // After the block insertions, before the dates.
            let at = sub
                .iter()
                .position(|item| *item == MenuItem::Separator)
                .unwrap_or(sub.len());
            sub.insert(at, item("Image\u{2026}", true, MenuAction::InsertImage));
        }
        items.push(MenuItem::Submenu {
            label: SharedString::new_static(label),
            items: sub,
        });
    }
    items.extend(table_submenu(state));
    items.push(MenuItem::Separator);
    items.push(item("Select all", true, MenuAction::SelectAll));
    if let Some(link) = link {
        items.push(MenuItem::Separator);
        items.push(item("Open link", true, MenuAction::OpenLink(link.clone())));
        items.push(item("Copy link", true, MenuAction::CopyLink(link)));
    }
    items
}

#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub position: Point<Pixels>,
    pub items: Vec<MenuItem>,
    pub open_submenu: Option<usize>,
    /// The pointer is over the open submenu, which lies outside the menu's
    /// bounds: a press there runs its item instead of closing the menu.
    pub in_submenu: bool,
}

/// The destination of the link at `pos`: a link's URL, an autolink or a
/// bare URL (`link-navigation.ts:67-93`).
pub fn link_at(state: &cerata_markdown::EditorState, pos: usize) -> Option<String> {
    let tree = state.tree();
    let node = tree.resolve(pos, 0);
    let text = state.text();
    for n in std::iter::once(node).chain(node.ancestors()) {
        match n.kind() {
            NodeKind::Link | NodeKind::Autolink => {
                let url = n.children().find(|c| c.kind() == NodeKind::Url)?;
                return Some(text.slice(url.range()).to_owned());
            }
            NodeKind::Url => return Some(text.slice(n.range()).to_owned()),
            _ => {}
        }
    }
    None
}

impl Editor {
    pub(super) fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        pos: usize,
        cx: &mut Context<Self>,
    ) {
        let has_text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| !text.is_empty());
        self.menu = Some(ContextMenu {
            position,
            items: body_items(
                has_text,
                link_at(&self.state, pos),
                &self.state,
                &self.image_presets(),
            ),
            open_submenu: None,
            in_submenu: false,
        });
        cx.notify();
    }

    pub fn run_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        match action {
            MenuAction::Cut => self.cut(cx),
            MenuAction::Copy => self.copy(cx),
            MenuAction::Paste => {
                if let Some(item) = cx.read_from_clipboard()
                    && let Some(text) = item.text()
                    && !self.paste_cells(&text, item.metadata().map(String::as_str), cx)
                {
                    self.paste_text(&text, cx);
                }
            }
            MenuAction::SelectAll => {
                self.update_state(cx, |state| state.select_all());
            }
            MenuAction::OpenLink(link) => self.follow_href(&link, window, cx),
            MenuAction::CopyLink(link) => cx.write_to_clipboard(ClipboardItem::new_string(link)),
            MenuAction::Command(id) => {
                let clock = super::actions::clock();
                self.update_state(cx, |state| state.run_command(id, &clock));
            }
            MenuAction::InsertImage => self.pick_images(cx),
            MenuAction::ImageWidth(preset) => {
                self.set_image_width_preset(preset, cx);
            }
        }
        cx.notify();
    }
}

impl ContextMenu {
    pub fn render(&self, _window: &mut Window, cx: &mut Context<Editor>) -> AnyElement {
        let list = menu_list(&self.items, self.open_submenu, true, cx);
        deferred(anchored().position(self.position).child(list))
            .with_priority(1)
            .into_any_element()
    }
}

fn menu_list(
    items: &[MenuItem],
    open: Option<usize>,
    top: bool,
    cx: &mut Context<Editor>,
) -> AnyElement {
    let mut list = div()
        .id(if top {
            "context-menu"
        } else {
            "context-submenu"
        })
        .occlude()
        .min_w(px(220.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(theme::overlay())
        .border_1()
        .border_color(theme::border())
        .shadow_md()
        .text_size(px(13.0))
        .flex()
        .flex_col();
    list = if top {
        list.on_mouse_down_out(cx.listener(|editor, _, _, cx| {
            if editor.menu.as_ref().is_some_and(|menu| !menu.in_submenu) {
                editor.menu = None;
                cx.notify();
            }
        }))
    } else {
        list.on_hover(cx.listener(|editor, hovered: &bool, _, _| {
            if let Some(menu) = &mut editor.menu {
                menu.in_submenu = *hovered;
            }
        }))
    };
    for (index, item) in items.iter().enumerate() {
        match item {
            MenuItem::Separator => {
                list = list.child(div().my(px(4.0)).h(px(1.0)).bg(theme::border()));
            }
            MenuItem::Action {
                label,
                shortcut,
                enabled,
                action,
            } => {
                let action = action.clone();
                let enabled = *enabled;
                let mut row = div()
                    .id(SharedString::from(format!("menu-{}", label)))
                    .debug_selector({
                        let label = label.clone();
                        move || format!("menu-{label}")
                    })
                    .px(px(10.0))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(label.clone())
                    .children(
                        shortcut
                            .clone()
                            .map(|s| div().text_color(theme::muted()).child(s)),
                    );
                if enabled {
                    row = row
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::panel()))
                        .on_click(cx.listener(move |editor, _, window, cx| {
                            editor.run_menu_action(action.clone(), window, cx)
                        }));
                } else {
                    row = row.text_color(theme::muted());
                }
                if top {
                    row = row.on_hover(cx.listener(|editor, hovered: &bool, _, cx| {
                        if *hovered
                            && let Some(menu) = &mut editor.menu
                            && menu.open_submenu.is_some()
                        {
                            menu.open_submenu = None;
                            menu.in_submenu = false;
                            cx.notify();
                        }
                    }));
                }
                list = list.child(row);
            }
            MenuItem::Submenu { label, items } => {
                let mut row = div()
                    .id(SharedString::from(format!("submenu-{label}")))
                    .debug_selector({
                        let label = label.clone();
                        move || format!("submenu-{label}")
                    })
                    .relative()
                    .px(px(10.0))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .hover(|s| s.bg(theme::panel()))
                    .child(label.clone())
                    .child(icon(Icon::ChevronRight, 12.0, theme::muted()))
                    .on_hover(cx.listener(move |editor, hovered: &bool, _, cx| {
                        if *hovered
                            && let Some(menu) = &mut editor.menu
                            && menu.open_submenu != Some(index)
                        {
                            menu.open_submenu = Some(index);
                            menu.in_submenu = false;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |editor, _, _, cx| {
                        if let Some(menu) = &mut editor.menu
                            && menu.open_submenu != Some(index)
                        {
                            menu.open_submenu = Some(index);
                            menu.in_submenu = false;
                            cx.notify();
                        }
                    }));
                if open == Some(index) {
                    // Kept inside the window: the Table submenu is taller
                    // than the space below its row near the bottom (US-053).
                    row = row.child(
                        div().absolute().left(px(216.0)).top(px(-4.0)).child(
                            anchored()
                                .snap_to_window_with_margin(px(8.0))
                                .child(menu_list(items, None, false, cx)),
                        ),
                    );
                }
                list = list.child(row);
            }
        }
    }
    list.into_any_element()
}

/// Documents the OS shows rather than runs. A note is untrusted content:
/// every local file a link names opens after a confirmation that shows its
/// path (US-033), a stronger one for anything else.
const OPENABLE: &[&str] = &[
    "avif", "bmp", "gif", "heic", "jpeg", "jpg", "markdown", "md", "pdf", "png", "tif", "tiff",
    "txt", "webp",
];

/// The existing folder or document a local link opens, checked under its
/// own name and through symlinks. A folder with an extension is refused:
/// macOS runs bundles (`.app`, `.prefPane`, `.workflow`...) rather than
/// showing them.
pub(super) fn openable(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let real = std::fs::canonicalize(path).ok()?;
    if real.is_dir() {
        let plain = |p: &std::path::Path| p.extension().is_none();
        return (plain(path) && plain(&real)).then(|| path.to_path_buf());
    }
    let shown = |p: &std::path::Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| OPENABLE.contains(&e.to_ascii_lowercase().as_str()))
    };
    (real.is_file() && shown(path) && shown(&real)).then(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_menu_follows_writers_layout() {
        let empty = cerata_markdown::EditorState::new("");
        let items = body_items(false, None, &empty, &[]);
        let labels: Vec<String> = items
            .iter()
            .map(|i| match i {
                MenuItem::Action { label, .. } | MenuItem::Submenu { label, .. } => {
                    label.to_string()
                }
                MenuItem::Separator => "---".to_owned(),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Cut",
                "Copy",
                "Paste",
                "---",
                "Format",
                "Paragraph",
                "Insert",
                "---",
                "Select all"
            ]
        );
        assert!(
            matches!(&items[2], MenuItem::Action { enabled: false, .. }),
            "empty clipboard"
        );
        let MenuItem::Submenu { items: format, .. } = &items[4] else {
            panic!("Format")
        };
        assert!(
            format
                .iter()
                .all(|i| !matches!(i, MenuItem::Action { enabled: false, .. }))
        );
        let bold = format.iter().find_map(|i| match i {
            MenuItem::Action {
                label, shortcut, ..
            } if label.as_ref() == "Bold" => shortcut.clone(),
            _ => None,
        });
        let expected = if cfg!(target_os = "macos") {
            "Cmd+B"
        } else {
            "Ctrl+B"
        };
        assert_eq!(bold.as_deref(), Some(expected));
        let with_link = body_items(true, Some("https://a.b".into()), &empty, &[]);
        assert!(with_link.iter().any(|i| matches!(
            i,
            MenuItem::Action {
                action: MenuAction::OpenLink(_),
                ..
            }
        )));
        assert!(matches!(
            &with_link[2],
            MenuItem::Action { enabled: true, .. }
        ));
    }

    #[test]
    fn local_links_never_open_something_the_os_runs() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "doc.pdf",
            "Note.MD",
            "notes",
            "Tool.app",
            "Prefs.prefPane",
            "tool.EXE",
            "run.bat",
            "short.lnk",
            "s.ps1",
            "help.chm",
            "app.appinstaller",
            "disk.iso",
            "macro.docm",
            "noext",
        ] {
            let path = dir.path().join(name);
            if ["notes", "Tool.app", "Prefs.prefPane"].contains(&name) {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"x").unwrap();
            }
        }
        for name in ["doc.pdf", "Note.MD", "notes"] {
            assert!(openable(&dir.path().join(name)).is_some(), "{name}");
        }
        for name in [
            "Tool.app",
            "Prefs.prefPane",
            "tool.EXE",
            "run.bat",
            "short.lnk",
            "s.ps1",
            "help.chm",
            "app.appinstaller",
            "disk.iso",
            "macro.docm",
            "noext",
            "missing.pdf",
        ] {
            assert!(openable(&dir.path().join(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn links_under_the_pointer() {
        let state =
            cerata_markdown::EditorState::new("see [site](https://a.b) and <https://c.d> end");
        assert_eq!(link_at(&state, 6).as_deref(), Some("https://a.b"));
        assert_eq!(link_at(&state, 30).as_deref(), Some("https://c.d"));
        assert_eq!(link_at(&state, 1), None);
    }

    fn table_items(doc: &str, at: &str) -> Vec<(String, bool)> {
        let mut state = cerata_markdown::EditorState::new(doc);
        let pos = doc.find(at).unwrap();
        state
            .select(cerata_markdown::Selection::cursor(pos))
            .unwrap();
        let items = body_items(false, None, &state, &[]);
        let Some(MenuItem::Submenu { items: table, .. }) = items
            .iter()
            .find(|i| matches!(i, MenuItem::Submenu { label, .. } if label.as_ref() == "Table"))
        else {
            return Vec::new();
        };
        table
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action { label, enabled, .. } => Some((label.to_string(), *enabled)),
                _ => None,
            })
            .collect()
    }

    /// US-053: the Table submenu shows with the caret in a drawn table,
    /// its row commands off in the header, column deletion off in a lone
    /// column while table deletion stays.
    #[test]
    fn the_table_submenu_follows_the_caret() {
        let doc = "x\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        assert!(table_items(doc, "x").is_empty(), "outside a table");
        let enabled = |items: &[(String, bool)], label: &str| {
            items
                .iter()
                .find(|(l, _)| l == label)
                .unwrap_or_else(|| panic!("{label}"))
                .1
        };
        let header = table_items(doc, "a |");
        assert_eq!(header.len(), cerata_markdown::table_ops::IDS.len());
        for label in ["Delete row", "Move row up", "Move row down"] {
            assert!(!enabled(&header, label), "{label}");
        }
        assert!(enabled(&header, "Insert row above"));
        let first = table_items(doc, "1 |");
        assert!(!enabled(&first, "Move row up"));
        assert!(enabled(&first, "Move row down"));
        let last = table_items(doc, "3 |");
        assert!(!enabled(&last, "Move row down"));
        let one = table_items("| a |\n|---|\n| 1 |\n", "1 |");
        assert!(!enabled(&one, "Delete column"));
        assert!(enabled(&one, "Delete table"));
    }
}
