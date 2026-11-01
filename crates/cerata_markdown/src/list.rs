//! List editing: the caret guard on list prefixes, Enter, Backspace, Tab and
//! Shift-Tab, the checkbox toggle and the list rendering geometry.
//!
//! Ported from prosemark-core `list/index.ts` (MIT). Nesting targets come
//! from the syntax tree, as in Writer, so loose lists, ordered parents and
//! tab indentation behave per CommonMark. Two of Writer's open bugs are fixed
//! (`TODOS.md:95-96`): Backspace on an empty item two levels deep outdents
//! it instead of leaving a line of spaces, and nested ordered items get the
//! same depth-aware indent as bullets instead of showing their source spaces.

use std::ops::Range;

use crate::change::{Bias, Change, ChangeSet};
use crate::history::UserEvent;
use crate::selection::{Selection, SelectionRange};
use crate::state::{EditorState, Transaction};
use crate::text::{Line, Text};
use crate::tree::{MarkdownTree, Node, NodeKind};

/// Width of one nesting level and of the marker column, in `ch`.
pub const LIST_UNIT_CH: usize = 3;
/// Offset of the checkbox's inner character from the marker: `- [x]`.
const TASK_INNER_OFFSET: usize = 3;

/// A bullet or task line's prefix: indent, marker, one space or tab, and
/// the optional checkbox (`BULLET_TASK_LINE_RE`, `list/index.ts:167`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BulletTaskLine {
    pub line_from: usize,
    pub marker_from: usize,
    pub body_from: usize,
    pub indent_len: usize,
    pub marker_len: usize,
    pub is_task: bool,
}

fn is_blank_byte(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// `^([ \t]*)[-+*][ \t](\[[ xX]\][ \t])?` on one line.
pub fn parse_bullet_task_line(line_from: usize, line: &str) -> Option<BulletTaskLine> {
    let b = line.as_bytes();
    let indent = b.iter().take_while(|c| is_blank_byte(**c)).count();
    if !matches!(b.get(indent), Some(b'-' | b'+' | b'*'))
        || !b.get(indent + 1).copied().is_some_and(is_blank_byte)
    {
        return None;
    }
    let mut end = indent + 2;
    let is_task = b.get(end) == Some(&b'[')
        && matches!(b.get(end + 1), Some(b' ' | b'x' | b'X'))
        && b.get(end + 2) == Some(&b']')
        && b.get(end + 3).copied().is_some_and(is_blank_byte);
    if is_task {
        end += 4;
    }
    Some(BulletTaskLine {
        line_from,
        marker_from: line_from + indent,
        body_from: line_from + end,
        indent_len: indent,
        marker_len: end - indent,
        is_task,
    })
}

fn line_str<'a>(text: &'a Text, line: &Line) -> &'a str {
    text.slice(line.range())
}

fn parse_at(text: &Text, pos: usize) -> Option<BulletTaskLine> {
    let line = text.line_at(pos);
    parse_bullet_task_line(line.from, line_str(text, &line))
}

/// The line holding `pos` has a list or task marker in the tree.
pub fn is_on_list_line(tree: &MarkdownTree, text: &Text, pos: usize) -> bool {
    let line = text.line_at(pos);
    tree.iter_range(line.from..line.to)
        .any(|n| matches!(n.kind(), NodeKind::ListMark | NodeKind::TaskMarker))
}

/// The bullet or task line holding `pos`, confirmed by the tree, so a
/// `- item` line inside a fence is not a list line.
pub fn list_line_at(tree: &MarkdownTree, text: &Text, pos: usize) -> Option<BulletTaskLine> {
    let parsed = parse_at(text, pos)?;
    is_on_list_line(tree, text, pos).then_some(parsed)
}

/// A list item line as the tree sees it (`ListItemLine`, `list/index.ts:377`):
/// the whitespace before its marker, and the whitespace a line needs to
/// nest under it (marker characters turned into spaces, tabs kept).
#[derive(Clone, Debug)]
pub struct ListItemLine<'t> {
    pub line: Line,
    pub item: Node<'t>,
    pub mark_from: usize,
    pub leading_ws: String,
    pub content_ws: String,
}

pub fn list_item_line_at<'t>(
    tree: &'t MarkdownTree,
    text: &Text,
    line: Line,
) -> Option<ListItemLine<'t>> {
    let mark = tree
        .iter_range(line.from..line.to)
        .find(|n| n.kind() == NodeKind::ListMark && n.from() >= line.from && n.from() <= line.to)?;
    let item = mark.parent()?;
    if item.kind() != NodeKind::ListItem {
        return None;
    }
    let content_from = match mark.next_sibling() {
        Some(content) if content.from() <= line.to => content.from(),
        _ => (mark.to() + 1).min(line.to),
    };
    let leading_ws = text.slice(line.from..mark.from()).to_owned();
    let marker: String = text
        .slice(mark.from()..content_from.max(mark.from()))
        .chars()
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    let content_ws = format!("{leading_ws}{marker}");
    Some(ListItemLine {
        line,
        item,
        mark_from: mark.from(),
        leading_ws,
        content_ws,
    })
}

fn is_list(node: Option<Node<'_>>) -> bool {
    node.is_some_and(|n| matches!(n.kind(), NodeKind::BulletList | NodeKind::OrderedList))
}

/// The item before `item` at its level, also across an adjacent list.
fn prev_list_item<'t>(item: Node<'t>) -> Option<Node<'t>> {
    if let Some(prev) = item.prev_sibling()
        && prev.kind() == NodeKind::ListItem
    {
        return Some(prev);
    }
    let prev_list = item.parent().and_then(|p| p.prev_sibling());
    if !is_list(prev_list) {
        return None;
    }
    prev_list?
        .last_child()
        .filter(|n| n.kind() == NodeKind::ListItem)
}

fn parent_list_item<'t>(item: Node<'t>) -> Option<Node<'t>> {
    item.parent()?
        .parent()
        .filter(|n| n.kind() == NodeKind::ListItem)
}

fn list_item_line_of<'t>(
    tree: &'t MarkdownTree,
    text: &Text,
    item: Node<'t>,
) -> Option<ListItemLine<'t>> {
    list_item_line_at(tree, text, text.line_at(item.from()))
}

/// `^(\d{1,9})([.)])` on an ordered marker.
fn ordered_mark(s: &str) -> Option<(usize, char)> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let delimiter = s[digits..]
        .chars()
        .next()
        .filter(|c| matches!(c, '.' | ')'))?;
    Some((s[..digits].parse().ok()?, delimiter))
}

/// `^\d+[.)]$`: an ordered list marker.
fn is_ordered_mark_text(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2
        && matches!(b[b.len() - 1], b'.' | b')')
        && b[..b.len() - 1].iter().all(u8::is_ascii_digit)
}

fn ordered_number_after(text: &Text, prev: Option<Node<'_>>) -> Option<usize> {
    let mark = prev?.child(NodeKind::ListMark)?;
    ordered_mark(text.slice(mark.range())).map(|(n, _)| n + 1)
}

fn renumber_ordered_mark(
    text: &Text,
    entry: &ListItemLine<'_>,
    number: Option<usize>,
) -> Option<Change> {
    let number = number?;
    let mark = entry.item.child(NodeKind::ListMark)?;
    let (current, delimiter) = ordered_mark(text.slice(mark.range()))?;
    (current != number).then(|| Change::new(mark.from()..mark.to(), format!("{number}{delimiter}")))
}

/// The ordered item line holding `pos` with no text yet (`1. `).
fn empty_ordered_item_at<'t>(
    tree: &'t MarkdownTree,
    text: &Text,
    pos: usize,
) -> Option<ListItemLine<'t>> {
    let line = text.line_at(pos);
    let s = line_str(text, &line);
    let rest = s.trim_start_matches([' ', '\t']);
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let shape = (1..=9).contains(&digits)
        && matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
        && rest[digits + 1..].bytes().all(is_blank_byte);
    if pos != line.to || !shape {
        return None;
    }
    let entry = list_item_line_at(tree, text, line)?;
    (entry.item.parent()?.kind() == NodeKind::OrderedList).then_some(entry)
}

/// A caret inside a list prefix moves to the nearest allowed boundary:
/// inside the indent to the marker, inside the marker to the body
/// (`clampCollapsedListPrefixRange`).
pub fn clamp_collapsed_list_prefix_range(
    tree: &MarkdownTree,
    text: &Text,
    range: SelectionRange,
) -> SelectionRange {
    if !range.is_empty() {
        return range;
    }
    let Some(parsed) = list_line_at(tree, text, range.head) else {
        return range;
    };
    let pos = range.head;
    if pos > parsed.line_from && pos < parsed.marker_from {
        SelectionRange::cursor(parsed.marker_from)
    } else if pos > parsed.marker_from && pos < parsed.body_from {
        SelectionRange::cursor(parsed.body_from)
    } else {
        range
    }
}

/// The selection filter every editor installs (`listPrefixSelectionGuard`).
pub fn prefix_selection_guard(state: &EditorState, selection: Selection) -> Selection {
    let ranges: Vec<SelectionRange> = selection
        .ranges()
        .iter()
        .map(|r| clamp_collapsed_list_prefix_range(state.tree(), state.text(), *r))
        .collect();
    if ranges == selection.ranges() {
        return selection;
    }
    Selection::new(ranges, selection.main_index())
}

fn selected_line_numbers(text: &Text, selection: &Selection) -> Vec<usize> {
    let mut numbers = Vec::new();
    for range in selection.ranges() {
        let from = text.line_at(range.from()).number;
        let end = if range.is_empty() {
            range.to()
        } else {
            range.from().max(range.to() - 1)
        };
        let to = text.line_at(end).number;
        numbers.extend(from..=to);
    }
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reindent {
    Indent,
    Outdent,
}

/// Tab nests under the previous item at the same level (its content
/// column); Shift-Tab lifts to the parent's indent. `None`: nowhere to go.
fn reindent_target(
    tree: &MarkdownTree,
    text: &Text,
    mode: Reindent,
    entry: &ListItemLine<'_>,
) -> Option<String> {
    match mode {
        Reindent::Indent => {
            let prev = prev_list_item(entry.item)?;
            list_item_line_of(tree, text, prev).map(|l| l.content_ws)
        }
        Reindent::Outdent => match parent_list_item(entry.item) {
            None => (!entry.leading_ws.is_empty()).then(String::new),
            Some(parent) => list_item_line_of(tree, text, parent).map(|l| l.leading_ws),
        },
    }
}

fn ordered_target_number(text: &Text, mode: Reindent, entry: &ListItemLine<'_>) -> Option<usize> {
    match mode {
        Reindent::Outdent => ordered_number_after(text, parent_list_item(entry.item)),
        Reindent::Indent => {
            let nested = prev_list_item(entry.item).and_then(|p| p.last_child());
            match nested {
                Some(list) if list.kind() == NodeKind::OrderedList => {
                    ordered_number_after(text, list.last_child())
                }
                _ => Some(1),
            }
        }
    }
}

/// Every line of an item's own paragraphs other than its marker line that
/// starts with whitespace: hard-wrapped lines and later paragraphs. A line
/// at the margin (a lazy continuation) is the writer's own paragraph.
fn item_paragraph_lines(text: &Text, item: Node<'_>) -> Vec<Line> {
    let marker_line = text.line_at(item.from()).number;
    let mut lines = Vec::new();
    for child in item.children() {
        if !matches!(child.kind(), NodeKind::Paragraph | NodeKind::Task) {
            continue;
        }
        let first = text.line_at(child.from()).number;
        let last = text.line_at(child.to()).number;
        for n in first..=last {
            let line = text.line(n);
            let starts_blank = line_str(text, &line)
                .bytes()
                .next()
                .is_some_and(is_blank_byte);
            if n != marker_line && starts_blank {
                lines.push(line);
            }
        }
    }
    lines
}

/// What a key handler decided.
enum Outcome {
    /// Not handled: the next handler (or the default) runs.
    Pass,
    /// Handled without changing anything (Tab with nowhere to go).
    Consumed,
    Apply(Transaction),
}

impl EditorState {
    fn run(&mut self, outcome: Outcome) -> bool {
        match outcome {
            Outcome::Pass => false,
            Outcome::Consumed => true,
            Outcome::Apply(transaction) => self.apply(transaction.scroll()).is_ok(),
        }
    }

    /// Tab on list lines: nests each selected item under the item before
    /// it; a selected parent moves its subtree. Consumed on any list line,
    /// even when nothing can move, so no literal tab breaks the list.
    pub fn list_indent(&mut self) -> bool {
        let outcome = reindent(self, Reindent::Indent);
        self.run(outcome)
    }

    /// Shift-Tab on list lines: lifts each selected item to its parent's
    /// level.
    pub fn list_outdent(&mut self) -> bool {
        let outcome = reindent(self, Reindent::Outdent);
        self.run(outcome)
    }

    /// Enter in a list: continues the item with the same marker (a new task
    /// is unchecked); on an empty item, a nested one steps out a level and a
    /// top-level one ends the list in place.
    pub fn list_enter(&mut self) -> bool {
        let outcome = list_enter(self);
        self.run(outcome)
    }

    /// Enter on an indent-only line right after a blank line clears the
    /// indent (leaving a continuation paragraph or an indented code block).
    pub fn leave_indented_blank(&mut self) -> bool {
        let outcome = leave_indented_blank(self);
        self.run(outcome)
    }

    /// Backspace at a list prefix boundary: at the marker it removes one
    /// indent level, at the body it removes the marker.
    pub fn list_backspace(&mut self) -> bool {
        let outcome = list_backspace(self);
        self.run(outcome)
    }

    /// A click released on the task prefix of the line holding `pos`. A
    /// press that turned into a drag selects text instead and must pass
    /// `dragged`: only a plain click toggles (Writer listens on `click`, not
    /// `mousedown`, `list/index.ts:1008-1050`).
    pub fn checkbox_click(&mut self, pos: usize, dragged: bool) -> bool {
        if dragged || self.is_loading() {
            return false;
        }
        match checkbox_toggle_from_line(self.text(), pos)
            .or_else(|| checkbox_toggle(self.text(), pos))
        {
            Some(change) => self
                .apply(Transaction::new().change(change).event(UserEvent::Command))
                .is_ok(),
            None => false,
        }
    }

    /// Left or right arrow at a list prefix: jumps across the indent or the
    /// marker as one step. `false` lets the plain motion run.
    pub fn list_arrow(&mut self, right: bool, extend: bool) -> bool {
        let Some(pos) = list_prefix_boundary_move(self, right) else {
            return false;
        };
        let main = self.selection().main();
        let selection = if extend {
            Selection::single(main.anchor, pos)
        } else {
            Selection::cursor(pos)
        };
        self.apply(
            Transaction::new()
                .select(selection)
                .event(UserEvent::Select)
                .scroll(),
        )
        .is_ok()
    }

    /// List rendering geometry for the whole document.
    pub fn list_decorations(&self) -> ListDecorations {
        list_decorations(self.tree(), self.text())
    }
}

fn reindent(state: &EditorState, mode: Reindent) -> Outcome {
    if state.is_loading() {
        return Outcome::Pass;
    }
    let (tree, text) = (state.tree(), state.text());
    let mut changes = Vec::new();
    let mut saw_list_line = false;
    let mut anchor: Option<(String, String)> = None;
    for number in selected_line_numbers(text, state.selection()) {
        let Some(entry) = list_item_line_at(tree, text, text.line(number)) else {
            continue;
        };
        saw_list_line = true;
        let mut own_target = false;
        let new_ws = match &anchor {
            Some((old, new))
                if entry.leading_ws.len() > old.len()
                    && entry.leading_ws.starts_with(old.as_str()) =>
            {
                format!("{new}{}", &entry.leading_ws[old.len()..])
            }
            _ => {
                let new = reindent_target(tree, text, mode, &entry)
                    .unwrap_or_else(|| entry.leading_ws.clone());
                anchor = Some((entry.leading_ws.clone(), new.clone()));
                own_target = true;
                new
            }
        };
        if new_ws == entry.leading_ws {
            continue;
        }
        changes.push(Change::new(
            entry.line.from..entry.mark_from,
            new_ws.clone(),
        ));
        if own_target
            && let Some(renumber) =
                renumber_ordered_mark(text, &entry, ordered_target_number(text, mode, &entry))
        {
            changes.push(renumber);
        }
        for line in item_paragraph_lines(text, entry.item) {
            if line_str(text, &line).starts_with(entry.leading_ws.as_str()) {
                changes.push(Change::new(
                    line.from..line.from + entry.leading_ws.len(),
                    new_ws.clone(),
                ));
            }
        }
    }
    if !saw_list_line {
        return Outcome::Pass;
    }
    if changes.is_empty() {
        return Outcome::Consumed;
    }
    let Ok(set) = ChangeSet::new(text, changes.clone()) else {
        return Outcome::Consumed;
    };
    // A caret at a line start stays there; anything after rides along.
    let map = |pos: usize| {
        let bias = if pos == text.line_at(pos).from {
            Bias::Left
        } else {
            Bias::Right
        };
        set.map_pos(pos, bias)
    };
    let ranges = state
        .selection()
        .ranges()
        .iter()
        .map(|r| SelectionRange::new(map(r.anchor), map(r.head)))
        .collect();
    let selection = Selection::new(ranges, state.selection().main_index());
    Outcome::Apply(
        Transaction::new()
            .changes(changes)
            .select(selection)
            .event(UserEvent::Command),
    )
}

/// Enter on an empty item (`exitEmptyItem`).
fn exit_empty_item(state: &EditorState, line: Line, entry: Option<ListItemLine<'_>>) -> Outcome {
    let (tree, text) = (state.tree(), state.text());
    let parent = entry.as_ref().and_then(|e| parent_list_item(e.item));
    let parent_line = parent.and_then(|p| list_item_line_of(tree, text, p));
    if let (Some(entry), Some(parent), Some(parent_line)) = (&entry, parent, parent_line) {
        let mut changes = vec![Change::new(
            line.from..entry.mark_from,
            parent_line.leading_ws.clone(),
        )];
        changes.extend(renumber_ordered_mark(
            text,
            entry,
            ordered_number_after(text, Some(parent)),
        ));
        let Ok(set) = ChangeSet::new(text, changes.clone()) else {
            return Outcome::Pass;
        };
        let caret = set.map_pos(line.to, Bias::Right);
        return Outcome::Apply(
            Transaction::new()
                .changes(changes)
                .select(Selection::cursor(caret))
                .event(UserEvent::Delete),
        );
    }
    Outcome::Apply(
        Transaction::new()
            .change(Change::delete(line.from..line.to))
            .select(Selection::cursor(line.from))
            .event(UserEvent::Delete),
    )
}

fn list_enter(state: &EditorState) -> Outcome {
    if state.is_loading() {
        return Outcome::Pass;
    }
    let (tree, text) = (state.tree(), state.text());
    let selection = state.selection();
    if selection.ranges().len() != 1 || !selection.main().is_empty() {
        return Outcome::Pass;
    }
    let head = selection.main().head;
    if let Some(entry) = empty_ordered_item_at(tree, text, head) {
        let line = entry.line;
        return exit_empty_item(state, line, Some(entry));
    }
    let Some(parsed) = list_line_at(tree, text, head) else {
        return Outcome::Pass;
    };
    let line = text.line_at(head);
    if parsed.body_from == line.to {
        return exit_empty_item(state, line, list_item_line_at(tree, text, line));
    }
    if head < parsed.body_from {
        return Outcome::Pass;
    }
    let s = line_str(text, &line);
    let indent = &s[..parsed.indent_len];
    let marker = &s[parsed.indent_len..parsed.indent_len + 1];
    let sep = &s[parsed.indent_len + 1..parsed.indent_len + 2];
    let continuation = if parsed.is_task {
        format!("{indent}{marker}{sep}[ ]{sep}")
    } else {
        format!("{indent}{marker}{sep}")
    };
    let caret = head + 1 + continuation.len();
    Outcome::Apply(
        Transaction::new()
            .change(Change::insert(head, format!("\n{continuation}")))
            .select(Selection::cursor(caret))
            .event(UserEvent::Command),
    )
}

fn leave_indented_blank(state: &EditorState) -> Outcome {
    let (tree, text) = (state.tree(), state.text());
    let selection = state.selection();
    if state.is_loading() || selection.ranges().len() != 1 {
        return Outcome::Pass;
    }
    let main = selection.main();
    let line = text.line_at(main.head);
    let s = line_str(text, &line);
    if !main.is_empty() || main.head != line.to || s.is_empty() || line.number == 0 {
        return Outcome::Pass;
    }
    if !s.bytes().all(is_blank_byte) {
        return Outcome::Pass;
    }
    let previous = text.line(line.number - 1);
    if !line_str(text, &previous).trim().is_empty() {
        return Outcome::Pass;
    }
    let node = tree.resolve(line.from, 1);
    if std::iter::once(node)
        .chain(node.ancestors())
        .any(|n| n.kind() == NodeKind::FencedCode)
    {
        return Outcome::Pass;
    }
    Outcome::Apply(
        Transaction::new()
            .change(Change::delete(line.from..line.to))
            .select(Selection::cursor(line.from))
            .event(UserEvent::Delete),
    )
}

fn list_backspace(state: &EditorState) -> Outcome {
    let (tree, text) = (state.tree(), state.text());
    let selection = state.selection();
    if state.is_loading() || selection.ranges().len() != 1 || !selection.main().is_empty() {
        return Outcome::Pass;
    }
    let head = selection.main().head;
    let Some(parsed) = list_line_at(tree, text, head) else {
        return Outcome::Pass;
    };
    let effective = if head > parsed.line_from && head < parsed.marker_from {
        parsed.marker_from
    } else if head > parsed.marker_from && head < parsed.body_from {
        parsed.body_from
    } else {
        head
    };
    if effective == parsed.line_from {
        return Outcome::Pass;
    }
    let line = text.line_at(head);
    let entry = list_item_line_at(tree, text, line);
    // One level back is where Shift-Tab would put the item.
    let outdent_ws = || {
        entry
            .as_ref()
            .and_then(|e| reindent_target(tree, text, Reindent::Outdent, e))
            .unwrap_or_default()
    };
    let replace = |range: Range<usize>, insert: String| {
        let caret = parsed.line_from + insert.len();
        Outcome::Apply(
            Transaction::new()
                .change(Change::new(range, insert))
                .select(Selection::cursor(caret))
                .event(UserEvent::Delete),
        )
    };
    if effective == parsed.marker_from {
        if parsed.indent_len == 0 {
            return Outcome::Pass;
        }
        return replace(parsed.line_from..parsed.marker_from, outdent_ws());
    }
    if effective == parsed.body_from {
        let insert = if parsed.indent_len > 0 {
            outdent_ws()
        } else {
            String::new()
        };
        // `TODOS.md:95`: on an empty item two levels deep, removing the
        // marker would leave a line of spaces; outdent it instead, like Enter
        // on an empty nested item.
        if parsed.body_from == line.to && !insert.is_empty() {
            return exit_empty_item(state, line, entry);
        }
        return replace(parsed.line_from..parsed.body_from, insert);
    }
    Outcome::Pass
}

/// The checkbox change for a task whose marker starts at `marker_pos`.
pub fn checkbox_toggle(text: &Text, marker_pos: usize) -> Option<Change> {
    let parsed = parse_at(text, marker_pos)?;
    if !parsed.is_task || parsed.marker_from != marker_pos {
        return None;
    }
    let inner = parsed.marker_from + TASK_INNER_OFFSET;
    let checked = text.slice(inner..inner + 1).eq_ignore_ascii_case("x");
    Some(Change::new(
        inner..inner + 1,
        if checked { " " } else { "x" },
    ))
}

/// The checkbox change for the task line holding `pos`.
pub fn checkbox_toggle_from_line(text: &Text, pos: usize) -> Option<Change> {
    let parsed = parse_at(text, pos)?;
    if !parsed.is_task {
        return None;
    }
    checkbox_toggle(text, parsed.marker_from)
}

/// Arrow left or right across a list prefix (`listPrefixBoundaryMove`).
fn list_prefix_boundary_move(state: &EditorState, right: bool) -> Option<usize> {
    let main = state.selection().main();
    if !main.is_empty() {
        return None;
    }
    let parsed = list_line_at(state.tree(), state.text(), main.head)?;
    let indented = parsed.marker_from > parsed.line_from;
    if !right {
        if main.head == parsed.body_from {
            return Some(parsed.marker_from);
        }
        if indented && main.head == parsed.marker_from {
            return Some(parsed.line_from);
        }
    } else {
        if main.head == parsed.line_from {
            return Some(if indented {
                parsed.marker_from
            } else {
                parsed.body_from
            });
        }
        if indented && main.head == parsed.marker_from {
            return Some(parsed.body_from);
        }
    }
    None
}

/// What a list prefix renders as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixKind {
    Bullet,
    Task { checked: bool },
}

/// List rendering, in `ch` units (`buildListDecorations`).
#[derive(Clone, Debug, PartialEq)]
pub enum ListDecoration {
    /// The source prefix drawn as a bullet or a checkbox: `width_ch` wide,
    /// the marker drawn at `marker_offset_ch` in a `LIST_UNIT_CH` column.
    Prefix {
        range: Range<usize>,
        kind: PrefixKind,
        width_ch: usize,
        marker_offset_ch: usize,
    },
    /// Leading source whitespace drawn as a fixed indent.
    IndentVisual {
        range: Range<usize>,
        width_ch: usize,
    },
    /// An ordered marker kept as text, at least `LIST_UNIT_CH` wide.
    OrderedMarker { range: Range<usize> },
    /// The body text after the prefix.
    Body { range: Range<usize> },
    /// A list line's start padding; marker lines pull their first visual
    /// line back by `hanging_ch` (a hanging indent).
    Line {
        line_start: usize,
        padding_ch: f32,
        hanging_ch: f32,
    },
    /// The source indent of a continuation line, collapsed.
    CollapsedIndent { range: Range<usize> },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListDecorations {
    pub all: Vec<ListDecoration>,
    /// Ranges the caret and Backspace treat as one step.
    pub atomic: Vec<Range<usize>>,
    /// Bullet and task marker ranges.
    pub marker: Vec<Range<usize>>,
}

fn depth_of(mark: Node<'_>) -> usize {
    mark.ancestors()
        .filter(|n| n.kind() == NodeKind::ListItem)
        .count()
        .saturating_sub(1)
}

fn has_same_line_child_list(text: &Text, item: Node<'_>, line: &Line) -> bool {
    item.children()
        .any(|c| is_list(Some(c)) && text.line_at(c.from()).number == line.number)
}

fn push_continuation_lines(
    text: &Text,
    item: Node<'_>,
    padding_ch: usize,
    out: &mut ListDecorations,
) {
    for line in item_paragraph_lines(text, item) {
        out.all.push(ListDecoration::Line {
            line_start: line.from,
            padding_ch: padding_ch as f32,
            hanging_ch: 0.0,
        });
        let ws = line_str(text, &line)
            .bytes()
            .take_while(|b| is_blank_byte(*b))
            .count();
        if ws > 0 {
            out.all.push(ListDecoration::CollapsedIndent {
                range: line.from..line.from + ws,
            });
            out.atomic.push(line.from..line.from + ws);
        }
    }
}

/// Indent steps of a nested item's leading whitespace: one fixed-width
/// indent and one atomic range per level.
fn push_indent_steps(line: &Line, mark_from: usize, depth: usize, out: &mut ListDecorations) {
    let leading = mark_from - line.from;
    if depth == 0 || leading < depth {
        return;
    }
    out.all.push(ListDecoration::IndentVisual {
        range: line.from..mark_from,
        width_ch: depth * LIST_UNIT_CH,
    });
    let step = leading / depth;
    for i in 0..depth {
        let from = line.from + i * step;
        let to = if i == depth - 1 {
            mark_from
        } else {
            line.from + (i + 1) * step
        };
        if to <= from {
            break;
        }
        out.atomic.push(from..to);
    }
}

pub fn list_decorations(tree: &MarkdownTree, text: &Text) -> ListDecorations {
    let mut out = ListDecorations::default();
    for mark in tree.iter().filter(|n| n.kind() == NodeKind::ListMark) {
        // A bare marker with no space after it is still plain text.
        if !text
            .slice(mark.to()..(mark.to() + 1).min(text.len()))
            .bytes()
            .next()
            .is_some_and(is_blank_byte)
        {
            continue;
        }
        let mark_text = text.slice(mark.range());
        let line = text.line_at(mark.from());
        let item = mark.parent();
        let holds_same_line_list = item.is_some_and(|i| has_same_line_child_list(text, i, &line));
        let parent = item.and_then(parent_list_item);
        let shares_parent_line =
            parent.is_some_and(|p| text.line_at(p.from()).number == line.number);
        let depth = depth_of(mark);
        let prefix_ch = (depth + 1) * LIST_UNIT_CH;
        if is_ordered_mark_text(mark_text) {
            out.all.push(ListDecoration::OrderedMarker {
                range: mark.range(),
            });
            // `TODOS.md:96`: nested ordered items collapse their source
            // indent and hang at their depth, like bullets.
            if !shares_parent_line {
                push_indent_steps(&line, mark.from(), depth, &mut out);
            }
            let body_from = mark.to() + 1;
            if body_from < line.to && !holds_same_line_list {
                out.all.push(ListDecoration::Body {
                    range: body_from..line.to,
                });
            }
            if !holds_same_line_list {
                out.all.push(ListDecoration::Line {
                    line_start: line.from,
                    padding_ch: prefix_ch as f32,
                    hanging_ch: prefix_ch as f32 + 0.4,
                });
            }
            if let Some(item) = item {
                push_continuation_lines(text, item, prefix_ch, &mut out);
            }
            continue;
        }
        if !matches!(mark_text, "-" | "+" | "*") {
            continue;
        }
        if !shares_parent_line {
            push_indent_steps(&line, mark.from(), depth, &mut out);
        }
        let mut kind = PrefixKind::Bullet;
        let mut prefix_end = mark.to() + 1;
        if let Some(task) = mark.next_sibling().filter(|n| n.kind() == NodeKind::Task)
            && let Some(marker) = task
                .first_child()
                .filter(|n| n.kind() == NodeKind::TaskMarker)
            && text
                .slice(marker.to()..(marker.to() + 1).min(text.len()))
                .bytes()
                .next()
                .is_some_and(is_blank_byte)
        {
            let inner = text.slice(marker.from() + 1..marker.to() - 1);
            kind = PrefixKind::Task {
                checked: inner.eq_ignore_ascii_case("x"),
            };
            prefix_end = marker.to() + 1;
        }
        let (range, width_ch, marker_offset_ch) = if shares_parent_line {
            (mark.from()..prefix_end, LIST_UNIT_CH, 0)
        } else {
            (line.from..prefix_end, prefix_ch, depth * LIST_UNIT_CH)
        };
        out.all.push(ListDecoration::Prefix {
            range,
            kind,
            width_ch,
            marker_offset_ch,
        });
        out.marker.push(mark.from()..prefix_end);
        out.atomic.push(mark.from()..prefix_end);
        if prefix_end < line.to && !holds_same_line_list {
            out.all.push(ListDecoration::Body {
                range: prefix_end..line.to,
            });
        }
        if !holds_same_line_list {
            out.all.push(ListDecoration::Line {
                line_start: line.from,
                padding_ch: prefix_ch as f32,
                hanging_ch: prefix_ch as f32,
            });
        }
        if let Some(item) = item {
            push_continuation_lines(text, item, prefix_ch, &mut out);
        }
    }
    out.atomic.sort_by_key(|r| (r.start, r.end));
    out
}

#[cfg(test)]
mod tests;
