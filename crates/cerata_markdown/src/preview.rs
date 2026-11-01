//! Inline Markdown rendered for a widget that shows text without its
//! syntax: table cells (US-032) and the text of HTML blocks (US-035).
//! Ported from `table-decorations.ts:207-343` (`renderMarkdownNode`).

use crate::tree::{MarkdownTree, Node, NodeKind};

/// A rendered inline node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Inline {
    Text(String),
    Break,
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Code(Vec<Inline>),
    Strike(Vec<Inline>),
    /// A Markdown link, autolink or bare URL; `href` is its destination.
    Link {
        href: Option<String>,
        children: Vec<Inline>,
    },
    /// A wiki link: `target` is the raw text between the brackets.
    Wiki {
        target: String,
        children: Vec<Inline>,
    },
}

/// Renders `markdown` as inline content. HTML stays text.
pub fn render(markdown: &str) -> Vec<Inline> {
    let tree = MarkdownTree::parse(markdown);
    node(markdown, tree.root())
}

/// The plain text of rendered nodes, for measuring and tests.
pub fn plain_text(nodes: &[Inline]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Inline::Text(t) => out.push_str(t),
            Inline::Break => out.push('\n'),
            Inline::Strong(c)
            | Inline::Emphasis(c)
            | Inline::Code(c)
            | Inline::Strike(c)
            | Inline::Link { children: c, .. }
            | Inline::Wiki { children: c, .. } => out.push_str(&plain_text(c)),
        }
    }
    out
}

const HIDDEN: &[NodeKind] = &[
    NodeKind::CodeMark,
    NodeKind::EmphasisMark,
    NodeKind::EscapeMark,
    NodeKind::LinkMark,
    NodeKind::StrikethroughMark,
];
const LINK_HIDDEN: &[NodeKind] = &[
    NodeKind::CodeMark,
    NodeKind::EmphasisMark,
    NodeKind::EscapeMark,
    NodeKind::LinkMark,
    NodeKind::StrikethroughMark,
    NodeKind::LinkLabel,
    NodeKind::LinkTitle,
    NodeKind::Url,
];

fn push_text(out: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(text);
    } else {
        out.push(Inline::Text(text.to_owned()));
    }
}

fn push_all(out: &mut Vec<Inline>, next: Vec<Inline>) {
    for n in next {
        match n {
            Inline::Text(t) => push_text(out, &t),
            other => out.push(other),
        }
    }
}

fn children(text: &str, parent: Node<'_>, hidden: &[NodeKind]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut pos = parent.from();
    for child in parent.children() {
        if child.from() > pos {
            push_text(&mut out, &text[pos..child.from()]);
        }
        if !hidden.contains(&child.kind()) {
            push_all(&mut out, node(text, child));
        }
        pos = pos.max(child.to());
    }
    if parent.to() > pos {
        push_text(&mut out, &text[pos..parent.to()]);
    }
    out
}

fn node(text: &str, n: Node<'_>) -> Vec<Inline> {
    use NodeKind as K;
    let slice = &text[n.range()];
    match n.kind() {
        K::Document | K::Paragraph => children(text, n, HIDDEN),
        K::StrongEmphasis => vec![Inline::Strong(children(text, n, HIDDEN))],
        K::Emphasis => vec![Inline::Emphasis(children(text, n, HIDDEN))],
        K::Strikethrough => vec![Inline::Strike(children(text, n, HIDDEN))],
        K::InlineCode => vec![Inline::Code(children(text, n, HIDDEN))],
        K::Link | K::Autolink => {
            let href = n.child(K::Url).map(|u| text[u.range()].trim().to_owned());
            let mut kids = children(text, n, LINK_HIDDEN);
            if kids.is_empty()
                && let Some(href) = &href
            {
                push_text(&mut kids, href);
            }
            vec![Inline::Link {
                href,
                children: kids,
            }]
        }
        K::WikiLink => {
            let open = if slice.starts_with('!') { 3 } else { 2 };
            let target =
                text[n.from() + open..n.to().saturating_sub(2).max(n.from() + open)].to_owned();
            let display = crate::wiki::parse(&target).display;
            vec![Inline::Wiki {
                target,
                children: vec![Inline::Text(display)],
            }]
        }
        K::Url => vec![Inline::Link {
            href: Some(slice.to_owned()),
            children: vec![Inline::Text(slice.to_owned())],
        }],
        K::Image => {
            let alt = children(text, n, LINK_HIDDEN);
            if alt.is_empty() {
                vec![Inline::Text(slice.to_owned())]
            } else {
                alt
            }
        }
        K::Escape => vec![Inline::Text(slice.get(1..).unwrap_or("").to_owned())],
        K::Entity => vec![Inline::Text(decode_entity(slice))],
        K::HardBreak => vec![Inline::Break],
        K::Dash => vec![Inline::Text(
            match slice.len() {
                2 => "\u{2013}",
                3 => "\u{2014}",
                _ => slice,
            }
            .to_owned(),
        )],
        K::Emoji => {
            let name = slice.get(1..slice.len().saturating_sub(1)).unwrap_or("");
            vec![Inline::Text(
                crate::decorate::emoji(name).unwrap_or(slice).to_owned(),
            )]
        }
        _ if n.first_child().is_some() => children(text, n, HIDDEN),
        _ => vec![Inline::Text(slice.to_owned())],
    }
}

/// `decodeMarkdownEntity`: numeric references and the six names Writer
/// knows; anything else stays as written.
pub fn decode_entity(entity: &str) -> String {
    let from_code = |code: Option<u32>| {
        code.and_then(char::from_u32)
            .map_or_else(|| entity.to_owned(), String::from)
    };
    let inner = entity.strip_suffix(';').unwrap_or(entity);
    if let Some(hex) = inner
        .strip_prefix("&#x")
        .or_else(|| inner.strip_prefix("&#X"))
    {
        return from_code(u32::from_str_radix(hex, 16).ok());
    }
    if let Some(dec) = inner.strip_prefix("&#") {
        return from_code(dec.parse().ok());
    }
    match entity {
        "&amp;" => "&",
        "&apos;" => "'",
        "&gt;" => ">",
        "&lt;" => "<",
        "&nbsp;" => "\u{a0}",
        "&quot;" => "\"",
        _ => entity,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::Inline::*;
    use super::*;

    fn t(s: &str) -> Inline {
        Text(s.to_owned())
    }

    /// `table-decorations.test.ts:67-95`.
    #[test]
    fn renders_inline_markdown_for_cells() {
        assert_eq!(
            render("**Bold** _em_ `code` [link](https://example.test) ~~gone~~ &amp; &lt; \\|"),
            vec![
                Strong(vec![t("Bold")]),
                t(" "),
                Emphasis(vec![t("em")]),
                t(" "),
                Code(vec![t("code")]),
                t(" "),
                Link {
                    href: Some("https://example.test".into()),
                    children: vec![t("link")]
                },
                t(" "),
                Strike(vec![t("gone")]),
                t(" & < |"),
            ]
        );
    }

    /// `table-decorations.test.ts:97-125`.
    #[test]
    fn renders_wiki_links_with_escaped_aliases_and_inside_emphasis() {
        assert_eq!(
            render("[[Format your notes\\|Formatting]]"),
            vec![Wiki {
                target: "Format your notes\\|Formatting".into(),
                children: vec![t("Formatting")]
            }]
        );
        assert_eq!(
            render("**[[Roadmap]]**"),
            vec![Strong(vec![Wiki {
                target: "Roadmap".into(),
                children: vec![t("Roadmap")]
            }])]
        );
    }

    /// `table-decorations.test.ts:127-132`.
    #[test]
    fn keeps_inline_html_as_text() {
        assert_eq!(
            render("<img src=x onerror=alert(1)> **safe**"),
            vec![t("<img src=x onerror=alert(1)> "), Strong(vec![t("safe")])]
        );
    }

    #[test]
    fn renders_dashes_emoji_and_entities() {
        assert_eq!(
            plain_text(&render("a -- b --- c :tada: &#x41;&#66;")),
            "a \u{2013} b \u{2014} c 🎉 AB"
        );
    }
}
