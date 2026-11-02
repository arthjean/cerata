//! A drawn GFM table laid out by the editor in the frame of the edit
//! (ADR 0007, decision 7): each cell's text is shaped like a short line,
//! wrapped at its column's width, from the decorations of the document.
//! Caret, selection, hit-testing, Up and Down and the IME window go through
//! this geometry. Sizing follows US-032 (`table-decorations.ts:485-534`):
//! a column is never narrower than its longest word and asks for its
//! content, capped at 48ch.

use std::ops::{Deref, Range};
use std::rc::Rc;
use std::sync::Arc;

use cerata_markdown::table::{self, Align, CellKind, Grid};
use cerata_markdown::{Decoration, EditorState, HideMode, Widget};
use gpui::{
    Bounds, Corners, Edges, Pixels, SharedString, TextAlign, Window, fill, point, px, quad, size,
};

use super::config::LineMetrics;
use super::display::DisplayLine;
use super::layout::{FrameInputs, LaidLine, LayoutCx};
use super::theme;

/// Corner radius of the table's border.
const RADIUS: f32 = 8.0;

/// One drawn cell: its box and the lines of its text, one per stretch
/// between `<br>` tags.
pub struct CellBox {
    /// Index of the row among the drawn rows (0 is the header).
    pub row: usize,
    pub column: usize,
    /// Buffer range of the content (an insertion point when empty).
    pub content: Range<usize>,
    /// A missing cell of a short row: drawn empty, holds no caret.
    pub missing: bool,
    /// Each line's `text_left` is column-relative and its `text_top` is
    /// relative to the block's top, like the table's line.
    pub segments: Vec<LaidLine>,
}

/// The geometry of a drawn table, relative to its line's top and the text
/// column's left edge.
pub struct TableLayout {
    pub grid: Arc<Grid>,
    /// Width of the grid, borders included.
    pub width: f32,
    /// Height of the whole block, its vertical padding included.
    pub height: f32,
    /// Top of the grid's outer border.
    pub grid_top: f32,
    /// Left edge and width of each column (its right border included).
    pub columns: Vec<(f32, f32)>,
    /// Top and height of each drawn row (its bottom border included).
    pub rows: Vec<(f32, f32)>,
    pub cells: Vec<CellBox>,
}

impl std::fmt::Debug for TableLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableLayout")
            .field("range", &self.grid.range)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("cells", &self.cells.len())
            .finish()
    }
}

impl PartialEq for TableLayout {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A line of text the caret can be on: a document line, or a line of a
/// table cell (which lives in its table's layout).
pub enum TextLine {
    Line(Box<LaidLine>),
    Cell {
        table: Rc<TableLayout>,
        cell: usize,
        segment: usize,
    },
}

impl Deref for TextLine {
    type Target = LaidLine;

    fn deref(&self) -> &LaidLine {
        match self {
            TextLine::Line(line) => line,
            TextLine::Cell {
                table,
                cell,
                segment,
            } => &table.cells[*cell].segments[*segment],
        }
    }
}

impl TableLayout {
    /// Box of a cell, relative to the line's top and the column's left.
    #[cfg(test)]
    pub fn cell_bounds(&self, cell: &CellBox) -> Bounds<Pixels> {
        let (x, w) = self.columns[cell.column];
        let (y, h) = self.rows[cell.row];
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    /// The cell line holding `pos` (a segment's bounds included), as
    /// `(cell, segment)`.
    pub fn segment_for(&self, pos: usize) -> Option<(usize, usize)> {
        self.cells.iter().enumerate().find_map(|(c, cell)| {
            if cell.missing || pos < cell.content.start || pos > cell.content.end {
                return None;
            }
            cell.segments
                .iter()
                .position(|s| s.start <= pos && pos <= s.end())
                .map(|s| (c, s))
        })
    }

    /// The cell line under `(x, y)` (column-relative, line-relative), or the
    /// nearest one when `nearest` (a click on a border or a padding).
    pub fn segment_at(&self, x: f32, y: f32, nearest: bool) -> Option<(usize, usize)> {
        let column = self
            .columns
            .iter()
            .position(|(left, w)| x >= *left && x < left + w);
        let row = self
            .rows
            .iter()
            .position(|(top, h)| y >= *top && y < top + h);
        let (column, row) = match (column, row) {
            (Some(c), Some(r)) => (c, r),
            _ if !nearest => return None,
            (c, r) => (
                c.unwrap_or_else(|| {
                    if x < self.columns.first().map_or(0.0, |c| c.0) {
                        0
                    } else {
                        self.columns.len().saturating_sub(1)
                    }
                }),
                r.unwrap_or_else(|| {
                    if y < self.rows.first().map_or(0.0, |r| r.0) {
                        0
                    } else {
                        self.rows.len().saturating_sub(1)
                    }
                }),
            ),
        };
        let mut index = self
            .cells
            .iter()
            .position(|c| c.row == row && c.column == column)?;
        if self.cells[index].missing {
            if !nearest {
                return None;
            }
            // A short row's missing cell: its last real cell.
            index = self.cells[..index]
                .iter()
                .rposition(|c| c.row == row && !c.missing)?;
        }
        let cell = &self.cells[index];
        let segment = cell
            .segments
            .iter()
            .position(|s| {
                let bottom = s.text_top + s.rows().max(1) as f32 * s.metrics.row_height;
                y < bottom
            })
            .unwrap_or(cell.segments.len().saturating_sub(1));
        Some((index, segment))
    }

    /// One visual row down or up from row `row` of a cell line: the next
    /// line of the cell, else the cell under `x` in the next drawn row, as
    /// `(cell, segment, row)`; `None` at the table's edge.
    pub fn step(
        &self,
        cell: usize,
        segment: usize,
        down: bool,
        x: f32,
    ) -> Option<(usize, usize, usize)> {
        let lines = &self.cells.get(cell)?.segments;
        if down && segment + 1 < lines.len() {
            return Some((cell, segment + 1, 0));
        }
        if !down && segment > 0 {
            let rows = lines[segment - 1].rows().max(1);
            return Some((cell, segment - 1, rows - 1));
        }
        let row = self.cells[cell].row;
        let target = if down { row + 1 } else { row.checked_sub(1)? };
        self.enter(target, down, x)
    }

    /// The cell line under `x` in drawn row `row`: its first visual row
    /// when entering from above (`down`), its last from below.
    pub fn enter(&self, row: usize, down: bool, x: f32) -> Option<(usize, usize, usize)> {
        let (top, height) = *self.rows.get(row)?;
        let y = if down { top + 1.0 } else { top + height - 1.0 };
        let (cell, segment) = self.segment_at(x, y, true)?;
        let line = &self.cells[cell].segments[segment];
        let last = line.rows().max(1) - 1;
        Some((cell, segment, if down { 0 } else { last }))
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

/// Display replacements of a cell line, then its invisible ranges.
type Replacements = (Vec<(Range<usize>, String)>, Vec<Range<usize>>);

/// Display replacements and invisible ranges of a cell line, from the
/// decorations: hidden marks, dashes, emoji, wiki links and tabs.
fn replacements(decorations: &[&Decoration], range: &Range<usize>) -> Replacements {
    let local = |r: &Range<usize>| {
        r.start.clamp(range.start, range.end) - range.start
            ..r.end.clamp(range.start, range.end) - range.start
    };
    let inside = |r: &Range<usize>| r.start < range.end && r.end > range.start;
    let mut out = Vec::new();
    let mut invisible = Vec::new();
    for decoration in decorations {
        match decoration {
            Decoration::Hide { range: r, mode } if inside(r) => match mode {
                HideMode::Collapse | HideMode::Remove => out.push((local(r), String::new())),
                HideMode::KeepSpace => invisible.push(local(r)),
                HideMode::Block => {}
            },
            Decoration::Replace {
                range: r,
                widget,
                block: false,
            } if inside(r) => {
                let glyph = match widget {
                    Widget::Dash(_) | Widget::Emoji(_) | Widget::WikiLink { .. } => {
                        widget.glyph().map(str::to_owned)
                    }
                    Widget::Tab => Some("\u{2007}".repeat(cerata_markdown::decorate::TAB_WIDTH_CH)),
                    _ => None,
                };
                if let Some(glyph) = glyph {
                    out.push((local(r), glyph));
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let mut disjoint: Vec<(Range<usize>, String)> = Vec::with_capacity(out.len());
    for (r, glyph) in out {
        match disjoint.last_mut() {
            Some((last, _)) if r.start < last.end => last.end = last.end.max(r.end),
            _ => disjoint.push((r, glyph)),
        }
    }
    (disjoint, invisible)
}

/// A shaped cell line before its column is known.
struct Measured {
    range: Range<usize>,
    display: DisplayLine,
    runs: Vec<gpui::TextRun>,
    /// Longest word and whole text, in pixels.
    word: f32,
    content: f32,
}

/// What a cell's lines are styled from: the decorations and spans that
/// start in the cell. Both are sorted by start, so a cell finds its own by
/// binary search instead of scanning the whole table's (US-048).
struct CellInputs<'a> {
    decorations: Vec<&'a Decoration>,
    spans: &'a [(Range<usize>, super::layout::Span)],
}

impl<'a> CellInputs<'a> {
    fn new(inputs: &'a FrameInputs) -> Self {
        Self {
            decorations: inputs
                .decorations
                .iter()
                .filter(|d| {
                    matches!(
                        d,
                        Decoration::Hide { .. } | Decoration::Replace { block: false, .. }
                    )
                })
                .collect(),
            spans: &inputs.spans,
        }
    }

    /// The decorations and spans starting in `cell` that may touch `line`.
    fn of(
        &self,
        cell: &Range<usize>,
        line: &Range<usize>,
    ) -> (&[&'a Decoration], &'a [(Range<usize>, super::layout::Span)]) {
        let d0 = self
            .decorations
            .partition_point(|d| d.range().start < cell.start);
        let d1 = self
            .decorations
            .partition_point(|d| d.range().start <= line.end);
        let s0 = self.spans.partition_point(|(r, _)| r.start < cell.start);
        let s1 = self.spans.partition_point(|(r, _)| r.start <= line.end);
        (
            &self.decorations[d0..d1.max(d0)],
            &self.spans[s0..s1.max(s0)],
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn measure(
    state: &EditorState,
    cell: &Range<usize>,
    range: Range<usize>,
    header: bool,
    inputs: &CellInputs<'_>,
    cx: &LayoutCx<'_>,
    window: &Window,
) -> Measured {
    let source = state.text().slice(range.clone());
    let (decorations, spans) = inputs.of(cell, &range);
    let (replaced, invisible) = replacements(decorations, &range);
    let display = DisplayLine::new(source, &replaced);
    let marked = cx.marked.as_ref().and_then(|m| {
        (m.start >= range.start && m.end <= range.end)
            .then(|| m.start - range.start..m.end - range.start)
    });
    let runs = super::layout::runs(
        &display,
        range.start,
        spans,
        &[],
        &invisible,
        &[],
        marked,
        None,
        header,
        false,
        false,
        cx,
    );
    let em = cx.config.text_size();
    let (word, content) = if display.text.is_empty() {
        (0.0, 0.0)
    } else {
        let shaped = window.text_system().shape_line(
            SharedString::from(display.text.clone()),
            px(em),
            &runs,
            None,
        );
        let mut word: f32 = 0.0;
        let mut start = None;
        for (i, c) in display
            .text
            .char_indices()
            .chain(std::iter::once((display.text.len(), ' ')))
        {
            match (c.is_whitespace(), start) {
                (false, None) => start = Some(i),
                (true, Some(s)) => {
                    word = word.max((shaped.x_for_index(i) - shaped.x_for_index(s)).as_f32());
                    start = None;
                }
                _ => {}
            }
        }
        (word.ceil(), shaped.width().as_f32().ceil())
    };
    Measured {
        range,
        display,
        runs,
        word,
        content,
    }
}

/// Lays out `grid` in a text column `cx.column_width` wide, with the
/// decorations of `inputs` (which cover the whole table).
pub fn layout(
    state: &EditorState,
    line_ix: usize,
    grid: Arc<Grid>,
    inputs: &FrameInputs,
    cx: &LayoutCx<'_>,
    window: &Window,
) -> TableLayout {
    let em = cx.config.text_size();
    let pad_x = table::CELL_HORIZONTAL_PADDING_EM * em;
    let pad_y = table::CELL_VERTICAL_PADDING_EM * em / 2.0;
    let border = table::BORDER_WIDTH;
    let row_height = em * table::CELL_LINE_HEIGHT;
    let padding = 2.0 * pad_x + border;
    let columns = grid.columns().max(1);
    let cell_inputs = CellInputs::new(inputs);

    // Measure every cell line once, unwrapped.
    struct Pending {
        row: usize,
        column: usize,
        content: Range<usize>,
        missing: bool,
        lines: Vec<Measured>,
    }
    let ends: Vec<usize> = state
        .selection()
        .ranges()
        .iter()
        .flat_map(|r| [r.anchor, r.head])
        .collect();
    let mut pending = Vec::new();
    let mut min = vec![padding; columns];
    let mut max = vec![padding; columns];
    for (row, (_, grid_line)) in grid.text_rows().enumerate() {
        let header = row == 0;
        for (column, cell) in grid_line.cells.iter().enumerate().take(columns) {
            let missing = cell.kind == CellKind::Missing;
            // A selection end in the spaces after the content (a typed
            // space): the cell shows them up to it.
            let tail = ends
                .iter()
                .copied()
                .filter(|e| *e > cell.content.end && cell.holds_edit(*e))
                .max();
            let content = cell.content.start..tail.unwrap_or(cell.content.end);
            let lines: Vec<Measured> = if missing {
                Vec::new()
            } else {
                let mut segments = cell.segments();
                if let (Some(tail), Some(last)) = (tail, segments.last_mut()) {
                    last.end = tail;
                }
                segments
                    .into_iter()
                    .map(|range| {
                        measure(
                            state,
                            &cell.content,
                            range,
                            header,
                            &cell_inputs,
                            cx,
                            window,
                        )
                    })
                    .collect()
            };
            for line in &lines {
                min[column] = min[column].max(line.word + padding);
                max[column] = max[column].max(line.content + padding);
            }
            pending.push(Pending {
                row,
                column,
                content,
                missing,
                lines,
            });
        }
    }
    let ch = cx.ch.max(em * 0.5);
    let cap = table::CELL_MAX_WIDTH_CH * ch + padding;
    let widths: Vec<f32> = table::size_columns(&min, &max, cx.column_width - border, cap)
        .into_iter()
        .map(f32::ceil)
        .collect();
    let mut lefts = Vec::with_capacity(columns);
    let mut x = border;
    for w in &widths {
        lefts.push(x);
        x += w;
    }
    let width = x;

    let metrics = LineMetrics {
        font_size: em,
        row_height,
        pad_top: 0.0,
        pad_bottom: 0.0,
    };
    // Shape at the column widths; cell tops are fixed once rows are known.
    let mut cells: Vec<CellBox> = pending
        .into_iter()
        .map(|p| {
            let inner = (widths[p.column] - padding).max(ch);
            let align = grid.alignments.get(p.column).copied().flatten();
            let segments = p
                .lines
                .into_iter()
                .map(|m| {
                    let shaped = window.text_system().shape_text(
                        SharedString::from(m.display.text.clone()),
                        px(em),
                        &m.runs,
                        Some(px(inner)),
                        None,
                    );
                    let wrapped = shaped
                        .ok()
                        .and_then(|mut lines| (!lines.is_empty()).then(|| lines.swap_remove(0)));
                    let row_starts = wrapped.as_ref().map_or_else(
                        || vec![0],
                        |line| {
                            std::iter::once(0)
                                .chain(line.wrap_boundaries().iter().map(|b| {
                                    line.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].index
                                }))
                                .collect()
                        },
                    );
                    // One row: aligned in its column; wrapped rows fill it.
                    let shift = if row_starts.len() == 1 {
                        let free = (inner - m.content).max(0.0);
                        match align {
                            Some(Align::Center) => (free / 2.0).floor(),
                            Some(Align::Right) => free,
                            _ => 0.0,
                        }
                    } else {
                        0.0
                    };
                    LaidLine {
                        ix: line_ix,
                        start: m.range.start,
                        len: m.range.len(),
                        metrics,
                        display: m.display,
                        wrapped,
                        row_starts,
                        text_left: lefts[p.column] + pad_x + shift,
                        text_top: 0.0,
                        prefix: None,
                        quote_bars: Vec::new(),
                        code: None,
                        table_source: None,
                        block: None,
                        inline_math: Vec::new(),
                        height: 0.0,
                    }
                })
                .collect();
            CellBox {
                row: p.row,
                column: p.column,
                content: p.content,
                missing: p.missing,
                segments,
            }
        })
        .collect();

    let row_count = cells.iter().map(|c| c.row + 1).max().unwrap_or(0);
    let mut heights = vec![row_height; row_count];
    for cell in &cells {
        let text: f32 = cell
            .segments
            .iter()
            .map(|s| s.rows().max(1) as f32 * row_height)
            .sum();
        heights[cell.row] = heights[cell.row].max(text.max(row_height));
    }
    let widget_pad = table::WIDGET_VERTICAL_PADDING_EM * em / 2.0;
    let grid_top = widget_pad;
    let mut rows = Vec::with_capacity(row_count);
    let mut y = grid_top + border;
    for h in &heights {
        let height = (h + 2.0 * pad_y + border).ceil();
        rows.push((y, height));
        y += height;
    }
    for cell in &mut cells {
        let mut top = rows[cell.row].0 + pad_y;
        for segment in &mut cell.segments {
            segment.text_top = top;
            let rows = segment.rows().max(1) as f32;
            segment.height = rows * row_height;
            top += segment.height;
        }
    }
    TableLayout {
        grid,
        width,
        height: (y + widget_pad).ceil(),
        grid_top,
        columns: lefts.into_iter().zip(widths).collect(),
        rows,
        cells,
    }
}

/// Border, header background and rules of a table whose line top is `top`.
pub fn paint_grid(t: &TableLayout, left: Pixels, top: Pixels, window: &mut Window) {
    let border = theme::border();
    let b = px(table::BORDER_WIDTH);
    let grid_height = t.rows.last().map_or(0.0, |(y, h)| y + h) - t.grid_top;
    let outer = Bounds::new(
        point(left, top + px(t.grid_top)),
        size(px(t.width), px(grid_height)),
    );
    if let Some((y, h)) = t.rows.first() {
        let inner = px(RADIUS - table::BORDER_WIDTH);
        window.paint_quad(quad(
            Bounds::new(
                point(left + b, top + px(*y)),
                size(px(t.width) - b * 2.0, px(*h)),
            ),
            Corners {
                top_left: inner,
                top_right: inner,
                bottom_left: px(0.0),
                bottom_right: px(0.0),
            },
            theme::code_background(),
            Edges::default(),
            theme::transparent(),
            Default::default(),
        ));
    }
    let last_row = t.rows.len().saturating_sub(1);
    for (r, (y, h)) in t.rows.iter().enumerate() {
        if r < last_row {
            window.paint_quad(fill(
                Bounds::new(
                    point(left + b, top + px(y + h) - b),
                    size(px(t.width) - b * 2.0, b),
                ),
                border,
            ));
        }
    }
    let rows_top = t.rows.first().map_or(0.0, |r| r.0);
    let rows_bottom = t.rows.last().map_or(0.0, |(y, h)| y + h) - table::BORDER_WIDTH;
    let last_column = t.columns.len().saturating_sub(1);
    for (c, (x, w)) in t.columns.iter().enumerate() {
        if c < last_column {
            window.paint_quad(fill(
                Bounds::new(
                    point(left + px(x + w) - b, top + px(rows_top)),
                    size(b, px(rows_bottom - rows_top)),
                ),
                border,
            ));
        }
    }
    window.paint_quad(quad(
        outer,
        px(RADIUS),
        theme::transparent(),
        b,
        border,
        Default::default(),
    ));
}

/// The text of every cell.
pub fn paint_text(
    t: &TableLayout,
    left: Pixels,
    top: Pixels,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    for cell in &t.cells {
        for segment in &cell.segments {
            let Some(wrapped) = &segment.wrapped else {
                continue;
            };
            let origin = point(left + px(segment.text_left), top + px(segment.text_top));
            if let Err(error) = wrapped.paint(
                origin,
                px(segment.metrics.row_height),
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                log::error!("failed to paint a table cell: {error:#}");
            }
        }
    }
}
