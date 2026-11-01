//! Horizontal scroll of code blocks (`codeFenceScroll.ts:32-134`, `:203-392`):
//! code lines never wrap; each block (fenced, indented, frontmatter) has one
//! offset, kept across edits and bounded by its widest line.

use std::collections::HashMap;

use cerata_markdown::LineSplice;

/// Thumb thickness, gap to the block's bottom edge, and shortest width
/// (`codeFenceScroll.ts:44-46`).
pub const THUMB_HEIGHT: f32 = 6.0;
pub const THUMB_INSET: f32 = 3.0;
pub const MIN_THUMB_WIDTH: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thumb {
    pub left: f32,
    pub width: f32,
    /// Offset moved per pixel the thumb is dragged.
    pub per_pixel: f32,
}

#[derive(Default, Debug)]
pub struct CodeScroll {
    /// Offset per block, keyed by the block's first line.
    offsets: HashMap<usize, f32>,
    /// Widest laid-out text and visible width per block, this frame.
    widths: HashMap<usize, (f32, f32)>,
}

impl CodeScroll {
    pub fn offset_of_line(&self, first_line: usize) -> f32 {
        self.offsets.get(&first_line).copied().unwrap_or(0.0)
    }

    /// Forgets the widths of the previous frame.
    pub fn begin_frame(&mut self) {
        self.widths.clear();
    }

    /// A laid-out line of the block starting on `first_line`, `text_width`
    /// wide in a visible width.
    pub fn measure(&mut self, first_line: usize, text_width: f32, visible: f32) {
        let entry = self.widths.entry(first_line).or_insert((0.0, visible));
        entry.0 = entry.0.max(text_width);
        entry.1 = visible;
    }

    /// Largest offset of a block: its widest line past the visible width.
    pub fn max(&self, first_line: usize) -> f32 {
        self.widths
            .get(&first_line)
            .map_or(0.0, |(width, visible)| (width - visible).max(0.0))
    }

    pub fn set(&mut self, first_line: usize, offset: f32) -> bool {
        let offset = clamp_offset(offset, self.max(first_line));
        let current = self.offset_of_line(first_line);
        if offset > 0.0 {
            self.offsets.insert(first_line, offset);
        } else {
            self.offsets.remove(&first_line);
        }
        offset != current
    }

    /// Bounds every offset by its block's measured width: a block that got
    /// narrower than the visible width goes back to 0.
    pub fn clamp_measured(&mut self) {
        let widths = &self.widths;
        self.offsets.retain(|line, offset| match widths.get(line) {
            Some((width, visible)) => {
                *offset = clamp_offset(*offset, (width - visible).max(0.0));
                *offset > 0.0
            }
            None => true,
        });
    }

    /// Remaps the blocks through line splices; `is_block_start(line)` drops
    /// those that no longer start a code block.
    pub fn remap(&mut self, splices: &[LineSplice], is_block_start: impl Fn(usize) -> bool) {
        if self.offsets.is_empty() {
            return;
        }
        let mut offsets: Vec<(usize, f32)> = self.offsets.drain().collect();
        for splice in splices {
            for (line, _) in &mut offsets {
                if *line >= splice.first + splice.old {
                    *line = *line + splice.new - splice.old;
                } else if *line >= splice.first {
                    *line = splice.first;
                }
            }
        }
        self.offsets = offsets
            .into_iter()
            .filter(|(line, _)| is_block_start(*line))
            .collect();
    }

    pub fn thumb(&self, first_line: usize) -> Option<Thumb> {
        let (_, visible) = *self.widths.get(&first_line)?;
        let max = self.max(first_line);
        (max > 0.0).then(|| thumb_geometry(self.offset_of_line(first_line), max, visible))
    }
}

pub fn clamp_offset(offset: f32, max: f32) -> f32 {
    offset.round().clamp(0.0, max.max(0.0))
}

/// The smallest move of `offset` that brings `caret_x` inside
/// `[left, right]` (`revealOffset`).
pub fn reveal_offset(offset: f32, caret_x: f32, left: f32, right: f32) -> f32 {
    if caret_x < left {
        offset - (left - caret_x)
    } else if caret_x > right {
        offset + (caret_x - right)
    } else {
        offset
    }
}

/// `thumbGeometry`: the thumb's share of the track is the visible share of
/// the content.
pub fn thumb_geometry(offset: f32, max: f32, track: f32) -> Thumb {
    let width = track.min((track * track / (track + max)).max(MIN_THUMB_WIDTH));
    let travel = track - width;
    Thumb {
        left: if travel > 0.0 {
            travel * clamp_offset(offset, max) / max
        } else {
            0.0
        },
        width,
        per_pixel: if travel > 0.0 { max / travel } else { 0.0 },
    }
}

/// A wheel gesture scrolls a code block only when mostly horizontal.
pub fn is_horizontal(dx: f32, dy: f32) -> bool {
    dx.abs() > dy.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_bounded_and_remapped() {
        let mut scroll = CodeScroll::default();
        scroll.measure(4, 900.0, 600.0);
        assert!(scroll.set(4, 1000.0));
        assert_eq!(
            scroll.offset_of_line(4),
            300.0,
            "bounded by the widest line"
        );
        // Two lines inserted above: the block moves down with its offset.
        scroll.remap(
            &[LineSplice {
                first: 1,
                old: 1,
                new: 3,
            }],
            |line| line == 6,
        );
        assert_eq!(scroll.offset_of_line(6), 300.0);
        // The block is no longer a block start: dropped.
        scroll.remap(&[], |_| false);
        assert_eq!(scroll.offset_of_line(6), 0.0);
    }

    #[test]
    fn a_narrower_block_goes_back_to_zero_and_loses_its_bar() {
        let mut scroll = CodeScroll::default();
        scroll.measure(0, 800.0, 500.0);
        scroll.set(0, 200.0);
        assert!(scroll.thumb(0).is_some());
        scroll.begin_frame();
        scroll.measure(0, 400.0, 500.0);
        scroll.clamp_measured();
        assert_eq!(scroll.offset_of_line(0), 0.0);
        assert!(scroll.thumb(0).is_none());
    }

    #[test]
    fn reveal_moves_by_the_smallest_amount_and_the_thumb_has_a_minimum() {
        assert_eq!(reveal_offset(0.0, 650.0, 0.0, 600.0), 50.0);
        assert_eq!(reveal_offset(100.0, 80.0, 100.0, 700.0), 80.0);
        assert_eq!(reveal_offset(100.0, 300.0, 100.0, 700.0), 100.0);
        let thumb = thumb_geometry(0.0, 100_000.0, 600.0);
        assert_eq!(thumb.width, MIN_THUMB_WIDTH);
        let half = thumb_geometry(150.0, 300.0, 600.0);
        assert_eq!(half.width, 400.0);
        assert_eq!(half.left, 100.0);
        assert_eq!(half.per_pixel, 1.5);
        assert!(is_horizontal(10.0, 2.0));
        assert!(!is_horizontal(3.0, 3.0));
    }
}
