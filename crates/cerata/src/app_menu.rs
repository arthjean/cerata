//! The application menus (`src-tauri/src/lib.rs:280-334`): Cerata
//! (Preferences, Quit), Edit and Window, as macOS's native menu bar.
//! Windows and Linux draw no menu bar.

use gpui::{Action, App, KeyBinding, MenuItem, OsAction, actions};

use crate::editor::actions as editor;
use crate::shell::OpenPreferences;

actions!(app, [Quit, Minimize, ToggleFullScreen, CloseWindow]);

/// One entry of a menu.
pub enum Entry {
    Item {
        label: &'static str,
        action: Box<dyn Action>,
        os: Option<OsAction>,
    },
    Separator,
}

fn item(label: &'static str, action: impl Action) -> Entry {
    Entry::Item {
        label,
        action: Box::new(action),
        os: None,
    }
}

fn os_item(label: &'static str, action: impl Action, os: OsAction) -> Entry {
    Entry::Item {
        label,
        action: Box::new(action),
        os: Some(os),
    }
}

/// The menus, in order, with their entries.
pub fn menus() -> Vec<(&'static str, Vec<Entry>)> {
    vec![
        (
            "Cerata",
            vec![
                item("Preferences…", OpenPreferences),
                Entry::Separator,
                item("Quit Cerata", Quit),
            ],
        ),
        (
            "Edit",
            vec![
                os_item("Undo", editor::Undo, OsAction::Undo),
                os_item("Redo", editor::Redo, OsAction::Redo),
                Entry::Separator,
                os_item("Cut", editor::Cut, OsAction::Cut),
                os_item("Copy", editor::Copy, OsAction::Copy),
                os_item("Paste", editor::Paste, OsAction::Paste),
                os_item("Select All", editor::SelectAll, OsAction::SelectAll),
            ],
        ),
        (
            "Window",
            vec![
                item("Minimize", Minimize),
                item("Toggle Full Screen", ToggleFullScreen),
                item("Close Window", CloseWindow),
            ],
        ),
    ]
}

/// The native form of [`menus`].
pub fn native_menus() -> Vec<gpui::Menu> {
    menus()
        .into_iter()
        .map(|(name, entries)| {
            gpui::Menu::new(name).items(entries.into_iter().map(|entry| match entry {
                Entry::Separator => MenuItem::separator(),
                Entry::Item { label, action, os } => MenuItem::Action {
                    name: label.into(),
                    action,
                    os_action: os,
                    checked: false,
                    disabled: false,
                },
            }))
        })
        .collect()
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    if cfg!(target_os = "macos") {
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(native_menus());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menus_follow_writer() {
        let labels: Vec<(&str, Vec<&str>)> = menus()
            .iter()
            .map(|(name, entries)| {
                (
                    *name,
                    entries
                        .iter()
                        .map(|entry| match entry {
                            Entry::Item { label, .. } => *label,
                            Entry::Separator => "-",
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                ("Cerata", vec!["Preferences…", "-", "Quit Cerata"]),
                (
                    "Edit",
                    vec!["Undo", "Redo", "-", "Cut", "Copy", "Paste", "Select All"]
                ),
                (
                    "Window",
                    vec!["Minimize", "Toggle Full Screen", "Close Window"]
                ),
            ]
        );
        assert_eq!(native_menus().len(), 3);
    }
}
