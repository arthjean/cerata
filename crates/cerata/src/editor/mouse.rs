//! Pointer input: hit-testing through the display map and the height map,
//! drag selection with frozen decorations, double and triple click, list
//! prefix zones, task checkboxes, widget selection, the wheel and the code
//! block scrollbars (US-018, US-019).
//!
//! Writer's references: `fold/core.ts:176-203` (a click on a widget selects
//! its decoration), `list/index.ts:695-751` and `:1031-1050` (prefix zones,
//! checkbox click), `drag-selection-gate.ts:39-198` (frozen reveal).

use cerata_markdown::Selection;
use cerata_markdown::list::parse_bullet_task_line;
use gpui::{
    Context, Div, InteractiveElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, ScrollEvent, Stateful, Window, px,
};

use super::code_scroll::{self, CodeScroll};
use super::layout::{LaidLine, Marker};
use super::{Editor, TOP_INSET};

/// Selection granularity of a press: single, double, triple click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unit {
    Char,
    Word,
    Line,
}

#[derive(Clone, Debug)]
pub enum Drag {
    Select {
        /// The range the press selected; the drag extends from it.
        origin: std::ops::Range<usize>,
        unit: Unit,
        moved: bool,
        /// A press on a task checkbox, toggled on release if not dragged.
        checkbox: Option<usize>,
    },
    /// Dragging a code block's scrollbar thumb.
    Thumb {
        first_line: usize,
        start_x: Pixels,
        start_offset: f32,
        per_pixel: f32,
    },
}

/// A hit on a code block's drawn scrollbar.
#[derive(Clone, Copy, Debug)]
pub struct ThumbHit {
    pub first_line: usize,
    pub bounds: gpui::Bounds<Pixels>,
    pub per_pixel: f32,
}

pub fn register(root: Stateful<Div>, cx: &mut Context<Editor>) -> Stateful<Div> {
    root.on_mouse_down(MouseButton::Left, cx.listener(Editor::on_mouse_down))
        .on_mouse_down(MouseButton::Right, cx.listener(Editor::on_right_mouse_down))
        .on_scroll(cx.listener(Editor::on_scroll_wheel))
}

/// Where a click in a list prefix goes: the nearest of the line start, the
/// marker and the body (`listPrefixClickPosition`).
fn prefix_zone(line: &LaidLine, text: &str, x: f32) -> Option<usize> {
    let prefix = line.prefix.as_ref()?;
    if x >= line.text_left {
        return None;
    }
    let parsed = parse_bullet_task_line(line.start, text);
    let (line_from, marker_from, body_from) = match parsed {
        Some(p) => (p.line_from, p.marker_from, p.body_from),
        None => (line.start, prefix.range.start, prefix.range.end),
    };
    let left = prefix.left.min(prefix.x);
    let marker_start = (line.text_left - prefix.width).max(left);
    let right = line.text_left;
    Some(if x < marker_start {
        if x - left < marker_start - x {
            line_from
        } else {
            marker_from
        }
    } else if x - marker_start < right - x {
        marker_from
    } else {
        body_from
    })
}

impl Editor {
    /// The document line and the scroll-space y under a window point.
    pub(super) fn line_at_point(&self, position: Point<Pixels>) -> Option<(usize, f64)> {
        let viewport = self.viewport?;
        let y = self.scroll_top()
            + (position.y - viewport.bounds.top() - px(TOP_INSET)).as_f32() as f64;
        Some((self.heights.find(y).0, y))
    }

    /// Buffer offset under a window point, through the height map and the
    /// line's display map (hidden syntax is skipped forward).
    pub fn offset_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let Some(viewport) = self.viewport else {
            return 0;
        };
        let Some((ix, y)) = self.line_at_point(position) else {
            return 0;
        };
        if y < 0.0 {
            return 0;
        }
        if y >= self.heights.total() {
            return self.state.text().len();
        }
        let line = self.layout_line(ix, window, cx);
        let local_y = (y - self.heights.prefix(ix)) as f32;
        let x = (position.x - viewport.column_left).as_f32();
        if let Some(block) = &line.block {
            if block.replaces {
                return if local_y < block.height / 2.0 {
                    line.start
                } else {
                    line.end()
                };
            }
            if local_y >= block.top {
                return line.end();
            }
        }
        if line.wrapped.is_none() {
            return line.start;
        }
        let row_height = line.metrics.row_height;
        let row = (((local_y - line.text_top) / row_height).floor().max(0.0) as usize)
            .min(line.rows().saturating_sub(1));
        if row == 0
            && let Some(pos) =
                prefix_zone(&line, self.state.text().slice(line.start..line.end()), x)
        {
            return pos;
        }
        Self::offset_in_row(&line, row, x)
    }

    /// The fold decoration (block or inline widget) under `offset`'s line
    /// at `position`, if the pointer is on a replaced widget.
    fn widget_at(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let (ix, y) = self.line_at_point(position)?;
        let line = self.layout_line(ix, window, cx);
        let block = line.block.as_ref()?;
        let local_y = (y - self.heights.prefix(ix)) as f32;
        let inside = local_y >= block.top && local_y < block.top + block.height;
        if !inside {
            return None;
        }
        let range = line.start..line.end();
        self.state
            .decorations_in(range.clone())
            .into_iter()
            .find_map(|d| match d {
                cerata_markdown::Decoration::Replace {
                    range: r,
                    block: true,
                    ..
                } if r.start <= range.start && r.end >= range.end => Some(r),
                cerata_markdown::Decoration::Widget {
                    pos, block: true, ..
                } if pos == range.end => {
                    // A widget below its revealed source: its image node.
                    self.state
                        .tree()
                        .iter_range(range.clone())
                        .find(|n| n.kind() == cerata_markdown::NodeKind::Image && n.to() == pos)
                        .map(|n| n.range())
                }
                _ => None,
            })
    }

    fn unit_range(&self, pos: usize, unit: Unit) -> std::ops::Range<usize> {
        let text = self.state.text();
        match unit {
            Unit::Char => pos..pos,
            Unit::Word => cerata_markdown::basic::word_around(text, pos).unwrap_or(pos..pos),
            Unit::Line => {
                let line = text.line_at(pos);
                let next = if line.number + 1 < text.line_count() {
                    text.line(line.number + 1).from
                } else {
                    line.to
                };
                line.from..next
            }
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if self.menu.take().is_some() {
            cx.notify();
        }
        self.goal_x = None;
        if let Some(pos) = self
            .mark_hits
            .iter()
            .find(|(bounds, _)| bounds.contains(&event.position))
            .map(|(_, pos)| *pos)
        {
            self.click_mark(pos, cx);
            return;
        }
        if let Some(hit) = self
            .thumbs
            .iter()
            .find(|t| t.bounds.contains(&event.position))
            .copied()
        {
            self.drag = Some(Drag::Thumb {
                first_line: hit.first_line,
                start_x: event.position.x,
                start_offset: self.code_scroll.offset_of_line(hit.first_line),
                per_pixel: hit.per_pixel,
            });
            // The next frame installs the window-wide drag listeners.
            cx.notify();
            return;
        }
        let unit = match event.click_count {
            0 | 1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        // A click on a link's text follows it, the caret stays.
        if unit == Unit::Char
            && !event.modifiers.shift
            && let Some(target) = self.link_hit(event.position, window, cx)
        {
            self.follow_link(target, window, cx);
            return;
        }
        // A click on a widget with no selection selects its whole
        // decoration, anchored at its end.
        if unit == Unit::Char
            && !event.modifiers.shift
            && self.state.selection().main().is_empty()
            && let Some(range) = self.widget_at(event.position, window, cx)
        {
            self.select(Selection::single(range.end, range.start), cx);
            return;
        }
        let pos = self.offset_for_point(event.position, window, cx);
        let checkbox = self.checkbox_at(event.position, window, cx);
        // Freeze what is revealed in the shape the user is looking at.
        self.state.begin_drag();
        let origin = self.unit_range(pos, unit);
        let main = self.state.selection().main();
        let selection = if event.modifiers.shift {
            Selection::single(main.anchor, pos)
        } else if origin.is_empty() {
            Selection::cursor(pos)
        } else {
            Selection::single(origin.start, origin.end)
        };
        self.drag = Some(Drag::Select {
            origin: if event.modifiers.shift {
                main.anchor..main.anchor
            } else {
                origin
            },
            unit,
            moved: false,
            checkbox,
        });
        self.select(selection, cx);
    }

    /// The task marker under the pointer: inside the drawn checkbox.
    fn checkbox_at(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let viewport = self.viewport?;
        let (ix, y) = self.line_at_point(position)?;
        let line = self.layout_line(ix, window, cx);
        let prefix = line.prefix.as_ref()?;
        if !matches!(prefix.marker, Marker::Task { .. }) {
            return None;
        }
        let local_y = (y - self.heights.prefix(ix)) as f32 - line.text_top;
        let x = (position.x - viewport.column_left).as_f32();
        let in_row = (0.0..line.metrics.row_height).contains(&local_y);
        let in_box = (prefix.x..prefix.x + prefix.width).contains(&x);
        (in_row && in_box).then_some(prefix.range.start)
    }

    /// Window-wide while a drag is on (`EditorElement::paint`), so a drag
    /// keeps going and ends outside the editor and the window.
    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            // The release happened outside the window and never reached us:
            // the drag ends where it last was.
            self.end_drag(cx);
            return;
        }
        match drag {
            Drag::Thumb {
                first_line,
                start_x,
                start_offset,
                per_pixel,
            } => {
                let offset = start_offset + (event.position.x - start_x).as_f32() * per_pixel;
                if self.code_scroll.set(first_line, offset) {
                    cx.notify();
                }
            }
            Drag::Select {
                origin,
                unit,
                checkbox,
                ..
            } => {
                let pos = self.offset_for_point(event.position, window, cx);
                let target = self.unit_range(pos, unit);
                let selection = if pos < origin.start {
                    Selection::single(origin.end, target.start.min(pos))
                } else {
                    Selection::single(origin.start, target.end.max(pos))
                };
                self.drag = Some(Drag::Select {
                    origin,
                    unit,
                    moved: true,
                    checkbox,
                });
                if selection != *self.state.selection() {
                    self.select(selection, cx);
                }
                // Keep the drag position in view.
                if let Some(viewport) = self.viewport {
                    let bounds = viewport.bounds;
                    let delta = if event.position.y < bounds.top() {
                        (event.position.y - bounds.top()).as_f32()
                    } else if event.position.y > bounds.bottom() {
                        (event.position.y - bounds.bottom()).as_f32()
                    } else {
                        0.0
                    };
                    if delta != 0.0 {
                        let top = self.scroll_top() + delta as f64;
                        self.set_scroll_top(top);
                    }
                }
            }
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Drag::Select {
            moved: false,
            checkbox: Some(pos),
            ..
        }) = self.drag
        {
            self.update_state(cx, |state| state.checkbox_click(pos, false));
        }
        self.end_drag(cx);
    }

    /// The drag ends: the selection stays at its last position and the
    /// reveal follows it again.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() || self.state.is_dragging() {
            self.state.end_drag();
            cx.notify();
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(self.config.row_height()));
        let (dx, dy) = (delta.x.as_f32(), delta.y.as_f32());
        if code_scroll::is_horizontal(dx, dy)
            && let Some(first) = self.code_block_line_at(event.position, window, cx)
        {
            let offset = self.code_scroll.offset_of_line(first) - dx;
            if self.code_scroll.set(first, offset) {
                cx.notify();
            }
            return;
        }
        if dy != 0.0 {
            let top = self.scroll_top() - dy as f64;
            self.set_scroll_top(top);
            cx.notify();
        }
    }

    /// First line of the code block under the pointer.
    fn code_block_line_at(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (ix, _) = self.line_at_point(position)?;
        let line = self.layout_line(ix, window, cx);
        let code = line.code.as_ref()?;
        Some(self.state.text().line_at(code.block_start).number)
    }

    /// Keeps the main caret visible inside its code block, by the smallest
    /// change of the block's offset.
    pub(super) fn reveal_caret_in_code(&mut self, window: &Window, cx: &mut Context<Self>) {
        let head = self.cursor();
        let (line, _) = self.row_at(head, window, cx);
        let (Some(code), Some(wrapped)) = (&line.code, &line.wrapped) else {
            return;
        };
        let display = line.display.to_display(head - line.start);
        let Some(position) = wrapped.position_for_index(display, px(line.metrics.row_height))
        else {
            return;
        };
        let first = self.state.text().line_at(code.block_start).number;
        let visible = self.column_width() - super::layout::CODE_PADDING * 2.0;
        let offset = code.scroll_x;
        let x = position.x.as_f32();
        let next = code_scroll::reveal_offset(offset, x, offset, offset + visible - 2.0);
        if next != offset {
            CodeScroll::set(&mut self.code_scroll, first, next);
            cx.notify();
        }
    }

    fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let pos = self.offset_for_point(event.position, window, cx);
        if !self
            .state
            .selection()
            .ranges()
            .iter()
            .any(|r| r.from() <= pos && pos <= r.to() && !r.is_empty())
        {
            self.select(Selection::cursor(pos), cx);
        }
        self.open_context_menu(event.position, pos, cx);
    }
}
