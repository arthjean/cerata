use std::ops::Range;

use crate::change::Change;
use crate::history::UserEvent;
use crate::selection::{Selection, SelectionRange};
use crate::state::{EditorState, Transaction};
use crate::text::Text;

pub const INDENT_UNIT: &str = "  ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    Word,
    Space,
    Other,
}

fn category(c: char) -> Category {
    if c.is_alphanumeric() || c == '_' {
        Category::Word
    } else if c.is_whitespace() {
        Category::Space
    } else {
        Category::Other
    }
}

pub fn char_step(text: &Text, pos: usize, forward: bool) -> usize {
    let s = text.as_str();
    if forward {
        if s.get(pos..pos + 2) == Some("\r\n") {
            return pos + 2;
        }
        s.get(pos..)
            .and_then(|rest| rest.chars().next())
            .map_or(pos, |c| pos + c.len_utf8())
    } else {
        if pos >= 2 && s.get(pos - 2..pos) == Some("\r\n") {
            return pos - 2;
        }
        s.get(..pos)
            .and_then(|head| head.chars().next_back())
            .map_or(pos, |c| pos - c.len_utf8())
    }
}

pub fn group_step(text: &Text, pos: usize, forward: bool) -> usize {
    let line = text.line_at(pos);
    if forward && pos >= line.to {
        return char_step(text, pos, true);
    }
    if !forward && pos <= line.from {
        return char_step(text, pos, false);
    }
    let content = text.slice(line.range());
    let at = pos - line.from;
    let mut current: Option<Category> = None;
    if forward {
        let mut end = at;
        for (i, c) in content[at..].char_indices() {
            let cat = category(c);
            match current {
                None | Some(Category::Space) => current = Some(cat),
                Some(group) if group != cat => break,
                _ => {}
            }
            end = at + i + c.len_utf8();
        }
        line.from + end
    } else {
        let mut start = at;
        for (i, c) in content[..at].char_indices().rev() {
            let cat = category(c);
            match current {
                None | Some(Category::Space) => current = Some(cat),
                Some(group) if group != cat => break,
                _ => {}
            }
            start = i;
        }
        line.from + start
    }
}

pub fn word_around(text: &Text, pos: usize) -> Option<Range<usize>> {
    let line = text.line_at(pos);
    let content = text.slice(line.range());
    let at = pos - line.from;
    let is_word = |c: char| category(c) == Category::Word;
    let start = content[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = content[at..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(content.len(), |(i, _)| at + i);
    (start < end).then(|| line.from + start..line.from + end)
}

fn line_blocks(state: &EditorState) -> Vec<(usize, usize)> {
    let text = state.text();
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for range in state.selection().ranges() {
        let first = text.line_at(range.from()).number;
        let mut last = text.line_at(range.to()).number;
        if last > first && text.line(last).from == range.to() {
            last -= 1;
        }
        match blocks.last_mut() {
            Some(block) if first <= block.1 + 1 => block.1 = block.1.max(last),
            _ => blocks.push((first, last)),
        }
    }
    blocks
}

impl EditorState {
    fn select_with(&mut self, selection: Selection) -> bool {
        if selection == *self.selection() {
            return true;
        }
        self.apply(
            Transaction::new()
                .select(selection)
                .event(UserEvent::Select)
                .scroll(),
        )
        .is_ok()
    }

    fn motion(
        &mut self,
        extend: bool,
        forward: bool,
        step: impl Fn(&Text, usize) -> usize,
    ) -> bool {
        let text = self.text();
        let ranges = self
            .selection()
            .ranges()
            .iter()
            .map(|r| {
                if extend {
                    SelectionRange::new(r.anchor, step(text, r.head))
                } else if !r.is_empty() {
                    SelectionRange::cursor(if forward { r.to() } else { r.from() })
                } else {
                    SelectionRange::cursor(step(text, r.head))
                }
            })
            .collect();
        let selection = Selection::new(ranges, self.selection().main_index());
        self.select_with(selection)
    }

    pub fn move_char(&mut self, forward: bool, extend: bool) -> bool {
        if self.list_arrow(forward, extend) {
            return true;
        }
        self.motion(extend, forward, |text, pos| char_step(text, pos, forward))
    }

    pub fn move_group(&mut self, forward: bool, extend: bool) -> bool {
        let cells = self.cell_contents();
        let heads: Vec<usize> = self.selection().ranges().iter().map(|r| r.head).collect();
        self.motion(extend, forward, |text, pos| {
            let to = group_step(text, pos, forward);
            let cell = heads
                .iter()
                .position(|h| *h == pos)
                .and_then(|i| cells.get(i).cloned().flatten());
            match cell {
                Some((c, _, _)) if forward && pos < c.end && to > c.end => c.end,
                Some((c, _, _)) if !forward && pos > c.start && to < c.start => c.start,
                Some((c, next, _)) if forward && pos == c.end => next,
                Some((c, _, prev)) if !forward && pos == c.start => prev,
                _ => to,
            }
        })
    }

    pub fn move_doc_boundary(&mut self, forward: bool, extend: bool) -> bool {
        let target = if forward { self.text().len() } else { 0 };
        let main = self.selection().main();
        let selection = if extend {
            Selection::single(main.anchor, target)
        } else {
            Selection::cursor(target)
        };
        self.select_with(selection)
    }

    pub fn select_all(&mut self) -> bool {
        let len = self.text().len();
        self.select_with(Selection::single(0, len))
    }

    pub fn delete_forward(&mut self) -> bool {
        self.delete_by(|text, pos| char_step(text, pos, true))
    }

    pub fn delete_group(&mut self, forward: bool) -> bool {
        self.delete_by(|text, pos| group_step(text, pos, forward))
    }

    fn delete_by(&mut self, step: impl Fn(&Text, usize) -> usize) -> bool {
        if self.is_loading() {
            return false;
        }
        if let Some(done) = self.clear_cells() {
            return done;
        }
        let text = self.text();
        let cut: Vec<std::ops::Range<usize>> = self
            .selection()
            .ranges()
            .iter()
            .map(|r| match r.is_empty() {
                true => {
                    let to = step(text, r.head);
                    r.head.min(to)..r.head.max(to)
                }
                false => r.range(),
            })
            .collect();
        if self.guard_atomic_deletes(&cut) || self.guard_table_joins(&cut) {
            return true;
        }
        let changes: Vec<Change> = self.cell_cuts(&cut).into_iter().flatten().collect();
        if changes.is_empty() {
            return true;
        }
        self.replace_ranges(changes, UserEvent::Delete)
    }

    pub fn move_lines(&mut self, up: bool) -> bool {
        if self.is_loading() {
            return false;
        }
        let text = self.text();
        let mut changes = Vec::new();
        let mut shifts: Vec<(Range<usize>, isize)> = Vec::new();
        for (first, last) in line_blocks(self) {
            let block = text.line(first).from..text.line(last).to;
            if up && first > 0 {
                let prev = text.line(first - 1);
                let br = text.slice(prev.to..block.start);
                let moved = format!(
                    "{}{br}{}",
                    text.slice(block.clone()),
                    text.slice(prev.range())
                );
                changes.push(Change::new(prev.from..block.end, moved));
                shifts.push((block, -((prev.range().len() + br.len()) as isize)));
            } else if !up && last + 1 < text.line_count() {
                let next = text.line(last + 1);
                let br = text.slice(block.end..next.from);
                let moved = format!(
                    "{}{br}{}",
                    text.slice(next.range()),
                    text.slice(block.clone())
                );
                changes.push(Change::new(block.start..next.to, moved));
                shifts.push((block, (next.range().len() + br.len()) as isize));
            }
        }
        if changes.is_empty() {
            return true;
        }
        let shift_of = |pos: usize| {
            shifts
                .iter()
                .find(|(block, _)| block.start <= pos && pos <= block.end)
                .map_or(0, |(_, shift)| *shift)
        };
        let moved = |pos: usize, shift: isize| (pos as isize + shift).max(0) as usize;
        let ranges = self
            .selection()
            .ranges()
            .iter()
            .map(|r| {
                let shift = shift_of(r.from());
                SelectionRange::new(moved(r.anchor, shift), moved(r.head, shift))
            })
            .collect();
        let selection = Selection::new(ranges, self.selection().main_index());
        self.apply(
            Transaction::new()
                .changes(changes)
                .select(selection)
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    pub fn copy_lines(&mut self, up: bool) -> bool {
        if self.is_loading() {
            return false;
        }
        let text = self.text();
        let mut changes = Vec::new();
        let mut shifts: Vec<(Range<usize>, isize)> = Vec::new();
        let mut inserted: isize = 0;
        for (first, last) in line_blocks(self) {
            let block = text.line(first).from..text.line(last).to;
            let br = if last + 1 < text.line_count() {
                text.slice(block.end..text.line(last + 1).from)
            } else if first > 0 {
                text.slice(text.line(first - 1).to..block.start)
            } else {
                "\n"
            };
            let copy = text.slice(block.clone());
            let added = (copy.len() + br.len()) as isize;
            changes.push(Change::insert(block.end, format!("{br}{copy}")));
            let own = if up { 0 } else { added };
            shifts.push((block, inserted + own));
            inserted += added;
        }
        let ranges = self
            .selection()
            .ranges()
            .iter()
            .map(|r| {
                let shift = shifts
                    .iter()
                    .find(|(block, _)| block.start <= r.from() && r.from() <= block.end)
                    .map_or(0, |(_, shift)| *shift);
                let at = |pos: usize| (pos as isize + shift).max(0) as usize;
                SelectionRange::new(at(r.anchor), at(r.head))
            })
            .collect();
        let selection = Selection::new(ranges, self.selection().main_index());
        self.apply(
            Transaction::new()
                .changes(changes)
                .select(selection)
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    pub fn delete_lines(&mut self) -> bool {
        if self.is_loading() {
            return false;
        }
        let text = self.text();
        let mut changes = Vec::new();
        let mut carets = Vec::new();
        let mut removed = 0usize;
        for (first, last) in line_blocks(self) {
            let from = text.line(first).from;
            let range = if last + 1 < text.line_count() {
                from..text.line(last + 1).from
            } else if first > 0 {
                text.line(first - 1).to..text.line(last).to
            } else {
                from..text.line(last).to
            };
            let caret = range.start - removed;
            let caret = if last + 1 >= text.line_count() && first > 0 {
                text.line(first - 1).from - removed
            } else {
                caret
            };
            removed += range.len();
            changes.push(Change::delete(range));
            carets.push(SelectionRange::cursor(caret));
        }
        let main = self
            .selection()
            .main_index()
            .min(carets.len().saturating_sub(1));
        self.apply(
            Transaction::new()
                .changes(changes)
                .select(Selection::new(carets, main))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    pub fn insert_blank_line(&mut self) -> bool {
        if self.is_loading() {
            return false;
        }
        let text = self.text();
        let mut seen = Vec::new();
        let changes: Vec<Change> = self
            .selection()
            .ranges()
            .iter()
            .filter_map(|r| {
                let line = text.line_at(r.to());
                if seen.contains(&line.number) {
                    return None;
                }
                seen.push(line.number);
                let content = text.slice(line.range());
                let indent =
                    &content[..content.len() - content.trim_start_matches([' ', '\t']).len()];
                Some(Change::insert(line.to, format!("\n{indent}")))
            })
            .collect();
        self.replace_ranges(changes, UserEvent::Command)
    }

    pub fn indent_lines(&mut self, more: bool) -> bool {
        if self.is_loading() {
            return false;
        }
        let text = self.text();
        let mut changes = Vec::new();
        for (first, last) in line_blocks(self) {
            for n in first..=last {
                let line = text.line(n);
                let content = text.slice(line.range());
                if more {
                    if !content.is_empty() || first == last {
                        changes.push(Change::insert(line.from, INDENT_UNIT));
                    }
                    continue;
                }
                let mut width = 0;
                for b in content.bytes() {
                    match b {
                        b' ' if width < INDENT_UNIT.len() => width += 1,
                        b'\t' if width == 0 => {
                            width = 1;
                            break;
                        }
                        _ => break,
                    }
                }
                if width > 0 {
                    changes.push(Change::delete(line.from..line.from + width));
                }
            }
        }
        if changes.is_empty() {
            return true;
        }
        self.apply(
            Transaction::new()
                .changes(changes)
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    pub fn select_next_occurrence(&mut self) -> bool {
        let text = self.text();
        if self
            .selection()
            .ranges()
            .iter()
            .any(SelectionRange::is_empty)
        {
            let ranges: Vec<SelectionRange> = self
                .selection()
                .ranges()
                .iter()
                .map(|r| match (r.is_empty(), word_around(text, r.head)) {
                    (true, Some(word)) => SelectionRange::new(word.start, word.end),
                    _ => *r,
                })
                .collect();
            let selection = Selection::new(ranges, self.selection().main_index());
            return self.select_with(selection);
        }
        let main = self.selection().main();
        let needle = text.slice(main.range());
        let doc = text.as_str();
        let last_end = self
            .selection()
            .ranges()
            .iter()
            .map(SelectionRange::to)
            .max()
            .unwrap_or(main.to());
        let taken = |from: usize| self.selection().ranges().iter().any(|r| r.from() == from);
        let found = doc[last_end..]
            .match_indices(needle)
            .map(|(i, _)| last_end + i)
            .chain(doc[..last_end].match_indices(needle).map(|(i, _)| i))
            .find(|from| !taken(*from));
        let Some(from) = found else {
            return true;
        };
        let mut ranges = self.selection().ranges().to_vec();
        ranges.push(SelectionRange::new(from, from + needle.len()));
        let main_index = ranges.len() - 1;
        let selection = Selection::new(ranges, main_index);
        let main_index = selection
            .ranges()
            .iter()
            .position(|r| r.from() == from)
            .unwrap_or(0);
        let selection = Selection::new(selection.ranges().to_vec(), main_index);
        self.select_with(selection)
    }

    pub fn select_all_occurrences(&mut self) -> bool {
        let text = self.text();
        let main = self.selection().main();
        let range = if main.is_empty() {
            match word_around(text, main.head) {
                Some(word) => word,
                None => return false,
            }
        } else {
            main.range()
        };
        let needle = text.slice(range.clone());
        let ranges: Vec<SelectionRange> = text
            .as_str()
            .match_indices(needle)
            .map(|(i, _)| SelectionRange::new(i, i + needle.len()))
            .collect();
        let main_index = ranges
            .iter()
            .position(|r| r.from() == range.start)
            .unwrap_or(0);
        self.select_with(Selection::new(ranges, main_index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(doc: &str, anchor: usize, head: usize) -> EditorState {
        let mut state = EditorState::with_filters(doc, Vec::new());
        state.select(Selection::single(anchor, head)).unwrap();
        state
    }

    fn heads(state: &EditorState) -> Vec<(usize, usize)> {
        state
            .selection()
            .ranges()
            .iter()
            .map(|r| (r.anchor, r.head))
            .collect()
    }

    #[test]
    fn group_motion_skips_spaces_then_one_category() {
        let text = Text::new("foo.bar  baz\nnext");
        assert_eq!(group_step(&text, 0, true), 3);
        assert_eq!(
            group_step(&text, 3, true),
            4,
            "punctuation is its own group"
        );
        assert_eq!(group_step(&text, 7, true), 12, "spaces then the word");
        assert_eq!(group_step(&text, 12, true), 13, "a line break is one step");
        assert_eq!(group_step(&text, 12, false), 9);
        assert_eq!(group_step(&text, 13, false), 12);
        assert_eq!(word_around(&text, 5), Some(4..7));
        assert_eq!(word_around(&text, 8), None);
    }

    #[test]
    fn char_and_word_motion_extend_and_collapse() {
        let mut s = state("héllo world", 0, 0);
        assert!(s.move_char(true, false));
        assert_eq!(heads(&s), [(1, 1)]);
        assert!(s.move_char(true, true));
        assert_eq!(heads(&s), [(1, 3)], "é is two bytes");
        assert!(s.move_char(false, false));
        assert_eq!(heads(&s), [(1, 1)], "a plain move collapses to the start");
        assert!(s.move_group(true, true));
        assert_eq!(heads(&s), [(1, 6)]);
        assert!(s.move_doc_boundary(true, false));
        assert_eq!(heads(&s), [(12, 12)]);
        assert!(s.delete_group(false));
        assert_eq!(s.doc(), "héllo ");
        s.select(Selection::cursor(0)).unwrap();
        assert!(s.delete_forward());
        assert_eq!(s.doc(), "éllo ");
    }

    #[test]
    fn lines_move_and_copy_with_the_selection() {
        let mut s = state("a\nb\nc", 2, 3);
        assert!(s.move_lines(true));
        assert_eq!(s.doc(), "b\na\nc");
        assert_eq!(heads(&s), [(0, 1)]);
        assert!(s.move_lines(false));
        assert!(s.move_lines(false));
        assert_eq!(s.doc(), "a\nc\nb");
        assert_eq!(heads(&s), [(4, 5)]);
        assert!(s.move_lines(false), "the last line stays, consumed");
        assert_eq!(s.doc(), "a\nc\nb");

        let mut s = state("one\ntwo", 1, 1);
        assert!(s.copy_lines(false));
        assert_eq!(s.doc(), "one\none\ntwo");
        assert_eq!(heads(&s), [(5, 5)], "on the lower copy");
        let mut s = state("one\ntwo", 5, 5);
        assert!(s.copy_lines(true));
        assert_eq!(s.doc(), "one\ntwo\ntwo");
        assert_eq!(heads(&s), [(5, 5)], "on the upper copy");
    }

    #[test]
    fn delete_lines_and_blank_line_below() {
        let mut s = state("a\nbb\nc", 3, 3);
        assert!(s.delete_lines());
        assert_eq!(s.doc(), "a\nc");
        assert_eq!(heads(&s), [(2, 2)]);
        let mut s = state("a\nb", 3, 3);
        assert!(s.delete_lines());
        assert_eq!(s.doc(), "a");
        assert_eq!(heads(&s), [(0, 0)]);

        let mut s = state("  item\nnext", 3, 3);
        assert!(s.insert_blank_line());
        assert_eq!(s.doc(), "  item\n  \nnext");
        assert_eq!(heads(&s), [(9, 9)]);
    }

    #[test]
    fn indent_more_and_less_by_one_unit() {
        let mut s = state("a\n b\n\tc", 0, 7);
        assert!(s.indent_lines(true));
        assert_eq!(s.doc(), "  a\n   b\n  \tc");
        assert!(s.indent_lines(false));
        assert!(s.indent_lines(false));
        assert_eq!(s.doc(), "a\nb\nc");
    }

    #[test]
    fn occurrences_select_word_then_next_then_all() {
        let mut s = state("cat dog cat cat", 1, 1);
        assert!(s.select_next_occurrence());
        assert_eq!(heads(&s), [(0, 3)]);
        assert!(s.select_next_occurrence());
        assert_eq!(heads(&s), [(0, 3), (8, 11)]);
        assert_eq!(s.selection().main().from(), 8);
        let mut s = state("cat dog cat cat", 9, 9);
        assert!(s.select_all_occurrences());
        assert_eq!(heads(&s), [(0, 3), (8, 11), (12, 15)]);
        assert_eq!(s.selection().main().from(), 8);
        assert!(s.select_all());
        assert_eq!(heads(&s), [(0, 15)]);
    }
}
