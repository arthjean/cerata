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

const RADIUS: f32 = 8.0;

pub struct CellBox {
    pub row: usize,
    pub column: usize,
    pub content: Range<usize>,
    pub missing: bool,
    pub segments: Vec<LaidLine>,
    source: Rc<str>,
    measures: Rc<[(f32, f32)]>,
    runs: Rc<[Rc<[gpui::TextRun]>]>,
    inner: f32,
}

pub struct Reuse<'a> {
    previous: &'a TableLayout,
    cells: Vec<Option<usize>>,
}

pub fn edit_ranges(
    selection: &cerata_markdown::Selection,
    drag: Option<&cerata_markdown::Selection>,
    marked: Option<&Range<usize>>,
) -> Vec<Range<usize>> {
    selection
        .ranges()
        .iter()
        .chain(drag.map(|d| d.ranges()).unwrap_or_default())
        .map(|r| r.anchor.min(r.head)..r.anchor.max(r.head))
        .chain(marked.cloned())
        .collect()
}

pub fn reusable<'a>(
    state: &EditorState,
    grid: &Grid,
    previous: &'a TableLayout,
    old: &[Range<usize>],
    new: &[Range<usize>],
) -> (Reuse<'a>, Option<Range<usize>>) {
    let touches = |ranges: &[Range<usize>], slot: Range<usize>| {
        ranges
            .iter()
            .any(|r| r.start <= slot.end && slot.start <= r.end)
    };
    let columns = grid.columns().max(1);
    let mut cells = Vec::new();
    let mut fresh: Option<Range<usize>> = None;
    for (row, (_, grid_line)) in grid.text_rows().enumerate() {
        for (column, cell) in grid_line.cells.iter().enumerate().take(columns) {
            let found = previous
                .cells
                .binary_search_by_key(&(row, column), |c| (c.row, c.column))
                .ok()
                .filter(|&i| {
                    let old_cell = &previous.cells[i];
                    let source = state.text().slice(cell.slot.clone());
                    let old_start =
                        (old_cell.content.start + cell.slot.start).checked_sub(cell.content.start);
                    old_cell.missing == (cell.kind == CellKind::Missing)
                        && &*old_cell.source == source
                        && old_start.is_some_and(|s| !touches(old, s..s + source.len()))
                        && !touches(new, cell.slot.clone())
                });
            if found.is_none() {
                fresh = Some(match fresh {
                    Some(r) => r.start.min(cell.slot.start)..r.end.max(cell.slot.end),
                    None => cell.slot.clone(),
                });
            }
            cells.push(found);
        }
    }
    (Reuse { previous, cells }, fresh)
}

pub struct TableLayout {
    pub grid: Arc<Grid>,
    pub width: f32,
    pub scroll: f32,
    pub height: f32,
    pub grid_top: f32,
    pub columns: Vec<(f32, f32)>,
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
    #[cfg(test)]
    pub fn cell_bounds(&self, cell: &CellBox) -> Bounds<Pixels> {
        let (x, w) = self.columns[cell.column];
        let (y, h) = self.rows[cell.row];
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

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

    pub fn grid_bottom(&self) -> f32 {
        self.rows.last().map_or(self.grid_top, |(y, h)| y + h)
    }
}

type Replacements = (Vec<(Range<usize>, String)>, Vec<Range<usize>>);

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

struct Measured {
    range: Range<usize>,
    display: DisplayLine,
    runs: Rc<[gpui::TextRun]>,
    word: f32,
    content: f32,
}

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
        runs: runs.into(),
        word,
        content,
    }
}

fn whole_pixels(exact: &[f32], min: &[f32], available: f32) -> Vec<f32> {
    let mut widths: Vec<f32> = exact.iter().map(|w| w.ceil()).collect();
    if exact.iter().sum::<f32>() > available + 0.5 {
        return widths;
    }
    let mut excess = widths.iter().sum::<f32>() - available.floor();
    while excess >= 1.0 {
        let slack = |i: usize| widths[i] - min[i].ceil();
        let Some(i) = (0..widths.len())
            .filter(|&i| slack(i) >= 1.0)
            .max_by(|&a, &b| slack(a).total_cmp(&slack(b)))
        else {
            break;
        };
        widths[i] -= 1.0;
        excess -= 1.0;
    }
    widths
}

#[allow(clippy::too_many_arguments)]
fn cell_line(
    ix: usize,
    range: Range<usize>,
    metrics: LineMetrics,
    display: DisplayLine,
    wrapped: Option<Rc<gpui::WrappedLine>>,
    row_starts: Vec<usize>,
    text_left: f32,
) -> LaidLine {
    LaidLine {
        ix,
        start: range.start,
        len: range.len(),
        metrics,
        display,
        wrapped,
        row_starts,
        text_left,
        text_top: 0.0,
        prefix: None,
        quote_bars: Vec::new(),
        code: None,
        table_source: None,
        block: None,
        inline_math: Vec::new(),
        height: 0.0,
    }
}

fn align_shift(align: Option<Align>, inner: f32, content: f32, rows: usize) -> f32 {
    if rows != 1 {
        return 0.0;
    }
    let free = (inner - content).max(0.0);
    match align {
        Some(Align::Center) => (free / 2.0).floor(),
        Some(Align::Right) => free,
        _ => 0.0,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrozenWidths {
    pub start: usize,
    pub column_width: f32,
    pub widths: Vec<f32>,
}

impl FrozenWidths {
    pub fn fits(&self, grid: &Grid, column_width: f32) -> bool {
        self.start == grid.range.start
            && self.widths.len() == grid.columns().max(1)
            && self.column_width == column_width
    }
}

#[allow(clippy::too_many_arguments)]
pub fn layout(
    state: &EditorState,
    line_ix: usize,
    grid: Arc<Grid>,
    inputs: &FrameInputs,
    frozen: Option<&[f32]>,
    scroll: f32,
    reuse: Option<&Reuse<'_>>,
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

    struct Pending<'a> {
        row: usize,
        column: usize,
        content: Range<usize>,
        missing: bool,
        source: Rc<str>,
        lines: Vec<Measured>,
        old: Option<&'a CellBox>,
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
            let tail = ends
                .iter()
                .copied()
                .filter(|e| *e > cell.content.end && cell.holds_edit(*e))
                .max();
            let content = cell.content.start..tail.unwrap_or(cell.content.end);
            let old = reuse.and_then(|r| {
                let i = (*r.cells.get(pending.len())?)?;
                Some(&r.previous.cells[i])
            });
            let lines: Vec<Measured> = if missing || old.is_some() {
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
            let measured = match old {
                Some(old) => &old.measures[..],
                None => &lines
                    .iter()
                    .map(|m| (m.word, m.content))
                    .collect::<Vec<_>>()[..],
            };
            for &(word, width) in measured {
                min[column] = min[column].max(word + padding);
                max[column] = max[column].max(width + padding);
            }
            pending.push(Pending {
                row,
                column,
                content,
                missing,
                source: match old {
                    Some(old) => old.source.clone(),
                    None => state.text().slice(cell.slot.clone()).into(),
                },
                lines,
                old,
            });
        }
    }
    let ch = cx.ch.max(em * 0.5);
    let cap = table::CELL_MAX_WIDTH_CH * ch + padding;
    let widths: Vec<f32> = match frozen {
        Some(widths) if widths.len() == columns => widths.to_vec(),
        _ => {
            let available = cx.column_width - border;
            let exact = table::size_columns(&min, &max, available, cap);
            whole_pixels(&exact, &min, available)
        }
    };
    let mut lefts = Vec::with_capacity(columns);
    let mut x = border - scroll;
    for w in &widths {
        lefts.push(x);
        x += w;
    }
    let width = x + scroll;

    let metrics = LineMetrics {
        font_size: em,
        row_height,
        pad_top: 0.0,
        pad_bottom: 0.0,
    };
    let mut cells: Vec<CellBox> = pending
        .into_iter()
        .map(|p| {
            let inner = (widths[p.column] - padding).max(ch);
            let align = grid.alignments.get(p.column).copied().flatten();
            let left = lefts[p.column] + pad_x;
            let mut lines = p.lines;
            if let Some(old) = p.old {
                let moved = |s: &LaidLine| {
                    let start = s.start - old.content.start + p.content.start;
                    start..start + s.len
                };
                if old.inner == inner {
                    let segments = old
                        .segments
                        .iter()
                        .zip(old.measures.iter())
                        .map(|(s, &(_, content))| {
                            let shift = align_shift(align, inner, content, s.row_starts.len());
                            cell_line(
                                line_ix,
                                moved(s),
                                metrics,
                                s.display.clone(),
                                s.wrapped.clone(),
                                s.row_starts.clone(),
                                left + shift,
                            )
                        })
                        .collect();
                    return CellBox {
                        row: p.row,
                        column: p.column,
                        content: p.content,
                        missing: p.missing,
                        segments,
                        source: p.source,
                        measures: old.measures.clone(),
                        runs: old.runs.clone(),
                        inner,
                    };
                }
                lines = old
                    .segments
                    .iter()
                    .zip(old.measures.iter())
                    .zip(old.runs.iter())
                    .map(|((s, &(word, content)), runs)| Measured {
                        range: moved(s),
                        display: s.display.clone(),
                        runs: runs.clone(),
                        word,
                        content,
                    })
                    .collect();
            }
            let measures = lines.iter().map(|m| (m.word, m.content)).collect();
            let runs = lines.iter().map(|m| m.runs.clone()).collect();
            let segments = lines
                .into_iter()
                .map(|m| {
                    let wrapped = window
                        .text_system()
                        .shape_text(
                            SharedString::from(m.display.text.clone()),
                            px(em),
                            &m.runs,
                            Some(px(inner)),
                            None,
                        )
                        .ok()
                        .and_then(|mut lines| (!lines.is_empty()).then(|| lines.swap_remove(0)))
                        .map(Rc::new);
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
                    let shift = align_shift(align, inner, m.content, row_starts.len());
                    cell_line(
                        line_ix,
                        m.range,
                        metrics,
                        m.display,
                        wrapped,
                        row_starts,
                        left + shift,
                    )
                })
                .collect();
            CellBox {
                row: p.row,
                column: p.column,
                content: p.content,
                missing: p.missing,
                segments,
                source: p.source,
                measures,
                runs,
                inner,
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
        scroll,
        height: (y + widget_pad).ceil(),
        grid_top,
        columns: lefts.into_iter().zip(widths).collect(),
        rows,
        cells,
    }
}

pub fn paint_grid(t: &TableLayout, left: Pixels, top: Pixels, window: &mut Window) {
    let left = left - px(t.scroll);
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
    let mask = window.content_mask().bounds;
    for (r, (y, h)) in t.rows.iter().enumerate() {
        let bottom = top + px(y + h);
        if r < last_row && bottom >= mask.top() && bottom <= mask.bottom() + px(1.0) {
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
                    point(left + px(x + t.scroll + w) - b, top + px(rows_top)),
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

pub fn paint_text(
    t: &TableLayout,
    left: Pixels,
    top: Pixels,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let mask = window.content_mask().bounds;
    for cell in &t.cells {
        let (y, h) = t.rows[cell.row];
        if top + px(y) > mask.bottom() || top + px(y + h) < mask.top() {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::whole_pixels;

    #[test]
    fn rounded_widths_never_overflow_a_table_that_fits() {
        let exact = [40.4, 120.3, 99.3, 140.0];
        let min = [40.0, 60.0, 80.0, 90.0];
        let widths = whole_pixels(&exact, &min, 400.0);
        assert!(widths.iter().sum::<f32>() <= 400.0, "{widths:?}");
        assert!(widths.iter().zip(&min).all(|(w, m)| w >= m));
        assert!(widths.iter().all(|w| w.fract() == 0.0));
        let wide = whole_pixels(&[300.5, 300.5], &[300.5, 300.5], 400.0);
        assert_eq!(wide, vec![301.0, 301.0]);
    }
}
