//! Ported from Writer `tests/editor-formatting.test.ts:59-486` and
//! `tests/editor-commands.test.ts`.

use super::*;

const CLOCK: Clock = Clock {
    year: 2026,
    month: 11,
    day: 1,
    hour: 9,
    minute: 5,
};

fn run(id: &str, doc: &str, anchor: usize, head: usize) -> EditorState {
    let mut state = EditorState::with_filters(doc, Vec::new());
    state.select(Selection::single(anchor, head)).unwrap();
    assert!(
        state.run_command(id, &CLOCK) || id == "format.link" || id == "format.clearFormatting",
        "{id} declined"
    );
    state
}

fn doc(id: &str, text: &str, anchor: usize) -> String {
    run(id, text, anchor, anchor).doc().to_owned()
}

fn sel(state: &EditorState) -> (usize, usize) {
    let main = state.selection().main();
    (main.from(), main.to())
}

#[test]
fn inline_toggles_wrap_words_selections_or_insert_markers() {
    for (id, marker) in [
        ("format.bold", "**"),
        ("format.italic", "*"),
        ("format.code", "`"),
        ("format.strikethrough", "~~"),
    ] {
        let len = marker.len();
        assert_eq!(
            doc(id, "hello world", 2),
            format!("{marker}hello{marker} world"),
            "{id}"
        );
        let s = run(id, "hello world", 0, 5);
        assert_eq!(s.doc(), format!("{marker}hello{marker} world"));
        assert_eq!(
            sel(&s),
            (len, len + 5),
            "{id}: the selection stays on the text"
        );
        let s = run(id, "hello ", 6, 6);
        assert_eq!(s.doc(), format!("hello {marker}{marker}"));
        assert_eq!(s.selection().main().head, 6 + len);
        let wrapped = format!("{marker}hello{marker} world");
        assert_eq!(doc(id, &wrapped, len + 2), "hello world", "{id} unwraps");
    }
}

#[test]
fn insert_link_selects_the_url_slot() {
    let s = run("format.link", "hello ", 6, 6);
    assert_eq!(s.doc(), "hello [](url)");
    assert_eq!(sel(&s), (9, 12));
    let s = run("format.link", "hello world", 0, 5);
    assert_eq!(s.doc(), "[hello](url) world");
    assert_eq!(sel(&s), (8, 11));
    // Inside a link: nothing to do.
    let mut s = EditorState::new("[a](b)");
    s.select(Selection::cursor(2)).unwrap();
    assert!(!s.run_command("format.link", &CLOCK));
}

#[test]
fn headings_and_paragraph() {
    assert_eq!(doc("format.heading2", "hello", 0), "## hello");
    assert_eq!(doc("format.heading3", "## hello", 3), "### hello");
    assert_eq!(doc("format.heading1", "hello", 0), "# hello");
    assert_eq!(doc("format.paragraph", "## hello", 3), "hello");
    assert_eq!(doc("format.paragraph", "hello", 2), "hello");
}

#[test]
fn line_commands_keep_the_caret_in_the_text() {
    let s = run("format.bulletList", "hello", 5, 5);
    assert_eq!((s.doc(), sel(&s)), ("- hello", (7, 7)));
    let s = run("format.taskList", "# T\n\nBuy milk", 13, 13);
    assert_eq!((s.doc(), sel(&s)), ("# T\n\n- [ ] Buy milk", (19, 19)));
    let s = run("format.blockquote", "> hello", 4, 4);
    assert_eq!((s.doc(), s.selection().main().head), ("hello", 2));
    let s = run("format.heading3", "## hello", 5, 5);
    assert_eq!((s.doc(), s.selection().main().head), ("### hello", 6));
    let s = run("format.numberedList", "one\ntwo", 1, 6);
    assert_eq!((s.doc(), sel(&s)), ("1. one\n2. two", (4, 12)));
}

#[test]
fn bullet_list_toggle() {
    let bullet = "format.bulletList";
    assert_eq!(doc(bullet, "hello", 0), "- hello");
    assert_eq!(run(bullet, "- hello\n- world", 0, 15).doc(), "hello\nworld");
    assert_eq!(run(bullet, "hello\nworld", 0, 11).doc(), "- hello\n- world");
    assert_eq!(doc(bullet, "- a\n  - b", 8), "- a\n  b");
    assert_eq!(doc(bullet, "- a\n  text", 8), "- a\n  - text");
    let s = run(bullet, "- [ ] Call the plumber", 22, 22);
    assert_eq!(
        (s.doc(), s.selection().main().head),
        ("- Call the plumber", 18)
    );
    assert_eq!(doc(bullet, "- a\n  - [x] b", 13), "- a\n  - b");
    assert_eq!(run(bullet, "- a\n  - b", 0, 9).doc(), "a\n  b");
}

#[test]
fn numbered_quote_and_task_toggles() {
    assert_eq!(
        run("format.numberedList", "hello\nworld", 0, 11).doc(),
        "1. hello\n2. world"
    );
    assert_eq!(
        run("format.numberedList", "1. hello\n2. world", 0, 17).doc(),
        "hello\nworld"
    );
    assert_eq!(doc("format.blockquote", "hello", 0), "> hello");
    assert_eq!(
        run("format.blockquote", "> hello\n> world", 0, 15).doc(),
        "hello\nworld"
    );
    let task = "format.taskList";
    assert_eq!(doc(task, "hello", 0), "- [ ] hello");
    assert_eq!(
        run(task, "- [ ] hello\n- [ ] world", 0, 23).doc(),
        "hello\nworld"
    );
    assert_eq!(
        run(task, "- [x] hello\n- [x] world", 0, 23).doc(),
        "hello\nworld"
    );
    assert_eq!(doc(task, "- hello", 0), "- [ ] hello");
    assert_eq!(doc(task, "- a\n  - b", 8), "- a\n  - [ ] b");
    assert_eq!(doc(task, "- a\n  text", 8), "- a\n  - [ ] text");
    assert_eq!(doc(task, "- a\n  - [ ] b", 10), "- a\n  b");
}

#[test]
fn clear_formatting_strips_inline_markers() {
    let clear = "format.clearFormatting";
    assert_eq!(run(clear, "**bold**", 0, 8).doc(), "bold");
    assert_eq!(run(clear, "*italic*", 0, 8).doc(), "italic");
    assert_eq!(run(clear, "~~struck~~", 0, 10).doc(), "struck");
    assert_eq!(run(clear, "`code`", 0, 6).doc(), "code");
    assert_eq!(
        run(clear, "**bold *and italic***", 0, 21).doc(),
        "bold and italic"
    );
    let mut s = EditorState::new("hello");
    s.select(Selection::cursor(2)).unwrap();
    assert!(!s.run_command(clear, &CLOCK));
    assert_eq!(s.doc(), "hello");
}

#[test]
fn fenced_code_block_toggle() {
    let fence = "format.codeBlock";
    assert_eq!(
        run(fence, "hello\nworld", 0, 11).doc(),
        "```\nhello\nworld\n```"
    );
    assert_eq!(
        run(fence, "```\nhello\nworld\n```", 0, 19).doc(),
        "hello\nworld"
    );
    assert_eq!(run(fence, "hello", 0, 5).doc(), "```\nhello\n```");
}

#[test]
fn inserts_table_rule_date_and_time() {
    let s = run("insert.table", "", 0, 0);
    assert!(s.doc().contains("| Column 1 | Column 2 | Column 3 |"));
    assert!(s.doc().contains("| --- | --- | --- |"));
    assert!(s.doc().contains("|  |  |  |"));
    assert_eq!(
        &s.doc()[s.selection().main().head - 2..s.selection().main().head],
        "| "
    );
    assert_eq!(doc("insert.horizontalRule", "", 0), "---\n");
    assert_eq!(doc("insert.horizontalRule", "hello", 5), "hello\n---\n");
    assert_eq!(doc("insert.today", "", 0), "2026-11-01");
    assert_eq!(doc("insert.now", "", 0), "09:05");
}

#[test]
fn a_loading_document_runs_no_command() {
    let mut s = EditorState::loading();
    for command in COMMANDS {
        assert!(!s.run_command(command.id, &CLOCK), "{}", command.id);
    }
    assert_eq!(s.doc(), "");
    assert!(!EditorState::new("x").run_command("format.nope", &CLOCK));
}

#[test]
fn the_registry_has_stable_unique_ids_labels_and_chords() {
    assert_eq!(COMMANDS.len(), 22);
    let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), COMMANDS.len());
    for command in COMMANDS {
        assert!(!command.label.is_empty(), "{}", command.id);
        assert!(std::ptr::eq(super::command(command.id).unwrap(), command));
    }
    for os in [Os::MacOs, Os::Windows, Os::Linux] {
        let chords: Vec<Chord> = COMMANDS.iter().filter_map(|c| c.shortcut(os)).collect();
        let mut unique = chords.clone();
        unique.dedup();
        for (i, a) in chords.iter().enumerate() {
            assert!(
                !chords[i + 1..].contains(a),
                "{os:?}: {} bound twice",
                a.label()
            );
            assert_eq!(command_for(a, os).and_then(|c| c.shortcut(os)), Some(*a));
        }
    }
}

#[test]
fn mac_shortcuts_match_writers_documentation() {
    // `docs/keyboard-shortcuts.md:50-70`.
    let documented = [
        ("format.bold", "Cmd+B"),
        ("format.italic", "Cmd+I"),
        ("format.link", "Cmd+K"),
        ("format.code", "Cmd+E"),
        ("format.strikethrough", "Cmd+Shift+X"),
        ("format.bulletList", "Cmd+Shift+8"),
        ("format.numberedList", "Cmd+Shift+7"),
        ("format.blockquote", "Cmd+Shift+."),
        ("format.taskList", "Cmd+Shift+Enter"),
        ("format.heading1", "Cmd+Alt+1"),
        ("format.heading6", "Cmd+Alt+6"),
        ("format.paragraph", "Cmd+Alt+0"),
    ];
    for (id, label) in documented {
        assert_eq!(
            command(id)
                .and_then(|c| c.shortcut(Os::MacOs))
                .map(|c| c.label())
                .as_deref(),
            Some(label),
            "{id}"
        );
    }
}

#[test]
fn windows_and_linux_use_ctrl_and_never_ctrl_alt() {
    for os in [Os::Windows, Os::Linux] {
        for command in COMMANDS {
            let Some(chord) = command.shortcut(os) else {
                continue;
            };
            assert!(!chord.cmd, "{}: no Cmd key on {os:?}", command.id);
            assert!(
                !(chord.ctrl && chord.alt),
                "{}: Ctrl+Alt is AltGr on {os:?}",
                command.id
            );
            assert_eq!(
                chord.ctrl,
                command.shortcut(Os::MacOs).is_some_and(|m| m.cmd),
                "{}",
                command.id
            );
        }
        assert_eq!(
            command("format.bold")
                .and_then(|c| c.shortcut(os))
                .map(|c| c.label())
                .as_deref(),
            Some("Ctrl+B")
        );
        assert_eq!(
            command("format.heading1")
                .and_then(|c| c.shortcut(os))
                .map(|c| c.label())
                .as_deref(),
            Some("Ctrl+Shift+1")
        );
    }
}

#[test]
fn every_shortcut_is_typeable_on_azerty_and_qwerty() {
    for layout in [&QWERTY_US, &AZERTY_FR] {
        for os in [Os::MacOs, Os::Windows, Os::Linux] {
            for command in COMMANDS {
                if let Some(chord) = command.shortcut(os) {
                    assert!(
                        layout.can_type(&chord),
                        "{} ({}) on {} {os:?}",
                        command.id,
                        chord.label(),
                        layout.name
                    );
                }
            }
        }
    }
    // A key missing from the layout is reported.
    let missing = Chord {
        cmd: false,
        ctrl: true,
        alt: false,
        shift: false,
        key: "F13",
    };
    assert!(!AZERTY_FR.can_type(&missing) && !QWERTY_US.can_type(&missing));
}

#[test]
fn two_carets_in_one_node_or_on_one_line_edit_it_once() {
    let carets = |a: usize, b: usize| {
        Selection::new(
            vec![SelectionRange::cursor(a), SelectionRange::cursor(b)],
            0,
        )
    };
    let mut state = EditorState::with_filters("**ab**", Vec::new());
    state.select(carets(3, 4)).unwrap();
    assert!(state.run_command("format.bold", &CLOCK));
    assert_eq!(state.doc(), "ab");
    assert_eq!(
        state.selection().ranges(),
        &[SelectionRange::cursor(1), SelectionRange::cursor(2)]
    );
    let mut state = EditorState::with_filters("text", Vec::new());
    state.select(carets(1, 3)).unwrap();
    assert!(state.run_command("format.heading1", &CLOCK));
    assert_eq!(state.doc(), "# text");
    assert_eq!(
        state.selection().ranges(),
        &[SelectionRange::cursor(3), SelectionRange::cursor(5)]
    );
}
