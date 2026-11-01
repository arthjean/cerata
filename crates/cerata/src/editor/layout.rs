//! Line layout: one document line, its decorations and its inline syntax,
//! turned into a display line shaped at the line's own font size, with the
//! chrome drawn around it (list marker, quote bars, code background) and
//! the block widget it holds.

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

/// Inline styles from the syntax tree, applied over line ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Bold,
    Italic,
    Code,
    Strike,
    Link,
    /// Revealed syntax: marks, URLs, fences.
    Muted,
}

/// Everything a run of lines needs, gathered once per frame for the range
/// the frame shows, so each line does not walk the tree again.
pub struct FrameInputs {
    pub range: Range<usize>,
    pub decorations: Vec<Decoration>,
    pub lists: ListDecorations,
    pub spans: Vec<(Range<usize>, Span)>,
    pub highlights: Vec<HighlightSpan>,
}

impl FrameInputs {
    pub fn collect(
        state: &EditorState,
        range: Range<usize>,
        highlight: &mut dyn FnMut(&str, &str) -> std::rc::Rc<Vec<HighlightSpan>>,
    ) -> Self {
        let decorations = state.decorations_in(range.clone());
        let lists = list_decorations_in(state.tree(), state.text(), range.clone());
        let mut spans = Vec::new();
        let mut highlights = Vec::new();
        for node in state.tree().iter_range(range.clone()) {
            use NodeKind as K;
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
            // A Url inside a link is syntax once revealed.
            if node.kind() == K::Url && node.parent().is_some_and(|p| p.kind() != K::Autolink) {
                spans.push((node.range(), Span::Muted));
            }
        }
        Self {
            range,
            decorations,
            lists,
            spans,
            highlights,
        }
    }

    pub fn covers(&self, line: &Range<usize>) -> bool {
        self.range.start <= line.start && line.end <= self.range.end
    }
}

/// Fonts resolved for this editor.
#[derive(Clone, Debug)]
pub struct Fonts {
    pub body: SharedString,
    pub mono: SharedString,
}

/// A block widget held by a line.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Image { url: String },
    Rule,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineBlock {
    pub kind: BlockKind,
    /// Top of the widget, relative to the line top.
    pub top: f32,
    pub width: f32,
    pub height: f32,
    /// The widget replaces the line's source (else it sits below it).
    pub replaces: bool,
}

/// What a block image measures to, as the widget host reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageBox {
    /// A local file that does not exist, or a scheme that is never
    /// loaded: the source stays, muted.
    Unavailable,
    Sized {
        width: f32,
        height: f32,
    },
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
    /// Column-relative x of the marker's `LIST_UNIT_CH` column.
    pub x: f32,
    pub width: f32,
    /// Column-relative x where the whole prefix starts, indentation
    /// included (Writer's `.cm-list-prefix` box, for clicks).
    pub left: f32,
    /// Buffer range of the source prefix (for clicks).
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodeChrome {
    pub first: bool,
    pub last: bool,
    /// Start of the code block, which keys its horizontal scroll offset.
    pub block_start: usize,
    pub scroll_x: f32,
    /// Width of the widest shaped text of this line, for the scroll bound.
    pub text_width: f32,
}

pub struct LaidLine {
    pub ix: usize,
    /// Absolute byte offset of the line start.
    pub start: usize,
    pub len: usize,
    pub metrics: LineMetrics,
    pub display: DisplayLine,
    /// `None` when the line is hidden or a block widget replaces it.
    pub wrapped: Option<WrappedLine>,
    /// Display offsets where each wrapped row starts.
    pub row_starts: Vec<usize>,
    /// Column-relative x of the text.
    pub text_left: f32,
    pub text_top: f32,
    pub prefix: Option<ListPrefix>,
    /// Column-relative x of each quote bar.
    pub quote_bars: Vec<f32>,
    pub code: Option<CodeChrome>,
    pub block: Option<LineBlock>,
    pub height: f32,
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

/// What laying out a line needs besides its inputs.
pub struct LayoutCx<'a> {
    pub config: &'a EditorConfig,
    pub fonts: &'a Fonts,
    /// Width of `0` in the body font, CSS's `ch`.
    pub ch: f32,
    /// Width of the text column.
    pub column_width: f32,
    /// IME marked text, underlined.
    pub marked: Option<Range<usize>>,
    /// Horizontal scroll offset of the code block starting at an offset.
    pub code_scroll: &'a dyn Fn(usize) -> f32,
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

/// Lays out document line `ix` of `state` from `inputs`.
pub fn layout_line(
    state: &EditorState,
    ix: usize,
    inputs: &FrameInputs,
    cx: &LayoutCx<'_>,
    image_box: &mut dyn FnMut(&str, f32) -> ImageBox,
    window: &Window,
) -> LaidLine {
    let text = state.text();
    let line = text.line(ix);
    let range = line.range();
    let source = text.slice(range.clone());
    let decorations: Vec<&Decoration> = inputs
        .decorations
        .iter()
        .filter(|d| overlaps(&d.range(), &range))
        .collect();

    let mut style: Option<LineStyle> = None;
    let mut quote = false;
    for decoration in &decorations {
        if let Decoration::Line {
            line_start,
            style: s,
        } = decoration
            && *line_start == line.from
        {
            match s {
                LineStyle::Quote => quote = true,
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

    // Whole-line hides (setext underline) and block widgets.
    let hidden_line = decorations.iter().any(|d| {
        matches!(d, Decoration::Hide { range: r, mode: HideMode::Block } if r.start <= line.from && r.end >= line.to)
    });
    let mut block: Option<LineBlock> = None;
    let mut muted_source = false;
    for decoration in &decorations {
        let (widget, replaces) = match decoration {
            Decoration::Replace {
                range: r,
                widget,
                block: true,
            } if r.start <= line.from && r.end >= line.to => (widget, true),
            Decoration::Widget {
                pos,
                widget,
                block: true,
            } if *pos == line.to => (widget, false),
            _ => continue,
        };
        match widget {
            Widget::Image { url } => match image_box(url, cx.column_width) {
                ImageBox::Sized { width, height } => {
                    block = Some(LineBlock {
                        kind: BlockKind::Image { url: url.clone() },
                        top: 0.0,
                        width,
                        height,
                        replaces,
                    });
                }
                ImageBox::Unavailable => muted_source = true,
            },
            Widget::HorizontalRule => {
                block = Some(LineBlock {
                    kind: BlockKind::Rule,
                    top: 0.0,
                    width: cx.column_width,
                    height: cx.config.row_height(),
                    replaces,
                });
            }
            _ => {}
        }
    }

    // Inline replacements, line-relative, sorted and disjoint.
    let mut replacements: Vec<(Range<usize>, String)> = Vec::new();
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
                    Widget::Dash(_) | Widget::Emoji(_) => widget.glyph().map(str::to_owned),
                    Widget::Tab => Some("\u{2007}".repeat(cerata_markdown::decorate::TAB_WIDTH_CH)),
                    // Inline images keep their source (block images fold).
                    _ => None,
                };
                if let Some(glyph) = glyph {
                    replacements.push((local(r), glyph));
                }
            }
            _ => {}
        }
    }

    // List geometry.
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
                // Drawn in the hanging indent; the source (and the space
                // after it) leaves the text.
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
            inputs,
            &invisible,
            marked,
            style,
            muted_source,
            cx,
        );
        let wrap = if code.is_some() {
            None
        } else {
            Some(px((cx.column_width - text_left).max(cx.ch * 4.0)))
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
            .and_then(|mut lines| (!lines.is_empty()).then(|| lines.swap_remove(0)));
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
        block,
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

/// Text runs of the display line: one per stretch of uniform style.
#[allow(clippy::too_many_arguments)]
fn runs(
    display: &DisplayLine,
    line_from: usize,
    inputs: &FrameInputs,
    invisible: &[Range<usize>],
    marked: Option<Range<usize>>,
    style: Option<LineStyle>,
    muted_source: bool,
    cx: &LayoutCx<'_>,
) -> Vec<TextRun> {
    let line_end = line_from + display.buffer_len;
    let line_range = line_from..line_end;
    let spans: Vec<(Range<usize>, Span)> = inputs
        .spans
        .iter()
        .filter(|(r, _)| overlaps(r, &line_range))
        .map(|(r, s)| {
            let start = r.start.clamp(line_from, line_end) - line_from;
            (start..r.end.clamp(line_from, line_end) - line_from, *s)
        })
        .collect();
    let highlights: Vec<(Range<usize>, Hsla)> = inputs
        .highlights
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
    for r in invisible {
        boundaries.extend([r.start, r.end]);
    }
    if let Some(m) = &marked {
        boundaries.extend([m.start, m.end]);
    }
    boundaries.retain(|b| *b <= display.buffer_len);
    boundaries.sort_unstable();
    boundaries.dedup();

    let code_line = matches!(style, Some(LineStyle::Code { .. }));
    let heading = matches!(style, Some(LineStyle::Heading(_)));
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
        s.invisible = invisible
            .iter()
            .any(|r| r.start <= offset && offset < r.end);
        s.marked = marked
            .as_ref()
            .is_some_and(|m| m.start <= offset && offset < m.end);
        s
    };
    let to_run = |len: usize, s: Style| {
        let family = if code_line || s.code {
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
