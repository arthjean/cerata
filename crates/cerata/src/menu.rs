//! The app's menu, on coss ui's (https://coss.com/ui/docs/components/menu),
//! re-implemented from its measures over Base UI's behavior. Every context
//! menu draws with it: the sidebar's, the tabs', the status bar's and the
//! editor's.
//!
//! The popup is the select's surface (`crate::select::surface`) with 4 px
//! of padding and at least 128 px wide. Items are 28 px high with 14 px
//! text, a shortcut at their end in 12 px muted text, a destructive one in
//! the danger color, a disabled one at 64 %. A check item keeps its check
//! in a 12 px column, and the other items of its list are inset to line up
//! with it. One level of submenus opens beside its row, scrolling when it
//! is taller than the window.
//!
//! The pointer and the keyboard move the same highlight, drawn as the
//! sidebar's active row (`theme::row_active`) rather than coss ui's 4 %
//! `accent`, which measured 1.08:1 against the dark popup: too faint to
//! show the keyboard where it is. A press outside
//! closes the menu. An open menu holds the keyboard focus: arrows move,
//! Right and Left enter and leave a submenu, Enter and Space choose,
//! Escape closes and gives the focus back.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, Div, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Pixels, Point, Role, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Toggled, Window, anchored, deferred, div,
    prelude::FluentBuilder as _, px,
};

use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::select::{line, surface};

const PADDING: f32 = 4.0;
const BORDER: f32 = 1.0;
const MIN_WIDTH: f32 = 128.0;
const ITEM_HEIGHT: f32 = 28.0;
const ITEM_RADIUS: f32 = crate::input::RADIUS * 0.6;
/// What a submenu keeps clear of the window's edges.
const EDGE: f32 = 8.0;
/// Between a menu's edge and its submenu's.
const SUBMENU_GAP: f32 = 4.0;

type Run<V> = Rc<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;

pub enum Item<V> {
    Action {
        label: SharedString,
        /// The chord that runs it outside the menu, shown at the end.
        shortcut: Option<SharedString>,
        enabled: bool,
        /// Drawn in the danger color (coss ui's `destructive` variant).
        destructive: bool,
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
            shortcut: None,
            enabled: true,
            destructive: false,
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
            shortcut: None,
            enabled: true,
            destructive: false,
            checked: Some(checked),
            run: Rc::new(run),
        }
    }

    pub fn submenu(label: impl Into<SharedString>, items: Vec<Item<V>>) -> Self {
        Self::Submenu {
            label: label.into(),
            items,
        }
    }

    /// Shows `chord` at the end of an action.
    pub fn shortcut(mut self, chord: impl Into<SharedString>) -> Self {
        if let Self::Action { shortcut, .. } = &mut self {
            *shortcut = Some(chord.into());
        }
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        if let Self::Action { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }

    pub fn destructive(mut self) -> Self {
        if let Self::Action { destructive, .. } = &mut self {
            *destructive = true;
        }
        self
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
    /// The highlighted item, and the one inside the open submenu.
    pub cursor: Option<usize>,
    pub sub_cursor: Option<usize>,
    /// The pointer is over the open submenu, which lies outside the menu's
    /// bounds: a press there runs its item instead of closing the menu.
    in_submenu: bool,
    /// The open submenu's scroll: one taller than the window scrolls, and
    /// the keyboard keeps its highlight in sight.
    sub_scroll: ScrollHandle,
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
            in_submenu: false,
            sub_scroll: ScrollHandle::new(),
            focus: None,
            restore: None,
        }
    }

    /// Highlights the first item: a menu opened from the keyboard (Shift+F10,
    /// the Menu key) is ready for Enter.
    pub fn highlight_first(&mut self) {
        self.cursor = step(&self.items, None, true);
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
    fn menu(&mut self) -> Option<&mut Menu<Self>>;
    fn take_menu(&mut self) -> Option<Menu<Self>>;
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
    if let Some(menu) = view.take_menu()
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
    let Some(menu) = view.menu() else {
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
                Some(items) if in_sub => {
                    menu.sub_cursor = step(items, menu.sub_cursor, forward);
                    if let Some(ix) = menu.sub_cursor {
                        menu.sub_scroll.scroll_to_item(ix);
                    }
                }
                _ => {
                    menu.cursor = step(&menu.items, menu.cursor, forward);
                    menu.open_submenu = None;
                }
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
                    if menu.open_submenu != Some(ix) {
                        menu.sub_scroll = ScrollHandle::new();
                    }
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

/// Moves the highlight to the row under the pointer: `index` of the top
/// list, or of the open submenu when `sub`. Over a top row, the submenu
/// opens when the row is its own and closes otherwise.
fn point_at<V: HasMenu>(view: &mut V, index: usize, sub: bool, cx: &mut Context<V>) {
    let Some(menu) = view.menu() else {
        return;
    };
    if sub {
        if menu.sub_cursor != Some(index) {
            menu.sub_cursor = Some(index);
            cx.notify();
        }
        return;
    }
    let submenu = matches!(menu.items.get(index), Some(Item::Submenu { .. }));
    let open = submenu.then_some(index);
    if menu.cursor != Some(index) || menu.open_submenu != open || menu.sub_cursor.is_some() {
        if menu.open_submenu != open {
            menu.sub_scroll = ScrollHandle::new();
        }
        menu.cursor = Some(index);
        menu.sub_cursor = None;
        menu.open_submenu = open;
        menu.in_submenu = false;
        cx.notify();
    }
}

/// The pointer left row `index`: its highlight goes, unless the row opened
/// the submenu, which keeps its row highlighted.
fn point_out<V: HasMenu>(view: &mut V, index: usize, sub: bool, cx: &mut Context<V>) {
    let Some(menu) = view.menu() else {
        return;
    };
    let cursor = if sub {
        &mut menu.sub_cursor
    } else {
        &mut menu.cursor
    };
    if *cursor == Some(index) && (sub || menu.open_submenu != Some(index)) {
        *cursor = None;
        cx.notify();
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
    let state = State {
        cursor: menu.cursor,
        open: menu.open_submenu,
        sub_cursor: menu.sub_cursor,
        sub_scroll: menu.sub_scroll.clone(),
        max_height: window.viewport_size().height - px(2.0 * EDGE),
    };
    let list = list(&menu.items, &state, true, cx)
        .track_focus(&focus)
        .key_context("Menu")
        .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
            let key = event.keystroke.key.clone();
            on_key(view, &key, window, cx);
        }));
    deferred(anchored().position(menu.position).child(list))
        .with_priority(1)
        .into_any_element()
}

struct State {
    cursor: Option<usize>,
    open: Option<usize>,
    sub_cursor: Option<usize>,
    sub_scroll: ScrollHandle,
    /// The window's height less its margins: a taller submenu scrolls.
    max_height: Pixels,
}

/// The popup of `items`: the menu's own list when `top`, else its open
/// submenu's.
fn list<V: HasMenu>(
    items: &[Item<V>],
    state: &State,
    top: bool,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    // A check item anywhere in the list insets every item to its label.
    let inset = items.iter().any(|item| {
        matches!(
            item,
            Item::Action {
                checked: Some(_),
                ..
            }
        )
    });
    let mut list = surface(div())
        .id(if top { "menu" } else { "submenu" })
        .role(Role::Menu)
        .occlude()
        .min_w(px(MIN_WIDTH))
        .p(px(PADDING))
        .text_size(px(14.0))
        .text_color(theme::text())
        .flex()
        .flex_col()
        .when(!top, |list| {
            list.max_h(state.max_height)
                .overflow_y_scroll()
                .track_scroll(&state.sub_scroll)
        });
    list = if top {
        list.on_mouse_down_out(cx.listener(|view, _, window, cx| {
            if view.menu().is_some_and(|menu| !menu.in_submenu) {
                close(view, false, window, cx);
            }
        }))
    } else {
        list.on_hover(cx.listener(|view, hovered: &bool, _, _| {
            if let Some(menu) = view.menu() {
                menu.in_submenu = *hovered;
            }
        }))
    };
    let cursor = if top { state.cursor } else { state.sub_cursor };
    for (index, item) in items.iter().enumerate() {
        let highlighted = cursor == Some(index) || (top && state.open == Some(index));
        match item {
            Item::Separator => {
                list = list.child(
                    div()
                        .flex_none()
                        .mx(px(8.0))
                        .my(px(4.0))
                        .h(px(1.0))
                        .bg(line()),
                );
            }
            Item::Action {
                label,
                shortcut,
                enabled,
                destructive,
                checked,
                run,
            } => {
                let run = run.clone();
                let mut row = row(SharedString::from(format!("menu-{top}-{index}")), label)
                    .role(if checked.is_some() {
                        Role::MenuItemCheckBox
                    } else {
                        Role::MenuItem
                    })
                    .when(*destructive, |row| row.text_color(theme::danger()))
                    .when(inset, |row| {
                        row.child(div().flex_none().w(px(12.0)).flex().items_center().when(
                            matches!(checked, Some(true)),
                            |slot| {
                                slot.child(div().flex_none().ml(px(-2.0)).child(icon(
                                    Icon::Check,
                                    16.0,
                                    theme::text(),
                                )))
                            },
                        ))
                    })
                    .child(label.clone())
                    .children(shortcut.clone().map(|chord| {
                        div()
                            .ml_auto()
                            .pl(px(8.0))
                            .text_size(px(12.0))
                            .text_color(theme::muted())
                            .child(chord)
                    }));
                if let Some(checked) = checked {
                    row = row.aria_toggled(if *checked {
                        Toggled::True
                    } else {
                        Toggled::False
                    });
                }
                if *enabled {
                    row = row
                        .when(highlighted, |row| row.bg(theme::row_active()))
                        .on_mouse_move(
                            cx.listener(move |view, _, _, cx| point_at(view, index, !top, cx)),
                        )
                        .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                            if !*hovered {
                                point_out(view, index, !top, cx);
                            }
                        }))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            choose(view, run.clone(), window, cx)
                        }));
                } else {
                    row = row.opacity(0.64);
                }
                list = list.child(row);
            }
            Item::Submenu { label, items } => {
                let is_open = top && state.open == Some(index);
                let mut row = row(SharedString::from(format!("submenu-{index}")), label)
                    .debug_selector({
                        let label = label.clone();
                        move || format!("submenu-{label}")
                    })
                    .role(Role::MenuItem)
                    .aria_expanded(is_open)
                    .relative()
                    .when(highlighted, |row| row.bg(theme::row_active()))
                    .when(inset, |row| row.child(div().flex_none().w(px(12.0))))
                    .child(label.clone())
                    .child(div().ml_auto().mr(px(-2.0)).child(icon(
                        Icon::ChevronRight,
                        16.0,
                        theme::text().opacity(0.8),
                    )))
                    .on_mouse_move(
                        cx.listener(move |view, _, _, cx| point_at(view, index, false, cx)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| point_at(view, index, false, cx)));
                if is_open {
                    // Beside the menu, clear of its edge (Base UI puts it
                    // at the row's edge, over the menu's padding), its
                    // first item level with the row, and kept inside the
                    // window: the editor's Table submenu is taller than
                    // the room below it.
                    row = row.child(
                        div()
                            .absolute()
                            .left_full()
                            .pl(px(PADDING + BORDER + SUBMENU_GAP))
                            .top(px(-(PADDING + BORDER)))
                            .child(
                                anchored()
                                    .snap_to_window_with_margin(px(EDGE))
                                    .child(self::list(items, state, false, cx)),
                            ),
                    );
                }
                list = list.child(row);
            }
        }
    }
    list
}

/// An item's row: 28 px, 8 px in from the popup's padding, rounded.
fn row(id: SharedString, label: &SharedString) -> Stateful<Div> {
    let selector = label.clone();
    div()
        .id(id)
        .debug_selector(move || format!("menu-{selector}"))
        .aria_label(label.clone())
        .flex_none()
        .h(px(ITEM_HEIGHT))
        .px(px(8.0))
        .rounded(px(ITEM_RADIUS))
        .flex()
        .items_center()
        .gap(px(8.0))
        .whitespace_nowrap()
        .cursor_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct View;

    fn items() -> Vec<Item<View>> {
        vec![
            Item::action("Cut", |_: &mut View, _, _| {}),
            Item::Separator,
            Item::action("Paste", |_: &mut View, _, _| {}).enabled(false),
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

    #[test]
    fn a_menu_opened_from_the_keyboard_highlights_its_first_item() {
        let mut menu = Menu::new(Point::default(), items());
        menu.highlight_first();
        assert_eq!(menu.cursor, Some(0));
        let mut first_disabled = items();
        first_disabled.remove(0);
        first_disabled.remove(0);
        let mut menu = Menu::new(Point::default(), first_disabled);
        menu.highlight_first();
        assert_eq!(menu.cursor, Some(1), "Paste is skipped");
    }
}
