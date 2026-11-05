use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use crate::selection::Selection;
use crate::table::Grid;
use crate::text::Text;
use crate::tree::{MarkdownTree, Node, NodeKind};

pub const HEADING_SCALES: [f32; 6] = [1.6, 1.4, 1.2, 1.1, 1.0, 0.9];
pub const HEADING_WEIGHT: u16 = 600;
pub const TAB_WIDTH_CH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HideMode {
    Collapse,
    Remove,
    Block,
    KeepSpace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkStyle {
    Emphasis,
    InlineCode,
    Strikethrough,
    RenderedLink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LineStyle {
    Heading(u8),
    Code { first: bool, last: bool },
    Quote,
    TableSource { first: bool, last: bool },
}

impl LineStyle {
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Widget {
    Image {
        url: String,
        width: Option<u32>,
    },
    WikiImage {
        target: String,
        width: Option<u32>,
    },
    HorizontalRule,
    Dash(u8),
    Emoji(&'static str),
    CodeInfo {
        label: String,
        code: String,
    },
    NestedQuoteBorder,
    Tab,
    Table {
        range: Range<usize>,
        grid: Arc<Grid>,
    },
    WikiLink {
        display: String,
    },
    Frontmatter,
    Html {
        source: String,
    },
    Math {
        tex: String,
        display: bool,
    },
    Mermaid {
        source: String,
    },
}

impl Widget {
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
    Replace {
        range: Range<usize>,
        widget: Widget,
        block: bool,
    },
    Widget {
        pos: usize,
        widget: Widget,
        block: bool,
    },
    Line {
        line_start: usize,
        style: LineStyle,
    },
}

impl Decoration {
    fn rank(&self) -> u8 {
        match self {
            Decoration::Line { .. } => 0,
            Decoration::Mark { style, .. } => 1 + *style as u8,
            Decoration::Hide { mode, .. } => 8 + *mode as u8,
            Decoration::Widget { .. } => 16,
            Decoration::Replace { .. } => 17,
        }
    }

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

pub fn decorate(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
) -> Vec<Decoration> {
    decorate_range(tree, text, selection, drag_frozen, 0..text.len())
}

pub fn decorate_range(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
    range: Range<usize>,
) -> Vec<Decoration> {
    decorate_range_with(tree, text, selection, drag_frozen, range, None)
}

pub fn decorate_range_with(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
    range: Range<usize>,
    source_table: Option<usize>,
) -> Vec<Decoration> {
    decorate_nodes(
        tree,
        text,
        selection,
        drag_frozen,
        range,
        source_table,
        false,
    )
    .0
}

pub fn decorate_outside_drawn_tables(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
    range: Range<usize>,
    source_table: Option<usize>,
) -> (Vec<Decoration>, Vec<Range<usize>>) {
    decorate_nodes(
        tree,
        text,
        selection,
        drag_frozen,
        range,
        source_table,
        true,
    )
}

fn decorate_nodes(
    tree: &MarkdownTree,
    text: &Text,
    selection: &Selection,
    drag_frozen: Option<&Selection>,
    range: Range<usize>,
    source_table: Option<usize>,
    skip_drawn_tables: bool,
) -> (Vec<Decoration>, Vec<Range<usize>>) {
    let selection = drag_frozen.unwrap_or(selection).clamp(text);
    let mut cx = Cx {
        text,
        selection: &selection,
        source_table,
        out: Vec::new(),
        drawn_tables: HashMap::new(),
    };
    let mut skipped: Vec<Range<usize>> = Vec::new();
    for node in tree.iter_range(range.clone()) {
        if skipped
            .last()
            .is_some_and(|t| t.start <= node.from() && node.to() <= t.end)
        {
            continue;
        }
        cx.hide(node);
        cx.fold(node);
        cx.line(node);
        if skip_drawn_tables
            && node.kind() == NodeKind::Table
            && cx.drawn_tables.get(&node.from()) == Some(&true)
        {
            skipped.push(node.range());
        }
    }
    cx.tabs(range);
    let mut out = cx.out;
    out.sort_by_key(|d| {
        let range = d.range();
        (range.start, range.end, d.rank())
    });
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
    (unique, skipped)
}

struct Cx<'a> {
    text: &'a Text,
    selection: &'a Selection,
    source_table: Option<usize>,
    out: Vec<Decoration>,
    drawn_tables: HashMap<usize, bool>,
}

impl Cx<'_> {
    fn touches(&self, range: Range<usize>) -> bool {
        self.selection.touches(range)
    }

    fn object_selected(&self, range: Range<usize>) -> bool {
        let ranges = self.selection.ranges();
        let equal = |r: &crate::selection::SelectionRange| {
            !r.is_empty() && r.from() == range.start && r.to() == range.end
        };
        ranges.iter().any(equal)
            && !ranges
                .iter()
                .any(|r| !equal(r) && r.from() <= range.end && r.to() >= range.start)
    }

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
            K::Escape if self.text.slice(node.range()) == "\\|" && self.in_drawn_table(node) => {
                self.hide_descendants(node, &[K::EscapeMark], HideMode::Collapse);
            }
            K::Escape => {
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

    fn fold(&mut self, node: Node<'_>) {
        match node.kind() {
            NodeKind::Image => self.fold_image(node),
            NodeKind::Table => self.fold_table(node),
            NodeKind::WikiLink => self.fold_wiki_link(node),
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

    fn fold_wiki_link(&mut self, node: Node<'_>) {
        let bang = self.text.slice(node.range()).starts_with('!');
        let image: Option<crate::wiki::ImageEmbed> = bang
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
        let line = self.text.line_at(start);
        let block = start == line.from && end == line.to;
        let object = image.is_some() && block && self.object_selected(start..end);
        let inside = self
            .selection
            .ranges()
            .iter()
            .any(|r| r.from() >= start && r.to() <= end);
        if inside && !object {
            return;
        }
        if let Some(embed) = image {
            self.out.push(Decoration::Replace {
                range: start..end,
                widget: Widget::WikiImage {
                    target: embed.path,
                    width: embed.size.map(|s| s.width),
                },
                block,
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

    fn fold_table(&mut self, node: Node<'_>) {
        let Some(grid) = Grid::from_node(self.text, node) else {
            self.drawn_tables.insert(node.from(), false);
            return;
        };
        let revealed = self.table_revealed(&grid);
        self.drawn_tables.insert(node.from(), !revealed);
        if revealed {
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
                widget: Widget::Table {
                    range: node.range(),
                    grid: Arc::new(grid),
                },
                block: true,
            });
        }
    }

    fn in_drawn_table(&mut self, node: Node<'_>) -> bool {
        let Some(table) = node.ancestors().find(|n| n.kind() == NodeKind::Table) else {
            return false;
        };
        if let Some(drawn) = self.drawn_tables.get(&table.from()) {
            return *drawn;
        }
        let drawn = Grid::from_node(self.text, table).is_some_and(|g| !self.table_revealed(&g));
        self.drawn_tables.insert(table.from(), drawn);
        drawn
    }

    fn table_revealed(&self, grid: &Grid) -> bool {
        if self.source_table == Some(grid.range.start) {
            return true;
        }
        if grid.has_extra() {
            return self.touches(grid.range.clone());
        }
        let zone = grid.zone();
        self.selection.ranges().iter().any(|r| {
            let touches = r.from() <= zone.end && r.to() >= zone.start;
            touches
                && !grid
                    .drawn_cells()
                    .any(|(_, _, c)| c.holds_edit(r.from()) && c.holds_edit(r.to()))
        })
    }

    fn fold_image(&mut self, node: Node<'_>) {
        let Some(url) = node.children().find(|c| c.kind() == NodeKind::Url) else {
            return;
        };
        let line = self.text.line_at(node.from());
        let block = node.from() == line.from && node.to() == line.to;
        let object = block && self.object_selected(node.range());
        if self.touches(url.range()) && !object {
            return;
        }
        let destination = normalize_destination(self.text.slice(url.range()));
        if destination.is_empty() {
            return;
        }
        let width = crate::image_size::image_node(self.text, node).and_then(|i| i.width());
        let widget = Widget::Image {
            url: destination,
            width,
        };
        if self.touches(node.range()) && !object {
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

pub fn emoji(name: &str) -> Option<&'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        include_str!("emoji.tsv")
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    });
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
