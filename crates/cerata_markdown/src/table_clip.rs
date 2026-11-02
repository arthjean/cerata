//! Cells on the clipboard (US-055, ADR 0007 decision 10): a rectangle of
//! cells copies as a GFM table (its first row as the header), with the
//! grid itself as metadata for the host to attach; a grid pasted into a
//! cell (that metadata, a GFM table or TSV of at least two cells) fills
//! the cells from it, adding the rows and columns it needs, in one step.

use crate::cells;
use crate::state::EditorState;
use crate::table::{Align, CellKind, Grid, RowKind};
use crate::table_ops::{At, Draft};
use crate::text::Text;
use crate::tree::{MarkdownTree, NodeKind};

/// Cells pasted at once, at most (ADR 0007, decision 10).
pub const PASTE_LIMIT: usize = 10_000;

/// A block of cells: each one's text as cell source (`\|` and `<br>`
/// escaped), and the alignment of each column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellBlock {
    pub rows: Vec<Vec<String>>,
    pub alignments: Vec<Option<Align>>,
}

/// What a paste into a table did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteCells {
    /// The caret is not in a drawn cell: paste as text.
    Outside,
    Pasted,
    /// Over [`PASTE_LIMIT`]: nothing pasted.
    TooLarge(usize),
}

impl CellBlock {
    pub fn cell_count(&self) -> usize {
        self.rows.iter().map(Vec::len).sum()
    }

    pub fn columns(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    /// The block as a GFM table: its first row is the header.
    pub fn to_gfm(&self) -> String {
        let columns = self.columns().max(1);
        let row = |cells: &[String]| {
            let mut line = String::from("|");
            for c in 0..columns {
                line.push(' ');
                line.push_str(cells.get(c).map_or("", String::as_str));
                line.push_str(" |");
            }
            line
        };
        let mut out = Vec::with_capacity(self.rows.len() + 1);
        out.push(row(self.rows.first().map_or(&[][..], Vec::as_slice)));
        let mut delimiter = String::from("|");
        for c in 0..columns {
            delimiter.push_str(match self.alignments.get(c).copied().flatten() {
                Some(Align::Left) => " :-- |",
                Some(Align::Center) => " :-: |",
                Some(Align::Right) => " --: |",
                None => " --- |",
            });
        }
        out.push(delimiter);
        out.extend(self.rows.iter().skip(1).map(|r| row(r)));
        out.join("\n")
    }

    /// Every cell made a valid cell source: `|` escaped, line breaks as
    /// `<br>`, edge spaces trimmed.
    pub fn escaped(mut self) -> Self {
        for row in &mut self.rows {
            for cell in row.iter_mut() {
                *cell = cells::escape(cell.trim(), false).0.trim().to_owned();
            }
        }
        self
    }
}

/// `text` when it is one GFM table and nothing else: its cells' sources.
pub fn parse_gfm(text: &str) -> Option<CellBlock> {
    let trimmed = text.trim_matches(['\n', '\r']);
    let doc = Text::new(trimmed);
    let tree = MarkdownTree::parse(trimmed);
    let node = tree.iter().find(|n| n.kind() == NodeKind::Table)?;
    let grid = Grid::from_node(&doc, node)?;
    if grid.zone() != (0..trimmed.len()) {
        return None;
    }
    let columns = grid.columns();
    let rows = grid
        .text_rows()
        .map(|(_, r)| {
            r.cells
                .iter()
                .take(columns)
                .map(|c| match c.kind {
                    CellKind::InGrid => trimmed[c.content.clone()].to_owned(),
                    _ => String::new(),
                })
                .collect()
        })
        .collect();
    Some(CellBlock {
        rows,
        alignments: grid.alignments,
    })
}

/// Tab-separated values from a spreadsheet: at least two cells, so at
/// least one tab. Plain text, to escape.
pub fn parse_tsv(text: &str) -> Option<CellBlock> {
    if !text.contains('\t') {
        return None;
    }
    let body = text
        .strip_suffix("\r\n")
        .or(text.strip_suffix('\n'))
        .unwrap_or(text);
    let rows = quoted_rows(body).unwrap_or_else(|| {
        body.split('\n')
            .map(|line| {
                line.strip_suffix('\r')
                    .unwrap_or(line)
                    .split('\t')
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    });
    let block = CellBlock {
        alignments: Vec::new(),
        rows,
    };
    (block.cell_count() >= 2).then(|| block.escaped())
}

/// The rows of `body` as spreadsheets write them: a cell holding a tab, a
/// line break or a quote is quoted, its quotes doubled (`"l1\nl2"\tx`).
/// `None` without a quoted cell or with one left open: split plainly.
fn quoted_rows(body: &str) -> Option<Vec<Vec<String>>> {
    let mut rows = vec![Vec::new()];
    let mut cell = String::new();
    let mut quoted_any = false;
    let mut chars = body.chars().peekable();
    let mut at_start = true;
    while let Some(c) = chars.next() {
        match c {
            '"' if at_start => {
                quoted_any = true;
                loop {
                    match chars.next()? {
                        '"' if chars.peek() == Some(&'"') => {
                            chars.next();
                            cell.push('"');
                        }
                        '"' => break,
                        other => cell.push(other),
                    }
                }
                at_start = false;
            }
            '\t' => {
                rows.last_mut()?.push(std::mem::take(&mut cell));
                at_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                rows.last_mut()?.push(std::mem::take(&mut cell));
                rows.push(Vec::new());
                at_start = true;
            }
            other => {
                cell.push(other);
                at_start = false;
            }
        }
    }
    rows.last_mut()?.push(cell);
    quoted_any.then_some(rows)
}

impl EditorState {
    /// The rectangle of cells as a block: the source of each selected
    /// cell (empty for a missing one) and its columns' alignments.
    pub fn copied_cells(&self) -> Option<CellBlock> {
        let (ts, grid) = self.table_selection_grid()?;
        let doc = self.doc();
        let rows = ts
            .rows()
            .filter(|r| {
                grid.rows
                    .get(*r)
                    .is_some_and(|row| row.kind != RowKind::Delimiter)
            })
            .map(|r| {
                ts.columns()
                    .map(|c| {
                        grid.cell(r, c)
                            .filter(|cell| cell.kind == CellKind::InGrid)
                            .map_or_else(String::new, |cell| doc[cell.content.clone()].to_owned())
                    })
                    .collect()
            })
            .collect();
        let alignments = ts
            .columns()
            .map(|c| grid.alignments.get(c).copied().flatten())
            .collect();
        Some(CellBlock { rows, alignments })
    }

    /// Cut over a rectangle: its cells emptied (the host copied them
    /// first), one undo step.
    pub fn cut_cells(&mut self) -> bool {
        self.clear_cells().unwrap_or(false)
    }

    /// Pastes `block` into the table from the caret's cell (the rectangle's
    /// top-left one), adding the rows and columns it needs: one undo step,
    /// the caret at the end of the last cell filled.
    pub fn paste_cells(&mut self, block: &CellBlock) -> PasteCells {
        // The rectangle it fills, short and empty rows included: what the
        // paste creates, which the limit bounds.
        let count = block.rows.len().saturating_mul(block.columns());
        if count > PASTE_LIMIT {
            return PasteCells::TooLarge(count);
        }
        let start = match self.table_selection_grid() {
            Some((ts, grid)) => Some((grid, *ts.rows().start(), *ts.columns().start())),
            None if self.selection().ranges().len() == 1 => {
                let main = self.selection().main();
                cells::cell_at(self, main.head).filter(|(g, r, c)| {
                    g.cell(*r, *c)
                        .is_some_and(|cell| cell.holds_edit(main.anchor))
                })
            }
            None => None,
        };
        let Some((grid, row, column)) = start else {
            return PasteCells::Outside;
        };
        if count == 0 {
            return PasteCells::Pasted;
        }
        let doc = self.doc().to_owned();
        let mut d = Draft::new(&doc, &grid);
        d.complete();
        // Draft positions of the drawn rows: the header, then the body.
        let first = if row == 0 { 0 } else { row - 1 };
        let needed_rows = first + block.rows.len();
        let needed_columns = column + block.columns();
        while d.columns() < needed_columns {
            d.push_column();
        }
        while d.rows.len() - 1 < needed_rows {
            let last = d.rows.len() - 1;
            let new = d.blank_row(last);
            d.rows.push(new);
        }
        let position = |drawn: usize| if drawn == 0 { 0 } else { drawn + 1 };
        let mut last = (position(first), column);
        for (i, values) in block.rows.iter().enumerate() {
            let at = position(first + i);
            for (j, value) in values.iter().enumerate() {
                let value = cells::escape(value.trim(), false).0;
                d.rows[at].slots[column + j] = format!(" {} ", value.trim());
                last = (at, column + j);
            }
        }
        // Pasting what is already there changes nothing, and that is fine.
        let _ = d.commit(self, last.0, last.1, At::Offset(usize::MAX));
        PasteCells::Pasted
    }
}

#[cfg(test)]
mod tests;
