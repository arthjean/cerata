use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    pub first_line: usize,
    pub old_line_count: usize,
    pub new_line_count: usize,
}

pub struct Buffer {
    text: String,
    line_starts: Vec<usize>,
    line_starts_utf16: Vec<usize>,
    len_utf16: usize,
}

impl Buffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        let mut line_starts_utf16 = vec![0];
        let mut utf16 = 0;
        for (ix, ch) in text.char_indices() {
            utf16 += ch.len_utf16();
            if ch == '\n' {
                line_starts.push(ix + 1);
                line_starts_utf16.push(utf16);
            }
        }
        Self {
            text,
            line_starts,
            line_starts_utf16,
            len_utf16: utf16,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn len_utf16(&self) -> usize {
        self.len_utf16
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_range(&self, ix: usize) -> Range<usize> {
        let start = self.line_starts[ix];
        let end = self
            .line_starts
            .get(ix + 1)
            .map_or(self.text.len(), |next| next - 1);
        start..end
    }

    pub fn line_text(&self, ix: usize) -> &str {
        &self.text[self.line_range(ix)]
    }

    pub fn line_for_offset(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(ix) => ix,
            Err(ix) => ix - 1,
        }
    }

    pub fn clip_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.clip_offset(offset);
        let line = self.line_for_offset(offset);
        let start = self.line_starts[line];
        self.line_starts_utf16[line]
            + self.text[start..offset]
                .chars()
                .map(char::len_utf16)
                .sum::<usize>()
    }

    pub fn offset_from_utf16(&self, offset_utf16: usize) -> usize {
        let offset_utf16 = offset_utf16.min(self.len_utf16);
        let line = match self.line_starts_utf16.binary_search(&offset_utf16) {
            Ok(ix) => return self.line_starts[ix],
            Err(ix) => ix - 1,
        };
        let mut utf16 = self.line_starts_utf16[line];
        let start = self.line_starts[line];
        for (ix, ch) in self.text[start..].char_indices() {
            if utf16 >= offset_utf16 {
                return start + ix;
            }
            utf16 += ch.len_utf16();
        }
        self.text.len()
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    pub fn edit(&mut self, range: Range<usize>, new_text: &str) -> LineEdit {
        let start = self.clip_offset(range.start);
        let end = self.clip_offset(range.end.max(start));
        let first_line = self.line_for_offset(start);
        let last_line = self.line_for_offset(end);

        let region_start = self.line_starts[first_line];
        let region_start_utf16 = self.line_starts_utf16[first_line];
        let old_next = self.line_starts.get(last_line + 1).copied();
        let old_next_utf16 = self
            .line_starts_utf16
            .get(last_line + 1)
            .copied()
            .unwrap_or(self.len_utf16);
        let old_region_utf16 = old_next_utf16 - region_start_utf16;

        self.text.replace_range(start..end, new_text);
        let delta = new_text.len() as isize - (end - start) as isize;
        let region_end = old_next.map_or(self.text.len(), |next| (next as isize + delta) as usize);

        let mut new_starts = vec![region_start];
        let mut new_starts_utf16 = vec![region_start_utf16];
        let mut utf16 = region_start_utf16;
        for (ix, ch) in self.text[region_start..region_end].char_indices() {
            utf16 += ch.len_utf16();
            let next = region_start + ix + 1;
            if ch == '\n' && (old_next.is_none() || next < region_end) {
                new_starts.push(next);
                new_starts_utf16.push(utf16);
            }
        }
        let new_region_utf16 = utf16 - region_start_utf16;
        let delta_utf16 = new_region_utf16 as isize - old_region_utf16 as isize;
        let new_line_count = new_starts.len();

        let tail = last_line + 1;
        for start in &mut self.line_starts[tail..] {
            *start = (*start as isize + delta) as usize;
        }
        for start in &mut self.line_starts_utf16[tail..] {
            *start = (*start as isize + delta_utf16) as usize;
        }
        self.line_starts.splice(first_line..tail, new_starts);
        self.line_starts_utf16
            .splice(first_line..tail, new_starts_utf16);
        self.len_utf16 = (self.len_utf16 as isize + delta_utf16) as usize;

        LineEdit {
            first_line,
            old_line_count: tail - first_line,
            new_line_count,
        }
    }

    pub fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(ix, _)| ix)
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map_or(self.text.len(), |ch| offset + ch.len_utf8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_index_consistent(buffer: &Buffer) {
        let fresh = Buffer::new(buffer.text().to_string());
        assert_eq!(buffer.line_starts, fresh.line_starts, "byte line starts");
        assert_eq!(
            buffer.line_starts_utf16, fresh.line_starts_utf16,
            "utf16 line starts"
        );
        assert_eq!(buffer.len_utf16, fresh.len_utf16);
    }

    #[test]
    fn edits_keep_the_line_index_consistent() {
        let mut buffer = Buffer::new("# Title\n\nhello **wörld**\n漢字 line\nlast");
        let edits: &[(Range<usize>, &str)] = &[
            (9..9, "x"),
            (0..2, ""),
            (3..12, "a\nb\nc"),
            (0..0, "\n\n"),
            (5..20, ""),
            (0..0, "🙂"),
        ];
        for (range, text) in edits {
            let end = buffer.clip_offset(range.end);
            buffer.edit(range.start..end, text);
            assert_index_consistent(&buffer);
        }
        let len = buffer.len();
        buffer.edit(len..len, "\n");
        assert_index_consistent(&buffer);
        buffer.edit(0..buffer.len(), "");
        assert_index_consistent(&buffer);
        assert_eq!(buffer.line_count(), 1);
    }

    #[test]
    fn line_edit_reports_structural_change() {
        let mut buffer = Buffer::new("a\nb\nc");
        assert_eq!(
            buffer.edit(2..3, "x\ny"),
            LineEdit {
                first_line: 1,
                old_line_count: 1,
                new_line_count: 2
            }
        );
        assert_eq!(buffer.text(), "a\nx\ny\nc");
        assert_eq!(
            buffer.edit(1..5, ""),
            LineEdit {
                first_line: 0,
                old_line_count: 3,
                new_line_count: 1
            }
        );
        assert_eq!(buffer.text(), "a\nc");
    }

    #[test]
    fn utf16_round_trip() {
        let buffer = Buffer::new("e🙂漢\nZ");
        assert_eq!(buffer.len_utf16(), 6);
        assert_eq!(buffer.offset_to_utf16(1), 1);
        assert_eq!(buffer.offset_to_utf16(5), 3);
        assert_eq!(buffer.offset_from_utf16(3), 5);
        assert_eq!(buffer.offset_from_utf16(5), 9);
        assert_eq!(buffer.offset_from_utf16(6), 10);
        for offset in [0, 1, 5, 8, 9, 10] {
            assert_eq!(
                buffer.offset_from_utf16(buffer.offset_to_utf16(offset)),
                offset
            );
        }
    }
}
