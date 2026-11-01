//! Text input from the OS: typed characters, dead keys and IME composition
//! through GPUI's `EntityInputHandler` (US-002 measured the path on each
//! backend). Positions cross this boundary in UTF-16; the buffer is UTF-8.

use std::ops::Range;

use cerata_markdown::{Change, Selection, SelectionRange, Text, Transaction, UserEvent};
use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point};

use super::{Composition, Editor};

/// UTF-16 length of `s`.
fn utf16_len(s: &str) -> usize {
    // One unit per char, two for chars outside the BMP (4-byte UTF-8).
    s.bytes()
        .map(|b| match b {
            0x80..=0xbf => 0,
            0xf0..=0xff => 2,
            _ => 1,
        })
        .sum()
}

pub fn to_utf16(text: &Text, offset: usize) -> usize {
    utf16_len(text.slice(0..text.floor_char_boundary(offset.min(text.len()))))
}

pub fn from_utf16(text: &Text, utf16: usize) -> usize {
    let mut units = 0;
    for (ix, c) in text.as_str().char_indices() {
        if units >= utf16 {
            return ix;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn range_to_utf16(text: &Text, range: &Range<usize>) -> Range<usize> {
    to_utf16(text, range.start)..to_utf16(text, range.end)
}

fn range_from_utf16(text: &Text, range: &Range<usize>) -> Range<usize> {
    from_utf16(text, range.start)..from_utf16(text, range.end)
}

impl Editor {
    /// Starts a composition: the state is saved so the composition never
    /// reaches the undo history.
    fn begin_composition(&mut self, replaced: Range<usize>) {
        if self.composition.is_none() {
            self.composition = Some(Composition {
                state: self.state.clone(),
                heights: self.heights.clone(),
                replaced,
            });
        }
    }

    /// Ends the composition: back to the saved state (a cancel), then the
    /// committed text, if any, as one typed edit.
    fn end_composition(&mut self, committed: Option<&str>, cx: &mut Context<Self>) {
        self.marked_range = None;
        let Some(saved) = self.composition.take() else {
            return;
        };
        self.state = saved.state;
        self.heights = saved.heights;
        match committed {
            Some(text) if !text.is_empty() => {
                let range = saved.replaced;
                let text = text.to_owned();
                self.update_state(cx, |state| {
                    let caret = range.start + text.len();
                    let _ = state.apply(
                        Transaction::new()
                            .change(Change::new(range, text))
                            .select(Selection::cursor(caret))
                            .event(UserEvent::Type)
                            .scroll(),
                    );
                });
            }
            _ => self.on_changed(cx),
        }
    }

    /// Typed text at every selection range (or `range`, from the OS).
    fn type_text(&mut self, range: Option<Range<usize>>, text: &str, cx: &mut Context<Self>) {
        if range.is_none() && text == "-" && self.open_frontmatter(cx) {
            return;
        }
        let text = text.to_owned();
        self.typed = true;
        self.update_state(cx, |state| {
            let ranges: Vec<SelectionRange> = match range {
                Some(range) => vec![SelectionRange::new(range.start, range.end)],
                None => state.selection().ranges().to_vec(),
            };
            let mut changes = Vec::new();
            let mut carets = Vec::new();
            let mut shift: isize = 0;
            for r in &ranges {
                // At a diagram's boundary: on a line of its own.
                let (insert, caret) = match r.is_empty() {
                    true => cerata_markdown::atomic::typed_at(state, r.head, &text),
                    false => None,
                }
                .unwrap_or_else(|| (text.clone(), text.len()));
                let end = r.from() as isize + shift + caret as isize;
                carets.push(SelectionRange::cursor(end.max(0) as usize));
                shift += insert.len() as isize - r.range().len() as isize;
                changes.push(Change::new(r.range(), insert));
            }
            let main = state.selection().main_index().min(carets.len() - 1);
            let _ = state.apply(
                Transaction::new()
                    .changes(changes)
                    .select(Selection::new(carets, main))
                    .event(UserEvent::Type)
                    .scroll(),
            );
        });
    }
}

impl Editor {
    /// `---` typed on a first line holding `--`, in a note without
    /// frontmatter, opens an empty properties panel instead of a rule
    /// (`editor-clipboard.ts:57-78`).
    fn open_frontmatter(&mut self, cx: &mut Context<Self>) -> bool {
        let main = self.state.selection().main();
        let doc = self.state.doc();
        let first_line = doc.split(['\n', '\r']).next().unwrap_or("");
        if self.state.selection().ranges().len() > 1
            || !main.is_empty()
            || main.head != 2
            || first_line != "--"
            || cerata_markdown::frontmatter::detect(doc).is_some()
        {
            return false;
        }
        // An empty block as Writer writes it; the caret after it, where
        // the panel shows.
        let eol = if doc[2..].starts_with("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let replaced = 0..(2 + if doc.len() > 2 { eol.len() } else { 0 });
        let block = cerata_markdown::frontmatter::join(Some(""), "", eol);
        let caret = block.len();
        self.update_state(cx, |state| {
            let _ = state.apply(
                Transaction::new()
                    .change(Change::new(replaced, block))
                    .select(Selection::cursor(caret))
                    .event(UserEvent::Command)
                    .scroll(),
            );
        });
        self.frontmatter
            .update(cx, |panel, cx| panel.focus_first(cx));
        true
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.state.text();
        let range = range_from_utf16(text, &range_utf16);
        actual_range.replace(range_to_utf16(text, &range));
        Some(text.slice(range).to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let main = self.state.selection().main();
        Some(UTF16Selection {
            range: range_to_utf16(self.state.text(), &main.range()),
            reversed: main.head < main.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| range_to_utf16(self.state.text(), range))
    }

    /// The OS accepts the marked text as it is (macOS `unmarkText`).
    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let committed = self
            .marked_range
            .as_ref()
            .map(|range| self.state.text().slice(range.clone()).to_owned());
        self.end_composition(committed.as_deref(), cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.marked_range.is_some() {
            // The composition ends: committed text, or nothing on a cancel
            // (Escape sends an empty commit).
            self.end_composition(Some(new_text), cx);
            return;
        }
        if new_text.is_empty() && range_utf16.is_none() {
            return;
        }
        let range = range_utf16.map(|r| range_from_utf16(self.state.text(), &r));
        self.goal_x = None;
        self.type_text(range, new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.state.text();
        let range = range_utf16
            .as_ref()
            .map(|r| range_from_utf16(text, r))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.state.selection().main().range());
        if new_text.is_empty() {
            self.end_composition(None, cx);
            return;
        }
        let start_utf16 = to_utf16(text, range.start);
        self.begin_composition(range.clone());
        let inserted_utf16 = utf16_len(new_text);
        let new_text = new_text.to_owned();
        let marked = range.start..range.start + new_text.len();
        // The marked text is in the buffer while composing, outside the
        // history: the saved state is what `end_composition` restores.
        self.update_state(cx, |state| {
            let selection = Selection::cursor(marked.end);
            let _ = state.apply(
                Transaction::new()
                    .change(Change::new(range.clone(), new_text.clone()))
                    .select(selection)
                    .event(UserEvent::Compose)
                    .scroll(),
            );
            if let Some(selected) = new_selected_range_utf16 {
                let text = state.text();
                let from = from_utf16(text, start_utf16 + selected.start.min(inserted_utf16));
                let to = from_utf16(text, start_utf16 + selected.end.min(inserted_utf16));
                let _ = state.select(Selection::single(from, to));
            }
        });
        self.marked_range = Some(marked);
    }

    /// Bounds computed from the model, so they are right between an edit
    /// and the next frame (macOS asks right after setting marked text). The
    /// IME places its candidate window from these.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = range_from_utf16(self.state.text(), &range_utf16);
        let start = self.caret_bounds(range.start, window, cx)?;
        let end = self.caret_bounds(range.end, window, cx)?;
        let right = if end.origin.y == start.origin.y {
            end.origin.x.max(start.origin.x)
        } else {
            start.origin.x
        };
        Some(Bounds::from_corners(
            start.origin,
            point(right, start.origin.y + start.size.height),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, element_bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_point(point, window, cx);
        Some(to_utf16(self.state.text(), offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_from_utf16(self.state.text(), &range_utf16);
        self.select(Selection::single(range.start, range.end), cx);
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(to_utf16(self.state.text(), self.state.text().len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_round_trip() {
        let text = Text::new("a\u{e9}\u{65e5}\u{1f600}b");
        assert_eq!(to_utf16(&text, 0), 0);
        assert_eq!(to_utf16(&text, 1), 1);
        assert_eq!(to_utf16(&text, 3), 2, "é is one unit");
        assert_eq!(to_utf16(&text, 6), 3, "日 is one unit");
        assert_eq!(to_utf16(&text, 10), 5, "the emoji is two");
        for offset in [0, 1, 3, 6, 10, 11] {
            assert_eq!(from_utf16(&text, to_utf16(&text, offset)), offset);
        }
    }
}
