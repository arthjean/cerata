//! Arrow up and down next to a widget: the caret enters the folded block so
//! its source can be edited (`revealBlockOnArrow.ts:44-146`). When nothing
//! applies, the caller performs a plain vertical move.

use crate::decorate::{Decoration, Widget};
use crate::selection::Selection;
use crate::state::EditorState;
use crate::text::Text;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vertical {
    Up,
    Down,
}

/// Where the caret goes when `direction` is pressed in `state`, if a widget
/// takes the move; `None` leaves it to the regular vertical motion.
pub fn reveal_block_on_arrow(state: &EditorState, direction: Vertical) -> Option<usize> {
    let decorations = state.decorations();
    let widgets: Vec<_> = decorations
        .iter()
        .filter(|d| d.widget().is_some_and(Widget::is_fold))
        .map(Decoration::range)
        .collect();
    let text = state.text();
    let head = state.selection().main().head;
    let target = at_widget_boundary(text, &widgets, head, direction)
        .or_else(|| on_adjacent_line(text, &widgets, head, direction))?;
    Some(reveal_at(state, head, target))
}

/// One position before `pos`, a line break counting as one.
fn before(text: &Text, pos: usize) -> Option<usize> {
    let s = text.as_str();
    if pos >= 2 && s.get(pos - 2..pos) == Some("\r\n") {
        return Some(pos - 2);
    }
    s.get(..pos)?
        .chars()
        .next_back()
        .map(|c| pos - c.len_utf8())
}

fn after(text: &Text, pos: usize) -> Option<usize> {
    let s = text.as_str();
    if s.get(pos..pos + 2) == Some("\r\n") {
        return Some(pos + 2);
    }
    s.get(pos..)?.chars().next().map(|c| pos + c.len_utf8())
}

/// The caret sits right outside a widget (`maybeRevealAtWidgetBoundary`).
fn at_widget_boundary(
    text: &Text,
    widgets: &[std::ops::Range<usize>],
    head: usize,
    direction: Vertical,
) -> Option<usize> {
    widgets.iter().find_map(|w| match direction {
        Vertical::Down if before(text, w.start) == Some(head) => Some(w.start),
        Vertical::Up if after(text, w.end) == Some(head) => Some(w.end),
        _ => None,
    })
}

fn is_blank(line: &str) -> bool {
    line.bytes().all(|b| b == b' ' || b == b'\t')
}

/// Start of the whitespace run ending at the start of line `number`
/// (`whitespaceRunStart`).
fn run_start(text: &Text, number: usize) -> usize {
    let mut number = number;
    let mut from = text.line(number).from;
    while number > 0 {
        let prev = text.line(number - 1);
        let content = text.slice(prev.range());
        if !is_blank(content) {
            return prev.from + content.trim_end_matches([' ', '\t']).len();
        }
        from = prev.from;
        number -= 1;
    }
    from
}

/// End of the whitespace run starting at the end of line `number`
/// (`whitespaceRunEnd`).
fn run_end(text: &Text, number: usize) -> usize {
    let mut number = number;
    let mut to = text.line(number).to;
    while number + 1 < text.line_count() {
        let next = text.line(number + 1);
        let content = text.slice(next.range());
        if !is_blank(content) {
            return next.from + (content.len() - content.trim_start_matches([' ', '\t']).len());
        }
        to = next.to;
        number += 1;
    }
    to
}

/// The nearest widget separated from the caret's line by whitespace only
/// (`revealWidgetOnAdjacentLine`).
fn on_adjacent_line(
    text: &Text,
    widgets: &[std::ops::Range<usize>],
    head: usize,
    direction: Vertical,
) -> Option<usize> {
    let line = text.line_at(head);
    match direction {
        Vertical::Up => {
            let start = run_start(text, line.number);
            if start == line.from {
                return None;
            }
            widgets
                .iter()
                .filter(|w| w.start <= line.from && w.end >= start && w.end < line.from)
                .map(|w| w.end)
                .max()
        }
        Vertical::Down => {
            let end = run_end(text, line.number);
            if end == line.to {
                return None;
            }
            widgets
                .iter()
                .filter(|w| w.start <= end && w.end >= line.to && w.start > line.to)
                .map(|w| w.start)
                .min()
        }
    }
}
// The caret on the target line in the column it had; at the block's
/// boundary when that column would stay hidden under a widget.
fn reveal_at(state: &EditorState, head: usize, target: usize) -> usize {
    let text = state.text();
    let column = head - text.line_at(head).from;
    let line = text.line_at(target);
    let in_column = text.floor_char_boundary((line.from + column).min(line.to));
    let probe = Selection::cursor(in_column);
    let hidden = crate::decorate::decorate(state.tree(), text, &probe, None).iter().any(|d| {
        matches!(d, Decoration::Replace { range, widget, .. } if widget.is_fold() && range.start < in_column && in_column < range.end)
    });
    if hidden { target } else { in_column }
}

impl EditorState {
    /// Arrow up or down: enters an adjacent widget's source and returns
    /// `true`, or returns `false` for the caller's plain vertical move.
    pub fn reveal_block_on_arrow(&mut self, direction: Vertical) -> bool {
        let Some(pos) = reveal_block_on_arrow(self, direction) else {
            return false;
        };
        let transaction = crate::state::Transaction::new()
            .select(Selection::cursor(pos))
            .event(crate::history::UserEvent::Select)
            .scroll();
        self.apply(transaction).is_ok()
    }
}
