use std::ops::Range;

use crate::change::Change;
use crate::selection::{Selection, SelectionRange};
use crate::state::EditorState;
use crate::table::{self, CellKind, Grid};
use crate::text::Text;
use crate::tree::{MarkdownTree, NodeKind};

pub fn grid_in(
    tree: &MarkdownTree,
    text: &Text,
    pos: usize,
    source: Option<usize>,
) -> Option<Grid> {
    let line = text.line_at(pos);
    tree.iter_range(line.from..line.to)
        .filter(|n| n.kind() == NodeKind::Table)
        .find_map(|n| {
            let grid = Grid::from_node(text, n)?;
            let zone = grid.zone();
            (zone.start <= pos
                && pos <= zone.end
                && !grid.has_extra()
                && source != Some(grid.range.start))
            .then_some(grid)
        })
}

pub fn grid_at(state: &EditorState, pos: usize) -> Option<Grid> {
    if !state.atomic_blocks() {
        return None;
    }
    grid_in(state.tree(), state.text(), pos, state.table_source())
}

pub fn cell_at(state: &EditorState, pos: usize) -> Option<(Grid, usize, usize)> {
    let grid = grid_at(state, pos)?;
    let (row, column) = grid.cell_holding(pos)?;
    Some((grid, row, column))
}

fn stretches(grid: &Grid) -> Vec<Range<usize>> {
    grid.drawn_cells()
        .flat_map(|(_, _, cell)| cell.stretches())
        .collect()
}

pub fn snap_pos(grid: &Grid, text: &Text, pos: usize, forward: bool) -> usize {
    if grid.content_at(pos).is_some() {
        return pos;
    }
    let all = stretches(grid);
    let zone = grid.zone();
    if forward {
        if let Some(s) = all.iter().find(|s| s.start >= pos) {
            return s.start;
        }
        let last = text.line_at(zone.end).number;
        if last + 1 < text.line_count() {
            return text.line(last + 1).from;
        }
        all.last().map_or(pos, |s| s.end)
    } else {
        if let Some(s) = all.iter().rev().find(|s| s.end <= pos) {
            return s.end;
        }
        let first = text.line_at(zone.start).number;
        if first > 0 {
            return text.line(first - 1).to;
        }
        all.first().map_or(pos, |s| s.start)
    }
}

pub fn snap(
    tree: &MarkdownTree,
    text: &Text,
    source: Option<usize>,
    selection: Selection,
    previous: &Selection,
    edited: bool,
) -> Selection {
    let mut changed = false;
    let covers =
        |r: &SelectionRange, grid: &Grid| r.from() <= grid.range.start && r.to() >= grid.range.end;
    let ranges: Vec<SelectionRange> = selection
        .ranges()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let before = previous.ranges().get(i).copied().unwrap_or(previous.main());
            if !r.is_empty()
                && let Some(grid) = grid_in(tree, text, r.head, source)
            {
                let zone = grid.zone();
                let inside = zone.start <= r.anchor && r.anchor <= zone.end;
                if inside && grid.cell_at(r.anchor) != grid.cell_at(r.head) {
                    return *r;
                }
            }
            let kept = |grid: &Grid, pos: usize| edited && grid.cell_holding(pos).is_some();
            let head = match grid_in(tree, text, r.head, source) {
                Some(grid) if !covers(r, &grid) && !kept(&grid, r.head) => {
                    snap_pos(&grid, text, r.head, r.head >= before.head)
                }
                _ => r.head,
            };
            let anchor = if r.is_empty() {
                head
            } else {
                match grid_in(tree, text, r.anchor, source) {
                    Some(grid) if !covers(r, &grid) && !kept(&grid, r.anchor) => {
                        snap_pos(&grid, text, r.anchor, r.anchor >= before.anchor)
                    }
                    _ => r.anchor,
                }
            };
            let out = SelectionRange::new(anchor, head);
            changed |= out != *r;
            out
        })
        .collect();
    if !changed {
        return selection;
    }
    Selection::new(ranges, selection.main_index())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellEdit {
    Outside,
    Edit(Change),
    Refused,
}

pub(crate) fn escape(text: &str, mut escaped: bool) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '|' if !escaped => out.push_str("\\|"),
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("<br>");
            }
            _ => {
                out.push(c);
                escaped = !escaped && c == '\\';
                continue;
            }
        }
        escaped = false;
    }
    (out, escaped)
}

pub fn cell_edit(state: &EditorState, range: Range<usize>, insert: &str) -> CellEdit {
    let Some(grid) = grid_at(state, range.start) else {
        return CellEdit::Outside;
    };
    let Some((row, cell)) = grid.drawn_cells().find_map(|(r, _, c)| {
        (c.content.start <= range.start && range.end <= c.slot.end.max(c.content.end))
            .then_some((r, c))
    }) else {
        return CellEdit::Outside;
    };
    let mut range = range;
    for atom in cell.atoms() {
        if range.start < atom.end && range.end > atom.start {
            range.start = range.start.min(atom.start);
            range.end = range.end.max(atom.end);
        }
    }
    let doc = state.doc();
    let odd = doc[..range.start]
        .bytes()
        .rev()
        .take_while(|b| *b == b'\\')
        .count()
        % 2
        == 1;
    let (mut insert, trailing) = escape(insert, odd);
    let line = grid.rows[row].line.clone();
    let pipes = |insert: &str| {
        let mut s = String::with_capacity(line.len() + insert.len());
        s.push_str(&doc[line.start..range.start]);
        s.push_str(insert);
        s.push_str(&doc[range.end..line.end]);
        table::pipe_count(&s)
    };
    let before = table::pipe_count(&doc[line.clone()]);
    if pipes(&insert) != before {
        if trailing || insert.is_empty() {
            insert.push(' ');
        }
        if pipes(&insert) != before {
            return CellEdit::Refused;
        }
    }
    CellEdit::Edit(Change::new(range, insert))
}

pub fn cell_delete(state: &EditorState, head: usize, cut: Range<usize>) -> CellEdit {
    let Some((grid, row, column)) = cell_at(state, head) else {
        return CellEdit::Outside;
    };
    let Some(cell) = grid.cell(row, column) else {
        return CellEdit::Outside;
    };
    let end = cell.content.end.max(head);
    let clamped = cut.start.max(cell.content.start)..cut.end.min(end).max(cut.start);
    if clamped.start >= clamped.end {
        return CellEdit::Refused;
    }
    cell_edit(state, clamped, "")
}

impl EditorState {
    pub fn guard_table_joins(&mut self, cuts: &[Range<usize>]) -> bool {
        if !self.atomic_blocks() {
            return false;
        }
        let mut hit = Vec::new();
        for (r, cut) in self.selection().ranges().iter().zip(cuts) {
            if !r.is_empty() || cut.is_empty() {
                hit.push(None);
                continue;
            }
            let text = self.text();
            let source = self.table_source();
            let grid = grid_in(self.tree(), text, cut.start, source)
                .or_else(|| grid_in(self.tree(), text, cut.end, source))
                .filter(|g| {
                    let zone = g.zone();
                    let outside = r.head < zone.start || r.head > zone.end;
                    outside && cut.start <= zone.end && cut.end >= zone.start
                });
            hit.push(grid.map(|g| g.range.clone()));
        }
        if hit.iter().all(Option::is_none) {
            return false;
        }
        let selection = self.selection().clone();
        let ranges: Vec<SelectionRange> = selection
            .ranges()
            .iter()
            .zip(&hit)
            .map(|(r, h)| match h {
                Some(table) => SelectionRange::new(table.start, table.end),
                None => *r,
            })
            .collect();
        let _ = self.apply(
            crate::state::Transaction::new()
                .select(Selection::new(ranges, selection.main_index()))
                .event(crate::history::UserEvent::Select)
                .scroll(),
        );
        true
    }

    pub(crate) fn cell_cuts(&self, cuts: &[Range<usize>]) -> Vec<Option<Change>> {
        self.selection()
            .ranges()
            .iter()
            .zip(cuts)
            .map(|(r, cut)| {
                let edit = if r.is_empty() {
                    cell_delete(self, r.head, cut.clone())
                } else {
                    cell_edit(self, cut.clone(), "")
                };
                match edit {
                    CellEdit::Outside => (!cut.is_empty()).then(|| Change::delete(cut.clone())),
                    CellEdit::Edit(change) => Some(change),
                    CellEdit::Refused => None,
                }
            })
            .collect()
    }
}

fn main_cell(state: &EditorState) -> Option<(Grid, usize, usize)> {
    if state.selection().ranges().len() != 1 {
        return None;
    }
    let r = state.selection().main();
    let (grid, row, column) = cell_at(state, r.head)?;
    let cell = grid.cell(row, column)?;
    cell.holds(r.anchor).then_some((grid, row, column))
}

fn eol(doc: &str, grid: &Grid) -> &'static str {
    if doc[grid.zone()].contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn new_row(doc: &str, grid: &Grid, reference: usize) -> (String, Vec<usize>) {
    let prefix = grid.rows.last().map_or("", |r| &doc[r.prefix.clone()]);
    let mut text = format!("{prefix}|");
    let mut points = Vec::with_capacity(grid.columns());
    for column in 0..grid.columns() {
        let width = grid
            .cell(reference, column)
            .filter(|c| c.kind == CellKind::InGrid)
            .map_or(2, |c| doc[c.slot.clone()].chars().count().max(2));
        points.push(text.len() + 1);
        text.push_str(&" ".repeat(width));
        text.push('|');
    }
    (text, points)
}

fn complete_row(doc: &str, grid: &Grid, row: usize, column: usize) -> Option<(Change, usize)> {
    let line = &grid.rows.get(row)?.line;
    let present = grid.rows[row]
        .cells
        .iter()
        .filter(|c| c.kind != CellKind::Missing)
        .count();
    if column < present {
        return None;
    }
    let mut insert = String::new();
    if !doc[line.clone()].trim_end().ends_with('|') {
        insert.push_str(" |");
    }
    let mut point = line.end;
    for _ in present..=column {
        point = line.end + insert.len() + 1;
        insert.push_str("  |");
    }
    Some((Change::insert(line.end, insert), point))
}

impl EditorState {
    fn select_cell_content(&mut self, grid: &Grid, row: usize, column: usize) -> bool {
        let Some(cell) = grid.cell(row, column) else {
            return false;
        };
        if cell.kind == CellKind::Missing {
            let Some((change, point)) = complete_row(self.doc(), grid, row, column) else {
                return false;
            };
            return self
                .apply(
                    crate::state::Transaction::new()
                        .change(change)
                        .select(Selection::cursor(point))
                        .event(crate::history::UserEvent::Command)
                        .scroll(),
                )
                .is_ok();
        }
        let content = cell.content.clone();
        self.apply(
            crate::state::Transaction::new()
                .select(Selection::single(content.start, content.end))
                .event(crate::history::UserEvent::Select)
                .scroll(),
        )
        .is_ok()
    }

    fn add_row(&mut self, grid: &Grid, after: usize, column: usize) -> bool {
        let doc = self.doc();
        let (row, points) = new_row(doc, grid, grid.rows.len() - 1);
        let eol = eol(doc, grid);
        let at = grid.rows[after].line.end;
        let caret = at + eol.len() + points.get(column).copied().unwrap_or(0);
        self.apply(
            crate::state::Transaction::new()
                .change(Change::insert(at, format!("{eol}{row}")))
                .select(Selection::cursor(caret))
                .event(crate::history::UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    pub fn leave_table(&mut self, down: bool) -> bool {
        let head = self.selection().main().head;
        let Some(grid) = grid_at(self, head) else {
            return false;
        };
        let zone = grid.zone();
        let text = self.text();
        let (first, last) = (
            text.line_at(zone.start).number,
            text.line_at(zone.end).number,
        );
        let eol = eol(self.doc(), &grid);
        let (change, caret) = if down {
            if last + 1 < text.line_count() {
                (None, text.line(last + 1).from)
            } else {
                (Some(Change::insert(zone.end, eol)), zone.end + eol.len())
            }
        } else if first > 0 {
            (None, text.line(first - 1).to)
        } else {
            (Some(Change::insert(0, eol)), 0)
        };
        let event = if change.is_some() {
            crate::history::UserEvent::Command
        } else {
            crate::history::UserEvent::Select
        };
        self.apply(
            crate::state::Transaction::new()
                .changes(change)
                .select(Selection::cursor(caret))
                .event(event)
                .scroll(),
        )
        .is_ok()
    }

    pub fn table_tab(&mut self, back: bool) -> bool {
        let Some((grid, row, column)) = main_cell(self) else {
            return false;
        };
        let order: Vec<(usize, usize)> = grid
            .text_rows()
            .flat_map(|(r, _)| (0..grid.columns()).map(move |c| (r, c)))
            .collect();
        let Some(i) = order.iter().position(|p| *p == (row, column)) else {
            return false;
        };
        if back {
            match i.checked_sub(1) {
                Some(prev) => self.select_cell_content(&grid, order[prev].0, order[prev].1),
                None => self.leave_table(false),
            }
        } else {
            match order.get(i + 1) {
                Some(&(r, c)) => self.select_cell_content(&grid, r, c),
                None => self.add_row(&grid, grid.rows.len() - 1, 0),
            }
        }
    }

    pub fn table_enter(&mut self) -> bool {
        let Some((grid, row, column)) = main_cell(self) else {
            return false;
        };
        let last = grid.rows.len() - 1;
        let below = if row == 0 { 2 } else { row + 1 };
        if below <= last {
            return self.select_cell_content(&grid, below, column);
        }
        let empty = grid.rows[row]
            .cells
            .iter()
            .filter(|c| c.kind == CellKind::InGrid)
            .all(|c| c.content.is_empty());
        if row >= 2 && empty {
            let doc = self.doc();
            let eol = eol(doc, &grid);
            let line = grid.rows[row].line.clone();
            let has_next = line.end < doc.len();
            let (change, caret) = if has_next {
                let above = grid.rows[row - 1].line.end;
                (Change::delete(above..line.end), above + eol.len())
            } else {
                let prefix = grid.rows[row].prefix.end;
                (Change::delete(prefix..line.end), prefix)
            };
            return self
                .apply(
                    crate::state::Transaction::new()
                        .change(change)
                        .select(Selection::cursor(caret))
                        .event(crate::history::UserEvent::Command)
                        .scroll(),
                )
                .is_ok();
        }
        self.add_row(&grid, last, column)
    }

    pub fn select_cell_or_all(&mut self) -> bool {
        if let Some((grid, row, column)) = main_cell(self)
            && let Some(cell) = grid.cell(row, column)
        {
            let main = self.selection().main();
            if main.range() != cell.content {
                let content = cell.content.clone();
                return self
                    .apply(
                        crate::state::Transaction::new()
                            .select(Selection::single(content.start, content.end))
                            .event(crate::history::UserEvent::Select),
                    )
                    .is_ok();
            }
        }
        self.select_all()
    }

    pub(crate) fn cell_contents(&self) -> Vec<Option<(Range<usize>, usize, usize)>> {
        self.selection()
            .ranges()
            .iter()
            .map(|r| {
                let (grid, row, column) = cell_at(self, r.head)?;
                let content = grid.cell(row, column)?.content.clone();
                let text = self.text();
                let next = snap_pos(&grid, text, (content.end + 1).min(text.len()), true);
                let prev = snap_pos(&grid, text, content.start.saturating_sub(1), false);
                Some((content, next, prev))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "intro\n\n| a | bb |\n| - | -- |\n| 1 |  |\n\nend";

    fn state(caret: usize) -> EditorState {
        let mut s = EditorState::new(DOC);
        s.select(Selection::cursor(caret)).unwrap();
        s
    }

    fn at(s: &EditorState) -> usize {
        s.selection().main().head
    }

    #[test]
    fn a_head_on_the_structure_goes_to_the_next_content() {
        let header = DOC.find("| a").unwrap();
        let a = header + 2;
        let bb = DOC.find("bb").unwrap();
        let one = DOC.find("1 |").unwrap();
        let empty = one + 4;
        let mut s = state(header - 1);
        s.select(Selection::cursor(header)).unwrap();
        assert_eq!(at(&s), a);
        s.select(Selection::cursor(a + 1)).unwrap();
        assert_eq!(at(&s), a + 1, "the end of `a` is in the cell");
        s.select(Selection::cursor(a + 2)).unwrap();
        assert_eq!(at(&s), bb);
        s.select(Selection::cursor(bb + 3)).unwrap();
        assert_eq!(at(&s), one);
        s.select(Selection::cursor(one - 1)).unwrap();
        assert_eq!(at(&s), bb + 2);
        s.select(Selection::cursor(empty)).unwrap();
        assert_eq!(at(&s), empty);
        let after = DOC.rfind("\n\nend").unwrap() + 1;
        s.select(Selection::cursor(empty + 1)).unwrap();
        assert_eq!(at(&s), after);
        let mut s = state(a);
        s.select(Selection::cursor(header)).unwrap();
        assert_eq!(at(&s), header - 1);
    }

    #[test]
    fn an_anchor_follows_unless_the_whole_table_is_selected() {
        let header = DOC.find("| a").unwrap();
        let end = DOC.rfind("|  |").unwrap() + 4;
        let mut s = state(0);
        s.select(Selection::single(header, end)).unwrap();
        assert_eq!(s.selection().main(), SelectionRange::new(header, end));
        let bb = DOC.find("bb").unwrap();
        s.select(Selection::single(bb - 1, bb + 1)).unwrap();
        assert_eq!(s.selection().main(), SelectionRange::new(bb, bb + 1));
    }

    #[test]
    fn a_source_only_editor_leaves_the_caret_alone() {
        let mut s = EditorState::new(DOC);
        s.set_atomic_blocks(false);
        let header = DOC.find("| a").unwrap();
        s.select(Selection::cursor(header)).unwrap();
        assert_eq!(at(&s), header);
        assert_eq!(
            cell_edit(&s, header + 2..header + 2, "|"),
            CellEdit::Outside
        );
    }

    #[test]
    fn a_table_with_extra_cells_is_edited_as_source() {
        let doc = "| a | b |\n|---|---|\n| 1 | 2 | extra |\n\nend";
        let mut s = EditorState::new(doc);
        let pipe = doc.find("| 2").unwrap();
        s.select(Selection::cursor(pipe)).unwrap();
        assert_eq!(at(&s), pipe, "the caret may sit on a pipe");
        let extra = doc.find("extra").unwrap();
        assert!(grid_at(&s, extra).is_none());
        assert_eq!(cell_edit(&s, extra..extra, "|"), CellEdit::Outside);
        s.select(Selection::cursor(extra)).unwrap();
        assert_eq!(at(&s), extra);
        assert!(!s.table_tab(false));
    }

    #[test]
    fn typing_escapes_pipes_and_breaks() {
        let a = DOC.find("| a").unwrap() + 3;
        let s = state(a);
        let edit = |text: &str| match cell_edit(&s, a..a, text) {
            CellEdit::Edit(c) => c.insert,
            other => panic!("{other:?}"),
        };
        assert_eq!(edit("|"), "\\|");
        assert_eq!(edit("x|y\nz\r\nw"), "x\\|y<br>z<br>w");
        assert_eq!(edit("\\|"), "\\|");
        let doc = "| a|\n| - |";
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(3)).unwrap();
        assert_eq!(
            cell_edit(&s, 3..3, "\\"),
            CellEdit::Edit(Change::new(3..3, "\\ "))
        );
        assert_eq!(cell_edit(&state(0), 0..0, "|"), CellEdit::Outside);
    }

    #[test]
    fn deletions_stay_in_the_cell() {
        let a = DOC.find("| a").unwrap() + 2;
        let s = state(a);
        assert_eq!(cell_delete(&s, a, a - 1..a), CellEdit::Refused);
        assert_eq!(
            cell_delete(&s, a + 1, a..a + 1),
            CellEdit::Edit(Change::delete(a..a + 1))
        );
        assert_eq!(cell_delete(&s, a + 1, a + 1..a + 2), CellEdit::Refused);
        let bb = DOC.find("bb").unwrap();
        assert_eq!(
            cell_delete(&s, bb + 2, a..bb + 2),
            CellEdit::Edit(Change::delete(bb..bb + 2))
        );
        let doc = "| a<br>b\\|c |\n| - |";
        let s = EditorState::new(doc);
        assert_eq!(
            cell_delete(&s, 7, 6..7),
            CellEdit::Edit(Change::delete(3..7))
        );
        assert_eq!(
            cell_delete(&s, 10, 9..10),
            CellEdit::Edit(Change::delete(8..10))
        );
    }

    #[test]
    fn backspace_below_a_table_selects_it_then_deletes_it() {
        let table = DOC.find("| a").unwrap()..DOC.rfind("|  |").unwrap() + 4;
        let below = table.end + 1;
        let mut s = state(below);
        assert!(s.delete_backward());
        assert_eq!(s.doc(), DOC);
        assert_eq!(
            s.selection().main(),
            SelectionRange::new(table.start, table.end)
        );
        assert!(s.delete_backward());
        assert_eq!(s.doc(), "intro\n\n\n\nend");
        let mut s = state(table.start - 1);
        assert!(s.delete_forward());
        assert_eq!(s.doc(), DOC);
        assert_eq!(s.selection().main().from(), table.start);
    }

    #[test]
    fn backspace_and_delete_keep_the_cells() {
        let a = DOC.find("| a").unwrap() + 2;
        let mut s = state(a);
        assert!(s.delete_backward());
        assert_eq!(s.doc(), DOC);
        s.select(Selection::cursor(a + 1)).unwrap();
        assert!(s.delete_forward());
        assert_eq!(s.doc(), DOC);
        assert!(s.delete_backward());
        assert!(s.doc().contains("|  | bb |"));
        assert!(s.delete_group(false));
        assert_eq!(s.doc().matches('|').count(), DOC.matches('|').count());
        assert!(s.undo());
        assert_eq!(s.doc(), DOC);
        assert_eq!(at(&s), a + 1);
    }

    #[test]
    fn words_typed_in_an_empty_cell_keep_their_spaces() {
        let empty = DOC.rfind("|  |").unwrap() + 2;
        let mut s = state(empty);
        for (i, c) in "a b  c".chars().enumerate() {
            s.set_time(i as u64 * 50);
            assert!(s.type_text(None, &c.to_string(), crate::history::UserEvent::Type));
        }
        assert!(s.doc().contains("| 1 | a b  c |"), "{:?}", s.doc());
        assert!(s.type_text(None, " ", crate::history::UserEvent::Type));
        let head = at(&s);
        assert!(cell_at(&s, head).is_some());
        assert!(s.delete_backward());
        assert!(s.doc().contains("| 1 | a b  c |"), "{:?}", s.doc());
        assert!(s.type_text(None, " ", crate::history::UserEvent::Type));
        s.select(Selection::cursor(at(&s) + 1)).unwrap();
        assert!(grid_at(&s, at(&s)).is_none() || cell_at(&s, at(&s)).is_some());
    }

    #[test]
    fn typing_in_a_cell_is_one_undo_step_and_touches_only_the_cell() {
        let a = DOC.find("| a").unwrap() + 3;
        let mut s = state(a);
        for (i, typed) in ["x", "|", "y", "\n", "z"].iter().enumerate() {
            s.set_time(i as u64 * 50);
            let before = s.doc().to_owned();
            assert!(s.type_text(None, typed, crate::history::UserEvent::Type));
            let head = at(&s);
            assert_eq!(
                &s.doc()[head..],
                &before[head - (s.doc().len() - before.len())..]
            );
            assert_eq!(&s.doc()[..a], &before[..a]);
        }
        assert!(s.doc().contains("| ax\\|y<br>z | bb |"));
        assert!(s.undo());
        assert_eq!(s.doc(), DOC);
        assert_eq!(at(&s), a);
        assert!(cell_at(&s, at(&s)).is_some());
    }

    fn selected(s: &EditorState) -> &str {
        &s.doc()[s.selection().main().range()]
    }

    #[test]
    fn tab_and_shift_tab_walk_the_cells_and_select_them() {
        let a = DOC.find("| a").unwrap() + 2;
        let mut s = state(a);
        assert!(s.table_tab(false));
        assert_eq!(selected(&s), "bb");
        assert!(s.table_tab(false));
        assert_eq!(selected(&s), "1");
        assert!(s.table_tab(true));
        assert_eq!(selected(&s), "bb");
        let empty = DOC.rfind("|  |").unwrap() + 2;
        s.select(Selection::cursor(empty)).unwrap();
        assert!(s.table_tab(false));
        assert!(
            s.doc().contains("| 1 |  |\n|   |  |\n\nend"),
            "{:?}",
            s.doc()
        );
        let caret = at(&s);
        assert_eq!(cell_at(&s, caret).map(|(_, r, c)| (r, c)), Some((3, 0)));
        assert!(s.undo());
        assert_eq!(s.doc(), DOC);
        let mut s = state(a);
        assert!(s.table_tab(true));
        assert_eq!(at(&s), DOC.find("| a").unwrap() - 1);
        assert!(!state(0).table_tab(false));
    }

    #[test]
    fn tab_into_a_missing_cell_completes_the_row() {
        let doc = "| a | b | c |\n| - | - | - |\n| 1 |\n\nx";
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(doc.find('1').unwrap())).unwrap();
        assert!(s.table_tab(false));
        assert_eq!(s.doc(), "| a | b | c |\n| - | - | - |\n| 1 |  |\n\nx");
        let caret = at(&s);
        assert_eq!(cell_at(&s, caret).map(|(_, r, c)| (r, c)), Some((2, 1)));
    }

    #[test]
    fn enter_goes_down_adds_a_row_and_leaves_on_an_empty_one() {
        let a = DOC.find("| a").unwrap() + 2;
        let mut s = state(a);
        assert!(s.table_enter());
        assert_eq!(selected(&s), "1");
        let bb = DOC.find("bb").unwrap();
        let mut s = state(bb);
        assert!(s.table_enter());
        assert_eq!(at(&s), DOC.rfind("|  |").unwrap() + 2);
        assert!(s.table_enter());
        let caret = at(&s);
        assert_eq!(cell_at(&s, caret).map(|(_, r, c)| (r, c)), Some((3, 1)));
        assert!(s.table_enter());
        assert_eq!(s.doc(), "intro\n\n| a | bb |\n| - | -- |\n| 1 |  |\n\nend");
        assert!(grid_at(&s, at(&s)).is_none());
        assert_eq!(s.doc()[at(&s)..].lines().next(), Some(""));
        let doc = "| a |\n| - |\n| x |\n|  |";
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(doc.len() - 2)).unwrap();
        assert!(s.table_enter());
        assert_eq!(s.doc(), "| a |\n| - |\n| x |\n");
        assert_eq!(at(&s), s.doc().len());
    }

    #[test]
    fn leaving_by_an_edge_creates_a_line_when_none_is_there() {
        let doc = "| a |\n| - |\n| x |";
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(doc.find('x').unwrap())).unwrap();
        assert!(s.leave_table(true));
        assert_eq!(s.doc(), "| a |\n| - |\n| x |\n");
        assert_eq!(at(&s), s.doc().len());
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(2)).unwrap();
        assert!(s.leave_table(false));
        assert_eq!(s.doc(), "\n| a |\n| - |\n| x |");
        assert_eq!(at(&s), 0);
        let doc = "> | a |\n> | - |\n> | x |";
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(doc.find('x').unwrap())).unwrap();
        assert!(s.table_tab(false));
        assert!(s.doc().ends_with("> | x |\n> |   |"), "{:?}", s.doc());
    }

    #[test]
    fn select_all_takes_the_cell_then_the_document() {
        let bb = DOC.find("bb").unwrap();
        let mut s = state(bb + 1);
        assert!(s.select_cell_or_all());
        assert_eq!(selected(&s), "bb");
        assert!(s.select_cell_or_all());
        assert_eq!(s.selection().main().range(), 0..DOC.len());
    }

    #[test]
    fn word_motions_stop_at_the_cell_edge_then_cross_it() {
        let doc = "| end.| next word |\n| - | - |";
        let mut s = EditorState::new(doc);
        s.select(Selection::cursor(2)).unwrap();
        assert!(s.move_group(true, false));
        assert_eq!(at(&s), 5, "end of `end`");
        assert!(s.move_group(true, false));
        assert_eq!(at(&s), 6, "the content's end, before the pipe");
        assert!(s.move_group(true, false));
        assert_eq!(&doc[at(&s)..at(&s) + 4], "next");
        assert!(s.move_group(false, false));
        assert_eq!(at(&s), 6);
    }
}
