//! The section rail (US-038): one 8 x 1 px mark per heading on the right
//! edge, the active one opaque, the others at 35 %; hovering opens a
//! 260 px outline. A click scrolls its heading to the top of the safe zone;
//! a right click offers "Copy heading link". Hidden under 5 headings.
//! Ported from `section-rail.tsx:15-200`, `use-active-headings.ts:9-79`
//! and `section-rail.css`.

use std::rc::Rc;
use std::time::Duration;

use cerata_markdown::headings::{self, Heading};
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, KeyDownEvent, MouseButton, MouseDownEvent,
    Pixels, Point, ScrollStrategy, SharedString, Task, UniformListScrollHandle, div, prelude::*,
    px, uniform_list,
};

use super::menu::{ContextMenu, MenuAction, MenuItem};
use super::{Autoscroll, Editor, SAFE_MARGIN, SCROLLBAR_WIDTH, theme};

const TICK_WIDTH: f32 = 8.0;
const TICK_HEIGHT: f32 = 1.0;
const TICK_GAP: f32 = 6.0;
/// Below this many headings a note needs no map.
pub const MIN_HEADINGS: usize = 5;
const RAIL_INNER_WIDTH: f32 = TICK_WIDTH + 2.0;
pub const POPOVER_WIDTH: f32 = 260.0;
const POPOVER_TRANSITION: Duration = Duration::from_millis(180);
/// The scroll-to-heading landing line, plus a little jitter
/// (`ACTIVE_OFFSET_PX`).
pub const ACTIVE_OFFSET: f64 = SAFE_MARGIN + 4.0;
/// Writer's default outline depth (`use-document-headings.ts:18`).
const MAX_DEPTH: u8 = 3;
/// Headings follow an edit this long after the last keystroke, keeping the
/// whole-document scan off the typing path (`use-document-headings.ts:85-87`).
pub const HEADINGS_DEBOUNCE: Duration = Duration::from_millis(150);
/// One outline row: 13 px text at 1.5, and the 4 px gap under it.
const ROW_HEIGHT: f32 = 13.0 * 1.5 + 4.0;

#[derive(Default)]
pub struct Rail {
    pub(super) headings: Rc<Vec<Heading>>,
    revision: Option<u64>,
    /// The parse waiting for typing to pause, and the revision it is for.
    refresh: Option<(u64, Task<()>)>,
    /// The outline's rows, virtualized: a long note has thousands.
    outline: UniformListScrollHandle,
    over_rail: bool,
    over_popover: bool,
    /// Bumped at each opening, to replay the popover's transition.
    openings: usize,
}

impl Rail {
    pub fn is_open(&self) -> bool {
        self.over_rail || self.over_popover
    }

    pub fn close(&mut self) -> bool {
        let open = self.is_open();
        self.over_rail = false;
        self.over_popover = false;
        open
    }
}

/// `buildHeadingLink`.
pub fn heading_link(heading: &Heading) -> String {
    format!("[{}](#{})", heading.text, heading.slug)
}

impl Editor {
    /// The note's headings (h1 to h3): parsed when the note opens, then
    /// [`HEADINGS_DEBOUNCE`] after the last edit.
    pub fn rail_headings(&mut self, cx: &mut Context<Self>) -> Rc<Vec<Heading>> {
        let revision = self.state.revision();
        if self.rail.revision.is_none() {
            self.parse_rail_headings();
        } else if self.rail.revision != Some(revision)
            && self
                .rail
                .refresh
                .as_ref()
                .is_none_or(|(r, _)| *r != revision)
        {
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HEADINGS_DEBOUNCE).await;
                this.update(cx, |editor, cx| {
                    // Typing inside a section moves no heading: no frame.
                    if editor.parse_rail_headings() {
                        cx.notify();
                    }
                })
                .ok();
            });
            self.rail.refresh = Some((revision, task));
        }
        self.rail.headings.clone()
    }

    /// `true` when the headings changed.
    fn parse_rail_headings(&mut self) -> bool {
        let parsed = headings::parse(self.state.doc(), MAX_DEPTH, MAX_DEPTH);
        let changed = *self.rail.headings != parsed;
        if changed {
            self.rail.headings = Rc::new(parsed);
        }
        self.rail.revision = Some(self.state.revision());
        self.rail.refresh = None;
        changed
    }

    /// The last heading above the top of the view plus 144 px, else the
    /// first (`computeActive`).
    pub fn active_heading(&self, headings: &[Heading]) -> Option<usize> {
        if headings.is_empty() {
            return None;
        }
        let threshold = self.scroll_top() + ACTIVE_OFFSET;
        let mut active = 0;
        // Lines from before the last edits until the debounced parse.
        let lines = self.heights.len();
        for (i, heading) in headings.iter().enumerate() {
            if self.heights.prefix(heading.line.min(lines)) > threshold {
                break;
            }
            active = i;
        }
        Some(active)
    }

    pub(super) fn rail_go_to(&mut self, pos: usize, cx: &mut Context<Self>) {
        let pos = self
            .state
            .text()
            .floor_char_boundary(pos.min(self.state.text().len()));
        self.autoscroll = Some(Autoscroll::Top(pos));
        cx.notify();
    }

    fn rail_menu(&mut self, heading: &Heading, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.rail_menu_at(heading, event.position, cx);
    }

    fn rail_menu_at(&mut self, heading: &Heading, position: Point<Pixels>, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.menu = Some(ContextMenu {
            position,
            items: vec![MenuItem::Action {
                label: "Copy heading link".into(),
                shortcut: None,
                enabled: true,
                action: MenuAction::CopyLink(heading_link(heading)),
            }],
            open_submenu: None,
        });
        cx.notify();
    }

    pub(super) fn render_rail(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let headings = self.rail_headings(cx);
        if headings.len() < MIN_HEADINGS {
            return None;
        }
        let viewport = self.viewport?;
        let view_height = viewport.bounds.size.height.as_f32();
        let active = self.active_heading(&headings);
        let open = self.rail.is_open();
        // Writer clips the marks to 70 % of the view (`section-rail.tsx`).
        // Only the marks that fit are built, a window kept around the
        // active one: a long note has thousands of headings.
        let step = TICK_HEIGHT + TICK_GAP;
        let shown = (((view_height * 0.7) / step).floor().max(1.0) as usize).min(headings.len());
        let first = active
            .unwrap_or(0)
            .saturating_sub(shown / 2)
            .min(headings.len() - shown);
        let clip = shown as f32 * step;

        let ticks = headings[first..first + shown]
            .iter()
            .enumerate()
            .map(|(k, heading)| {
                let i = first + k;
                let pos = heading.pos;
                let link_heading = heading.clone();
                let menu_heading = heading.clone();
                // A Tab stop (Ctrl+M) with a focus ring around the 8 x 1 mark;
                // Shift+F10 opens its menu.
                let tick = crate::a11y::button(div().id(("rail-tick", i)), heading.text.clone())
                    .w(px(TICK_WIDTH + 2.0))
                    .h(px(TICK_HEIGHT + TICK_GAP))
                    .flex()
                    .items_center()
                    .capture_key_down(cx.listener(|editor, event: &KeyDownEvent, window, cx| {
                        // The mark's open menu takes Enter, Space and Escape
                        // before they click the mark (the editor's context menu
                        // has no keyboard focus of its own).
                        let Some(menu) = &editor.menu else {
                            return;
                        };
                        let key = event.keystroke.key.as_str();
                        if !matches!(key, "enter" | "space" | "escape") {
                            return;
                        }
                        let action = menu.items.iter().find_map(|item| match item {
                            MenuItem::Action {
                                enabled: true,
                                action,
                                ..
                            } => Some(action.clone()),
                            _ => None,
                        });
                        editor.menu = None;
                        if key != "escape"
                            && let Some(action) = action
                        {
                            editor.run_menu_action(action, window, cx);
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_key_down(
                        cx.listener(move |editor, event: &KeyDownEvent, window, cx| {
                            let stroke = &event.keystroke;
                            if stroke.key == "f10" && stroke.modifiers.shift {
                                // Beside the rail, where the outline would open.
                                let at = editor.viewport.map_or(window.mouse_position(), |v| {
                                    gpui::point(
                                        v.bounds.right() - px(POPOVER_WIDTH),
                                        v.bounds.center().y,
                                    )
                                });
                                editor.rail_menu_at(&menu_heading, at, cx);
                            }
                        }),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |editor, _, _, cx| editor.rail_go_to(pos, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                            editor.rail_menu(&link_heading, event, cx)
                        }),
                    )
                    .child(
                        div()
                            .w(px(TICK_WIDTH))
                            .h(px(TICK_HEIGHT))
                            .bg(theme::text())
                            .opacity(if Some(i) == active { 1.0 } else { 0.35 }),
                    );
                #[cfg(test)]
                let tick = tick.debug_selector(move || format!("rail-tick-{i}"));
                tick
            });
        let rail = div()
            .id("section-rail")
            .w(px(RAIL_INNER_WIDTH))
            .h(px(clip))
            .overflow_hidden()
            .flex()
            .flex_col()
            .items_end()
            .role(gpui::Role::Navigation)
            .aria_label(SharedString::from("Document sections"))
            .on_hover(cx.listener(|editor, hovered: &bool, _, cx| {
                let was = editor.rail.is_open();
                editor.rail.over_rail = *hovered;
                if !was && editor.rail.is_open() {
                    editor.rail.openings += 1;
                    // The outline opens on the active heading.
                    let headings = editor.rail.headings.clone();
                    if let Some(active) = editor.active_heading(&headings) {
                        editor
                            .rail
                            .outline
                            .scroll_to_item(active, ScrollStrategy::Center);
                    }
                }
                cx.notify();
            }))
            // The ticks fade out while the outline shows.
            .when(open, |d| d.opacity(0.0))
            .children(ticks);
        #[cfg(test)]
        let rail = rail.debug_selector(|| "section-rail".into());

        let mut layer = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(SCROLLBAR_WIDTH))
            .w(px(POPOVER_WIDTH))
            .flex()
            .items_center()
            .justify_end()
            .child(rail);
        if open {
            let count = headings.len();
            let rows = uniform_list(
                "rail-rows",
                count,
                cx.processor(|editor, range: std::ops::Range<usize>, _window, cx| {
                    let headings = editor.rail.headings.clone();
                    let active = editor.active_heading(&headings);
                    range
                        .filter_map(|i| {
                            let heading = headings.get(i)?;
                            Some(outline_row(i, heading, Some(i) == active, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.rail.outline)
            .h(px((count as f32 * ROW_HEIGHT).min(view_height * 0.7 - 24.0)));
            let popover = div()
                .id("rail-popover")
                .absolute()
                .right_0()
                .w(px(POPOVER_WIDTH))
                .px(px(16.0))
                .py(px(12.0))
                .rounded(px(12.0))
                .bg(theme::overlay())
                .border_1()
                .border_color(theme::border())
                .shadow_md()
                .occlude()
                .on_hover(cx.listener(|editor, hovered: &bool, _, cx| {
                    editor.rail.over_popover = *hovered;
                    cx.notify();
                }))
                .child(rows);
            #[cfg(test)]
            let popover = popover.debug_selector(|| "rail-popover".into());
            if cx.reduce_motion() {
                layer = layer.child(popover);
            } else {
                layer = layer.child(popover.with_animation(
                    ("rail-popover-in", self.rail.openings),
                    Animation::new(POPOVER_TRANSITION),
                    |el, t| el.opacity(t),
                ));
            }
        }
        Some(layer.into_any_element())
    }
}

/// One outline row: the heading's text, the active one in the accent color.
fn outline_row(i: usize, heading: &Heading, active: bool, cx: &mut Context<Editor>) -> AnyElement {
    let pos = heading.pos;
    let link_heading = heading.clone();
    let color = if active {
        theme::accent()
    } else {
        theme::muted()
    };
    let row = div()
        .id(("rail-row", i))
        .w_full()
        .h(px(ROW_HEIGHT))
        .pb(px(4.0))
        .truncate()
        .text_size(px(13.0))
        .line_height(gpui::relative(1.5))
        .text_color(color)
        .hover(|s| s.text_color(theme::text()))
        .role(gpui::Role::Button)
        .aria_label(SharedString::from(heading.text.clone()))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |editor, _, _, cx| editor.rail_go_to(pos, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                editor.rail_menu(&link_heading, event, cx)
            }),
        )
        .child(SharedString::from(heading.text.clone()));
    #[cfg(test)]
    let row = row.debug_selector(move || format!("rail-row-{i}"));
    row.into_any_element()
}
