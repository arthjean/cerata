use std::path::PathBuf;
use std::time::Instant;

use cerata_core::history::{self, DeletedNote, VersionInfo};
use cerata_core::merge::{DiffLine, line_diff};
use cerata_core::store::{ContentHash, Store};
use chrono::TimeZone;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    UniformListScrollHandle, Window, div, prelude::FluentBuilder, px, uniform_list,
};

use crate::editor::text_field::{FieldEvent, TextField};
use crate::editor::theme;

pub const EMPTY: &str = "No history yet. Cerata saves a version 30 seconds after you stop typing.";
pub const NO_DELETED: &str = "No deleted notes with history.";
const ROW_HEIGHT: f32 = 44.0;
const LIST_WIDTH: f32 = 300.0;

#[derive(Clone, Debug, PartialEq)]
pub enum HistoryEvent {
    Close,
    Restore { path: PathBuf, bytes: Vec<u8> },
    RestoreDeleted { note: DeletedNote, bytes: Vec<u8> },
    Copy(String),
    PurgeNote { note: i64 },
    PurgeBefore { ts: i64 },
    Notice(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Note,
    Deleted,
}

#[derive(Clone, Debug)]
enum Row {
    Day(String),
    Version(usize),
    Deleted(usize),
}

pub struct HistoryPanel {
    pub focus_handle: FocusHandle,
    store: Option<Store>,
    pub unavailable: Option<String>,
    pub view: View,
    pub path: Option<PathBuf>,
    relative: Option<String>,
    title: String,
    current: String,
    pub note: Option<i64>,
    pub versions: Vec<VersionInfo>,
    pub deleted: Vec<DeletedNote>,
    rows: Vec<Row>,
    pub selected: Option<usize>,
    pub preview: Option<Result<String, String>>,
    preview_of: Option<ContentHash>,
    preview_bytes: Option<Vec<u8>>,
    pub show_changes: bool,
    pub loading: bool,
    opened_at: Instant,
    pub load_time: Option<std::time::Duration>,
    pub date_field: Option<Entity<TextField>>,
    date_error: Option<String>,
    scroll: UniformListScrollHandle,
    _tasks: Vec<Task<()>>,
    _field_events: Option<Subscription>,
}

impl EventEmitter<HistoryEvent> for HistoryPanel {}

impl Focusable for HistoryPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn local(ts: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::Local.timestamp_opt(ts, 0).single()
}

pub fn day_label(ts: i64) -> String {
    local(ts).map_or_else(String::new, |time| {
        time.format("%A, %B %-d, %Y").to_string()
    })
}

pub fn short_day_label(ts: i64) -> String {
    local(ts).map_or_else(String::new, |time| time.format("%b %-d, %Y").to_string())
}

pub fn time_label(ts: i64) -> String {
    local(ts).map_or_else(String::new, |time| time.format("%H:%M").to_string())
}

pub fn parse_day(text: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    chrono::Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|time| time.timestamp())
}

impl HistoryPanel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Option<Store>,
        unavailable: Option<String>,
        path: Option<PathBuf>,
        relative: Option<String>,
        title: String,
        current: String,
        view: View,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            store,
            unavailable,
            view,
            path,
            relative,
            title,
            current,
            note: None,
            versions: Vec::new(),
            deleted: Vec::new(),
            rows: Vec::new(),
            selected: None,
            preview: None,
            preview_of: None,
            preview_bytes: None,
            show_changes: false,
            loading: true,
            opened_at: Instant::now(),
            load_time: None,
            date_field: None,
            date_error: None,
            scroll: UniformListScrollHandle::new(),
            _tasks: Vec::new(),
            _field_events: None,
        };
        panel.reload(cx);
        panel
    }

    pub fn is_available(&self) -> bool {
        self.unavailable.is_none() && self.store.is_some()
    }

    pub fn set_current(&mut self, current: String, cx: &mut Context<Self>) {
        self.current = current;
        cx.notify();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone().filter(|_| self.unavailable.is_none()) else {
            self.loading = false;
            cx.notify();
            return;
        };
        self.loading = true;
        self.opened_at = Instant::now();
        let view = self.view;
        let relative = self.relative.clone();
        let kept_seq = self.selected_version().map(|v| v.seq);
        let task = cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    match view {
                        View::Note => {
                            let found = match relative {
                                Some(relative) => history::versions_of_path(&store, &relative),
                                None => Ok(None),
                            };
                            found.map(|found| (found, Vec::new()))
                        }
                        View::Deleted => history::deleted_notes(&store).map(|d| (None, d)),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.load_time = Some(this.opened_at.elapsed());
                match read {
                    Ok((found, deleted)) => {
                        let (note, versions) =
                            found.map_or((None, Vec::new()), |(id, v)| (Some(id), v));
                        this.note = note;
                        this.versions = versions;
                        this.deleted = deleted;
                    }
                    Err(error) => {
                        log::warn!("history unavailable: {error}");
                        this.unavailable = Some(format!("History is unavailable: {error}"));
                    }
                }
                this.rebuild_rows();
                let keep = match this.view {
                    View::Note => {
                        kept_seq.and_then(|seq| this.versions.iter().position(|v| v.seq == seq))
                    }
                    View::Deleted => this.selected.filter(|s| *s < this.len()),
                };
                this.selected = None;
                this.select(keep.or((this.len() > 0).then_some(0)), cx);
                cx.notify();
            });
        });
        self._tasks.push(task);
        cx.notify();
    }

    fn len(&self) -> usize {
        match self.view {
            View::Note => self.versions.len(),
            View::Deleted => self.deleted.len(),
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        match self.view {
            View::Note => {
                let mut day = None;
                for (ix, version) in self.versions.iter().enumerate() {
                    let label = day_label(version.ts);
                    if day.as_ref() != Some(&label) {
                        self.rows.push(Row::Day(label.clone()));
                        day = Some(label);
                    }
                    self.rows.push(Row::Version(ix));
                }
            }
            View::Deleted => {
                self.rows = (0..self.deleted.len()).map(Row::Deleted).collect();
            }
        }
    }

    fn row_of(&self, selected: usize) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            Row::Version(ix) | Row::Deleted(ix) => *ix == selected,
            Row::Day(_) => false,
        })
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.selected = None;
            self.preview = None;
            self.preview_of = None;
            self.preview_bytes = None;
            self.show_changes = false;
            self.rows.clear();
            self.reload(cx);
        }
    }

    pub fn select(&mut self, selected: Option<usize>, cx: &mut Context<Self>) {
        self.selected = selected;
        if let Some(row) = selected.and_then(|s| self.row_of(s)) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        let hash = match (self.view, selected) {
            (View::Note, Some(s)) => self.versions.get(s).map(|v| v.hash),
            (View::Deleted, Some(s)) => self.deleted.get(s).map(|d| d.head),
            _ => None,
        };
        if hash == self.preview_of {
            cx.notify();
            return;
        }
        self.preview_of = hash;
        self.preview = None;
        self.preview_bytes = None;
        let (Some(hash), Some(store)) = (hash, self.store.clone()) else {
            cx.notify();
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move { history::content(&store, &hash) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.preview_of != Some(hash) {
                    return;
                }
                this.preview = Some(match &bytes {
                    Ok(Some(bytes)) => cerata_core::fs::text::decode(bytes)
                        .map(|(text, _)| text)
                        .map_err(|_| "This version is not UTF-8 text.".to_owned()),
                    Ok(None) => Err("This version is no longer in the history.".into()),
                    Err(error) => Err(format!("This version could not be read: {error}")),
                });
                this.preview_bytes = bytes.ok().flatten();
                cx.notify();
            });
        });
        self._tasks.push(task);
        cx.notify();
    }

    pub fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(s) if down => (s + 1).min(len - 1),
            Some(s) => s.saturating_sub(1),
        };
        self.select(Some(next), cx);
    }

    pub fn selected_version(&self) -> Option<&VersionInfo> {
        (self.view == View::Note)
            .then(|| self.selected.and_then(|s| self.versions.get(s)))
            .flatten()
    }

    pub fn toggle_changes(&mut self, cx: &mut Context<Self>) {
        if self.view == View::Note {
            self.show_changes = !self.show_changes;
            cx.notify();
        }
    }

    pub fn restore(&mut self, cx: &mut Context<Self>) {
        let Some(bytes) = self.preview_bytes.clone() else {
            return;
        };
        match self.view {
            View::Note => {
                if let Some(path) = self.path.clone() {
                    cx.emit(HistoryEvent::Restore { path, bytes });
                }
            }
            View::Deleted => {
                if let Some(note) = self.selected.and_then(|s| self.deleted.get(s)).cloned() {
                    cx.emit(HistoryEvent::RestoreDeleted { note, bytes });
                }
            }
        }
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(Ok(text)) = &self.preview {
            cx.emit(HistoryEvent::Copy(text.clone()));
        }
    }

    pub fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        let (Some(version), Some(store)) = (self.selected_version().cloned(), self.store.clone())
        else {
            return;
        };
        let token = cerata_core::epoch::EpochCounter::new().token();
        let pinned = !version.pinned;
        let task = cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    history::set_pinned(&store, &token, version.note, version.seq, pinned)
                        .recv()
                        .map_err(|_| "the store stopped".to_owned())
                        .and_then(|result| result.map_err(|error| error.to_string()))
                })
                .await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(_) => this.reload(cx),
                Err(error) => cx.emit(HistoryEvent::Notice(format!("Version not pinned: {error}"))),
            });
        });
        self._tasks.push(task);
    }

    pub fn purge_note(&mut self, cx: &mut Context<Self>) {
        if let Some(note) = self.note.filter(|_| self.view == View::Note) {
            cx.emit(HistoryEvent::PurgeNote { note });
        }
    }

    pub fn ask_date(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.date_field.get_or_insert_with(|| {
            cx.new(|cx| TextField::new("YYYY-MM-DD", cx).labeled("Delete history before this date"))
        });
        let field = field.clone();
        self._field_events = Some(cx.subscribe(
            &field,
            |this, _, event: &FieldEvent, cx| match event {
                FieldEvent::Submit { .. } => this.submit_date(cx),
                FieldEvent::Escape => {
                    this.date_field = None;
                    this.date_error = None;
                    cx.notify();
                }
                _ => {}
            },
        ));
        let handle = field.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    pub fn submit_date(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &self.date_field else {
            return;
        };
        let text = field.read(cx).text.clone();
        match parse_day(&text) {
            Some(ts) => {
                self.date_error = None;
                cx.emit(HistoryEvent::PurgeBefore { ts });
            }
            None => self.date_error = Some("Enter a date as YYYY-MM-DD.".into()),
        }
        cx.notify();
    }

    pub fn close_date(&mut self, cx: &mut Context<Self>) {
        self.date_field = None;
        self.date_error = None;
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .date_field
            .as_ref()
            .is_some_and(|field| field.read(cx).focus_handle.is_focused(window))
        {
            return;
        }
        match event.keystroke.key.as_str() {
            "down" => self.move_selection(true, cx),
            "up" => self.move_selection(false, cx),
            "enter" => {
                let selected = self.selected.or((self.len() > 0).then_some(0));
                self.preview_of = None;
                self.select(selected, cx);
            }
            "escape" => cx.emit(HistoryEvent::Close),
            _ => return,
        }
        cx.stop_propagation();
    }
}

fn button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    cx: &mut Context<HistoryPanel>,
    run: impl Fn(&mut HistoryPanel, &mut Window, &mut Context<HistoryPanel>) + 'static,
) -> AnyElement {
    crate::a11y::button(div().id(id), label)
        .min_h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .border_color(theme::border())
        .bg(theme::background())
        .text_size(px(13.0))
        .when(enabled, |b| {
            b.cursor_pointer()
                .hover(|s| s.bg(theme::row_hover()))
                .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
        })
        .when(!enabled, |b| b.tab_stop(false).text_color(theme::muted()))
        .child(label)
        .into_any_element()
}

fn message(text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .p(px(24.0))
        .text_size(px(14.0))
        .text_color(theme::muted())
        .child(text.into())
        .into_any_element()
}

fn diff_view(lines: Vec<DiffLine>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .text_size(px(13.0))
        .children(lines.into_iter().map(|line| {
            let (sign, text, background) = match line {
                DiffLine::Same(text) => (" ", text, None),
                DiffLine::Removed(text) => ("\u{2212}", text, Some(theme::danger().opacity(0.14))),
                DiffLine::Added(text) => ("+", text, Some(theme::accent().opacity(0.14))),
            };
            div()
                .flex()
                .min_h(px(18.0))
                .when_some(background, |d, bg| d.bg(bg))
                .child(
                    div()
                        .w(px(20.0))
                        .flex_none()
                        .text_color(theme::text())
                        .child(sign),
                )
                .child(div().flex_1().min_w_0().whitespace_normal().child(text))
        }))
        .into_any_element()
}

impl Render for HistoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let note_view = self.view == View::Note;
        let heading = match self.view {
            View::Note => format!("History of {}", self.title),
            View::Deleted => "Deleted Notes".to_owned(),
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.0))
                    .child(heading),
            )
            .child(if note_view {
                button(
                    "history-deleted",
                    "Deleted Notes",
                    self.is_available(),
                    cx,
                    |this, _, cx| this.set_view(View::Deleted, cx),
                )
            } else {
                button(
                    "history-note",
                    "Back to Note",
                    self.path.is_some(),
                    cx,
                    |this, _, cx| this.set_view(View::Note, cx),
                )
            })
            .child(button("history-close", "Close", true, cx, |_, _, cx| {
                cx.emit(HistoryEvent::Close)
            }));

        let body: AnyElement = if let Some(reason) = &self.unavailable {
            message(reason.clone())
        } else if self.store.is_none() {
            message("History is unavailable for this note.")
        } else if self.loading && self.rows.is_empty() {
            message("Loading history\u{2026}")
        } else if self.rows.is_empty() {
            message(if note_view { EMPTY } else { NO_DELETED })
        } else {
            let list = uniform_list(
                "history-list",
                self.rows.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .filter_map(|ix| this.rows.get(ix).cloned().map(|row| (ix, row)))
                        .map(|(ix, row)| this.render_row(ix, row, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .h_full();
            let selected = self.selected.is_some() && self.preview_bytes.is_some();
            let pinned = self.selected_version().is_some_and(|v| v.pinned);
            let actions = div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .px(px(16.0))
                .py(px(8.0))
                .border_b_1()
                .border_color(theme::border())
                .child(button(
                    "history-restore",
                    "Restore",
                    selected,
                    cx,
                    |this, _, cx| this.restore(cx),
                ))
                .child(button(
                    "history-copy",
                    "Copy",
                    selected,
                    cx,
                    |this, _, cx| this.copy(cx),
                ))
                .when(note_view, |d| {
                    d.child(button(
                        "history-pin",
                        if pinned { "Unpin" } else { "Pin Version" },
                        self.selected_version().is_some(),
                        cx,
                        |this, _, cx| this.toggle_pin(cx),
                    ))
                    .child(button(
                        "history-changes",
                        if self.show_changes {
                            "Show Version"
                        } else {
                            "Show Changes"
                        },
                        selected,
                        cx,
                        |this, _, cx| this.toggle_changes(cx),
                    ))
                });
            let deleted_path = (!note_view)
                .then(|| self.selected.and_then(|s| self.deleted.get(s)))
                .flatten()
                .map(|d| d.path.clone());
            let preview: AnyElement = match &self.preview {
                None if self.selected.is_some() => message("Loading\u{2026}"),
                None => message("Select a version to preview it."),
                Some(Err(error)) => message(error.clone()),
                Some(Ok(text)) if self.show_changes && note_view => {
                    diff_view(line_diff(text, &self.current))
                }
                Some(Ok(text)) => div()
                    .text_size(px(13.0))
                    .children(text.lines().map(|line| {
                        div()
                            .min_h(px(18.0))
                            .whitespace_normal()
                            .child(line.to_owned())
                    }))
                    .into_any_element(),
            };
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex_none()
                        .w(px(LIST_WIDTH))
                        .h_full()
                        .border_r_1()
                        .border_color(theme::border())
                        .child(list),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(actions)
                        .child(
                            div()
                                .id("history-preview")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px(px(16.0))
                                .py(px(8.0))
                                .children(deleted_path.map(|path| {
                                    div()
                                        .pb(px(8.0))
                                        .text_size(px(12.0))
                                        .text_color(theme::muted())
                                        .whitespace_normal()
                                        .child(path)
                                }))
                                .child(preview),
                        ),
                )
                .into_any_element()
        };

        let footer = (self.is_available() && note_view).then(|| {
            let has_versions = !self.versions.is_empty();
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .px(px(16.0))
                .py(px(8.0))
                .border_t_1()
                .border_color(theme::border())
                .child(button(
                    "history-purge-note",
                    "Delete History of This Note",
                    has_versions,
                    cx,
                    |this, _, cx| this.purge_note(cx),
                ))
                .child(button(
                    "history-purge-before",
                    "Delete History Before\u{2026}",
                    true,
                    cx,
                    |this, window, cx| this.ask_date(window, cx),
                ))
                .children(self.date_field.clone().map(|field| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(div().w(px(140.0)).child(field))
                        .child(button(
                            "history-purge-date",
                            "Delete Versions",
                            true,
                            cx,
                            |this, _, cx| this.submit_date(cx),
                        ))
                }))
                .children(self.date_error.clone().map(|error| {
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::danger())
                        .child(error)
                }))
        });

        div()
            .id("history-panel")
            .key_context("HistoryPanel")
            .track_focus(&self.focus_handle)
            .role(gpui::Role::Dialog)
            .aria_label("Note history")
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.on_key(event, window, cx)
                }),
            )
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(theme::background())
            .child(header)
            .child(body)
            .children(footer)
    }
}

impl HistoryPanel {
    fn render_row(&self, ix: usize, row: Row, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Day(label) => div()
                .id(("history-day", ix))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px(px(16.0))
                .flex()
                .items_end()
                .pb(px(6.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::muted())
                .child(label)
                .into_any_element(),
            Row::Version(index) | Row::Deleted(index) => {
                let (primary, secondary) = match self.view {
                    View::Note => {
                        let Some(v) = self.versions.get(index) else {
                            return div().into_any_element();
                        };
                        let pin = if v.pinned { " \u{b7} Pinned" } else { "" };
                        (
                            format!("{} \u{b7} {}{pin}", time_label(v.ts), v.cause.label()),
                            v.device_name.clone(),
                        )
                    }
                    View::Deleted => {
                        let Some(d) = self.deleted.get(index) else {
                            return div().into_any_element();
                        };
                        let versions = if d.versions == 1 {
                            "1 version".to_owned()
                        } else {
                            format!("{} versions", d.versions)
                        };
                        (
                            d.path.clone(),
                            format!(
                                "Deleted {} {} \u{b7} {versions}",
                                short_day_label(d.deleted_at),
                                time_label(d.deleted_at)
                            ),
                        )
                    }
                };
                let selected = self.selected == Some(index);
                let name = format!("{primary}, {secondary}");
                crate::a11y::control(div().id(("history-row", ix)), gpui::Role::ListItem, name)
                    .aria_selected(selected)
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px(px(16.0))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::row_active()))
                    .when(!selected, |d| d.hover(|s| s.bg(theme::row_hover())))
                    .child(div().text_size(px(13.0)).text_ellipsis().child(primary))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::muted())
                            .text_ellipsis()
                            .child(secondary),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(Some(index), cx);
                        window.focus(&this.focus_handle, cx);
                    }))
                    .into_any_element()
            }
        }
    }
}
