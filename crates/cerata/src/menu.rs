//! Context menus of the app shell (sidebar, tabs, status bar), drawn in
//! the window like the editor's: actions, check items, separators and one
//! level of submenus. A press outside closes the menu. An open menu holds
//! the keyboard focus: arrows move, Enter chooses, Escape closes and gives
//! the focus back.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, Div, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Pixels, Point, Role, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Toggled, Window, anchored, deferred, div, prelude::FluentBuilder as _, px,
};

use crate::editor::theme;

type Run<V> = Rc<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;

pub enum Item<V> {
    Action {
        label: SharedString,
        enabled: bool,
        /// `Some` for a check item.
        checked: Option<bool>,
        run: Run<V>,
    },
    Separator,
    Submenu {
        label: SharedString,
        items: Vec<Item<V>>,
    },
}

impl<V> Item<V> {
    pub fn action(
        label: impl Into<SharedString>,
        run: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        Self::Action {
            label: label.into(),
            enabled: true,
            checked: None,
            run: Rc::new(run),
        }
    }

    pub fn check(
        label: impl Into<SharedString>,
        checked: bool,
        run: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        Self::Action {
            label: label.into(),
            enabled: true,
            checked: Some(checked),
            run: Rc::new(run),
        }
    }

    #[cfg(test)]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Action { label, .. } | Self::Submenu { label, .. } => Some(label),
            Self::Separator => None,
        }
    }
}

pub struct Menu<V> {
    pub position: Point<Pixels>,
    pub items: Vec<Item<V>>,
    pub open_submenu: Option<usize>,
    /// The item the keyboard is on, and the one inside the open submenu.
    pub cursor: Option<usize>,
    pub sub_cursor: Option<usize>,
    /// Taken on the first frame; the focus to give back on close.
    focus: Option<FocusHandle>,
    restore: Option<FocusHandle>,
}

impl<V> Menu<V> {
    pub fn new(position: Point<Pixels>, items: Vec<Item<V>>) -> Self {
        Self {
            position,
            items,
            open_submenu: None,
            cursor: None,
            sub_cursor: None,
            focus: None,
            restore: None,
        }
    }

    /// Labels in order, `-` for separators (tests compare menus with it).
    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|item| item.label().map_or_else(|| "-".to_owned(), str::to_owned))
            .collect()
    }

    /// Runs the action labeled `label`, searching submenus too.
    #[cfg(test)]
    pub fn find(&self, label: &str) -> Option<Run<V>> {
        fn walk<V>(items: &[Item<V>], label: &str) -> Option<Run<V>> {
            items.iter().find_map(|item| match item {
                Item::Action { label: l, run, .. } if l.as_ref() == label => Some(run.clone()),
                Item::Submenu { items, .. } => walk(items, label),
                _ => None,
            })
        }
        walk(&self.items, label)
    }
}

/// A view that shows one of these menus.
pub trait HasMenu: Sized + 'static {
    fn menu(&mut self) -> &mut Option<Menu<Self>>;
}

/// The next item the keyboard can land on from `from`, wrapping around;
/// separators and disabled items are skipped.
fn step<V>(items: &[Item<V>], from: Option<usize>, forward: bool) -> Option<usize> {
    let landable: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| match item {
            Item::Action { enabled, .. } => *enabled,
            Item::Submenu { .. } => true,
            Item::Separator => false,
        })
        .map(|(ix, _)| ix)
        .collect();
    let first = if forward {
        landable.first()
    } else {
        landable.last()
    };
    let Some(from) = from else {
        return first.copied();
    };
    match landable.iter().position(|ix| *ix == from) {
        Some(pos) if forward => landable.get(pos + 1).or(landable.first()).copied(),
        Some(pos) => pos
            .checked_sub(1)
            .and_then(|pos| landable.get(pos))
            .or(landable.last())
            .copied(),
        None => first.copied(),
    }
}

/// Closes the menu; `restore` gives the focus back to where it was when
/// the menu opened (keyboard close or a chosen item).
fn close<V: HasMenu>(view: &mut V, restore: bool, window: &mut Window, cx: &mut Context<V>) {
    if let Some(menu) = view.menu().take()
        && restore
        && let Some(previous) = menu.restore
    {
        window.focus(&previous, cx);
    }
    cx.notify();
}

fn choose<V: HasMenu>(view: &mut V, run: Run<V>, window: &mut Window, cx: &mut Context<V>) {
    close(view, true, window, cx);
    run(view, window, cx);
    cx.notify();
}

/// Escape closes (or leaves the submenu), arrows move, Right and Left
/// enter and leave a submenu, Enter and Space choose.
fn on_key<V: HasMenu>(view: &mut V, key: &str, window: &mut Window, cx: &mut Context<V>) {
    let Some(menu) = view.menu().as_mut() else {
        return;
    };
    let in_sub = menu.sub_cursor.is_some();
    let activate = key == "enter" || key == "space";
    let chosen: Option<Run<V>> = match key {
        "escape" | "left" if in_sub => {
            menu.sub_cursor = None;
            menu.open_submenu = None;
            None
        }
        "escape" => {
            cx.stop_propagation();
            close(view, true, window, cx);
            return;
        }
        "down" | "up" => {
            let forward = key == "down";
            let sub_items = menu.open_submenu.and_then(|ix| match menu.items.get(ix) {
                Some(Item::Submenu { items, .. }) => Some(items),
                _ => None,
            });
            match sub_items {
                Some(items) if in_sub => menu.sub_cursor = step(items, menu.sub_cursor, forward),
                _ => menu.cursor = step(&menu.items, menu.cursor, forward),
            }
            None
        }
        "right" | "enter" | "space" => {
            let top = menu
                .cursor
                .and_then(|ix| menu.items.get(ix).map(|item| (ix, item)));
            match top {
                Some((_, Item::Submenu { items, .. })) if in_sub => {
                    match menu.sub_cursor.and_then(|ix| items.get(ix)) {
                        Some(Item::Action {
                            run, enabled: true, ..
                        }) if activate => Some(run.clone()),
                        _ => None,
                    }
                }
                Some((ix, Item::Submenu { items, .. })) => {
                    let first = step(items, None, true);
                    menu.open_submenu = Some(ix);
                    menu.sub_cursor = first;
                    None
                }
                Some((
                    _,
                    Item::Action {
                        run, enabled: true, ..
                    },
                )) if activate => Some(run.clone()),
                _ => None,
            }
        }
        _ => return,
    };
    cx.stop_propagation();
    match chosen {
        Some(run) => choose(view, run, window, cx),
        None => cx.notify(),
    }
}

/// Draws `menu`; on its first frame it takes the keyboard focus.
pub fn render<V: HasMenu>(
    menu: &mut Menu<V>,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let focus = match &menu.focus {
        Some(focus) => focus.clone(),
        None => {
            let focus = cx.focus_handle();
            menu.restore = window.focused(cx);
            window.focus(&focus, cx);
            menu.focus = Some(focus.clone());
            focus
        }
    };
    let cursors = (menu.cursor, menu.sub_cursor);
    let list = list(&menu.items, menu.open_submenu, cursors, true, cx)
        .track_focus(&focus)
        .key_context("ShellMenu")
        .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
            let key = event.keystroke.key.clone();
            on_key(view, &key, window, cx);
        }));
    deferred(anchored().position(menu.position).child(list))
        .with_priority(1)
        .into_any_element()
}

fn list<V: HasMenu>(
    items: &[Item<V>],
    open: Option<usize>,
    (cursor, sub_cursor): (Option<usize>, Option<usize>),
    top: bool,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let mut list = div()
        .id(if top { "shell-menu" } else { "shell-submenu" })
        .role(Role::Menu)
        .occlude()
        .min_w(px(200.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(theme::overlay())
        .border_1()
        .border_color(theme::border())
        .shadow_md()
        .text_size(px(13.0))
        .flex()
        .flex_col();
    if top {
        list = list.on_mouse_down_out(cx.listener(|view, _, window, cx| {
            close(view, false, window, cx);
        }));
    }
    for (index, item) in items.iter().enumerate() {
        let current = cursor == Some(index);
        match item {
            Item::Separator => {
                list = list.child(div().my(px(4.0)).h(px(1.0)).bg(theme::border()));
            }
            Item::Action {
                label,
                enabled,
                checked,
                run,
            } => {
                let run = run.clone();
                let mut row = div()
                    .id(SharedString::from(format!("menu-{top}-{index}")))
                    .role(if checked.is_some() {
                        Role::MenuItemCheckBox
                    } else {
                        Role::MenuItem
                    })
                    .aria_label(label.clone())
                    .px(px(10.0))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(current, |row| row.bg(theme::panel()))
                    .child(div().w(px(12.0)).child(match checked {
                        Some(true) => "✓",
                        _ => "",
                    }))
                    .child(label.clone());
                if let Some(checked) = checked {
                    row = row.aria_toggled(if *checked {
                        Toggled::True
                    } else {
                        Toggled::False
                    });
                }
                if *enabled {
                    row = row
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::panel()))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            choose(view, run.clone(), window, cx)
                        }));
                } else {
                    row = row.text_color(theme::muted());
                }
                if top {
                    row = row.on_hover(cx.listener(|view, hovered: &bool, _, cx| {
                        if *hovered
                            && let Some(menu) = view.menu()
                            && menu.open_submenu.is_some()
                        {
                            menu.open_submenu = None;
                            menu.sub_cursor = None;
                            cx.notify();
                        }
                    }));
                }
                list = list.child(row);
            }
            Item::Submenu { label, items } => {
                let is_open = open == Some(index);
                let mut row = div()
                    .id(SharedString::from(format!("submenu-{index}")))
                    .role(Role::MenuItem)
                    .aria_label(label.clone())
                    .aria_expanded(is_open)
                    .relative()
                    .px(px(10.0))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(current, |row| row.bg(theme::panel()))
                    .hover(|s| s.bg(theme::panel()))
                    .child(div().w(px(12.0)))
                    .child(div().flex_1().child(label.clone()))
                    .child("›")
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        if *hovered && let Some(menu) = view.menu() {
                            menu.open_submenu = Some(index);
                            cx.notify();
                        }
                    }));
                if is_open {
                    row = row.child(div().absolute().top(px(-4.0)).left_full().child(self::list(
                        items,
                        None,
                        (sub_cursor, None),
                        false,
                        cx,
                    )));
                }
                list = list.child(row);
            }
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    struct View;

    fn items() -> Vec<Item<View>> {
        let mut disabled = Item::action("Paste", |_: &mut View, _, _| {});
        if let Item::Action { enabled, .. } = &mut disabled {
            *enabled = false;
        }
        vec![
            Item::action("Cut", |_: &mut View, _, _| {}),
            Item::Separator,
            disabled,
            Item::action("Copy", |_: &mut View, _, _| {}),
        ]
    }

    #[test]
    fn arrows_skip_separators_and_disabled_items_and_wrap() {
        let items = items();
        assert_eq!(step(&items, None, true), Some(0));
        assert_eq!(step(&items, Some(0), true), Some(3));
        assert_eq!(step(&items, Some(3), true), Some(0));
        assert_eq!(step(&items, None, false), Some(3));
        assert_eq!(step(&items, Some(0), false), Some(3));
    }
}
