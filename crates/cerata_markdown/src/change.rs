use crate::text::Text;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

impl Change {
    pub fn new(range: Range<usize>, insert: impl Into<String>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            insert: insert.into(),
        }
    }

    pub fn insert(pos: usize, insert: impl Into<String>) -> Self {
        Self::new(pos..pos, insert)
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self::new(range, "")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bias {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChangeError {
    #[error("change {from}..{to} is outside the document (length {len})")]
    OutOfBounds { from: usize, to: usize, len: usize },
    #[error("change {from}..{to} does not fall on character boundaries")]
    NotCharBoundary { from: usize, to: usize },
    #[error("changes overlap at {at}")]
    Overlap { at: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    changes: Vec<Change>,
}

impl ChangeSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(text: &Text, mut changes: Vec<Change>) -> Result<Self, ChangeError> {
        changes.retain(|c| !(c.from == c.to && c.insert.is_empty()));
        changes.sort_by_key(|c| (c.from, c.to));
        let len = text.len();
        let mut last_to = 0;
        for (index, change) in changes.iter().enumerate() {
            let (from, to) = (change.from, change.to);
            if from > to || to > len {
                return Err(ChangeError::OutOfBounds { from, to, len });
            }
            if !text.is_char_boundary(from) || !text.is_char_boundary(to) {
                return Err(ChangeError::NotCharBoundary { from, to });
            }
            if index > 0 && from < last_to {
                return Err(ChangeError::Overlap { at: from });
            }
            last_to = to;
        }
        Ok(Self { changes })
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub(crate) fn apply(&self, text: &mut Text) {
        for change in self.changes.iter().rev() {
            text.replace(change.from..change.to, &change.insert);
        }
    }

    pub fn invert(&self, text: &Text) -> ChangeSet {
        let mut shift: isize = 0;
        let changes = self
            .changes
            .iter()
            .map(|change| {
                let from = (change.from as isize + shift) as usize;
                shift += change.insert.len() as isize - (change.to - change.from) as isize;
                Change {
                    from,
                    to: from + change.insert.len(),
                    insert: text.slice(change.from..change.to).to_string(),
                }
            })
            .collect();
        ChangeSet { changes }
    }

    pub fn map_pos(&self, pos: usize, bias: Bias) -> usize {
        let mut shift: isize = 0;
        for change in &self.changes {
            if pos < change.from {
                break;
            }
            let start = (change.from as isize + shift) as usize;
            if change.from == change.to {
                if pos == change.from && bias == Bias::Left {
                    return start;
                }
            } else if pos == change.from {
                return start;
            } else if pos < change.to {
                return match bias {
                    Bias::Left => start,
                    Bias::Right => start + change.insert.len(),
                };
            }
            shift += change.insert.len() as isize - (change.to - change.from) as isize;
        }
        (pos as isize + shift) as usize
    }

    pub fn ranges_before(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.changes.iter().map(|c| c.from..c.to)
    }

    pub fn ranges_after(&self) -> Vec<Range<usize>> {
        let mut shift: isize = 0;
        self.changes
            .iter()
            .map(|change| {
                let from = (change.from as isize + shift) as usize;
                shift += change.insert.len() as isize - (change.to - change.from) as isize;
                from..from + change.insert.len()
            })
            .collect()
    }

    pub fn compose(&self, text: &Text, next: &ChangeSet) -> ChangeSet {
        let (Some(first), Some(last)) = (self.changes.first(), self.changes.last()) else {
            return next.clone();
        };
        let (Some(next_first), Some(next_last)) = (next.changes.first(), next.changes.last())
        else {
            return self.clone();
        };
        let mut end = text.clone();
        self.apply(&mut end);
        let grown = end.len() as isize - text.len() as isize;
        next.apply(&mut end);
        let total = end.len() as isize - text.len() as isize;
        let from = first.from.min(next_first.from);
        let to = (last.to as isize).max(next_last.to as isize - grown) as usize;
        let end_to = (to as isize + total) as usize;
        ChangeSet {
            changes: vec![Change {
                from,
                to,
                insert: end.slice(from..end_to).to_string(),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(doc: &str, changes: Vec<Change>) -> ChangeSet {
        ChangeSet::new(&Text::new(doc), changes).unwrap()
    }

    #[test]
    fn rejects_out_of_bounds_overlapping_and_split_characters() {
        let text = Text::new("héllo");
        assert!(matches!(
            ChangeSet::new(&text, vec![Change::insert(99, "x")]),
            Err(ChangeError::OutOfBounds { .. })
        ));
        assert!(matches!(
            ChangeSet::new(&text, vec![Change::delete(1..2)]),
            Err(ChangeError::NotCharBoundary { .. })
        ));
        assert!(matches!(
            ChangeSet::new(&text, vec![Change::delete(0..3), Change::delete(1..4)]),
            Err(ChangeError::Overlap { .. })
        ));
    }

    #[test]
    fn maps_positions_with_bias() {
        let changes = set(
            "abcdef",
            vec![Change::insert(2, "XY"), Change::delete(3..5)],
        );
        assert_eq!(changes.map_pos(1, Bias::Left), 1);
        assert_eq!(changes.map_pos(2, Bias::Left), 2);
        assert_eq!(changes.map_pos(2, Bias::Right), 4);
        assert_eq!(changes.map_pos(3, Bias::Left), 5);
        assert_eq!(changes.map_pos(4, Bias::Left), 5);
        assert_eq!(changes.map_pos(4, Bias::Right), 5);
        assert_eq!(changes.map_pos(5, Bias::Left), 5);
        assert_eq!(changes.map_pos(6, Bias::Left), 6);
    }

    #[test]
    fn invert_restores_the_start_document() {
        let doc = Text::new("hello world");
        let changes = set(
            "hello world",
            vec![Change::new(0..5, "bye"), Change::insert(11, "!")],
        );
        let mut text = doc.clone();
        changes.apply(&mut text);
        assert_eq!(text.as_str(), "bye world!");
        changes.invert(&doc).apply(&mut text);
        assert_eq!(text, doc);
    }

    #[test]
    fn compose_equals_applying_both() {
        let cases: Vec<(&str, Vec<Change>, Vec<Change>)> = vec![
            (
                "abc",
                vec![Change::insert(1, "x")],
                vec![Change::insert(2, "y")],
            ),
            (
                "abc",
                vec![Change::insert(3, "x")],
                vec![Change::delete(0..1)],
            ),
            (
                "hello",
                vec![Change::delete(1..3)],
                vec![Change::insert(3, "ZZ")],
            ),
            (
                "a\nb",
                vec![Change::new(0..1, "- ")],
                vec![Change::insert(4, "q")],
            ),
        ];
        for (doc, first, second) in cases {
            let start = Text::new(doc);
            let a = ChangeSet::new(&start, first).unwrap();
            let mut middle = start.clone();
            a.apply(&mut middle);
            let b = ChangeSet::new(&middle, second).unwrap();
            let mut expected = middle.clone();
            b.apply(&mut expected);
            let composed = a.compose(&start, &b);
            let mut actual = start.clone();
            composed.apply(&mut actual);
            assert_eq!(actual, expected, "{doc:?}");
        }
    }
}
