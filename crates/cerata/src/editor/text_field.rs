//! A one-line text field for the find bar: typing, IME, selection, the
//! clipboard, and Enter, Shift+Enter and Escape reported to its owner.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, ShapedLine, SharedString, Style, Styled, Task, TextRun,
    UTF16Selection, Window, actions, div, fill, point, prelude::*, px, relative, size,
};

use super::theme;

/// The caret is shown, then hidden, this long each (Paneflow's
/// `text_input.rs`); typing or moving it shows it again at once.
const BLINK: Duration = Duration::from_millis(500);
const CARET_WIDTH: f32 = 1.0;

/// Whether the caret shows `since` the last edit or move.
fn caret_visible(since: Duration) -> bool {
    (since.as_millis() / BLINK.as_millis()).is_multiple_of(2)
}

actions!(
    text_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        Home,
        End,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Submit,
        SubmitBackward,
        Escape,
    ]
);

pub const CONTEXT: &str = "TextField";

pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new(secondary!("a"), SelectAll, c),
        KeyBinding::new(secondary!("c"), Copy, c),
        KeyBinding::new(secondary!("x"), Cut, c),
        KeyBinding::new(secondary!("v"), Paste, c),
        KeyBinding::new("enter", Submit, c),
        KeyBinding::new("shift-enter", SubmitBackward, c),
        KeyBinding::new("escape", Escape, c),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    Changed,
    Submit { backward: bool },
    Escape,
}

pub struct TextField {
    pub focus_handle: FocusHandle,
    pub text: String,
    pub selection: Range<usize>,
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    /// The accessible name; the placeholder unless [`Self::labeled`].
    label: Option<SharedString>,
    pub invalid: bool,
    /// No border, background or padding: the parent draws the field.
    bare: bool,
    /// Text in the muted color (a property name).
    muted: bool,
    text_size: Pixels,
    last_layout: Option<(ShapedLine, Bounds<Pixels>)>,
    /// The last edit or caret move, which the blink counts from.
    blink_anchor: Instant,
    /// Repaints the blink while the field has the focus.
    blink_task: Option<Task<()>>,
    caret_shown: bool,
    /// When the field took the focus, for its frame's focus animation.
    focused_since: Option<Instant>,
}

impl EventEmitter<FieldEvent> for TextField {}

impl TextField {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            text: String::new(),
            selection: 0..0,
            marked: None,
            placeholder: placeholder.into(),
            label: None,
            invalid: false,
            bare: false,
            muted: false,
            text_size: px(13.0),
            last_layout: None,
            blink_anchor: Instant::now(),
            blink_task: None,
            caret_shown: false,
            focused_since: None,
        }
    }

    /// A field without its own chrome, its text `text_size` on a line 5
    /// taller, for a parent that draws the field (the sidebar's Search).
    /// Names the field for assistive technology when its placeholder is an
    /// example or empty.
    pub fn labeled(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    pub fn bare(mut self, text_size: f32) -> Self {
        self.bare = true;
        self.text_size = px(text_size);
        self
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_owned();
        self.selection = 0..self.text.len();
        self.marked = None;
        self.blink_anchor = Instant::now();
        cx.notify();
    }

    /// When the field took the focus, while it has it.
    pub fn focused_since(&self) -> Option<Instant> {
        self.focused_since
    }

    /// Whether the caret shows now; while the field has the focus in an
    /// active window, a task repaints it at each blink.
    fn update_caret(&mut self, window: &Window, cx: &mut Context<Self>) {
        let focused = self.focus_handle.is_focused(window);
        if focused != self.focused_since.is_some() {
            self.focused_since = focused.then(Instant::now);
            self.blink_anchor = Instant::now();
        }
        if !focused || !window.is_window_active() {
            self.blink_task = None;
            self.caret_shown = false;
            return;
        }
        if self.blink_task.is_none() {
            self.blink_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(BLINK).await;
                    let alive = this.update_in(cx, |this, window, cx| {
                        let alive =
                            this.focus_handle.is_focused(window) && window.is_window_active();
                        if alive {
                            cx.notify();
                        } else {
                            this.blink_task = None;
                        }
                        alive
                    });
                    if !matches!(alive, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        self.caret_shown = caret_visible(self.blink_anchor.elapsed());
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let range = self.clamp(range);
        self.text.replace_range(range.clone(), text);
        let caret = range.start + text.len();
        self.selection = caret..caret;
        self.blink_anchor = Instant::now();
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let fix = |p: usize| {
            let mut p = p.min(self.text.len());
            while !self.text.is_char_boundary(p) {
                p -= 1;
            }
            p
        };
        let (a, b) = (fix(range.start), fix(range.end));
        a.min(b)..a.max(b)
    }

    fn prev(&self, pos: usize) -> usize {
        self.text[..pos]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, pos: usize) -> usize {
        self.text[pos..]
            .chars()
            .next()
            .map_or(pos, |c| pos + c.len_utf8())
    }

    fn to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .encode_utf16()
            .count()
    }

    fn offset_from_utf16(&self, utf16: usize) -> usize {
        let mut units = 0;
        for (ix, c) in self.text.char_indices() {
            if units >= utf16 {
                return ix;
            }
            units += c.len_utf16();
        }
        self.text.len()
    }

    fn on_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            self.prev(self.selection.start)..self.selection.start
        } else {
            self.selection.clone()
        };
        self.replace(range, "", cx);
    }

    fn on_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            self.selection.start..self.next(self.selection.start)
        } else {
            self.selection.clone()
        };
        self.replace(range, "", cx);
    }

    fn move_to(&mut self, pos: usize, cx: &mut Context<Self>) {
        self.selection = pos..pos;
        self.blink_anchor = Instant::now();
        cx.notify();
    }

    fn select_to(&mut self, pos: usize, cx: &mut Context<Self>) {
        // The anchor is the end that does not move.
        let anchor = if pos <= self.selection.start {
            self.selection.end
        } else {
            self.selection.start
        };
        self.selection = anchor.min(pos)..anchor.max(pos);
        self.blink_anchor = Instant::now();
        cx.notify();
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_caret(window, cx);
        let border = if self.invalid {
            theme::danger()
        } else {
            theme::border()
        };
        let label = self
            .label
            .clone()
            .unwrap_or_else(|| self.placeholder.clone());
        let root = div()
            .id(ElementId::View(cx.entity_id()))
            .role(gpui::Role::TextInput)
            .aria_label(label)
            .aria_value(SharedString::from(self.text.clone()))
            .aria_placeholder(self.placeholder.clone())
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            // The parent sets the width; a fixed one overflowed the
            // settings cards.
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .map(|d| {
                if self.bare {
                    d.h(self.text_size + px(5.0))
                } else {
                    d.h(px(26.0))
                        .px(px(6.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(border)
                        .bg(theme::background())
                }
            })
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(|f: &mut Self, _: &Left, _, cx| {
                let pos = if f.selection.is_empty() {
                    f.prev(f.selection.start)
                } else {
                    f.selection.start
                };
                f.move_to(pos, cx)
            }))
            .on_action(cx.listener(|f: &mut Self, _: &Right, _, cx| {
                let pos = if f.selection.is_empty() {
                    f.next(f.selection.end)
                } else {
                    f.selection.end
                };
                f.move_to(pos, cx)
            }))
            .on_action(cx.listener(|f: &mut Self, _: &SelectLeft, _, cx| {
                let head = if f.selection.start < f.selection.end {
                    f.selection.start
                } else {
                    f.selection.end
                };
                let pos = f.prev(head);
                f.select_to(pos, cx)
            }))
            .on_action(cx.listener(|f: &mut Self, _: &SelectRight, _, cx| {
                let pos = f.next(f.selection.end);
                f.select_to(pos, cx)
            }))
            .on_action(cx.listener(|f: &mut Self, _: &Home, _, cx| f.move_to(0, cx)))
            .on_action(cx.listener(|f: &mut Self, _: &End, _, cx| {
                let end = f.text.len();
                f.move_to(end, cx)
            }))
            .on_action(cx.listener(|f: &mut Self, _: &SelectAll, _, cx| {
                f.selection = 0..f.text.len();
                cx.notify()
            }))
            .on_action(cx.listener(|f: &mut Self, _: &Copy, _, cx| {
                if !f.selection.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        f.text[f.selection.clone()].to_owned(),
                    ));
                }
            }))
            .on_action(cx.listener(|f: &mut Self, _: &Cut, _, cx| {
                if !f.selection.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        f.text[f.selection.clone()].to_owned(),
                    ));
                    let range = f.selection.clone();
                    f.replace(range, "", cx);
                }
            }))
            .on_action(cx.listener(|f: &mut Self, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    let line = text.lines().next().unwrap_or("").to_owned();
                    let range = f.selection.clone();
                    f.replace(range, &line, cx);
                }
            }))
            .on_action(cx.listener(|_: &mut Self, _: &Submit, _, cx| {
                cx.emit(FieldEvent::Submit { backward: false })
            }))
            .on_action(cx.listener(|_: &mut Self, _: &SubmitBackward, _, cx| {
                cx.emit(FieldEvent::Submit { backward: true })
            }))
            .on_action(cx.listener(|_: &mut Self, _: &Escape, _, cx| cx.emit(FieldEvent::Escape)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|f: &mut Self, event: &MouseDownEvent, window, cx| {
                    f.click_at(event.position, window, cx)
                }),
            )
            .child(FieldElement { field: cx.entity() });
        #[cfg(test)]
        let root = {
            let name = format!("text-field-{}", self.placeholder);
            gpui::InteractiveElement::debug_selector(root, move || name)
        };
        root
    }
}

impl TextField {
    /// A press at `position`, on the field or on a frame around it: the
    /// focus, and the caret at the closest character.
    pub fn click_at(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let pos = self.index_for_point(position);
        self.move_to(pos, cx);
    }

    fn index_for_point(&self, position: Point<Pixels>) -> usize {
        match &self.last_layout {
            Some((line, bounds)) => line.closest_index_for_x(position.x - bounds.left()),
            None => self.text.len(),
        }
    }
}

struct FieldElement {
    field: Entity<TextField>,
}

impl IntoElement for FieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<ShapedLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let field = self.field.read(cx);
        let (text, color) = if field.text.is_empty() {
            (field.placeholder.to_string(), theme::muted())
        } else if field.muted {
            (field.text.clone(), theme::muted())
        } else {
            (field.text.clone(), theme::text())
        };
        let run = TextRun {
            len: text.len(),
            font: window.text_style().font(),
            color,
            background_color: None,
            underline: field.marked.as_ref().map(|_| gpui::UnderlineStyle {
                thickness: px(1.0),
                color: Some(theme::text()),
                wavy: false,
            }),
            strikethrough: None,
        };
        Some(
            window
                .text_system()
                .shape_line(text.into(), field.text_size, &[run], None),
        )
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        line: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(line) = line.take() else { return };
        let focus = self.field.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        let line_height = bounds.size.height;
        let (selection, empty) = {
            let field = self.field.read(cx);
            (field.selection.clone(), field.text.is_empty())
        };
        // A selection shows only while the field has the focus: an
        // unfocused field with highlighted text reads as selected.
        if !empty && !selection.is_empty() && focus.is_focused(window) {
            let x0 = line.x_for_index(selection.start);
            let x1 = line.x_for_index(selection.end);
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(bounds.left() + x0, bounds.top()),
                    point(bounds.left() + x1, bounds.bottom()),
                ),
                theme::selection(),
            ));
        }
        let _ = line.paint(
            bounds.origin,
            line_height,
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
        let (caret_shown, text_size) = {
            let field = self.field.read(cx);
            (field.caret_shown, field.text_size)
        };
        if focus.is_focused(window) && selection.is_empty() && caret_shown {
            let x = if empty {
                px(0.0)
            } else {
                line.x_for_index(selection.start)
            };
            // As tall as the text plus 2 px, centered on the line.
            let height = (text_size + px(2.0)).min(line_height);
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        bounds.left() + x,
                        bounds.top() + (line_height - height) / 2.0,
                    ),
                    size(px(CARET_WIDTH), height),
                ),
                theme::accent(),
            ));
        }
        self.field
            .update(cx, |field, _| field.last_layout = Some((line, bounds)));
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range =
            self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end);
        actual.replace(self.to_utf16(range.start)..self.to_utf16(range.end));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16(self.selection.start)..self.to_utf16(self.selection.end),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.to_utf16(r.start)..self.to_utf16(r.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        self.marked = None;
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let range = self.clamp(range);
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        let caret = range.start + text.len();
        self.selection = caret..caret;
        self.blink_anchor = Instant::now();
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (line, _) = self.last_layout.as_ref()?;
        let start = line.x_for_index(self.offset_from_utf16(range_utf16.start));
        let end = line.x_for_index(self.offset_from_utf16(range_utf16.end));
        Some(Bounds::from_corners(
            point(bounds.left() + start, bounds.top()),
            point(bounds.left() + end, bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.to_utf16(self.index_for_point(point)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caret_blinks_every_half_second_from_the_last_edit() {
        assert!(caret_visible(Duration::ZERO));
        assert!(caret_visible(Duration::from_millis(499)));
        assert!(!caret_visible(Duration::from_millis(500)));
        assert!(caret_visible(Duration::from_millis(1000)));
    }
}
