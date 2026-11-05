use std::ops::Range;

use yaml_rust2::{Yaml, YamlLoader};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: String,
    pub complex: bool,
    span: Range<usize>,
    key_end: usize,
    value_end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Properties {
    Entries {
        entries: Vec<Entry>,
        prefix: Range<usize>,
    },
    Invalid(String),
}

#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    pub key: &'a str,
    pub value: &'a str,
    pub origin: Option<usize>,
}

fn load(source: &str) -> Result<Option<Yaml>, String> {
    YamlLoader::load_from_str(source)
        .map(|docs| docs.into_iter().next())
        .map_err(|e| e.to_string())
}

fn scalar_text(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(s) if !s.contains('\n') => Some(s.clone()),
        Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        Yaml::Null => Some(String::new()),
        _ => None,
    }
}

fn key_text(key: &Yaml) -> Option<String> {
    match key {
        Yaml::Null => Some("null".to_owned()),
        other => scalar_text(other),
    }
}

fn lines(text: &str) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    crate::frontmatter::lines(text, 0)
        .filter(move |(start, _, next)| start < next || *start < text.len())
}

fn starts_entry(line: &str) -> bool {
    line.bytes()
        .next()
        .is_some_and(|b| !matches!(b, b' ' | b'\t' | b'#' | b'-'))
}

fn key_end(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    if let Some(&quote @ (b'"' | b'\'')) = bytes.first() {
        i = 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && quote == b'"' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                break;
            }
            i += 1;
        }
        i += 1;
    }
    while i < bytes.len() {
        if bytes[i] == b':' && bytes.get(i + 1).is_none_or(|b| matches!(b, b' ' | b'\t')) {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn parse(yaml: &str) -> Properties {
    let doc = match load(yaml) {
        Ok(doc) => doc,
        Err(error) => return Properties::Invalid(error),
    };
    let hash = match doc {
        None | Some(Yaml::Null) => {
            return Properties::Entries {
                entries: Vec::new(),
                prefix: 0..yaml.len(),
            };
        }
        Some(Yaml::Hash(hash)) => hash,
        Some(_) => {
            return Properties::Invalid("The frontmatter is not a list of properties".into());
        }
    };
    let not_editable = || Properties::Invalid("These properties can only be edited as text".into());

    let starts: Vec<(usize, usize)> = lines(yaml)
        .filter(|&(start, end, _)| starts_entry(&yaml[start..end]))
        .map(|(start, end, _)| (start, end))
        .collect();
    if starts.len() != hash.len() {
        return not_editable();
    }
    let mut entries = Vec::with_capacity(hash.len());
    for (i, ((key, value), &(start, end))) in hash.iter().zip(&starts).enumerate() {
        let line = &yaml[start..end];
        let Some(k_end) = key_end(line) else {
            return not_editable();
        };
        let written = load(&format!("{}: x", &line[..k_end]));
        let matches =
            matches!(written, Ok(Some(Yaml::Hash(ref h))) if h.keys().next() == Some(key));
        let Some(key) = key_text(key).filter(|_| matches) else {
            return not_editable();
        };
        let span_end = starts.get(i + 1).map_or(yaml.len(), |&(next, _)| next);
        let mut value_end = end;
        for (s, e, _) in crate::frontmatter::lines(yaml, start).skip(1) {
            if s >= span_end || !yaml[s..e].starts_with([' ', '\t', '-']) {
                break;
            }
            value_end = e;
        }
        let (text, complex) = match scalar_text(value) {
            Some(text) => (text, false),
            None => {
                let source = yaml[start + k_end + 1..value_end]
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                (source, true)
            }
        };
        entries.push(Entry {
            key,
            value: text,
            complex,
            span: start..span_end,
            key_end: start + k_end,
            value_end,
        });
    }
    let prefix = 0..starts.first().map_or(yaml.len(), |&(start, _)| start);
    Properties::Entries { entries, prefix }
}

fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn format_key(key: &str) -> String {
    let plain = key == key.trim()
        && !key.is_empty()
        && matches!(load(&format!("{key}: x")), Ok(Some(Yaml::Hash(ref h)))
            if h.len() == 1 && h.keys().next().and_then(key_text).as_deref() == Some(key));
    if plain { key.to_owned() } else { quoted(key) }
}

fn format_value(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_owned();
    }
    let plain = value == value.trim()
        && matches!(load(&format!("k: {value}")), Ok(Some(Yaml::Hash(ref h)))
            if h.len() == 1 && h.values().next().and_then(scalar_text).as_deref() == Some(value));
    if plain {
        value.to_owned()
    } else {
        quoted(value)
    }
}

fn strip_break(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

pub fn write(yaml: &str, properties: &Properties, rows: &[Row<'_>]) -> Option<String> {
    let Properties::Entries { entries, prefix } = properties else {
        return None;
    };
    let eol = if yaml.contains("\r\n") { "\r\n" } else { "\n" };
    let mut pieces: Vec<String> = Vec::new();
    let head = strip_break(&yaml[prefix.clone()]);
    if !head.is_empty() {
        pieces.push(head.to_owned());
    }
    for row in rows.iter().filter(|r| !r.key.trim().is_empty()) {
        let entry = row.origin.and_then(|i| entries.get(i));
        let piece = match entry {
            Some(e) if e.key == row.key && (e.complex || e.value == row.value) => {
                yaml[e.span.clone()].to_owned()
            }
            Some(e) if e.complex => {
                format!("{}{}", format_key(row.key), &yaml[e.key_end..e.span.end])
            }
            Some(e) => format!(
                "{}: {}{}",
                format_key(row.key),
                format_value(row.value),
                &yaml[e.value_end..e.span.end]
            ),
            None => format!("{}: {}", format_key(row.key), format_value(row.value)),
        };
        pieces.push(strip_break(&piece).to_owned());
    }
    Some(pieces.join(eol))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(yaml: &str) -> Vec<(String, String, bool)> {
        match parse(yaml) {
            Properties::Entries { entries, .. } => entries
                .into_iter()
                .map(|e| (e.key, e.value, e.complex))
                .collect(),
            Properties::Invalid(error) => panic!("invalid: {error}"),
        }
    }

    fn e(key: &str, value: &str, complex: bool) -> (String, String, bool) {
        (key.into(), value.into(), complex)
    }

    fn rows(properties: &Properties) -> Vec<(String, String)> {
        match properties {
            Properties::Entries { entries, .. } => entries
                .iter()
                .map(|e| (e.key.clone(), e.value.clone()))
                .collect(),
            Properties::Invalid(_) => Vec::new(),
        }
    }

    fn rewrite(yaml: &str, edited: &[(&str, &str, Option<usize>)]) -> String {
        let properties = parse(yaml);
        let rows: Vec<Row<'_>> = edited
            .iter()
            .map(|&(key, value, origin)| Row { key, value, origin })
            .collect();
        write(yaml, &properties, &rows).unwrap()
    }

    #[test]
    fn parses_simple_key_value_pairs() {
        assert_eq!(
            entries("title: Hello\nauthor: Jane"),
            vec![e("title", "Hello", false), e("author", "Jane", false)]
        );
    }

    #[test]
    fn parses_booleans_and_numbers() {
        assert_eq!(
            entries("draft: false\nweight: 42\npi: 3.14"),
            vec![
                e("draft", "false", false),
                e("weight", "42", false),
                e("pi", "3.14", false)
            ]
        );
    }

    #[test]
    fn parses_arrays_as_complex() {
        let parsed = entries("tags:\n  - markdown\n  - test");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "tags");
        assert!(parsed[0].2);
        assert!(parsed[0].1.contains("markdown") && parsed[0].1.contains("test"));
    }

    #[test]
    fn parses_flow_arrays_and_nested_objects_as_complex() {
        assert!(entries("tags: [a, b, c]")[0].2);
        let nested = entries("nested:\n  key: value\n  other: 123");
        assert_eq!(nested[0].0, "nested");
        assert!(nested[0].2);
    }

    #[test]
    fn handles_null_values() {
        let parsed = entries("empty: null\nalso_empty:");
        assert_eq!(parsed[0].1, "");
        assert_eq!(parsed[1].1, "");
    }

    #[test]
    fn returns_no_entries_for_an_empty_string() {
        assert!(entries("").is_empty());
        assert!(entries("   ").is_empty());
    }

    #[test]
    fn a_non_mapping_or_invalid_yaml_is_not_editable() {
        assert!(matches!(parse("just a string"), Properties::Invalid(_)));
        assert!(matches!(parse("- a\n- b"), Properties::Invalid(_)));
        let Properties::Invalid(error) = parse("title: [unclosed") else {
            panic!("invalid YAML parsed");
        };
        assert!(!error.is_empty());
        assert!(write("title: [unclosed", &parse("title: [unclosed"), &[]).is_none());
    }

    #[test]
    fn serializes_simple_entries() {
        let yaml = rewrite("", &[("title", "Hello", None), ("draft", "false", None)]);
        assert_eq!(yaml, "title: Hello\ndraft: false");
    }

    #[test]
    fn skips_entries_with_empty_keys() {
        let yaml = rewrite("", &[("", "orphan", None), ("valid", "kept", None)]);
        assert_eq!(yaml, "valid: kept");
        assert_eq!(rewrite("", &[]), "");
        assert_eq!(rewrite("", &[("", "x", None)]), "");
    }

    #[test]
    fn coerces_scalar_types_and_quotes_what_yaml_would_misread() {
        let yaml = rewrite(
            "",
            &[
                ("bool", "true", None),
                ("num", "42", None),
                ("str", "hello", None),
                ("code", "007", None),
                ("colon", "a: b", None),
                ("hash", "x #y", None),
                ("empty", "", None),
                ("a key", "v", None),
                ("k: 1", "v", None),
            ],
        );
        assert_eq!(
            yaml,
            "bool: true\nnum: 42\nstr: hello\ncode: \"007\"\ncolon: \"a: b\"\nhash: \"x #y\"\nempty: \"\"\na key: v\n\"k: 1\": v"
        );
        assert_eq!(
            rows(&parse(&yaml)),
            [
                ("bool", "true"),
                ("num", "42"),
                ("str", "hello"),
                ("code", "007"),
                ("colon", "a: b"),
                ("hash", "x #y"),
                ("empty", ""),
                ("a key", "v"),
                ("k: 1", "v"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
    }

    #[test]
    fn simple_frontmatter_round_trips() {
        let original = "title: Test\nauthor: Jane\ndraft: false";
        let properties = parse(original);
        let all: Vec<(String, String)> = rows(&properties);
        let edited: Vec<(&str, &str, Option<usize>)> = all
            .iter()
            .enumerate()
            .map(|(i, (k, v))| (k.as_str(), v.as_str(), Some(i)))
            .collect();
        assert_eq!(rewrite(original, &edited), original);
    }

    #[test]
    fn non_scalar_values_and_comments_are_written_back_as_they_were() {
        let yaml = "# about\ntitle: Old   # inline\ntags:\n- a\n- b\nmeta: {x: 1, y: [2, 3]}\n\n# trailing\nlong: |\n  one\n  two";
        let parsed = rows(&parse(yaml));
        assert_eq!(parsed[0], ("title".into(), "Old".into()));
        assert_eq!(parsed[1], ("tags".into(), "- a - b".into()));
        let written = rewrite(
            yaml,
            &[
                ("title", "New", Some(0)),
                ("tags", &parsed[1].1, Some(1)),
                ("meta", &parsed[2].1, Some(2)),
                ("long", &parsed[3].1, Some(3)),
            ],
        );
        assert_eq!(
            written,
            "# about\ntitle: New\ntags:\n- a\n- b\nmeta: {x: 1, y: [2, 3]}\n\n# trailing\nlong: |\n  one\n  two"
        );
    }

    #[test]
    fn a_renamed_list_keeps_its_items_and_a_removed_entry_its_neighbors() {
        let yaml = "a: 1\ntags:\n  - x\nb: 2";
        assert_eq!(
            rewrite(yaml, &[("labels", "- x", Some(1)), ("b", "2", Some(2))]),
            "labels:\n  - x\nb: 2"
        );
        assert_eq!(
            rewrite(
                yaml,
                &[("a", "1", Some(0)), ("b", "2", Some(2)), ("c", "3", None)]
            ),
            "a: 1\nb: 2\nc: 3"
        );
    }

    #[test]
    fn a_multi_line_plain_scalar_is_rewritten_whole() {
        let yaml = "summary: one\n  two\nnext: x";
        assert_eq!(rows(&parse(yaml))[0].1, "one two");
        assert_eq!(
            rewrite(
                yaml,
                &[("summary", "short", Some(0)), ("next", "x", Some(1))]
            ),
            "summary: short\nnext: x"
        );
    }

    #[test]
    fn crlf_is_kept() {
        let yaml = "a: 1\r\nb: 2";
        assert_eq!(
            rewrite(yaml, &[("a", "3", Some(0)), ("b", "2", Some(1))]),
            "a: 3\r\nb: 2"
        );
    }
}
