//! HTML blocks drawn in place of their source (US-035), from the tree
//! `cerata_markdown::html` sanitized: Writer's `.cm-html-block-widget`
//! styles (`html-block-decorations.ts:230-301`). Links follow the editor's
//! rules (US-033); `<summary>` opens and closes its `<details>`; images
//! load through the editor's image host, so only `https:`, `data:` and
//! files inside the workspace are read.

use std::rc::Rc;

use cerata_markdown::html::{self, Element, Node};
use gpui::{
    AnyElement, Context, FontWeight, MouseButton, SharedString, Window, div, prelude::*, px,
    relative,
};

use super::blocks::{BlockStyle, rich_text, rich_text_element};
use super::layout::ImageBox;
use super::{Editor, LoadJob, theme};

/// What drawing one block needs.
struct Cx<'a> {
    key: u64,
    style: &'a BlockStyle,
    row_height: f32,
    /// `<details>` seen so far, in document order: their index.
    details: usize,
    images: usize,
    jobs: Vec<LoadJob>,
}

impl Editor {
    /// The element drawn for the HTML block `source` in a column `width`
    /// wide.
    pub(super) fn html_element(
        &mut self,
        source: &str,
        width: f32,
        style: &BlockStyle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (key, nodes) = self.rich.html(source);
        let mut hcx = Cx {
            key,
            style,
            row_height: self.config.row_height(),
            details: 0,
            images: 0,
            jobs: Vec::new(),
        };
        let body = if html::is_empty(&nodes) {
            // Nothing survives sanitizing: the source, muted.
            vec![
                div()
                    .font_family(style.fonts.mono.clone())
                    .text_color(theme::muted())
                    .children(source.lines().map(|l| SharedString::from(l.to_owned())))
                    .into_any_element(),
            ]
        } else {
            self.html_blocks(&nodes, width, &mut hcx, window, cx)
        };
        let jobs = std::mem::take(&mut hcx.jobs);
        self.start_loads(jobs, cx);
        div()
            .w(px(width))
            .py(px(style.font_size * 0.25))
            .text_size(px(style.font_size))
            .line_height(relative(1.6))
            .flex()
            .flex_col()
            .children(body)
            .into_any_element()
    }

    /// Blocks of `nodes`: runs of inline content become paragraphs of
    /// rich text.
    fn html_blocks(
        &mut self,
        nodes: &[Node],
        width: f32,
        hcx: &mut Cx<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let mut run: Vec<Node> = Vec::new();
        let flush = |run: &mut Vec<Node>,
                     out: &mut Vec<AnyElement>,
                     hcx: &Cx<'_>,
                     cx: &mut Context<Self>| {
            if !html::is_empty(run) {
                let rich = rich_text(&html::paragraph(run), &hcx.style.fonts, false);
                out.push(div().child(rich_text_element(rich, cx)).into_any_element());
            }
            run.clear();
        };
        for node in nodes {
            match node {
                Node::Element(e) if node.is_block() => {
                    flush(&mut run, &mut out, hcx, cx);
                    out.push(self.html_block(e, width, hcx, window, cx));
                }
                other => run.push(other.clone()),
            }
        }
        flush(&mut run, &mut out, hcx, cx);
        out
    }

    fn html_block(
        &mut self,
        e: &Element,
        width: f32,
        hcx: &mut Cx<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let em = hcx.style.font_size;
        let children = |editor: &mut Self, width: f32, hcx: &mut Cx<'_>, cx: &mut Context<Self>| {
            editor.html_blocks(&e.children, width, hcx, window, cx)
        };
        match e.tag.as_str() {
            tag @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                let scale = match tag {
                    "h1" => 1.6,
                    "h2" => 1.4,
                    "h3" => 1.2,
                    _ => 1.0,
                };
                let rich = rich_text(&html::paragraph(&e.children), &hcx.style.fonts, true);
                div()
                    .mt(px(em * 0.5))
                    .mb(px(em * 0.25))
                    .text_size(px(em * scale))
                    .line_height(relative(1.3))
                    .font_weight(FontWeight(600.0))
                    .child(rich_text_element(rich, cx))
                    .into_any_element()
            }
            "p" => div()
                .my(px(em * 0.5))
                .children(children(self, width, hcx, cx))
                .into_any_element(),
            "blockquote" => div()
                .my(px(em * 0.5))
                .pl(px(em))
                .border_l(px(3.0))
                .border_color(theme::border())
                .text_color(theme::muted())
                .children(children(self, width - em - 3.0, hcx, cx))
                .into_any_element(),
            "hr" => div()
                .my(px(em * 0.75))
                .h(px(1.0))
                .bg(theme::border())
                .into_any_element(),
            "pre" => div()
                .my(px(em * 0.5))
                .px(px(em))
                .py(px(em * 0.75))
                .rounded(px(4.0))
                .bg(theme::code_background())
                .font_family(hcx.style.fonts.mono.clone())
                .text_size(px(em * 0.9))
                .children(
                    html::plain(&e.children)
                        .trim_end_matches('\n')
                        .lines()
                        .map(|l| {
                            div()
                                .min_h(px(em * 1.4))
                                .child(SharedString::from(l.to_owned()))
                        }),
                )
                .into_any_element(),
            "ul" | "ol" => {
                let ordered = e.tag == "ol";
                let start: usize = e.attr("start").and_then(|s| s.parse().ok()).unwrap_or(1);
                let mut items = Vec::new();
                let mut n = start;
                for child in &e.children {
                    let Node::Element(li) = child else { continue };
                    let marker: SharedString = if ordered {
                        format!("{n}.").into()
                    } else {
                        "\u{2022}".into()
                    };
                    n += 1;
                    let body = self.html_blocks(&li.children, width - em * 1.5, hcx, window, cx);
                    items.push(
                        div()
                            .flex()
                            .child(div().w(px(em * 1.5)).flex_shrink_0().child(marker))
                            .child(div().flex_1().min_w_0().children(body)),
                    );
                }
                div().my(px(em * 0.25)).children(items).into_any_element()
            }
            "dt" => div()
                .font_weight(FontWeight(600.0))
                .children(children(self, width, hcx, cx))
                .into_any_element(),
            "dd" => div()
                .pl(px(em * 1.5))
                .children(children(self, width - em * 1.5, hcx, cx))
                .into_any_element(),
            "table" => self.html_table(e, width, hcx, window, cx),
            "details" => self.html_details(e, width, hcx, window, cx),
            "img" => self.html_image(e, width, hcx),
            "figcaption" | "caption" => div()
                .text_size(px(em * 0.9))
                .text_color(theme::muted())
                .children(children(self, width, hcx, cx))
                .into_any_element(),
            "video" | "audio" => {
                let fallback = children(self, width, hcx, cx);
                if fallback.is_empty() {
                    let what = if e.tag == "video" { "Video" } else { "Audio" };
                    div()
                        .text_color(theme::muted())
                        .child(SharedString::from(format!(
                            "{what} can't be played in the editor"
                        )))
                        .into_any_element()
                } else {
                    div().children(fallback).into_any_element()
                }
            }
            "source" | "col" | "colgroup" => div().into_any_element(),
            _ => div()
                .children(children(self, width, hcx, cx))
                .into_any_element(),
        }
    }

    fn html_table(
        &mut self,
        e: &Element,
        width: f32,
        hcx: &mut Cx<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        fn rows<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
            for child in &e.children {
                if let Node::Element(c) = child {
                    match c.tag.as_str() {
                        "tr" => out.push(c),
                        "thead" | "tbody" | "tfoot" => rows(c, out),
                        _ => {}
                    }
                }
            }
        }
        let mut found = Vec::new();
        rows(e, &mut found);
        let em = hcx.style.font_size;
        let columns = found
            .iter()
            .map(|r| {
                r.children
                    .iter()
                    .filter(|c| matches!(c, Node::Element(_)))
                    .count()
            })
            .max()
            .unwrap_or(1)
            .max(1);
        let cell_width = width / columns as f32 - em * 1.6;
        let mut out = Vec::new();
        for row in found {
            let mut cells = Vec::new();
            for cell in &row.children {
                let Node::Element(cell) = cell else { continue };
                let head = cell.tag == "th";
                let body = self.html_blocks(&cell.children, cell_width, hcx, window, cx);
                cells.push(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(em * 0.8))
                        .py(px(em * 0.4))
                        .border_1()
                        .border_color(theme::border())
                        .when(head, |d| {
                            d.bg(theme::code_background())
                                .font_weight(FontWeight(600.0))
                        })
                        .children(body),
                );
            }
            out.push(div().flex().children(cells));
        }
        div()
            .my(px(em * 0.5))
            .flex()
            .flex_col()
            .children(out)
            .into_any_element()
    }

    fn html_details(
        &mut self,
        e: &Element,
        width: f32,
        hcx: &mut Cx<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let index = hcx.details;
        hcx.details += 1;
        let key = hcx.key;
        let default_open = e.attr("open").is_some();
        let open = self.rich.details_open(key, index, default_open);
        let em = hcx.style.font_size;
        let summary = e.children.iter().find_map(|c| match c {
            Node::Element(s) if s.tag == "summary" => Some(s),
            _ => None,
        });
        let label = summary.map_or_else(
            || vec![cerata_markdown::preview::Inline::Text("Details".into())],
            |s| html::paragraph(&s.children),
        );
        let rich = rich_text(&label, &hcx.style.fonts, true);
        let marker = if open { "\u{25be}" } else { "\u{25b8}" };
        let summary_row = div()
            .id(SharedString::from(format!("html-summary-{key}-{index}")))
            .flex()
            .gap(px(6.0))
            .cursor_pointer()
            .font_weight(FontWeight(600.0))
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(cerata_markdown::preview::plain_text(
                &label,
            )))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, _, _, cx| {
                    cx.stop_propagation();
                    editor.rich.toggle_details(key, index, default_open);
                    cx.notify();
                }),
            )
            .child(div().text_color(theme::muted()).child(marker))
            .child(rich_text_element(rich, cx));
        #[cfg(test)]
        let summary_row = summary_row.debug_selector(move || format!("html-summary-{index}"));
        let mut details = div()
            .my(px(em * 0.5))
            .p(px(em * 0.5))
            .rounded(px(4.0))
            .border_1()
            .border_color(theme::border())
            .child(summary_row);
        if open {
            let rest: Vec<Node> = e
                .children
                .iter()
                .filter(|c| !matches!(c, Node::Element(s) if s.tag == "summary"))
                .cloned()
                .collect();
            let body = div().children(self.html_blocks(&rest, width - em - 2.0, hcx, window, cx));
            #[cfg(test)]
            let body = body.debug_selector(move || format!("html-details-body-{index}"));
            details = details.child(body);
        }
        details.into_any_element()
    }

    fn html_image(&mut self, e: &Element, width: f32, hcx: &mut Cx<'_>) -> AnyElement {
        let alt = e.attr("alt").unwrap_or("").to_owned();
        let index = hcx.images;
        hcx.images += 1;
        let Some(src) = e.attr("src").filter(|s| !s.is_empty()) else {
            return div()
                .text_color(theme::muted())
                .child(alt)
                .into_any_element();
        };
        let max = e
            .attr("width")
            .and_then(|w| w.trim_end_matches("px").parse::<f32>().ok())
            .map_or(width, |w| w.min(width));
        let (image_box, job) = self.images.image_box(src, max, hcx.row_height);
        hcx.jobs.extend(job);
        match image_box {
            ImageBox::Sized { width, height } => {
                let id = format!("html-image-{index}");
                self.images.element(src, width, height, id)
            }
            ImageBox::Unavailable => div()
                .text_color(theme::muted())
                .child(alt)
                .into_any_element(),
        }
    }
}

/// Parsed blocks by source, and which `<details>` the user opened or
/// closed.
#[derive(Default)]
pub struct HtmlBlocks {
    trees: std::collections::HashMap<u64, Rc<Vec<Node>>>,
    toggled: std::collections::HashMap<(u64, usize), bool>,
}

impl HtmlBlocks {
    pub fn tree(&mut self, source: &str, key: u64) -> Rc<Vec<Node>> {
        if self.trees.len() > 128 {
            self.trees.clear();
        }
        self.trees
            .entry(key)
            .or_insert_with(|| Rc::new(html::render(source)))
            .clone()
    }

    pub fn is_open(&self, key: u64, index: usize, default: bool) -> bool {
        self.toggled.get(&(key, index)).copied().unwrap_or(default)
    }

    pub fn toggle(&mut self, key: u64, index: usize, default: bool) {
        let open = self.is_open(key, index, default);
        self.toggled.insert((key, index), !open);
    }
}
