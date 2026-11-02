//! GFM tables: the grid of source ranges that rendering, typing and
//! commands share (ADR 0007), and the sizing rules of US-032, ported from
//! `components/editor-area/table-decorations.ts` (the alignment parse,
//! `:110-162`, and the sizing `tableTheme` asks the browser for, `:485-534`:
//! cells capped at 48ch, columns never narrower than their longest word).

use std::ops::Range;

use crate::text::Text;
use crate::tree::{Node, NodeKind};

/// Ceiling on the width one column may demand, in `ch`.
pub const CELL_MAX_WIDTH_CH: f32 = 48.0;
/// Line height of a cell, relative to the font size.
pub const CELL_LINE_HEIGHT: f32 = 1.4;
/// Vertical padding of a cell (both sides together), in em.
pub const CELL_VERTICAL_PADDING_EM: f32 = 1.0;
/// Horizontal padding of a cell, per side, in em.
pub const CELL_HORIZONTAL_PADDING_EM: f32 = 0.8;
/// Padding above and below the table (both together), in em.
pub const WIDGET_VERTICAL_PADDING_EM: f32 = 0.5;
pub const BORDER_WIDTH: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Which line of the table a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowKind {
    Header,
    Delimiter,
    Body,
}

/// Where a cell stands against the header's width (GFM).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellKind {
    /// One of the header's columns.
    InGrid,
    /// A body row shorter than the header: the column has no source yet;
    /// its slot and content are the insertion point at the row's end.
    Missing,
    /// Past the header's width: not drawn, as in GFM.
    Extra,
}

/// One cell, in document offsets.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    /// Between its two `|` (or the row's edge when a pipe is left out).
    pub slot: Range<usize>,
    /// The slot without its edge spaces. An empty cell's content is its
    /// insertion point: after the first space following its opening `|`,
    /// else right after that `|`.
    pub content: Range<usize>,
    pub kind: CellKind,
    /// The `<br>` tags of the content (`<br/>` and `<br />` too, any
    /// case), each drawn as a line break.
    pub breaks: Vec<Range<usize>>,
    /// The escaped pipes (`\|`) of the content, drawn as `|`.
    pub escapes: Vec<Range<usize>>,
}

impl Cell {
    /// The stretches of the content between its breaks, in order.
    pub fn segments(&self) -> Vec<Range<usize>> {
        let mut out = Vec::with_capacity(self.breaks.len() + 1);
        let mut start = self.content.start;
        for br in &self.breaks {
            out.push(start..br.start);
            start = br.end;
        }
        out.push(start..self.content.end);
        out
    }

    /// The `<br>` tags and the escaped pipes, in order: units the caret
    /// never splits.
    pub fn atoms(&self) -> Vec<Range<usize>> {
        let mut atoms: Vec<Range<usize>> =
            self.breaks.iter().chain(&self.escapes).cloned().collect();
        atoms.sort_by_key(|a| a.start);
        atoms
    }

    /// The caret positions of this cell: its content split around the
    /// inside of each atom, bounds included.
    pub fn stretches(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut start = self.content.start;
        for atom in self.atoms() {
            out.push(start..atom.start);
            start = atom.end;
        }
        out.push(start..self.content.end);
        out
    }

    /// `pos` is in this cell for an edit: in its content or in the spaces
    /// after it (an edit may leave the caret after a typed space, which GFM
    /// trims from the content), not inside a `<br>` or a `\|`.
    pub fn holds_edit(&self, pos: usize) -> bool {
        self.content.start <= pos
            && pos <= self.slot.end.max(self.content.end)
            && !self
                .breaks
                .iter()
                .chain(&self.escapes)
                .any(|b| b.start < pos && pos < b.end)
    }

    /// `pos` is a caret position of this cell: in its content, bounds
    /// included, and not inside a `<br>` or a `\|`.
    pub fn holds(&self, pos: usize) -> bool {
        self.content.start <= pos
            && pos <= self.content.end
            && !self
                .breaks
                .iter()
                .chain(&self.escapes)
                .any(|b| b.start < pos && pos < b.end)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GridRow {
    pub kind: RowKind,
    /// The whole line, its container prefix included.
    pub line: Range<usize>,
    /// Quote marks and list indentation before the row.
    pub prefix: Range<usize>,
    /// The cells: the header's columns first, then missing or extra ones.
    /// The delimiter row's cells hold its dashes.
    pub cells: Vec<Cell>,
}

/// A table's rows and cells, read from the tree's table node (ADR 0007,
/// decision 3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Grid {
    /// The table node.
    pub range: Range<usize>,
    /// One per column, from the delimiter row.
    pub alignments: Vec<Option<Align>>,
    /// The header, the delimiter row, then the body rows.
    pub rows: Vec<GridRow>,
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Unescaped `|` in `range` of `text`.
fn pipes_in(text: &str, range: Range<usize>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut escaped = false;
    for (offset, b) in text.as_bytes()[range.clone()].iter().enumerate() {
        if *b == b'|' && !escaped {
            out.push(range.start + offset);
        }
        escaped = !escaped && *b == b'\\';
    }
    out
}

/// The cells of a row from its pipes. A stretch before the first pipe or
/// after the last one is a cell only when it holds text.
fn row_cells(text: &str, row: Range<usize>, pipes: &[usize]) -> Vec<Cell> {
    let bytes = text.as_bytes();
    let blank = |r: &Range<usize>| bytes[r.clone()].iter().all(|b| is_ws(*b));
    let mut slots = Vec::new();
    let lead = row.start..pipes.first().copied().unwrap_or(row.end);
    if !blank(&lead) || pipes.is_empty() {
        slots.push(lead);
    }
    for pair in pipes.windows(2) {
        slots.push(pair[0] + 1..pair[1]);
    }
    if let Some(last) = pipes.last() {
        let tail = last + 1..row.end;
        if !blank(&tail) {
            slots.push(tail);
        }
    }
    slots
        .into_iter()
        .map(|slot| {
            let inner = &bytes[slot.clone()];
            let lead = inner.iter().take_while(|b| is_ws(**b)).count();
            let content = if lead == slot.len() {
                let at = if lead > 0 { slot.start + 1 } else { slot.start };
                at..at
            } else {
                let trail = inner.iter().rev().take_while(|b| is_ws(**b)).count();
                slot.start + lead..slot.end - trail
            };
            let escapes = escaped_pipes(text, content.clone());
            Cell {
                slot,
                content,
                kind: CellKind::InGrid,
                breaks: Vec::new(),
                escapes,
            }
        })
        .collect()
}

/// The `\|` pairs of `range`.
fn escaped_pipes(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut escaped = false;
    for i in range {
        if bytes[i] == b'|' && escaped {
            out.push(i - 1..i + 1);
        }
        escaped = !escaped && bytes[i] == b'\\';
    }
    out
}

/// The unescaped `|` of a line: what a cell edit must keep.
pub fn pipe_count(line: &str) -> usize {
    pipes_in(line, 0..line.len()).len()
}

/// `<br>`, `<br/>` or `<br />`, any case.
pub fn is_break_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "<br>" | "<br/>" | "<br />"
    )
}

impl Grid {
    /// The grid of a `Table` node; `None` when its second line is not a
    /// delimiter row.
    pub fn from_node(text: &Text, node: Node<'_>) -> Option<Grid> {
        if node.kind() != NodeKind::Table {
            return None;
        }
        let doc = text.as_str();
        let mut rows = Vec::new();
        for child in node.children() {
            let kind = match child.kind() {
                NodeKind::TableHeader => RowKind::Header,
                NodeKind::TableDelimiter => RowKind::Delimiter,
                NodeKind::TableRow => RowKind::Body,
                _ => continue,
            };
            let line = text.line_at(child.from());
            let pipes: Vec<usize> = match kind {
                RowKind::Delimiter => pipes_in(doc, child.range()),
                _ => child
                    .children()
                    .filter(|c| c.kind() == NodeKind::TableDelimiter)
                    .map(|c| c.from())
                    .collect(),
            };
            let mut cells = row_cells(doc, child.range(), &pipes);
            if kind != RowKind::Delimiter {
                for tag in child
                    .descendants()
                    .filter(|n| n.kind() == NodeKind::HtmlTag)
                {
                    if !is_break_tag(text.slice(tag.range())) {
                        continue;
                    }
                    if let Some(cell) = cells
                        .iter_mut()
                        .find(|c| c.content.start <= tag.from() && tag.to() <= c.content.end)
                    {
                        cell.breaks.push(tag.range());
                    }
                }
            }
            rows.push(GridRow {
                kind,
                line: line.from..line.to,
                prefix: line.from..child.from(),
                cells,
            });
        }
        let delimiter_ok = rows.len() >= 2
            && rows[0].kind == RowKind::Header
            && rows[1].kind == RowKind::Delimiter
            && !rows[1].cells.is_empty()
            && rows[1]
                .cells
                .iter()
                .all(|c| is_delimiter(text.slice(c.content.clone())));
        if !delimiter_ok {
            return None;
        }
        let columns = rows[0].cells.len();
        let alignments = (0..columns)
            .map(|i| {
                rows[1]
                    .cells
                    .get(i)
                    .and_then(|c| alignment(text.slice(c.content.clone())))
            })
            .collect();
        for row in rows.iter_mut().skip(2) {
            for cell in row.cells.iter_mut().skip(columns) {
                cell.kind = CellKind::Extra;
            }
            let end = row.line.end;
            while row.cells.len() < columns {
                row.cells.push(Cell {
                    slot: end..end,
                    content: end..end,
                    kind: CellKind::Missing,
                    breaks: Vec::new(),
                    escapes: Vec::new(),
                });
            }
        }
        Some(Grid {
            range: node.range(),
            alignments,
            rows,
        })
    }

    pub fn columns(&self) -> usize {
        self.alignments.len()
    }

    /// A body row has cells past the header's width. GFM does not draw
    /// them, so the table is not edited in place: its source shows on
    /// contact, as in Writer, and those cells stay in sight while editing.
    pub fn has_extra(&self) -> bool {
        self.rows
            .iter()
            .any(|r| r.cells.iter().any(|c| c.kind == CellKind::Extra))
    }

    /// The header and the body rows, with their index in `rows`.
    pub fn text_rows(&self) -> impl Iterator<Item = (usize, &GridRow)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.kind != RowKind::Delimiter)
    }

    pub fn cell(&self, row: usize, column: usize) -> Option<&Cell> {
        self.rows.get(row)?.cells.get(column)
    }

    /// The row (its index in `rows`) and the column of the slot holding
    /// `pos`, bounds included; `None` on the delimiter row, a pipe outside
    /// any slot or a prefix.
    pub fn cell_at(&self, pos: usize) -> Option<(usize, usize)> {
        self.text_rows().find_map(|(r, row)| {
            if pos < row.line.start || pos > row.line.end {
                return None;
            }
            row.cells
                .iter()
                .position(|c| {
                    c.kind != CellKind::Missing && c.slot.start <= pos && pos <= c.slot.end
                })
                .map(|c| (r, c))
        })
    }

    /// The drawn cell holding the caret position `pos` ([`Cell::holds`]).
    pub fn content_at(&self, pos: usize) -> Option<(usize, usize)> {
        self.text_rows().find_map(|(r, row)| {
            row.cells
                .iter()
                .position(|c| c.kind == CellKind::InGrid && c.holds(pos))
                .map(|c| (r, c))
        })
    }

    /// The drawn cell `pos` is in for an edit ([`Cell::holds_edit`]).
    pub fn cell_holding(&self, pos: usize) -> Option<(usize, usize)> {
        self.text_rows().find_map(|(r, row)| {
            row.cells
                .iter()
                .position(|c| c.kind == CellKind::InGrid && c.holds_edit(pos))
                .map(|c| (r, c))
        })
    }

    /// From the first line's start to the last row's end: what the drawn
    /// table covers.
    pub fn zone(&self) -> Range<usize> {
        let start = self.rows.first().map_or(self.range.start, |r| r.line.start);
        let end = self.rows.last().map_or(self.range.end, |r| r.line.end);
        start..end
    }

    /// Every drawn cell in document order: `(row, column, cell)`.
    pub fn drawn_cells(&self) -> impl Iterator<Item = (usize, usize, &Cell)> {
        self.text_rows().flat_map(|(r, row)| {
            row.cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.kind == CellKind::InGrid)
                .map(move |(c, cell)| (r, c, cell))
        })
    }
}

fn alignment(cell: &str) -> Option<Align> {
    match (cell.starts_with(':'), cell.ends_with(':')) {
        (true, true) => Some(Align::Center),
        (false, true) => Some(Align::Right),
        (true, false) => Some(Align::Left),
        (false, false) => None,
    }
}

fn is_delimiter(cell: &str) -> bool {
    let inner = cell.strip_prefix(':').unwrap_or(cell);
    let inner = inner.strip_suffix(':').unwrap_or(inner);
    !inner.is_empty() && inner.bytes().all(|b| b == b'-')
}

/// Column widths for an `auto` table layout: each column gets at least its
/// longest word (`min`) and asks for its content capped at `cap`
/// (`max`, never below `min`). When the asks fit in `available` they are
/// granted; else the room above the minimums is shared in proportion to
/// what each column asks beyond its minimum. Widths include padding.
pub fn size_columns(min: &[f32], max: &[f32], available: f32, cap: f32) -> Vec<f32> {
    let asks: Vec<f32> = min
        .iter()
        .zip(max)
        .map(|(lo, hi)| hi.min(cap).max(*lo))
        .collect();
    let total_ask: f32 = asks.iter().sum();
    if total_ask <= available {
        return asks;
    }
    let total_min: f32 = min.iter().sum();
    if total_min >= available {
        return min.to_vec();
    }
    let spare = available - total_min;
    let extra: f32 = asks.iter().zip(min).map(|(a, m)| a - m).sum();
    asks.iter()
        .zip(min)
        .map(|(a, m)| {
            if extra > 0.0 {
                m + spare * (a - m) / extra
            } else {
                *m
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::MarkdownTree;

    fn grid(doc: &str) -> Grid {
        try_grid(doc).unwrap()
    }

    fn try_grid(doc: &str) -> Option<Grid> {
        let text = Text::new(doc);
        let tree = MarkdownTree::parse(doc);
        let node = tree.iter().find(|n| n.kind() == NodeKind::Table)?;
        Grid::from_node(&text, node)
    }

    /// Source of each drawn cell's content, row by row (delimiter row out).
    fn contents<'a>(doc: &'a str, g: &Grid) -> Vec<Vec<&'a str>> {
        g.text_rows()
            .map(|(_, row)| {
                row.cells
                    .iter()
                    .filter(|c| c.kind == CellKind::InGrid)
                    .map(|c| &doc[c.content.clone()])
                    .collect()
            })
            .collect()
    }

    #[test]
    fn cells_carry_their_slot_and_trimmed_content() {
        let doc = "| a | bb |\n| - | -- |\n|  x|y  |";
        let g = grid(doc);
        assert_eq!(g.range, 0..doc.len());
        assert_eq!(g.rows.len(), 3);
        assert_eq!(g.rows[1].kind, RowKind::Delimiter);
        assert_eq!(contents(doc, &g), vec![vec!["a", "bb"], vec!["x", "y"]]);
        let header = &g.rows[0].cells;
        assert_eq!(&doc[header[0].slot.clone()], " a ");
        assert_eq!(&doc[header[1].slot.clone()], " bb ");
        assert_eq!(header[1].content, 6..8);
        let body = &g.rows[2];
        assert_eq!(&doc[body.cells[0].slot.clone()], "  x");
        assert_eq!(&doc[body.line.clone()], "|  x|y  |");
        assert!(body.prefix.is_empty());
    }

    #[test]
    fn an_empty_cell_has_an_insertion_point() {
        let doc = "| a |  | c |\n| - | - | - |\n||x|  |";
        let g = grid(doc);
        let header = &g.rows[0].cells;
        // After the first space following its `|`.
        assert_eq!(header[1].content, 6..6);
        assert_eq!(&doc[header[1].slot.clone()], "  ");
        let body = &g.rows[2].cells;
        let row_start = g.rows[2].line.start;
        // `||`: right after the opening pipe.
        assert_eq!(body[0].content, row_start + 1..row_start + 1);
        assert_eq!(&doc[body[1].content.clone()], "x");
        assert_eq!(body[2].content.start, row_start + 5);
        assert!(body.iter().all(|c| c.kind == CellKind::InGrid));
    }

    #[test]
    fn short_rows_get_missing_cells_and_long_rows_extra_ones() {
        let doc = "| a | b | c |\n| - | - | - |\n| 1 |\n| 1 | 2 | 3 | 4 | 5 |";
        let g = grid(doc);
        let short = &g.rows[2];
        assert_eq!(short.cells.len(), 3);
        assert_eq!(short.cells[0].kind, CellKind::InGrid);
        for cell in &short.cells[1..] {
            assert_eq!(cell.kind, CellKind::Missing);
            assert_eq!(cell.content, short.line.end..short.line.end);
        }
        let long = &g.rows[3];
        assert_eq!(long.cells.len(), 5);
        assert!(long.cells[..3].iter().all(|c| c.kind == CellKind::InGrid));
        assert!(long.cells[3..].iter().all(|c| c.kind == CellKind::Extra));
        assert_eq!(&doc[long.cells[4].content.clone()], "5");
        assert_eq!(g.drawn_cells().count(), 3 + 1 + 3);
    }

    #[test]
    fn alignments_escapes_and_code_spans() {
        let doc = "| L | C | R | N |\n|:--|:-:|--:|---|\n| a \\| b | `x|y` | z | w |";
        let g = grid(doc);
        assert_eq!(
            g.alignments,
            vec![
                Some(Align::Left),
                Some(Align::Center),
                Some(Align::Right),
                None
            ]
        );
        let body = &g.rows[2].cells;
        // `\|` stays in its cell.
        assert_eq!(&doc[body[0].content.clone()], "a \\| b");
        // An unescaped `|` in a code span cuts it (GFM).
        assert_eq!(&doc[body[1].content.clone()], "`x");
        assert_eq!(&doc[body[2].content.clone()], "y`");
        assert_eq!(body[3].kind, CellKind::InGrid);
        assert_eq!(body[4].kind, CellKind::Extra);
        assert!(try_grid("| a |\n| b |\n").is_none());
    }

    #[test]
    fn cell_at_finds_slots_and_skips_the_delimiter_row() {
        let doc = "| a | b |\n| - | - |\n| 1 | 2 |";
        let g = grid(doc);
        assert_eq!(g.cell_at(0), None, "on a pipe");
        assert_eq!(g.cell_at(1), Some((0, 0)));
        assert_eq!(g.cell_at(2), Some((0, 0)));
        assert_eq!(g.cell_at(4), Some((0, 0)));
        assert_eq!(g.cell_at(5), Some((0, 1)));
        let delimiter = g.rows[1].line.clone();
        for pos in delimiter.start..=delimiter.end {
            assert_eq!(g.cell_at(pos), None, "{pos}");
        }
        let two = doc.rfind('2').unwrap();
        assert_eq!(g.cell_at(two), Some((2, 1)));
        assert_eq!(g.cell_at(doc.len()), None);
        assert_eq!(g.content_at(two), Some((2, 1)));
        assert_eq!(g.content_at(two - 1), None, "padding");
    }

    #[test]
    fn breaks_split_a_cell_into_segments() {
        let doc = "| a<br>b<BR/>c `<br>` |\n| - |\n| x |";
        let g = grid(doc);
        let cell = &g.rows[0].cells[0];
        assert_eq!(cell.breaks.len(), 2);
        let segments: Vec<&str> = cell.segments().iter().map(|r| &doc[r.clone()]).collect();
        assert_eq!(segments, vec!["a", "b", "c `<br>`"]);
        let br = cell.breaks[0].clone();
        assert!(cell.holds(br.start) && cell.holds(br.end));
        assert!(!cell.holds(br.start + 1));
        assert!(is_break_tag("<Br />"));
        let doc = "| a\\|b |\n| - |";
        let cell = &grid(doc).rows[0].cells[0];
        assert_eq!(cell.escapes, vec![3..5]);
        assert!(!cell.holds(4) && cell.holds(3) && cell.holds(5));
        assert_eq!(cell.stretches(), vec![2..3, 5..6]);
        assert!(!is_break_tag("<br x>"));
    }

    #[test]
    fn a_table_at_the_end_without_a_final_break() {
        let doc = "x\n\n| a | b |\n| - | - |\n| 1 | 2 |";
        let g = grid(doc);
        let last = g.rows.last().unwrap();
        assert_eq!(last.line.end, doc.len());
        assert_eq!(g.zone(), doc.find('|').unwrap()..doc.len());
        assert_eq!(&doc[last.cells[1].content.clone()], "2");
        assert_eq!(last.cells[1].slot.end, doc.len() - 1);
    }

    #[test]
    fn a_table_in_a_quote_keeps_its_prefixes() {
        let doc = "> | a | b |\n> | - | - |\n> | 1 | 2 |\n\nx";
        let g = grid(doc);
        assert_eq!(contents(doc, &g), vec![vec!["a", "b"], vec!["1", "2"]]);
        for row in &g.rows {
            assert_eq!(&doc[row.prefix.clone()], "> ");
            assert_eq!(doc[row.line.clone()].len(), 11);
        }
        assert_eq!(g.zone().start, 0);
        let two = doc.rfind('2').unwrap();
        assert_eq!(g.rows[2].cells[1].content, two..two + 1);
    }

    #[test]
    fn a_table_in_a_list_item_keeps_its_indentation() {
        let doc = "- item\n\n  | a | b |\n  | - | - |\n  | 1 | 2 |\n";
        let g = grid(doc);
        assert_eq!(contents(doc, &g), vec![vec!["a", "b"], vec!["1", "2"]]);
        for row in &g.rows {
            assert_eq!(&doc[row.prefix.clone()], "  ");
        }
        let one = doc.rfind('1').unwrap();
        assert_eq!(g.content_at(one), Some((2, 0)));
        assert_eq!(g.rows[2].line.end, doc.len() - 1);
    }

    #[test]
    fn columns_size_between_longest_word_and_capped_content() {
        // Everything fits: each column gets its content.
        assert_eq!(
            size_columns(&[10.0, 20.0], &[30.0, 40.0], 100.0, 500.0),
            vec![30.0, 40.0]
        );
        // A long prose column asks for at most the cap.
        assert_eq!(
            size_columns(&[10.0, 20.0], &[30.0, 900.0], 1000.0, 300.0),
            vec![30.0, 300.0]
        );
        // Too narrow: the spare room is shared, nobody below its word.
        let w = size_columns(&[10.0, 20.0], &[110.0, 220.0], 130.0, 500.0);
        assert!(w[0] >= 10.0 && w[1] >= 20.0);
        assert!((w.iter().sum::<f32>() - 130.0).abs() < 1e-3);
        // Narrower than the words: words win, no word is cut.
        assert_eq!(
            size_columns(&[50.0, 60.0], &[80.0, 90.0], 40.0, 500.0),
            vec![50.0, 60.0]
        );
    }
}
