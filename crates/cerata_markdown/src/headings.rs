//! Document headings and their GFM anchors: `#slug` links (US-033) and the
//! section rail (US-038). Ported from `lib/heading-slug.ts` and
//! `hooks/use-document-headings.ts:27-80`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// github-slugger's rule: lowercase, drop U+2000-U+206F, U+2E00-U+2E7F and
/// ASCII punctuation except `-` and `_`, then one `-` per run of spaces.
pub fn slugify(text: &str) -> String {
    let kept: String = text
        .to_lowercase()
        .chars()
        .filter(|c| {
            !matches!(c, '\u{2000}'..='\u{206F}' | '\u{2E00}'..='\u{2E7F}')
                && !(c.is_ascii_punctuation() && *c != '-' && *c != '_')
        })
        .collect();
    kept.split(crate::decorate::is_js_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Unique slugs in document order: a repeat gets `-2`, `-3`, skipping any
/// slug already taken (`createHeadingSlugger`).
#[derive(Default)]
pub struct Slugger {
    used: HashSet<String>,
    counts: HashMap<String, usize>,
}

impl Slugger {
    pub fn slug(&mut self, text: &str) -> String {
        let base = slugify(text);
        let mut n = self.counts.get(&base).copied().unwrap_or(0) + 1;
        let mut candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        while self.used.contains(&candidate) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        self.counts.insert(base, n);
        self.used.insert(candidate.clone());
        candidate
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// Zero-based line number.
    pub line: usize,
    /// Byte offset of the line start.
    pub pos: usize,
    pub slug: String,
}

static ATX: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*#*\s*$").ok());
static FENCE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^(`{3,}|~{3,})").ok());

/// ATX headings up to `max_depth`, outside fences. Slugs count every
/// heading up to `slug_depth` (at least `max_depth`), so an anchor to a
/// deeper heading stays right when the list leaves it out.
pub fn parse(content: &str, max_depth: u8, slug_depth: u8) -> Vec<Heading> {
    let (Some(atx), Some(fence)) = (ATX.as_ref(), FENCE.as_ref()) else {
        return Vec::new();
    };
    let slug_depth = slug_depth.max(max_depth);
    let mut slugger = Slugger::default();
    let mut out = Vec::new();
    let mut open_fence: Option<(char, usize)> = None;
    let mut pos = 0;
    for (number, line) in content.split('\n').enumerate() {
        let trimmed = line.trim_start_matches(crate::decorate::is_js_space);
        if let Some(m) = fence.captures(trimmed).and_then(|c| c.get(1)) {
            let marker = m.as_str();
            let ch = marker.chars().next().unwrap_or('`');
            match open_fence {
                None => open_fence = Some((ch, marker.len())),
                Some((c, len)) if c == ch && marker.len() >= len => open_fence = None,
                Some(_) => {}
            }
        } else if open_fence.is_none()
            && let Some(caps) = atx.captures(line)
            && let (Some(hashes), Some(text)) = (caps.get(1), caps.get(2))
        {
            let level = hashes.as_str().len() as u8;
            let text = text.as_str().trim();
            if !text.is_empty() && level <= slug_depth {
                let slug = slugger.slug(text);
                if level <= max_depth {
                    out.push(Heading {
                        level,
                        text: text.to_owned(),
                        line: number,
                        pos,
                        slug,
                    });
                }
            }
        }
        pos += line.len() + 1;
    }
    out
}

/// The first heading of `content` whose slug is `slug`, any level.
pub fn find_by_slug(content: &str, slug: &str) -> Option<Heading> {
    parse(content, 6, 6).into_iter().find(|h| h.slug == slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `heading-slug.test.ts`.
    #[test]
    fn slugs_follow_github_slugger() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("foo.bar/baz"), "foobarbaz");
        assert_eq!(slugify("(parens) and [brackets]"), "parens-and-brackets");
        assert_eq!(slugify("Hello   World"), "hello-world");
        assert_eq!(slugify("\tHello\nWorld\t"), "hello-world");
        assert_eq!(slugify("Q3"), "q3");
        assert_eq!(slugify("GFM-style"), "gfm-style");
        assert_eq!(slugify("foo_bar"), "foo_bar");
        assert_eq!(slugify("Café"), "café");
        assert_eq!(slugify("日本語"), "日本語");
        assert_eq!(slugify("Title — Em Dash"), "title-em-dash");
        assert_eq!(slugify("1.0 Release Notes"), "10-release-notes");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn repeated_slugs_are_numbered_without_collisions() {
        let mut s = Slugger::default();
        assert_eq!(
            [s.slug("Title"), s.slug("Title"), s.slug("Title")],
            ["title", "title-2", "title-3"]
        );
        let mut s = Slugger::default();
        assert_eq!(
            [s.slug("Title 2"), s.slug("Title"), s.slug("Title")],
            ["title-2", "title", "title-3"]
        );
    }

    /// `document-headings.test.ts`.
    #[test]
    fn headings_skip_fences_and_count_deeper_levels() {
        assert!(parse("", 3, 3).is_empty());
        let h = parse("# Title\nSome body.\n## Section\n### Subsection\n", 3, 3);
        assert_eq!(
            h.iter()
                .map(|h| (h.level, h.text.as_str()))
                .collect::<Vec<_>>(),
            [(1, "Title"), (2, "Section"), (3, "Subsection")]
        );
        assert_eq!(parse("# A\n## B\n### C\n#### D", 2, 2).len(), 2);
        let h = parse("intro\n# Title\npara\n## Section", 3, 3);
        assert_eq!((h[0].line, h[0].pos), (1, 6));
        assert_eq!((h[1].line, h[1].pos), (3, "intro\n# Title\npara\n".len()));
        let texts = |c: &str| {
            parse(c, 3, 3)
                .into_iter()
                .map(|h| h.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            texts("# Real\n```\n# Fake\n## Also fake\n```\n## Real too"),
            ["Real", "Real too"]
        );
        assert_eq!(texts("# Real\n~~~\n# Fake\n~~~\n## Done"), ["Real", "Done"]);
        assert_eq!(texts("#NotHeading\n# Heading"), ["Heading"]);
        assert_eq!(texts("## Title ##"), ["Title"]);
        assert!(texts("# ").is_empty());
        assert_eq!(parse("# Hello, World!", 3, 3)[0].slug, "hello-world");
        assert_eq!(parse("# Café & Crème", 3, 3)[0].slug, "café-crème");
        let slugs = |h: Vec<Heading>| h.into_iter().map(|h| h.slug).collect::<Vec<_>>();
        assert_eq!(
            slugs(parse("# Setup\n## Setup\n### Setup", 3, 3)),
            ["setup", "setup-2", "setup-3"]
        );
        // The rail lists H1 and H2, the H3 in between still counts.
        assert_eq!(slugs(parse("# A\n### A\n## A", 2, 6)), ["a", "a-3"]);
        let found = find_by_slug("# A\n## B\n### A", "a-2").unwrap();
        assert_eq!(found.level, 3);
    }
}
