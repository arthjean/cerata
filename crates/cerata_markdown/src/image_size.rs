//! Image widths in Obsidian's syntax (ADR 0008): `![alt|300](x.png)`,
//! `![300](x.png)` and `![[x.png|300]]`, read and written by pure functions
//! that the renderer, the resize handles and the width commands share.
//!
//! Obsidian documents the forms in `Basic formatting syntax.md:216-225` and
//! `Embed files.md:46-62` (`obsidianmd/obsidian-help`). A height (`|WxH`)
//! is read and ignored; it is never written.

use std::ops::Range;

use crate::change::Change;
use crate::history::UserEvent;
use crate::selection::{Selection, SelectionRange};
use crate::state::{EditorState, Transaction};
use crate::text::Text;
use crate::tree::{MarkdownTree, Node, NodeKind};

/// The most digits a width or a height is written with.
const MAX_DIGITS: usize = 5;

/// A size written after an image's `|`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageSize {
    pub width: u32,
    /// Read, never applied nor written.
    pub height: Option<u32>,
}

/// `^[0-9]{1,5}(x[0-9]{1,5})?$` with a width of at least 1.
pub fn parse_size(s: &str) -> Option<ImageSize> {
    let digits =
        |d: &str| (1..=MAX_DIGITS).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit());
    let (width, height) = match s.split_once('x') {
        Some((w, h)) => (w, Some(h)),
        None => (s, None),
    };
    if !digits(width) || height.is_some_and(|h| !digits(h)) {
        return None;
    }
    let width: u32 = width.parse().ok()?;
    let height = match height {
        Some(h) => Some(h.parse().ok()?),
        None => None,
    };
    (width >= 1).then_some(ImageSize { width, height })
}

/// Where a label (an alt text, an embed's inner text) holds a size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizeSpan {
    /// The bytes the size takes in the label, its `|` (or `\|`) included.
    pub span: Range<usize>,
    /// The digits alone.
    pub digits: Range<usize>,
    pub size: ImageSize,
}

/// The last `|` of `s` not escaped by an odd run of backslashes.
fn last_unescaped_pipe(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    (0..bytes.len()).rev().find(|&i| {
        bytes[i] == b'|' && bytes[..i].iter().rev().take_while(|b| **b == b'\\').count() % 2 == 0
    })
}

/// The size in a Markdown image's alt text: after its last unescaped `|`,
/// else the whole alt when it has the form of a size (`![300](x.png)`).
pub fn alt_size(alt: &str) -> Option<SizeSpan> {
    if let Some(bar) = last_unescaped_pipe(alt)
        && let Some(size) = parse_size(&alt[bar + 1..])
    {
        return Some(SizeSpan {
            span: bar..alt.len(),
            digits: bar + 1..alt.len(),
            size,
        });
    }
    parse_size(alt).map(|size| SizeSpan {
        span: 0..alt.len(),
        digits: 0..alt.len(),
        size,
    })
}

/// The size in an embed's inner text (`x.png|300`), read on its last `|`
/// segment. A `\|` separates too, as in a table (`wiki::parse`).
pub fn embed_size(inner: &str) -> Option<SizeSpan> {
    let bar = inner.rfind('|').filter(|bar| *bar > 0)?;
    let size = parse_size(&inner[bar + 1..])?;
    let start = if inner.as_bytes()[bar - 1] == b'\\' {
        bar - 1
    } else {
        bar
    };
    Some(SizeSpan {
        span: start..inner.len(),
        digits: bar + 1..inner.len(),
        size,
    })
}

/// A Markdown image or an image embed, by its node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageNode {
    pub range: Range<usize>,
    /// The alt text of a Markdown image, the inner text of an embed.
    pub label: Range<usize>,
    pub embed: bool,
    pub size: Option<ImageSize>,
    /// Alone on its line: drawn as a block.
    pub block: bool,
}

impl ImageNode {
    pub fn width(&self) -> Option<u32> {
        self.size.map(|s| s.width)
    }

    /// The size's bytes in the document.
    fn size_span(&self, text: &Text) -> Option<SizeSpan> {
        let label = text.slice(self.label.clone());
        if self.embed {
            embed_size(label)
        } else {
            alt_size(label)
        }
    }

    /// Where the caret goes to edit the alt text: its end, before a size.
    pub fn alt_end(&self, text: &Text) -> usize {
        self.size_span(text)
            .map_or(self.label.end, |size| self.label.start + size.span.start)
    }
}

/// `node` as an image: a Markdown image, or an embed naming an image file.
pub fn image_node(text: &Text, node: Node<'_>) -> Option<ImageNode> {
    let range = node.range();
    let line = text.line_at(range.start);
    let block = range.start == line.from && range.end == line.to;
    match node.kind() {
        // An embed holds an `Image` node with no destination.
        NodeKind::Image if node.children().any(|c| c.kind() == NodeKind::Url) => {
            let mut marks = node.children().filter(|c| c.kind() == NodeKind::LinkMark);
            let open = marks.next()?;
            let close = marks.next()?;
            let label = open.to()..close.from();
            Some(ImageNode {
                size: alt_size(text.slice(label.clone())).map(|s| s.size),
                range,
                label,
                embed: false,
                block,
            })
        }
        NodeKind::WikiLink if text.slice(range.clone()).starts_with("![[") => {
            if range.end < range.start + 5 {
                return None;
            }
            let label = range.start + 3..range.end - 2;
            crate::wiki::image_embed_target(text.slice(label.clone()))?;
            Some(ImageNode {
                size: embed_size(text.slice(label.clone())).map(|s| s.size),
                range,
                label,
                embed: true,
                block,
            })
        }
        _ => None,
    }
}

/// The image whose node holds `pos`, bounds included.
pub fn image_at(tree: &MarkdownTree, text: &Text, pos: usize) -> Option<ImageNode> {
    tree.iter_range(pos..pos)
        .filter(|n| n.from() <= pos && pos <= n.to())
        .filter_map(|n| image_node(text, n))
        .last()
}

/// The change that sets an image's width to `width`, or removes its size
/// for `None`, leaving the rest of its alt text or embed alone; `None`
/// when the text already says so.
pub fn width_change(text: &Text, image: &ImageNode, width: Option<u32>) -> Option<Change> {
    let label = text.slice(image.label.clone());
    let base = image.label.start;
    let current = image.size_span(text);
    match (current, width) {
        (None, None) => None,
        (Some(current), None) => Some(Change::delete(
            base + current.span.start..base + current.span.end,
        )),
        (Some(current), Some(width)) => {
            if current.size
                == (ImageSize {
                    width,
                    height: None,
                })
            {
                return None;
            }
            Some(Change::new(
                base + current.digits.start..base + current.digits.end,
                width.to_string(),
            ))
        }
        (None, Some(width)) => {
            let insert = if !image.embed && label.is_empty() {
                width.to_string()
            } else if image.embed && label.contains("\\|") {
                format!("\\|{width}")
            } else {
                format!("|{width}")
            };
            Some(Change::insert(image.label.end, insert))
        }
    }
}

impl EditorState {
    /// Sets the width of the image whose node is `range` (US-063), in one
    /// undo step; a selection equal to the image (an object selection,
    /// ADR 0008) stays equal to it. `false` when nothing changed.
    pub fn set_image_width(&mut self, range: Range<usize>, width: Option<u32>) -> bool {
        if self.is_loading() {
            return false;
        }
        let Some(image) =
            image_at(self.tree(), self.text(), range.start).filter(|image| image.range == range)
        else {
            return false;
        };
        let Some(change) = width_change(self.text(), &image, width) else {
            return false;
        };
        let delta = change.insert.len() as isize - (change.to - change.from) as isize;
        let end = (range.end as isize + delta) as usize;
        let selection = self.selection();
        let ranges: Vec<SelectionRange> = selection
            .ranges()
            .iter()
            .map(|r| {
                let map = |pos: usize| {
                    if pos <= change.from {
                        pos
                    } else if pos >= change.to {
                        (pos as isize + delta) as usize
                    } else {
                        change.from
                    }
                };
                if r.from() == range.start && r.to() == range.end && !r.is_empty() {
                    if r.anchor < r.head {
                        SelectionRange::new(range.start, end)
                    } else {
                        SelectionRange::new(end, range.start)
                    }
                } else {
                    SelectionRange::new(map(r.anchor), map(r.head))
                }
            })
            .collect();
        let selection = Selection::new(ranges, selection.main_index());
        self.apply(
            Transaction::new()
                .change(change)
                .select(selection)
                .event(UserEvent::Command),
        )
        .is_ok()
    }
}

impl EditorState {
    /// Removes the Markdown of the image whose node is `range` (US-069),
    /// in one undo step; its file stays. The selection follows the text: an
    /// object selection becomes a caret where the image was, a caret
    /// elsewhere stays (a toolbar click, US-070). `false` when no image is
    /// there.
    pub fn delete_image(&mut self, range: Range<usize>) -> bool {
        if self.is_loading()
            || image_at(self.tree(), self.text(), range.start).is_none_or(|i| i.range != range)
        {
            return false;
        }
        self.apply(
            Transaction::new()
                .change(Change::new(range, ""))
                .event(UserEvent::Command),
        )
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: Option<u32>) -> Option<ImageSize> {
        Some(ImageSize { width, height })
    }

    #[test]
    fn sizes_have_one_to_five_digits_and_an_optional_height() {
        assert_eq!(parse_size("300"), size(300, None));
        assert_eq!(parse_size("100x50"), size(100, Some(50)));
        assert_eq!(parse_size("99999"), size(99_999, None));
        assert_eq!(parse_size("1x0"), size(1, Some(0)));
        for refused in [
            "", "0", "00", "300px", "50%", " 300", "300 ", "123456", "x50", "100x", "100x50x2",
            "1e3", "+30", "-3", "100X50",
        ] {
            assert_eq!(parse_size(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn an_alt_text_holds_a_size_after_its_last_unescaped_pipe_or_whole() {
        let read = |alt: &str| alt_size(alt).map(|s| (s.span, s.size.width, s.size.height));
        assert_eq!(read("alt|300"), Some((3..7, 300, None)));
        assert_eq!(read("alt|100x50"), Some((3..10, 100, Some(50))));
        assert_eq!(read("a|b|300"), Some((3..7, 300, None)));
        assert_eq!(read("|300"), Some((0..4, 300, None)));
        assert_eq!(read("300"), Some((0..3, 300, None)));
        assert_eq!(read("640x480"), Some((0..7, 640, Some(480))));
        // An escaped backslash before the pipe leaves it unescaped.
        assert_eq!(read("a\\\\|300"), Some((3..7, 300, None)));
        for refused in [
            "alt\\|300",
            "alt|300px",
            "alt|50%",
            "alt|0",
            "alt| 300",
            "alt|123456",
            "alt",
            "",
            "a|b",
        ] {
            assert_eq!(alt_size(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn an_embed_holds_its_size_on_its_last_segment() {
        let read = |inner: &str| embed_size(inner).map(|s| (s.span, s.digits, s.size.width));
        assert_eq!(read("x.png|300"), Some((5..9, 6..9, 300)));
        assert_eq!(read("x.png|alt|300"), Some((9..13, 10..13, 300)));
        assert_eq!(read("x.png\\|300"), Some((5..10, 7..10, 300)));
        assert_eq!(read("x.png|300x200").map(|r| r.2), Some(300));
        for refused in ["x.png", "x.png|alt", "x.png|300px", "|300", "x.png|0"] {
            assert_eq!(embed_size(refused), None, "{refused:?}");
        }
    }

    fn state(doc: &str) -> EditorState {
        EditorState::with_filters(doc, Vec::new())
    }

    fn first_image(doc: &str) -> (EditorState, ImageNode) {
        let state = state(doc);
        let pos = doc.find('!').unwrap_or(0);
        let image = image_at(state.tree(), state.text(), pos).expect("an image");
        (state, image)
    }

    fn written(doc: &str, width: Option<u32>) -> String {
        let (state, image) = first_image(doc);
        let mut out = doc.to_owned();
        if let Some(change) = width_change(state.text(), &image, width) {
            out.replace_range(change.from..change.to, &change.insert);
        }
        out
    }

    #[test]
    fn widths_are_set_replaced_and_removed_without_touching_the_rest() {
        for (doc, width, expected) in [
            ("![a](x)", Some(300), "![a|300](x)"),
            ("![a|100x50](x)", Some(300), "![a|300](x)"),
            ("![a|300](x)", None, "![a](x)"),
            ("![300](x)", None, "![](x)"),
            ("![a|b](x)", Some(300), "![a|b|300](x)"),
            ("![[x.png]]", Some(300), "![[x.png|300]]"),
            ("![[x.png|300]]", None, "![[x.png]]"),
            ("![[x.png|100x50]]", Some(250), "![[x.png|250]]"),
            ("![[x.png|alt]]", Some(250), "![[x.png|alt|250]]"),
            ("![](x)", Some(300), "![300](x)"),
            ("![300](x)", Some(120), "![120](x)"),
            ("![a\\|300](x)", Some(80), "![a\\|300|80](x)"),
            (
                "![a](<my x.png> \"t\")",
                Some(64),
                "![a|64](<my x.png> \"t\")",
            ),
        ] {
            assert_eq!(written(doc, width), expected, "{doc} with {width:?}");
        }
        // Nothing to write: the text already says so.
        let (state, image) = first_image("![a|300](x)");
        assert_eq!(width_change(state.text(), &image, Some(300)), None);
        let (state, image) = first_image("![a](x)");
        assert_eq!(width_change(state.text(), &image, None), None);
    }

    #[test]
    fn a_written_width_reads_back() {
        for doc in [
            "![a](x)",
            "![](x)",
            "![a|b](x)",
            "![a|10x5](x)",
            "![[x.png]]",
            "![[x.png|alt]]",
        ] {
            for width in [1, 64, 300, 99_999] {
                let out = written(doc, Some(width));
                let (_, image) = first_image(&out);
                assert_eq!(image.width(), Some(width), "{out}");
                assert_eq!(image.size.and_then(|s| s.height), None, "{out}");
                let back = written(&out, None);
                let (_, image) = first_image(&back);
                assert_eq!(image.size, None, "{back}");
            }
        }
    }

    #[test]
    fn images_are_found_with_their_label_and_whether_they_are_blocks() {
        let doc = "![wide|300](a.png)\n\nsee ![[b.png|40]] and ![[Note]]\n";
        let (state, image) = first_image(doc);
        assert_eq!(image.range, 0..18);
        assert_eq!(state.text().slice(image.label.clone()), "wide|300");
        assert_eq!(
            (image.width(), image.block, image.embed),
            (Some(300), true, false)
        );
        assert_eq!(image.alt_end(state.text()), 6, "after `wide`");
        let embed = doc.find("![[b").unwrap_or(0);
        let found = image_at(state.tree(), state.text(), embed + 4).expect("the embed");
        assert_eq!(
            (found.width(), found.block, found.embed),
            (Some(40), false, true)
        );
        assert_eq!(found.alt_end(state.text()), embed + 8);
        let note = doc.find("![[Note").unwrap_or(0);
        assert_eq!(image_at(state.tree(), state.text(), note + 3), None);
    }

    #[test]
    fn setting_a_width_is_one_undo_step_and_keeps_an_object_selection() {
        let doc = "intro\n\n![a](x.png)\n\nend";
        let mut state = state(doc);
        let image = 7..18;
        let _ = state.select(Selection::single(image.end, image.start));
        assert!(state.set_image_width(image.clone(), Some(300)));
        assert_eq!(state.doc(), "intro\n\n![a|300](x.png)\n\nend");
        let main = state.selection().main();
        assert_eq!((main.anchor, main.head), (22, 7), "still the whole image");
        assert!(!state.set_image_width(7..22, Some(300)), "already 300");
        assert!(!state.set_image_width(0..5, Some(300)), "no image there");
        state.undo();
        assert_eq!(state.doc(), doc);
    }

    #[test]
    fn deleting_an_image_removes_its_markdown_in_one_undo_step() {
        let doc = "intro

![a|300](x.png)

![[pic.png|40]]
";
        let mut state = state(doc);
        assert!(!state.delete_image(0..5), "no image there");
        assert!(!state.delete_image(7..12), "not the image's node");
        let _ = state.select(Selection::single(22, 7));
        assert!(state.delete_image(7..22));
        assert_eq!(
            state.doc(),
            "intro



![[pic.png|40]]
"
        );
        assert_eq!(state.selection().main(), SelectionRange::cursor(7));
        state.undo();
        assert_eq!(state.doc(), doc);
        let embed = doc.find("![[").unwrap_or(0);
        let _ = state.select(Selection::cursor(2));
        assert!(state.delete_image(embed..embed + 15));
        assert_eq!(
            state.doc(),
            "intro

![a|300](x.png)


"
        );
        assert_eq!(state.selection().main(), SelectionRange::cursor(2), "stays");
    }
}
