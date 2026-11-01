//! HTML blocks (US-035): sanitized with `ammonia` on Writer's allow list,
//! then read into a small tree the editor draws. Ported from
//! `html-block-decorations.ts:12-164`.
//!
//! Writer's DOMPurify keeps the text of a tag it removes; so does
//! `ammonia`, except for `script` and `style`, whose content is dropped.
//! `iframe`, `object`, `embed` and every `on*` attribute are not on the
//! list. URLs keep `ammonia`'s scheme list (no `file:`, no `javascript:`).

use std::collections::HashSet;

use crate::preview::{self, Inline};

/// `html-block-decorations.ts:12-82`.
const ALLOWED_TAGS: &[&str] = &[
    "div",
    "p",
    "span",
    "br",
    "hr",
    "section",
    "article",
    "aside",
    "header",
    "footer",
    "nav",
    "main",
    "details",
    "summary",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "strong",
    "em",
    "b",
    "i",
    "u",
    "s",
    "del",
    "ins",
    "sub",
    "sup",
    "code",
    "pre",
    "blockquote",
    "kbd",
    "mark",
    "small",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "caption",
    "colgroup",
    "col",
    "img",
    "picture",
    "source",
    "video",
    "audio",
    "figure",
    "figcaption",
    "a",
    "ruby",
    "rt",
    "rp",
];

/// `html-block-decorations.ts:84-108`, without `rel` and `target`, which
/// Writer forces on links (`ammonia`'s `link_rel` here; links open through
/// the editor's rules, never in a frame).
const ALLOWED_ATTRIBUTES: &[&str] = &[
    "href", "src", "alt", "title", "width", "height", "align", "valign", "colspan", "rowspan",
    "id", "class", "style", "dir", "lang", "open", "controls", "autoplay", "loop", "muted",
    "poster",
];

/// Tags laid out as blocks; the others flow as text.
const BLOCK_TAGS: &[&str] = &[
    "div",
    "p",
    "section",
    "article",
    "aside",
    "header",
    "footer",
    "nav",
    "main",
    "details",
    "summary",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "caption",
    "colgroup",
    "col",
    "img",
    "picture",
    "source",
    "video",
    "audio",
    "figure",
    "figcaption",
    "hr",
];

const VOID_TAGS: &[&str] = &["br", "hr", "img", "source", "col", "wbr", "area", "input"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Node {
    pub fn is_block(&self) -> bool {
        matches!(self, Node::Element(e) if BLOCK_TAGS.contains(&e.tag.as_str()))
    }
}

/// Whether Writer leaves this block as source (`/^<(?:script|style)[\s>]/i`).
pub fn is_script_or_style(source: &str) -> bool {
    let lower = source
        .trim_start()
        .get(..7)
        .unwrap_or("")
        .to_ascii_lowercase();
    ["<script", "<style"].iter().any(|open| {
        lower.starts_with(open)
            && source
                .trim_start()
                .as_bytes()
                .get(open.len())
                .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>')
    })
}

/// The HTML with only Writer's tags and attributes left.
pub fn sanitize(source: &str) -> String {
    ammonia::Builder::default()
        .tags(ALLOWED_TAGS.iter().copied().collect::<HashSet<_>>())
        .tag_attributes(Default::default())
        .generic_attributes(ALLOWED_ATTRIBUTES.iter().copied().collect::<HashSet<_>>())
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(source)
        .to_string()
}

/// The sanitized HTML of `source` as a tree.
pub fn render(source: &str) -> Vec<Node> {
    parse(&sanitize(source))
}

fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        match tail.find(';').filter(|&semi| semi <= 10) {
            Some(semi) => {
                out.push_str(&preview::decode_entity(&tail[..=semi]));
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads serialized HTML (as `ammonia` writes it: quoted attributes,
/// escaped text, void tags unclosed) into a tree. Unbalanced closing tags
/// close up to their match, or are ignored.
pub fn parse(html: &str) -> Vec<Node> {
    let mut stack: Vec<Element> = vec![Element {
        tag: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
    }];
    let push = |stack: &mut Vec<Element>, node: Node| {
        if let Some(top) = stack.last_mut() {
            top.children.push(node);
        }
    };
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push(&mut stack, Node::Text(decode(rest)));
            break;
        };
        if lt > 0 {
            push(&mut stack, Node::Text(decode(&rest[..lt])));
        }
        let tail = &rest[lt..];
        let Some(gt) = tail.find('>') else {
            push(&mut stack, Node::Text(decode(tail)));
            break;
        };
        let inner = &tail[1..gt];
        rest = &tail[gt + 1..];
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            if let Some(depth) = stack.iter().rposition(|e| e.tag == name).filter(|&d| d > 0) {
                while stack.len() > depth {
                    let Some(done) = stack.pop() else { break };
                    push(&mut stack, Node::Element(done));
                }
            }
            continue;
        }
        let inner = inner.trim_end_matches('/');
        let name_end = inner
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(inner.len());
        let tag = inner[..name_end].to_ascii_lowercase();
        let attrs = attributes(&inner[name_end..]);
        let element = Element {
            tag,
            attrs,
            children: Vec::new(),
        };
        if VOID_TAGS.contains(&element.tag.as_str()) {
            push(&mut stack, Node::Element(element));
        } else {
            stack.push(element);
        }
    }
    while stack.len() > 1 {
        let Some(done) = stack.pop() else { break };
        push(&mut stack, Node::Element(done));
    }
    stack.pop().map(|root| root.children).unwrap_or_default()
}

fn attributes(text: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let name_end = rest
            .find(|c: char| c == '=' || c.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let value = match rest.strip_prefix('=') {
            Some(after) => {
                let after = after.trim_start();
                match after.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let body = &after[1..];
                        let end = body.find(q).unwrap_or(body.len());
                        rest = body.get(end + 1..).unwrap_or("");
                        decode(&body[..end])
                    }
                    _ => {
                        let end = after
                            .find(|c: char| c.is_ascii_whitespace())
                            .unwrap_or(after.len());
                        rest = &after[end..];
                        decode(&after[..end])
                    }
                }
            }
            None => String::new(),
        };
        if !name.is_empty() {
            attrs.push((name, value));
        }
        rest = rest.trim_start();
    }
    attrs
}

fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            space = true;
        } else {
            if space {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    if space {
        out.push(' ');
    }
    out
}

/// A text node: white space collapsed, inline Markdown read
/// (`convertInlineMarkdown`), its edge spaces kept.
fn text_inline(text: &str, out: &mut Vec<Inline>) {
    let collapsed = collapse(text);
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        if !collapsed.is_empty() {
            out.push(Inline::Text(" ".into()));
        }
        return;
    }
    if collapsed.starts_with(' ') {
        out.push(Inline::Text(" ".into()));
    }
    out.extend(preview::render(trimmed));
    if collapsed.ends_with(' ') {
        out.push(Inline::Text(" ".into()));
    }
}

fn verbatim(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) if e.tag == "br" => out.push('\n'),
            Node::Element(e) => verbatim(&e.children, out),
        }
    }
}

/// The text of `nodes` as written, line breaks included (`pre`).
pub fn plain(nodes: &[Node]) -> String {
    let mut out = String::new();
    verbatim(nodes, &mut out);
    out
}

/// Inline content: tags Cerata draws map to their style, the others to
/// their text.
pub fn inline(nodes: &[Node]) -> Vec<Inline> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            Node::Text(t) => text_inline(t, &mut out),
            Node::Element(e) => match e.tag.as_str() {
                "br" => out.push(Inline::Break),
                "strong" | "b" => out.push(Inline::Strong(inline(&e.children))),
                "em" | "i" => out.push(Inline::Emphasis(inline(&e.children))),
                "s" | "del" => out.push(Inline::Strike(inline(&e.children))),
                "code" | "kbd" => out.push(Inline::Code(vec![Inline::Text(plain(&e.children))])),
                "a" => out.push(Inline::Link {
                    href: e.attr("href").map(str::to_owned),
                    children: inline(&e.children),
                }),
                "img" => {
                    if let Some(alt) = e.attr("alt").filter(|a| !a.is_empty()) {
                        out.push(Inline::Text(alt.to_owned()));
                    }
                }
                _ => out.extend(inline(&e.children)),
            },
        }
    }
    out
}

/// [`inline`] for a paragraph of its own: no space at its edges.
pub fn paragraph(nodes: &[Node]) -> Vec<Inline> {
    let mut out = inline(nodes);
    while matches!(out.first(), Some(Inline::Text(t)) if t.trim().is_empty()) {
        out.remove(0);
    }
    while matches!(out.last(), Some(Inline::Text(t)) if t.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Whether `nodes` hold anything visible.
pub fn is_empty(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        Node::Text(t) => t.trim().is_empty(),
        Node::Element(e) => !matches!(e.tag.as_str(), "img" | "hr") && is_empty(&e.children),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(nodes: &[Node], out: &mut Vec<String>) {
        for n in nodes {
            if let Node::Element(e) = n {
                out.push(e.tag.clone());
                tags(&e.children, out);
            }
        }
    }

    fn all_tags(source: &str) -> Vec<String> {
        let mut out = Vec::new();
        tags(&render(source), &mut out);
        out
    }

    #[test]
    fn dangerous_tags_never_render_and_event_attributes_are_removed() {
        let clean = sanitize(
            "<div onclick=\"x()\" onmouseover='y'>hi<script>alert(1)</script><style>p{}</style>\
             <iframe src=\"https://a.test\"></iframe><object data=\"x\"></object><embed src=\"x\"></div>",
        );
        for banned in [
            "onclick",
            "onmouseover",
            "<script",
            "alert",
            "<style",
            "p{}",
            "<iframe",
            "<object",
            "<embed",
        ] {
            assert!(!clean.contains(banned), "{banned} in {clean}");
        }
        assert_eq!(all_tags("<div onclick=\"x()\">hi</div>"), ["div"]);
    }

    #[test]
    fn javascript_and_file_urls_are_dropped() {
        let nodes = render("<a href=\"javascript:alert(1)\">x</a><img src=\"file:///etc/passwd\">");
        let Node::Element(a) = &nodes[0] else {
            panic!()
        };
        assert_eq!(a.attr("href"), None);
        let Node::Element(img) = &nodes[1] else {
            panic!()
        };
        assert_eq!(img.attr("src"), None);
        let nodes = render("<a href=\"https://a.test/?q=1&amp;r=2\" target=\"_top\">x</a>");
        let Node::Element(a) = &nodes[0] else {
            panic!()
        };
        assert_eq!(a.attr("href"), Some("https://a.test/?q=1&r=2"));
        assert_eq!(a.attr("target"), None);
    }

    #[test]
    fn an_unsupported_tag_keeps_its_text() {
        assert_eq!(all_tags("<p><marquee>hello</marquee></p>"), ["p"]);
        assert_eq!(
            inline(&render("<marquee>hello</marquee>")),
            vec![Inline::Text("hello".into())]
        );
    }

    #[test]
    fn details_keep_their_summary_and_open_state() {
        let nodes = render("<details open><summary>More</summary><p>Body</p></details>");
        let Node::Element(details) = &nodes[0] else {
            panic!()
        };
        assert_eq!(details.tag, "details");
        assert!(details.attr("open").is_some());
        assert!(matches!(&details.children[0], Node::Element(s) if s.tag == "summary"));
    }

    #[test]
    fn inline_markdown_and_entities_are_read_in_text() {
        let nodes = render("<p>Use **bold** &amp; `code` <b>here</b></p>");
        let Node::Element(p) = &nodes[0] else {
            panic!()
        };
        let inl = inline(&p.children);
        assert!(inl.contains(&Inline::Strong(vec![Inline::Text("bold".into())])));
        assert_eq!(preview::plain_text(&inl), "Use bold & code here");
    }

    #[test]
    fn script_and_style_blocks_stay_source() {
        assert!(is_script_or_style("<script>x</script>"));
        assert!(is_script_or_style("  <STYLE type=x>"));
        assert!(!is_script_or_style("<scripts>"));
        assert!(!is_script_or_style("<div>"));
    }
}
