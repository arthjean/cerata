//! tree-sitter-md side of the spike.
//!
//! `TsDoc` is the two-level orchestration Cerata would own: one incremental
//! block tree, plus one inline tree per `inline` / `pipe_table_cell` node,
//! stored in coordinates relative to that node. After an edit only the block
//! tree is reparsed incrementally and only the inline nodes that intersect the
//! edit or the block tree's changed ranges are reparsed; every other inline
//! tree is reused untouched (only its start offset shifts).
//!
//! `TsNaive` is the crate's own `MarkdownParser`, which edits every inline
//! tree and calls the inline parser once per inline node on every reparse.

use tree_sitter::{InputEdit, Node, Parser, Point, Range, Tree};
use tree_sitter_md::{MarkdownParser, MarkdownTree};

pub struct Inline {
    /// Absolute byte offset of the host node in the current text.
    pub start: usize,
    pub end: usize,
    /// Inline tree in coordinates relative to `start`.
    pub tree: Tree,
}

pub struct TsDoc {
    pub text: String,
    block_parser: Parser,
    inline_parser: Parser,
    pub block: Tree,
    pub inlines: Vec<Inline>,
    virtual_newline: bool,
    /// Block-tree share of the last `reparse` (edit + incremental parse).
    pub last_block_time: std::time::Duration,
}

/// One node of the combined tree, in absolute coordinates.
#[derive(Debug, Clone)]
pub struct TsNode {
    pub kind: String,
    pub start: usize,
    pub end: usize,
    pub named: bool,
    pub depth: usize,
    pub inline: bool,
    pub parent_kind: String,
}

fn is_inline_host(kind: &str) -> bool {
    kind == "inline" || kind == "pipe_table_cell"
}

fn relative_point(point: Point, base: Point) -> Point {
    if point.row == base.row {
        Point::new(0, point.column - base.column)
    } else {
        Point::new(point.row - base.row, point.column)
    }
}

/// Same included ranges as `MarkdownParser`: the host node minus its named
/// children (block continuations such as list indentation or `> `).
fn inline_ranges(node: Node<'_>) -> Vec<Range> {
    let mut range = node.range();
    let mut ranges = Vec::new();
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.is_named() {
                let child_range = child.range();
                ranges.push(Range {
                    start_byte: range.start_byte,
                    start_point: range.start_point,
                    end_byte: child_range.start_byte,
                    end_point: child_range.start_point,
                });
                range.start_byte = child_range.end_byte;
                range.start_point = child_range.end_point;
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    ranges.push(range);
    ranges
}

/// Parser input callback. With `virtual_newline` the parser sees one extra
/// `\n` past the end of the buffer: tree-sitter-md's block scanner does not
/// treat EOF as a line end (setext underlines and the closing frontmatter
/// fence on the last line are missed, `text\n- ` yields an ERROR node).
fn read(bytes: &[u8], i: usize, virtual_newline: bool) -> &[u8] {
    if i < bytes.len() {
        &bytes[i..]
    } else if virtual_newline && i == bytes.len() {
        b"\n"
    } else {
        &[]
    }
}

fn parse_inline(
    parser: &mut Parser,
    text: &str,
    node: Node<'_>,
    virtual_newline: bool,
) -> Result<Tree, String> {
    let base_byte = node.start_byte();
    let base_point = node.start_position();
    let ranges: Vec<Range> = inline_ranges(node)
        .into_iter()
        .map(|r| Range {
            start_byte: r.start_byte - base_byte,
            end_byte: r.end_byte - base_byte,
            start_point: relative_point(r.start_point, base_point),
            end_point: relative_point(r.end_point, base_point),
        })
        .collect();
    parser
        .set_included_ranges(&ranges)
        .map_err(|e| format!("included ranges: {e:?}"))?;
    let bytes = &text.as_bytes()[base_byte..];
    parser
        .parse_with_options(&mut |i, _| read(bytes, i, virtual_newline), None, None)
        .ok_or_else(|| "inline parse failed".to_string())
}

/// Inline host nodes whose range intersects `[from, to]` (inclusive, so an
/// insertion at the very end of an inline node still hits it).
fn collect_hosts<'t>(node: Node<'t>, from: usize, to: usize, out: &mut Vec<Node<'t>>) {
    if node.end_byte() < from || node.start_byte() > to {
        return;
    }
    if is_inline_host(node.kind()) {
        out.push(node);
        return;
    }
    let mut cursor = node.walk();
    // `goto_first_child_for_byte` returns None when the byte falls in a gap
    // between children (a blank line), hence the linear fallback.
    if cursor
        .goto_first_child_for_byte(from.saturating_sub(1))
        .is_none()
        && !cursor.goto_first_child()
    {
        return;
    }
    loop {
        let child = cursor.node();
        if child.start_byte() > to {
            break;
        }
        collect_hosts(child, from, to, out);
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Row/column of a byte offset. Linear scan: a rope gives this in O(log n),
/// so callers keep it outside the timed region.
pub fn point_at(text: &str, byte: usize) -> Point {
    let before = &text.as_bytes()[..byte];
    let row = before.iter().filter(|&&b| b == b'\n').count();
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    Point::new(row, byte - line_start)
}

/// Applies a text replacement and returns the matching `InputEdit`.
pub fn edit_text(text: &mut String, start: usize, old_end: usize, new: &str) -> InputEdit {
    let start_position = point_at(text, start);
    let old_end_position = point_at(text, old_end);
    text.replace_range(start..old_end, new);
    let new_end = start + new.len();
    InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position,
        old_end_position,
        new_end_position: point_at(text, new_end),
    }
}

impl TsDoc {
    pub fn new(text: String, virtual_newline: bool) -> Result<Self, String> {
        let mut block_parser = Parser::new();
        block_parser
            .set_language(&tree_sitter_md::LANGUAGE.into())
            .map_err(|e| e.to_string())?;
        let mut inline_parser = Parser::new();
        inline_parser
            .set_language(&tree_sitter_md::INLINE_LANGUAGE.into())
            .map_err(|e| e.to_string())?;
        let bytes = text.as_bytes();
        let block = block_parser
            .parse_with_options(&mut |i, _| read(bytes, i, virtual_newline), None, None)
            .ok_or("block parse failed")?;
        let mut hosts = Vec::new();
        collect_hosts(block.root_node(), 0, usize::MAX, &mut hosts);
        let mut inlines = Vec::with_capacity(hosts.len());
        for host in hosts {
            inlines.push(Inline {
                start: host.start_byte(),
                end: host.end_byte(),
                tree: parse_inline(&mut inline_parser, &text, host, virtual_newline)?,
            });
        }
        Ok(Self {
            text,
            block_parser,
            inline_parser,
            block,
            inlines,
            virtual_newline,
            last_block_time: std::time::Duration::ZERO,
        })
    }

    /// Reparses after `self.text` was changed as described by `edit`.
    /// Returns the number of inline trees that were reparsed.
    pub fn reparse(&mut self, edit: &InputEdit) -> Result<usize, String> {
        let started = std::time::Instant::now();
        let mut old = self.block.clone();
        old.edit(edit);
        let bytes = self.text.as_bytes();
        let nl = self.virtual_newline;
        let block = self
            .block_parser
            .parse_with_options(&mut |i, _| read(bytes, i, nl), Some(&old), None)
            .ok_or("block reparse failed")?;
        self.last_block_time = started.elapsed();

        // Drop inline trees the edit touched, shift the ones after it.
        let delta = edit.new_end_byte as isize - edit.old_end_byte as isize;
        let lo = self.inlines.partition_point(|e| e.end < edit.start_byte);
        let hi = self
            .inlines
            .partition_point(|e| e.start <= edit.old_end_byte);
        self.inlines.drain(lo..hi.max(lo));
        for entry in &mut self.inlines[lo..] {
            entry.start = (entry.start as isize + delta) as usize;
            entry.end = (entry.end as isize + delta) as usize;
        }

        // Dirty region in new coordinates: the edit plus whatever the block
        // structure changed.
        let mut from = edit.start_byte;
        let mut to = edit.new_end_byte;
        for range in old.changed_ranges(&block) {
            from = from.min(range.start_byte);
            to = to.max(range.end_byte);
        }
        let mut hosts = Vec::new();
        collect_hosts(block.root_node(), from, to, &mut hosts);
        if let (Some(first), Some(last)) = (hosts.first(), hosts.last()) {
            from = from.min(first.start_byte());
            to = to.max(last.end_byte());
        }
        let lo = self.inlines.partition_point(|e| e.end < from);
        let hi = self.inlines.partition_point(|e| e.start <= to);
        let mut fresh = Vec::with_capacity(hosts.len());
        for host in &hosts {
            fresh.push(Inline {
                start: host.start_byte(),
                end: host.end_byte(),
                tree: parse_inline(&mut self.inline_parser, &self.text, *host, nl)?,
            });
        }
        let reparsed = fresh.len();
        self.inlines.splice(lo..hi.max(lo), fresh);
        self.block = block;
        Ok(reparsed)
    }

    /// Structural fingerprint (every node with its absolute range, plus the
    /// inline tree index) used to check incremental == fresh parse.
    pub fn fingerprint(&self) -> String {
        let mut out = String::new();
        for entry in &self.inlines {
            out.push_str(&format!("I {}..{}\n", entry.start, entry.end));
        }
        for n in self.nodes() {
            out.push_str(&format!("{} {} {}..{}\n", n.depth, n.kind, n.start, n.end));
        }
        out
    }

    /// Every node of the combined tree (block nodes, then inline nodes
    /// grafted under their host), in document order.
    pub fn nodes(&self) -> Vec<TsNode> {
        let mut out = Vec::new();
        self.walk(self.block.root_node(), 0, false, 0, "", &mut out);
        out
    }

    fn walk(
        &self,
        node: Node<'_>,
        base: usize,
        inline: bool,
        depth: usize,
        parent_kind: &str,
        out: &mut Vec<TsNode>,
    ) {
        let kind = node.kind();
        out.push(TsNode {
            kind: kind.to_string(),
            // Clamped: with the virtual newline the root ends one byte past
            // the buffer.
            start: (base + node.start_byte()).min(self.text.len()),
            end: (base + node.end_byte()).min(self.text.len()),
            named: node.is_named(),
            depth,
            inline,
            parent_kind: parent_kind.to_string(),
        });
        if !inline && is_inline_host(kind) {
            let start = node.start_byte();
            if let Ok(i) = self.inlines.binary_search_by_key(&start, |e| e.start) {
                let root = self.inlines[i].tree.root_node();
                let mut cursor = root.walk();
                if cursor.goto_first_child() {
                    loop {
                        self.walk(cursor.node(), start, true, depth + 1, kind, out);
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
            return;
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                self.walk(cursor.node(), base, inline, depth + 1, kind, out);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    pub fn has_error(&self) -> bool {
        self.block.root_node().has_error()
            || self.inlines.iter().any(|e| e.tree.root_node().has_error())
    }
}

/// The crate's `MarkdownParser` wrapper, used as the naive baseline.
pub struct TsNaive {
    pub text: String,
    parser: MarkdownParser,
    tree: MarkdownTree,
}

impl TsNaive {
    pub fn new(text: String) -> Result<Self, String> {
        let mut parser = MarkdownParser::default();
        let tree = parser
            .parse(text.as_bytes(), None)
            .ok_or("MarkdownParser failed")?;
        Ok(Self { text, parser, tree })
    }

    pub fn reparse(&mut self, edit: &InputEdit) -> Result<(), String> {
        self.tree.edit(edit);
        self.tree = self
            .parser
            .parse(self.text.as_bytes(), Some(&self.tree))
            .ok_or("MarkdownParser reparse failed")?;
        Ok(())
    }
}
