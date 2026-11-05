use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub number: usize,
    pub from: usize,
    pub to: usize,
}

impl Line {
    pub fn range(&self) -> Range<usize> {
        self.from..self.to
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text {
    string: String,
    line_starts: Vec<usize>,
}

impl Text {
    pub fn new(string: impl Into<String>) -> Self {
        let string = string.into();
        let line_starts = line_starts(&string, 0);
        Self {
            string,
            line_starts,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    pub fn len(&self) -> usize {
        self.string.len()
    }

    pub fn is_empty(&self) -> bool {
        self.string.is_empty()
    }

    pub fn slice(&self, range: Range<usize>) -> &str {
        self.string.get(range).unwrap_or("")
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line(&self, number: usize) -> Line {
        let number = number.min(self.line_starts.len().saturating_sub(1));
        let from = self.line_starts.get(number).copied().unwrap_or(0);
        let to = match self.line_starts.get(number + 1) {
            Some(next) => line_content_end(&self.string, *next),
            None => self.string.len(),
        };
        Line { number, from, to }
    }

    pub fn line_at(&self, pos: usize) -> Line {
        let pos = pos.min(self.string.len());
        let number = match self.line_starts.binary_search(&pos) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        self.line(number)
    }

    pub fn is_char_boundary(&self, pos: usize) -> bool {
        self.string.is_char_boundary(pos)
    }

    pub fn floor_char_boundary(&self, pos: usize) -> usize {
        let mut pos = pos.min(self.string.len());
        while !self.string.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    }

    pub(crate) fn replace(&mut self, range: Range<usize>, insert: &str) {
        let Range { start, end } = range;
        self.string.replace_range(start..end, insert);
        let first_after = self.line_starts.partition_point(|&s| s < start).max(1);
        let last_kept = self.line_starts.partition_point(|&s| s <= end);
        let delta = insert.len() as isize - (end - start) as isize;
        let tail: Vec<usize> = self.line_starts[last_kept..]
            .iter()
            .map(|&s| (s as isize + delta) as usize)
            .collect();
        self.line_starts.truncate(first_after);
        let rescan_from = self.line_starts.last().copied().unwrap_or(0);
        let rescan_to = tail.first().copied().unwrap_or(self.string.len());
        let fresh = line_starts(&self.string[rescan_from..rescan_to], rescan_from);
        self.line_starts.extend(fresh.into_iter().skip(1));
        self.line_starts.extend(tail);
        self.line_starts.dedup();
    }
}

impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

fn line_starts(text: &str, base: usize) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![base];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => starts.push(base + i + 1),
            b'\r' if bytes.get(i + 1) != Some(&b'\n') => starts.push(base + i + 1),
            _ => {}
        }
        i += 1;
    }
    starts
}

fn line_content_end(text: &str, next_start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = next_start;
    if end > 0 && bytes.get(end - 1) == Some(&b'\n') {
        end -= 1;
    }
    if end > 0 && bytes.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &Text) -> Vec<(usize, usize)> {
        (0..text.line_count())
            .map(|n| {
                let line = text.line(n);
                (line.from, line.to)
            })
            .collect()
    }

    #[test]
    fn indexes_lf_crlf_and_cr_breaks() {
        let text = Text::new("a\nbc\r\nd\re");
        assert_eq!(lines(&text), vec![(0, 1), (2, 4), (6, 7), (8, 9)]);
        assert_eq!(text.line_at(3).number, 1);
        assert_eq!(text.line_at(5).number, 1);
        assert_eq!(text.line_at(6).number, 2);
        assert_eq!(text.line_at(99).number, 3);
    }

    #[test]
    fn replace_keeps_the_line_index_equal_to_a_fresh_index() {
        let cases: &[(&str, std::ops::Range<usize>, &str)] = &[
            ("one\ntwo\nthree", 2..9, "X\nY"),
            ("one\ntwo", 3..4, ""),
            ("a\r\nb", 1..2, ""),
            ("a\rb", 2..2, "\n"),
            ("", 0..0, "\n\n"),
            ("abc", 3..3, "\n"),
            ("l1\nl2\nl3\nl4", 0..11, "z"),
        ];
        for (doc, range, insert) in cases {
            let mut text = Text::new(*doc);
            text.replace(range.clone(), insert);
            let mut expected = doc.to_string();
            expected.replace_range(range.clone(), insert);
            assert_eq!(text, Text::new(expected), "{doc:?} {range:?} {insert:?}");
        }
    }
}
