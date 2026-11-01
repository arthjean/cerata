//! The frontmatter panel (US-034): one row per property, drawn in place of
//! the YAML while the selection is away from it. Ported from
//! `frontmatter-panel.tsx:105-158` and `use-frontmatter-entries.ts:10-111`.
//!
//! Each edit rewrites the frontmatter through the editor
//! ([`PanelEvent::Write`]), entry by entry (`yaml_entries::write`), so the
//! properties it does not touch keep their text. A YAML the panel cannot
//! read is shown raw with its error and never written.

use cerata_markdown::yaml_entries::{self, Properties, Row as YamlRow};
use gpui::{
    AnyElement, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Render, SharedString, Subscription, Window, div, prelude::*, px, relative,
};

use super::text_field::{self, FieldEvent, TextField};
use super::{actions, theme};

const TEXT_SIZE: f32 = 13.0;

/// What the panel asks of the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    /// The new YAML between the delimiters; `None` removes the block.
    Write(Option<String>),
    /// Give the focus back to the text.
    FocusEditor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Key,
    Value,
}

struct Row {
    id: u64,
    key: Entity<TextField>,
    value: Entity<TextField>,
    /// The entry it shows, by index in the parsed properties.
    origin: Option<usize>,
    /// A list or map: shown, edited only as text.
    complex: bool,
    _events: [Subscription; 2],
    /// Focus-out listeners, registered at the first frame (they need the
    /// window).
    blur: Option<[Subscription; 2]>,
}

pub struct FrontmatterPanel {
    /// The YAML last read or written; `None` without frontmatter.
    yaml: Option<String>,
    properties: Properties,
    rows: Vec<Row>,
    next_id: u64,
    /// A field to focus at the next frame.
    focus: Option<(u64, Field)>,
}

impl EventEmitter<PanelEvent> for FrontmatterPanel {}

impl FrontmatterPanel {
    pub fn new() -> Self {
        Self {
            yaml: None,
            properties: Properties::Entries {
                entries: Vec::new(),
                prefix: 0..0,
            },
            rows: Vec::new(),
            next_id: 0,
            focus: None,
        }
    }

    fn field(
        &mut self,
        id: u64,
        which: Field,
        text: &str,
        cx: &mut Context<Self>,
    ) -> (Entity<TextField>, Subscription) {
        let (placeholder, label) = match which {
            Field::Key => ("key", "Property name"),
            Field::Value => ("value", "Property value"),
        };
        let field = cx.new(|cx| {
            let mut field = TextField::new(placeholder, cx)
                .labeled(label)
                .bare(TEXT_SIZE);
            if which == Field::Key {
                field = field.muted();
            }
            field.text = text.to_owned();
            field.selection = text.len()..text.len();
            field
        });
        let sub = cx.subscribe(&field, move |panel, _, event: &FieldEvent, cx| {
            panel.on_field(id, which, *event, cx);
        });
        (field, sub)
    }

    fn new_row(
        &mut self,
        key: &str,
        value: &str,
        origin: Option<usize>,
        complex: bool,
        cx: &mut Context<Self>,
    ) -> Row {
        let id = self.next_id;
        self.next_id += 1;
        let (key, key_sub) = self.field(id, Field::Key, key, cx);
        let (value, value_sub) = self.field(id, Field::Value, value, cx);
        Row {
            id,
            key,
            value,
            origin,
            complex,
            _events: [key_sub, value_sub],
            blur: None,
        }
    }

    /// Follows the document's frontmatter, `None` when it has none. The
    /// YAML the panel itself wrote changes nothing.
    pub fn sync(&mut self, yaml: Option<&str>, cx: &mut Context<Self>) {
        if self.yaml.as_deref() == yaml {
            return;
        }
        self.yaml = yaml.map(str::to_owned);
        let Some(yaml) = yaml else {
            self.properties = Properties::Entries {
                entries: Vec::new(),
                prefix: 0..0,
            };
            self.rows.clear();
            cx.notify();
            return;
        };
        self.properties = yaml_entries::parse(yaml);
        let wanted: Vec<(String, String, Option<usize>, bool)> = match &self.properties {
            Properties::Entries { entries, .. } if entries.is_empty() => {
                vec![(String::new(), String::new(), None, false)]
            }
            Properties::Entries { entries, .. } => entries
                .iter()
                .enumerate()
                .map(|(i, e)| (e.key.clone(), e.value.clone(), Some(i), e.complex))
                .collect(),
            Properties::Invalid(_) => Vec::new(),
        };
        // Rows are kept where they can be, so a field keeps its focus.
        self.rows.truncate(wanted.len());
        for (i, (key, value, origin, complex)) in wanted.into_iter().enumerate() {
            match self.rows.get_mut(i) {
                Some(row) => {
                    row.origin = origin;
                    row.complex = complex;
                    for (field, text) in [(&row.key, key), (&row.value, value)] {
                        field.update(cx, |f, cx| {
                            if f.text != text {
                                f.selection = text.len()..text.len();
                                f.text = text;
                                cx.notify();
                            }
                        });
                    }
                }
                None => {
                    let row = self.new_row(&key, &value, origin, complex, cx);
                    self.rows.push(row);
                }
            }
        }
        cx.notify();
    }

    /// Focuses the first property name at the next frame (`---` typed on
    /// the first line).
    pub fn focus_first(&mut self, cx: &mut Context<Self>) {
        self.focus = self.rows.first().map(|r| (r.id, Field::Key));
        cx.notify();
    }

    pub fn has_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.rows.iter().any(|r| {
            r.key.focus_handle(cx).is_focused(window) || r.value.focus_handle(cx).is_focused(window)
        })
    }

    fn index(&self, id: u64) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }

    /// Writes the rows to the document (`commit`).
    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(yaml) = self.yaml.clone() else {
            return;
        };
        if self.rows.is_empty() {
            self.yaml = None;
            cx.emit(PanelEvent::Write(None));
            cx.emit(PanelEvent::FocusEditor);
            cx.notify();
            return;
        }
        let texts: Vec<(String, String, Option<usize>)> = self
            .rows
            .iter()
            .map(|r| {
                (
                    r.key.read(cx).text.clone(),
                    r.value.read(cx).text.clone(),
                    r.origin,
                )
            })
            .collect();
        let rows: Vec<YamlRow<'_>> = texts
            .iter()
            .map(|(key, value, origin)| YamlRow {
                key,
                value,
                origin: *origin,
            })
            .collect();
        let Some(written) = yaml_entries::write(&yaml, &self.properties, &rows) else {
            return;
        };
        // The rows with a key are, in order, the entries just written.
        self.properties = yaml_entries::parse(&written);
        let mut next = 0;
        for (row, (key, _, _)) in self.rows.iter_mut().zip(&texts) {
            if key.trim().is_empty() {
                row.origin = None;
            } else {
                row.origin = Some(next);
                next += 1;
            }
        }
        self.yaml = Some(written.clone());
        cx.emit(PanelEvent::Write(Some(written)));
        cx.notify();
    }

    fn add_row(&mut self, cx: &mut Context<Self>) {
        let row = self.new_row("", "", None, false, cx);
        self.focus = Some((row.id, Field::Key));
        self.rows.push(row);
        cx.notify();
    }

    fn remove_row(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.index(id) else {
            return;
        };
        self.rows.remove(ix);
        self.focus = ix
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .map(|r| (r.id, Field::Value));
        self.commit(cx);
    }

    fn on_field(&mut self, id: u64, which: Field, event: FieldEvent, cx: &mut Context<Self>) {
        let Some(ix) = self.index(id) else {
            return;
        };
        match (event, which) {
            (FieldEvent::Changed, _) => self.commit(cx),
            (FieldEvent::Submit { backward: false }, Field::Key) => {
                self.focus = Some((id, Field::Value));
                cx.notify();
            }
            // Enter on the last value adds a row.
            (FieldEvent::Submit { backward: false }, Field::Value) if ix + 1 == self.rows.len() => {
                self.add_row(cx)
            }
            (FieldEvent::Submit { backward: false }, Field::Value) => {
                self.focus = Some((self.rows[ix + 1].id, Field::Key));
                cx.notify();
            }
            (FieldEvent::Escape, _) => cx.emit(PanelEvent::FocusEditor),
            _ => {}
        }
    }

    /// A row that loses the focus without a key is removed.
    fn on_blur(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        // The focus may only be moving to the row's other field.
        cx.defer_in(window, move |panel, window, cx| {
            let Some(row) = panel.index(id).map(|ix| &panel.rows[ix]) else {
                return;
            };
            let inside = row.key.focus_handle(cx).is_focused(window)
                || row.value.focus_handle(cx).is_focused(window);
            if !inside && row.key.read(cx).text.trim().is_empty() {
                panel.remove_row(id, cx);
                // The focus went where the user put it.
                panel.focus = None;
            }
        });
    }

    /// Moves the focus `delta` fields away, in reading order; `false` past
    /// the first or last field, where Tab leaves the panel.
    fn step(&mut self, delta: isize, cx: &mut Context<Self>, window: &Window) -> bool {
        let fields: Vec<(u64, Field)> = self
            .rows
            .iter()
            .flat_map(|r| {
                let value = (!r.complex).then_some((r.id, Field::Value));
                std::iter::once((r.id, Field::Key)).chain(value)
            })
            .collect();
        let current = fields.iter().position(|&(id, which)| {
            self.rows.iter().any(|r| {
                r.id == id
                    && match which {
                        Field::Key => r.key.focus_handle(cx).is_focused(window),
                        Field::Value => r.value.focus_handle(cx).is_focused(window),
                    }
            })
        });
        if let Some(target) = current
            .and_then(|i| i.checked_add_signed(delta))
            .and_then(|i| fields.get(i))
        {
            self.focus = Some(*target);
            cx.notify();
            return true;
        }
        false
    }

    /// The row above or below, same column.
    fn vertical(&mut self, down: bool, window: &Window, cx: &mut Context<Self>) {
        let current = self.rows.iter().enumerate().find_map(|(i, r)| {
            if r.key.focus_handle(cx).is_focused(window) {
                Some((i, Field::Key))
            } else if r.value.focus_handle(cx).is_focused(window) {
                Some((i, Field::Value))
            } else {
                None
            }
        });
        let Some((i, which)) = current else {
            return;
        };
        let target = if down {
            i.checked_add(1)
        } else {
            i.checked_sub(1)
        };
        if let Some(row) = target.and_then(|t| self.rows.get(t)) {
            let which = if row.complex { Field::Key } else { which };
            self.focus = Some((row.id, which));
            cx.notify();
        }
    }

    fn row_element(&self, row: &Row, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id;
        let focused = row.key.focus_handle(cx).is_focused(window)
            || row.value.focus_handle(cx).is_focused(window);
        let group: SharedString = format!("fm-row-{id}").into();
        let value: AnyElement = if row.complex {
            div()
                .id(("fm-complex", id))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::muted())
                .child(row.value.read(cx).text.clone())
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .child(row.value.clone())
                .into_any_element()
        };
        div()
            .id(("fm-row", id))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(16.0))
            .px(px(12.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .when(focused, |d| d.bg(theme::row_hover()))
            // Backspace in an empty row removes it (`handleKeyDown`).
            .capture_action(cx.listener(move |panel, _: &text_field::Backspace, _, cx| {
                let Some(ix) = panel.index(id) else {
                    return;
                };
                let row = &panel.rows[ix];
                if row.key.read(cx).text.is_empty() && row.value.read(cx).text.is_empty() {
                    cx.stop_propagation();
                    panel.remove_row(id, cx);
                }
            }))
            .child(
                div()
                    .w(relative(0.25))
                    .min_w(px(80.0))
                    .max_w(px(144.0))
                    .flex_shrink_0()
                    .child(row.key.clone()),
            )
            .child(value)
            .child(
                div()
                    .id(("fm-remove", id))
                    .size(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_color(theme::muted())
                    .opacity(0.0)
                    .group_hover(group, |s| s.opacity(1.0))
                    .hover(|s| s.text_color(theme::text()))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from("Remove property"))
                    .on_click(cx.listener(move |panel, _, _, cx| panel.remove_row(id, cx)))
                    .child("\u{00d7}"),
            )
            .into_any_element()
    }
}

impl Render for FrontmatterPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus-out listeners need the window.
        for ix in 0..self.rows.len() {
            if self.rows[ix].blur.is_some() {
                continue;
            }
            let id = self.rows[ix].id;
            let key = self.rows[ix].key.focus_handle(cx);
            let value = self.rows[ix].value.focus_handle(cx);
            let on_key = cx.on_focus_out(&key, window, move |p, _, window, cx| {
                p.on_blur(id, window, cx)
            });
            let on_value = cx.on_focus_out(&value, window, move |p, _, window, cx| {
                p.on_blur(id, window, cx)
            });
            self.rows[ix].blur = Some([on_key, on_value]);
        }
        if let Some((id, which)) = self.focus.take()
            && let Some(row) = self.index(id).map(|ix| &self.rows[ix])
        {
            let handle: FocusHandle = match which {
                Field::Key => row.key.focus_handle(cx),
                Field::Value => row.value.focus_handle(cx),
            };
            window.defer(cx, move |window, cx| window.focus(&handle, cx));
        }

        let root = div()
            .id("frontmatter")
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pb(px(24.0))
            .text_size(px(TEXT_SIZE))
            .cursor(CursorStyle::Arrow)
            // The text behind never sees a click in the panel.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // Past the edges the editor moves the focus on.
            .on_action(cx.listener(|p, _: &actions::Indent, window, cx| {
                if !p.step(1, cx, window) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|p, _: &actions::Outdent, window, cx| {
                if !p.step(-1, cx, window) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|p, _: &actions::Up, window, cx| p.vertical(false, window, cx)))
            .on_action(
                cx.listener(|p, _: &actions::Down, window, cx| p.vertical(true, window, cx)),
            );

        if let Properties::Invalid(error) = &self.properties {
            let raw = self.yaml.clone().unwrap_or_default();
            return root
                .child(
                    div()
                        .id("frontmatter-error")
                        .text_color(theme::danger())
                        .child(SharedString::from(format!(
                            "These properties can't be shown: {error}"
                        ))),
                )
                .child(
                    div()
                        .id("frontmatter-raw")
                        .debug_selector(|| "frontmatter-raw-text".into())
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(theme::code_background())
                        .text_color(theme::muted())
                        .flex()
                        .flex_col()
                        .children(raw.lines().map(|l| SharedString::from(l.to_owned()))),
                )
                .into_any_element();
        }

        let rows: Vec<AnyElement> = self
            .rows
            .iter()
            .map(|row| self.row_element(row, window, cx))
            .collect();
        root.child(div().flex().flex_col().gap(px(6.0)).children(rows))
            .child(
                div().flex().pt(px(4.0)).child(
                    div()
                        .id("frontmatter-add")
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .text_color(theme::muted())
                        .hover(|s| s.text_color(theme::text()))
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from("Add property"))
                        .on_click(cx.listener(|panel, _, _, cx| panel.add_row(cx)))
                        .child("+")
                        .child("Add property"),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
impl FrontmatterPanel {
    pub fn rows_text(&self, cx: &gpui::App) -> Vec<(String, String)> {
        self.rows
            .iter()
            .map(|r| (r.key.read(cx).text.clone(), r.value.read(cx).text.clone()))
            .collect()
    }

    pub fn field_handle(&self, row: usize, value: bool, cx: &gpui::App) -> FocusHandle {
        let row = &self.rows[row];
        if value {
            row.value.focus_handle(cx)
        } else {
            row.key.focus_handle(cx)
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.properties {
            Properties::Invalid(error) => Some(error),
            Properties::Entries { .. } => None,
        }
    }
}
