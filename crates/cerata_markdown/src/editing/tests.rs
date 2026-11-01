use super::*;
use crate::code::{CodeBlockKind, code_block_at, code_blocks};
use crate::decorate::{Decoration, LineStyle, TAB_WIDTH_CH, Widget};

fn marked(marked: &str) -> EditorState {
    let anchor = marked.find('|').unwrap();
    let rest = &marked[anchor + 1..];
    let head = rest.find('|').map_or(anchor, |offset| anchor + offset);
    let mut state = EditorState::new(marked.replace('|', ""));
    state.select(Selection::single(anchor, head)).unwrap();
    state
}

fn marked_doc(state: &EditorState) -> String {
    let doc = state.doc();
    let head = state.selection().main().head;
    format!("{}|{}", &doc[..head], &doc[head..])
}

fn run(command: fn(&mut EditorState) -> bool, doc: &str) -> Option<String> {
    let mut s = marked(doc);
    command(&mut s).then(|| marked_doc(&s))
}

// Ported from Writer `tests/blockquote-enter.test.ts:34-58`.
#[test]
fn enter_on_an_empty_quote_line_removes_one_level() {
    let enter = EditorState::blockquote_enter;
    assert_eq!(
        run(enter, "> A quoted line\n> |").as_deref(),
        Some("> A quoted line\n|")
    );
    assert_eq!(
        run(enter, "> outer\n> > nested\n> > |").as_deref(),
        Some("> outer\n> > nested\n> |")
    );
    assert_eq!(run(enter, "> a\n\n> |").as_deref(), Some("> a\n\n|"));
    assert_eq!(
        run(enter, "> a\n> |\n## Next").as_deref(),
        Some("> a\n|\n## Next")
    );
    assert_eq!(run(enter, "> text|"), None);
    assert_eq!(run(enter, "> a\n>|"), None);
    assert_eq!(run(enter, "```\n> |\n```"), None);
}

#[test]
fn enter_runs_the_handlers_in_writers_order() {
    let enter = EditorState::insert_newline;
    // Quote, then list, then the indent-only line, then markup, then plain.
    assert_eq!(run(enter, "> a\n> |").as_deref(), Some("> a\n|"));
    assert_eq!(run(enter, "- a|").as_deref(), Some("- a\n- |"));
    assert_eq!(
        run(enter, "- a\n\n  para\n\n  |").as_deref(),
        Some("- a\n\n  para\n\n|")
    );
    assert_eq!(run(enter, "1. One|").as_deref(), Some("1. One\n2. |"));
    assert_eq!(
        run(enter, "1. a|\n2. b\n3. c").as_deref(),
        Some("1. a\n2. |\n3. b\n4. c")
    );
    assert_eq!(run(enter, "> text|").as_deref(), Some("> text\n> |"));
    assert_eq!(run(enter, "> - a|").as_deref(), Some("> - a\n> - |"));
    assert_eq!(run(enter, "  para|").as_deref(), Some("  para\n  |"));
    assert_eq!(run(enter, "plain|").as_deref(), Some("plain\n|"));
    // In a fence nothing list-like happens.
    assert_eq!(
        run(enter, "```\n- |\n```").as_deref(),
        Some("```\n- \n|\n```")
    );
    // Multiple carets each get a newline.
    let mut s = EditorState::new("a\nb");
    s.select(Selection::new(
        vec![SelectionRange::cursor(1), SelectionRange::cursor(3)],
        0,
    ))
    .unwrap();
    assert!(s.insert_newline());
    assert_eq!(s.doc(), "a\n\nb\n");
}

#[test]
fn backspace_and_tab_fall_back_to_plain_edits() {
    assert_eq!(
        run(EditorState::delete_backward, "- |foo").as_deref(),
        Some("|foo")
    );
    assert_eq!(
        run(EditorState::delete_backward, "ab|c").as_deref(),
        Some("a|c")
    );
    assert_eq!(
        run(EditorState::delete_backward, "a\r\n|b").as_deref(),
        Some("a|b")
    );
    assert_eq!(
        run(EditorState::delete_backward, "é|").as_deref(),
        Some("|")
    );
    assert_eq!(
        run(EditorState::delete_backward, "a|bc|d").as_deref(),
        Some("a|d")
    );
    assert_eq!(
        run(EditorState::indent, "- a\n- b|").as_deref(),
        Some("- a\n  - b|")
    );
    assert_eq!(run(EditorState::indent, "x|").as_deref(), Some("x\t|"));
    assert_eq!(
        run(EditorState::outdent, "- a\n  - b|").as_deref(),
        Some("- a\n- b|")
    );
    assert_eq!(run(EditorState::outdent, "x|"), None);
}

#[test]
fn editing_commands_do_nothing_while_loading() {
    let mut s = EditorState::loading();
    assert!(!s.insert_newline());
    assert!(!s.delete_backward());
    assert!(!s.indent());
    assert!(!s.list_enter());
    assert_eq!(s.doc(), "");
}

#[test]
fn home_and_end_go_to_visual_line_boundaries() {
    let home = |doc: &str| {
        let mut s = marked(doc);
        assert!(s.move_line_boundary_unwrapped(false, false));
        marked_doc(&s)
    };
    let end = |doc: &str| {
        let mut s = marked(doc);
        assert!(s.move_line_boundary_unwrapped(true, false));
        marked_doc(&s)
    };
    assert_eq!(end("a|bc\nd"), "abc|\nd");
    assert_eq!(home("ab|c"), "|abc");
    // Home stops at the first non-blank first, then at the line start.
    assert_eq!(home("   ab|c"), "   |abc");
    assert_eq!(home("   |abc"), "|   abc");
    // A wrapped row: the renderer's visual line decides.
    let mut s = marked("hello |world again");
    let rows = |pos: usize| if pos < 6 { 0..6 } else { 6..17 };
    assert!(s.move_line_boundary(true, false, &rows));
    assert_eq!(s.selection().main().head, 17);
    assert!(s.move_line_boundary(false, true, &rows));
    assert_eq!(s.selection().main(), SelectionRange::new(17, 6));
    // A row boundary on the wrong side falls back to the document line.
    let mut s = marked("abc|def");
    assert!(s.move_line_boundary(true, false, &|_| 0..1));
    assert_eq!(s.selection().main().head, 6);
}

#[test]
fn code_blocks_label_their_language_and_copy_without_fences() {
    let doc =
        "---\ntitle: x\n---\n\n```rust\nfn a() {}\n```\n\n    indented\n    code\n\n```foo\nz\n";
    let s = EditorState::new(doc);
    let blocks = code_blocks(s.tree(), s.text());
    assert_eq!(blocks.len(), 4);
    assert_eq!(
        (
            blocks[0].kind,
            blocks[0].label.as_str(),
            blocks[0].code.as_str()
        ),
        (CodeBlockKind::Frontmatter, "YAML (FRONTMATTER)", "title: x")
    );
    assert_eq!(
        (
            blocks[1].label.as_str(),
            blocks[1].code.as_str(),
            blocks[1].language.as_str()
        ),
        ("RUST", "fn a() {}", "rust")
    );
    assert_eq!(
        (blocks[2].kind, blocks[2].code.as_str()),
        (CodeBlockKind::Indented, "indented\ncode")
    );
    // An unclosed fence of an unknown language: code to the end.
    assert_eq!(
        (blocks[3].label.as_str(), blocks[3].code.as_str()),
        ("FOO", "z\n")
    );
    // The caret right after a closing fence is still in its block.
    let after = doc.find("```\n\n").unwrap() + 3;
    assert_eq!(
        code_block_at(s.tree(), after).map(|n| n.kind()),
        Some(NodeKind::FencedCode)
    );
}

#[test]
fn code_quote_and_tab_chrome() {
    let doc = "```py\nx\n```\n\n> a\n> > b\n\nx\tt";
    let d = EditorState::new(doc).decorations();
    let lines: Vec<(usize, LineStyle)> = d
        .iter()
        .filter_map(|d| match d {
            Decoration::Line { line_start, style } => Some((*line_start, *style)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lines,
        [
            (
                0,
                LineStyle::Code {
                    first: true,
                    last: false
                }
            ),
            (
                6,
                LineStyle::Code {
                    first: false,
                    last: false
                }
            ),
            (
                8,
                LineStyle::Code {
                    first: false,
                    last: true
                }
            ),
            (13, LineStyle::Quote),
            (17, LineStyle::Quote),
        ]
    );
    assert!(
        LineStyle::Code {
            first: true,
            last: false
        }
        .is_monospace()
    );
    assert!(d.contains(&Decoration::Widget {
        pos: 0,
        widget: Widget::CodeInfo {
            label: "PY".into(),
            code: "x".into()
        },
        block: false
    }));
    // The nested quote's bar sits at its inner `>`.
    assert!(d.contains(&Decoration::Widget {
        pos: 19,
        widget: Widget::NestedQuoteBorder,
        block: false
    }));
    let tab = doc.find('\t').unwrap();
    assert!(d.contains(&Decoration::Replace {
        range: tab..tab + 1,
        widget: Widget::Tab,
        block: false
    }));
    assert_eq!(TAB_WIDTH_CH, 4);
}
