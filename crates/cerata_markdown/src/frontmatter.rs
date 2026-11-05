use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frontmatter {
    pub range: Range<usize>,
    pub opening_mark: Range<usize>,
    pub closing_mark: Range<usize>,
    pub content: Option<Range<usize>>,
    pub body_start: usize,
}

pub(crate) fn lines(text: &str, from: usize) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let bytes = text.as_bytes();
    let mut pos = (from <= bytes.len()).then_some(from);
    std::iter::from_fn(move || {
        let start = pos?;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'\n' && bytes[end] != b'\r' {
            end += 1;
        }
        if end >= bytes.len() {
            pos = None;
            return Some((start, end, end));
        }
        let next = if bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n') {
            end + 2
        } else {
            end + 1
        };
        pos = Some(next);
        Some((start, end, next))
    })
}

fn is_delimiter_line(line: &str) -> bool {
    line.strip_prefix("---")
        .is_some_and(|rest| rest.bytes().all(|b| b == b' ' || b == b'\t'))
}

pub fn detect(text: &str) -> Option<Frontmatter> {
    let mut iter = lines(text, 0);
    let (open_start, open_end, content_from) = iter.next()?;
    if !is_delimiter_line(&text[open_start..open_end]) {
        return None;
    }
    for (start, end, next) in iter {
        if !is_delimiter_line(&text[start..end]) {
            continue;
        }
        let content_to = break_start(text, start);
        return Some(Frontmatter {
            range: 0..end,
            opening_mark: 0..3,
            closing_mark: start..start + 3,
            content: (content_from < content_to).then_some(content_from..content_to),
            body_start: next,
        });
    }
    None
}

fn break_start(text: &str, line_start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = line_start;
    if pos > 0 && bytes[pos - 1] == b'\n' {
        pos -= 1;
    }
    if pos > 0 && bytes[pos - 1] == b'\r' {
        pos -= 1;
    }
    pos
}

pub fn join(frontmatter: Option<&str>, body: &str, eol: &str) -> String {
    match frontmatter {
        None => body.to_owned(),
        Some(yaml) => format!("---{eol}{yaml}{eol}---{eol}{body}"),
    }
}

pub fn split(text: &str) -> (Option<&str>, &str) {
    match detect(text) {
        Some(fm) => (
            Some(fm.content.map_or("", |r| &text[r])),
            text.get(fm.body_start..).unwrap_or(""),
        ),
        None => (None, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_writes_writers_delimiters_and_round_trips() {
        assert_eq!(
            join(Some("title: Hello"), "# Content", "\n"),
            "---\ntitle: Hello\n---\n# Content"
        );
        assert_eq!(join(None, "# Content", "\n"), "# Content");
        assert_eq!(join(Some(""), "Body", "\n"), "---\n\n---\nBody");
        for original in [
            "---\ntitle: Test\ntags: [a, b]\n---\n\n# Hello\n\nWorld",
            "---\n\n---\nBody",
            "---\r\ntitle: Test\r\n---\r\nBody",
        ] {
            let (frontmatter, body) = split(original);
            let eol = if original.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            assert_eq!(join(frontmatter, body, eol), original, "{original:?}");
        }
        assert_eq!(split("---\n\n---\nBody").0, Some(""));
    }

    #[test]
    fn a_body_is_written_verbatim_under_its_frontmatter() {
        assert_eq!(
            join(None, "# Project Plan\n\nBody text.", "\n"),
            "# Project Plan\n\nBody text."
        );
        assert_eq!(
            join(Some("title: Hello"), "Body text.", "\n"),
            "---\ntitle: Hello\n---\nBody text."
        );
    }

    #[test]
    fn only_at_offset_zero_and_closed() {
        assert!(detect("---\ntitle: a\n---\nbody").is_some());
        assert!(detect("\n---\ntitle: a\n---\n").is_none());
        assert!(detect("text\n---\na\n---\n").is_none());
        assert!(detect("---\ntitle: a\n").is_none());
        assert!(detect(" ---\na\n---").is_none());
    }

    #[test]
    fn ranges_match_writer() {
        let fm = detect("---\ntitle: a\n---\nbody").unwrap();
        assert_eq!(fm.range, 0..16);
        assert_eq!(fm.closing_mark, 13..16);
        assert_eq!(fm.content, Some(4..12));
        assert_eq!(fm.body_start, 17);
        let empty = detect("---\n---").unwrap();
        assert_eq!(empty.content, None);
        assert_eq!(empty.range, 0..7);
        assert_eq!(detect("--- \na\n---  \nx").unwrap().closing_mark, 7..10);
    }

    #[test]
    fn split_returns_yaml_and_body() {
        assert_eq!(split("---\na: 1\n---\nhi"), (Some("a: 1"), "hi"));
        assert_eq!(split("---\n---\n"), (Some(""), ""));
        assert_eq!(split("# no"), (None, "# no"));
        assert_eq!(split("---\r\na: 1\r\n---\r\nhi"), (Some("a: 1"), "hi"));
    }

    #[test]
    fn split_matches_writers_cases() {
        assert_eq!(
            split("---\ntitle: Hello\ndate: 2024-01-01\n---\n# Content"),
            (Some("title: Hello\ndate: 2024-01-01"), "# Content")
        );
        let plain = "# Just a heading\n\nSome text";
        assert_eq!(split(plain), (None, plain));
        assert_eq!(
            split("---\ntitle: Test\n---\n\n# Content"),
            (Some("title: Test"), "\n# Content")
        );
        assert_eq!(split("---\n\n---\nBody"), (Some(""), "Body"));
        let mid = "Some text\n---\ntitle: Nope\n---\nMore text";
        assert_eq!(split(mid), (None, mid));
        assert_eq!(split("---\n---\nBody"), (Some(""), "Body"));
        assert_eq!(split("---\n---"), (Some(""), ""));
        assert_eq!(split(""), (None, ""));
    }
}
