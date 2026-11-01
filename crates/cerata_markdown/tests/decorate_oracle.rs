//! US-011 and US-012: `decorate` gives exactly the decorations Writer's
//! `hideExtension` and `foldExtension` hold, on every fixture of the corpus
//! and of the parser spike, for every caret of the snapshot: document
//! bounds, line bounds, node bounds, two carets at once, short ranges and
//! the whole document. Snapshots come from Writer itself, run headless by
//! `scripts/oracle/oracle.ts snapshots` (UTF-8 byte offsets). Line styles
//! come from Writer view plugins the oracle cannot capture; they are left
//! out here and tested in `src/decorate/tests.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cerata_markdown::{
    Decoration, EditorState, HideMode, MarkStyle, Selection, SelectionRange, Widget, decorate,
};
use std::path::Path;

fn describe(decoration: &Decoration) -> Option<String> {
    let range = decoration.range();
    let span = format!("{}..{}", range.start, range.end);
    Some(match decoration {
        Decoration::Hide { mode, .. } => match mode {
            HideMode::Collapse | HideMode::Remove => format!("hide {span}"),
            HideMode::Block => format!("hide-block {span}"),
            HideMode::KeepSpace => format!("invisible {span}"),
        },
        Decoration::Mark { style, .. } => {
            let class = match style {
                MarkStyle::Emphasis => "cm-emphasis",
                MarkStyle::InlineCode => "cm-inline-code",
                MarkStyle::Strikethrough => "cm-strikethrough",
                MarkStyle::RenderedLink => "cm-rendered-link",
            };
            format!("mark {class} {span}")
        }
        Decoration::Replace { widget, block, .. } | Decoration::Widget { widget, block, .. } => {
            let kind = if matches!(decoration, Decoration::Replace { .. }) {
                "replace"
            } else {
                "widget"
            };
            let name = match widget {
                Widget::Image { url } => format!("ImageWidget {url}"),
                Widget::HorizontalRule => "HorizontalRuleWidget".to_owned(),
                Widget::Dash(count) => format!("DashWidget {count}"),
                Widget::Emoji(emoji) => format!("EmojiWidget {emoji}"),
                // View-plugin chrome the oracle cannot capture (see top),
                // and the rich blocks of Writer's own extensions (tables,
                // math, Mermaid, HTML), outside prosemark-core's fold set.
                _ => return None,
            };
            let block = if *block { "block" } else { "inline" };
            format!("{kind} {block} {name} {span}")
        }
        Decoration::Line { .. } => return None,
    })
}

#[test]
fn decorations_match_writer_snapshots() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut fixtures = 0;
    let mut cases = 0;
    let mut failures = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(root.join("oracle/decorate"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let snapshot: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let name = snapshot["fixture"].as_str().unwrap();
        let source = ["corpus", "parser_spike"]
            .iter()
            .map(|dir| root.join(dir).join(name))
            .find(|p| p.exists())
            .unwrap_or_else(|| panic!("no fixture for {name}"));
        let doc = std::fs::read_to_string(source).unwrap();
        // No selection filters: the snapshots were taken without them.
        let mut state = EditorState::with_filters(doc, Vec::new());
        fixtures += 1;
        for case in snapshot["cases"].as_array().unwrap() {
            let ranges: Vec<SelectionRange> = case["selection"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    SelectionRange::new(
                        r[0].as_u64().unwrap() as usize,
                        r[1].as_u64().unwrap() as usize,
                    )
                })
                .collect();
            let selection = Selection::new(ranges, 0);
            state.select(selection.clone()).unwrap();
            let expected: Vec<String> = case["decorations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_str().unwrap().to_owned())
                .collect();
            // The entry point the editor uses, and the pure function.
            let actual: Vec<String> = state.decorations().iter().filter_map(describe).collect();
            let pure: Vec<String> = decorate(state.tree(), state.text(), &selection, None)
                .iter()
                .filter_map(describe)
                .collect();
            assert_eq!(actual, pure);
            cases += 1;
            if actual != expected {
                let missing: Vec<_> = expected.iter().filter(|d| !actual.contains(d)).collect();
                let extra: Vec<_> = actual.iter().filter(|d| !expected.contains(d)).collect();
                failures.push(format!(
                    "{name} {:?}: missing {missing:?}, extra {extra:?}",
                    case["selection"]
                ));
            }
        }
    }
    assert!(fixtures >= 50, "only {fixtures} fixtures");
    assert!(
        failures.is_empty(),
        "{} of {cases} cases differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
