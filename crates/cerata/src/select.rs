//! A select on coss ui's (https://coss.com/ui/docs/components/select),
//! re-implemented from its measures and behavior over Base UI: a 32 px
//! trigger with an up-down chevron; a popup that puts the chosen option
//! over the trigger, its text on the trigger's text; 28 px options with the
//! check in a 16 px column; scroll arrows at the ends of a long list.
//!
//! The open popup holds the keyboard focus: Up, Down, Home and End move,
//! letters jump to the next option that starts with them, Enter and Space
//! choose, Escape and Tab close. A press outside closes it.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, Div, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Role, ScrollHandle,
    SharedString, Size, Stateful, StatefulInteractiveElement, Styled, Window, anchored, deferred,
    div, linear_color_stop, linear_gradient, point, prelude::FluentBuilder as _, px,
};

use crate::editor::theme;
use crate::icons::{Icon, icon};

const ITEM_HEIGHT: f32 = 28.0;
const PADDING: f32 = 4.0;
const BORDER: f32 = 1.0;
const RADIUS: f32 = crate::input::RADIUS;
/// What the popup keeps clear of the window's edges.
const EDGE: f32 = 8.0;
/// The option's text starts 37 px into the popup (border 1, padding 4,
/// option padding 8, check column 16, gap 8), the trigger's 12 px into
/// it: the popup starts 25 px left of the trigger to line them up.
const TEXT_SHIFT: f32 = 25.0;
/// Pixels an arrow scrolls per frame while the pointer is on it.
const ARROW_SPEED: f32 = 4.0;
const TYPEAHEAD_RESET: Duration = Duration::from_millis(700);

/// coss ui's `accent`, the highlighted option: the text at 4 %.
fn highlight() -> Hsla {
    theme::text().opacity(0.04)
}

/// The trigger around `value`, `width` wide, focused through `focus`; the
/// caller opens the popup. Keyboard focus draws the input's ring.
pub fn trigger(
    el: Stateful<Div>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    width: f32,
    open: bool,
    focus: &FocusHandle,
    window: &Window,
) -> Stateful<Div> {
    let ring = focus.is_focused(window) && window.last_input_was_keyboard();
    crate::a11y::control(el, Role::ComboBox, label)
        .track_focus(focus)
        // The ring below replaces the shell's focus border.
        .focus_visible(|s| s)
        .aria_expanded(open)
        .relative()
        .flex_none()
        .w(px(width))
        .h(px(crate::input::HEIGHT))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .px(px(crate::input::INSET))
        .rounded(px(RADIUS))
        .border_color(if ring {
            crate::input::ring_color()
        } else {
            crate::input::border()
        })
        .bg(crate::input::background())
        .when(!ring, |d| d.shadow(vec![crate::input::rest_shadow()]))
        .when(ring, |d| {
            let color = crate::input::ring_color();
            d.child(crate::input::halo(
                4.0,
                3.0,
                RADIUS,
                color.opacity(color.a * 0.24),
            ))
        })
        .text_size(px(14.0))
        .text_color(theme::text())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(value.into()),
        )
        .child(icon(Icon::UnfoldMore, 16.0, theme::text().opacity(0.8)).mr(px(-4.0)))
}

/// Where the popup goes: its top-left corner, its height, and the scroll
/// that puts the chosen option over the trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub origin: Point<Pixels>,
    pub height: f32,
    pub scroll: f32,
}

/// Base UI's `alignItemWithTrigger`: the chosen option (the first when
/// none is) lands on the trigger. The popup is cut where it would leave
/// the window, and a cut above is scrolled away so the option stays on the
/// trigger. With less than three options' room left, it takes the whole
/// height and keeps the option in view instead.
pub fn place(
    anchor: Bounds<Pixels>,
    viewport: Size<Pixels>,
    count: usize,
    selected: Option<usize>,
) -> Placement {
    let content = count as f32 * ITEM_HEIGHT + 2.0 * (PADDING + BORDER);
    let window = f32::from(viewport.height);
    let room = (window - 2.0 * EDGE).max(ITEM_HEIGHT);
    let least = (3.0 * ITEM_HEIGHT + 2.0 * (PADDING + BORDER)).min(content);
    let target = f32::from(anchor.top()) + (f32::from(anchor.size.height) - ITEM_HEIGHT) / 2.0;
    let row = BORDER + PADDING + selected.unwrap_or(0) as f32 * ITEM_HEIGHT;
    let ideal = target - row;
    let (mut top, mut height, mut scroll) = if ideal >= EDGE {
        (ideal, content.min(window - EDGE - ideal), 0.0)
    } else {
        let cut = EDGE - ideal;
        (EDGE, (content - cut).min(room), cut)
    };
    if height < least {
        height = content.min(room);
        top = if ideal >= EDGE {
            window - EDGE - height
        } else {
            EDGE
        };
        scroll = (top + row - target).clamp(0.0, content - height);
    }
    let left = (f32::from(anchor.left()) - TEXT_SHIFT).max(EDGE);
    Placement {
        origin: point(px(left), px(top)),
        height,
        scroll,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrow {
    Up,
    Down,
}

/// An open popup: whose it is (`key`), its options and the trigger's
/// bounds.
pub struct SelectPopup {
    pub key: String,
    pub options: Vec<SharedString>,
    pub selected: Option<usize>,
    pub highlighted: Option<usize>,
    pub anchor: Bounds<Pixels>,
    placement: Option<Placement>,
    scroll: ScrollHandle,
    arrow: Option<Arrow>,
    typed: String,
    typed_at: Option<Instant>,
    /// Taken on the first frame; the focus to give back on close.
    focus: Option<FocusHandle>,
    restore: Option<FocusHandle>,
}

impl SelectPopup {
    pub fn new(
        key: impl Into<String>,
        options: Vec<SharedString>,
        selected: Option<usize>,
        anchor: Bounds<Pixels>,
    ) -> Self {
        Self {
            key: key.into(),
            options,
            selected,
            highlighted: selected,
            anchor,
            placement: None,
            scroll: ScrollHandle::new(),
            arrow: None,
            typed: String::new(),
            typed_at: None,
            focus: None,
            restore: None,
        }
    }

    /// The highlight `step` options on, kept in the list (no wrap).
    fn step(&mut self, step: isize) {
        let last = self.options.len().saturating_sub(1);
        let next = match self.highlighted {
            Some(ix) => ix.saturating_add_signed(step).min(last),
            None if step < 0 => last,
            None => 0,
        };
        self.highlight(next);
    }

    fn highlight(&mut self, ix: usize) {
        if ix < self.options.len() {
            self.highlighted = Some(ix);
            self.scroll.scroll_to_item(ix);
        }
    }

    /// Typeahead: the keys typed within 700 ms of each other make a prefix,
    /// matched from the highlighted option on (a repeated letter cycles).
    pub fn type_char(&mut self, ch: &str, now: Instant) {
        if self
            .typed_at
            .is_none_or(|at| now.duration_since(at) > TYPEAHEAD_RESET)
        {
            self.typed.clear();
        }
        self.typed_at = Some(now);
        self.typed.push_str(&ch.to_lowercase());
        let repeated = self.typed.chars().all(|c| self.typed.starts_with(c));
        let prefix = if repeated {
            self.typed.chars().take(1).collect::<String>()
        } else {
            self.typed.clone()
        };
        let count = self.options.len();
        let from = self.highlighted.unwrap_or(0);
        // A new prefix may match the highlighted option itself; a repeated
        // letter moves past it.
        let skip = usize::from(repeated && self.highlighted.is_some());
        let found = (0..count)
            .map(|offset| (from + skip + offset) % count.max(1))
            .find(|ix| self.options[*ix].to_lowercase().starts_with(&prefix));
        if let Some(ix) = found {
            self.highlight(ix);
        }
    }
}

/// A view that shows one of these popups.
pub trait HasSelect: Sized + 'static {
    fn select(&mut self) -> &mut Option<SelectPopup>;
    /// `option` was chosen in the popup opened for `key`.
    fn choose(&mut self, key: &str, option: &str, window: &mut Window, cx: &mut Context<Self>);
}

/// Closes the popup; `restore` gives the focus back to where it was when
/// it opened (the trigger).
pub fn close<V: HasSelect>(view: &mut V, restore: bool, window: &mut Window, cx: &mut Context<V>) {
    if let Some(popup) = view.select().take()
        && restore
        && let Some(previous) = popup.restore
    {
        window.focus(&previous, cx);
    }
    cx.notify();
}

fn choose<V: HasSelect>(view: &mut V, ix: usize, window: &mut Window, cx: &mut Context<V>) {
    let Some((key, option)) = view.select().as_ref().and_then(|popup| {
        let option = popup.options.get(ix)?;
        Some((popup.key.clone(), option.clone()))
    }) else {
        return;
    };
    close(view, true, window, cx);
    view.choose(&key, &option, window, cx);
}

fn on_key<V: HasSelect>(
    view: &mut V,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let Some(popup) = view.select().as_mut() else {
        return;
    };
    let page = ((popup.placement.map_or(0.0, |p| p.height) / ITEM_HEIGHT) as isize - 1).max(1);
    let keystroke = &event.keystroke;
    match keystroke.key.as_str() {
        "down" => popup.step(1),
        "up" => popup.step(-1),
        "pagedown" => popup.step(page),
        "pageup" => popup.step(-page),
        "home" => popup.highlight(0),
        "end" => popup.highlight(popup.options.len().saturating_sub(1)),
        "enter" | "space" if popup.highlighted.is_some() => {
            let ix = popup.highlighted.unwrap_or(0);
            cx.stop_propagation();
            choose(view, ix, window, cx);
            return;
        }
        "escape" | "tab" => {
            cx.stop_propagation();
            close(view, true, window, cx);
            return;
        }
        _ => match keystroke.key_char.as_deref() {
            Some(ch)
                if !keystroke.modifiers.control
                    && !keystroke.modifiers.platform
                    && ch.chars().all(|c| !c.is_control()) =>
            {
                popup.type_char(ch, Instant::now());
            }
            _ => return,
        },
    }
    cx.stop_propagation();
    cx.notify();
}

fn arrow<V: HasSelect>(direction: Arrow, cx: &mut Context<V>) -> AnyElement {
    let surface = theme::overlay();
    let up = direction == Arrow::Up;
    div()
        .id(if up {
            "select-scroll-up"
        } else {
            "select-scroll-down"
        })
        .absolute()
        .left_0()
        .right_0()
        .when(up, |d| d.top_0().rounded_t(px(RADIUS - BORDER)))
        .when(!up, |d| d.bottom_0().rounded_b(px(RADIUS - BORDER)))
        .h(px(40.0))
        .flex()
        .justify_center()
        .when(up, |d| d.items_start())
        .when(!up, |d| d.items_end())
        // Solid on the arrow's 24 px, then fading over the options.
        .bg(linear_gradient(
            if up { 180.0 } else { 0.0 },
            linear_color_stop(surface, 0.6),
            linear_color_stop(surface.opacity(0.0), 1.0),
        ))
        .child(div().h(px(24.0)).flex().items_center().child(icon(
            if up {
                Icon::ChevronUp
            } else {
                Icon::ChevronDown
            },
            16.0,
            theme::text(),
        )))
        .on_hover(cx.listener(move |view: &mut V, hovered: &bool, _, cx| {
            if let Some(popup) = view.select().as_mut() {
                popup.arrow = hovered.then_some(direction);
                cx.notify();
            }
        }))
        .into_any_element()
}

/// Draws `popup`; on its first frame it places itself and takes the
/// keyboard focus.
pub fn render<V: HasSelect>(
    popup: &mut SelectPopup,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let focus = match &popup.focus {
        Some(focus) => focus.clone(),
        None => {
            let focus = cx.focus_handle();
            popup.restore = window.focused(cx);
            window.focus(&focus, cx);
            popup.focus = Some(focus.clone());
            focus
        }
    };
    let placement = match popup.placement {
        Some(placement) => placement,
        None => {
            let placement = place(
                popup.anchor,
                window.viewport_size(),
                popup.options.len(),
                popup.selected,
            );
            popup
                .scroll
                .set_offset(point(px(0.0), px(-placement.scroll)));
            popup.placement = Some(placement);
            // The scroll arrows need the list's laid-out height.
            window.request_animation_frame();
            placement
        }
    };

    let offset = f32::from(popup.scroll.offset().y);
    let max = f32::from(popup.scroll.max_offset().y);
    if let Some(direction) = popup.arrow {
        let next = match direction {
            Arrow::Up => (offset + ARROW_SPEED).min(0.0),
            Arrow::Down => (offset - ARROW_SPEED).max(-max),
        };
        popup.scroll.set_offset(point(px(0.0), px(next)));
        if next == 0.0 || next == -max {
            popup.arrow = None;
        } else {
            window.request_animation_frame();
        }
    }
    let can_up = offset < -0.5;
    let can_down = -offset < max - 0.5;

    let mut list = div()
        .id("select-list")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&popup.scroll)
        .p(px(PADDING))
        .flex()
        .flex_col();
    for (ix, label) in popup.options.iter().enumerate() {
        let selected = popup.selected == Some(ix);
        let highlighted = popup.highlighted == Some(ix);
        list = list.child(
            div()
                .id(("select-option", ix))
                .role(Role::ListBoxOption)
                .aria_label(label.clone())
                .aria_selected(selected)
                .flex_none()
                .h(px(ITEM_HEIGHT))
                .flex()
                .items_center()
                .gap(px(8.0))
                .pl(px(8.0))
                .pr(px(16.0))
                .rounded(px(RADIUS * 0.6))
                .when(highlighted, |d| d.bg(highlight()))
                .child(div().flex_none().w(px(16.0)).when(selected, |d| {
                    d.child(icon(Icon::Check, 16.0, theme::text()))
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(label.clone()),
                )
                .on_mouse_move(cx.listener(move |view: &mut V, _, _, cx| {
                    if let Some(popup) = view.select().as_mut()
                        && popup.highlighted != Some(ix)
                    {
                        popup.highlighted = Some(ix);
                        cx.notify();
                    }
                }))
                .on_click(
                    cx.listener(move |view: &mut V, _, window, cx| choose(view, ix, window, cx)),
                ),
        );
    }

    let anchor = popup.anchor;
    let surface = div()
        .id("select-popup")
        .role(Role::ListBox)
        .occlude()
        .track_focus(&focus)
        .key_context("SelectPopup")
        .on_key_down(
            cx.listener(|view: &mut V, event: &KeyDownEvent, window, cx| {
                on_key(view, event, window, cx)
            }),
        )
        // A press on the trigger is the trigger's: it closes the popup.
        .on_mouse_down_out(
            cx.listener(move |view: &mut V, event: &MouseDownEvent, window, cx| {
                if !anchor.contains(&event.position) {
                    close(view, false, window, cx);
                }
            }),
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .relative()
        .min_w(anchor.size.width + px(2.0 * (PADDING + BORDER) + 20.0))
        .max_w(px(420.0))
        .h(px(placement.height))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(theme::text().opacity(if theme::palette().dark { 0.06 } else { 0.08 }))
        .bg(theme::overlay())
        .shadow(vec![
            crate::input::shadow(10.0, 15.0, -3.0, 0.05),
            crate::input::shadow(4.0, 6.0, -4.0, 0.05),
        ])
        .text_size(px(14.0))
        .text_color(theme::text())
        .child(list)
        .when(can_up, |d| d.child(arrow(Arrow::Up, cx)))
        .when(can_down, |d| d.child(arrow(Arrow::Down, cx)));

    deferred(
        anchored()
            .position(placement.origin)
            .snap_to_window_with_margin(px(EDGE))
            .child(surface),
    )
    .with_priority(1)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn anchor(top: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(400.0), px(top)), size(px(176.0), px(32.0)))
    }

    fn window() -> Size<Pixels> {
        size(px(1200.0), px(800.0))
    }

    #[test]
    fn the_chosen_option_lands_on_the_trigger() {
        // Third of five: its row (1 + 4 + 2 * 28 = 61) sits 2 px into the
        // trigger, so the popup starts 59 px above the trigger's top.
        let placed = place(anchor(300.0), window(), 5, Some(2));
        assert_eq!(placed.origin, point(px(375.0), px(300.0 + 2.0 - 61.0)));
        assert_eq!(placed.height, 5.0 * 28.0 + 10.0);
        assert_eq!(placed.scroll, 0.0);
    }

    #[test]
    fn near_the_top_the_popup_is_cut_and_scrolled_to_stay_aligned() {
        let placed = place(anchor(60.0), window(), 12, Some(6));
        assert_eq!(placed.origin.y, px(EDGE));
        // Row 6 (1 + 4 + 168 = 173) still meets the trigger (60 + 2).
        assert_eq!(f32::from(placed.origin.y) + 173.0 - placed.scroll, 62.0);
        assert_eq!(placed.height, 12.0 * 28.0 + 10.0 - placed.scroll);
    }

    #[test]
    fn without_room_for_three_options_the_popup_takes_the_whole_height() {
        // Near the bottom, the first option would leave the popup 1 row.
        let placed = place(anchor(740.0), window(), 5, Some(0));
        assert_eq!(placed.height, 5.0 * 28.0 + 10.0);
        assert_eq!(placed.origin.y, px(800.0 - EDGE - placed.height));
    }

    #[test]
    fn near_the_bottom_a_long_list_is_cut_and_scrolls() {
        let placed = place(anchor(400.0), window(), 300, Some(0));
        assert_eq!(placed.origin.y, px(402.0 - 5.0));
        assert_eq!(placed.height, 800.0 - EDGE - 397.0);
        assert_eq!(placed.scroll, 0.0);
    }

    #[test]
    fn a_long_list_is_cut_to_the_window_and_scrolled_to_the_choice() {
        let placed = place(anchor(400.0), window(), 300, Some(150));
        assert_eq!(placed.height, 800.0 - 2.0 * EDGE);
        assert_eq!(placed.origin.y, px(EDGE));
        let row = 1.0 + 4.0 + 150.0 * 28.0;
        assert_eq!(f32::from(placed.origin.y) + row - placed.scroll, 402.0);
    }

    #[test]
    fn typeahead_matches_prefixes_and_cycles_on_a_repeated_letter() {
        let options = ["Arial", "Avenir", "Baskerville", "Bodoni", "Menlo"]
            .map(SharedString::from)
            .to_vec();
        let mut popup = SelectPopup::new("k", options, Some(0), anchor(300.0));
        let t0 = Instant::now();
        popup.type_char("b", t0);
        assert_eq!(popup.highlighted, Some(2));
        popup.type_char("o", t0 + Duration::from_millis(100));
        assert_eq!(popup.highlighted, Some(3), "\"bo\" is Bodoni");
        popup.type_char("a", t0 + Duration::from_secs(2));
        assert_eq!(popup.highlighted, Some(0), "a pause starts over");
        popup.type_char("a", t0 + Duration::from_millis(2100));
        assert_eq!(popup.highlighted, Some(1), "a repeated letter cycles");
        popup.step(-5);
        assert_eq!(popup.highlighted, Some(0), "no wrap");
    }
}
