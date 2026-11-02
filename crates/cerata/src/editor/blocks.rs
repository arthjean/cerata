//! Rich block widgets drawn in place of their source (frontmatter, HTML,
//! formulas, diagrams). Each block is a GPUI element tree laid out in the
//! editor's prepaint. A block reserves its estimated height until the
//! first frame measures it; the measured height is kept per content and
//! column width, so the line is laid out at its real height from then on.
//! Tables are laid out by the editor itself (`table.rs`, ADR 0007).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use cerata_markdown::Widget;
use cerata_markdown::preview::Inline;
use gpui::{
    AnyElement, App, Context, FontStyle, FontWeight, Hsla, MouseButton, SharedString,
    StrikethroughStyle, StyledText, TextRun, div, font, prelude::*, px,
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

    /// The height `widget` takes in a column `width` wide: measured when a
    /// frame drew it at that width, else estimated.
    pub fn height(&mut self, widget: &Widget, width: f32, font_size: f32) -> f32 {
        let key = key(widget);
        if let Some(height) = self.measured.get(&(key, width as u32)) {
            return *height;
        }
        match widget {
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
