//! Display map of one line: buffer offsets to offsets in the shaped text
//! once hidden syntax is removed and folded syntax replaced by its glyph.

use std::ops::Range;

/// A run of the line: source shown as is, or replaced (by an empty string
/// when hidden).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Line-relative buffer range.
    pub buffer: Range<usize>,
    /// Range in [`DisplayLine::text`].
    pub display: Range<usize>,
    pub source: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    pub segments: Vec<Segment>,
    pub buffer_len: usize,
}

impl DisplayLine {
    /// `line` with each `(range, replacement)` applied; ranges are
    /// line-relative, sorted and disjoint, and an empty replacement hides.
    pub fn new(line: &str, replacements: &[(Range<usize>, String)]) -> Self {
        let mut text = String::with_capacity(line.len());
        let mut segments = Vec::new();
        let mut ix = 0;
        fn push(
            text: &mut String,
            segments: &mut Vec<Segment>,
            buffer: Range<usize>,
            shown: &str,
            source: bool,
        ) {
            let start = text.len();
            text.push_str(shown);
            segments.push(Segment {
                buffer,
                display: start..text.len(),
                source,
            });
        }
        for (range, replacement) in replacements {
            let range = range.start.max(ix).min(line.len())..range.end.min(line.len());
            if range.start > ix {
                push(
                    &mut text,
                    &mut segments,
                    ix..range.start,
                    &line[ix..range.start],
                    true,
                );
            }
            if range.end > range.start || !replacement.is_empty() {
                push(&mut text, &mut segments, range.clone(), replacement, false);
            }
            ix = ix.max(range.end);
        }
        if ix < line.len() || segments.is_empty() {
            push(&mut text, &mut segments, ix..line.len(), &line[ix..], true);
        }
        Self {
            text,
            segments,
            buffer_len: line.len(),
        }
    }

    #[cfg(test)]
    pub fn plain(line: &str) -> Self {
        Self::new(line, &[])
    }

    /// Display offset of a line-relative buffer offset. An offset inside a
    /// replaced run maps to where the run starts.
    pub fn to_display(&self, buffer: usize) -> usize {
        for segment in &self.segments {
            if buffer < segment.buffer.end || buffer == segment.buffer.start {
                return if segment.source {
                    segment.display.start + buffer.saturating_sub(segment.buffer.start)
                } else {
                    segment.display.start
                };
            }
        }
        self.text.len()
    }

    /// Buffer offset of a display offset. At the edge of hidden syntax the
    /// offset skips it forward, so a selection that ends after visible text
    /// includes the markup that follows it and a click at the start of a
    /// heading lands after its hidden `# `. Inside a replacement glyph it
    /// goes to the end of the replaced source.
    pub fn to_buffer(&self, display: usize) -> usize {
        for segment in &self.segments {
            if display < segment.display.end {
                return if segment.source {
                    segment.buffer.start + (display - segment.display.start)
                } else if display == segment.display.start {
                    segment.buffer.start
                } else {
                    segment.buffer.end
                };
            }
        }
        self.buffer_len
    }

    /// Visible source runs with their display start, for styling.
    pub fn source_runs(&self) -> impl Iterator<Item = &Segment> {
        self.segments.iter().filter(|s| !s.display.is_empty())
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init, reason = "expected match lists")]
mod tests {
    use super::*;

    fn hide(ranges: &[Range<usize>]) -> Vec<(Range<usize>, String)> {
        ranges.iter().map(|r| (r.clone(), String::new())).collect()
    }

    #[test]
    fn maps_offsets_across_hidden_ranges() {
        // "a **b** c" with both `**` hidden: display "a b c".
        let line = DisplayLine::new("a **b** c", &hide(&[2..4, 5..7]));
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
    }

    #[test]
    fn hidden_ranges_at_line_edges_and_glyphs() {
        let heading = DisplayLine::new("## Title", &hide(&[0..3]));
        assert_eq!(heading.text, "Title");
        assert_eq!(heading.to_buffer(0), 3);
        let link = DisplayLine::new("[a](u)", &hide(&[0..1, 2..6]));
        assert_eq!(link.text, "a");
        assert_eq!(link.to_buffer(1), 6);
        let dash = DisplayLine::new("a -- b", &[(2..4, "\u{2013}".to_owned())]);
        assert_eq!(dash.text, "a \u{2013} b");
        assert_eq!(dash.to_display(3), 2);
        assert_eq!(dash.to_buffer(2), 2, "before the glyph");
        assert_eq!(dash.to_buffer(2 + '\u{2013}'.len_utf8()), 4, "after it");
        assert_eq!(DisplayLine::plain("").to_buffer(0), 0);
    }
}
