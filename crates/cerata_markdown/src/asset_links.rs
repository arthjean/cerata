//! Destinations of a note that point into a folder (EP-015): found and
//! rewritten by pure functions, so a rename, a move or a duplicate keeps a
//! note's images on screen without parsing its text twice nor touching
//! its code.
//!
//! A destination points into a folder when, once unescaped, percent-decoded
//! and normalized (`\` and `/` separators, `.` and `..` resolved), its path
//! starts with the folder's segments and goes on past them. Markdown links
//! and images (inline or by reference) are read on their `Url` node, wiki
//! links and embeds on their target, before `|` and `#`.

use std::ops::Range;

use crate::change::{Change, ChangeSet};
use crate::text::Text;
use crate::tree::{MarkdownTree, Node, NodeKind};

/// How a destination is written; its rewrite keeps the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkForm {
    /// `![a](foo-assets/x.png)`.
    Plain,
    /// `![a](<foo-assets/x.png>)`.
    Angle,
    /// A plain destination holding a `%XX` escape: `foo-assets/my%20x.png`.
    Percent,
    /// `![[foo-assets/x.png|300]]` or `[[foo-assets/x.png]]`.
    Wiki,
}

/// A destination of a note that points into the folder asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderLink {
    /// The whole destination (angle brackets included) or wiki target.
    pub range: Range<usize>,
    /// The bytes naming the folder, from the start of the path to the end of
    /// its last segment: what a rewrite replaces.
    pub prefix: Range<usize>,
    pub form: LinkForm,
}

/// Every destination of `text` that points into `folder`, a path relative
/// to the note (`foo-assets`, `../notes/foo-assets`), in document order.
pub fn links_into(text: &str, folder: &str) -> Vec<FolderLink> {
    links_into_tree(&MarkdownTree::parse(text), text, folder)
}

/// [`links_into`] with the tree the caller already has.
pub fn links_into_tree(tree: &MarkdownTree, text: &str, folder: &str) -> Vec<FolderLink> {
    let folder = normalized(folder.split(['/', '\\']).map(str::to_owned));
    if folder.iter().all(|s| s == "..") {
        return Vec::new();
    }
    let mut links = Vec::new();
    for node in tree.iter() {
        let found = match node.kind() {
            NodeKind::Url
                if node.parent().is_some_and(|p| {
                    matches!(
                        p.kind(),
                        NodeKind::Link | NodeKind::Image | NodeKind::LinkReference
                    )
                }) =>
            {
                markdown_link(text, node.range(), &folder)
            }
            NodeKind::WikiLink => wiki_link(text, node, &folder),
            _ => None,
        };
        if let Some(link) = found
            && !in_code(node)
        {
            links.push(link);
        }
    }
    links
}

/// The changes that replace the folder of each link by `new_folder`, a path
/// relative to the note, keeping the rest of the destination and its title.
pub fn rewrite(text: &str, links: &[FolderLink], new_folder: &str) -> Vec<Change> {
    let new_folder = new_folder.replace('\\', "/");
    let new_folder = new_folder.trim_end_matches('/');
    links
        .iter()
        .filter_map(|link| {
            let change = match link.form {
                LinkForm::Wiki => Change::new(link.prefix.clone(), new_folder),
                LinkForm::Angle => Change::new(link.prefix.clone(), angle_escape(new_folder)),
                LinkForm::Percent => Change::new(link.prefix.clone(), percent_encode(new_folder)),
                LinkForm::Plain => {
                    let written = markdown_destination(new_folder);
                    if written.starts_with('<') {
                        let rest = &text[link.prefix.end..link.range.end];
                        Change::new(
                            link.range.clone(),
                            format!("<{}{rest}>", angle_escape(new_folder)),
                        )
                    } else {
                        Change::new(link.prefix.clone(), written)
                    }
                }
            };
            (text.get(change.from..change.to) != Some(change.insert.as_str())).then_some(change)
        })
        .collect()
}

/// [`links_into`] then [`rewrite`]: the changes that move every destination
/// of `text` from `folder` to `new_folder`.
pub fn rewrite_folder(text: &str, folder: &str, new_folder: &str) -> Vec<Change> {
    rewrite(text, &links_into(text, folder), new_folder)
}

/// `text` with every destination into `folder` moved to `new_folder` (a
/// closed note rewritten on disk, a duplicate before it is written);
/// `None` when nothing changes.
pub fn rewrite_text(text: &str, folder: &str, new_folder: &str) -> Option<String> {
    let changes = rewrite_folder(text, folder, new_folder);
    if changes.is_empty() {
        return None;
    }
    let mut doc = Text::new(text);
    ChangeSet::new(&doc, changes).ok()?.apply(&mut doc);
    Some(doc.as_str().to_owned())
}

/// Writer's `formatMarkdownDestination`: angle brackets around a
/// destination with spaces or parentheses. `%` is escaped, since a local
/// destination is percent-decoded when it is resolved.
pub fn markdown_destination(path: &str) -> String {
    let path = path.replace('%', "%25");
    if path.contains([' ', '\t', '(', ')', '<', '>']) {
        format!("<{}>", path.replace('<', "%3C").replace('>', "%3E"))
    } else {
        path
    }
}

fn angle_escape(path: &str) -> String {
    path.replace('%', "%25")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '\t' => out.push_str("%09"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            c => out.push(c),
        }
    }
    out
}

/// Inside code, the frontmatter or raw HTML, where nothing is a link.
fn in_code(node: Node<'_>) -> bool {
    node.ancestors().any(|a| {
        matches!(
            a.kind(),
            NodeKind::InlineCode
                | NodeKind::FencedCode
                | NodeKind::CodeBlock
                | NodeKind::Frontmatter
                | NodeKind::HtmlBlock
                | NodeKind::CommentBlock
                | NodeKind::ProcessingInstructionBlock
        )
    })
}

/// `.` and empty segments dropped, `..` resolved; a leading `..` stays.
fn normalized(segments: impl Iterator<Item = String>) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    for segment in segments {
        push(&mut stack, segment);
    }
    stack
}

fn push(stack: &mut Vec<String>, segment: String) {
    match segment.as_str() {
        "" | "." => {}
        ".." if stack.last().is_some_and(|s| s != "..") => {
            stack.pop();
        }
        _ => stack.push(segment),
    }
}

/// A path's segments, each with its bytes (relative to the path) and its
/// logical text.
type Segments = Vec<(Range<usize>, String)>;

/// The end of the folder's prefix in `segments`: the end of the segment
/// after which the normalized path equals `folder` and keeps it as a prefix
/// to a longer path. `None` when the path does not point into it.
fn prefix_end(segments: &Segments, folder: &[String]) -> Option<usize> {
    let mut stack: Vec<String> = Vec::new();
    let mut end = None;
    for (range, segment) in segments {
        push(&mut stack, segment.clone());
        if stack.len() < folder.len() || stack[..folder.len()] != *folder {
            end = None;
        } else if stack.len() == folder.len() {
            end = Some(range.end);
        }
    }
    (stack.len() > folder.len()).then_some(end).flatten()
}

/// A Markdown destination: `\` before ASCII punctuation escapes it (`\/`
/// stays a separator), any other `\` separates, as on Windows; each segment
/// is percent-decoded.
fn markdown_segments(path: &str) -> Segments {
    let bytes = path.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut logical = Vec::new();
    let mut i = 0;
    let close = |segments: &mut Segments, logical: &mut Vec<u8>, start: usize, end: usize| {
        segments.push((start..end, percent_decode(logical)));
        logical.clear();
    };
    while i < bytes.len() {
        match bytes[i] {
            b'/' => {
                close(&mut segments, &mut logical, start, i);
                i += 1;
                start = i;
            }
            b'\\' => match bytes.get(i + 1) {
                Some(b'/') => {
                    close(&mut segments, &mut logical, start, i);
                    i += 2;
                    start = i;
                }
                Some(next) if next.is_ascii_punctuation() => {
                    logical.push(*next);
                    i += 2;
                }
                _ => {
                    close(&mut segments, &mut logical, start, i);
                    i += 1;
                    start = i;
                }
            },
            b => {
                logical.push(b);
                i += 1;
            }
        }
    }
    close(&mut segments, &mut logical, start, bytes.len());
    segments
}

/// `%XX` escapes decoded; the bytes as written when one is malformed or the
/// result is not UTF-8 (`decodeURIComponent` keeping its input).
fn percent_decode(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes).into_owned();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            match raw
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => return raw,
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or(raw)
}

fn has_percent_escape(path: &str) -> bool {
    let bytes = path.as_bytes();
    (0..bytes.len()).any(|i| {
        bytes[i] == b'%'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
    })
}

/// A local path: no root, no drive, no scheme.
fn is_relative(first: &str, path: &str) -> bool {
    !path.starts_with(['/', '\\']) && !first.contains(':')
}

fn markdown_link(text: &str, url: Range<usize>, folder: &[String]) -> Option<FolderLink> {
    let raw = text.get(url.clone())?;
    let angle = raw.len() >= 2 && raw.starts_with('<') && raw.ends_with('>');
    let inner = if angle {
        url.start + 1..url.end - 1
    } else {
        url.clone()
    };
    let path = text.get(inner.clone())?;
    let segments = markdown_segments(path);
    if !is_relative(segments.first().map_or("", |s| s.1.as_str()), path) {
        return None;
    }
    let end = prefix_end(&segments, folder)?;
    let form = if angle {
        LinkForm::Angle
    } else if has_percent_escape(path) {
        LinkForm::Percent
    } else {
        LinkForm::Plain
    };
    Some(FolderLink {
        range: url,
        prefix: inner.start..inner.start + end,
        form,
    })
}

fn wiki_link(text: &str, node: Node<'_>, folder: &[String]) -> Option<FolderLink> {
    let open = if text.get(node.range())?.starts_with('!') {
        3
    } else {
        2
    };
    let inner = node.from() + open..node.to().checked_sub(2)?;
    let raw = text.get(inner.clone())?;
    let target_end = match raw.find('|') {
        Some(i) if i > 0 && raw.as_bytes()[i - 1] == b'\\' => i - 1,
        Some(i) => i,
        None => raw.len(),
    };
    let target_end = raw[..target_end].find('#').unwrap_or(target_end);
    let lead = raw.len() - raw.trim_start().len();
    let start = inner.start + lead.min(target_end);
    let target = text.get(start..inner.start + target_end)?.trim_end();
    let mut segments = Vec::new();
    let mut from = 0;
    for part in target.split(['/', '\\']) {
        segments.push((from..from + part.len(), part.to_owned()));
        from += part.len() + 1;
    }
    if !is_relative(segments.first().map_or("", |s| s.1.as_str()), target) {
        return None;
    }
    let end = prefix_end(&segments, folder)?;
    Some(FolderLink {
        range: start..start + target.len(),
        prefix: start..start + end,
        form: LinkForm::Wiki,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewritten(text: &str, folder: &str, new_folder: &str) -> String {
        rewrite_text(text, folder, new_folder).unwrap_or_else(|| text.to_owned())
    }

    fn found<'a>(text: &'a str, folder: &str) -> Vec<&'a str> {
        links_into(text, folder)
            .into_iter()
            .map(|l| &text[l.range])
            .collect()
    }

    #[test]
    fn finds_every_form_of_destination_into_the_folder() {
        let text = "![a](foo-assets/a.png) [b](<foo-assets/b c.png>) ![[foo-assets/c.png|300]]\n\
                    [[foo-assets/d.pdf#p|Doc]] ![e](./foo-assets/e.png) ![f](foo-assets\\f.png)\n\
                    ![g](other/g.png) ![h](https://x.org/foo-assets/h.png) ![i](/foo-assets/i.png)\n\
                    \n[ref]: foo-assets/r.png \"Title\"\n";
        assert_eq!(
            found(text, "foo-assets"),
            [
                "foo-assets/a.png",
                "<foo-assets/b c.png>",
                "foo-assets/c.png",
                "foo-assets/d.pdf",
                "./foo-assets/e.png",
                "foo-assets\\f.png",
                "foo-assets/r.png",
            ]
        );
    }

    #[test]
    fn compares_whole_segments_after_decoding_and_normalizing() {
        assert!(found("![x](foo-assets-old/x.png)", "foo-assets").is_empty());
        assert!(found("![x](foo-assetsx.png)", "foo-assets").is_empty());
        assert!(
            found("![x](foo-assets/)", "foo-assets").is_empty(),
            "the folder itself"
        );
        assert!(found("![x](foo-assets/../x.png)", "foo-assets").is_empty());
        assert_eq!(found("![x](foo%2Dassets/x.png)", "foo-assets").len(), 1);
        assert_eq!(found("![x](foo\\-assets/x.png)", "foo-assets").len(), 1);
        assert_eq!(
            found("![x](sub/../foo-assets/x.png)", "foo-assets").len(),
            1
        );
        assert_eq!(
            found("![x](../a/foo-assets/x.png)", "../a/foo-assets").len(),
            1
        );
        assert_eq!(
            found("![x](my%20notes-assets/x.png)", "my notes-assets").len(),
            1
        );
    }

    #[test]
    fn ignores_code_frontmatter_and_html() {
        let text = "---\nimage: \"[[foo-assets/f.png]]\"\n---\n\n`![a](foo-assets/a.png)` \
                    `[[foo-assets/b.png]]`\n\n```\n![c](foo-assets/c.png)\n[[foo-assets/c.png]]\n```\n\n    \
                    ![d](foo-assets/d.png)\n\n<div>\n![e](foo-assets/e.png) [[foo-assets/e.png]]\n</div>\n\n\
                    ![kept](foo-assets/k.png)\n";
        assert_eq!(found(text, "foo-assets"), ["foo-assets/k.png"]);
    }

    #[test]
    fn rewrites_the_prefix_and_keeps_the_rest() {
        let cases = [
            ("![a](foo-assets/x.png)", "![a](bar-assets/x.png)"),
            (
                "![a](foo-assets/x.png \"T\")",
                "![a](bar-assets/x.png \"T\")",
            ),
            ("![a](<foo-assets/x y.png>)", "![a](<bar-assets/x y.png>)"),
            ("![a](./foo-assets/x.png)", "![a](bar-assets/x.png)"),
            ("![a](foo-assets\\x.png)", "![a](bar-assets\\x.png)"),
            (
                "![a|300](foo-assets/x.png#f)",
                "![a|300](bar-assets/x.png#f)",
            ),
            ("![[foo-assets/x.png|300]]", "![[bar-assets/x.png|300]]"),
            ("[[foo-assets/x.pdf#p|Doc]]", "[[bar-assets/x.pdf#p|Doc]]"),
            ("[r]: foo-assets/x.png", "[r]: bar-assets/x.png"),
        ];
        for (before, after) in cases {
            assert_eq!(
                rewritten(before, "foo-assets", "bar-assets"),
                after,
                "{before}"
            );
        }
    }

    #[test]
    fn a_new_path_with_spaces_keeps_the_form_or_takes_brackets() {
        let to = "my notes-assets";
        assert_eq!(
            rewritten("![a](foo-assets/x.png \"T\")", "foo-assets", to),
            "![a](<my notes-assets/x.png> \"T\")",
            "a plain destination takes brackets"
        );
        assert_eq!(
            rewritten("![a](<foo-assets/x.png>)", "foo-assets", to),
            "![a](<my notes-assets/x.png>)"
        );
        assert_eq!(
            rewritten("![a](foo-assets/my%20x.png)", "foo-assets", to),
            "![a](my%20notes-assets/my%20x.png)",
            "a percent-encoded destination stays encoded"
        );
        assert_eq!(
            rewritten("![[foo-assets/x.png|300]]", "foo-assets", to),
            "![[my notes-assets/x.png|300]]"
        );
        assert_eq!(
            rewritten("![a](foo-assets/x.png)", "foo-assets", "100%-assets"),
            "![a](100%25-assets/x.png)"
        );
    }

    #[test]
    fn rewrites_to_a_folder_elsewhere_and_leaves_the_rest_alone() {
        let text =
            "# Note\n\n![a](foo-assets/x.png) and `foo-assets/y.png` [o](foo-assets-old/z.png)\n";
        assert_eq!(
            rewritten(text, "foo-assets", "../old/foo-assets"),
            "# Note\n\n![a](../old/foo-assets/x.png) and `foo-assets/y.png` [o](foo-assets-old/z.png)\n"
        );
        assert!(rewrite_folder(text, "foo-assets", "foo-assets").is_empty());
        assert!(rewrite_folder("no links", "foo-assets", "bar-assets").is_empty());
    }

    #[test]
    fn writes_destinations_as_writer_does() {
        assert_eq!(
            markdown_destination("Writer TODOs-assets/image.png"),
            "<Writer TODOs-assets/image.png>"
        );
        assert_eq!(
            markdown_destination("note-assets/image.png"),
            "note-assets/image.png"
        );
    }
}
