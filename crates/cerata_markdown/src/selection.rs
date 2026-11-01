//! Selections: one or more ranges, each with an anchor and a head.

use crate::change::{Bias, ChangeSet};
use crate::text::Text;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectionRange {
    pub anchor: usize,
    pub head: usize,
}

impl SelectionRange {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn cursor(pos: usize) -> Self {
        Self::new(pos, pos)
    }

    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.from()..self.to()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Writer's contact rule (`rangeTouchesRange`, `utils.ts:126-132`):
    /// inclusive at both ends, so a caret right after `**bold**` touches it.
    pub fn touches(&self, range: Range<usize>) -> bool {
        touches(self.from()..self.to(), range)
    }

    pub fn map(&self, changes: &ChangeSet) -> Self {
        if self.is_empty() {
            // A caret moves after text typed at it, like CodeMirror's default.
            let pos = changes.map_pos(self.head, Bias::Right);
            return Self::cursor(pos);
        }
        let (anchor_bias, head_bias) = if self.anchor < self.head {
            (Bias::Left, Bias::Right)
        } else {
            (Bias::Right, Bias::Left)
        };
        Self::new(
            changes.map_pos(self.anchor, anchor_bias),
            changes.map_pos(self.head, head_bias),
        )
    }

    /// Clamped into `text` and onto character boundaries.
    pub fn clamp(&self, text: &Text) -> Self {
        Self::new(
            text.floor_char_boundary(self.anchor),
            text.floor_char_boundary(self.head),
        )
    }
}

/// Inclusive contact of two ranges: `a.from <= b.to && b.from <= a.to`.
pub fn touches(a: Range<usize>, b: Range<usize>) -> bool {
    a.start <= b.end && b.start <= a.end
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    ranges: Vec<SelectionRange>,
    main: usize,
}

impl Selection {
    pub fn single(anchor: usize, head: usize) -> Self {
        Self {
            ranges: vec![SelectionRange::new(anchor, head)],
            main: 0,
        }
    }

    pub fn cursor(pos: usize) -> Self {
        Self::single(pos, pos)
    }

    /// Ranges are sorted and overlapping ones merged; `main` follows its
    /// range. An empty list becomes a caret at 0.
    pub fn new(ranges: Vec<SelectionRange>, main: usize) -> Self {
        if ranges.is_empty() {
            return Self::cursor(0);
        }
        let main_range = ranges.get(main).or(ranges.first()).copied();
        let mut sorted = ranges;
        sorted.sort_by_key(|r| (r.from(), r.to()));
        let mut merged: Vec<SelectionRange> = Vec::with_capacity(sorted.len());
        let mut main_index = 0;
        for range in sorted {
            let is_main = Some(range) == main_range;
            if let Some(last) = merged.last_mut() {
                // CodeMirror's rule: a caret at the end of the previous range
                // joins it; two non-empty ranges only merge when they overlap.
                let overlaps = if range.is_empty() {
                    range.from() <= last.to()
                } else {
                    range.from() < last.to()
                };
                if overlaps {
                    let from = last.from().min(range.from());
                    let to = last.to().max(range.to());
                    let forward = last.head >= last.anchor;
                    *last = if forward {
                        SelectionRange::new(from, to)
                    } else {
                        SelectionRange::new(to, from)
                    };
                    if is_main {
                        main_index = merged.len() - 1;
                    }
                    continue;
                }
            }
            if is_main {
                main_index = merged.len();
            }
            merged.push(range);
        }
        Self {
            ranges: merged,
            main: main_index,
        }
    }

    pub fn ranges(&self) -> &[SelectionRange] {
        &self.ranges
    }

    pub fn main_index(&self) -> usize {
        self.main
    }

    pub fn main(&self) -> SelectionRange {
        self.ranges
            .get(self.main)
            .copied()
            .unwrap_or(SelectionRange::cursor(0))
    }

    /// True when any range touches `range` (inclusive contact).
    pub fn touches(&self, range: Range<usize>) -> bool {
        self.ranges.iter().any(|r| r.touches(range.clone()))
    }

    pub fn map(&self, changes: &ChangeSet) -> Self {
        Self::new(
            self.ranges.iter().map(|r| r.map(changes)).collect(),
            self.main,
        )
    }

    pub fn clamp(&self, text: &Text) -> Self {
        Self::new(
            self.ranges.iter().map(|r| r.clamp(text)).collect(),
            self.main,
        )
    }

    pub fn is_within(&self, text: &Text) -> bool {
        self.ranges.iter().all(|r| {
            r.to() <= text.len() && text.is_char_boundary(r.anchor) && text.is_char_boundary(r.head)
        })
    }
}

impl Default for Selection {
    fn default() -> Self {
        Self::cursor(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlapping_ranges_and_keeps_main() {
        let selection = Selection::new(
            vec![
                SelectionRange::new(8, 10),
                SelectionRange::new(0, 4),
                SelectionRange::new(3, 6),
            ],
            0,
        );
        assert_eq!(
            selection.ranges(),
            &[SelectionRange::new(0, 6), SelectionRange::new(8, 10)]
        );
        assert_eq!(selection.main(), SelectionRange::new(8, 10));
    }

    #[test]
    fn identical_carets_collapse_but_adjacent_ranges_stay_apart() {
        let carets = Selection::new(
            vec![SelectionRange::cursor(2), SelectionRange::cursor(2)],
            1,
        );
        assert_eq!(carets.ranges().len(), 1);
        let adjacent = Selection::new(
            vec![SelectionRange::new(0, 2), SelectionRange::new(2, 4)],
            0,
        );
        assert_eq!(adjacent.ranges().len(), 2);
    }

    #[test]
    fn contact_is_inclusive_at_both_ends() {
        let caret = SelectionRange::cursor(5);
        assert!(caret.touches(0..5));
        assert!(caret.touches(5..9));
        assert!(!caret.touches(6..9));
    }
}
