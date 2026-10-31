//! Buffer offsets to display offsets within one line, given hidden ranges.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    /// The line text with hidden ranges removed.
    pub text: String,
    /// Hidden ranges, line-relative buffer offsets, sorted and disjoint.
    pub hidden: Vec<Range<usize>>,
    pub buffer_len: usize,
}

impl DisplayLine {
    pub fn new(line: &str, hidden: Vec<Range<usize>>) -> Self {
        let mut text = String::with_capacity(line.len());
        let mut ix = 0;
        for range in &hidden {
            text.push_str(&line[ix..range.start]);
            ix = range.end;
        }
        text.push_str(&line[ix..]);
        Self {
            text,
            hidden,
            buffer_len: line.len(),
        }
    }

    /// Display offset of a line-relative buffer offset. An offset inside a
    /// hidden range maps to where that range collapsed.
    pub fn to_display(&self, buffer: usize) -> usize {
        let mut removed = 0;
        for range in &self.hidden {
            if buffer >= range.end {
                removed += range.len();
            } else if buffer > range.start {
                return range.start - removed;
            } else {
                break;
            }
        }
        buffer.min(self.buffer_len) - removed
    }

    /// Buffer offset of a display offset. At the boundary of a hidden range
    /// the offset skips it forward: it lands on the next visible character,
    /// so a selection that ends after visible text includes the markup that
    /// follows it (closing `**`, `](url)`), and a click at the start of a
    /// heading lands after its hidden `# `.
    pub fn to_buffer(&self, display: usize) -> usize {
        let mut removed = 0;
        for range in &self.hidden {
            if range.start - removed <= display {
                removed += range.len();
            } else {
                break;
            }
        }
        (display + removed).min(self.buffer_len)
    }

    /// Visible buffer segments with their display start.
    pub fn visible_segments(&self) -> Vec<(Range<usize>, usize)> {
        let mut segments = Vec::new();
        let mut ix = 0;
        let mut display = 0;
        for range in &self.hidden {
            if range.start > ix {
                segments.push((ix..range.start, display));
                display += range.start - ix;
            }
            ix = range.end;
        }
        if ix < self.buffer_len {
            segments.push((ix..self.buffer_len, display));
        }
        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_offsets_across_hidden_ranges() {
        // "a **b** c" with both `**` hidden: display "a b c".
        let line = DisplayLine::new("a **b** c", vec![2..4, 5..7]);
        assert_eq!(line.text, "a b c");
        assert_eq!(line.to_display(0), 0);
        assert_eq!(line.to_display(3), 2, "inside `**` collapses");
        assert_eq!(line.to_display(4), 2);
        assert_eq!(line.to_display(5), 3);
        assert_eq!(line.to_display(7), 3);
        assert_eq!(line.to_display(9), 5);
        assert_eq!(line.to_buffer(2), 4, "before `b`, after the hidden `**`");
        assert_eq!(line.to_buffer(3), 7, "after `b`, skips the closing `**`");
        assert_eq!(line.to_buffer(5), 9);
        for display in 0..=line.text.len() {
            assert_eq!(line.to_display(line.to_buffer(display)), display);
        }
        assert_eq!(
            line.visible_segments(),
            vec![(0..2, 0), (4..5, 2), (7..9, 3)]
        );
    }

    #[test]
    fn hidden_range_at_line_edges() {
        let heading = DisplayLine::new("## Title", vec![0..3]);
        assert_eq!(heading.text, "Title");
        assert_eq!(heading.to_buffer(0), 3);
        let link = DisplayLine::new("[a](u)", vec![0..1, 2..6]);
        assert_eq!(link.text, "a");
        assert_eq!(link.to_buffer(1), 6);
        let image = DisplayLine::new("![x](y)", vec![0..7]);
        assert_eq!(image.text, "");
        assert_eq!(image.to_buffer(0), 7);
    }
}
