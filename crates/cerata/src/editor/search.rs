//! Find and replace in the open note (`editor-search-store.ts:28-125`,
//! `editor-search-overlay.tsx:23-146`, `editor-search-overview.tsx:8-75`,
//! `editor-search-extensions.ts:29-125`): literal, case-insensitive, a
//! `n/total` counter, results marked on the scrollbar (at most 5 000), and a
//! jump that keeps the result 140 px from the edges.

use std::ops::Range;

use cerata_markdown::{Change, EditorState, Selection, Transaction, UserEvent};
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, px,
};

use super::Editor;
use super::text_field::{FieldEvent, TextField};
use super::theme;

/// Scrollbar marks drawn at most (`MAX_OVERVIEW_MATCHES`).
pub const MAX_MARKS: usize = 5000;

/// Every occurrence of `query` in `doc`, literal and case-insensitive.
pub fn find_all(doc: &str, query: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    if query.is_ascii() {
        let needle = query.as_bytes();
        let hay = doc.as_bytes();
        let first = needle[0];
        let (lower, upper) = (first.to_ascii_lowercase(), first.to_ascii_uppercase());
        let mut from = 0;
        while from + needle.len() <= hay.len() {
            let Some(found) = memchr::memchr2(lower, upper, &hay[from..=hay.len() - needle.len()])
            else {
                break;
            };
            let at = from + found;
            if hay[at..at + needle.len()].eq_ignore_ascii_case(needle) {
                out.push(at..at + needle.len());
                from = at + needle.len();
            } else {
                from = at + 1;
            }
        }
        return out;
    }
    // Unicode: a result starts at a char whose lowercase starts the
    // lowercased query; lowercased chars are then compared one by one.
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let first = needle[0];
    let end_of_match = |at: usize| -> Option<usize> {
        let mut k = 0;
        for (ix, c) in doc[at..].char_indices() {
            if k == needle.len() {
                return Some(at + ix);
            }
            for lower in c.to_lowercase() {
                if k >= needle.len() || lower != needle[k] {
                    return None;
                }
                k += 1;
            }
        }
        (k == needle.len()).then_some(doc.len())
    };
    let mut resume = 0;
    for (at, c) in doc.char_indices() {
        if at < resume {
            continue;
        }
        let starts = if c.is_ascii() {
            c.to_ascii_lowercase() == first
        } else {
            c.to_lowercase().next() == Some(first)
        };
        if starts && let Some(end) = end_of_match(at) {
            out.push(at..end);
            resume = end;
        }
    }
    out
}

/// The index of the result the counter shows: the first containing or
/// after `head`, else the first (`computeMatchInfo`).
pub fn current_index(matches: &[Range<usize>], head: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let i = matches.partition_point(|m| m.end < head);
    Some(if i < matches.len() { i } else { 0 })
}

pub enum SearchEvent {
    /// Move to the next (or previous) result.
    Step {
        forward: bool,
    },
    ReplaceNext,
    ReplaceAll,
    Close,
    QueryChanged,
}

pub struct SearchBar {
    pub find: Entity<TextField>,
    pub replace: Entity<TextField>,
    matches: Vec<Range<usize>>,
    current: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SearchEvent> for SearchBar {}

impl SearchBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let find = cx.new(|cx| TextField::new("Find", cx));
        let replace = cx.new(|cx| TextField::new("Replace", cx));
        let subscriptions = vec![
            cx.subscribe(&find, |_, _, event: &FieldEvent, cx| match event {
                FieldEvent::Changed => cx.emit(SearchEvent::QueryChanged),
                FieldEvent::Submit { backward } => {
                    cx.emit(SearchEvent::Step { forward: !backward })
                }
                FieldEvent::Escape => cx.emit(SearchEvent::Close),
            }),
            cx.subscribe(&replace, |_, _, event: &FieldEvent, cx| match event {
                FieldEvent::Changed => {}
                FieldEvent::Submit { .. } => cx.emit(SearchEvent::ReplaceNext),
                FieldEvent::Escape => cx.emit(SearchEvent::Close),
            }),
        ];
        Self {
            find,
            replace,
            matches: Vec::new(),
            current: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn query(&self, cx: &gpui::App) -> String {
        self.find.read(cx).text.clone()
    }

    pub fn matches(&self) -> &[Range<usize>] {
        &self.matches
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// Results touching `range`, with their index.
    pub fn matches_in(&self, range: Range<usize>) -> Vec<(usize, Range<usize>)> {
        let start = self.matches.partition_point(|m| m.end < range.start);
        self.matches[start..]
            .iter()
            .take_while(|m| m.start <= range.end)
            .enumerate()
            .map(|(i, m)| (start + i, m.clone()))
            .collect()
    }

    /// The results drawn on the scrollbar.
    pub fn marks(&self) -> impl Iterator<Item = (usize, Range<usize>)> + '_ {
        self.matches.iter().take(MAX_MARKS).cloned().enumerate()
    }

    /// Recounts on every document or selection change, or query edit.
    pub fn document_changed(&mut self, state: &EditorState, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.matches = find_all(state.doc(), &query);
        self.current = current_index(&self.matches, state.selection().main().head);
        let invalid = !query.is_empty() && self.matches.is_empty();
        self.find.update(cx, |field, cx| {
            if field.invalid != invalid {
                field.invalid = invalid;
                cx.notify();
            }
        });
        cx.notify();
    }

    pub fn counter(&self) -> String {
        match self.current {
            Some(i) => format!("{}/{}", i + 1, self.matches.len()),
            None => "0/0".to_owned(),
        }
    }
}

impl Focusable for SearchBar {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.find.read(cx).focus_handle.clone()
    }
}

impl Render for SearchBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let counter: SharedString = self.counter().into();
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(8.0))
                .h(px(26.0))
                .flex()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(theme::border())
                .bg(theme::background())
                .text_size(px(12.0))
                .cursor_pointer()
                .child(label)
        };
        div()
            .key_context("SearchBar")
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(8.0))
            .rounded(px(8.0))
            .bg(theme::overlay())
            .border_1()
            .border_color(theme::border())
            .text_size(px(13.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().w(px(220.0)).child(self.find.clone()))
                    .child(
                        div()
                            .id("search-counter")
                            .w(px(64.0))
                            .text_color(theme::muted())
                            .child(counter),
                    )
                    .child(button("search-prev", "\u{2191}").on_click(
                        cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Step { forward: false })),
                    ))
                    .child(button("search-next", "\u{2193}").on_click(
                        cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Step { forward: true })),
                    ))
                    .child(
                        button("search-close", "\u{2715}")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Close))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().w(px(220.0)).child(self.replace.clone()))
                    .child(
                        button("search-replace", "Replace")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::ReplaceNext))),
                    )
                    .child(
                        button("search-replace-all", "Replace all")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::ReplaceAll))),
                    ),
            )
    }
}

impl Editor {
    /// Cmd+F: opens the bar (pre-filled by a one-line selection) and
    /// focuses its find field.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let main = self.state.selection().main();
        let prefill = (!main.is_empty())
            .then(|| self.state.text().slice(main.range()).to_owned())
            .filter(|text| !text.contains('\n'));
        let search = match &self.search {
            Some(search) => search.clone(),
            None => {
                let search = cx.new(SearchBar::new);
                cx.subscribe_in(&search, window, Self::on_search_event)
                    .detach();
                self.search = Some(search.clone());
                search
            }
        };
        let find = search.read(cx).find.clone();
        let previous = std::mem::take(&mut self.last_query);
        find.update(cx, |field, cx| match &prefill {
            Some(text) => field.set_text(text, cx),
            None if field.text.is_empty() => field.set_text(&previous, cx),
            None => {
                field.selection = 0..field.text.len();
                cx.notify();
            }
        });
        search.update(cx, |search, cx| search.document_changed(&self.state, cx));
        window.focus(&find.read(cx).focus_handle.clone(), cx);
        cx.notify();
    }

    /// Escape: closes the bar and gives the focus back to the text.
    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = self.search.take() {
            self.last_query = search.read(cx).query(cx);
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// The editor's tab became active or inactive; an inactive tab closes
    /// its search (`use-close-editor-search-when-inactive.ts`).
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active && self.search.take().is_some() {
            cx.notify();
        }
    }

    fn on_search_event(
        &mut self,
        _: &Entity<SearchBar>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Step { forward } => self.search_step(*forward, window, cx),
            SearchEvent::ReplaceNext => self.replace_next(cx),
            SearchEvent::ReplaceAll => self.replace_all(cx),
            SearchEvent::Close => self.close_search(window, cx),
            SearchEvent::QueryChanged => {
                if let Some(search) = self.search.clone() {
                    search.update(cx, |search, cx| search.document_changed(&self.state, cx));
                }
                cx.notify();
            }
        }
    }

    /// Cmd+G, Cmd+Shift+G, Enter, Shift+Enter: selects the next (or
    /// previous) result after (or before) the selection, wrapping around.
    pub fn search_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.search.clone() else {
            self.open_search(window, cx);
            return;
        };
        let matches = search.read(cx).matches().to_vec();
        if matches.is_empty() {
            return;
        }
        let main = self.state.selection().main();
        let target = if forward {
            matches
                .iter()
                .find(|m| m.start >= main.to() && *m != &main.range())
                .unwrap_or(&matches[0])
        } else {
            matches
                .iter()
                .rev()
                .find(|m| m.end <= main.from() && *m != &main.range())
                .unwrap_or(&matches[matches.len() - 1])
        }
        .clone();
        self.jump_to(target, cx);
    }

    fn jump_to(&mut self, target: Range<usize>, cx: &mut Context<Self>) {
        self.select(Selection::single(target.start, target.end), cx);
        self.reveal(target.start, cx);
    }

    /// A click on a scrollbar mark jumps to its result.
    pub(super) fn click_mark(&mut self, pos: usize, cx: &mut Context<Self>) {
        let Some(search) = self.search.clone() else {
            return;
        };
        if let Some(m) = search
            .read(cx)
            .matches()
            .iter()
            .find(|m| m.start == pos)
            .cloned()
        {
            self.jump_to(m, cx);
        }
    }

    /// Enter in the replace field: replaces the selected result, then
    /// selects the next one.
    pub fn replace_next(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.clone() else {
            return;
        };
        let replacement = search.read(cx).replace.read(cx).text.clone();
        let matches = search.read(cx).matches().to_vec();
        let main = self.state.selection().main();
        let Some(selected) = matches.iter().find(|m| **m == main.range()).cloned() else {
            if let Some(next) = matches
                .iter()
                .find(|m| m.start >= main.to())
                .or(matches.first())
            {
                self.jump_to(next.clone(), cx);
            }
            return;
        };
        let caret = selected.start + replacement.len();
        self.update_state(cx, |state| {
            let _ = state.apply(
                Transaction::new()
                    .change(Change::new(selected, replacement.clone()))
                    .select(Selection::cursor(caret))
                    .event(UserEvent::Command)
                    .scroll(),
            );
        });
        let next = search
            .read(cx)
            .matches()
            .iter()
            .find(|m| m.start >= caret)
            .cloned();
        if let Some(next) = next {
            self.jump_to(next, cx);
        }
    }

    /// Replaces every result in one transaction: one undo step.
    pub fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.clone() else {
            return;
        };
        let replacement = search.read(cx).replace.read(cx).text.clone();
        let changes: Vec<Change> = search
            .read(cx)
            .matches()
            .iter()
            .map(|m| Change::new(m.clone(), replacement.clone()))
            .collect();
        if changes.is_empty() {
            return;
        }
        self.update_state(cx, |state| {
            let _ = state.apply(
                Transaction::new()
                    .changes(changes)
                    .event(UserEvent::Command)
                    .scroll(),
            );
        });
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init, reason = "expected match lists")]
mod tests {
    use super::*;

    #[test]
    fn literal_and_case_insensitive() {
        let doc = "Foo foo FOO f.o fOo";
        assert_eq!(find_all(doc, "foo"), [0..3, 4..7, 8..11, 16..19]);
        assert_eq!(find_all(doc, "f.o"), [12..15], "literal, not a regex");
        assert_eq!(find_all("aaa", "aa"), [0..2], "non-overlapping");
        assert_eq!(find_all("Été été", "ÉTÉ"), [0..5, 6..11]);
        assert!(find_all(doc, "").is_empty());
    }

    #[test]
    fn the_counter_starts_at_the_result_at_or_after_the_caret() {
        let matches = [0..3, 10..13, 20..23];
        assert_eq!(current_index(&matches, 0), Some(0));
        assert_eq!(current_index(&matches, 11), Some(1));
        assert_eq!(current_index(&matches, 14), Some(2));
        assert_eq!(current_index(&matches, 30), Some(0), "wraps to the first");
        assert_eq!(current_index(&[], 3), None);
    }

    #[test]
    fn counting_a_megabyte_finds_every_match() {
        let doc = "the quick brown fox jumps over the lazy dog. ".repeat(24_000);
        assert_eq!(find_all(&doc, "LAZY").len(), 24_000);
    }

    /// The criterion is about the shipped build:
    /// `cargo test -p cerata --release -- --ignored`.
    #[test]
    #[ignore = "release measurement"]
    fn counting_a_megabyte_stays_under_five_milliseconds() {
        let doc = "the quick brown fox jumps over the lazy dog. ".repeat(24_000);
        assert!(doc.len() > 1_000_000);
        let start = std::time::Instant::now();
        let matches = find_all(&doc, "lazy");
        let elapsed = start.elapsed();
        assert_eq!(matches.len(), 24_000);
        assert!(elapsed.as_secs_f64() < 0.005, "{elapsed:?}");
        // An accented query takes the Unicode path.
        let doc = "le café brûlé du chien paresseux. ".repeat(31_000);
        let start = std::time::Instant::now();
        let matches = find_all(&doc, "CAFÉ");
        let elapsed = start.elapsed();
        assert!(doc.len() > 1_000_000);
        assert_eq!(matches.len(), 31_000);
        assert!(elapsed.as_secs_f64() < 0.005, "accented query: {elapsed:?}");
    }
}
