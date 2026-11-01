//! US-010: 10 000 random edits never make the parser panic, and every
//! windowed reparse equals a full parse of the same text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cerata_markdown::{Change, ChangeSet, MarkdownTree, Text};

/// Deterministic xorshift, so a failure replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "x",
    "word ",
    " ",
    "\n",
    "\n\n",
    "- ",
    "* ",
    "1. ",
    "2) ",
    "> ",
    "# ",
    "## ",
    "`",
    "```",
    "~~~",
    "**",
    "*",
    "_",
    "~~",
    "[",
    "]",
    "(",
    ")",
    "![a](b.png)",
    "[l](u)",
    "[[w]]",
    "<div>",
    "</div>",
    "<b>",
    "|",
    "| a | b |\n| - | - |\n",
    "---",
    "===",
    "\t",
    "    ",
    ":smile:",
    "--",
    "\\",
    "&amp;",
    "- [ ] ",
    "<https://a.b>",
    "www.x.y ",
    "é",
    "日本",
];

fn seed_document() -> String {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
    let mut doc = String::new();
    for name in [
        "note",
        "blockquote",
        "lists-nested",
        "code-fenced",
        "table",
        "links-inline",
    ] {
        doc.push_str(&std::fs::read_to_string(fixtures.join(format!("{name}.md"))).unwrap());
        doc.push('\n');
    }
    for i in 0..60 {
        doc.push_str(&format!("- item {i} with `code` and https://e.com/{i}\n"));
    }
    doc
}

fn floor_boundary(text: &str, mut pos: usize) -> usize {
    pos = pos.min(text.len());
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[test]
fn ten_thousand_random_edits_reparse_like_a_full_parse() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut doc = seed_document();
    let mut tree = MarkdownTree::parse(&doc);
    let mut windowed = 0;
    for step in 0..10_000 {
        let old = Text::new(doc.clone());
        let pos = floor_boundary(&doc, rng.below(doc.len() + 1));
        let change = if rng.below(3) == 0 && pos < doc.len() {
            let end = floor_boundary(&doc, pos + 1 + rng.below(6));
            let end = if end <= pos {
                (pos + 1..=doc.len())
                    .find(|&e| doc.is_char_boundary(e))
                    .unwrap()
            } else {
                end
            };
            Change::delete(pos..end)
        } else {
            Change::insert(pos, TOKENS[rng.below(TOKENS.len())])
        };
        let changes = ChangeSet::new(&old, vec![change.clone()]).unwrap();
        doc.replace_range(change.from..change.to, &change.insert);
        let window = tree.reparse_window(old.as_str(), &doc, &changes);
        windowed += usize::from(window.is_some());
        let incremental = window.unwrap_or_else(|| MarkdownTree::parse(&doc));
        let full = MarkdownTree::parse(&doc);
        assert_eq!(
            incremental.dump(),
            full.dump(),
            "step {step}: edit {change:?} on {:?}",
            old.as_str()
                .get(pos.saturating_sub(40)..(pos + 40).min(old.len()))
        );
        tree = incremental;
        // Keep the document from drifting to empty or huge.
        if doc.len() > 12_000 || doc.len() < 200 {
            doc = seed_document();
            tree = MarkdownTree::parse(&doc);
        }
    }
    // Most edits stay in their window; the rest fall back to a full parse.
    assert!(windowed > 5_000, "only {windowed} windowed reparses");
}

#[test]
fn duplicate_definitions_reparse_like_a_full_parse() {
    // pulldown-cmark keeps the first definition of a label only: the window
    // must not depend on a definition outside it.
    let doc = "[a]: /x\n\np1\n\np2\n\np3\n\n[a]: /y\n\np4\n";
    let edits = [
        Change::insert(doc.find("p3").unwrap() + 2, "z"),
        Change::delete(0..8),
        Change::insert(doc.find("p2").unwrap(), "[a]: /w\n\n"),
    ];
    for change in edits {
        let mut new = doc.to_owned();
        new.replace_range(change.from..change.to, &change.insert);
        let old = Text::new(doc);
        let changes = ChangeSet::new(&old, vec![change.clone()]).unwrap();
        let tree = MarkdownTree::parse(doc).reparse(doc, &new, &changes);
        assert_eq!(tree.dump(), MarkdownTree::parse(&new).dump(), "{change:?}");
    }
}

#[test]
fn a_line_break_before_the_last_dollars_reparses_like_a_full_parse() {
    // No `$` edited, yet the last `$$` now opens a line: unclosed math.
    let doc = "# a\n\np1\n\np2\n\ntext $$ x\n\np3\n";
    let at = doc.find("$$").unwrap();
    for change in [Change::insert(at, "\n"), Change::new(at - 5..at, "")] {
        let mut new = doc.to_owned();
        new.replace_range(change.from..change.to, &change.insert);
        let old = Text::new(doc);
        let changes = ChangeSet::new(&old, vec![change.clone()]).unwrap();
        let tree = MarkdownTree::parse(doc).reparse(doc, &new, &changes);
        let full = MarkdownTree::parse(&new);
        assert_eq!(full.root().last_child().unwrap().name(), "Math");
        assert_eq!(tree.dump(), full.dump(), "{change:?}");
    }
}

#[test]
fn unclosed_fence_and_display_math_run_to_the_end() {
    let fence = MarkdownTree::parse("a\n\n```\ncode\n\n# not heading\n");
    let code = fence.root().last_child().unwrap();
    assert_eq!(code.name(), "FencedCode");
    assert_eq!(code.to(), fence.root().to());
    let math = MarkdownTree::parse("para\n\n$$\nE = mc^2\n\n- not a list\n");
    let block = math.root().last_child().unwrap();
    assert_eq!(block.name(), "Math");
    assert_eq!(block.range(), 6..math.root().to());
    assert_eq!(
        block
            .child(cerata_markdown::NodeKind::MathMark)
            .unwrap()
            .range(),
        6..8
    );
    // Closed later: Writer's inline rule.
    let closed = MarkdownTree::parse("$$\nx\n$$\n");
    assert_eq!(closed.root().first_child().unwrap().name(), "Paragraph");
}
