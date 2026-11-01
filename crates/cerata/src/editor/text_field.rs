//! A one-line text field for the find bar: typing, IME, selection, the
//! clipboard, and Enter, Shift+Enter and Escape reported to its owner.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, ShapedLine, SharedString, Style, Styled, TextRun,
    UTF16Selection, Window, actions, div, fill, point, prelude::*, px, relative, size,
};

use super::theme;

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
    pub invalid: bool,
    last_layout: Option<(ShapedLine, Bounds<Pixels>)>,
}

impl EventEmitter<FieldEvent> for TextField {}

impl TextField {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: String::new(),
            selection: 0..0,
            marked: None,
            placeholder: placeholder.into(),
            invalid: false,
            last_layout: None,
        }
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_owned();
        self.selection = 0..self.text.len();
        self.marked = None;
        cx.notify();
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let range = self.clamp(range);
        self.text.replace_range(range.clone(), text);
        let caret = range.start + text.len();
        self.selection = caret..caret;
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
        cx.notify();
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = if self.invalid {
            theme::danger()
        } else {
            theme::border()
        };
        let root = div()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .w(px(220.0))
            .h(px(26.0))
            .px(px(6.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(border)
            .bg(theme::background())
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
                    window.focus(&f.focus_handle, cx);
                    let pos = f.index_for_point(event.position);
                    f.move_to(pos, cx);
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
                .shape_line(text.into(), px(13.0), &[run], None),
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
        if !empty && !selection.is_empty() {
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
        if focus.is_focused(window) && selection.is_empty() {
            let x = if empty {
                px(0.0)
            } else {
                line.x_for_index(selection.start)
            };
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.left() + x, bounds.top() + px(4.0)),
                    size(px(1.5), line_height - px(8.0)),
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
