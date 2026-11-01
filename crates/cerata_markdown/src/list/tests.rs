//! Ported from Writer `tests/list-extension.test.ts:88-1080`. `|` marks the
//! caret, two `|` a selection (anchor first).

// Expected values are written as lists of ranges; float fields are
// compared in guards.
#![allow(clippy::single_range_in_vec_init, clippy::redundant_guards)]

use super::*;

fn state(doc: &str, anchor: usize, head: usize) -> EditorState {
    let mut state = EditorState::new(doc);
    state.select(Selection::single(anchor, head)).unwrap();
    state
}

fn marked(marked: &str) -> EditorState {
    let anchor = marked.find('|').unwrap();
    let rest = &marked[anchor + 1..];
    let head = rest.find('|').map_or(anchor, |offset| anchor + offset);
    state(&marked.replace('|', ""), anchor, head)
}

fn marked_doc(state: &EditorState) -> String {
    let doc = state.doc();
    let head = state.selection().main().head;
    format!("{}|{}", &doc[..head], &doc[head..])
}

/// Runs `command` on `doc`: the marked result, or `None` when it passed.
fn run(command: fn(&mut EditorState) -> bool, doc: &str) -> Option<String> {
    let mut s = marked(doc);
    command(&mut s).then(|| marked_doc(&s))
}

fn text_after(command: fn(&mut EditorState) -> bool, doc: &str) -> String {
    let mut s = marked(doc);
    assert!(command(&mut s), "{doc:?} passed");
    s.doc().to_owned()
}

fn on_list_line(doc: &str, pos: usize) -> bool {
    let s = EditorState::new(doc);
    is_on_list_line(s.tree(), s.text(), pos)
}

#[test]
fn is_on_list_line_reads_the_tree() {
    assert!(on_list_line("- foo", 0) && on_list_line("- foo", 5));
    assert!(on_list_line("- [ ] task", 0) && on_list_line("- [ ] task", 10));
    assert!(on_list_line("- [ ] ", 6));
    assert!(!on_list_line("just text", 4));
    assert!(on_list_line("1. foo", 3));
}

#[test]
fn list_line_at_is_gated_by_the_tree() {
    let doc = "```\n- item\n```\n";
    let s = EditorState::new(doc);
    assert!(list_line_at(s.tree(), s.text(), doc.find("item").unwrap()).is_none());
    // The caret guard leaves carets alone inside a fence.
    let between = doc.find("- item").unwrap() + 1;
    let mut s = EditorState::new(doc);
    s.select(Selection::cursor(between)).unwrap();
    assert_eq!(s.selection().main().head, between);
    // Backspace there is not a list operation.
    let mut s = state(doc, doc.find("item").unwrap(), doc.find("item").unwrap());
    assert!(!s.list_backspace());
}

#[test]
fn a_tab_after_the_marker_is_a_list_line() {
    let s = EditorState::new("-\titem");
    let parsed = list_line_at(s.tree(), s.text(), 3).unwrap();
    assert_eq!(
        (parsed.marker_from, parsed.body_from, parsed.is_task),
        (0, 2, false)
    );
    assert_eq!(
        text_after(EditorState::list_enter, "-\titem|"),
        "-\titem\n-\t"
    );
}

#[test]
fn parses_bullet_and_task_prefixes() {
    assert_eq!(
        parse_bullet_task_line(0, "    - item"),
        Some(BulletTaskLine {
            line_from: 0,
            marker_from: 4,
            body_from: 6,
            indent_len: 4,
            marker_len: 2,
            is_task: false
        })
    );
    assert_eq!(
        parse_bullet_task_line(0, "  - [x] item"),
        Some(BulletTaskLine {
            line_from: 0,
            marker_from: 2,
            body_from: 8,
            indent_len: 2,
            marker_len: 6,
            is_task: true
        })
    );
    assert_eq!(parse_bullet_task_line(0, "1. item"), None);
    // US-013: `-text` without a space is not a list line.
    assert_eq!(parse_bullet_task_line(0, "-text"), None);
    let s = EditorState::new("-text");
    assert!(s.list_decorations().all.is_empty());
    assert!(!on_list_line("-text", 2));
}

const NESTED: &str = "- parent\n    - item";
const L2: usize = 9;

#[test]
fn clamps_carets_inside_the_prefix() {
    let s = EditorState::new(NESTED);
    let clamp = |pos| {
        clamp_collapsed_list_prefix_range(s.tree(), s.text(), SelectionRange::cursor(pos)).head
    };
    assert_eq!(clamp(L2 + 2), L2 + 4);
    assert_eq!(clamp(L2 + 5), L2 + 6);
    let range = SelectionRange::new(L2 + 2, L2 + 5);
    assert_eq!(
        clamp_collapsed_list_prefix_range(s.tree(), s.text(), range),
        range
    );
    // An indented code block that looks like a bullet.
    let code = EditorState::new("    - item");
    assert_eq!(
        clamp_collapsed_list_prefix_range(code.tree(), code.text(), SelectionRange::cursor(2)).head,
        2
    );
    // Wired as the editor's selection filter.
    let mut s = EditorState::new(NESTED);
    s.select(Selection::cursor(L2 + 2)).unwrap();
    assert_eq!(s.selection().main().head, L2 + 4);
}

#[test]
fn arrows_cross_only_the_allowed_prefix_boundaries() {
    let moved = |doc: &str, pos: usize, right: bool| {
        let mut s = state(doc, pos, pos);
        s.list_arrow(right, false)
            .then(|| s.selection().main().head)
    };
    assert_eq!(moved(NESTED, L2 + 6, false), Some(L2 + 4));
    assert_eq!(moved(NESTED, L2 + 4, false), Some(L2));
    assert_eq!(moved(NESTED, L2, true), Some(L2 + 4));
    assert_eq!(moved(NESTED, L2 + 4, true), Some(L2 + 6));
    assert_eq!(moved("- item", 0, false), None);
    assert_eq!(moved("- item", 0, true), Some(2));
    // Shift extends.
    let mut s = state(NESTED, L2 + 6, L2 + 6);
    assert!(s.list_arrow(false, true));
    assert_eq!(s.selection().main(), SelectionRange::new(L2 + 6, L2 + 4));
}

#[test]
fn list_item_lines_read_leading_and_content_whitespace() {
    let entry = |doc: &str, n: usize| {
        let s = EditorState::new(doc);
        list_item_line_at(s.tree(), s.text(), s.text().line(n - 1))
            .map(|e| (e.mark_from, e.leading_ws, e.content_ws))
    };
    assert_eq!(
        entry("- a\n  - b", 2),
        Some((6, "  ".into(), "    ".into()))
    );
    assert_eq!(entry("1. a", 1).map(|e| e.2), Some("   ".into()));
    assert_eq!(entry("10. a", 1).map(|e| e.2), Some("    ".into()));
    assert_eq!(
        entry("- a\n\t- b", 2).map(|e| (e.1, e.2)),
        Some(("\t".into(), "\t  ".into()))
    );
    assert_eq!(
        entry("-\tb", 1).map(|e| (e.1, e.2)),
        Some(("".into(), " \t".into()))
    );
    assert_eq!(entry("-   a", 1).map(|e| e.2), Some("    ".into()));
    assert_eq!(entry("- [ ] a", 1).map(|e| e.2), Some("  ".into()));
    assert_eq!(entry("para", 1), None);
    assert_eq!(entry("- a\n\n- b", 2), None);
    assert_eq!(entry("- a\n  cont", 2), None);
}

#[test]
fn enter_continues_items_with_the_same_marker() {
    let enter = EditorState::list_enter;
    assert_eq!(run(enter, "- foo|").as_deref(), Some("- foo\n- |"));
    assert_eq!(text_after(enter, "+ foo|"), "+ foo\n+ ");
    assert_eq!(text_after(enter, "* foo|"), "* foo\n* ");
    // A new task is always unchecked.
    assert_eq!(text_after(enter, "- [x] done|"), "- [x] done\n- [ ] ");
    assert_eq!(text_after(enter, "- a\n  - b|"), "- a\n  - b\n  - ");
    assert_eq!(
        run(enter, "- a\n  - b\n    - c\n      - d|").as_deref(),
        Some("- a\n  - b\n    - c\n      - d\n      - |")
    );
    assert_eq!(
        run(enter, "- a\n  - b\n    - c|d").as_deref(),
        Some("- a\n  - b\n    - c\n    - |d")
    );
}

#[test]
fn enter_on_an_empty_item_ends_or_steps_out() {
    let enter = EditorState::list_enter;
    assert_eq!(run(enter, "- |").as_deref(), Some("|"));
    assert_eq!(run(enter, "- [ ] |").as_deref(), Some("|"));
    assert_eq!(
        run(enter, "# S\n\n1. Only step\n2. |").as_deref(),
        Some("# S\n\n1. Only step\n|")
    );
    assert_eq!(
        run(enter, "1. One\n2. Two\n3. |").as_deref(),
        Some("1. One\n2. Two\n|")
    );
    assert_eq!(
        run(enter, "1. a\n2. b\n   1. x\n   2. |").as_deref(),
        Some("1. a\n2. b\n   1. x\n3. |")
    );
    assert_eq!(
        run(enter, "- a\n- b\n- |\n## Next").as_deref(),
        Some("- a\n- b\n|\n## Next")
    );
    assert_eq!(
        run(enter, "- a\n- |\n\n## Next").as_deref(),
        Some("- a\n|\n\n## Next")
    );
    assert_eq!(
        run(enter, "- a\n- [ ] |\n- b").as_deref(),
        Some("- a\n|\n- b")
    );
    assert_eq!(run(enter, "- a\n\n- |").as_deref(), Some("- a\n\n|"));
    assert_eq!(run(enter, "- a\n  - |").as_deref(), Some("- a\n- |"));
    assert_eq!(
        run(enter, "- a\n  - b\n    - |").as_deref(),
        Some("- a\n  - b\n  - |")
    );
    assert_eq!(
        run(enter, "- a\n  - b\n    - [ ] |").as_deref(),
        Some("- a\n  - b\n  - [ ] |")
    );
    assert_eq!(run(enter, "- a\n\n  - |").as_deref(), Some("- a\n\n- |"));
    assert_eq!(
        run(enter, "- a\n\t- b\n\t  - |").as_deref(),
        Some("- a\n\t- b\n\t- |")
    );
    // Text typed after ending the list is not a list line.
    let mut s = marked("- a\n- |");
    assert!(s.list_enter());
    s.apply(Transaction::new().change(Change::insert(s.doc().len(), "What")))
        .unwrap();
    assert!(!is_on_list_line(s.tree(), s.text(), s.doc().len()));
    let last = s.text().line_at(s.doc().len());
    assert!(list_item_line_at(s.tree(), s.text(), last).is_none());
}

#[test]
fn enter_defers_outside_its_cases() {
    let enter = EditorState::list_enter;
    assert_eq!(
        run(enter, "1. One|"),
        None,
        "ordered continuation is the markup fallback's"
    );
    assert_eq!(run(enter, "|- foo"), None);
    assert_eq!(run(enter, "hello|"), None);
    assert_eq!(run(enter, "-| f|oo"), None);
    // Nothing happens in a fence.
    assert_eq!(run(enter, "```\n- |\n```"), None);
}

#[test]
fn enter_on_an_indent_only_line_after_a_blank_clears_it() {
    let leave = EditorState::leave_indented_blank;
    assert_eq!(
        run(leave, "- a\n\n  para\n\n  |").as_deref(),
        Some("- a\n\n  para\n\n|")
    );
    assert_eq!(
        run(leave, "x\n\n    code\n\n    |").as_deref(),
        Some("x\n\n    code\n\n|")
    );
    assert_eq!(run(leave, "- a\n  para\n  |"), None);
    assert_eq!(run(leave, "```\nfn\n\n    |\n```"), None);
}

#[test]
fn backspace_at_the_body_removes_the_marker() {
    let back = EditorState::list_backspace;
    assert_eq!(run(back, "  - |foo").as_deref(), Some("|foo"));
    assert_eq!(run(back, "- |foo").as_deref(), Some("|foo"));
    assert_eq!(run(back, "- a\n- |foo").as_deref(), Some("- a\n|foo"));
    assert_eq!(run(back, "- a\n- |").as_deref(), Some("- a\n|"));
    assert_eq!(run(back, "- a\n- [ ] |").as_deref(), Some("- a\n|"));
    assert_eq!(run(back, "- a\n\n- |foo").as_deref(), Some("- a\n\n|foo"));
    assert_eq!(run(back, "- a\n  - |b").as_deref(), Some("- a\n|b"));
    assert_eq!(
        run(back, "- a\n  - b\n    - |c").as_deref(),
        Some("- a\n  - b\n  |c")
    );
    assert_eq!(run(back, "  - [ ] |task").as_deref(), Some("|task"));
    assert_eq!(
        run(back, "- a\n\t- b\n\t\t- |c").as_deref(),
        Some("- a\n\t- b\n\t|c")
    );
    assert_eq!(
        run(back, "- a\n  - b\n    - |c").as_deref(),
        Some("- a\n  - b\n  |c")
    );
}

#[test]
fn backspace_at_the_marker_removes_one_level() {
    let back = EditorState::list_backspace;
    assert_eq!(
        run(back, "- a\n  - b\n    |- c").as_deref(),
        Some("- a\n  - b\n  |- c")
    );
    assert_eq!(run(back, "|- foo"), None);
    assert_eq!(
        run(back, "- a\n\t- b\n\t\t|- c").as_deref(),
        Some("- a\n\t- b\n\t|- c")
    );
    assert_eq!(run(back, "1. a\n   |- b").as_deref(), Some("1. a\n|- b"));
    assert_eq!(run(back, "|  - foo"), None);
    assert_eq!(run(back, "- fo|o"), None);
    assert_eq!(run(back, "hello|"), None);
}

#[test]
fn backspace_on_an_empty_deep_item_leaves_no_line_of_spaces() {
    // `TODOS.md:95`: Writer turned `    - ` into `  `.
    let back = EditorState::list_backspace;
    assert_eq!(
        run(back, "- a\n  - b\n    - |").as_deref(),
        Some("- a\n  - b\n  - |")
    );
    assert_eq!(
        run(back, "- a\n  - b\n    - [ ] |").as_deref(),
        Some("- a\n  - b\n  - [ ] |")
    );
    // One level deep there is no whitespace to leave.
    assert_eq!(run(back, "- a\n  - |").as_deref(), Some("- a\n|"));
}

#[test]
fn tab_nests_under_the_previous_item() {
    let tab = EditorState::list_indent;
    assert_eq!(run(tab, "- a\n- b|").as_deref(), Some("- a\n  - b|"));
    assert_eq!(text_after(tab, "- a\n- [ ] b|"), "- a\n  - [ ] b");
    assert_eq!(text_after(tab, "- a\n  - b\n  - c|"), "- a\n  - b\n    - c");
    // No parent: consumed, unchanged.
    assert_eq!(run(tab, "- a|").as_deref(), Some("- a|"));
    assert_eq!(run(tab, "- a\n  - b|").as_deref(), Some("- a\n  - b|"));
    assert_eq!(run(tab, "hello|"), None);
    assert_eq!(text_after(tab, "- a\n|- b\n- c|"), "- a\n  - b\n  - c");
    assert_eq!(text_after(tab, "|- a\npara\n- b|"), "- a\npara\n  - b");
    assert_eq!(run(tab, "* a\n- b|").as_deref(), Some("* a\n  - b|"));
    assert_eq!(run(tab, "- a\n+ b|").as_deref(), Some("- a\n  + b|"));
    assert_eq!(run(tab, "1. a\n- b|").as_deref(), Some("1. a\n   - b|"));
    assert_eq!(run(tab, "- a\n1. b|").as_deref(), Some("- a\n  1. b|"));
    assert_eq!(run(tab, "1. a\n2. b|").as_deref(), Some("1. a\n   1. b|"));
    assert_eq!(
        run(tab, "1. a\n2. b\n3. |").as_deref(),
        Some("1. a\n2. b\n   1. |")
    );
    assert_eq!(
        run(tab, "1. a\n   1. x\n2. |").as_deref(),
        Some("1. a\n   1. x\n   2. |")
    );
    assert_eq!(
        run(tab, "- a\n\t- b\n\t- c|").as_deref(),
        Some("- a\n\t- b\n\t  - c|")
    );
    assert_eq!(run(tab, "- a\n\t- b|").as_deref(), Some("- a\n\t- b|"));
    assert_eq!(
        run(tab, "- a\n  - b\n   - c\n  - d|").as_deref(),
        Some("- a\n  - b\n   - c\n     - d|")
    );
    assert_eq!(run(tab, "- a\n\n- b|").as_deref(), Some("- a\n\n  - b|"));
    assert_eq!(
        run(tab, "- a\n\n  - b\n\n  - c|").as_deref(),
        Some("- a\n\n  - b\n\n    - c|")
    );
    assert_eq!(
        run(tab, "- a\n\npara\n\n- b|").as_deref(),
        Some("- a\n\npara\n\n- b|")
    );
    assert_eq!(run(tab, "- a\n|- b").as_deref(), Some("- a\n|  - b"));
    assert_eq!(run(tab, "- a\n- |b").as_deref(), Some("- a\n  - |b"));
}

#[test]
fn tab_moves_subtrees_and_wrapped_lines() {
    let tab = EditorState::list_indent;
    let mut s = marked("- x\n|- a\n  - b\n    - c|");
    assert!(s.list_indent());
    assert_eq!(s.doc(), "- x\n  - a\n    - b\n      - c");
    assert_eq!(s.selection().main().range(), 4..27);
    assert_eq!(
        text_after(tab, "- a\n|- b\n  - c\n- d|"),
        "- a\n  - b\n    - c\n  - d"
    );
    assert_eq!(run(tab, "|- a\n  - b\n    - c|").map(|_| ()), Some(()));
    assert_eq!(
        text_after(tab, "|- a\n  - b\n    - c|"),
        "- a\n  - b\n    - c"
    );
    assert_eq!(
        text_after(tab, "- a\n  |- b\n  - c|"),
        "- a\n  - b\n    - c"
    );
    assert_eq!(
        run(tab, "- a\n- b one|\n  b two\n  b three").as_deref(),
        Some("- a\n  - b one|\n    b two\n    b three")
    );
    assert_eq!(
        text_after(tab, "- a\n- [ ] b|\n  b two\n\n  second\n  para"),
        "- a\n  - [ ] b\n    b two\n\n    second\n    para"
    );
    assert_eq!(
        text_after(tab, "- a\n- b one|\nb two"),
        "- a\n  - b one\nb two"
    );
    assert_eq!(
        text_after(tab, "- x\n|- a\n  a two\n  - b\n    b two|"),
        "- x\n  - a\n    a two\n    - b\n      b two"
    );
}

#[test]
fn undo_after_a_selection_tab_restores_text_and_selection() {
    let before = marked("- x\n|- a\n  - b\n    - c|");
    let mut s = before.clone();
    assert!(s.list_indent());
    assert!(s.undo());
    assert_eq!(s.doc(), before.doc());
    assert_eq!(s.selection(), before.selection());
}

#[test]
fn tab_works_on_freshly_pasted_lines() {
    let mut s = state("- a\n", 4, 4);
    s.apply(
        Transaction::new()
            .change(Change::insert(4, "- p\n  - q"))
            .select(Selection::cursor(4)),
    )
    .unwrap();
    s.select(Selection::single(4, 13)).unwrap();
    assert!(s.list_indent());
    assert_eq!(s.doc(), "- a\n  - p\n    - q");
}

#[test]
fn shift_tab_lifts_to_the_parent_level() {
    let untab = EditorState::list_outdent;
    assert_eq!(
        run(untab, "1. a\n2. b\n   1. x\n   2. |").as_deref(),
        Some("1. a\n2. b\n   1. x\n3. |")
    );
    assert_eq!(text_after(untab, "- a\n  - b|"), "- a\n- b");
    assert_eq!(
        text_after(untab, "- a\n  - b\n    - c|"),
        "- a\n  - b\n  - c"
    );
    assert_eq!(run(untab, "- a|").as_deref(), Some("- a|"));
    assert_eq!(text_after(untab, "- a\n|  - b\n  - c|"), "- a\n- b\n- c");
    assert_eq!(run(untab, "1. a\n   - b|").as_deref(), Some("1. a\n- b|"));
    assert_eq!(
        run(untab, "- a\n\t- b\n\t  - c|").as_deref(),
        Some("- a\n\t- b\n\t- c|")
    );
    assert_eq!(run(untab, "- a\n\t- b|").as_deref(), Some("- a\n- b|"));
    assert_eq!(
        run(untab, "- a\n  - b\n   - c|\n  - d").as_deref(),
        Some("- a\n  - b\n- c|\n  - d")
    );
    assert_eq!(run(untab, "- a\n\n  - b|").as_deref(), Some("- a\n\n- b|"));
    assert_eq!(
        text_after(untab, "- x\n|  - a\n    - b\n      - c|"),
        "- x\n- a\n  - b\n    - c"
    );
    assert_eq!(
        text_after(untab, "- x\n|\t- a\n\t  - b|"),
        "- x\n- a\n  - b"
    );
    assert_eq!(run(untab, "- a\n|  - b").as_deref(), Some("- a\n|- b"));
    assert_eq!(
        run(untab, "- a\n  - b one|\n    b two").as_deref(),
        Some("- a\n- b one|\n  b two")
    );
    assert_eq!(
        text_after(untab, "- a\n\t- b one|\n\t  b two"),
        "- a\n- b one\n  b two"
    );
    assert_eq!(
        text_after(untab, "- a\n  - b one|\n b two"),
        "- a\n- b one\n b two"
    );
}

#[test]
fn checkbox_toggles_on_click_release_only() {
    let text = |doc: &str| Text::new(doc);
    assert_eq!(
        checkbox_toggle(&text("- [ ] task"), 0),
        Some(Change::new(3..4, "x"))
    );
    assert_eq!(
        checkbox_toggle(&text("- [x] done"), 0),
        Some(Change::new(3..4, " "))
    );
    assert_eq!(
        checkbox_toggle(&text("  - [ ] nested"), 2),
        Some(Change::new(5..6, "x"))
    );
    assert_eq!(
        checkbox_toggle_from_line(&text("  - [ ] nested"), 0),
        Some(Change::new(5..6, "x"))
    );
    assert_eq!(checkbox_toggle_from_line(&text("  - nested"), 0), None);
    assert_eq!(checkbox_toggle(&text("- foo"), 0), None);
    // Through the editor: a drag across the box selects, a click toggles.
    let mut s = EditorState::new("- [ ] task");
    assert!(!s.checkbox_click(1, true));
    assert_eq!(s.doc(), "- [ ] task");
    assert!(s.checkbox_click(1, false));
    assert_eq!(s.doc(), "- [x] task");
    assert!(s.undo());
    assert_eq!(s.doc(), "- [ ] task");
}

fn prefixes(doc: &str) -> Vec<(Range<usize>, PrefixKind, usize, usize)> {
    EditorState::new(doc)
        .list_decorations()
        .all
        .into_iter()
        .filter_map(|d| match d {
            ListDecoration::Prefix {
                range,
                kind,
                width_ch,
                marker_offset_ch,
            } => Some((range, kind, width_ch, marker_offset_ch)),
            _ => None,
        })
        .collect()
}

#[test]
fn prefixes_render_as_source_backed_marks() {
    assert!(
        EditorState::new("-").list_decorations().marker.is_empty(),
        "a bare `-` is text"
    );
    assert_eq!(EditorState::new("- ").list_decorations().marker, [0..2]);
    // Three markers plus one and two indent steps.
    assert_eq!(
        EditorState::new("- a\n  - b\n    - c")
            .list_decorations()
            .atomic
            .len(),
        6
    );
    assert_eq!(prefixes("- "), [(0..2, PrefixKind::Bullet, 3, 0)]);
    assert_eq!(
        prefixes("- [ ] "),
        [(0..6, PrefixKind::Task { checked: false }, 3, 0)]
    );
    assert_eq!(prefixes("- body"), [(0..2, PrefixKind::Bullet, 3, 0)]);
    assert_eq!(
        prefixes("- [ ] body"),
        [(0..6, PrefixKind::Task { checked: false }, 3, 0)]
    );
    assert_eq!(
        prefixes("- a\n  - [x] nested")[1],
        (4..12, PrefixKind::Task { checked: true }, 6, 3)
    );
    // `- - w`: two one-step prefixes side by side, one hanging indent.
    assert_eq!(
        prefixes("- - w"),
        [
            (0..2, PrefixKind::Bullet, 3, 0),
            (2..4, PrefixKind::Bullet, 3, 0)
        ]
    );
    let hanging: Vec<_> = EditorState::new("- - w")
        .list_decorations()
        .all
        .into_iter()
        .filter_map(|d| match d {
            ListDecoration::Line {
                padding_ch,
                hanging_ch,
                ..
            } if hanging_ch > 0.0 => Some((padding_ch, hanging_ch)),
            _ => None,
        })
        .collect();
    assert_eq!(hanging, [(6.0, 6.0)]);
    // Nesting geometry after Tab.
    let mut s = marked("1. a\n- b|");
    assert!(s.list_indent());
    assert_eq!(
        prefixes(s.doc())
            .iter()
            .map(|p| (p.2, p.3))
            .collect::<Vec<_>>(),
        [(6, 3)]
    );
    let mut s = marked("- a\n\t- b\n\t- c|");
    assert!(s.list_indent());
    assert_eq!(
        prefixes(s.doc())
            .iter()
            .map(|p| (p.2, p.3))
            .collect::<Vec<_>>(),
        [(3, 0), (6, 3), (9, 6)]
    );
}

/// Continuation lines (padding without hanging) and collapsed indents.
fn continuation(doc: &str) -> (Vec<(usize, f32)>, Vec<Range<usize>>) {
    let s = EditorState::new(doc);
    let mut lines = Vec::new();
    let mut hidden = Vec::new();
    for d in s.list_decorations().all {
        match d {
            ListDecoration::Line {
                line_start,
                padding_ch,
                hanging_ch,
            } if hanging_ch == 0.0 => {
                // One-based, as in Writer's tests.
                lines.push((s.text().line_at(line_start).number + 1, padding_ch));
            }
            ListDecoration::CollapsedIndent { range } => hidden.push(range),
            _ => {}
        }
    }
    (lines, hidden)
}

#[test]
fn continuation_lines_sit_at_the_body_column() {
    assert_eq!(
        continuation("- one two\n  three four\nfive"),
        (vec![(2, 3.0)], vec![10..12])
    );
    let atomic = EditorState::new("- one two\n  three four\nfive")
        .list_decorations()
        .atomic;
    assert!(atomic.contains(&(10..12)));
    assert_eq!(
        continuation("- a\n  - [ ] b\n    c\n1. d\n   e"),
        (vec![(3, 6.0), (5, 3.0)], vec![14..18, 25..28])
    );
    assert!(continuation("- a\n  - b\n\npara").0.is_empty());
    assert!(continuation("- a\nlazy").0.is_empty());
    assert!(continuation("- a\n  - b\nlazy").0.is_empty());
    assert!(continuation("- a\n").0.is_empty());
    assert_eq!(
        continuation("- a\n\n  b one\n  b two\n- c"),
        (vec![(3, 3.0), (4, 3.0)], vec![5..7, 13..15])
    );
    assert_eq!(
        continuation("- a\n  - b\n\n  para"),
        (vec![(4, 3.0)], vec![11..13])
    );
}

#[test]
fn nested_ordered_items_hide_their_source_indent() {
    // `TODOS.md:96`: the leading spaces of `   1. x` are drawn as one
    // indent level and the line hangs at depth 1, like a nested bullet.
    let d = EditorState::new("1. a\n   1. x").list_decorations();
    assert!(d.all.contains(&ListDecoration::IndentVisual {
        range: 5..8,
        width_ch: 3
    }));
    assert!(d.atomic.contains(&(5..8)));
    assert!(d.all.contains(&ListDecoration::Line {
        line_start: 5,
        padding_ch: 6.0,
        hanging_ch: 6.4
    }));
    assert!(d.all.contains(&ListDecoration::Line {
        line_start: 0,
        padding_ch: 3.0,
        hanging_ch: 3.4
    }));
}

#[test]
fn enter_on_the_3000_item_list_stays_under_a_frame() {
    let mut doc = String::from("# List\n\n");
    for i in 0..3000 {
        doc.push_str(&format!("- item {i}\n"));
        if i == 1500 {
            doc.push_str("  - sub item!!\n");
        }
    }
    let pos = doc.find("sub item!!").unwrap() + "sub item!!".len();
    let mut s = state(&doc, pos, pos);
    let start = std::time::Instant::now();
    assert!(s.list_enter());
    let elapsed = start.elapsed();
    assert!(s.doc()[pos..].starts_with("\n  - \n"));
    // Release builds measure the budget (`examples/tree_bench.rs`); a debug
    // build only guards against a pathological regression.
    let budget = if cfg!(debug_assertions) { 200 } else { 16 };
    assert!(elapsed.as_millis() < budget, "{elapsed:?}");
}
