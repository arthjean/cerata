use std::ops::Range;

use crate::change::Change;
use crate::history::UserEvent;
use crate::selection::{Selection, SelectionRange};
use crate::state::{EditorState, Transaction};
use crate::text::Line;
use crate::tree::NodeKind;

fn leading_ws(s: &str) -> &str {
    &s[..s.len() - s.trim_start_matches([' ', '\t']).len()]
}

fn is_empty_quote_line(s: &str) -> bool {
    let mut rest = s;
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let spaces = rest
            .bytes()
            .take(3)
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        let after = &rest[spaces..];
        let Some(after) = after.strip_prefix('>') else {
            return false;
        };
        let Some(after) = after.strip_prefix([' ', '\t']) else {
            return false;
        };
        rest = after;
    }
    true
}

fn quote_prefix(s: &str) -> &str {
    let mut end = 0;
    loop {
        let rest = &s[end..];
        let spaces = rest
            .bytes()
            .take(3)
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        if !rest[spaces..].starts_with('>') {
            return &s[..end];
        }
        end += spaces + 1;
        if s[end..].starts_with([' ', '\t']) {
            end += 1;
        }
    }
}

impl EditorState {
    fn in_kind(&self, pos: usize, kind: NodeKind) -> bool {
        let node = self.tree().resolve(pos, 1);
        std::iter::once(node)
            .chain(node.ancestors())
            .any(|n| n.kind() == kind)
    }

    fn line_of(&self, pos: usize) -> (Line, &str) {
        let line = self.text().line_at(pos);
        (line, self.text().slice(line.range()))
    }

    pub fn blockquote_enter(&mut self) -> bool {
        if self.is_loading() || self.selection().ranges().len() != 1 {
            return false;
        }
        let main = self.selection().main();
        let (line, s) = self.line_of(main.head);
        if !main.is_empty() || main.head != line.to || !is_empty_quote_line(s) {
            return false;
        }
        if !self.in_kind(line.from, NodeKind::Blockquote) {
            return false;
        }
        let keep = s.rfind('>').unwrap_or(0);
        let insert = s[..keep].to_owned();
        let caret = line.from + insert.len();
        let transaction = Transaction::new()
            .change(Change::new(line.range(), insert))
            .select(Selection::cursor(caret))
            .event(UserEvent::Delete)
            .scroll();
        self.apply(transaction).is_ok()
    }

    fn continue_markup(&mut self) -> bool {
        if self.selection().ranges().len() != 1 || !self.selection().main().is_empty() {
            return false;
        }
        let head = self.selection().main().head;
        let (line, s) = self.line_of(head);
        let in_quote = self.in_kind(line.from, NodeKind::Blockquote);
        let quote = if in_quote { quote_prefix(s) } else { "" };
        let rest = &s[quote.len()..];
        let indent = leading_ws(rest);
        let marker = &rest[indent.len()..];
        let digits = marker.bytes().take_while(u8::is_ascii_digit).count();
        let delimiter = marker.as_bytes().get(digits).copied();
        let ordered = (1..=9).contains(&digits)
            && matches!(delimiter, Some(b'.' | b')'))
            && marker
                .as_bytes()
                .get(digits + 1)
                .is_some_and(|b| *b == b' ' || *b == b'\t')
            && self.in_kind(
                line.from + quote.len() + indent.len(),
                NodeKind::OrderedList,
            );
        let mut changes = Vec::new();
        let continuation = if ordered {
            let body = quote.len() + indent.len() + digits + 2;
            if head < line.from + body {
                return false;
            }
            let number: usize = marker[..digits].parse().unwrap_or(1);
            let delimiter = char::from(delimiter.unwrap_or(b'.'));
            let sep = &marker[digits + 1..digits + 2];
            changes.extend(self.renumber_after(line, number + 1));
            format!("{quote}{indent}{}{delimiter}{sep}", number + 1)
        } else if !quote.is_empty() && head >= line.from + quote.len() {
            let bullet = crate::list::parse_bullet_task_line(0, rest);
            match bullet {
                Some(b) if head >= line.from + quote.len() + b.body_from => {
                    let mark = &rest[b.indent_len..b.indent_len + 2];
                    if b.is_task {
                        let sep = &mark[1..];
                        format!("{quote}{indent}{mark}[ ]{sep}")
                    } else {
                        format!("{quote}{indent}{mark}")
                    }
                }
                _ => quote.to_owned(),
            }
        } else {
            return false;
        };
        changes.push(Change::insert(head, format!("\n{continuation}")));
        let caret = head + 1 + continuation.len();
        let transaction = Transaction::new()
            .changes(changes)
            .select(Selection::cursor(caret))
            .event(UserEvent::Command)
            .scroll();
        self.apply(transaction).is_ok()
    }

    fn renumber_after(&self, line: Line, next: usize) -> Vec<Change> {
        let tree = self.tree();
        let Some(mark) = tree
            .iter_range(line.from..line.to)
            .find(|n| n.kind() == NodeKind::ListMark && n.from() >= line.from)
        else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        let mut expected = next + 1;
        let mut item = mark.parent().and_then(|i| i.next_sibling());
        while let Some(current) = item.filter(|i| i.kind() == NodeKind::ListItem) {
            let Some(m) = current.child(NodeKind::ListMark) else {
                break;
            };
            let s = self.text().slice(m.range());
            let digits = s.bytes().take_while(u8::is_ascii_digit).count();
            let Ok(number) = s[..digits].parse::<usize>() else {
                break;
            };
            if number >= expected {
                break;
            }
            changes.push(Change::new(
                m.from()..m.from() + digits,
                expected.to_string(),
            ));
            expected += 1;
            item = current.next_sibling();
        }
        changes
    }

    pub fn type_text(&mut self, range: Option<Range<usize>>, text: &str, event: UserEvent) -> bool {
        if self.is_loading() {
            return false;
        }
        if let Some(done) = self.type_over_cells(text, event) {
            return done;
        }
        let ranges: Vec<SelectionRange> = match range {
            Some(range) => vec![SelectionRange::new(range.start, range.end)],
            None => self.selection().ranges().to_vec(),
        };
        let mut changes = Vec::new();
        let mut carets = Vec::new();
        let mut shift: isize = 0;
        for r in &ranges {
            let (change, caret) = match crate::cells::cell_edit(self, r.range(), text) {
                crate::cells::CellEdit::Edit(change) => {
                    let caret = change.insert.len();
                    (change, caret)
                }
                crate::cells::CellEdit::Refused => {
                    let moved = |pos: usize| (pos as isize + shift).max(0) as usize;
                    carets.push(SelectionRange::new(moved(r.anchor), moved(r.head)));
                    continue;
                }
                crate::cells::CellEdit::Outside => {
                    let typed = match r.is_empty() {
                        true => crate::atomic::typed_at(self, r.head, text),
                        false => None,
                    };
                    let (insert, caret) = typed.unwrap_or_else(|| (text.to_owned(), text.len()));
                    (Change::new(r.range(), insert), caret)
                }
            };
            let end = change.from as isize + shift + caret as isize;
            carets.push(SelectionRange::cursor(end.max(0) as usize));
            shift += change.insert.len() as isize - (change.to - change.from) as isize;
            changes.push(change);
        }
        if changes.is_empty() {
            return true;
        }
        let main = self.selection().main_index().min(carets.len() - 1);
        self.apply(
            Transaction::new()
                .changes(changes)
                .select(Selection::new(carets, main))
                .event(event)
                .scroll(),
        )
        .is_ok()
    }

    pub fn insert_newline(&mut self) -> bool {
        if self.is_loading() {
            return false;
        }
        if self.blockquote_enter()
            || self.list_enter()
            || self.leave_indented_blank()
            || self.continue_markup()
        {
            return true;
        }
        let text = self.text();
        let changes: Vec<Change> = self
            .selection()
            .ranges()
            .iter()
            .map(|r| {
                let line = text.line_at(r.from());
                let before = text.slice(line.from..r.from());
                let indent = leading_ws(before);
                Change::new(r.range(), format!("\n{indent}"))
            })
            .collect();
        self.replace_ranges(changes, UserEvent::Type)
    }

    pub fn delete_backward(&mut self) -> bool {
        if self.is_loading() {
            return false;
        }
        if let Some(done) = self.clear_cells() {
            return done;
        }
        let s = self.doc();
        let cut: Vec<std::ops::Range<usize>> = self
            .selection()
            .ranges()
            .iter()
            .map(|r| {
                if !r.is_empty() {
                    return r.range();
                }
                let pos = r.head;
                let width = if s.get(pos.saturating_sub(2)..pos) == Some("\r\n") {
                    2
                } else {
                    s[..pos].chars().next_back().map_or(0, char::len_utf8)
                };
                pos - width..pos
            })
            .collect();
        if self.guard_atomic_deletes(&cut) || self.guard_table_joins(&cut) {
            return true;
        }
        if self.list_backspace() {
            return true;
        }
        let changes: Vec<Change> = self.cell_cuts(&cut).into_iter().flatten().collect();
        if changes.is_empty() {
            return true;
        }
        self.replace_ranges(changes, UserEvent::Delete)
    }

    pub fn indent(&mut self) -> bool {
        if self.is_loading() {
            return false;
        }
        if self.list_indent() {
            return true;
        }
        let changes = self
            .selection()
            .ranges()
            .iter()
            .map(|r| Change::new(r.range(), "\t"))
            .collect();
        self.replace_ranges(changes, UserEvent::Type)
    }

    pub fn outdent(&mut self) -> bool {
        if self.is_loading() {
            return false;
        }
        self.list_outdent()
    }

    pub(crate) fn replace_ranges(&mut self, changes: Vec<Change>, event: UserEvent) -> bool {
        let mut sorted = changes.clone();
        sorted.sort_by_key(|c| c.from);
        let mut shift: isize = 0;
        let mut carets = Vec::with_capacity(sorted.len());
        for change in &sorted {
            let end = change.from as isize + shift + change.insert.len() as isize;
            carets.push(SelectionRange::cursor(end.max(0) as usize));
            shift += change.insert.len() as isize - (change.to - change.from) as isize;
        }
        let main = self
            .selection()
            .main_index()
            .min(carets.len().saturating_sub(1));
        let transaction = Transaction::new()
            .changes(changes)
            .select(Selection::new(carets, main))
            .event(event)
            .scroll();
        self.apply(transaction).is_ok()
    }

    pub fn move_line_boundary(
        &mut self,
        forward: bool,
        extend: bool,
        visual_line: &dyn Fn(usize) -> Range<usize>,
    ) -> bool {
        self.boundary(forward, extend, Some(visual_line))
    }

    pub fn move_line_boundary_unwrapped(&mut self, forward: bool, extend: bool) -> bool {
        self.boundary(forward, extend, None)
    }

    fn boundary(
        &mut self,
        forward: bool,
        extend: bool,
        visual_line: Option<&dyn Fn(usize) -> Range<usize>>,
    ) -> bool {
        let text = self.text();
        let ranges: Vec<SelectionRange> = self
            .selection()
            .ranges()
            .iter()
            .map(|r| {
                let line = text.line_at(r.head);
                let row = visual_line.map_or_else(|| line.range(), |f| f(r.head));
                if let Some((grid, ro, co)) = crate::cells::cell_at(self, r.head)
                    && let Some(cell) = grid.cell(ro, co)
                {
                    let to = if forward { row.end } else { row.start };
                    let moved = to.clamp(cell.content.start, cell.content.end);
                    return if extend {
                        SelectionRange::new(r.anchor, moved)
                    } else {
                        SelectionRange::cursor(moved)
                    };
                }
                let mut moved = if forward { row.end } else { row.start };
                let wrong_way = if forward {
                    moved < r.head
                } else {
                    moved > r.head
                };
                let target = if forward { line.to } else { line.from };
                if wrong_way || (moved == r.head && moved != target) {
                    moved = target;
                }
                if !forward && moved == line.from && line.to > line.from {
                    let space = leading_ws(text.slice(line.range())).len();
                    if space > 0 && r.head != line.from + space {
                        moved = line.from + space;
                    }
                }
                let moved = text.floor_char_boundary(moved.min(text.len()));
                if extend {
                    SelectionRange::new(r.anchor, moved)
                } else {
                    SelectionRange::cursor(moved)
                }
            })
            .collect();
        let selection = Selection::new(ranges, self.selection().main_index());
        if selection != *self.selection() {
            let transaction = Transaction::new()
                .select(selection)
                .event(UserEvent::Select)
                .scroll();
            let _ = self.apply(transaction);
        }
        true
    }
}

#[cfg(test)]
mod tests;
