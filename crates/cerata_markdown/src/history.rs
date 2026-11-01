//! Undo history, grouped in time like CodeMirror's `history()` (Writer's
//! `basicSetup.ts:30-68` uses its defaults).

use crate::change::ChangeSet;
use crate::selection::Selection;
use crate::text::Text;

/// Consecutive edits closer than this join one undo step
/// (CodeMirror's `newGroupDelay`).
pub const GROUP_DELAY_MS: u64 = 500;

/// What produced a transaction. Only typing and deleting join a previous
/// undo step, as CodeMirror's `joinableUserEvent` (`/^(input\.type|delete)/`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UserEvent {
    #[default]
    None,
    /// Typed text (`input.type`).
    Type,
    /// IME composition (`input.type.compose`): always joins.
    Compose,
    /// Backspace, Delete and their word variants.
    Delete,
    /// An editing command (list Enter, formatting, paste...).
    Command,
    /// Selection change by keyboard or pointer.
    Select,
    Undo,
    Redo,
    /// The document was replaced from disk (Writer's `writer.reload`).
    Reload,
}

impl UserEvent {
    fn joins(self) -> bool {
        matches!(
            self,
            UserEvent::Type | UserEvent::Delete | UserEvent::Compose
        )
    }
}

/// One undo (or redo) step: the changes to apply now, in the coordinates of
/// the current document, and the selections on both sides of the step.
#[derive(Clone, Debug)]
struct Step {
    changes: ChangeSet,
    selection_before: Selection,
    selection_after: Selection,
    time_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    done: Vec<Step>,
    undone: Vec<Step>,
    /// A selection-only transaction since the last step: the next edit starts
    /// a new step (CodeMirror's non-empty `selectionsAfter`).
    broken: bool,
}

/// What `undo` and `redo` hand back to the state to apply.
pub(crate) struct Replay {
    pub changes: ChangeSet,
    pub selection: Selection,
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.done.len()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn note_selection(&mut self) {
        self.broken = true;
    }

    /// Records an edit. `before` is the start document, `after` the result.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &mut self,
        before: &Text,
        after: &Text,
        changes: &ChangeSet,
        selection_before: &Selection,
        selection_after: &Selection,
        event: UserEvent,
        time_ms: u64,
    ) {
        let inverse = changes.invert(before);
        self.undone.clear();
        let join = match self.done.last() {
            Some(last) if !self.broken => {
                event == UserEvent::Compose
                    || (event.joins()
                        && time_ms.saturating_sub(last.time_ms) < GROUP_DELAY_MS
                        && adjacent(&last.changes, changes))
            }
            _ => false,
        };
        self.broken = false;
        if join && let Some(last) = self.done.last_mut() {
            // Undo the new edit, then the previous step, as one change.
            last.changes = inverse.compose(after, &last.changes);
            last.selection_after = selection_after.clone();
            last.time_ms = time_ms;
            return;
        }
        self.done.push(Step {
            changes: inverse,
            selection_before: selection_before.clone(),
            selection_after: selection_after.clone(),
            time_ms,
        });
    }

    pub(crate) fn take_undo(&mut self) -> Option<Replay> {
        let step = self.done.pop()?;
        let replay = Replay {
            changes: step.changes.clone(),
            selection: step.selection_before.clone(),
        };
        self.undone.push(step);
        Some(replay)
    }

    pub(crate) fn take_redo(&mut self) -> Option<Replay> {
        let step = self.undone.pop()?;
        let replay = Replay {
            changes: step.changes.clone(),
            selection: step.selection_after.clone(),
        };
        self.done.push(step);
        Some(replay)
    }

    /// After a replay, the step moved to the other stack must hold the
    /// changes that reverse what was just applied.
    pub(crate) fn settle_replay(&mut self, redo: bool, inverse: ChangeSet) {
        let stack = if redo {
            &mut self.done
        } else {
            &mut self.undone
        };
        if let Some(step) = stack.last_mut() {
            step.changes = inverse;
        }
        self.broken = true;
    }
}

/// CodeMirror's `isAdjacent`: the previous step's affected ranges (in the
/// current document, the A side of its inverse) touch the new edit's ranges
/// in the document it started from.
fn adjacent(previous_inverse: &ChangeSet, next: &ChangeSet) -> bool {
    previous_inverse.ranges_before().any(|a| {
        next.ranges_before()
            .any(|b| b.start <= a.end && b.end >= a.start)
    })
}
