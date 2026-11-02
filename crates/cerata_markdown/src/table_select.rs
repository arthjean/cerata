//! Rectangular selection of cells in a drawn table (US-054, ADR 0007
//! decision 2): an anchor cell and a head cell, projected as one selection
//! range per selected cell, so formatting, deletion and copy act on each
//! cell. The state lives until the next commit that does not come from it.

use std::ops::RangeInclusive;

use crate::cells::{self, CellEdit};
use crate::change::Change;
use crate::history::UserEvent;
use crate::selection::{Selection, SelectionRange};
use crate::state::{EditorState, Transaction};
use crate::table::{CellKind, Grid, RowKind};

/// Cells from `anchor` to `head`, as `(row, column)` in `Grid::rows` (the
/// delimiter row is never one), of the table starting at `table`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableSelection {
    pub table: usize,
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl TableSelection {
    pub fn rows(&self) -> RangeInclusive<usize> {
        self.anchor.0.min(self.head.0)..=self.anchor.0.max(self.head.0)
    }

    pub fn columns(&self) -> RangeInclusive<usize> {
        self.anchor.1.min(self.head.1)..=self.anchor.1.max(self.head.1)
    }

    /// `(row, column)` is in the rectangle.
    pub fn contains(&self, row: usize, column: usize) -> bool {
        self.rows().contains(&row) && self.columns().contains(&column)
    }

    /// The selected cells of `grid` that have source, row by row.
    pub fn cells<'a>(&self, grid: &'a Grid) -> Vec<(usize, usize, &'a crate::table::Cell)> {
        let mut out = Vec::new();
        for r in self.rows() {
            let Some(row) = grid.rows.get(r) else {
                continue;
            };
            if row.kind == RowKind::Delimiter {
                continue;
            }
            for c in self.columns() {
                if let Some(cell) = row.cells.get(c).filter(|c| c.kind == CellKind::InGrid) {
                    out.push((r, c, cell));
                }
            }
        }
        out
    }
}

/// The grid row `steps` drawn rows from `row`, over the delimiter row,
/// clamped to the table.
fn step_row(grid: &Grid, row: usize, steps: isize) -> usize {
    let drawn: Vec<usize> = grid.text_rows().map(|(r, _)| r).collect();
    let at = drawn.iter().position(|r| *r == row).unwrap_or(0) as isize;
    let to = (at + steps).clamp(0, drawn.len() as isize - 1) as usize;
    drawn[to]
}

impl EditorState {
    pub fn table_selection(&self) -> Option<&TableSelection> {
        self.cell_selection.as_ref()
    }

    /// The grid of the rectangular selection, if it still has one.
    pub fn table_selection_grid(&self) -> Option<(TableSelection, Grid)> {
        let ts = *self.cell_selection.as_ref()?;
        let grid = cells::grid_at(self, ts.table)?;
        (grid.range.start == ts.table).then_some((ts, grid))
    }

    /// Selects the cells from `anchor` to `head` of `grid`: one range per
    /// non-empty cell (the head cell's insertion point when all are
    /// empty), the head cell's range as the main one.
    pub fn select_cells(
        &mut self,
        grid: &Grid,
        anchor: (usize, usize),
        head: (usize, usize),
    ) -> bool {
        let ts = TableSelection {
            table: grid.range.start,
            anchor,
            head,
        };
        let cells = ts.cells(grid);
        let mut ranges: Vec<SelectionRange> = cells
            .iter()
            .filter(|(_, _, c)| !c.content.is_empty())
            .map(|(_, _, c)| SelectionRange::new(c.content.start, c.content.end))
            .collect();
        let main = cells
            .iter()
            .filter(|(_, _, c)| !c.content.is_empty())
            .position(|(r, c, _)| (*r, *c) == head);
        if ranges.is_empty() {
            let Some(cell) = grid.cell(head.0, head.1) else {
                return false;
            };
            ranges.push(SelectionRange::cursor(cell.content.start));
        }
        let main = main.unwrap_or(ranges.len() - 1);
        let ok = self
            .apply(
                Transaction::new()
                    .select(Selection::new(ranges, main))
                    .event(UserEvent::Select)
                    .scroll(),
            )
            .is_ok();
        self.cell_selection = Some(ts);
        ok
    }

    /// Shift+arrow: moves the rectangle's head cell by `rows` drawn rows and
    /// `columns` columns; from a caret or a range in one cell, at its
    /// content's edge in that direction (`at_edge`), starts a rectangle.
    /// `false` when it does not apply.
    pub fn extend_cells(&mut self, rows: isize, columns: isize, at_edge: bool) -> bool {
        if let Some((ts, grid)) = self.table_selection_grid() {
            let head = (
                step_row(&grid, ts.head.0, rows),
                (ts.head.1 as isize + columns).clamp(0, grid.columns() as isize - 1) as usize,
            );
            return self.select_cells(&grid, ts.anchor, head);
        }
        let main = self.selection().main();
        if self.selection().ranges().len() != 1 {
            return false;
        }
        let Some((grid, row, column)) = cells::cell_at(self, main.head) else {
            return false;
        };
        let Some(cell) = grid.cell(row, column) else {
            return false;
        };
        if !cell.holds(main.anchor) {
            return false;
        }
        let edge = match columns {
            1 => main.head >= cell.content.end,
            -1 => main.head <= cell.content.start,
            _ => at_edge,
        };
        if !edge {
            return false;
        }
        let head = (
            step_row(&grid, row, rows),
            (column as isize + columns).clamp(0, grid.columns() as isize - 1) as usize,
        );
        if head == (row, column) {
            return false;
        }
        self.select_cells(&grid, (row, column), head)
    }

    /// An arrow without Shift on a rectangle: the caret at the start of its
    /// first cell (`forward` false: Up, Left) or the end of its last.
    pub fn collapse_cells(&mut self, forward: bool) -> bool {
        let Some((ts, grid)) = self.table_selection_grid() else {
            return false;
        };
        let rows = ts.rows();
        let columns = ts.columns();
        let (row, column) = if forward {
            (*rows.end(), *columns.end())
        } else {
            (*rows.start(), *columns.start())
        };
        let Some(cell) = grid.cell(row, column) else {
            return false;
        };
        let caret = if forward {
            cell.content.end
        } else {
            cell.content.start
        };
        self.apply(
            Transaction::new()
                .select(Selection::cursor(caret))
                .event(UserEvent::Select)
                .scroll(),
        )
        .is_ok()
    }

    /// Typing over a rectangle: the anchor cell's content is replaced and
    /// the selection is that cell's caret. `None` without a rectangle.
    pub(crate) fn type_over_cells(&mut self, text: &str, event: UserEvent) -> Option<bool> {
        let (ts, grid) = self.table_selection_grid()?;
        let cell = grid.cell(ts.anchor.0, ts.anchor.1)?;
        let change = match cells::cell_edit(self, cell.content.clone(), text) {
            CellEdit::Edit(change) => change,
            CellEdit::Outside | CellEdit::Refused => return Some(false),
        };
        let caret = change.from + change.insert.len();
        Some(
            self.apply(
                Transaction::new()
                    .change(change)
                    .select(Selection::cursor(caret))
                    .event(event)
                    .scroll(),
            )
            .is_ok(),
        )
    }

    /// Backspace or Delete over a rectangle: every selected cell emptied,
    /// rows and columns kept, in one step. `None` without a rectangle.
    pub(crate) fn clear_cells(&mut self) -> Option<bool> {
        let (ts, grid) = self.table_selection_grid()?;
        let changes: Vec<Change> = ts
            .cells(&grid)
            .into_iter()
            .filter(|(_, _, c)| !c.content.is_empty())
            .map(|(_, _, c)| Change::delete(c.content.clone()))
            .collect();
        let head = grid.cell(ts.head.0, ts.head.1)?.content.start;
        if changes.is_empty() {
            return Some(false);
        }
        let removed: usize = changes
            .iter()
            .filter(|c| c.from < head)
            .map(|c| c.to - c.from)
            .sum();
        Some(
            self.apply(
                Transaction::new()
                    .changes(changes)
                    .select(Selection::cursor(head - removed))
                    .event(UserEvent::Delete)
                    .scroll(),
            )
            .is_ok(),
        )
    }
}

#[cfg(test)]
mod tests;
