use std::ops::Range;

use crate::motion::Vertical;
use crate::selection::{Selection, SelectionRange};
use crate::state::EditorState;
use crate::text::Text;
use crate::tree::{MarkdownTree, Node, NodeKind};

pub fn mermaid_body(text: &Text, node: Node<'_>) -> Option<String> {
    if node.kind() != NodeKind::FencedCode {
        return None;
    }
    let info = node.child(NodeKind::CodeInfo)?;
    if !text
        .slice(info.range())
        .trim()
        .to_ascii_lowercase()
        .starts_with("mermaid")
    {
        return None;
    }
    let marks = node
        .children()
        .filter(|c| c.kind() == NodeKind::CodeMark)
        .count();
    if marks < 2 {
        return None;
    }
    let block = crate::code::describe(text, node)?;
    let body = block.code.trim();
    (!body.is_empty()).then(|| body.to_owned())
}

pub fn block_at(tree: &MarkdownTree, text: &Text, pos: usize) -> Option<Range<usize>> {
    let node = tree.resolve(pos, 1);
    std::iter::once(node)
        .chain(node.ancestors())
        .find(|n| n.kind() == NodeKind::FencedCode)
        .filter(|n| mermaid_body(text, *n).is_some())
        .map(|n| n.range())
        .or_else(|| {
            let node = tree.resolve(pos, -1);
            std::iter::once(node)
                .chain(node.ancestors())
                .find(|n| n.kind() == NodeKind::FencedCode && n.to() == pos)
                .filter(|n| mermaid_body(text, *n).is_some())
                .map(|n| n.range())
        })
}

pub fn block_inside(tree: &MarkdownTree, text: &Text, pos: usize) -> Option<Range<usize>> {
    block_at(tree, text, pos).filter(|b| b.start < pos && pos < b.end)
}

pub fn skip(
    tree: &MarkdownTree,
    text: &Text,
    selection: Selection,
    previous: &Selection,
) -> Selection {
    let mut changed = false;
    let ranges: Vec<SelectionRange> = selection
        .ranges()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let before = previous
                .ranges()
                .get(i)
                .map_or(previous.main().head, |p| p.head);
            let head = match block_inside(tree, text, r.head) {
                Some(b) if r.head >= before => b.end,
                Some(b) => b.start,
                None => r.head,
            };
            let anchor = match block_inside(tree, text, r.anchor) {
                Some(_) if r.is_empty() => head,
                Some(b) if head >= b.end => b.start,
                Some(b) => b.end,
                None => r.anchor,
            };
            let out = SelectionRange::new(anchor, head);
            changed |= out != *r;
            out
        })
        .collect();
    if !changed {
        return selection;
    }
    Selection::new(ranges, selection.main_index())
}

pub fn step_over(state: &EditorState, direction: Vertical) -> Option<usize> {
    if !state.atomic_blocks() {
        return None;
    }
    let head = state.selection().main().head;
    let text = state.text();
    let block = block_at(state.tree(), text, head)?;
    match direction {
        Vertical::Down if head == block.start => Some(block.end),
        Vertical::Up if head == block.end => Some(block.start),
        Vertical::Down if head == block.end => {
            let line = text.line_at(head);
            (line.number + 1 < text.line_count()).then(|| text.line(line.number + 1).from)
        }
        _ => None,
    }
}

pub fn typed_at(state: &EditorState, pos: usize, typed: &str) -> Option<(String, usize)> {
    if !state.atomic_blocks() || typed.is_empty() || typed.contains(['\n', '\r']) {
        return None;
    }
    let text = state.text();
    let block = block_at(state.tree(), text, pos)?;
    let eol = if text.slice(block.clone()).contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if pos == block.start {
        Some((format!("{typed}{eol}"), typed.len()))
    } else if pos == block.end {
        Some((format!("{eol}{typed}"), eol.len() + typed.len()))
    } else {
        None
    }
}

impl EditorState {
    pub fn guard_atomic_deletes(&mut self, ranges: &[Range<usize>]) -> bool {
        if !self.atomic_blocks() {
            return false;
        }
        let (tree, text) = (self.tree(), self.text());
        let cut = |r: &Range<usize>| -> Option<Range<usize>> {
            let block = block_inside(tree, text, r.start)
                .or_else(|| block_inside(tree, text, r.end))
                .or_else(|| {
                    block_at(tree, text, r.start).filter(|b| r.start < b.end && r.end > b.start)
                })?;
            (!(r.start <= block.start && block.end <= r.end)).then_some(block)
        };
        let blocks: Vec<Option<Range<usize>>> = ranges.iter().map(cut).collect();
        if blocks.iter().all(Option::is_none) {
            return false;
        }
        let selection = self.selection().clone();
        let selected: Vec<SelectionRange> = selection
            .ranges()
            .iter()
            .zip(&blocks)
            .map(|(r, b)| match b {
                Some(b) => SelectionRange::new(b.start, b.end),
                None => *r,
            })
            .collect();
        let _ = self.apply(
            crate::state::Transaction::new()
                .select(Selection::new(selected, selection.main_index()))
                .event(crate::history::UserEvent::Select)
                .scroll(),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::history::UserEvent;
    use crate::state::Transaction;

    const DOC: &str = "intro\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\nend";

    fn fence() -> Range<usize> {
        DOC.find("```").unwrap()..DOC.rfind("```").unwrap() + 3
    }

    fn state(caret: usize) -> EditorState {
        let mut s = EditorState::new(DOC);
        s.select(Selection::cursor(caret)).unwrap();
        s
    }

    #[test]
    fn only_closed_non_empty_mermaid_fences_are_atomic() {
        let s = EditorState::new(DOC);
        let f = fence();
        assert_eq!(block_at(s.tree(), s.text(), f.start), Some(f.clone()));
        assert_eq!(block_at(s.tree(), s.text(), f.end), Some(f.clone()));
        assert_eq!(block_inside(s.tree(), s.text(), f.start), None);
        assert_eq!(block_inside(s.tree(), s.text(), f.start + 4), Some(f));
        for doc in [
            "```mermaid\nflowchart LR\n  A --> B\n",
            "```mermaid\n\n```\n",
            "```rust\nfn main() {}\n```\n",
        ] {
            let s = EditorState::new(doc);
            assert_eq!(block_at(s.tree(), s.text(), 4), None, "{doc:?}");
        }
    }

    #[test]
    fn a_caret_never_lands_inside_a_block() {
        let f = fence();
        let mut s = state(f.start);
        s.select(Selection::cursor(f.start + 1)).unwrap();
        assert_eq!(s.selection().main().head, f.end);
        s.select(Selection::cursor(f.end - 1)).unwrap();
        assert_eq!(s.selection().main().head, f.start);
        s.select(Selection::single(0, f.start + 5)).unwrap();
        assert_eq!(s.selection().main(), SelectionRange::new(0, f.end));
        s.select(Selection::single(f.start + 5, DOC.len())).unwrap();
        assert_eq!(
            s.selection().main(),
            SelectionRange::new(f.start, DOC.len())
        );
        s.select(Selection::single(f.start + 5, 0)).unwrap();
        assert_eq!(s.selection().main(), SelectionRange::new(f.end, 0));
    }

    #[test]
    fn up_and_down_step_over_a_block() {
        let f = fence();
        let next_line = f.end + 1;
        let s = state(f.start);
        assert_eq!(step_over(&s, Vertical::Down), Some(f.end));
        assert_eq!(step_over(&s, Vertical::Up), None);
        let s = state(f.end);
        assert_eq!(step_over(&s, Vertical::Up), Some(f.start));
        assert_eq!(step_over(&s, Vertical::Down), Some(next_line));
        assert_eq!(step_over(&state(0), Vertical::Down), None);
    }

    #[test]
    fn a_deletion_that_cuts_a_block_selects_it_first() {
        let f = fence();
        let one = |r: Range<usize>| vec![r];
        let mut s = state(f.end);
        assert!(s.guard_atomic_deletes(&one(f.end - 1..f.end)));
        assert_eq!(s.selection().main(), SelectionRange::new(f.start, f.end));
        assert!(!s.guard_atomic_deletes(std::slice::from_ref(&f)));
        let mut s = state(f.start);
        assert!(s.guard_atomic_deletes(&one(f.start..f.start + 1)));
        assert_eq!(s.selection().main(), SelectionRange::new(f.start, f.end));
        let mut s = state(f.end + 1);
        assert!(!s.guard_atomic_deletes(&one(f.end..f.end + 1)));
    }

    #[test]
    fn text_typed_at_a_boundary_takes_its_own_line() {
        let f = fence();
        let s = state(f.start);
        assert_eq!(typed_at(&s, f.start, "x"), Some(("x\n".into(), 1)));
        assert_eq!(typed_at(&s, f.end, "x"), Some(("\nx".into(), 2)));
        assert_eq!(typed_at(&s, 0, "x"), None);
        assert_eq!(typed_at(&s, f.start, "\n"), None);
    }

    #[test]
    fn an_edit_keeps_the_caret_out_of_a_block() {
        let open = "```mermaid\nflowchart LR\n  A --> B\n``";
        let mut s = EditorState::new(open);
        s.select(Selection::cursor(open.len())).unwrap();
        s.apply(
            Transaction::new()
                .change(Change::insert(open.len(), "`"))
                .select(Selection::cursor(open.len() + 1))
                .event(UserEvent::Type),
        )
        .unwrap();
        assert_eq!(s.selection().main().head, open.len() + 1);
    }
}
