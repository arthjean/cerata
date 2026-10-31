//! Probes: extract, from each parser's native output, the observation Writer's
//! tests make (node slices, top-level block names, values).

use pulldown_cmark::{Event, LinkType, Options, Tag};

use crate::{pd, ts::TsDoc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Frontmatter,
    Math,
    /// Inline links (Lezer `Link` with a destination).
    Link,
    /// Inline images.
    Image,
    /// Destination of inline links and images.
    Url,
    /// Title of inline links and images, quotes included.
    Title,
    /// Top-level block names, in Lezer vocabulary.
    Blocks,
    Escape,
    /// Dashes of length 2 or 3.
    Dash,
    Emoji,
    Wiki,
}

fn trim_eol(s: &str) -> String {
    s.trim_end_matches(['\n', '\r']).to_string()
}

fn heading_name(slice: &str, level: usize) -> String {
    let last = slice.trim_end().lines().last().unwrap_or("").trim();
    if !slice.trim_start().starts_with('#') && last.starts_with('=') {
        "SetextHeading1".to_string()
    } else if !slice.trim_start().starts_with('#') && last.starts_with('-') {
        "SetextHeading2".to_string()
    } else {
        format!("ATXHeading{level}")
    }
}

pub fn ts(doc: &TsDoc, probe: Probe) -> Vec<String> {
    let text = doc.text.as_str();
    let nodes = doc.nodes();
    let slices = |pred: &dyn Fn(&crate::ts::TsNode) -> bool| -> Vec<String> {
        nodes
            .iter()
            .filter(|n| pred(n))
            .map(|n| text[n.start..n.end].to_string())
            .collect()
    };
    let in_link =
        |n: &crate::ts::TsNode| n.parent_kind == "inline_link" || n.parent_kind == "image";
    match probe {
        Probe::Frontmatter => nodes
            .iter()
            .filter(|n| n.kind == "minus_metadata")
            .map(|n| trim_eol(&text[n.start..n.end]))
            .collect(),
        Probe::Math => slices(&|n| n.kind == "latex_block"),
        Probe::Link => slices(&|n| n.kind == "inline_link"),
        Probe::Image => slices(&|n| n.kind == "image"),
        Probe::Url => slices(&|n| n.kind == "link_destination" && in_link(n)),
        Probe::Title => slices(&|n| n.kind == "link_title" && in_link(n)),
        Probe::Blocks => nodes
            .iter()
            .filter(|n| {
                !n.inline
                    && n.named
                    && n.kind != "section"
                    && (n.parent_kind == "document" || n.parent_kind == "section")
            })
            .map(|n| {
                let slice = &text[n.start..n.end];
                match n.kind.as_str() {
                    "paragraph" => "Paragraph".to_string(),
                    "list" if slice.starts_with(['-', '*', '+']) => "BulletList".to_string(),
                    "list" => "OrderedList".to_string(),
                    "setext_heading" | "atx_heading" => {
                        heading_name(slice, slice.chars().take_while(|&c| c == '#').count())
                    }
                    "thematic_break" => "HorizontalRule".to_string(),
                    "minus_metadata" => "Frontmatter".to_string(),
                    other => other.to_string(),
                }
            })
            .collect(),
        Probe::Escape => slices(&|n| n.kind == "backslash_escape" && n.inline),
        // No node kind exists for these in the shipped grammar.
        Probe::Dash | Probe::Emoji => Vec::new(),
        Probe::Wiki => slices(&|n| n.kind == "wiki_link"),
    }
}

pub fn pd(text: &str, probe: Probe) -> Vec<String> {
    let slice = |r: &std::ops::Range<usize>| text[r.clone()].to_string();
    if probe == Probe::Dash {
        // Closest native feature: smart punctuation turns `--` and `---`
        // into en and em dashes.
        return pd::events(text, pd::options() | Options::ENABLE_SMART_PUNCTUATION)
            .iter()
            .filter(
                |(e, _)| matches!(e, Event::Text(t) if &**t == "\u{2013}" || &**t == "\u{2014}"),
            )
            .map(|(_, r)| slice(r))
            .collect();
    }
    let events = pd::events(text, pd::options());
    let mut out = Vec::new();
    let mut depth = 0usize;
    for (event, range) in &events {
        match event {
            Event::Start(tag) => {
                match (probe, tag) {
                    (Probe::Frontmatter, Tag::MetadataBlock(_)) => {
                        out.push(trim_eol(&slice(range)))
                    }
                    (
                        Probe::Link,
                        Tag::Link {
                            link_type: LinkType::Inline,
                            ..
                        },
                    )
                    | (
                        Probe::Image,
                        Tag::Image {
                            link_type: LinkType::Inline,
                            ..
                        },
                    ) => {
                        out.push(slice(range));
                    }
                    (
                        Probe::Url,
                        Tag::Link {
                            link_type: LinkType::Inline,
                            dest_url,
                            ..
                        }
                        | Tag::Image {
                            link_type: LinkType::Inline,
                            dest_url,
                            ..
                        },
                    ) => out.push(dest_url.to_string()),
                    (
                        Probe::Title,
                        Tag::Link {
                            link_type: LinkType::Inline,
                            title,
                            ..
                        }
                        | Tag::Image {
                            link_type: LinkType::Inline,
                            title,
                            ..
                        },
                    ) if !title.is_empty() => out.push(format!("\"{title}\"")),
                    (
                        Probe::Wiki,
                        Tag::Link {
                            link_type: LinkType::WikiLink { .. },
                            ..
                        }
                        | Tag::Image {
                            link_type: LinkType::WikiLink { .. },
                            ..
                        },
                    ) => out.push(slice(range)),
                    (Probe::Blocks, _) if depth == 0 => out.push(match tag {
                        Tag::Paragraph => "Paragraph".to_string(),
                        Tag::List(None) => "BulletList".to_string(),
                        Tag::List(Some(_)) => "OrderedList".to_string(),
                        Tag::Heading { level, .. } => heading_name(&slice(range), *level as usize),
                        Tag::MetadataBlock(_) => "Frontmatter".to_string(),
                        other => format!("{other:?}"),
                    }),
                    _ => {}
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Rule if probe == Probe::Blocks && depth == 0 => {
                out.push("HorizontalRule".to_string());
            }
            Event::InlineMath(_) | Event::DisplayMath(_) if probe == Probe::Math => {
                out.push(slice(range));
            }
            _ => {}
        }
    }
    // Escapes, emoji: pulldown-cmark emits no event for them (the escaping
    // backslash is simply absent from the Text ranges).
    out
}
