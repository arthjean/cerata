//! Windowed reparse: ADR 0002's fallback when a full reparse misses the
//! budget (US-010: 2 ms on the 3 000-item list, 16 ms on 1 MiB).
//!
//! Only a window of blocks around the edit is reparsed: the top-level blocks
//! it touches plus one neighbor on each side, or, inside one big list, the
//! items it touches plus one neighbor. One more block (or item) on each side
//! is reparsed as a sentinel and must come out identical to the old one:
//! the parse then resynchronized before and after the window, so the rest
//! of the old tree is still exact and is kept, shifted. Whenever a guard
//! fails, the whole document is parsed. The result is always the tree a
//! full parse would build (checked by the fuzz test against full parses).

use super::{Elt, MarkdownTree, NONE, NodeData, NodeKind, blocks};
use crate::change::ChangeSet;

/// The unclosed-`$$` block (`blocks::unclosed_display_math`) depends on the
/// last `$$` of the document, which an edit without any `$` can move to a
/// line start (a line break typed before it): a window never builds that
/// block, so when `tree` (windowed, of `text`) should hold one, a full
/// parse must. A wiki link over the `$$` is undecided here: full parse too.
fn opens_unclosed_math(tree: &MarkdownTree, text: &str) -> bool {
    let Some(pos) = text.rfind("$$") else {
        return false;
    };
    let indent = &text[line_start(text, pos)..pos];
    if indent.len() >= 4 || !indent.bytes().all(|b| b == b' ') {
        return false;
    }
    let Some(paragraph) = tree
        .root()
        .children()
        .find(|b| b.from() <= pos && pos < b.to().max(b.from() + 1))
    else {
        return false;
    };
    if paragraph.kind() != NodeKind::Paragraph {
        return false;
    }
    let mut covering = paragraph
        .children()
        .filter(|c| c.from() <= pos && pos < c.to());
    covering
        .next()
        .is_none_or(|c| c.kind() == NodeKind::WikiLink)
}

fn line_start(text: &str, pos: usize) -> usize {
    memchr::memrchr2(b'\n', b'\r', &text.as_bytes()[..pos.min(text.len())]).map_or(0, |i| i + 1)
}

fn line_end(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    memchr::memchr2(b'\n', b'\r', &text.as_bytes()[pos..]).map_or(text.len(), |i| pos + i)
}

fn shift(pos: usize, delta: isize) -> usize {
    (pos as isize + delta) as usize
}

impl MarkdownTree {
    /// The tree of `text`, which `changes` made from `old_text` (the text of
    /// this tree). Equal to `MarkdownTree::parse(text)`, usually cheaper.
    pub fn reparse(&self, old_text: &str, text: &str, changes: &ChangeSet) -> MarkdownTree {
        self.reparse_window(old_text, text, changes)
            .unwrap_or_else(|| MarkdownTree::parse(text))
    }

    /// The windowed reparse alone: `None` when the edit needs a full parse.
    pub fn reparse_window(
        &self,
        old_text: &str,
        text: &str,
        changes: &ChangeSet,
    ) -> Option<MarkdownTree> {
        let tree = self.reparse_window_unchecked(old_text, text, changes)?;
        (!opens_unclosed_math(&tree, text)).then_some(tree)
    }

    fn reparse_window_unchecked(
        &self,
        old_text: &str,
        text: &str,
        changes: &ChangeSet,
    ) -> Option<MarkdownTree> {
        let first = changes.changes().first()?;
        let last = changes.changes().last()?;
        if self.root().to() != old_text.len() {
            return None;
        }
        let delta = text.len() as isize - old_text.len() as isize;
        // Writer's unclosed-`$$` rule and frontmatter look at the whole
        // document: any `$` edited, or an edit on the first line or inside
        // the frontmatter, reparses everything.
        let dollar = changes.changes().iter().any(|c| {
            c.insert.contains('$') || old_text.get(c.from..c.to).is_none_or(|s| s.contains('$'))
        });
        let frontmatter = self.root().child(NodeKind::Frontmatter);
        let protected = frontmatter.map_or_else(|| line_end(old_text, 0), |f| f.to()) + 1;
        // A `<details>` block spans blank lines (`blocks::merge_details`).
        if dollar
            || first.from <= protected
            || self.root().last_child()?.kind() == NodeKind::Math
            || blocks::has_details(text)
            || blocks::has_details(old_text)
        {
            return None;
        }
        if frontmatter.is_none()
            && text.starts_with("---")
            && crate::frontmatter::detect(text).is_some()
        {
            return None;
        }
        let body = frontmatter
            .map_or(0, |f| line_end(old_text, f.to()) + 1)
            .min(old_text.len());
        let (span_from, span_to) = (
            line_start(old_text, first.from),
            line_end(old_text, last.to),
        );
        let tops: Vec<u32> = self
            .root()
            .children()
            .filter(|c| c.kind() != NodeKind::Frontmatter)
            .map(|c| c.id)
            .collect();
        if tops.is_empty() {
            return None;
        }
        let node = |id: u32| super::Node { tree: self, id };
        // Blocks touching the edited lines; an edit between two blocks
        // touches both neighbors.
        let i = tops
            .iter()
            .position(|&id| node(id).to() >= span_from)
            .unwrap_or(tops.len() - 1);
        let j = tops
            .iter()
            .rposition(|&id| node(id).from() <= span_to)
            .unwrap_or(0);
        let (i, j) = (i.min(j), i.max(j));
        if i == j
            && matches!(
                node(tops[i]).kind(),
                NodeKind::BulletList | NodeKind::OrderedList
            )
            && let Some(tree) = self.reparse_items(
                old_text,
                text,
                delta,
                tops[i],
                i + 1 == tops.len(),
                span_from,
                span_to,
            )
        {
            return Some(tree);
        }
        let lo = i.checked_sub(2);
        let hi = (j + 2 < tops.len()).then_some(j + 2);
        let window_from = lo.map_or(body, |k| line_start(old_text, node(tops[k]).from()));
        let window_to = hi.map_or(old_text.len(), |k| line_end(old_text, node(tops[k]).to()));
        let new_to = shift(window_to, delta);
        let mut window = blocks::parse_window(text, window_from, new_to);
        if let Some(k) = lo
            && !window.first().is_some_and(|e| same(node(tops[k]), e, 0))
        {
            return None;
        }
        if let Some(k) = hi
            && !window.last().is_some_and(|e| same(node(tops[k]), e, delta))
        {
            return None;
        }
        wrap_window(text, &mut window, window_from..new_to);
        let start = lo.map_or(tops[0], |k| tops[k]);
        let end = hi.map_or(self.nodes.len() as u32, |k| node(tops[k]).data().end);
        let previous = node(start).prev_sibling().map_or(NONE, |n| n.id);
        Some(self.splice(text, delta, start, end, 0, previous, &window, None))
    }

    /// The same inside one list: `list` is the only block touched.
    #[allow(clippy::too_many_arguments)]
    fn reparse_items(
        &self,
        old_text: &str,
        text: &str,
        delta: isize,
        list: u32,
        list_is_last: bool,
        span_from: usize,
        span_to: usize,
    ) -> Option<MarkdownTree> {
        let node = |id: u32| super::Node { tree: self, id };
        let items: Vec<u32> = node(list).children().map(|c| c.id).collect();
        let k = items.iter().position(|&id| node(id).to() >= span_from)?;
        let l = items
            .iter()
            .rposition(|&id| node(id).from() <= span_to)?
            .max(k);
        let lo = k.checked_sub(2)?;
        let hi = (l + 2 < items.len()).then_some(l + 2);
        if hi.is_none() && !list_is_last {
            return None;
        }
        let window_from = line_start(old_text, node(items[lo]).from());
        let window_to = hi.map_or(old_text.len(), |h| line_end(old_text, node(items[h]).to()));
        let new_to = shift(window_to, delta);
        let mut window = blocks::parse_window(text, window_from, new_to);
        let [parsed] = window.as_mut_slice() else {
            return None;
        };
        if parsed.kind != node(list).kind()
            || !parsed
                .children
                .first()
                .is_some_and(|e| same(node(items[lo]), e, 0))
        {
            return None;
        }
        if let Some(h) = hi
            && !parsed
                .children
                .last()
                .is_some_and(|e| same(node(items[h]), e, delta))
        {
            return None;
        }
        let list_to = if hi.is_some() {
            shift(node(list).to(), delta)
        } else {
            parsed.to
        };
        let mut new_items = std::mem::take(&mut parsed.children);
        wrap_window(text, &mut new_items, window_from..new_to);
        let start = items[lo];
        let end = hi.map_or(node(list).data().end, |h| node(items[h]).data().end);
        let previous = node(start).prev_sibling().map_or(NONE, |n| n.id);
        Some(self.splice(
            text,
            delta,
            start,
            end,
            list,
            previous,
            &new_items,
            Some(list_to),
        ))
    }

    /// Old nodes before `start`, then `window` (children of `parent`), then
    /// the old nodes from `end` on, shifted.
    #[allow(clippy::too_many_arguments)]
    fn splice(
        &self,
        text: &str,
        delta: isize,
        start: u32,
        end: u32,
        parent: u32,
        previous: u32,
        window: &[Elt],
        parent_to: Option<usize>,
    ) -> MarkdownTree {
        let mut tree = MarkdownTree {
            nodes: Vec::with_capacity(self.nodes.len() + 64),
        };
        tree.nodes.extend_from_slice(&self.nodes[..start as usize]);
        let mut prev = previous;
        for elt in window {
            let id = tree.flatten(elt, parent);
            if prev != NONE {
                tree.nodes[prev as usize].next_sibling = id;
            }
            tree.nodes[id as usize].prev_sibling = prev;
            prev = id;
        }
        let d = tree.nodes.len() as i64 - end as i64;
        let moved = |index: u32| -> u32 {
            if index == NONE || (index as i64) < start as i64 {
                index
            } else {
                (index as i64 + d) as u32
            }
        };
        let first_after = tree.nodes.len() as u32;
        for data in &self.nodes[end as usize..] {
            tree.nodes.push(NodeData {
                kind: data.kind,
                from: shift(data.from, delta),
                to: shift(data.to, delta),
                parent: moved(data.parent),
                end: moved(data.end),
                next_sibling: moved(data.next_sibling),
                prev_sibling: moved(data.prev_sibling),
            });
        }
        // The window's last node links to the first old node after it.
        if prev != NONE {
            let sibling = tree
                .nodes
                .get(first_after as usize)
                .is_some_and(|n| n.parent == parent);
            tree.nodes[prev as usize].next_sibling = if sibling { first_after } else { NONE };
        }
        // Ancestors of the window grow with it.
        let total = tree.nodes.len() as u32;
        if parent != 0 {
            let old = &self.nodes[parent as usize];
            let (old_end, old_next) = (old.end, old.next_sibling);
            let list = &mut tree.nodes[parent as usize];
            list.end = moved(old_end);
            list.next_sibling = moved(old_next);
            if let Some(to) = parent_to {
                list.to = to;
            }
        }
        let root = &mut tree.nodes[0];
        root.end = total;
        root.to = text.len();
        tree
    }
}

/// The old subtree at `old` equals `elt` shifted by `delta`.
fn same(old: super::Node<'_>, elt: &Elt, delta: isize) -> bool {
    old.kind() == elt.kind
        && shift(old.from(), delta) == elt.from
        && shift(old.to(), delta) == elt.to
        && old.children().count() == elt.children.len()
        && old
            .children()
            .zip(&elt.children)
            .all(|(o, e)| same(o, e, delta))
}

fn wrap_window(text: &str, window: &mut Vec<Elt>, range: std::ops::Range<usize>) {
    let mut holder = Elt::with(
        NodeKind::Document,
        range.start,
        range.end,
        std::mem::take(window),
    );
    blocks::wrap_wiki_links_in(text, &mut holder, range);
    *window = holder.children;
}
