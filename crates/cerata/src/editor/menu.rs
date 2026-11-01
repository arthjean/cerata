//! The editor's context menu, drawn in the app (`editor-context-menu.ts:20-143`,
//! `editor-body-menu.ts:10-70`): Cut, Copy, Paste in plain text, the
//! Format, Paragraph and Insert submenus from the command registry with
//! their shortcuts, Select all, and Open or Copy link over a link.

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

/// The body menu's items. `link` is the destination under the pointer.
pub fn body_items(has_clipboard_text: bool, link: Option<String>) -> Vec<MenuItem> {
    let os = current_os();
    let item = |label: &str, enabled: bool, action: MenuAction| MenuItem::Action {
        label: SharedString::from(label.to_owned()),
        shortcut: None,
        enabled,
        action,
    };
    let mut items = vec![
        item("Cut", true, MenuAction::Cut),
        item("Copy", true, MenuAction::Copy),
        item("Paste", has_clipboard_text, MenuAction::Paste),
        MenuItem::Separator,
    ];
    for (label, layout) in SUBMENUS {
        let sub = layout
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
            .collect();
        items.push(MenuItem::Submenu {
            label: SharedString::new_static(label),
            items: sub,
        });
    }
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
            items: body_items(has_text, link_at(&self.state, pos)),
            open_submenu: None,
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
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
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
        .flex_col()
        .on_mouse_down_out(cx.listener(|editor, _, _, cx| {
            editor.menu = None;
            cx.notify();
        }));
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
                            cx.notify();
                        }
                    }));
                }
                list = list.child(row);
            }
            MenuItem::Submenu { label, items } => {
                let mut row = div()
                    .id(SharedString::from(format!("submenu-{label}")))
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
                        if *hovered && let Some(menu) = &mut editor.menu {
                            menu.open_submenu = Some(index);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |editor, _, _, cx| {
                        if let Some(menu) = &mut editor.menu {
                            menu.open_submenu = Some(index);
                            cx.notify();
                        }
                    }));
                if open == Some(index) {
                    row = row.child(
                        div()
                            .absolute()
                            .left(px(216.0))
                            .top(px(-4.0))
                            .child(menu_list(items, None, false, cx)),
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
        let items = body_items(false, None);
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
        let with_link = body_items(true, Some("https://a.b".into()));
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
}
