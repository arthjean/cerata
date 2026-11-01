//! Editor state of one open file: text, selection, Markdown tree, undo
//! history and the scroll anchor its tab restores.
//!
//! Every open file owns its own `EditorState`, so undo history, caret and
//! scroll position are per file by construction (Writer resets a shared
//! history on tab swap, `use-prosemark-editor.ts:139-203`).

use crate::change::{Bias, Change, ChangeError, ChangeSet};
use crate::decorate::{Decoration, decorate, decorate_range};
use crate::history::{History, UserEvent};
use crate::selection::Selection;
use crate::text::Text;
use crate::tree::MarkdownTree;
use std::ops::Range;

/// A selection filter runs on every transaction that sets the selection,
/// after the document changes are applied, and may replace the selection
/// (CodeMirror's `transactionFilter` returning `{ selection, sequential:
/// true }`). Its output is in the new document and is clamped into it.
pub type SelectionFilter = fn(&EditorState, Selection) -> Selection;

/// Lines an edit replaced, for a renderer that keeps one item per line:
/// `old` lines starting at line `first` became `new` lines. Applied in order,
/// each splice is in the coordinates left by the previous ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSplice {
    pub first: usize,
    pub old: usize,
    pub new: usize,
}

/// Where a tab's viewport starts: the line holding `pos` and a pixel offset
/// into it. Mapped through every edit, so it survives changes above it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollAnchor {
    pub pos: usize,
    pub offset: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Transaction {
    pub changes: Vec<Change>,
    /// The selection after the transaction, in the new document. `None` maps
    /// the current selection through the changes.
    pub selection: Option<Selection>,
    pub user_event: UserEvent,
    pub add_to_history: bool,
    /// Milliseconds on any monotonic clock; only differences matter (undo
    /// grouping).
    pub time_ms: u64,
    pub scroll_into_view: bool,
}

impl Transaction {
    pub fn new() -> Self {
        Self {
            add_to_history: true,
            ..Self::default()
        }
    }

    pub fn change(mut self, change: Change) -> Self {
        self.changes.push(change);
        self
    }

    pub fn changes(mut self, changes: impl IntoIterator<Item = Change>) -> Self {
        self.changes.extend(changes);
        self
    }

    pub fn select(mut self, selection: Selection) -> Self {
        self.selection = Some(selection);
        self
    }

    pub fn event(mut self, event: UserEvent) -> Self {
        self.user_event = event;
        self
    }

    pub fn at(mut self, time_ms: u64) -> Self {
        self.time_ms = time_ms;
        self
    }

    pub fn without_history(mut self) -> Self {
        self.add_to_history = false;
        self
    }

    pub fn scroll(mut self) -> Self {
        self.scroll_into_view = true;
        self
    }
}

/// What an applied transaction did.
#[derive(Clone, Debug)]
pub struct Applied {
    /// The changes in the coordinates of the previous document; map
    /// external positions (anchors) through them.
    pub changes: ChangeSet,
    pub doc_changed: bool,
    pub selection_changed: bool,
    pub scroll_into_view: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransactionError {
    #[error(transparent)]
    Change(#[from] ChangeError),
    #[error("selection {anchor}..{head} is outside the new document (length {len})")]
    Selection {
        anchor: usize,
        head: usize,
        len: usize,
    },
}

/// A position that follows edits: map it through `Applied::changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub pos: usize,
    pub bias: Bias,
}

impl Anchor {
    pub fn new(pos: usize, bias: Bias) -> Self {
        Self { pos, bias }
    }

    pub fn map(self, changes: &ChangeSet) -> Self {
        Self {
            pos: changes.map_pos(self.pos, self.bias),
            bias: self.bias,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EditorState {
    text: Text,
    selection: Selection,
    tree: MarkdownTree,
    history: History,
    filters: Vec<SelectionFilter>,
    scroll: ScrollAnchor,
    /// Edits since the last save point.
    revision: u64,
    saved_revision: u64,
    loading: bool,
    /// The selection a pointer drag started from, while it is in flight.
    drag: Option<Selection>,
    /// Line splices since the last [`EditorState::take_line_splices`].
    splices: Vec<LineSplice>,
    /// The time stamped on transactions that carry none, for undo grouping.
    now_ms: u64,
}

impl EditorState {
    /// A loaded document with the default selection filters.
    pub fn new(text: impl Into<String>) -> Self {
        Self::with_filters(text, default_filters())
    }

    pub fn with_filters(text: impl Into<String>, filters: Vec<SelectionFilter>) -> Self {
        let text = Text::new(text);
        let tree = MarkdownTree::parse(text.as_str());
        Self {
            text,
            selection: Selection::cursor(0),
            tree,
            history: History::default(),
            filters,
            scroll: ScrollAnchor::default(),
            revision: 0,
            saved_revision: 0,
            loading: false,
            drag: None,
            splices: Vec::new(),
            now_ms: 0,
        }
    }

    /// A placeholder for a file still being read: empty, and editing
    /// commands refuse to run on it (`run_command` returns `false`).
    pub fn loading() -> Self {
        let mut state = Self::new("");
        state.loading = true;
        state
    }

    /// Replaces the document with content read from disk: not undoable, the
    /// caret kept where it was (clamped), and the history dropped, because
    /// its positions refer to the replaced text (Writer's `writer.reload`).
    pub fn reload(&mut self, content: impl Into<String>) {
        let head = self.selection.main().head;
        let old_lines = self.text.line_count();
        self.text = Text::new(content);
        self.splices.push(LineSplice {
            first: 0,
            old: old_lines,
            new: self.text.line_count(),
        });
        let pos = self.text.floor_char_boundary(head);
        self.selection = Selection::cursor(pos);
        self.tree = MarkdownTree::parse(self.text.as_str());
        self.history.clear();
        self.scroll.pos = self.text.floor_char_boundary(self.scroll.pos);
        self.revision += 1;
        self.saved_revision = self.revision;
        self.loading = false;
        self.drag = None;
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn doc(&self) -> &str {
        self.text.as_str()
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn tree(&self) -> &MarkdownTree {
        &self.tree
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Live-preview decorations for the current tree and selection, frozen
    /// on the drag's starting selection while a drag is in flight.
    pub fn decorations(&self) -> Vec<Decoration> {
        decorate(&self.tree, &self.text, &self.selection, self.drag.as_ref())
    }

    /// Decorations of the nodes touching `range` (inclusive): the subset of
    /// [`EditorState::decorations`] a renderer needs for the lines it shows.
    pub fn decorations_in(&self, range: Range<usize>) -> Vec<Decoration> {
        decorate_range(
            &self.tree,
            &self.text,
            &self.selection,
            self.drag.as_ref(),
            range,
        )
    }

    /// The current time, in ms on any monotonic clock: transactions without
    /// their own time stamp ([`Transaction::at`]) take it, so typing groups
    /// into undo steps by real time.
    pub fn set_time(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// Line splices made by every edit (and reload) since the last call.
    pub fn take_line_splices(&mut self) -> Vec<LineSplice> {
        std::mem::take(&mut self.splices)
    }

    /// A pointer drag starts: what is revealed stays as it is until
    /// [`EditorState::end_drag`], so text does not shift under the pointer
    /// (Writer's `drag-selection-gate.ts`).
    pub fn begin_drag(&mut self) {
        self.drag = Some(self.selection.clone());
    }

    /// The drag ends: decorations follow the live selection again.
    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    pub fn scroll_anchor(&self) -> ScrollAnchor {
        self.scroll
    }

    pub fn set_scroll_anchor(&mut self, anchor: ScrollAnchor) {
        self.scroll = ScrollAnchor {
            pos: self.text.floor_char_boundary(anchor.pos),
            offset: anchor.offset,
        };
    }

    /// Applies `tr` atomically: on error nothing changes.
    pub fn apply(&mut self, tr: Transaction) -> Result<Applied, TransactionError> {
        let changes = ChangeSet::new(&self.text, tr.changes)?;
        let mut text = self.text.clone();
        changes.apply(&mut text);
        let explicit = tr.selection.is_some();
        let selection = match tr.selection {
            Some(selection) => {
                if !selection.is_within(&text) {
                    let main = selection.main();
                    return Err(TransactionError::Selection {
                        anchor: main.anchor,
                        head: main.head,
                        len: text.len(),
                    });
                }
                selection
            }
            None => self.selection.map(&changes),
        };
        Ok(self.commit(
            text,
            changes,
            selection,
            explicit,
            tr.user_event,
            tr.add_to_history,
            if tr.time_ms == 0 {
                self.now_ms
            } else {
                tr.time_ms
            },
            tr.scroll_into_view,
        ))
    }

    /// Sets the selection alone (keyboard motion, pointer).
    pub fn select(&mut self, selection: Selection) -> Result<Applied, TransactionError> {
        self.apply(
            Transaction::new()
                .select(selection)
                .event(UserEvent::Select),
        )
    }

    pub fn undo(&mut self) -> bool {
        self.replay(false)
    }

    pub fn redo(&mut self) -> bool {
        self.replay(true)
    }

    fn replay(&mut self, redo: bool) -> bool {
        let taken = if redo {
            self.history.take_redo()
        } else {
            self.history.take_undo()
        };
        let Some(replay) = taken else {
            return false;
        };
        let inverse = replay.changes.invert(&self.text);
        let mut text = self.text.clone();
        replay.changes.apply(&mut text);
        let selection = replay.selection.clamp(&text);
        let event = if redo {
            UserEvent::Redo
        } else {
            UserEvent::Undo
        };
        self.commit(text, replay.changes, selection, true, event, false, 0, true);
        self.history.settle_replay(redo, inverse);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        text: Text,
        changes: ChangeSet,
        selection: Selection,
        explicit_selection: bool,
        event: UserEvent,
        add_to_history: bool,
        time_ms: u64,
        scroll_into_view: bool,
    ) -> Applied {
        let doc_changed = !changes.is_empty();
        let previous_text = std::mem::replace(&mut self.text, text);
        if doc_changed {
            push_splices(&mut self.splices, &previous_text, &self.text, &changes);
        }
        let previous_selection = self.selection.clone();
        if doc_changed {
            self.tree = self
                .tree
                .reparse(previous_text.as_str(), self.text.as_str(), &changes);
            self.scroll.pos = changes.map_pos(self.scroll.pos, Bias::Left);
            // A frozen drag is only remapped by edits.
            self.drag = self.drag.take().map(|frozen| frozen.map(&changes));
            self.revision += 1;
        }
        self.selection = selection;
        if explicit_selection {
            for filter in self.filters.clone() {
                let filtered = filter(self, self.selection.clone());
                self.selection = filtered.clamp(&self.text);
            }
        }
        if doc_changed && add_to_history {
            self.history.record(
                &previous_text,
                &self.text,
                &changes,
                &previous_selection,
                &self.selection,
                event,
                time_ms,
            );
        } else if doc_changed && !matches!(event, UserEvent::Undo | UserEvent::Redo) {
            // A change outside the history invalidates its positions.
            self.history.clear();
        } else if !doc_changed && self.selection != previous_selection {
            self.history.note_selection();
        }
        Applied {
            changes,
            doc_changed,
            selection_changed: self.selection != previous_selection,
            scroll_into_view,
        }
    }
}

/// The line splices of `changes` (from `before` to `after`), merged where
/// two changes touch the same line.
fn push_splices(out: &mut Vec<LineSplice>, before: &Text, after: &Text, changes: &ChangeSet) {
    // (old first, old last, new first, new last), in document order.
    let mut groups: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut delta: isize = 0;
    for change in changes.changes() {
        let old_first = before.line_at(change.from).number;
        let old_last = before.line_at(change.to).number;
        let new_from = (change.from as isize + delta).max(0) as usize;
        let new_to = new_from + change.insert.len();
        delta += change.insert.len() as isize - (change.to - change.from) as isize;
        let new_first = after.line_at(new_from).number;
        let new_last = after.line_at(new_to.min(after.len())).number;
        match groups.last_mut() {
            Some(group) if old_first <= group.1 => {
                group.1 = group.1.max(old_last);
                group.3 = group.3.max(new_last);
            }
            _ => groups.push((old_first, old_last, new_first, new_last)),
        }
    }
    for (old_first, old_last, new_first, new_last) in groups {
        out.push(LineSplice {
            first: new_first,
            old: old_last - old_first + 1,
            new: new_last - new_first + 1,
        });
    }
}

/// The selection filters every editor installs.
pub fn default_filters() -> Vec<SelectionFilter> {
    vec![crate::list::prefix_selection_guard]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::SelectionRange;

    fn typed(state: &mut EditorState, pos: usize, text: &str, time_ms: u64) {
        state
            .apply(
                Transaction::new()
                    .change(Change::insert(pos, text))
                    .event(UserEvent::Type)
                    .at(time_ms),
            )
            .unwrap();
    }

    /// Applies `splices` to one item per line of `before`, tagging new items.
    fn replay_splices(before: &str, splices: &[LineSplice]) -> Vec<Option<usize>> {
        let mut items: Vec<Option<usize>> = (0..Text::new(before).line_count()).map(Some).collect();
        for splice in splices {
            items.splice(
                splice.first..splice.first + splice.old,
                std::iter::repeat_n(None, splice.new),
            );
        }
        items
    }

    #[test]
    fn line_splices_track_every_edit_and_keep_untouched_lines() {
        let before = "a
b
c
d
e";
        let mut state = EditorState::new(before);
        state
            .apply(Transaction::new().changes([
                Change::insert(
                    1, "
x
y",
                ),
                Change::new(4..5, "C"),
                Change::delete(5..6),
            ]))
            .unwrap();
        assert_eq!(
            state.doc(),
            "a
x
y
b
Cd
e"
        );
        let splices = state.take_line_splices();
        let items = replay_splices(before, &splices);
        assert_eq!(items.len(), state.text().line_count());
        assert_eq!(items, vec![None, None, None, Some(1), None, Some(4)]);
        assert!(state.take_line_splices().is_empty());

        assert!(state.undo());
        assert_eq!(replay_splices(state.doc(), &[]).len(), 5);
        let undo = state.take_line_splices();
        let after_undo = replay_splices(
            "a
x
y
b
Cd
e",
            &undo,
        );
        assert_eq!(after_undo.len(), 5);
        assert_eq!(after_undo.last(), Some(&Some(5)));

        state.reload(
            "one
two",
        );
        assert_eq!(
            state.take_line_splices(),
            vec![LineSplice {
                first: 0,
                old: 5,
                new: 2
            }]
        );
    }

    #[test]
    fn keystrokes_closer_than_500_ms_undo_together() {
        let mut state = EditorState::new("");
        typed(&mut state, 0, "a", 0);
        typed(&mut state, 1, "b", 300);
        typed(&mut state, 2, "c", 790);
        typed(&mut state, 3, "d", 1400);
        assert_eq!(state.doc(), "abcd");
        assert!(state.undo());
        assert_eq!(state.doc(), "abc");
        assert!(state.undo());
        assert_eq!(state.doc(), "");
        assert!(!state.undo());
        assert!(state.redo());
        assert_eq!(state.doc(), "abc");
        assert!(state.redo());
        assert_eq!(state.doc(), "abcd");
        assert!(!state.redo());
    }

    #[test]
    fn a_caret_move_or_a_command_starts_a_new_undo_step() {
        let mut state = EditorState::new("");
        typed(&mut state, 0, "ab", 0);
        state.select(Selection::cursor(0)).unwrap();
        typed(&mut state, 0, "x", 10);
        state
            .apply(
                Transaction::new()
                    .change(Change::insert(3, "!"))
                    .event(UserEvent::Command)
                    .at(20),
            )
            .unwrap();
        assert!(state.undo());
        assert_eq!(state.doc(), "xab");
        assert!(state.undo());
        assert_eq!(state.doc(), "ab");
    }

    #[test]
    fn undo_restores_the_selection_from_before_the_step() {
        let mut state = EditorState::new("hello");
        state.select(Selection::single(0, 5)).unwrap();
        state
            .apply(
                Transaction::new()
                    .change(Change::new(0..5, "bye"))
                    .select(Selection::cursor(3))
                    .event(UserEvent::Command),
            )
            .unwrap();
        state.undo();
        assert_eq!(state.doc(), "hello");
        assert_eq!(state.selection().main(), SelectionRange::new(0, 5));
        state.redo();
        assert_eq!(state.selection().main(), SelectionRange::cursor(3));
    }

    #[test]
    fn an_edit_past_the_end_is_rejected_and_leaves_the_buffer_unchanged() {
        let mut state = EditorState::new("abc");
        state.select(Selection::cursor(2)).unwrap();
        let before = (state.doc().to_string(), state.selection().clone());
        let error = state
            .apply(
                Transaction::new()
                    .change(Change::insert(1, "x"))
                    .change(Change::delete(2..9)),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TransactionError::Change(ChangeError::OutOfBounds { .. })
        ));
        assert_eq!((state.doc().to_string(), state.selection().clone()), before);
        assert!(!state.history().can_undo());
        let error = state
            .apply(Transaction::new().select(Selection::cursor(4)))
            .unwrap_err();
        assert!(matches!(error, TransactionError::Selection { .. }));
    }

    #[test]
    fn a_filter_sees_the_new_document_and_cannot_leave_it() {
        // Pushes every caret 100 bytes right: way past the end.
        fn overshoot(_: &EditorState, selection: Selection) -> Selection {
            let main = selection.main();
            Selection::cursor(main.head + 100)
        }
        fn doc_len_caret(state: &EditorState, _: Selection) -> Selection {
            Selection::cursor(state.doc().len())
        }
        let mut state = EditorState::with_filters("ab", vec![doc_len_caret, overshoot]);
        state
            .apply(
                Transaction::new()
                    .change(Change::insert(0, "xyz"))
                    .select(Selection::cursor(0)),
            )
            .unwrap();
        assert_eq!(state.doc(), "xyzab");
        assert_eq!(state.selection().main(), SelectionRange::cursor(5));
    }

    #[test]
    fn multiple_carets_type_in_one_transaction() {
        let mut state = EditorState::new("a\nb");
        state
            .select(Selection::new(
                vec![SelectionRange::cursor(1), SelectionRange::cursor(3)],
                0,
            ))
            .unwrap();
        state
            .apply(
                Transaction::new()
                    .changes([Change::insert(1, "!"), Change::insert(3, "!")])
                    .event(UserEvent::Type),
            )
            .unwrap();
        assert_eq!(state.doc(), "a!\nb!");
        assert_eq!(
            state.selection().ranges(),
            &[SelectionRange::cursor(2), SelectionRange::cursor(5)]
        );
        state.undo();
        assert_eq!(state.doc(), "a\nb");
    }

    #[test]
    fn each_file_keeps_its_history_caret_and_scroll() {
        let mut first = EditorState::new("one");
        let mut second = EditorState::new("two");
        typed(&mut first, 3, "!", 0);
        first.set_scroll_anchor(ScrollAnchor {
            pos: 2,
            offset: 7.5,
        });
        second.select(Selection::cursor(1)).unwrap();
        typed(&mut second, 0, "#", 0);
        // Typing above the scroll anchor keeps it on the same text.
        typed(&mut first, 0, ">> ", 1_000);
        assert_eq!(
            first.scroll_anchor(),
            ScrollAnchor {
                pos: 5,
                offset: 7.5
            }
        );
        assert_eq!(second.selection().main(), SelectionRange::cursor(2));
        assert!(second.undo());
        assert_eq!(second.doc(), "two");
        assert_eq!(first.doc(), ">> one!");
        assert_eq!(first.history().undo_depth(), 2);
    }

    #[test]
    fn reload_is_not_undoable_and_clamps_the_caret() {
        let mut state = EditorState::new("long text");
        typed(&mut state, 9, "!", 0);
        state.select(Selection::cursor(10)).unwrap();
        state.reload("short");
        assert_eq!(state.selection().main(), SelectionRange::cursor(5));
        assert!(!state.undo());
        assert!(!state.is_dirty());
    }

    #[test]
    fn dirty_until_saved() {
        let mut state = EditorState::new("a");
        assert!(!state.is_dirty());
        typed(&mut state, 1, "b", 0);
        assert!(state.is_dirty());
        state.mark_saved();
        assert!(!state.is_dirty());
    }
}
