use std::ops::Range;

use cerata_markdown::list::{ListDecoration, ListDecorations, PrefixKind, list_decorations_in};
use cerata_markdown::{Decoration, EditorState, HideMode, LineStyle, NodeKind, Widget};
use cerata_render::HighlightSpan;
use gpui::{
    Font, FontStyle, FontWeight, Hsla, ShapedLine, SharedString, StrikethroughStyle, TextRun,
    UnderlineStyle, Window, WrappedLine, font, px,
};

use super::config::{EditorConfig, LineMetrics};
use super::display::DisplayLine;
use super::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Bold,
    Italic,
    Code,
    Strike,
    Link,
    Muted,
}

pub struct FrameInputs {
    pub range: Range<usize>,
    pub decorations: Vec<Decoration>,
    by_start: Vec<(usize, usize)>,
    long: Vec<usize>,
    pub lists: ListDecorations,
    pub spans: Vec<(Range<usize>, Span)>,
    pub highlights: Vec<HighlightSpan>,
    drawn_tables: Vec<Range<usize>>,
}

impl FrameInputs {
    pub fn collect(
        state: &EditorState,
        range: Range<usize>,
        highlight: &mut dyn FnMut(&str, &str) -> std::rc::Rc<Vec<HighlightSpan>>,
    ) -> Self {
        let decorations = state.decorations_in(range.clone());
        Self::from_decorations(state, range, decorations, Vec::new(), highlight)
    }

    pub fn collect_frame(
        state: &EditorState,
        range: Range<usize>,
        highlight: &mut dyn FnMut(&str, &str) -> std::rc::Rc<Vec<HighlightSpan>>,
    ) -> Self {
        let (decorations, drawn_tables) = state.frame_decorations_in(range.clone());
        Self::from_decorations(state, range, decorations, drawn_tables, highlight)
    }

    fn from_decorations(
        state: &EditorState,
        range: Range<usize>,
        decorations: Vec<Decoration>,
        drawn_tables: Vec<Range<usize>>,
        highlight: &mut dyn FnMut(&str, &str) -> std::rc::Rc<Vec<HighlightSpan>>,
    ) -> Self {
        let lists = list_decorations_in(state.tree(), state.text(), range.clone());
        let mut spans = Vec::new();
        let mut highlights = Vec::new();
        let mut skipped = drawn_tables.iter().peekable();
        for node in state.tree().iter_range(range.clone()) {
            use NodeKind as K;
            while skipped.peek().is_some_and(|t| t.end < node.from()) {
                skipped.next();
            }
            if skipped.peek().is_some_and(|t| {
                t.start <= node.from() && node.to() <= t.end && node.kind() != K::Table
            }) {
                continue;
            }
            let span = match node.kind() {
                K::StrongEmphasis => Some(Span::Bold),
                K::Emphasis => Some(Span::Italic),
                K::InlineCode => Some(Span::Code),
                K::Strikethrough => Some(Span::Strike),
                K::Link | K::Autolink | K::WikiLink => Some(Span::Link),
                K::Url if node.parent().is_none_or(|p| p.kind() != K::Image) => Some(Span::Link),
                K::HeaderMark
                | K::EmphasisMark
                | K::StrikethroughMark
                | K::EscapeMark
                | K::QuoteMark
                | K::LinkMark
                | K::LinkTitle
                | K::CodeInfo
                | K::FrontmatterMark
                | K::MathMark
                | K::HtmlTag
                | K::Comment => Some(Span::Muted),
                K::CodeMark => Some(
                    if node.parent().is_some_and(|p| p.kind() == K::InlineCode) {
                        Span::Code
                    } else {
                        Span::Muted
                    },
                ),
                K::FencedCode => {
                    if let Some(block) = cerata_markdown::code::describe(state.text(), node)
                        && !block.language.is_empty()
                    {
                        let code = state.text().slice(block.code_range.clone());
                        let base = block.code_range.start;
                        highlights.extend(highlight(&block.language, code).iter().map(|s| {
                            HighlightSpan {
                                range: base + s.range.start..base + s.range.end,
                                kind: s.kind,
                            }
                        }));
                    }
                    None
                }
                _ => None,
            };
            if let Some(span) = span {
                spans.push((node.range(), span));
            }
            if node.kind() == K::Url && node.parent().is_some_and(|p| p.kind() != K::Autolink) {
                spans.push((node.range(), Span::Muted));
            }
        }
        let mut inputs = Self {
            range,
            decorations,
            by_start: Vec::new(),
            long: Vec::new(),
            lists,
            spans,
            highlights,
            drawn_tables,
        };
        inputs.index();
        inputs
    }

    pub fn covers_content(&self, range: &Range<usize>) -> bool {
        self.covers(range)
            && !self
                .drawn_tables
                .iter()
                .any(|t| t.start <= range.end && range.start <= t.end)
    }

    fn index(&mut self) {
        self.by_start.clear();
        self.long.clear();
        for (i, d) in self.decorations.iter().enumerate() {
            let r = d.range();
            if r.len() > SHORT_DECORATION {
                self.long.push(i);
            } else {
                self.by_start.push((r.start, i));
            }
        }
        self.by_start.sort_unstable();
    }

    pub fn clear_decorations(&mut self) {
        self.decorations.clear();
        self.index();
    }

    pub fn line_decorations(&self, line: &Range<usize>) -> Vec<&Decoration> {
        let from = line.start.saturating_sub(SHORT_DECORATION);
        let lo = self.by_start.partition_point(|(s, _)| *s < from);
        let hi = self.by_start.partition_point(|(s, _)| *s <= line.end);
        let mut hits: Vec<usize> = self.by_start[lo..hi]
            .iter()
            .map(|(_, i)| *i)
            .chain(self.long.iter().copied())
            .filter(|i| overlaps(&self.decorations[*i].range(), line))
            .collect();
        hits.sort_unstable();
        hits.into_iter().map(|i| &self.decorations[i]).collect()
    }

    pub fn covers(&self, line: &Range<usize>) -> bool {
        self.range.start <= line.start && line.end <= self.range.end
    }
}

const SHORT_DECORATION: usize = 1024;

#[derive(Clone, Debug)]
pub struct Fonts {
    pub body: SharedString,
    pub mono: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Image { url: String },
    Rule,
    Rich(Widget),
    Table(std::rc::Rc<super::table::TableLayout>),
}

pub trait BlockSizer {
    fn image_box(&mut self, url: &str, requested: Option<f32>, width: f32) -> ImageBox;
    fn embed_box(&mut self, target: &str, requested: Option<f32>, width: f32) -> ImageBox;
    fn math(&mut self, tex: &str, display: bool) -> super::math::MathState;
    fn rich_height(&mut self, widget: &Widget, width: f32) -> f32;
    fn table(&mut self, ix: usize, scroll: f32) -> Option<std::rc::Rc<super::table::TableLayout>>;
    fn keep_table(&mut self, ix: usize, table: std::rc::Rc<super::table::TableLayout>);
    fn frozen_widths(&mut self) -> &mut Option<super::table::FrozenWidths>;

    fn previous_table(
        &mut self,
        ix: usize,
    ) -> Option<(std::rc::Rc<super::table::TableLayout>, Vec<Range<usize>>)>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineBlock {
    pub kind: BlockKind,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub replaces: bool,
    pub atomic_end: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageBox {
    Unavailable,
    Sized { width: f32, height: f32 },
}

#[derive(Clone, Debug)]
pub enum Marker {
    Bullet,
    Task { checked: bool },
    Ordered(Box<ShapedLine>),
}

#[derive(Clone, Debug)]
pub struct ListPrefix {
    pub marker: Marker,
    pub x: f32,
    pub width: f32,
    pub left: f32,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodeChrome {
    pub first: bool,
    pub last: bool,
    pub block_start: usize,
    pub scroll_x: f32,
    pub text_width: f32,
}

pub struct LaidLine {
    pub ix: usize,
    pub start: usize,
    pub len: usize,
    pub metrics: LineMetrics,
    pub display: DisplayLine,
    pub wrapped: Option<std::rc::Rc<WrappedLine>>,
    pub row_starts: Vec<usize>,
    pub text_left: f32,
    pub text_top: f32,
    pub prefix: Option<ListPrefix>,
    pub quote_bars: Vec<f32>,
    pub code: Option<CodeChrome>,
    pub table_source: Option<(bool, bool)>,
    pub block: Option<LineBlock>,
    pub inline_math: Vec<InlineMath>,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct InlineMath {
    pub range: Range<usize>,
    pub state: super::math::MathState,
}

impl LaidLine {
    pub fn end(&self) -> usize {
        self.start + self.len
    }

    pub fn rows(&self) -> usize {
        if self.wrapped.is_some() {
            self.row_starts.len()
        } else {
            0
        }
    }
}

pub struct LayoutCx<'a> {
    pub config: &'a EditorConfig,
    pub fonts: &'a Fonts,
    pub ch: f32,
    pub column_width: f32,
    pub marked: Option<Range<usize>>,
    pub code_scroll: &'a dyn Fn(usize) -> f32,
    pub image_preview: Option<(usize, f32)>,
}

pub const QUOTE_INDENT_EM: f32 = 1.0;
pub const QUOTE_BAR_WIDTH: f32 = 3.0;
pub const CODE_PADDING: f32 = 12.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: bool,
    muted: bool,
    invisible: bool,
    marked: bool,
    color: Option<Hsla>,
}

fn overlaps(range: &Range<usize>, line: &Range<usize>) -> bool {
    range.start <= line.end && range.end >= line.start
}

pub fn layout_line(
    state: &EditorState,
    ix: usize,
    inputs: &FrameInputs,
    cx: &LayoutCx<'_>,
    sizer: &mut dyn BlockSizer,
    window: &Window,
) -> LaidLine {
    let text = state.text();
    let line = text.line(ix);
    let range = line.range();
    let source = text.slice(range.clone());
    let decorations: Vec<&Decoration> = inputs.line_decorations(&range);

    let mut style: Option<LineStyle> = None;
    let mut quote = false;
    let mut table_source: Option<(bool, bool)> = None;
    for decoration in &decorations {
        if let Decoration::Line {
            line_start,
            style: s,
        } = decoration
            && *line_start == line.from
        {
            match s {
                LineStyle::Quote => quote = true,
                LineStyle::TableSource { first, last } => table_source = Some((*first, *last)),
                LineStyle::Heading(_) => style = Some(*s),
                LineStyle::Code { .. } if !matches!(style, Some(LineStyle::Heading(_))) => {
                    style = Some(*s)
                }
                LineStyle::Code { .. } => {}
            }
        }
    }
    let metrics = cx.config.metrics(style);
    let em = metrics.font_size;

    let hidden_line = decorations.iter().any(|d| match d {
        Decoration::Hide {
            range: r,
            mode: HideMode::Block,
        } => r.start <= line.from && r.end >= line.to,
        Decoration::Replace {
            range: r,
            widget,
            block: true,
        } if super::blocks::is_rich(widget) => r.start < line.from && line.from <= r.end,
        _ => false,
    });
    let mut block: Option<LineBlock> = None;
    let mut muted_source = false;
    for decoration in &decorations {
        let (widget, replaces, end) = match decoration {
            Decoration::Replace {
                range: r,
                widget,
                block: true,
            } if super::blocks::is_rich(widget) => {
                if line.from <= r.start && r.start <= line.to {
                    (widget, true, r.end)
                } else {
                    continue;
                }
            }
            Decoration::Replace {
                range: r,
                widget,
                block: true,
            } if r.start <= line.from && r.end >= line.to => (widget, true, r.end),
            Decoration::Widget {
                pos,
                widget,
                block: true,
            } if *pos == line.to => (widget, false, *pos),
            _ => continue,
        };
        let preview = match decoration {
            Decoration::Replace { range: r, .. } => cx
                .image_preview
                .filter(|(start, _)| *start == r.start)
                .map(|(_, w)| w),
            _ => None,
        };
        let image = match widget {
            Widget::Image { url, width } => Some((
                sizer.image_box(url, preview.or(width.map(|w| w as f32)), cx.column_width),
                url.clone(),
            )),
            Widget::WikiImage { target, width } => Some((
                sizer.embed_box(target, preview.or(width.map(|w| w as f32)), cx.column_width),
                super::images::embed_key(target),
            )),
            _ => None,
        };
        if let Some((image_box, url)) = image {
            match image_box {
                ImageBox::Sized { width, height } => {
                    block = Some(LineBlock {
                        kind: BlockKind::Image { url },
                        left: ((cx.column_width - width) / 2.0).max(0.0),
                        top: 0.0,
                        width,
                        height,
                        replaces,
                        atomic_end: None,
                    });
                }
                ImageBox::Unavailable => muted_source = true,
            }
            continue;
        }
        match widget {
            Widget::HorizontalRule => {
                block = Some(LineBlock {
                    kind: BlockKind::Rule,
                    left: 0.0,
                    top: 0.0,
                    width: cx.column_width,
                    height: cx.config.row_height(),
                    replaces,
                    atomic_end: None,
                });
            }
            Widget::Table { grid, .. } => {
                let scroll = (cx.code_scroll)(grid.range.start);
                let laid = match sizer.table(ix, scroll) {
                    Some(laid) => laid,
                    None => {
                        let previous = sizer.previous_table(ix);
                        let ranges = super::table::edit_ranges(
                            state.selection(),
                            state.drag_selection(),
                            cx.marked.as_ref(),
                        );
                        let (reuse, fresh) = match &previous {
                            Some((table, old)) => {
                                let (reuse, fresh) =
                                    super::table::reusable(state, grid, table, old, &ranges);
                                (Some(reuse), fresh)
                            }
                            None => (None, Some(grid.range.clone())),
                        };
                        let own;
                        let table_inputs = match fresh {
                            Some(range) if !inputs.covers_content(&range) => {
                                own = FrameInputs::collect(state, range, &mut |_, _| {
                                    std::rc::Rc::new(Vec::new())
                                });
                                &own
                            }
                            _ => inputs,
                        };
                        let head = state.selection().main().head;
                        let zone = grid.zone();
                        let editing = zone.start <= head && head <= zone.end;
                        let frozen = sizer.frozen_widths();
                        let keep = frozen
                            .as_ref()
                            .filter(|f| editing && f.fits(grid, cx.column_width))
                            .map(|f| f.widths.clone());
                        let laid = std::rc::Rc::new(super::table::layout(
                            state,
                            ix,
                            grid.clone(),
                            table_inputs,
                            keep.as_deref(),
                            scroll,
                            reuse.as_ref(),
                            cx,
                            window,
                        ));
                        if editing && keep.is_none() {
                            *frozen = Some(super::table::FrozenWidths {
                                start: grid.range.start,
                                column_width: cx.column_width,
                                widths: laid.columns.iter().map(|(_, w)| *w).collect(),
                            });
                        }
                        sizer.keep_table(ix, laid.clone());
                        laid
                    }
                };
                block = Some(LineBlock {
                    left: 0.0,
                    top: 0.0,
                    width: laid.width,
                    height: laid.height,
                    kind: BlockKind::Table(laid),
                    replaces,
                    atomic_end: None,
                });
            }
            w if super::blocks::is_rich(w) => {
                block = Some(LineBlock {
                    kind: BlockKind::Rich(w.clone()),
                    left: 0.0,
                    top: 0.0,
                    width: cx.column_width,
                    height: sizer.rich_height(w, cx.column_width),
                    replaces,
                    atomic_end: matches!(w, Widget::Mermaid { .. }).then_some(end),
                });
            }
            _ => {}
        }
    }

    let mut replacements: Vec<(Range<usize>, String)> = Vec::new();
    let mut inline_math: Vec<InlineMath> = Vec::new();
    let mut errors: Vec<Range<usize>> = Vec::new();
    let mut invisible: Vec<Range<usize>> = Vec::new();
    let local = |r: &Range<usize>| {
        r.start.clamp(line.from, line.to) - line.from..r.end.clamp(line.from, line.to) - line.from
    };
    for decoration in &decorations {
        match decoration {
            Decoration::Hide { range: r, mode } => match mode {
                HideMode::Collapse | HideMode::Remove => {
                    replacements.push((local(r), String::new()))
                }
                HideMode::KeepSpace => invisible.push(local(r)),
                HideMode::Block => {}
            },
            Decoration::Replace {
                range: r,
                widget,
                block: false,
            } => {
                let glyph = match widget {
                    Widget::Dash(_) | Widget::Emoji(_) | Widget::WikiLink { .. } => {
                        widget.glyph().map(str::to_owned)
                    }
                    Widget::Tab => Some("\u{2007}".repeat(cerata_markdown::decorate::TAB_WIDTH_CH)),
                    Widget::Math { tex, display } => {
                        let state = sizer.math(tex, *display);
                        let width = match &state {
                            super::math::MathState::Ready(svg) => Some(svg.width),
                            super::math::MathState::Pending => {
                                Some(super::math::estimated_width(tex, cx.config.text_size()))
                            }
                            super::math::MathState::Error(_) => {
                                errors.push(local(r));
                                None
                            }
                        };
                        inline_math.push(InlineMath {
                            range: local(r),
                            state,
                        });
                        width.map(|w| {
                            "\u{2007}".repeat((w / cx.ch.max(1.0)).ceil().max(1.0) as usize)
                        })
                    }
                    _ => None,
                };
                if let Some(glyph) = glyph {
                    replacements.push((local(r), glyph));
                }
            }
            _ => {}
        }
    }

    let mut text_left = 0.0;
    let mut prefix = None;
    let mut hanging = 0.0;
    let mut indent_ch = 0usize;
    for decoration in &inputs.lists.all {
        match decoration {
            ListDecoration::Line {
                line_start,
                padding_ch,
                hanging_ch,
            } if *line_start == line.from => {
                text_left = padding_ch * cx.ch;
                hanging = hanging_ch * cx.ch;
            }
            ListDecoration::Prefix {
                range: r,
                kind,
                width_ch: _,
                marker_offset_ch,
            } if r.start >= line.from && r.start <= line.to => {
                replacements.push((local(r), String::new()));
                prefix = Some(ListPrefix {
                    marker: match kind {
                        PrefixKind::Bullet => Marker::Bullet,
                        PrefixKind::Task { checked } => Marker::Task { checked: *checked },
                    },
                    x: *marker_offset_ch as f32 * cx.ch,
                    width: cerata_markdown::list::LIST_UNIT_CH as f32 * cx.ch,
                    left: 0.0,
                    range: r.clone(),
                });
            }
            ListDecoration::IndentVisual { range: r, width_ch }
                if r.start >= line.from && r.start <= line.to =>
            {
                replacements.push((local(r), String::new()));
                indent_ch = *width_ch;
            }
            ListDecoration::CollapsedIndent { range: r }
                if r.start >= line.from && r.start <= line.to =>
            {
                replacements.push((local(r), String::new()));
            }
            ListDecoration::OrderedMarker { range: r }
                if r.start >= line.from && r.start <= line.to =>
            {
                let end = (r.end + 1).min(line.to);
                replacements.push((local(&(r.start..end)), String::new()));
                let label = SharedString::from(text.slice(r.clone()).to_owned());
                let run = text_run(label.len(), &cx.fonts.body, false, theme::text());
                let shaped = window.text_system().shape_line(label, px(em), &[run], None);
                prefix = Some(ListPrefix {
                    marker: Marker::Ordered(Box::new(shaped)),
                    x: indent_ch as f32 * cx.ch,
                    width: cerata_markdown::list::LIST_UNIT_CH as f32 * cx.ch,
                    left: 0.0,
                    range: line.from..end,
                });
            }
            _ => {}
        }
    }
    if let Some(prefix) = &mut prefix {
        prefix.left = text_left - hanging;
        prefix.x += prefix.left;
    }
    replacements.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let mut disjoint: Vec<(Range<usize>, String)> = Vec::with_capacity(replacements.len());
    for (r, glyph) in replacements {
        match disjoint.last_mut() {
            Some((last, _)) if r.start < last.end => last.end = last.end.max(r.end),
            _ => disjoint.push((r, glyph)),
        }
    }

    let mut quote_bars = Vec::new();
    if quote {
        quote_bars.push(0.0);
        text_left += QUOTE_INDENT_EM * cx.config.text_size();
    }

    if table_source.is_some() {
        text_left += CODE_PADDING;
    }
    let code = match style {
        Some(LineStyle::Code { first, last }) => {
            let block_start = cerata_markdown::code::code_block_at(state.tree(), line.from)
                .map_or(line.from, |n| n.from());
            text_left += CODE_PADDING;
            Some(CodeChrome {
                first,
                last,
                block_start,
                scroll_x: (cx.code_scroll)(block_start),
                text_width: 0.0,
            })
        }
        _ => None,
    };

    let replaced = block.as_ref().is_some_and(|b| b.replaces);
    let display = if replaced || hidden_line {
        DisplayLine::new(source, &[(0..source.len(), String::new())])
    } else {
        DisplayLine::new(source, &disjoint)
    };

    let (wrapped, row_starts) = if replaced || hidden_line {
        (None, Vec::new())
    } else {
        let marked = cx.marked.as_ref().and_then(|m| {
            (m.start >= line.from && m.end <= line.to)
                .then(|| m.start - line.from..m.end - line.from)
        });
        let runs = runs(
            &display,
            line.from,
            &inputs.spans,
            &inputs.highlights,
            &invisible,
            &errors,
            marked,
            style,
            false,
            muted_source,
            table_source.is_some(),
            cx,
        );
        let right_padding = if table_source.is_some() {
            CODE_PADDING
        } else {
            0.0
        };
        let wrap = if code.is_some() {
            None
        } else {
            Some(px(
                (cx.column_width - text_left - right_padding).max(cx.ch * 4.0)
            ))
        };
        let shaped = window.text_system().shape_text(
            SharedString::from(display.text.clone()),
            px(em),
            &runs,
            wrap,
            None,
        );
        let wrapped = shaped
            .ok()
            .and_then(|mut lines| (!lines.is_empty()).then(|| lines.swap_remove(0)))
            .map(std::rc::Rc::new);
        let row_starts = wrapped.as_ref().map_or_else(
            || vec![0],
            |line| {
                std::iter::once(0)
                    .chain(line.wrap_boundaries().iter().map(|boundary| {
                        line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
                    }))
                    .collect()
            },
        );
        (wrapped, row_starts)
    };

    let mut code = code;
    if let (Some(code), Some(wrapped)) = (&mut code, &wrapped) {
        code.text_width = wrapped.unwrapped_layout.width.as_f32();
    }
    let rows = if wrapped.is_some() {
        row_starts.len()
    } else {
        0
    };
    let text_height = if hidden_line {
        0.0
    } else {
        metrics.pad_top + rows as f32 * metrics.row_height + metrics.pad_bottom
    };
    let height = match &mut block {
        Some(block) if block.replaces => block.height,
        Some(block) => {
            block.top = text_height;
            text_height + block.height
        }
        None => text_height,
    };
    LaidLine {
        ix,
        start: line.from,
        len: range.len(),
        metrics,
        display,
        wrapped,
        row_starts,
        text_left,
        text_top: metrics.pad_top,
        prefix,
        quote_bars,
        code,
        table_source,
        block,
        inline_math,
        height,
    }
}

fn text_run(len: usize, family: &SharedString, bold: bool, color: Hsla) -> TextRun {
    let mut run_font: Font = font(family.clone());
    if bold {
        run_font.weight = FontWeight::BOLD;
    }
    TextRun {
        len,
        font: run_font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn runs(
    display: &DisplayLine,
    line_from: usize,
    spans: &[(Range<usize>, Span)],
    highlights: &[HighlightSpan],
    invisible: &[Range<usize>],
    errors: &[Range<usize>],
    marked: Option<Range<usize>>,
    style: Option<LineStyle>,
    header: bool,
    muted_source: bool,
    table_source: bool,
    cx: &LayoutCx<'_>,
) -> Vec<TextRun> {
    let line_end = line_from + display.buffer_len;
    let line_range = line_from..line_end;
    let spans: Vec<(Range<usize>, Span)> = spans
        .iter()
        .filter(|(r, _)| overlaps(r, &line_range))
        .map(|(r, s)| {
            let start = r.start.clamp(line_from, line_end) - line_from;
            (start..r.end.clamp(line_from, line_end) - line_from, *s)
        })
        .collect();
    let highlights: Vec<(Range<usize>, Hsla)> = highlights
        .iter()
        .filter(|h| overlaps(&h.range, &line_range))
        .map(|h| {
            (
                h.range.start.max(line_from) - line_from..h.range.end.min(line_end) - line_from,
                theme::highlight(h.kind),
            )
        })
        .collect();
    let mut boundaries = vec![0, display.buffer_len];
    for (r, _) in &spans {
        boundaries.extend([r.start, r.end]);
    }
    for (r, _) in &highlights {
        boundaries.extend([r.start, r.end]);
    }
    for r in invisible.iter().chain(errors) {
        boundaries.extend([r.start, r.end]);
    }
    if let Some(m) = &marked {
        boundaries.extend([m.start, m.end]);
    }
    boundaries.retain(|b| *b <= display.buffer_len);
    boundaries.sort_unstable();
    boundaries.dedup();

    let code_line = matches!(style, Some(LineStyle::Code { .. }));
    let heading = matches!(style, Some(LineStyle::Heading(_))) || header;
    let style_at = |offset: usize| {
        let mut s = Style {
            bold: heading,
            muted: muted_source,
            ..Style::default()
        };
        for (r, span) in &spans {
            if r.start <= offset && offset < r.end {
                match span {
                    Span::Bold => s.bold = true,
                    Span::Italic => s.italic = true,
                    Span::Code => s.code = true,
                    Span::Strike => s.strike = true,
                    Span::Link => s.link = true,
                    Span::Muted => s.muted = true,
                }
            }
        }
        if code_line {
            s.code = false;
            s.color = highlights
                .iter()
                .rev()
                .find(|(r, _)| r.start <= offset && offset < r.end)
                .map(|(_, c)| *c);
        }
        if errors.iter().any(|r| r.start <= offset && offset < r.end) {
            s.code = true;
            s.color = Some(theme::danger());
        }
        s.invisible = invisible
            .iter()
            .any(|r| r.start <= offset && offset < r.end);
        s.marked = marked
            .as_ref()
            .is_some_and(|m| m.start <= offset && offset < m.end);
        s
    };
    let to_run = |len: usize, s: Style| {
        let family = if code_line || table_source || s.code {
            &cx.fonts.mono
        } else {
            &cx.fonts.body
        };
        let mut run_font = font(family.clone());
        if heading {
            run_font.weight = FontWeight(cerata_markdown::decorate::HEADING_WEIGHT as f32);
        }
        if s.bold && !heading {
            run_font.weight = FontWeight::BOLD;
        }
        if s.italic {
            run_font.style = FontStyle::Italic;
        }
        let color = if s.invisible {
            theme::transparent()
        } else if let Some(color) = s.color {
            color
        } else if s.muted {
            theme::muted()
        } else if s.link {
            theme::accent()
        } else {
            theme::text()
        };
        TextRun {
            len,
            font: run_font,
            color,
            background_color: (s.code && !s.invisible).then(theme::code_background),
            underline: s.marked.then(|| UnderlineStyle {
                thickness: px(2.0),
                color: Some(theme::text()),
                wavy: false,
            }),
            strikethrough: s.strike.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
        }
    };

    let mut out: Vec<TextRun> = Vec::new();
    let mut last: Option<Style> = None;
    let mut push = |out: &mut Vec<TextRun>, len: usize, s: Style| {
        if len == 0 {
            return;
        }
        if last == Some(s)
            && let Some(run) = out.last_mut()
        {
            run.len += len;
            return;
        }
        out.push(to_run(len, s));
        last = Some(s);
    };
    for segment in display.source_runs() {
        if !segment.source {
            let s = style_at(segment.buffer.start);
            push(
                &mut out,
                segment.display.len(),
                Style {
                    invisible: false,
                    ..s
                },
            );
            continue;
        }
        let mut start = segment.buffer.start;
        for &boundary in boundaries
            .iter()
            .filter(|&&b| b > segment.buffer.start && b < segment.buffer.end)
            .chain(std::iter::once(&segment.buffer.end))
        {
            push(&mut out, boundary - start, style_at(start));
            start = boundary;
        }
    }
    out
}
