use std::cmp::Ordering;
use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use crate::cells;
use crate::change::Change;
use crate::commands::minimal_edit;
use crate::history::UserEvent;
use crate::selection::Selection;
use crate::state::{EditorState, Transaction};
use crate::table::{self, Align, CellKind, Grid, RowKind};

pub const IDS: &[&str] = &[
    "table.rowAbove",
    "table.rowBelow",
    "table.rowDuplicate",
    "table.rowUp",
    "table.rowDown",
    "table.rowDelete",
    "table.columnLeft",
    "table.columnRight",
    "table.columnDuplicate",
    "table.columnMoveLeft",
    "table.columnMoveRight",
    "table.columnDelete",
    "table.alignLeft",
    "table.alignCenter",
    "table.alignRight",
    "table.alignNone",
    "table.sortAscending",
    "table.sortDescending",
    "table.format",
    "table.editSource",
    "table.delete",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    source: Option<usize>,
    pub(crate) kind: RowKind,
    head: String,
    pub(crate) slots: Vec<String>,
    tail: String,
}

pub(crate) struct Draft<'a> {
    pub(crate) grid: &'a Grid,
    eol: &'static str,
    old: Vec<Range<usize>>,
    prefixes: Vec<&'a str>,
    pub(crate) rows: Vec<Row>,
    columns: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum At {
    Start,
    Offset(usize),
}

pub(crate) fn eol_of(doc: &str, grid: &Grid) -> &'static str {
    if doc[grid.zone()].contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn close(row: &mut Row) {
    if !row.tail.contains('|') {
        if let Some(last) = row.slots.last_mut() {
            last.push(' ');
        }
        row.tail.insert(0, '|');
    }
}

fn open(row: &mut Row) {
    if !row.head.contains('|') {
        row.head.push('|');
    }
}

fn blank(width: usize) -> String {
    " ".repeat(width.max(2))
}

impl<'a> Draft<'a> {
    pub(crate) fn new(doc: &'a str, grid: &'a Grid) -> Self {
        let mut old = Vec::with_capacity(grid.rows.len());
        let mut prefixes = Vec::with_capacity(grid.rows.len());
        let rows = grid
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                old.push(r.line.clone());
                prefixes.push(&doc[r.prefix.clone()]);
                let present: Vec<&table::Cell> = r
                    .cells
                    .iter()
                    .filter(|c| c.kind != CellKind::Missing)
                    .collect();
                let (head, slots, tail) = match (present.first(), present.last()) {
                    (Some(first), Some(last)) => (
                        doc[r.prefix.end..first.slot.start].to_owned(),
                        present
                            .iter()
                            .map(|c| doc[c.slot.clone()].to_owned())
                            .collect(),
                        doc[last.slot.end..r.line.end].to_owned(),
                    ),
                    _ => (
                        doc[r.prefix.end..r.line.end].to_owned(),
                        Vec::new(),
                        String::new(),
                    ),
                };
                Row {
                    source: Some(i),
                    kind: r.kind,
                    head,
                    slots,
                    tail,
                }
            })
            .collect();
        Self {
            grid,
            eol: eol_of(doc, grid),
            old,
            prefixes,
            rows,
            columns: grid.columns(),
        }
    }

    pub(crate) fn columns(&self) -> usize {
        self.columns
    }

    pub(crate) fn push_column(&mut self) {
        for row in &mut self.rows {
            close(row);
            let slot = if row.kind == RowKind::Delimiter {
                " --- ".to_owned()
            } else {
                blank(3)
            };
            row.slots.push(slot);
        }
        self.columns += 1;
    }

    pub(crate) fn complete(&mut self) {
        let columns = self.columns();
        for row in &mut self.rows {
            if row.slots.len() >= columns {
                continue;
            }
            close(row);
            while row.slots.len() < columns {
                row.slots.push(blank(2));
            }
        }
    }

    pub(crate) fn blank_row(&self, reference: usize) -> Row {
        let slots = (0..self.columns())
            .map(|c| {
                let width = self
                    .rows
                    .get(reference)
                    .and_then(|r| r.slots.get(c))
                    .map_or(2, |s| s.chars().count());
                blank(width)
            })
            .collect();
        Row {
            source: None,
            kind: RowKind::Body,
            head: "|".into(),
            slots,
            tail: "|".into(),
        }
    }

    fn in_place(&mut self) {
        for (i, row) in self.rows.iter_mut().enumerate() {
            row.source = Some(i);
        }
    }

    fn prefix(&self, i: usize) -> &'a str {
        self.prefixes
            .get(i)
            .or(self.prefixes.last())
            .copied()
            .unwrap_or("")
    }

    fn line(&self, i: usize) -> String {
        let row = &self.rows[i];
        format!(
            "{}{}{}{}",
            self.prefix(i),
            row.head,
            row.slots.join("|"),
            row.tail
        )
    }

    fn changes(&self, doc: &str) -> Vec<Change> {
        let lines: Vec<String> = (0..self.rows.len()).map(|i| self.line(i)).collect();
        let n = self.old.len();
        let mut kept = vec![false; n];
        for row in &self.rows {
            if let Some(j) = row.source {
                kept[j] = true;
            }
        }
        let mut changes = Vec::new();
        let mut j = 0;
        while j < n {
            if kept[j] {
                j += 1;
                continue;
            }
            let first = j;
            while j < n && !kept[j] {
                j += 1;
            }
            let range = if j < n {
                self.old[first].start..self.old[j].start
            } else if first > 0 {
                self.old[first - 1].end..self.old[n - 1].end
            } else {
                self.old[0].start..self.old[n - 1].end
            };
            changes.push(Change::delete(range));
        }
        let mut pending = String::new();
        for (row, line) in self.rows.iter().zip(&lines) {
            match row.source {
                None => {
                    pending.push_str(line);
                    pending.push_str(self.eol);
                }
                Some(j) => {
                    let old = &self.old[j];
                    if !pending.is_empty() {
                        changes.push(Change::insert(old.start, std::mem::take(&mut pending)));
                    }
                    if let Some((range, insert)) = minimal_edit(&doc[old.clone()], line) {
                        changes.push(Change::new(
                            old.start + range.start..old.start + range.end,
                            insert,
                        ));
                    }
                }
            }
        }
        if !pending.is_empty() {
            let tail = pending.trim_end_matches(self.eol);
            if let Some(last) = self.old.last() {
                changes.push(Change::insert(last.end, format!("{}{tail}", self.eol)));
            }
        }
        changes
    }

    fn content(&self, row: usize, column: usize, line_start: usize) -> Option<Range<usize>> {
        let line = self.line(row);
        let prefix = self.prefix(row).len();
        let pipes = table::pipes_in(&line, prefix..line.len());
        let cells = table::row_cells(&line, prefix..line.len(), &pipes);
        let cell = cells.get(column).or(cells.last())?;
        Some(line_start + cell.content.start..line_start + cell.content.end)
    }

    pub(crate) fn commit(self, state: &mut EditorState, row: usize, column: usize, at: At) -> bool {
        let doc = state.doc();
        let changes = self.changes(doc);
        if changes.is_empty() {
            return false;
        }
        let start = self.old.first().map_or(0, |r| r.start);
        let row = row.min(self.rows.len().saturating_sub(1));
        let line_start = start
            + (0..row)
                .map(|i| self.line(i).len() + self.eol.len())
                .sum::<usize>();
        let line = self.line(row);
        let selection = self.content(row, column, line_start).map(|content| {
            let floor = |pos: usize| {
                let mut pos = pos.min(line_start + line.len());
                while pos > line_start && !line.is_char_boundary(pos - line_start) {
                    pos -= 1;
                }
                pos
            };
            match at {
                At::Start => Selection::cursor(content.start),
                At::Offset(n) => {
                    Selection::cursor(floor(content.start.saturating_add(n).min(content.end)))
                }
            }
        });
        let mut tr = Transaction::new().changes(changes);
        if let Some(selection) = selection {
            tr = tr.select(selection);
        }
        state.apply(tr.event(UserEvent::Command).scroll()).is_ok()
    }
}

fn context(state: &EditorState) -> Option<(Grid, usize, usize, usize)> {
    let head = state.selection().main().head;
    let (grid, row, column) = cells::cell_at(state, head)?;
    let cell = grid.cell(row, column)?;
    let offset = head
        .saturating_sub(cell.content.start)
        .min(cell.content.len());
    Some((grid, row, column, offset))
}

pub fn enabled(state: &EditorState, id: &str) -> bool {
    let Some((grid, row, column, _)) = context(state) else {
        return false;
    };
    let body = row >= 2;
    let last = grid.rows.len() - 1;
    match id {
        "table.rowDelete" => body,
        "table.rowUp" => body && row > 2,
        "table.rowDown" => body && row < last,
        "table.columnDelete" => grid.columns() > 1,
        "table.columnMoveLeft" => column > 0,
        "table.columnMoveRight" => column + 1 < grid.columns(),
        _ => IDS.contains(&id),
    }
}

pub fn run(state: &mut EditorState, id: &str) -> bool {
    if !enabled(state, id) {
        return false;
    }
    let Some((grid, row, column, offset)) = context(state) else {
        return false;
    };
    match id {
        "table.editSource" => return state.edit_table_source(&grid),
        "table.delete" => return delete_table(state, &grid),
        _ => {}
    }
    let doc = state.doc().to_owned();
    let mut d = Draft::new(&doc, &grid);
    d.complete();
    let (row, column, at) = match id {
        "table.rowAbove" if row == 0 => {
            let delimiter = d.rows[1].clone();
            d.rows.remove(1);
            let mut header = d.blank_row(0);
            header.kind = RowKind::Header;
            d.rows.insert(0, header);
            d.rows.insert(
                1,
                Row {
                    source: None,
                    ..delimiter
                },
            );
            (0, column, At::Start)
        }
        "table.rowAbove" => {
            let new = d.blank_row(row);
            d.rows.insert(row, new);
            (row, column, At::Start)
        }
        "table.rowBelow" => {
            let at = if row == 0 { 2 } else { row + 1 };
            let new = d.blank_row(row);
            d.rows.insert(at, new);
            (at, column, At::Start)
        }
        "table.rowDuplicate" => {
            let at = if row == 0 { 2 } else { row + 1 };
            let mut copy = d.rows[row].clone();
            copy.source = None;
            copy.kind = RowKind::Body;
            d.rows.insert(at, copy);
            (at, column, At::Offset(offset))
        }
        "table.rowDelete" => {
            d.rows.remove(row);
            let next = if row < d.rows.len() {
                row
            } else if row > 2 {
                row - 1
            } else {
                0
            };
            (next, column, At::Start)
        }
        "table.rowUp" | "table.rowDown" => {
            let other = if id == "table.rowUp" {
                row - 1
            } else {
                row + 1
            };
            d.rows.swap(row, other);
            d.in_place();
            (other, column, At::Offset(offset))
        }
        "table.columnLeft" | "table.columnRight" | "table.columnDuplicate" => {
            let at = if id == "table.columnLeft" {
                column
            } else {
                column + 1
            };
            for r in &mut d.rows {
                let reference = r.slots.get(column).cloned().unwrap_or_default();
                let slot = if id == "table.columnDuplicate" || r.kind == RowKind::Delimiter {
                    reference
                } else {
                    blank(reference.chars().count())
                };
                let at = at.min(r.slots.len());
                if at == r.slots.len() {
                    close(r);
                }
                if at == 0 {
                    open(r);
                }
                r.slots.insert(at, slot);
            }
            let at_caret = if id == "table.columnDuplicate" {
                At::Offset(offset)
            } else {
                At::Start
            };
            (row, at, at_caret)
        }
        "table.columnDelete" => {
            for r in &mut d.rows {
                if column < r.slots.len() {
                    r.slots.remove(column);
                }
                if r.slots.len() == 1 && !r.head.contains('|') && !r.tail.contains('|') {
                    open(r);
                    close(r);
                }
            }
            (row, column.min(d.columns() - 2), At::Start)
        }
        "table.columnMoveLeft" | "table.columnMoveRight" => {
            let other = if id == "table.columnMoveLeft" {
                column - 1
            } else {
                column + 1
            };
            for r in &mut d.rows {
                if column < r.slots.len() && other < r.slots.len() {
                    r.slots.swap(column, other);
                }
            }
            (row, other, At::Offset(offset))
        }
        "table.alignLeft" | "table.alignCenter" | "table.alignRight" | "table.alignNone" => {
            let align = match id {
                "table.alignLeft" => Some(Align::Left),
                "table.alignCenter" => Some(Align::Center),
                "table.alignRight" => Some(Align::Right),
                _ => None,
            };
            if let Some(slot) = d.rows[1].slots.get_mut(column) {
                *slot = realign(slot, align);
            }
            (row, column, At::Offset(offset))
        }
        "table.sortAscending" | "table.sortDescending" => {
            let descending = id == "table.sortDescending";
            let caret_row = d.rows[row].clone();
            let mut body = d.rows.split_off(2);
            let keys: Vec<String> = body
                .iter()
                .map(|r| sort_key(r.slots.get(column).map_or("", String::as_str)))
                .collect();
            let numbers: Vec<Option<f64>> = keys.iter().map(|k| number(k)).collect();
            let mut order: Vec<usize> = (0..body.len()).collect();
            order.sort_by(|a, b| {
                let ord = match (numbers[*a], numbers[*b]) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => natural_cmp(&keys[*a], &keys[*b]),
                };
                if descending { ord.reverse() } else { ord }
            });
            let mut sorted: Vec<Row> = order.iter().map(|i| body[*i].clone()).collect();
            d.rows.append(&mut sorted);
            body.clear();
            d.in_place();
            let at = if row >= 2 {
                d.rows
                    .iter()
                    .skip(2)
                    .position(|r| r.slots == caret_row.slots && r.head == caret_row.head)
                    .map_or(row, |p| p + 2)
            } else {
                row
            };
            (at, column, At::Offset(offset))
        }
        "table.format" => {
            format(&mut d);
            (row, column, At::Offset(offset))
        }
        _ => return false,
    };
    d.commit(state, row, column, at)
}

fn realign(slot: &str, align: Option<Align>) -> String {
    let lead = slot.len() - slot.trim_start().len();
    let trail = slot.len() - slot.trim_end().len();
    let marker = slot.trim();
    let width = marker.len().max(3);
    let dashes = |n: usize| "-".repeat(n);
    let marker = match align {
        Some(Align::Left) => format!(":{}", dashes(width - 1)),
        Some(Align::Center) => format!(":{}:", dashes(width - 2)),
        Some(Align::Right) => format!("{}:", dashes(width - 1)),
        None => dashes(width),
    };
    format!("{}{marker}{}", &slot[..lead], &slot[slot.len() - trail..])
}

fn sort_key(slot: &str) -> String {
    let content = slot.trim();
    let mut text = String::new();
    let mut rest = content;
    while let Some(i) = rest.find('<') {
        let tag_end = rest[i..].find('>').map(|e| i + e + 1);
        match tag_end {
            Some(end) if table::is_break_tag(&rest[i..end]) => {
                text.push_str(&rest[..i]);
                text.push(' ');
                rest = &rest[end..];
            }
            _ => {
                text.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    text.push_str(rest);
    crate::preview::plain_text(&crate::preview::render(&text)).to_lowercase()
}

fn number(key: &str) -> Option<f64> {
    let key = key.trim();
    let numeric = key.bytes().any(|b| b.is_ascii_digit())
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || b"+-.".contains(&b));
    numeric.then(|| key.parse::<f64>().ok()).flatten()
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a, b);
    loop {
        match (a.chars().next(), b.chars().next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = a.len() - a.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                let db = b.len() - b.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                let na = a[..da].trim_start_matches('0');
                let nb = b[..db].trim_start_matches('0');
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[da..];
                b = &b[db..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a = &a[x.len_utf8()..];
                b = &b[y.len_utf8()..];
            }
        }
    }
}

fn format(d: &mut Draft<'_>) {
    let columns = d.columns();
    let width = |s: &str| UnicodeWidthStr::width(s);
    let mut widths = vec![3; columns];
    for row in d.rows.iter().filter(|r| r.kind != RowKind::Delimiter) {
        for (c, slot) in row.slots.iter().take(columns).enumerate() {
            widths[c] = widths[c].max(width(slot.trim()));
        }
    }
    let alignments = d.grid.alignments.clone();
    for row in &mut d.rows {
        row.head = "|".into();
        row.tail = "|".into();
        for (c, slot) in row.slots.iter_mut().take(columns).enumerate() {
            let w = widths[c];
            let align = alignments.get(c).copied().flatten();
            let text = if row.kind == RowKind::Delimiter {
                realign(&"-".repeat(w), align)
            } else {
                let content = slot.trim();
                let free = w.saturating_sub(width(content));
                let (left, right) = match align {
                    Some(Align::Right) => (free, 0),
                    Some(Align::Center) => (free / 2, free - free / 2),
                    _ => (0, free),
                };
                format!("{}{content}{}", " ".repeat(left), " ".repeat(right))
            };
            *slot = format!(" {text} ");
        }
    }
}

fn delete_table(state: &mut EditorState, grid: &Grid) -> bool {
    let doc = state.doc();
    let zone = grid.zone();
    let eol = eol_of(doc, grid);
    let range = if doc[zone.end..].starts_with(eol) {
        zone.start..zone.end + eol.len()
    } else if doc[..zone.start].ends_with(eol) {
        zone.start - eol.len()..zone.end
    } else {
        zone.clone()
    };
    let caret = range.start;
    state
        .apply(
            Transaction::new()
                .change(Change::delete(range))
                .select(Selection::cursor(caret))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
}

#[cfg(test)]
mod tests;
