use crate::change::Change;
use crate::history::UserEvent;
use crate::selection::Selection;
use crate::state::{EditorState, Transaction};
use crate::tree::NodeKind;

impl EditorState {
    pub fn insert_images(&mut self, pos: usize, images: &[String]) -> bool {
        if self.is_loading() || images.is_empty() {
            return false;
        }
        let pos = self.text().floor_char_boundary(pos);
        if self.images_go_inline(pos) {
            return self.type_text(Some(pos..pos), &images.join(" "), UserEvent::Command);
        }
        self.insert_image_blocks(pos, images)
    }

    pub fn insert_image_blocks(&mut self, pos: usize, images: &[String]) -> bool {
        if self.is_loading() || images.is_empty() {
            return false;
        }
        let pos = self.text().floor_char_boundary(pos);
        let text = self.text();
        let blank = |s: &str| s.trim().is_empty();
        let line = text.line_at(pos);
        let (left, right) = text.slice(line.range()).split_at(pos - line.from);
        let previous_filled =
            line.number > 0 && !blank(text.slice(text.line(line.number - 1).range()));
        let next_blank = (line.number + 1 < text.line_count())
            .then(|| blank(text.slice(text.line(line.number + 1).range())));
        let after_line = if next_blank == Some(true) { "" } else { "\n" };
        let (range, before, after) = match (blank(left), blank(right)) {
            (true, true) => (
                line.range(),
                if previous_filled { "\n" } else { "" },
                after_line,
            ),
            (false, true) => (line.to..line.to, "\n\n", after_line),
            (true, false) => (
                line.from..line.from,
                if previous_filled { "\n" } else { "" },
                "\n\n",
            ),
            (false, false) => (pos..pos, "\n\n", "\n\n"),
        };
        let block = images.join("\n\n");
        let caret = range.start + before.len() + block.len() + 1;
        self.apply(
            Transaction::new()
                .change(Change::new(range, format!("{before}{block}{after}")))
                .select(Selection::cursor(caret))
                .event(UserEvent::Command)
                .scroll(),
        )
        .is_ok()
    }

    fn images_go_inline(&self, pos: usize) -> bool {
        if crate::cells::cell_at(self, pos).is_some() {
            return true;
        }
        [1, -1].into_iter().any(|side| {
            let node = self.tree().resolve(pos, side);
            std::iter::once(node).chain(node.ancestors()).any(|n| {
                matches!(
                    n.kind(),
                    NodeKind::Table | NodeKind::ListItem | NodeKind::Blockquote
                )
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(text: &str, images: &[&str]) -> String {
        let pos = text.find('^').unwrap();
        let mut state = EditorState::new(text.replacen('^', "", 1));
        let images: Vec<String> = images.iter().map(|s| (*s).to_owned()).collect();
        assert!(state.insert_images(pos, &images));
        let caret = state.selection().main().head;
        let mut out = state.doc().to_owned();
        out.insert(caret, '^');
        out
    }

    #[test]
    fn a_blank_line_takes_the_first_image() {
        assert_eq!(insert("^", &["![a](a.png)"]), "![a](a.png)\n^");
        assert_eq!(
            insert("Intro\n^\nNext", &["![a](a.png)", "![b](b.png)"]),
            "Intro\n\n![a](a.png)\n\n![b](b.png)\n^\nNext"
        );
        assert_eq!(
            insert("Intro\n\n^\n\nNext", &["![a](a.png)"]),
            "Intro\n\n![a](a.png)\n^\nNext"
        );
    }

    #[test]
    fn images_follow_a_line_ended_by_the_caret() {
        assert_eq!(
            insert("Intro^", &["![a](a.png)"]),
            "Intro\n\n![a](a.png)\n^"
        );
        assert_eq!(
            insert("Intro^\nNext", &["![a](a.png)"]),
            "Intro\n\n![a](a.png)\n^\nNext"
        );
    }

    #[test]
    fn the_line_is_split_at_the_caret() {
        assert_eq!(
            insert("Before^ after", &["![a](a.png)"]),
            "Before\n\n![a](a.png)\n^\n after"
        );
        assert_eq!(
            insert("Prev\n^Line", &["![a](a.png)"]),
            "Prev\n\n![a](a.png)\n^\nLine"
        );
    }

    #[test]
    fn lists_quotes_and_tables_take_images_inline() {
        assert_eq!(
            insert("- item^", &["![a](a.png)", "![b](b.png)"]),
            "- item![a](a.png) ![b](b.png)^"
        );
        assert_eq!(insert("> quote^", &["![a](a.png)"]), "> quote![a](a.png)^");
        let table = "| A | B |\n| --- | --- |\n| x^ | y |\n";
        let out = insert(table, &["![a|300](a.png)"]);
        assert!(out.contains(r"x![a\|300](a.png)^"), "{out}");
    }

    #[test]
    fn a_block_boundary_before_a_list_takes_blocks() {
        let mut state = EditorState::new(
            "Intro

- item
",
        );
        assert!(state.insert_image_blocks(7, &["![a](a.png)".to_owned()]));
        assert_eq!(
            state.doc(),
            "Intro

![a](a.png)

- item
"
        );
    }

    #[test]
    fn one_undo_removes_every_image() {
        let mut state = EditorState::new("Intro");
        let images = vec!["![a](a.png)".to_owned(), "![b](b.png)".to_owned()];
        assert!(state.insert_images(5, &images));
        assert!(state.undo());
        assert_eq!(state.doc(), "Intro");
    }

    #[test]
    fn a_tracked_position_follows_edits() {
        let mut state = EditorState::new("abc");
        let id = state.track(3);
        let _ = state.apply(Transaction::new().change(Change::insert(0, "xx")));
        let _ = state.apply(Transaction::new().change(Change::insert(5, "typed")));
        assert_eq!(state.untrack(id), Some(5), "stays before text typed at it");
        assert_eq!(state.untrack(id), None);
    }
}
