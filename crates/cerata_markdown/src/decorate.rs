//! Live-preview decorations: which syntax is hidden, styled or replaced by a
//! widget for a given tree and selection. A pure function of its inputs, so
//! the renderer (EP-003) only lays out what it gets.
//!
//! Ported from prosemark-core: the hide specs (`hide/core.ts:35-124`,
//! `hide/index.ts:22-109`), the fold specs (`fold/core.ts:80-149`,
//! `fold/image.ts`, `fold/horizontalRule.ts`, `fold/dashes.ts`,
//! `fold/emoji.ts`) and the heading line styles
//! (`components/editor-area/heading-decorations.ts`). The contact rule is
//! `utils.ts:126-132`: bounds are inclusive, so a caret right after `**`
//! still reveals it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use crate::selection::Selection;
use crate::text::Text;
use crate::tree::{MarkdownTree, Node, NodeKind};

/// Heading font size per level (H1 first), relative to the body
/// (`syntaxHighlighting.ts` `HEADING_SCALES`).
pub const HEADING_SCALES: [f32; 6] = [1.6, 1.4, 1.2, 1.1, 1.0, 0.9];
/// Font weight of heading lines.
pub const HEADING_WEIGHT: u16 = 600;
/// Width of a tab, in `ch`.
pub const TAB_WIDTH_CH: usize = 4;

/// How hidden syntax is removed from the rendered line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HideMode {
    /// Zero width (Writer's `font-size: 0` mark).
    Collapse,
    /// Removed from the rendered text (Writer's `Decoration.replace({})`).
    Remove,
    /// The whole line is removed (setext underline).
    Block,
    /// Not drawn but keeps its width (fence marks, quote `>`).
    KeepSpace,
}

/// Inline styles kept while their syntax is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkStyle {
    /// Emphasis and strong emphasis share one wrapper (`cm-emphasis`).
    Emphasis,
    InlineCode,
    Strikethrough,
    /// A folded link (`cm-rendered-link`).
    RenderedLink,
}

/// Per-line styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LineStyle {
    /// ATX or setext heading of this level (1 to 6).
    Heading(u8),
    /// A line of a code block (fenced, indented or frontmatter): monospace,
    /// code background, no wrapping; the first and last lines are rounded.
    Code { first: bool, last: bool },
    /// A line of a blockquote: the quote bar.
    Quote,
    /// A line of a table whose source is revealed: monospace on the code
    /// background, wrapping (`.cm-table-source-line`).
    TableSource { first: bool, last: bool },
}

impl LineStyle {
    /// Font scale relative to the body.
    pub fn scale(self) -> f32 {
        match self {
            LineStyle::Heading(level) => HEADING_SCALES[usize::from(level.clamp(1, 6)) - 1],
            _ => 1.0,
        }
    }

    pub fn weight(self) -> u16 {
        match self {
            LineStyle::Heading(_) => HEADING_WEIGHT,
            _ => 400,
        }
    }

    pub fn is_monospace(self) -> bool {
        matches!(self, LineStyle::Code { .. } | LineStyle::TableSource { .. })
    }
}

/// What a fold shows in place of its source.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Widget {
    /// An image, by its normalized destination.
    Image {
        url: String,
    },
    /// An Obsidian image embed (`![[pic.png]]`), by its target
    /// (`wiki::image_embed_target`); the host finds the file.
    WikiImage {
        target: String,
    },
    HorizontalRule,
    /// `--` (2) or `---` (3).
    Dash(u8),
    Emoji(&'static str),
    /// A code block's first line: its language label and a button copying
    /// `code` (`CodeBlockInfoWidget`).
    CodeInfo {
        label: String,
        code: String,
    },
    /// The extra bar of a nested quote, drawn at the inner `>`.
    NestedQuoteBorder,
    /// A tab, drawn `TAB_WIDTH_CH` wide (`tabWidthExtension.ts`).
    Tab,
    /// A folded GFM table: its rows, quote prefixes removed, one per line.
    Table {
        source: String,
    },
    /// A folded wiki link: its alias, else its target
    /// (`wiki-link-extension.ts` `WikiLinkWidget`).
    WikiLink {
        display: String,
    },
    /// The frontmatter, drawn as its properties panel.
    Frontmatter,
    /// An HTML block, drawn sanitized (`HtmlBlockWidget`).
    Html {
        source: String,
    },
    /// A formula (`MathWidget`): `$...$` inline, `$$...$$` displayed.
    Math {
        tex: String,
        display: bool,
    },
    /// A closed ` ```mermaid ` fence, drawn as its diagram's canvas
    /// (`MermaidWidget`); `source` is its body, trimmed.
    Mermaid {
        source: String,
    },
}

impl Widget {
    /// A fold of the source (image, rule, dash, emoji, table), as opposed
    /// to the chrome drawn next to it.
    pub fn is_fold(&self) -> bool {
        matches!(
            self,
            Widget::Image { .. }
                | Widget::WikiImage { .. }
                | Widget::HorizontalRule
                | Widget::Dash(_)
                | Widget::Emoji(_)
                | Widget::Table { .. }
                | Widget::Frontmatter
                | Widget::Html { .. }
                | Widget::Math { .. }
                | Widget::Mermaid { .. }
        )
    }

    /// The text a dash or emoji widget renders.
    pub fn glyph(&self) -> Option<&str> {
        match self {
            Widget::Dash(2) => Some("\u{2013}"),
            Widget::Dash(3) => Some("\u{2014}"),
            Widget::Emoji(emoji) => Some(emoji),
            Widget::WikiLink { display } => Some(display),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Decoration {
    Hide {
        range: Range<usize>,
        mode: HideMode,
    },
    Mark {
        range: Range<usize>,
        style: MarkStyle,
    },
    /// `range` is drawn as `widget`; `block` widgets take whole lines.
    Replace {
        range: Range<usize>,
        widget: Widget,
        block: bool,
    },
    /// `widget` is drawn at `pos`, the source stays visible.
    Widget {
        pos: usize,
        widget: Widget,
        block: bool,
    },
    /// Style of the line starting at `line_start`.
    Line {
        line_start: usize,
        style: LineStyle,
    },
}

impl Decoration {
    /// Order among decorations of the same range.
    fn rank(&self) -> u8 {
        match self {
            Decoration::Line { .. } => 0,
            Decoration::Mark { style, .. } => 1 + *style as u8,
            Decoration::Hide { mode, .. } => 8 + *mode as u8,
            Decoration::Widget { .. } => 16,
            Decoration::Replace { .. } => 17,
        }
    }

    /// The widget a fold draws, if this is one.
    pub fn widget(&self) -> Option<&Widget> {
        match self {
            Decoration::Replace { widget, .. } | Decoration::Widget { widget, .. } => Some(widget),
            _ => None,
        }
    }

    pub fn range(&self) -> Range<usize> {
        match self {
            Decoration::Hide { range, .. }
            | Decoration::Mark { range, .. }
            | Decoration::Replace { range, .. } => range.clone(),
            Decoration::Widget { pos, .. } => *pos..*pos,
            Decoration::Line { line_start, .. } => *line_start..*line_start,
        }
    }
}

/// Decorations of `text` (parsed as `tree`) for `selection`. While a drag is
/// in flight, `drag_frozen` holds the selection the drag started from
/// (mapped through later edits by `EditorState`): the revealed syntax stays
/// what it was, so the text does not shift under the pointer
/// (`unfurlFreeze.ts`). Selections past the end are clamped. Sorted by
/// position, without duplicates.
pub fn decorate(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
) -> Vec<Decoration> {
    decorate_range(tree, text, selection, drag_frozen, 0..text.len())
}

/// [`decorate`] restricted to the nodes touching `range` (inclusive) and the
/// tabs inside it. A node that spans past `range` still yields all its
/// decorations, so a renderer filters by line.
pub fn decorate_range(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
    range: Range<usize>,
) -> Vec<Decoration> {
    let selection = drag_frozen.unwrap_or(selection).clamp(text);
    let mut cx = Cx {
        text,
        selection: &selection,
        out: Vec::new(),
    };
    for node in tree.iter_range(range.clone()) {
        cx.hide(node);
        cx.fold(node);
        cx.line(node);
    }
    cx.tabs(range);
    let mut out = cx.out;
    out.sort_by_key(|d| {
        let range = d.range();
        (range.start, range.end, d.rank())
    });
    // Nested nodes hide the same mark twice (a quote inside a quote).
    let mut unique: Vec<Decoration> = Vec::with_capacity(out.len());
    for decoration in out {
        let run_start = unique.partition_point(|d| {
            let (a, b) = (d.range(), decoration.range());
            (a.start, a.end, d.rank()) < (b.start, b.end, decoration.rank())
        });
        if !unique[run_start..].contains(&decoration) {
            unique.push(decoration);
        }
    }
    unique
}

struct Cx<'a> {
    text: &'a Text,
    selection: &'a Selection,
    out: Vec<Decoration>,
}

impl Cx<'_> {
    fn touches(&self, range: Range<usize>) -> bool {
        self.selection.touches(range)
    }

    /// Hides every descendant of `node` (itself included) named in `kinds`.
    fn hide_descendants(&mut self, node: Node<'_>, kinds: &[NodeKind], mode: HideMode) {
        for child in std::iter::once(node).chain(node.descendants()) {
            if kinds.contains(&child.kind()) {
                self.out.push(Decoration::Hide {
                    range: child.range(),
                    mode,
                });
            }
        }
    }

    fn mark(&mut self, node: Node<'_>, style: MarkStyle) {
        self.out.push(Decoration::Mark {
            range: node.range(),
            style,
        });
    }

    /// `hide/index.ts:22-109`, through `hide/core.ts:35-124`.
    fn hide(&mut self, node: Node<'_>) {
        use NodeKind as K;
        match node.kind() {
            K::AtxHeading(_) => {
                if !self.touches(node.range())
                    && let Some(mark) = node.first_child()
                {
                    let to = (mark.to() + 1).min(node.to());
                    self.out.push(Decoration::Hide {
                        range: mark.from()..to,
                        mode: HideMode::Collapse,
                    });
                }
            }
            K::SetextHeading(_) => {
                if !self.touches(node.range()) {
                    self.hide_descendants(node, &[K::HeaderMark], HideMode::Block);
                }
            }
            K::StrongEmphasis | K::Emphasis => {
                self.mark(node, MarkStyle::Emphasis);
                if !self.touches(node.range()) {
                    self.hide_descendants(node, &[K::EmphasisMark], HideMode::Remove);
                }
            }
            K::InlineCode => {
                self.mark(node, MarkStyle::InlineCode);
                if !self.touches(node.range()) {
                    self.hide_descendants(node, &[K::CodeMark], HideMode::Collapse);
                }
            }
            K::Link => {
                if !self.touches(node.range()) {
                    self.mark(node, MarkStyle::RenderedLink);
                    self.hide_descendants(
                        node,
                        &[K::LinkMark, K::Url, K::LinkTitle],
                        HideMode::Collapse,
                    );
                }
            }
            K::Autolink => {
                if !self.touches(node.range()) {
                    self.hide_descendants(node, &[K::LinkMark], HideMode::Collapse);
                }
            }
            K::Strikethrough => {
                self.mark(node, MarkStyle::Strikethrough);
                if !self.touches(node.range()) {
                    self.hide_descendants(node, &[K::StrikethroughMark], HideMode::Remove);
                }
            }
            K::Escape => {
                // Revealed while the WORD around it (or, for a lone `\x`,
                // its line) is touched (`hide/index.ts:89-93`).
                let zone = match word_at(self.text, node.from()) {
                    Some(word) if word.end > node.from() + 1 => word,
                    _ => self.text.line_at(node.from()).range(),
                };
                if !self.touches(zone) && !self.touches(node.range()) {
                    self.hide_descendants(node, &[K::EscapeMark], HideMode::Collapse);
                }
            }
            K::FencedCode if !self.touches(node.range()) => {
                self.hide_descendants(node, &[K::CodeMark, K::CodeInfo], HideMode::KeepSpace);
            }
            K::Blockquote if !self.touches(node.range()) => {
                self.hide_descendants(node, &[K::QuoteMark], HideMode::KeepSpace);
            }
            _ => {}
        }
    }

    /// `fold/core.ts:80-149` with the image, emoji, rule and dash specs.
    fn fold(&mut self, node: Node<'_>) {
        match node.kind() {
            NodeKind::Image => self.fold_image(node),
            NodeKind::Table => self.fold_table(node),
            NodeKind::WikiLink => self.fold_wiki_link(node),
            // The properties panel while the selection is away
            // (`frontmatter-panel.tsx`), else the YAML as a code block.
            // `html-block-decorations.ts:185-218`: the sanitized HTML
            // while untouched; `script` and `style` blocks stay source.
            NodeKind::HtmlBlock
                if !self.touches(node.range())
                    && !crate::html::is_script_or_style(self.text.slice(node.range())) =>
            {
                self.out.push(Decoration::Replace {
                    range: node.range(),
                    widget: Widget::Html {
                        source: self.text.slice(node.range()).to_owned(),
                    },
                    block: true,
                });
            }
            NodeKind::Math if !self.touches(node.range()) => self.fold_math(node),
            // A diagram stays drawn whatever the selection: it is edited in
            // its canvas, and the caret steps over it (`atomic.rs`,
            // `mermaid-decorations.ts:186-197`).
            NodeKind::FencedCode if self.mermaid_source(node).is_some() => {
                if let Some(source) = self.mermaid_source(node) {
                    self.out.push(Decoration::Replace {
                        range: node.range(),
                        widget: Widget::Mermaid { source },
                        block: true,
                    });
                }
            }
            NodeKind::Frontmatter if !self.touches(node.range()) => {
                self.out.push(Decoration::Replace {
                    range: node.range(),
                    widget: Widget::Frontmatter,
                    block: true,
                });
            }
            NodeKind::Emoji => {
                if self.touches(node.range()) || node.to() < node.from() + 2 {
                    return;
                }
                let name = self.text.slice(node.from() + 1..node.to() - 1);
                if let Some(emoji) = emoji(name) {
                    self.out.push(Decoration::Replace {
                        range: node.range(),
                        widget: Widget::Emoji(emoji),
                        block: false,
                    });
                }
            }
            NodeKind::HorizontalRule => {
                if !self.touches(node.range()) {
                    self.out.push(Decoration::Replace {
                        range: node.range(),
                        widget: Widget::HorizontalRule,
                        block: true,
                    });
                }
            }
            NodeKind::Dash => {
                let count = node.to() - node.from();
                if !self.touches(node.range()) && (2..=3).contains(&count) {
                    self.out.push(Decoration::Replace {
                        range: node.range(),
                        widget: Widget::Dash(count as u8),
                        block: false,
                    });
                }
            }
            _ => {}
        }
    }

    /// `wiki-link-extension.ts:195-238`: the link text while no selection
    /// range lies inside the brackets, else the source (link-colored by
    /// the renderer). An image embed (`![[pic.png]]`) folds to its image,
    /// a block when alone on its line; another embed keeps its `!` as
    /// source.
    fn fold_wiki_link(&mut self, node: Node<'_>) {
        let bang = self.text.slice(node.range()).starts_with('!');
        let image = bang
            .then(|| {
                let range = node.range();
                (range.end >= range.start + 5)
                    .then(|| {
                        crate::wiki::image_embed_target(
                            self.text.slice(range.start + 3..range.end - 2),
                        )
                    })
                    .flatten()
            })
            .flatten();
        let start = node.from() + usize::from(bang && image.is_none());
        let end = node.to();
        if end < start + 4 {
            return;
        }
        let inside = self
            .selection
            .ranges()
            .iter()
            .any(|r| r.from() >= start && r.to() <= end);
        if inside {
            return;
        }
        if let Some(target) = image {
            let line = self.text.line_at(start);
            self.out.push(Decoration::Replace {
                range: start..end,
                widget: Widget::WikiImage { target },
                block: start == line.from && end == line.to,
            });
            return;
        }
        let inner = self.text.slice(start + 2..end - 2);
        self.out.push(Decoration::Replace {
            range: start..end,
            widget: Widget::WikiLink {
                display: crate::wiki::parse(inner).display,
            },
            block: false,
        });
    }

    fn mermaid_source(&self, node: Node<'_>) -> Option<String> {
        crate::atomic::mermaid_body(self.text, node)
    }

    /// `math-decorations.ts:56-72`: the rendered formula while untouched; an
    /// empty one keeps its source. A `$$` formula alone on its lines is a
    /// block; elsewhere it flows with the text, displayed.
    fn fold_math(&mut self, node: Node<'_>) {
        let Some(formula) = node.children().find(|c| c.kind() == NodeKind::MathFormula) else {
            return;
        };
        let tex = self.text.slice(formula.range());
        if tex.trim().is_empty() {
            return;
        }
        let display = self.text.slice(node.range()).starts_with("$$");
        let (first, last) = (self.text.line_at(node.from()), self.text.line_at(node.to()));
        let alone = self.text.slice(first.from..node.from()).trim().is_empty()
            && self.text.slice(node.to()..last.to).trim().is_empty();
        self.out.push(Decoration::Replace {
            range: node.range(),
            widget: Widget::Math {
                tex: tex.to_owned(),
                display,
            },
            block: display && alone,
        });
    }

    /// `table-decorations.ts:507-523`: the rendered grid while untouched,
    /// else the source lines on the code background.
    fn fold_table(&mut self, node: Node<'_>) {
        let rows: Vec<&str> = node
            .children()
            .filter(|c| {
                matches!(
                    c.kind(),
                    NodeKind::TableHeader | NodeKind::TableDelimiter | NodeKind::TableRow
                )
            })
            .map(|c| self.text.slice(c.range()))
            .collect();
        let source = rows.join("\n");
        if crate::table::parse(&source, |l| l).is_none() {
            return;
        }
        if self.touches(node.range()) {
            let (first, last) = (
                self.text.line_at(node.from()).number,
                self.text.line_at(node.to()).number,
            );
            for n in first..=last {
                self.out.push(Decoration::Line {
                    line_start: self.text.line(n).from,
                    style: LineStyle::TableSource {
                        first: n == first,
                        last: n == last,
                    },
                });
            }
        } else {
            self.out.push(Decoration::Replace {
                range: node.range(),
                widget: Widget::Table { source },
                block: true,
            });
        }
    }

    /// `fold/image.ts:94-121`: replaced when untouched, a widget after the
    /// source when touched, nothing while a caret is in the destination.
    fn fold_image(&mut self, node: Node<'_>) {
        let Some(url) = node.children().find(|c| c.kind() == NodeKind::Url) else {
            return;
        };
        if self.touches(url.range()) {
            return;
        }
        let destination = normalize_destination(self.text.slice(url.range()));
        if destination.is_empty() {
            return;
        }
        let line = self.text.line_at(node.from());
        let block = node.from() == line.from && node.to() == line.to;
        let widget = Widget::Image { url: destination };
        if self.touches(node.range()) {
            self.out.push(Decoration::Widget {
                pos: node.to(),
                widget,
                block,
            });
        } else {
            self.out.push(Decoration::Replace {
                range: node.range(),
                widget,
                block,
            });
        }
    }

    /// `heading-decorations.ts`: the line holding a heading's start.
    /// Also code blocks (`codeFenceExtension.ts:28-112`) and quotes
    /// (`blockQuote.ts:86-120`).
    fn line(&mut self, node: Node<'_>) {
        if let Some(level) = node.kind().heading_level() {
            let line_start = self.text.line_at(node.from()).from;
            self.out.push(Decoration::Line {
                line_start,
                style: LineStyle::Heading(level),
            });
            return;
        }
        if node.kind() == NodeKind::Frontmatter && !self.touches(node.range()) {
            return;
        }
        if self.mermaid_source(node).is_some() {
            return;
        }
        if let Some(block) = crate::code::describe(self.text, node) {
            let (first, last) = (self.text.line_at(node.from()), self.text.line_at(node.to()));
            for n in first.number..=last.number {
                let line = self.text.line(n);
                self.out.push(Decoration::Line {
                    line_start: line.from,
                    style: LineStyle::Code {
                        first: n == first.number,
                        last: n == last.number,
                    },
                });
            }
            self.out.push(Decoration::Widget {
                pos: first.from,
                widget: Widget::CodeInfo {
                    label: block.label,
                    code: block.code,
                },
                block: false,
            });
            return;
        }
        // The outermost quote draws a bar on each of its lines; a nested
        // quote adds one at its inner `>`.
        if node.kind() == NodeKind::Blockquote
            && !node.ancestors().any(|a| a.kind() == NodeKind::Blockquote)
        {
            let (first, last) = (
                self.text.line_at(node.from()).number,
                self.text.line_at(node.to()).number,
            );
            for n in first..=last {
                self.out.push(Decoration::Line {
                    line_start: self.text.line(n).from,
                    style: LineStyle::Quote,
                });
            }
            for mark in node
                .descendants()
                .filter(|d| d.kind() == NodeKind::QuoteMark)
            {
                if mark.from() != self.text.line_at(mark.from()).from {
                    self.out.push(Decoration::Widget {
                        pos: mark.from(),
                        widget: Widget::NestedQuoteBorder,
                        block: false,
                    });
                }
            }
        }
    }

    /// Every tab is drawn at a fixed width.
    fn tabs(&mut self, range: Range<usize>) {
        let start = range.start.min(self.text.len());
        let end = range.end.min(self.text.len()).max(start);
        let bytes = &self.text.as_str().as_bytes()[start..end];
        for pos in memchr::memchr_iter(b'\t', bytes).map(|pos| start + pos) {
            self.out.push(Decoration::Replace {
                range: pos..pos + 1,
                widget: Widget::Tab,
                block: false,
            });
        }
    }
}

/// JavaScript's `\s`, which CodeMirror's categorizer uses for spaces.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Vim's WORD around `pos` on its line: the run of non-space characters
/// (`utils.ts` `stateWORDAt`). `None` when `pos` sits between two spaces.
pub(crate) fn word_at(text: &Text, pos: usize) -> Option<Range<usize>> {
    let line = text.line_at(pos);
    let content = text.slice(line.range());
    let at = pos - line.from;
    let start = content[..at]
        .char_indices()
        .rev()
        .find(|(_, c)| is_js_space(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = content[at..]
        .char_indices()
        .find(|(_, c)| is_js_space(*c))
        .map_or(content.len(), |(i, _)| at + i);
    (start != end).then(|| line.from + start..line.from + end)
}

/// Writer's `normalizeMarkdownDestination` (`lib/paths.ts:41-62`).
pub fn normalize_markdown_destination(destination: &str) -> String {
    normalize_destination(destination)
}

pub(crate) fn normalize_destination(destination: &str) -> String {
    let mut normalized = destination.trim_matches(is_js_space);
    if normalized.len() >= 2 && normalized.starts_with('<') && normalized.ends_with('>') {
        normalized = normalized[1..normalized.len() - 1].trim_matches(is_js_space);
    }
    const ESCAPABLE: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~ \t";
    let mut out = String::with_capacity(normalized.len());
    let mut chars = normalized.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && ESCAPABLE.contains(next)
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// The emoji for a `:name:` shortcode (node-emoji 2.2.0 `get`, over the
/// emojilib table in `emoji.tsv`).
pub fn emoji(name: &str) -> Option<&'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        include_str!("emoji.tsv")
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    });
    // node-emoji's `normalizeName`: a name matching `/:.+:/` loses its first
    // and last characters.
    let wrapped = match (name.find(':'), name.rfind(':')) {
        (Some(first), Some(last)) => name[first + 1..last].chars().next().is_some(),
        _ => false,
    };
    let name = if wrapped {
        let mut chars = name.chars();
        chars.next();
        chars.next_back();
        chars.as_str()
    } else {
        name
    };
    table.get(name).copied()
}

#[cfg(test)]
mod tests;
