//! Blocks rendered as code: fenced and indented code, and the frontmatter
//! (`codeFenceScroll.ts:56-62`, `codeFenceExtension.ts:28-112`).

use std::ops::Range;

use crate::text::Text;
use crate::tree::{MarkdownTree, Node, NodeKind};

/// The label Writer shows on a frontmatter block.
pub const FRONTMATTER_LABEL: &str = "YAML (FRONTMATTER)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeBlockKind {
    Fenced,
    Indented,
    Frontmatter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlock {
    pub kind: CodeBlockKind,
    pub range: Range<usize>,
    /// The fence's info string as written (`rust`), empty when none.
    pub language: String,
    /// What the block's first line shows: the language in uppercase, or
    /// `YAML (FRONTMATTER)`.
    pub label: String,
    /// The code the copy button copies: without the fences, and without
    /// the four-space indent of indented code.
    pub code: String,
    /// Range of the code itself (between the fences).
    pub code_range: Range<usize>,
}

pub fn is_code_block_node(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::FencedCode | NodeKind::CodeBlock | NodeKind::Frontmatter
    )
}

/// The code block holding `pos`, the one ending at `pos` first, so a caret
/// right after a closing fence is still in its block (`codeBlockAt`).
pub fn code_block_at(tree: &MarkdownTree, pos: usize) -> Option<Node<'_>> {
    [-1, 1].into_iter().find_map(|side| {
        let node = tree.resolve(pos, side);
        std::iter::once(node)
            .chain(node.ancestors())
            .find(|n| is_code_block_node(n.kind()))
    })
}

pub fn describe(text: &Text, node: Node<'_>) -> Option<CodeBlock> {
    let kind = match node.kind() {
        NodeKind::FencedCode => CodeBlockKind::Fenced,
        NodeKind::CodeBlock => CodeBlockKind::Indented,
        NodeKind::Frontmatter => CodeBlockKind::Frontmatter,
        _ => return None,
    };
    let (language, label, code_range) = match kind {
        CodeBlockKind::Frontmatter => {
            let content = node
                .child(NodeKind::FrontmatterContent)
                .map_or(node.to()..node.to(), |c| c.range());
            (String::new(), FRONTMATTER_LABEL.to_owned(), content)
        }
        CodeBlockKind::Indented => (String::new(), String::new(), node.range()),
        CodeBlockKind::Fenced => {
            let language = node
                .child(NodeKind::CodeInfo)
                .map_or(String::new(), |i| text.slice(i.range()).to_owned());
            let label = language.to_uppercase();
            let first = text.line_at(node.from());
            let start = (first.to + 1).min(node.to());
            // The closing fence, when there is one on a later line. Writer
            // cut four bytes (`\n```), which truncates unclosed fences and
            // longer fences; this keeps exactly the content.
            let closing = node.last_child().filter(|c| {
                c.kind() == NodeKind::CodeMark && text.line_at(c.from()).number > first.number
            });
            let end = closing.map_or(node.to(), |c| {
                text.line_at(c.from()).from.saturating_sub(1).max(start)
            });
            let end = if closing.is_some()
                && text.as_str().get(end.saturating_sub(1)..end) == Some("\r")
            {
                end - 1
            } else {
                end
            };
            (language, label, start..end.max(start))
        }
    };
    let code = match kind {
        CodeBlockKind::Indented => text
            .slice(code_range.clone())
            .split('\n')
            .map(strip_indent)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => text.slice(code_range.clone()).to_owned(),
    };
    Some(CodeBlock {
        kind,
        range: node.range(),
        language,
        label,
        code,
        code_range,
    })
}

/// `^(?: {1,4}|\t)` removed.
fn strip_indent(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix('\t') {
        return rest;
    }
    let spaces = line.bytes().take(4).take_while(|b| *b == b' ').count();
    &line[spaces..]
}

/// Every code block of the document, in order.
pub fn code_blocks(tree: &MarkdownTree, text: &Text) -> Vec<CodeBlock> {
    tree.iter()
        .filter(|n| is_code_block_node(n.kind()))
        .filter_map(|n| describe(text, n))
        .collect()
}
