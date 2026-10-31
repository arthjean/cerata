//! Fixture corpus: one case per Writer custom rule plus the cases of the
//! three Writer test files, with Writer's expected result for each probe.
//! Expectations are derived from the Lezer rules cited on each `Rule`.

use crate::probes::Probe::{self, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    Frontmatter,
    Math,
    SpaceDestination,
    NestedLink,
    SetextDash,
    Escape,
    Dash,
    Emoji,
    WikiLink,
}

impl Rule {
    pub const ALL: [Rule; 9] = [
        Rule::Frontmatter,
        Rule::Math,
        Rule::SpaceDestination,
        Rule::NestedLink,
        Rule::SetextDash,
        Rule::Escape,
        Rule::Dash,
        Rule::Emoji,
        Rule::WikiLink,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rule::Frontmatter => "frontmatter at offset 0 (markdown/frontmatter.ts:37-91)",
            Rule::Math => "Pandoc math $..$ / $$..$$ (markdown/mathMarkdown.ts:57-101)",
            Rule::SpaceDestination => {
                "link destinations with spaces (markdown/spaceDestinationLinks.ts:150-187)"
            }
            Rule::NestedLink => {
                "nested links as plain text (markdown/nestedLinkAsPlainText.ts:59-87)"
            }
            Rule::SetextDash => "single `-` is not an H2 (markdown/setextDashUnderline.ts:50-58)",
            Rule::Escape => "escape of any char (hide/index.ts:109-126)",
            Rule::Dash => "dashes -- / --- (fold/dashes.ts:6-27)",
            Rule::Emoji => "emoji :word: (fold/emoji.ts:9-37)",
            Rule::WikiLink => "wiki links (editor-area/wiki-link-extension.ts:41-50)",
        }
    }
}

pub struct Case {
    pub id: &'static str,
    pub rule: Rule,
    /// Writer's expected observation for each probe.
    pub checks: &'static [(Probe, &'static [&'static str])],
    /// Native result recorded from the run (tree-sitter with the virtual
    /// trailing newline, pulldown-cmark with `pd::options()`), pinned by the
    /// tests so the ADR cannot drift from the code.
    pub ts_native: bool,
    pub pd_native: bool,
}

const fn case(
    id: &'static str,
    rule: Rule,
    checks: &'static [(Probe, &'static [&'static str])],
    ts_native: bool,
    pd_native: bool,
) -> Case {
    Case {
        id,
        rule,
        checks,
        ts_native,
        pd_native,
    }
}

const PS: &str = "Paragraph";
const BL: &str = "BulletList";

pub const CASES: &[Case] = &[
    // Frontmatter.
    case(
        "frontmatter-offset0",
        Rule::Frontmatter,
        &[(Frontmatter, &["---\ntitle: Hello\ntags: [a, b]\n---"])],
        true,
        true,
    ),
    case(
        "frontmatter-not-offset0",
        Rule::Frontmatter,
        &[(Frontmatter, &[])],
        true,
        false,
    ),
    case(
        "frontmatter-leading-blank-line",
        Rule::Frontmatter,
        &[(Frontmatter, &[])],
        true,
        false,
    ),
    case(
        "frontmatter-eof-no-newline",
        Rule::Frontmatter,
        &[(Frontmatter, &["---\ntitle: x\n---"])],
        true,
        true,
    ),
    case(
        "frontmatter-unclosed",
        Rule::Frontmatter,
        &[(Frontmatter, &[])],
        true,
        true,
    ),
    case(
        "frontmatter-empty",
        Rule::Frontmatter,
        &[(Frontmatter, &["---\n---"])],
        true,
        false,
    ),
    // Math (tests/math.test.ts plus two extra cases).
    case(
        "math-inline",
        Rule::Math,
        &[(Math, &["$e^{i\\pi} + 1 = 0$"])],
        true,
        true,
    ),
    case(
        "math-display",
        Rule::Math,
        &[(Math, &["$$\\sum_{i=1}^n i$$"])],
        true,
        true,
    ),
    case(
        "math-escaped-dollar",
        Rule::Math,
        &[(Math, &[])],
        true,
        true,
    ),
    case("math-currency", Rule::Math, &[(Math, &[])], false, true),
    case("math-opener-space", Rule::Math, &[(Math, &[])], false, true),
    case("math-closer-space", Rule::Math, &[(Math, &[])], false, true),
    case(
        "math-next-to-currency",
        Rule::Math,
        &[(Math, &["$x+y$"])],
        true,
        true,
    ),
    case(
        "math-display-inner-space",
        Rule::Math,
        &[(Math, &["$$ x^2 $$"])],
        true,
        true,
    ),
    case(
        "math-display-blank",
        Rule::Math,
        &[(Math, &["$$  $$"])],
        true,
        true,
    ),
    case(
        "math-currency-then-math",
        Rule::Math,
        &[(Math, &["$5 and $x$"])],
        false,
        false,
    ),
    case(
        "math-in-code-span",
        Rule::Math,
        &[(Math, &["$y$"])],
        true,
        true,
    ),
    // Spaced link destinations (tests/markdown-links.test.ts).
    case(
        "link-space-destination",
        Rule::SpaceDestination,
        &[
            (Link, &["[Todo Label](Writer TODOs.md)"]),
            (Url, &["Writer TODOs.md"]),
        ],
        false,
        false,
    ),
    case(
        "image-space-destination",
        Rule::SpaceDestination,
        &[
            (
                Image,
                &["![Image Label](Writer TODOs-assets/20260525 image.png)"],
            ),
            (Url, &["Writer TODOs-assets/20260525 image.png"]),
        ],
        false,
        false,
    ),
    case(
        "link-space-destination-title",
        Rule::SpaceDestination,
        &[
            (Link, &["[Todo](Writer TODOs.md \"Readable Title\")"]),
            (Url, &["Writer TODOs.md"]),
            (Title, &["\"Readable Title\""]),
        ],
        false,
        false,
    ),
    case(
        "link-plain-destination-title",
        Rule::SpaceDestination,
        &[
            (Url, &["writer-todos.md"]),
            (Title, &["\"Readable Title\""]),
        ],
        true,
        true,
    ),
    // Nested links.
    case(
        "nested-inline-link",
        Rule::NestedLink,
        &[(Link, &["[outer [inner](b) text](a)"]), (Url, &["a"])],
        false,
        false,
    ),
    case(
        "nested-shortcut-link",
        Rule::NestedLink,
        &[(Link, &["[a [b] c](d)"])],
        false,
        true,
    ),
    // Setext (tests/setext-dash-underline.test.ts:22-37), without and with a
    // trailing newline.
    case(
        "setext-single-dash-eol",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        false,
        false,
    ),
    case(
        "setext-single-dash-space",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        false,
        false,
    ),
    case(
        "setext-single-dash-text",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        true,
        true,
    ),
    case(
        "setext-double-dash",
        Rule::SetextDash,
        &[(Blocks, &["SetextHeading2"])],
        true,
        true,
    ),
    case(
        "setext-triple-dash",
        Rule::SetextDash,
        &[(Blocks, &["SetextHeading2"])],
        true,
        true,
    ),
    case(
        "setext-single-equal",
        Rule::SetextDash,
        &[(Blocks, &["SetextHeading1"])],
        true,
        true,
    ),
    case(
        "setext-single-dash-multiline",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        false,
        false,
    ),
    case(
        "setext-single-dash-eol-nl",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        false,
        false,
    ),
    case(
        "setext-single-dash-space-nl",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        false,
        false,
    ),
    case(
        "setext-single-dash-text-nl",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        true,
        true,
    ),
    case(
        "setext-double-dash-nl",
        Rule::SetextDash,
        &[(Blocks, &["SetextHeading2"])],
        true,
        true,
    ),
    case(
        "setext-triple-dash-nl",
        Rule::SetextDash,
        &[(Blocks, &["SetextHeading2"])],
        true,
        true,
    ),
    case(
        "setext-single-equal-nl",
        Rule::SetextDash,
        &[(Blocks, &["SetextHeading1"])],
        true,
        true,
    ),
    case(
        "setext-single-dash-multiline-nl",
        Rule::SetextDash,
        &[(Blocks, &[PS, BL])],
        false,
        false,
    ),
    // Extended escape.
    case(
        "escape-ascii-punctuation",
        Rule::Escape,
        &[(Escape, &["\\*", "\\_"])],
        true,
        false,
    ),
    case(
        "escape-any-char",
        Rule::Escape,
        &[(Escape, &["\\a", "\\é", "\\1"])],
        false,
        false,
    ),
    case(
        "escape-in-code-span",
        Rule::Escape,
        &[(Escape, &["\\b"])],
        false,
        false,
    ),
    // Dashes (Dash nodes of length 2 or 3, the ones Writer renders).
    case("dash-en", Rule::Dash, &[(Dash, &["--"])], false, true),
    case("dash-em", Rule::Dash, &[(Dash, &["---"])], false, true),
    case("dash-four", Rule::Dash, &[(Dash, &[])], true, true),
    case("dash-in-word", Rule::Dash, &[(Dash, &["--"])], false, true),
    case(
        "dash-in-code-span",
        Rule::Dash,
        &[(Dash, &["---"])],
        false,
        true,
    ),
    // Emoji.
    case(
        "emoji-basic",
        Rule::Emoji,
        &[(Emoji, &[":smile:"])],
        false,
        false,
    ),
    case(
        "emoji-two",
        Rule::Emoji,
        &[(Emoji, &[":a:", ":b_c:"])],
        false,
        false,
    ),
    case("emoji-time", Rule::Emoji, &[(Emoji, &[])], true, true),
    case(
        "emoji-ratio",
        Rule::Emoji,
        &[(Emoji, &[":2:"])],
        false,
        false,
    ),
    case(
        "emoji-in-code-span",
        Rule::Emoji,
        &[(Emoji, &[":y:"])],
        false,
        false,
    ),
    // Wiki links.
    case(
        "wiki-basic",
        Rule::WikiLink,
        &[(Wiki, &["[[Some Note]]"])],
        false,
        true,
    ),
    case(
        "wiki-alias",
        Rule::WikiLink,
        &[(Wiki, &["[[Note|Alias]]"])],
        false,
        true,
    ),
    case(
        "wiki-embed",
        Rule::WikiLink,
        &[(Wiki, &["![[img.png]]"])],
        false,
        true,
    ),
    case(
        "wiki-in-code-span",
        Rule::WikiLink,
        &[(Wiki, &["[[y]]"])],
        false,
        true,
    ),
];

/// Node kinds that must exist in the tree-sitter tree of `markers.md` for
/// the decoration rules to work without reconstructing markers from text.
pub const TS_MARKER_KINDS: &[&str] = &[
    "atx_h1_marker",
    "setext_h1_underline",
    "emphasis_delimiter",
    "code_span_delimiter",
    "link_destination",
    "link_title",
    "link_text",
    "image_description",
    "uri_autolink",
    "backslash_escape",
    "latex_span_delimiter",
    "list_marker_minus",
    "list_marker_dot",
    "task_list_marker_checked",
    "block_quote_marker",
    "fenced_code_block_delimiter",
    "info_string",
    "pipe_table_delimiter_row",
];

/// The long note of `e2e/specs/list-enter-exit.spec.js:238-251`.
pub fn list_3000() -> String {
    let mut lines = vec!["# Long note".to_string(), String::new()];
    for i in 0..3000 {
        lines.push(format!(
            "- item {i} with a `code span`, a link https://example.com/{i} and enough text to wrap onto a second line in the editor column"
        ));
    }
    lines.push("- im creating a sub item!!".to_string());
    lines
        .push("- jev API (from the core package): an async generator yielding events.".to_string());
    lines.push(String::new());
    lines.join("\n")
}

/// A mixed note of at least 1 MiB exercising every block and inline kind.
pub fn note_1mb() -> String {
    let mut out = String::from("---\ntitle: Big note\ntags: [spike, bench]\n---\n\n");
    let mut i = 0usize;
    while out.len() < 1 << 20 {
        let h = "#".repeat(1 + i % 6);
        out.push_str(&format!(
            "{h} Section {i} with *emphasis*\n\n\
Paragraph text {i}: **strong**, _em_, `code`, [a link](https://example.com/{i} \"t\"), \
$x_{i}^2$, :smile:, well--known --- dashes, [[Note {i}]] and \\* escapes. \
Enough prose to wrap onto several lines in the editor column, again and again.\n\
Second line of the same paragraph with ~~strike~~ and <https://auto.link/{i}>.\n\n\
- bullet one {i}\n- bullet two with `code`\n  - nested bullet\n- [x] task done\n- [ ] task open\n\n\
1. first\n2. second\n\n\
> A quote {i} with *style*\n> on two lines.\n\n\
```rust\nfn f{i}() -> u32 {{ {i} }}\n```\n\n\
| col a | col b |\n|---|:---:|\n| {i} | **b** |\n\n\
$$\n\\sum_{{k=0}}^{{{i}}} k\n$$\n\n\
Setext title {i}\n---\n\n"
        ));
        i += 1;
    }
    out
}
