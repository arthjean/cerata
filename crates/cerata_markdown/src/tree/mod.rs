//! `MarkdownTree`: the syntax tree every live-preview rule reads.
//!
//! The tree has the shape of the Lezer tree Writer's rules were written
//! against (same node names, same ranges, marker nodes included), so the
//! rules port one to one and the tree is checked node for node against
//! Writer's own parser (`scripts/oracle`). Block structure comes from
//! pulldown-cmark (ADR 0002); inline content is parsed by a port of Lezer's
//! inline parser with Writer's extensions, which are written against it.
//!
//! The tree is rebuilt from the whole document after every edit, so it is
//! always complete: no rule ever sees an unparsed region.

mod blocks;
mod incremental;
mod inline;

use std::ops::Range;

/// Node types, named as in Lezer's Markdown grammar plus Writer's
/// extensions (`prosemark-core/markdown/*.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Document,
    Paragraph,
    /// `#` to `######`: level 1 to 6.
    AtxHeading(u8),
    /// `===` (1) or `---` (2) underline.
    SetextHeading(u8),
    HeaderMark,
    Blockquote,
    QuoteMark,
    BulletList,
    OrderedList,
    ListItem,
    ListMark,
    Task,
    TaskMarker,
    FencedCode,
    CodeBlock,
    CodeMark,
    CodeInfo,
    CodeText,
    HtmlBlock,
    CommentBlock,
    ProcessingInstructionBlock,
    LinkReference,
    LinkLabel,
    HorizontalRule,
    Table,
    TableHeader,
    TableRow,
    TableCell,
    TableDelimiter,
    Frontmatter,
    FrontmatterMark,
    FrontmatterContent,
    Math,
    MathMark,
    MathFormula,
    Escape,
    EscapeMark,
    Entity,
    InlineCode,
    HtmlTag,
    Comment,
    ProcessingInstruction,
    Autolink,
    Url,
    Emphasis,
    StrongEmphasis,
    EmphasisMark,
    Strikethrough,
    StrikethroughMark,
    HardBreak,
    Link,
    Image,
    LinkMark,
    LinkTitle,
    Emoji,
    EmojiMark,
    Dash,
    /// `[[target]]`, `[[target|alias]]` or `![[embed]]`, from Writer's
    /// regex (`wiki-link-extension.ts:41-50`). Not a Lezer node: it wraps
    /// the `Link` or `Image` Lezer builds for the same brackets.
    WikiLink,
}

impl NodeKind {
    /// The Lezer node name.
    pub fn name(self) -> &'static str {
        use NodeKind::*;
        match self {
            Document => "Document",
            Paragraph => "Paragraph",
            AtxHeading(1) => "ATXHeading1",
            AtxHeading(2) => "ATXHeading2",
            AtxHeading(3) => "ATXHeading3",
            AtxHeading(4) => "ATXHeading4",
            AtxHeading(5) => "ATXHeading5",
            AtxHeading(_) => "ATXHeading6",
            SetextHeading(1) => "SetextHeading1",
            SetextHeading(_) => "SetextHeading2",
            HeaderMark => "HeaderMark",
            Blockquote => "Blockquote",
            QuoteMark => "QuoteMark",
            BulletList => "BulletList",
            OrderedList => "OrderedList",
            ListItem => "ListItem",
            ListMark => "ListMark",
            Task => "Task",
            TaskMarker => "TaskMarker",
            FencedCode => "FencedCode",
            CodeBlock => "CodeBlock",
            CodeMark => "CodeMark",
            CodeInfo => "CodeInfo",
            CodeText => "CodeText",
            HtmlBlock => "HTMLBlock",
            CommentBlock => "CommentBlock",
            ProcessingInstructionBlock => "ProcessingInstructionBlock",
            LinkReference => "LinkReference",
            LinkLabel => "LinkLabel",
            HorizontalRule => "HorizontalRule",
            Table => "Table",
            TableHeader => "TableHeader",
            TableRow => "TableRow",
            TableCell => "TableCell",
            TableDelimiter => "TableDelimiter",
            Frontmatter => "Frontmatter",
            FrontmatterMark => "FrontmatterMark",
            FrontmatterContent => "FrontmatterContent",
            Math => "Math",
            MathMark => "MathMark",
            MathFormula => "MathFormula",
            Escape => "Escape",
            EscapeMark => "EscapeMark",
            Entity => "Entity",
            InlineCode => "InlineCode",
            HtmlTag => "HTMLTag",
            Comment => "Comment",
            ProcessingInstruction => "ProcessingInstruction",
            Autolink => "Autolink",
            Url => "URL",
            Emphasis => "Emphasis",
            StrongEmphasis => "StrongEmphasis",
            EmphasisMark => "EmphasisMark",
            Strikethrough => "Strikethrough",
            StrikethroughMark => "StrikethroughMark",
            HardBreak => "HardBreak",
            Link => "Link",
            Image => "Image",
            LinkMark => "LinkMark",
            LinkTitle => "LinkTitle",
            Emoji => "Emoji",
            EmojiMark => "EmojiMark",
            Dash => "Dash",
            WikiLink => "WikiLink",
        }
    }

    /// Heading level of an ATX or setext heading node.
    pub fn heading_level(self) -> Option<u8> {
        match self {
            NodeKind::AtxHeading(level) | NodeKind::SetextHeading(level) => Some(level),
            _ => None,
        }
    }

    /// Nodes whose content is code: never decorated as Markdown
    /// (`codeFenceScroll.ts:56-62` treats frontmatter as code too).
    pub fn is_code(self) -> bool {
        matches!(
            self,
            NodeKind::FencedCode
                | NodeKind::CodeBlock
                | NodeKind::InlineCode
                | NodeKind::CodeText
                | NodeKind::CodeInfo
                | NodeKind::Frontmatter
        )
    }
}

/// An element under construction, before it is flattened into the arena.
#[derive(Clone, Debug)]
pub(crate) struct Elt {
    pub kind: NodeKind,
    pub from: usize,
    pub to: usize,
    pub children: Vec<Elt>,
    /// A bullet list made by Writer's single-dash setext guard, which the
    /// following items of the same list join.
    pub dash_guard: bool,
}

impl Elt {
    pub fn new(kind: NodeKind, from: usize, to: usize) -> Self {
        Self {
            kind,
            from,
            to,
            children: Vec::new(),
            dash_guard: false,
        }
    }

    pub fn with(kind: NodeKind, from: usize, to: usize, children: Vec<Elt>) -> Self {
        Self {
            kind,
            from,
            to,
            children,
            dash_guard: false,
        }
    }
}

#[derive(Clone, Debug)]
struct NodeData {
    kind: NodeKind,
    from: usize,
    to: usize,
    parent: u32,
    /// Index one past the last node of this node's subtree (preorder).
    end: u32,
    next_sibling: u32,
    prev_sibling: u32,
}

const NONE: u32 = u32::MAX;

/// A wiki link found by Writer's pattern, outside code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLinkSpan {
    pub range: Range<usize>,
    pub embed: bool,
    /// The text between the brackets.
    pub inner: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct MarkdownTree {
    /// Nodes in preorder; index 0 is the document.
    nodes: Vec<NodeData>,
}

impl MarkdownTree {
    /// Parses the whole document. Never panics, whatever the input.
    pub fn parse(text: &str) -> Self {
        let root = blocks::parse_document(text);
        fn count(elt: &Elt) -> usize {
            1 + elt.children.iter().map(count).sum::<usize>()
        }
        let mut tree = Self {
            nodes: Vec::with_capacity(count(&root)),
        };
        tree.flatten(&root, NONE);
        tree
    }

    fn flatten(&mut self, elt: &Elt, parent: u32) -> u32 {
        let index = self.nodes.len() as u32;
        self.nodes.push(NodeData {
            kind: elt.kind,
            from: elt.from,
            to: elt.to,
            parent,
            end: index + 1,
            next_sibling: NONE,
            prev_sibling: NONE,
        });
        let mut previous = NONE;
        for child in &elt.children {
            let child_index = self.flatten(child, index);
            if previous != NONE {
                self.nodes[previous as usize].next_sibling = child_index;
                self.nodes[child_index as usize].prev_sibling = previous;
            }
            previous = child_index;
        }
        let end = self.nodes.len() as u32;
        self.nodes[index as usize].end = end;
        index
    }

    pub fn root(&self) -> Node<'_> {
        Node { tree: self, id: 0 }
    }

    /// Every node in document order (preorder), the document first.
    pub fn iter(&self) -> impl Iterator<Item = Node<'_>> + '_ {
        (0..self.nodes.len() as u32).map(move |id| Node { tree: self, id })
    }

    /// Nodes whose range touches `range` (inclusive), in preorder.
    pub fn iter_range(&self, range: Range<usize>) -> impl Iterator<Item = Node<'_>> + '_ {
        let mut id = 0u32;
        std::iter::from_fn(move || {
            while (id as usize) < self.nodes.len() {
                let data = &self.nodes[id as usize];
                if data.to < range.start || data.from > range.end {
                    id = data.end;
                    continue;
                }
                let node = Node { tree: self, id };
                id += 1;
                return Some(node);
            }
            None
        })
    }

    /// The innermost node containing `pos`, entering nodes that start or end
    /// exactly at it when `side` asks for it: `-1` prefers nodes ending at
    /// `pos`, `1` nodes starting at it, `0` either (Lezer's `resolveInner`).
    pub fn resolve(&self, pos: usize, side: i8) -> Node<'_> {
        let mut current = self.root();
        'descend: loop {
            for child in current.children() {
                let enter = match side {
                    s if s < 0 => child.from() < pos && pos <= child.to(),
                    s if s > 0 => child.from() <= pos && pos < child.to(),
                    _ => child.from() < pos && pos < child.to(),
                };
                if enter {
                    current = child;
                    continue 'descend;
                }
            }
            return current;
        }
    }

    /// Lezer-style dump, one `Name from..to` line per node, indented by depth.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.dump_node(self.root(), 0, &mut out);
        out
    }

    fn dump_node(&self, node: Node<'_>, depth: usize, out: &mut String) {
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{} {}..{}\n", node.name(), node.from(), node.to()));
        for child in node.children() {
            self.dump_node(child, depth + 1, out);
        }
    }

    /// Wiki links of the document, in order.
    pub fn wiki_links(&self) -> Vec<WikiLinkSpan> {
        self.iter()
            .filter(|n| n.kind() == NodeKind::WikiLink)
            .map(|n| {
                let embed = n.first_child().is_some_and(|c| c.kind() == NodeKind::Image);
                let open = if embed { 3 } else { 2 };
                WikiLinkSpan {
                    range: n.range(),
                    embed,
                    inner: n.from() + open..n.to().saturating_sub(2),
                }
            })
            .collect()
    }
}

/// A node of a `MarkdownTree`.
#[derive(Clone, Copy)]
pub struct Node<'t> {
    tree: &'t MarkdownTree,
    id: u32,
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.tree, other.tree) && self.id == other.id
    }
}

impl Eq for Node<'_> {}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}..{}", self.name(), self.from(), self.to())
    }
}

impl<'t> Node<'t> {
    fn data(&self) -> &'t NodeData {
        &self.tree.nodes[self.id as usize]
    }

    fn at(&self, id: u32) -> Option<Node<'t>> {
        (id != NONE).then_some(Node {
            tree: self.tree,
            id,
        })
    }

    pub fn kind(&self) -> NodeKind {
        self.data().kind
    }

    pub fn name(&self) -> &'static str {
        self.kind().name()
    }

    pub fn from(&self) -> usize {
        self.data().from
    }

    pub fn to(&self) -> usize {
        self.data().to
    }

    pub fn range(&self) -> Range<usize> {
        self.from()..self.to()
    }

    pub fn parent(&self) -> Option<Node<'t>> {
        self.at(self.data().parent)
    }

    pub fn first_child(&self) -> Option<Node<'t>> {
        let data = self.data();
        (data.end > self.id + 1).then_some(Node {
            tree: self.tree,
            id: self.id + 1,
        })
    }

    pub fn last_child(&self) -> Option<Node<'t>> {
        let mut child = self.first_child()?;
        while let Some(next) = child.next_sibling() {
            child = next;
        }
        Some(child)
    }

    pub fn next_sibling(&self) -> Option<Node<'t>> {
        self.at(self.data().next_sibling)
    }

    pub fn prev_sibling(&self) -> Option<Node<'t>> {
        self.at(self.data().prev_sibling)
    }

    pub fn children(&self) -> impl Iterator<Item = Node<'t>> + 't {
        let mut next = self.first_child();
        std::iter::from_fn(move || {
            let current = next?;
            next = current.next_sibling();
            Some(current)
        })
    }

    /// First direct child of `kind`.
    pub fn child(&self, kind: NodeKind) -> Option<Node<'t>> {
        self.children().find(|c| c.kind() == kind)
    }

    /// This node and every node below it, in preorder.
    pub fn descendants(&self) -> impl Iterator<Item = Node<'t>> + 't {
        let tree = self.tree;
        (self.id..self.data().end).map(move |id| Node { tree, id })
    }

    /// Ancestors from the parent up to the document.
    pub fn ancestors(&self) -> impl Iterator<Item = Node<'t>> + 't {
        let mut next = self.parent();
        std::iter::from_fn(move || {
            let current = next?;
            next = current.parent();
            Some(current)
        })
    }

    /// Writer's lineage path, `Document/…/Name` (`fold/core.ts:56-64`).
    pub fn path(&self) -> String {
        let mut names: Vec<&str> = self.ancestors().map(|n| n.name()).collect();
        names.reverse();
        names.push(self.name());
        names.join("/")
    }
}

#[cfg(test)]
mod tests;
