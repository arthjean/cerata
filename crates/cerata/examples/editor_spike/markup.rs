//! Hand-written line scanner for the three constructs the spike hides (ATX
//! heading marks, `**strong**`, `[links](url)`) plus block images, and the
//! pure decoration decision that hides or reveals their markup.
//!
//! The real parser is chosen by US-003; only the decision logic here mirrors
//! Writer: `apps/desktop/src/lib/prosemark-core/hide/core.ts:35-124` (a node
//! is revealed when any selection range touches it, `rangeTouchesRange` in
//! `prosemark-core/utils.ts:126`, inclusive on both ends) and the specs in
//! `hide/index.ts:22-80`.

use std::ops::Range;

/// Writer's heading scales, H1 first (`syntaxHighlighting.ts:34`).
pub const HEADING_SCALES: [f32; 6] = [1.6, 1.4, 1.2, 1.1, 1.0, 0.9];

#[derive(Debug, Clone, PartialEq)]
pub enum LineKind {
    Paragraph,
    Heading {
        level: u8,
    },
    /// A line that is exactly one image, rendered as a block widget.
    ImageBlock {
        url: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    HeadingMark,
    Strong,
    Link,
    Image,
}

/// A syntax node, in line-relative byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    /// The extent the selection must touch to reveal the node.
    pub range: Range<usize>,
    /// The markup hidden while the node is not touched.
    pub marks: Vec<Range<usize>>,
    /// The styled content (strong text, link text).
    pub content: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineSyntax {
    pub kind: LineKind,
    pub nodes: Vec<Node>,
}

pub fn scan_line(text: &str) -> LineSyntax {
    if let Some(level) = heading_level(text) {
        let mark_end = (level as usize + 1).min(text.len());
        let mut nodes = vec![Node {
            kind: NodeKind::HeadingMark,
            range: 0..text.len(),
            marks: vec![0..mark_end],
            content: mark_end..text.len(),
        }];
        nodes.extend(scan_inline(text, mark_end));
        return LineSyntax {
            kind: LineKind::Heading { level },
            nodes,
        };
    }
    if let Some((alt, url)) = block_image(text) {
        return LineSyntax {
            kind: LineKind::ImageBlock {
                url: text[url.clone()].to_string(),
            },
            nodes: vec![Node {
                kind: NodeKind::Image,
                range: 0..text.len(),
                marks: vec![0..text.len()],
                content: alt,
            }],
        };
    }
    LineSyntax {
        kind: LineKind::Paragraph,
        nodes: scan_inline(text, 0),
    }
}

fn heading_level(text: &str) -> Option<u8> {
    let hashes = text.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    match text.as_bytes().get(hashes) {
        None | Some(b' ') => Some(hashes as u8),
        _ => None,
    }
}

/// `![alt](url)` covering the whole line: (alt range, url range).
fn block_image(text: &str) -> Option<(Range<usize>, Range<usize>)> {
    let rest = text.strip_prefix("![")?;
    let alt_end = 2 + rest.find(']')?;
    let after = &text[alt_end..];
    let url_rest = after.strip_prefix("](")?;
    let url_start = alt_end + 2;
    let url_end = url_start + url_rest.find(')')?;
    if url_end + 1 != text.len() || url_end == url_start {
        return None;
    }
    Some((2..alt_end, url_start..url_end))
}

fn scan_inline(text: &str, from: usize) -> Vec<Node> {
    let bytes = text.as_bytes();
    let mut nodes = Vec::new();
    let mut ix = from;
    while ix < bytes.len() {
        if bytes[ix] == b'*'
            && bytes.get(ix + 1) == Some(&b'*')
            && let Some(node) = strong_at(text, ix)
        {
            ix = node.range.end;
            nodes.push(node);
            continue;
        }
        if bytes[ix] == b'['
            && (ix == 0 || bytes[ix - 1] != b'!')
            && let Some(node) = link_at(text, ix)
        {
            ix = node.range.end;
            nodes.push(node);
            continue;
        }
        ix += 1;
    }
    nodes
}

fn strong_at(text: &str, start: usize) -> Option<Node> {
    let content_start = start + 2;
    let close = content_start + text.get(content_start..)?.find("**")?;
    let content = &text[content_start..close];
    if content.is_empty()
        || content.starts_with(char::is_whitespace)
        || content.ends_with(char::is_whitespace)
    {
        return None;
    }
    Some(Node {
        kind: NodeKind::Strong,
        range: start..close + 2,
        marks: vec![start..content_start, close..close + 2],
        content: content_start..close,
    })
}

fn link_at(text: &str, start: usize) -> Option<Node> {
    let text_start = start + 1;
    let text_end = text_start + text[text_start..].find(']')?;
    let rest = &text[text_end..];
    let url_rest = rest.strip_prefix("](")?;
    let url_start = text_end + 2;
    let url_len = url_rest.find(')')?;
    let url = &url_rest[..url_len];
    if text_end == text_start || url.is_empty() || url.contains(char::is_whitespace) {
        return None;
    }
    let end = url_start + url_len + 1;
    Some(Node {
        kind: NodeKind::Link,
        range: start..end,
        marks: vec![start..text_start, text_end..end],
        content: text_start..text_end,
    })
}

/// Writer's `rangeTouchesRange`: inclusive on both ends, so a caret right
/// after a node still reveals it.
pub fn touches(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start <= b.end && b.start <= a.end
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMode {
    /// The source line is replaced by the widget.
    Replace,
    /// The source is revealed and the widget is drawn below it.
    Below,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDecorations {
    /// Hidden byte ranges, line-relative, sorted and disjoint.
    pub hidden: Vec<Range<usize>>,
    pub block: Option<BlockMode>,
}

/// The pure decoration decision for one line. `selections` are absolute
/// buffer ranges; `line_start` converts the line's relative node ranges.
pub fn decorate_line(
    syntax: &LineSyntax,
    line_start: usize,
    selections: &[Range<usize>],
) -> LineDecorations {
    let mut hidden = Vec::new();
    let mut block = None;
    for node in &syntax.nodes {
        let absolute = line_start + node.range.start..line_start + node.range.end;
        let touched = selections.iter().any(|sel| touches(&absolute, sel));
        if node.kind == NodeKind::Image {
            block = Some(if touched {
                BlockMode::Below
            } else {
                BlockMode::Replace
            });
            continue;
        }
        if !touched {
            hidden.extend(node.marks.iter().filter(|m| !m.is_empty()).cloned());
        }
    }
    hidden.sort_by_key(|range| range.start);
    LineDecorations { hidden, block }
}

/// Vertical metrics of a line, following Writer's spacing scale
/// (`prosemark-theme.css:155-187` and `:259-271`) at a given body size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    pub font_size: f32,
    pub row_height: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
    pub bold: bool,
}

/// Writer's default `--writer-editor-line-height`.
pub const BODY_LINE_HEIGHT: f32 = 1.875;
const GLYPH: f32 = 1.125;
const HEADING_GLYPH: f32 = 1.17;
const HEADING_LINE_HEIGHT: f32 = 1.3;

pub fn line_metrics(kind: &LineKind, base: f32) -> LineMetrics {
    match kind {
        LineKind::Heading { level } => {
            let scale = HEADING_SCALES[(*level as usize).clamp(1, 6) - 1];
            let px = base / 16.0;
            let line_gap = (BODY_LINE_HEIGHT - GLYPH) * base;
            let leading = (HEADING_LINE_HEIGHT - HEADING_GLYPH) * scale * base / 2.0;
            LineMetrics {
                font_size: base * scale,
                row_height: HEADING_LINE_HEIGHT * base * scale,
                pad_top: (12.0 * px - leading).max(0.0),
                pad_bottom: (8.0 * px - line_gap / 2.0 - leading).max(0.0),
                bold: true,
            }
        }
        LineKind::Paragraph | LineKind::ImageBlock { .. } => LineMetrics {
            font_size: base,
            row_height: BODY_LINE_HEIGHT * base,
            pad_top: 0.0,
            pad_bottom: 0.0,
            bold: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "plain **bold** and [link](https://example.com/a) end";

    fn hidden_text(text: &str, decorations: &LineDecorations) -> String {
        let mut out = String::new();
        let mut ix = 0;
        for range in &decorations.hidden {
            out.push_str(&text[ix..range.start]);
            ix = range.end;
        }
        out.push_str(&text[ix..]);
        out
    }

    #[test]
    fn scanner_finds_the_three_constructs() {
        let syntax = scan_line(LINE);
        assert_eq!(syntax.kind, LineKind::Paragraph);
        let kinds: Vec<_> = syntax.nodes.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [NodeKind::Strong, NodeKind::Link]);
        assert_eq!(syntax.nodes[0].range, 6..14);
        assert_eq!(syntax.nodes[1].marks, vec![19..20, 24..48]);

        let heading = scan_line("### Title with **bold**");
        assert_eq!(heading.kind, LineKind::Heading { level: 3 });
        assert_eq!(heading.nodes[0].marks, vec![0..4]);
        assert_eq!(heading.nodes[1].kind, NodeKind::Strong);
        assert_eq!(scan_line("#nospace").kind, LineKind::Paragraph);
        assert_eq!(scan_line("####### seven").kind, LineKind::Paragraph);
        assert_eq!(scan_line("#").kind, LineKind::Heading { level: 1 });
        assert_eq!(scan_line("** not strong **").nodes, vec![]);

        let image = scan_line("![alt](images/a.svg)");
        assert_eq!(
            image.kind,
            LineKind::ImageBlock {
                url: "images/a.svg".into()
            }
        );
        assert_eq!(scan_line("text ![alt](a.svg)").kind, LineKind::Paragraph);
    }

    #[test]
    fn markup_is_hidden_when_the_selection_does_not_touch_the_node() {
        let syntax = scan_line(LINE);
        let far = 1000..1000;
        let decorations = decorate_line(&syntax, 0, &[far]);
        assert_eq!(hidden_text(LINE, &decorations), "plain bold and link end");
    }

    #[test]
    fn markup_is_revealed_as_soon_as_the_selection_touches_the_node() {
        let syntax = scan_line(LINE);
        // Caret right before `**`, at its inclusive edge.
        let at_open = decorate_line(&syntax, 0, &[6..6]);
        assert_eq!(
            hidden_text(LINE, &at_open),
            "plain **bold** and link end",
            "strong revealed, link still hidden"
        );
        // Caret right after the closing `)`.
        let after_link = decorate_line(&syntax, 0, &[48..48]);
        assert_eq!(
            hidden_text(LINE, &after_link),
            "plain bold and [link](https://example.com/a) end"
        );
        // One byte before the node: not touching.
        let before = decorate_line(&syntax, 0, &[5..5]);
        assert_eq!(hidden_text(LINE, &before), "plain bold and link end");
        // A range selection spanning both.
        let both = decorate_line(&syntax, 0, &[0..LINE.len()]);
        assert_eq!(hidden_text(LINE, &both), LINE);
    }

    #[test]
    fn line_offset_is_applied_to_absolute_selections() {
        let syntax = scan_line("## Heading");
        let start = 500;
        assert_eq!(
            decorate_line(&syntax, start, &[0..0]).hidden,
            vec![0..3],
            "selection on another line hides the mark"
        );
        assert_eq!(
            decorate_line(&syntax, start, &[start + 10..start + 10]).hidden,
            Vec::<Range<usize>>::new(),
            "caret at the end of the heading reveals it"
        );
    }

    #[test]
    fn image_blocks_switch_between_replace_and_below() {
        let syntax = scan_line("![alt](a.svg)");
        assert_eq!(
            decorate_line(&syntax, 10, &[0..0]).block,
            Some(BlockMode::Replace)
        );
        assert_eq!(
            decorate_line(&syntax, 10, &[23..23]).block,
            Some(BlockMode::Below)
        );
    }

    #[test]
    fn heading_metrics_follow_writer_spacing() {
        let body = line_metrics(&LineKind::Paragraph, 16.0);
        assert_eq!(body.row_height, 30.0);
        let h1 = line_metrics(&LineKind::Heading { level: 1 }, 16.0);
        assert!((h1.font_size - 25.6).abs() < 1e-4);
        assert!((h1.row_height - 33.28).abs() < 1e-4);
        assert!((h1.pad_top - (12.0 - 0.065 * 25.6)).abs() < 1e-4);
        let sizes: Vec<f32> = (1..=6)
            .map(|level| line_metrics(&LineKind::Heading { level }, 16.0).font_size / 16.0)
            .collect();
        for (size, scale) in sizes.iter().zip(HEADING_SCALES) {
            assert!((size - scale).abs() < 1e-6);
        }
    }
}
