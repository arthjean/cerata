use cerata_markdown::NodeKind;
use cerata_markdown::commands::command;
use gpui::{ClipboardItem, Context, Pixels, Point, SharedString, Window};

use super::Editor;
use super::actions::current_os;
use super::image_actions::ImageAction;
use crate::menu::{HasMenu, Item, Menu};

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    OpenLink(String),
    CopyLink(String),
    Command(&'static str),
    InsertImage,
    ImageWidth(super::resize::Preset, std::ops::Range<usize>),
    Image(super::image_actions::ImageAction, std::ops::Range<usize>),
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

pub fn body_items(
    has_clipboard_text: bool,
    link: Option<String>,
    state: &cerata_markdown::EditorState,
    image: Option<&super::image_actions::ImageMenu>,
) -> Vec<MenuItem> {
    let os = current_os();
    let item = |label: &str, enabled: bool, action: MenuAction| MenuItem::Action {
        label: SharedString::from(label.to_owned()),
        shortcut: None,
        enabled,
        action,
    };
    let mut items = Vec::new();
    if let Some(image) = image {
        items.push(MenuItem::Submenu {
            label: SharedString::new_static("Image width"),
            items: width_items(image),
        });
        items.extend(image.actions.iter().map(|(action, enabled)| {
            item(
                action.label(),
                *enabled,
                MenuAction::Image(*action, image.range.clone()),
            )
        }));
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

pub fn width_items(image: &super::image_actions::ImageMenu) -> Vec<MenuItem> {
    image
        .presets
        .iter()
        .map(|(preset, enabled)| MenuItem::Action {
            label: SharedString::new_static(preset.label()),
            shortcut: None,
            enabled: *enabled,
            action: MenuAction::ImageWidth(*preset, image.range.clone()),
        })
        .collect()
}

pub struct ContextMenu {
    #[cfg(test)]
    pub items: Vec<MenuItem>,
    pub popup: Menu<Editor>,
}

impl ContextMenu {
    pub fn new(position: Point<Pixels>, items: Vec<MenuItem>) -> Self {
        Self {
            popup: Menu::new(position, popup_items(&items)),
            #[cfg(test)]
            items,
        }
    }
}

fn popup_items(items: &[MenuItem]) -> Vec<Item<Editor>> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Separator => Item::Separator,
            MenuItem::Submenu { label, items } => Item::submenu(label.clone(), popup_items(items)),
            MenuItem::Action {
                label,
                shortcut,
                enabled,
                action,
            } => {
                let run = action.clone();
                let mut item =
                    Item::action(label.clone(), move |editor: &mut Editor, window, cx| {
                        editor.run_menu_action(run.clone(), window, cx)
                    })
                    .enabled(*enabled);
                if let Some(chord) = shortcut {
                    item = item.shortcut(chord.clone());
                }
                if removes(action) {
                    item = item.destructive();
                }
                item
            }
        })
        .collect()
}

fn removes(action: &MenuAction) -> bool {
    matches!(
        action,
        MenuAction::Command("table.rowDelete" | "table.columnDelete" | "table.delete")
            | MenuAction::Image(ImageAction::Delete, _)
    )
}

impl HasMenu for Editor {
    fn menu(&mut self) -> Option<&mut Menu<Self>> {
        self.menu.as_mut().map(|menu| &mut menu.popup)
    }

    fn take_menu(&mut self) -> Option<Menu<Self>> {
        self.menu.take().map(|menu| menu.popup)
    }
}

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
        self.menu = Some(ContextMenu::new(
            position,
            body_items(
                has_text,
                link_at(&self.state, pos),
                &self.state,
                self.image_menu().as_ref(),
            ),
        ));
        cx.notify();
    }

    pub fn run_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.take().is_some() {
            window.focus(&self.focus_handle, cx);
        }
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
            MenuAction::ImageWidth(preset, range) => {
                if let Some(image) = self.block_image(&range) {
                    self.set_image_width_preset_of(&image, preset, cx);
                }
            }
            MenuAction::Image(action, range) => {
                self.run_image_action(action, &range, window, cx);
            }
        }
        cx.notify();
    }
}

const OPENABLE: &[&str] = &[
    "avif", "bmp", "gif", "heic", "jpeg", "jpg", "markdown", "md", "pdf", "png", "tif", "tiff",
    "txt", "webp",
];

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
        let items = body_items(false, None, &empty, None);
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
        let with_link = body_items(true, Some("https://a.b".into()), &empty, None);
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
        let items = body_items(false, None, &state, None);
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
