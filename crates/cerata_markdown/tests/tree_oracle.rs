//! The `MarkdownTree` of every corpus fixture equals the Lezer tree Writer's
//! own parser builds (`tests/fixtures/oracle/trees`, recorded by
//! `scripts/oracle`: `bun oracle.ts trees ...`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cerata_markdown::{MarkdownTree, Node, NodeKind};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Fixture path for an oracle file stem.
pub fn fixture_path(stem: &str) -> PathBuf {
    for dir in ["corpus", "parser_spike"] {
        let path = fixtures().join(dir).join(format!("{stem}.md"));
        if path.exists() {
            return path;
        }
    }
    panic!("no fixture for {stem}");
}

/// Lezer-format dump without Cerata's own `WikiLink` wrappers.
fn lezer_dump(tree: &MarkdownTree) -> String {
    fn walk(node: Node<'_>, depth: usize, out: &mut String) {
        let depth = if node.kind() == NodeKind::WikiLink {
            depth
        } else {
            out.push_str(&format!(
                "{}{} {}..{}\n",
                "  ".repeat(depth),
                node.name(),
                node.from(),
                node.to()
            ));
            depth + 1
        };
        for child in node.children() {
            walk(child, depth, out);
        }
    }
    let mut out = String::new();
    walk(tree.root(), 0, &mut out);
    out
}

#[test]
fn trees_match_writers_lezer_trees() {
    let dir = fixtures().join("oracle/trees");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    assert!(entries.len() >= 90, "oracle trees missing");
    let mut failures = Vec::new();
    for path in &entries {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let expected = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let doc = std::fs::read_to_string(fixture_path(stem)).unwrap();
        let actual = lezer_dump(&MarkdownTree::parse(&doc));
        if actual != expected {
            let first = expected
                .lines()
                .zip(actual.lines())
                .position(|(e, a)| e != a)
                .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
            failures.push(format!(
                "{stem}: line {}\n  expected: {:?}\n  actual:   {:?}",
                first + 1,
                expected.lines().nth(first).unwrap_or("<end>"),
                actual.lines().nth(first).unwrap_or("<end>"),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} trees differ from Writer:\n{}",
        failures.len(),
        entries.len(),
        failures.join("\n")
    );
}
