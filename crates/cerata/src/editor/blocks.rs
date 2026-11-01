//! Rich block widgets drawn in place of their source: GFM tables (US-032).
//! Each block is a GPUI element tree laid out in the editor's prepaint. A
//! block reserves its estimated height until the first frame measures it;
//! the measured height is kept per content and column width, so the line
//! is laid out at its real height from then on.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use cerata_markdown::Widget;
use cerata_markdown::preview::Inline;
use cerata_markdown::table::{self, Align, Table};
use gpui::{
    AnyElement, App, Context, FontStyle, FontWeight, Hsla, MouseButton, SharedString,
    StrikethroughStyle, StyledText, TextRun, Window, div, font, prelude::*, px, relative,
};

use super::layout::Fonts;
use super::{Editor, theme};

/// Where a click on rendered text leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A Markdown destination, as written.
    Url(String),
    /// The raw text between a wiki link's brackets.
    Wiki(String),
}

/// What laying out a rich block needs from the editor.
#[derive(Clone)]
pub struct BlockStyle {
    pub fonts: Fonts,
    pub font_size: f32,
}

#[derive(Default)]
pub struct RichBlocks {
    /// Measured heights, by content and column width.
    measured: HashMap<(u64, u32), f32>,
    tables: HashMap<u64, Rc<Option<Table>>>,
    html: super::html::HtmlBlocks,
}

fn key(widget: &Widget) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    widget.hash(&mut hasher);
    hasher.finish()
}

/// Widgets drawn by this host, as opposed to images and rules.
pub fn is_rich(widget: &Widget) -> bool {
    matches!(
        widget,
        Widget::Table { .. }
            | Widget::Frontmatter
            | Widget::Html { .. }
            | Widget::Math { .. }
            | Widget::Mermaid { .. }
    )
}

impl RichBlocks {
    /// The sanitized tree of an HTML block, and its key.
    pub fn html(&mut self, source: &str) -> (u64, Rc<Vec<cerata_markdown::html::Node>>) {
        let key = key(&Widget::Html {
            source: source.to_owned(),
        });
        (key, self.html.tree(source, key))
    }

    pub fn details_open(&self, key: u64, index: usize, default: bool) -> bool {
        self.html.is_open(key, index, default)
    }

    pub fn toggle_details(&mut self, key: u64, index: usize, default: bool) {
        self.html.toggle(key, index, default);
    }

    fn table(&mut self, source: &str, key: u64) -> Rc<Option<Table>> {
        if self.tables.len() > 256 {
            self.tables.clear();
        }
        self.tables
            .entry(key)
            .or_insert_with(|| Rc::new(table::parse(source, |l| l)))
            .clone()
    }

    /// The height `widget` takes in a column `width` wide: measured when a
    /// frame drew it at that width, else estimated.
    pub fn height(&mut self, widget: &Widget, width: f32, font_size: f32) -> f32 {
        let key = key(widget);
        if let Some(height) = self.measured.get(&(key, width as u32)) {
            return *height;
        }
        match widget {
            Widget::Table { source } => match self.table(source, key).as_ref() {
                Some(t) => table::estimate_height(t, font_size),
                None => font_size,
            },
            // One row and the Add button, until a frame measures it.
            Widget::Frontmatter => 96.0,
            Widget::Html { source } => source.lines().count().max(1) as f32 * font_size * 1.6,
            Widget::Mermaid { .. } => {
                super::mermaid::CANVAS_HEIGHT + super::mermaid::CANVAS_MARGIN * 2.0
            }
            _ => font_size,
        }
    }

    /// Records the height a frame measured; `true` when it changed.
    pub fn record(&mut self, widget: &Widget, width: f32, height: f32) -> bool {
        if self.measured.len() > 1024 {
            self.measured.clear();
        }
        let slot = self
            .measured
            .entry((key(widget), width as u32))
            .or_insert(-1.0);
        let changed = (*slot - height).abs() > 0.5;
        *slot = height;
        changed
    }

    /// The element drawn for `widget` in a column `width` wide.
    pub fn element(
        &mut self,
        widget: &Widget,
        width: f32,
        style: &BlockStyle,
        window: &Window,
        cx: &mut Context<Editor>,
    ) -> AnyElement {
        match widget {
            Widget::Table { source } => {
                let parsed = self.table(source, key(widget));
                match parsed.as_ref() {
                    Some(t) => table_element(t, width, style, window, cx),
                    None => div().into_any_element(),
                }
            }
            _ => div().into_any_element(),
        }
    }
}

// --- Inline text -------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: bool,
}

/// Rendered inline nodes flattened into one string, its runs and the
/// ranges that are links.
pub struct RichText {
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub links: Vec<(Range<usize>, LinkTarget)>,
}

pub fn rich_text(nodes: &[Inline], fonts: &Fonts, base_bold: bool) -> RichText {
    let mut text = String::new();
    let mut spans: Vec<(Range<usize>, InlineStyle)> = Vec::new();
    let mut links = Vec::new();
    fn walk(
        nodes: &[Inline],
        style: InlineStyle,
        text: &mut String,
        spans: &mut Vec<(Range<usize>, InlineStyle)>,
        links: &mut Vec<(Range<usize>, LinkTarget)>,
    ) {
        for node in nodes {
            match node {
                Inline::Text(t) => {
                    let start = text.len();
                    text.push_str(t);
                    spans.push((start..text.len(), style));
                }
                Inline::Break => {
                    let start = text.len();
                    text.push('\n');
                    spans.push((start..text.len(), style));
                }
                Inline::Strong(c) => walk(
                    c,
                    InlineStyle {
                        bold: true,
                        ..style
                    },
                    text,
                    spans,
                    links,
                ),
                Inline::Emphasis(c) => walk(
                    c,
                    InlineStyle {
                        italic: true,
                        ..style
                    },
                    text,
                    spans,
                    links,
                ),
                Inline::Code(c) => walk(
                    c,
                    InlineStyle {
                        code: true,
                        ..style
                    },
                    text,
                    spans,
                    links,
                ),
                Inline::Strike(c) => walk(
                    c,
                    InlineStyle {
                        strike: true,
                        ..style
                    },
                    text,
                    spans,
                    links,
                ),
                Inline::Link { href, children } => {
                    let start = text.len();
                    walk(
                        children,
                        InlineStyle {
                            link: true,
                            ..style
                        },
                        text,
                        spans,
                        links,
                    );
                    if let Some(href) = href {
                        links.push((start..text.len(), LinkTarget::Url(href.clone())));
                    }
                }
                Inline::Wiki { target, children } => {
                    let start = text.len();
                    walk(
                        children,
                        InlineStyle {
                            link: true,
                            ..style
                        },
                        text,
                        spans,
                        links,
                    );
                    links.push((start..text.len(), LinkTarget::Wiki(target.clone())));
                }
            }
        }
    }
    walk(
        nodes,
        InlineStyle::default(),
        &mut text,
        &mut spans,
        &mut links,
    );
    let runs = spans
        .iter()
        .filter(|(r, _)| !r.is_empty())
        .map(|(r, s)| {
            let family = if s.code { &fonts.mono } else { &fonts.body };
            let mut f = font(family.clone());
            if s.bold {
                f.weight = FontWeight::BOLD;
            } else if base_bold {
                f.weight = FontWeight(600.0);
            }
            if s.italic {
                f.style = FontStyle::Italic;
            }
            let color: Hsla = if s.link {
                theme::accent()
            } else {
                theme::text()
            };
            TextRun {
                len: r.len(),
                font: f,
                color,
                background_color: s.code.then(theme::code_background),
                underline: None,
                strikethrough: s.strike.then(|| StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(color),
                }),
            }
        })
        .collect();
    RichText {
        text: text.into(),
        runs,
        links,
    }
}

/// `rich` as a text element whose links open on click. A click elsewhere
/// falls through to the editor.
pub fn rich_text_element(rich: RichText, cx: &mut Context<Editor>) -> AnyElement {
    let styled = StyledText::new(rich.text.clone()).with_runs(rich.runs);
    if rich.links.is_empty() {
        return styled.into_any_element();
    }
    let layout = styled.layout().clone();
    let links = rich.links;
    let editor = cx.entity().downgrade();
    div()
        .child(styled)
        .on_mouse_down(MouseButton::Left, move |event, window, cx: &mut App| {
            let Ok(index) = layout.index_for_position(event.position) else {
                return;
            };
            let Some((_, target)) = links.iter().find(|(r, _)| r.contains(&index)) else {
                return;
            };
            cx.stop_propagation();
            let target = target.clone();
            editor
                .update(cx, |editor, cx| editor.follow_link(target, window, cx))
                .ok();
        })
        .into_any_element()
}

// --- Tables ------------------------------------------------------------------

fn shape_width(window: &Window, text: &str, family: &SharedString, bold: bool, size: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let mut f = font(family.clone());
    if bold {
        f.weight = FontWeight(600.0);
    }
    let run = TextRun {
        len: text.len(),
        font: f,
        color: theme::text(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::from(text.to_owned()), px(size), &[run], None)
        .width()
        .as_f32()
}

/// Longest word and whole content of a cell, in pixels.
fn cell_extent(cell: &[Inline], style: &BlockStyle, bold: bool, window: &Window) -> (f32, f32) {
    let plain = cerata_markdown::preview::plain_text(cell);
    let family = &style.fonts.body;
    let longest = plain
        .split_whitespace()
        .map(|w| shape_width(window, w, family, bold, style.font_size))
        .fold(0.0, f32::max);
    let content = plain
        .lines()
        .map(|l| shape_width(window, l, family, bold, style.font_size))
        .fold(0.0, f32::max);
    (longest.ceil(), content.ceil())
}

/// Column widths, padding included, for a table in a column `available`
/// pixels wide.
fn column_widths(t: &Table, available: f32, style: &BlockStyle, window: &Window) -> Vec<f32> {
    let em = style.font_size;
    let padding = 2.0 * table::CELL_HORIZONTAL_PADDING_EM * em + table::BORDER_WIDTH;
    let ch = shape_width(window, "0", &style.fonts.body, false, em).max(em * 0.5);
    let columns = t.columns();
    let mut min = vec![padding; columns];
    let mut max = vec![padding; columns];
    let rows = std::iter::once((&t.header, true)).chain(t.rows.iter().map(|r| (r, false)));
    for (row, bold) in rows {
        for (c, cell) in row.iter().enumerate().take(columns) {
            let (word, content) = cell_extent(cell, style, bold, window);
            min[c] = min[c].max(word + padding);
            max[c] = max[c].max(content + padding);
        }
    }
    let cap = table::CELL_MAX_WIDTH_CH * ch + padding;
    table::size_columns(&min, &max, available - table::BORDER_WIDTH, cap)
        .into_iter()
        .map(f32::ceil)
        .collect()
}

fn table_element(
    t: &Table,
    available: f32,
    style: &BlockStyle,
    window: &Window,
    cx: &mut Context<Editor>,
) -> AnyElement {
    let em = style.font_size;
    let widths = column_widths(t, available, style, window);
    let columns = widths.len();
    let border = theme::border();
    let cell =
        |content: &[Inline], c: usize, header: bool, last_row: bool, cx: &mut Context<Editor>| {
            let rich = rich_text(content, &style.fonts, header);
            let d = div();
            #[cfg(test)]
            let d = {
                let name = if header {
                    "table-header".to_owned()
                } else {
                    "table-cell".to_owned()
                };
                d.debug_selector(move || format!("{name}-{c}"))
            };
            let mut d = d
                .w(px(widths[c]))
                .flex_none()
                .px(px(table::CELL_HORIZONTAL_PADDING_EM * em))
                .py(px(table::CELL_VERTICAL_PADDING_EM * em / 2.0))
                .line_height(relative(table::CELL_LINE_HEIGHT))
                .border_color(border)
                .child(rich_text_element(rich, cx));
            if c + 1 < columns {
                d = d.border_r_1();
            }
            if !last_row {
                d = d.border_b_1();
            }
            match t.alignments.get(c).copied().flatten() {
                Some(Align::Center) => d = d.text_center(),
                Some(Align::Right) => d = d.text_right(),
                _ => {}
            }
            if header {
                d = d.bg(theme::code_background());
            }
            d
        };
    let body_rows = t.rows.len();
    let mut grid = div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(border)
        .rounded(px(8.0))
        .overflow_hidden()
        .text_size(px(em))
        .text_color(theme::text())
        .font_family(style.fonts.body.clone());
    let mut header = div().flex();
    for (c, content) in t.header.iter().enumerate() {
        header = header.child(cell(content, c, true, body_rows == 0, cx));
    }
    grid = grid.child(header);
    for (r, row) in t.rows.iter().enumerate() {
        let mut line = div().flex();
        for (c, content) in row.iter().enumerate() {
            line = line.child(cell(content, c, false, r + 1 == body_rows, cx));
        }
        grid = grid.child(line);
    }
    div()
        .w(px(available))
        .py(px(table::WIDGET_VERTICAL_PADDING_EM * em / 2.0))
        .child(div().flex().child(grid))
        .into_any_element()
}
