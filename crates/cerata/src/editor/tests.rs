//! GPUI interaction tests of the editor, on the test platform: real
//! keystrokes through the key bindings, input through the input handler,
//! pointer events through the element's listeners. The test text system is
//! monospace and deterministic.

use std::path::PathBuf;

use cerata_markdown::{Selection, Transaction};
use gpui::{
    ClipboardEntry, ClipboardItem, Entity, EntityInputHandler, Image, ImageFormat, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollEvent, TestAppContext, VisualTestContext, point, px, size,
};

use super::config::EditorConfig;
use super::{Editor, SAFE_MARGIN, actions};

fn open_at<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    path: Option<PathBuf>,
    config: EditorConfig,
) -> (Entity<Editor>, &'a mut VisualTestContext) {
    cx.update(actions::init);
    let text = text.to_owned();
    let (editor, cx) = cx.add_window_view(move |window, cx| {
        let editor = Editor::new(text, path, config, cx);
        window.focus(&editor.focus_handle, cx);
        editor
    });
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    cx.run_until_parked();
    (editor, cx)
}

fn open<'a>(cx: &'a mut TestAppContext, text: &str) -> (Entity<Editor>, &'a mut VisualTestContext) {
    open_at(cx, text, None, EditorConfig::default())
}

/// The OS's command modifier, as the bindings spell it.
fn secondary(keys: &str) -> String {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    format!("{modifier}-{keys}")
}

fn set_cursor(editor: &Entity<Editor>, cx: &mut VisualTestContext, pos: usize) {
    editor.update(cx, |editor, cx| editor.select(Selection::cursor(pos), cx));
    cx.run_until_parked();
}

fn doc(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> String {
    editor.read_with(cx, |editor, _| editor.state.doc().to_owned())
}

fn selection(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> Vec<(usize, usize)> {
    editor.read_with(cx, |editor, _| {
        editor
            .state
            .selection()
            .ranges()
            .iter()
            .map(|r| (r.anchor, r.head))
            .collect()
    })
}

/// The window point of the caret at `pos`, a pixel inside it.
fn point_at(editor: &Entity<Editor>, cx: &mut VisualTestContext, pos: usize) -> Point<Pixels> {
    editor.update_in(cx, |editor, window, cx| {
        let caret = editor.caret_bounds(pos, window, cx).expect("caret");
        point(
            caret.origin.x + px(0.5),
            caret.origin.y + caret.size.height / 2.0,
        )
    })
}

fn press(cx: &mut VisualTestContext, position: Point<Pixels>, clicks: usize, shift: bool) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
        click_count: clicks,
        first_mouse: false,
    });
}

fn release(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
}

fn drag_to(cx: &mut VisualTestContext, position: Point<Pixels>, pressed: bool) {
    cx.simulate_event(MouseMoveEvent {
        position,
        pressed_button: pressed.then_some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
}

/// A deterministic note of about `bytes`, with headings, emphasis, links,
/// lists, code and quotes.
fn long_note(bytes: usize) -> String {
    let mut out = String::new();
    let mut i = 0;
    while out.len() < bytes {
        let level = i % 6 + 1;
        out.push_str(&format!("{} Heading {i}\n\n", "#".repeat(level)));
        out.push_str(&format!(
            "Paragraph {i} with **bold** text, a [link](https://example.com/{i}) and `code`, \
             long enough to wrap across the text column of the editor at the default width.\n\n"
        ));
        out.push_str(&format!("- item {i}\n- [ ] task {i}\n\n> quote {i}\n\n"));
        if i % 7 == 0 {
            out.push_str("```rust\nfn main() { println!(\"hello\"); }\n```\n\n");
        }
        i += 1;
    }
    out
}

// US-016 ---------------------------------------------------------------

#[gpui::test]
fn each_heading_level_is_laid_out_at_its_own_font_size(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "# a\n## b\n### c\n#### d\n##### e\n###### f\nbody\n");
    set_cursor(&editor, cx, 42);
    editor.update_in(cx, |editor, window, cx| {
        let scales = cerata_markdown::decorate::HEADING_SCALES;
        for level in 1..=6 {
            let line = editor.layout_line(level - 1, window, cx);
            let wrapped = line.wrapped.as_ref().expect("text line");
            assert_eq!(
                wrapped.font_size(),
                px(16.0 * scales[level - 1]),
                "H{level}"
            );
            assert_eq!(line.display.text, ["a", "b", "c", "d", "e", "f"][level - 1]);
            let height = line.metrics.pad_top + line.metrics.row_height + line.metrics.pad_bottom;
            assert_eq!(line.height, height);
            assert_eq!(
                editor.heights.get(level - 1),
                height as f64,
                "exact height in the map"
            );
            let next = editor.line_screen_top(level) - editor.line_screen_top(level - 1);
            assert!((next.as_f32() - height).abs() < 1e-3);
        }
        assert_eq!(
            editor.heights.get(6),
            30.0,
            "body: 16 px at line height 1.875"
        );
    });
}

#[gpui::test]
fn the_column_is_734_px_centered_and_follows_size_zoom_and_width(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "text\n");
    editor.read_with(cx, |editor, _| {
        let viewport = editor.viewport.expect("painted");
        assert_eq!(viewport.column_width, 734.0);
        let left = viewport.column_left - viewport.bounds.left();
        let right = viewport.bounds.right() - (viewport.column_left + px(734.0));
        assert!(
            (left - right).abs() <= px(1.0),
            "centered: {left:?} vs {right:?}"
        );
    });

    let config = EditorConfig {
        zoom: 1.5,
        content_width: 900.0,
        ..EditorConfig::default()
    };
    let (editor, cx) = open_at(cx, "text\n", None, config);
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.viewport.expect("painted").column_width, 900.0);
        let line = editor.layout_line(0, window, cx);
        assert_eq!(line.wrapped.as_ref().expect("text").font_size(), px(24.0));
        assert_eq!(line.height, 45.0, "1.875 x 24");
    });
}

#[gpui::test]
fn only_visible_lines_and_a_margin_are_laid_out(cx: &mut TestAppContext) {
    let note = long_note(256 * 1024);
    let (editor, cx) = open(cx, &note);
    let (lines, laid_out) =
        editor.read_with(cx, |e, _| (e.state.text().line_count(), e.laid_out.clone()));
    assert!(lines > 3000, "{lines}");
    assert!(laid_out.len() < 120, "laid out {laid_out:?} of {lines}");

    cx.simulate_event(ScrollEvent {
        position: point(px(600.0), px(400.0)),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-60_000.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        assert!(editor.laid_out.start > 500, "{:?}", editor.laid_out);
        assert!(editor.laid_out.len() < 120);
        // Hit-testing through the height map lands on the line drawn there.
        let ix = editor.scroll_line() + 3;
        let top = editor.line_screen_top(ix);
        let viewport = editor.viewport.expect("painted");
        let offset = editor.offset_for_point(
            point(viewport.column_left + px(1.0), top + px(1.0)),
            window,
            cx,
        );
        assert_eq!(editor.state.text().line_at(offset).number, ix);
    });
}

/// `![x](file)` lines whose image is measured after the first frame.
fn image_note(dir: &std::path::Path) -> String {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="251"></svg>"#;
    std::fs::write(dir.join("a.svg"), svg).unwrap();
    let mut note = String::from("![a](a.svg)\n\n");
    for i in 0..200 {
        note.push_str(&format!("line {i}\n\n"));
    }
    note
}

#[gpui::test]
fn a_height_measured_off_screen_does_not_move_the_first_visible_line(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let note = image_note(dir.path());
    // The editor opens scrolled past the image, before it is measured.
    let path = dir.path().join("note.md");
    cx.update(actions::init);
    let (editor, cx) = cx.add_window_view(move |window, cx| {
        let mut editor = Editor::new(note, Some(path), EditorConfig::default(), cx);
        // The caret away from the image, so it is folded.
        let _ = editor.state.select(Selection::cursor(40));
        editor.set_scroll_top(2000.0);
        window.focus(&editor.focus_handle, cx);
        editor
    });
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    cx.run_until_parked();
    let (first, before) = editor.update_in(cx, |editor, window, cx| {
        // The image line is above the viewport, not yet measured: one row.
        // Laying it out starts its measurement.
        assert_eq!(editor.layout_line(0, window, cx).height, 30.0);
        let line = editor.scroll_line();
        (line, editor.line_screen_top(line))
    });
    // The measurement arrives, the next frame lays the line out again.
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.layout_line(0, window, cx).height, 251.0, "measured");
        assert_eq!(editor.heights.get(0), 251.0);
        assert_eq!(editor.scroll_line(), first);
        assert_eq!(
            editor.line_screen_top(first),
            before,
            "the first visible line did not move"
        );
    });
}

#[gpui::test]
fn an_unreadable_image_keeps_its_estimated_height(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.png"), b"not a png").unwrap();
    let path = dir.path().join("note.md");
    let (editor, cx) = open_at(
        cx,
        "![x](bad.png)\nafter\n",
        Some(path),
        EditorConfig::default(),
    );
    set_cursor(&editor, cx, 20);
    let estimated = editor.update_in(cx, |editor, window, cx| {
        editor.layout_line(0, window, cx).height
    });
    assert_eq!(estimated, 30.0, "one row before it is measured");
    // A retry (a new frame, a new layout) keeps the same height.
    editor.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.layout_line(0, window, cx).height, estimated);
        assert_eq!(editor.heights.get(0), estimated as f64);
    });
}

/// Open a 1 MB note and scroll it, timing the app's work per frame on the
/// test platform. A measurement, run in release:
/// `cargo test -p cerata --release -- --ignored megabyte`.
#[gpui::test]
#[ignore = "measurement: run with --release --ignored"]
fn megabyte_note_first_frame_and_scroll_frames(cx: &mut TestAppContext) {
    let note = long_note(1 << 20);
    let start = std::time::Instant::now();
    let (editor, cx) = open(cx, &note);
    let first = start.elapsed();
    let mut frames = Vec::new();
    for _ in 0..60 {
        let start = std::time::Instant::now();
        cx.simulate_event(ScrollEvent {
            position: point(px(600.0), px(400.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-1800.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        frames.push(start.elapsed());
    }
    frames.sort();
    let p99 = frames[frames.len() * 99 / 100 - 1];
    let lines = editor.read_with(cx, |e, _| e.state.text().line_count());
    println!(
        "1 MB note ({} bytes, {lines} lines): first frame {first:?}, scroll p50 {:?}, p99 {p99:?}, max {:?}",
        note.len(),
        frames[30],
        frames[59]
    );
    assert!(first.as_millis() < 100, "first frame {first:?}");
    assert!(p99.as_secs_f64() < 1.0 / 60.0, "scroll frame p99 {p99:?}");
}

// US-017 ---------------------------------------------------------------

#[gpui::test]
fn ime_composition_marks_text_then_commits_one_undo_step(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "ab\n");
    set_cursor(&editor, cx, 1);
    editor.update_in(cx, |editor, window, cx| {
        editor.replace_and_mark_text_in_range(None, "に", None, window, cx);
        editor.replace_and_mark_text_in_range(None, "にほ", Some(2..2), window, cx);
        assert_eq!(editor.state.doc(), "aにほb\n");
        assert_eq!(
            editor.marked_text_range(window, cx),
            Some(1..3),
            "UTF-16 units"
        );
        let bounds = editor
            .bounds_for_range(1..1, Default::default(), window, cx)
            .expect("bounds");
        let caret = editor.caret_bounds(1, window, cx).expect("caret");
        assert_eq!(
            bounds.origin, caret.origin,
            "the candidate window opens at the caret"
        );
        editor.replace_text_in_range(None, "日本", window, cx);
        assert_eq!(editor.state.doc(), "a日本b\n");
        assert!(editor.marked_range.is_none());
        assert_eq!(editor.state.history().undo_depth(), 1);
    });
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), "ab\n", "one step undoes the composition");
}

#[gpui::test]
fn a_composition_canceled_by_escape_leaves_buffer_and_history_unchanged(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "ab\n");
    set_cursor(&editor, cx, 1);
    cx.simulate_input("x");
    let depth = editor.read_with(cx, |e, _| e.state.history().undo_depth());
    editor.update_in(cx, |editor, window, cx| {
        editor.replace_and_mark_text_in_range(None, "に", None, window, cx);
        editor.replace_and_mark_text_in_range(None, "にほん", None, window, cx);
        // Escape: the IME sends an empty commit (Windows IMM32, `lparam == 0`).
        editor.replace_text_in_range(None, "", window, cx);
        assert!(editor.marked_range.is_none());
    });
    assert_eq!(doc(&editor, cx), "axb\n");
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.state.history().undo_depth(), depth);
        assert_eq!(editor.state.selection().main().head, 2);
    });
    // An empty marked text (macOS) cancels the same way.
    editor.update_in(cx, |editor, window, cx| {
        editor.replace_and_mark_text_in_range(None, "^", None, window, cx);
        editor.replace_and_mark_text_in_range(None, "", None, window, cx);
    });
    assert_eq!(doc(&editor, cx), "axb\n");
}

#[gpui::test]
fn typing_and_dead_keys_reach_the_buffer(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "\n");
    set_cursor(&editor, cx, 0);
    cx.simulate_input("caf");
    editor.update_in(cx, |editor, window, cx| {
        // A dead key: marked `´`, then the composed character.
        editor.replace_and_mark_text_in_range(None, "\u{b4}", None, window, cx);
        editor.replace_text_in_range(None, "\u{e9}", window, cx);
    });
    assert_eq!(doc(&editor, cx), "caf\u{e9}\n");
}

#[gpui::test]
fn every_editing_shortcut_is_bound(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "one two\nthree\nfour two\n");
    let s = secondary;
    // Undo and redo.
    set_cursor(&editor, cx, 3);
    cx.simulate_input("!");
    cx.simulate_keystrokes(&s("z"));
    assert_eq!(doc(&editor, cx), "one two\nthree\nfour two\n");
    cx.simulate_keystrokes(&s("shift-z"));
    assert_eq!(doc(&editor, cx), "one! two\nthree\nfour two\n");
    cx.simulate_keystrokes(&s("z"));
    // Select all.
    cx.simulate_keystrokes(&s("a"));
    assert_eq!(selection(&editor, cx), [(0, 23)]);
    // Next occurrence, all occurrences.
    set_cursor(&editor, cx, 5);
    cx.simulate_keystrokes(&s("d"));
    cx.simulate_keystrokes(&s("d"));
    assert_eq!(selection(&editor, cx), [(4, 7), (19, 22)]);
    set_cursor(&editor, cx, 5);
    cx.simulate_keystrokes(&s("shift-l"));
    assert_eq!(selection(&editor, cx).len(), 2);
    // Move and copy lines.
    set_cursor(&editor, cx, 9);
    cx.simulate_keystrokes("alt-up");
    assert_eq!(doc(&editor, cx), "three\none two\nfour two\n");
    cx.simulate_keystrokes("alt-down");
    assert_eq!(doc(&editor, cx), "one two\nthree\nfour two\n");
    cx.simulate_keystrokes("alt-shift-down");
    assert_eq!(doc(&editor, cx), "one two\nthree\nthree\nfour two\n");
    cx.simulate_keystrokes("alt-shift-up");
    assert_eq!(doc(&editor, cx), "one two\nthree\nthree\nthree\nfour two\n");
    // Delete line.
    cx.simulate_keystrokes(&s("shift-k"));
    assert_eq!(doc(&editor, cx), "one two\nthree\nthree\nfour two\n");
    // Insert line below.
    set_cursor(&editor, cx, 1);
    cx.simulate_keystrokes(&s("enter"));
    assert_eq!(doc(&editor, cx), "one two\n\nthree\nthree\nfour two\n");
    assert_eq!(selection(&editor, cx), [(8, 8)]);
    // Indent more and less, Tab and Shift+Tab.
    set_cursor(&editor, cx, 0);
    cx.simulate_keystrokes(&s("]"));
    assert!(doc(&editor, cx).starts_with("  one"));
    cx.simulate_keystrokes(&s("["));
    assert!(doc(&editor, cx).starts_with("one"));
    cx.simulate_keystrokes("tab");
    assert!(doc(&editor, cx).starts_with("\tone"));
    cx.simulate_keystrokes("backspace");
    // Word selection.
    set_cursor(&editor, cx, 0);
    cx.simulate_keystrokes("alt-shift-right");
    assert_eq!(selection(&editor, cx), [(0, 3)]);
    cx.simulate_keystrokes("alt-shift-left");
    assert_eq!(selection(&editor, cx), [(0, 0)]);
    // Find, find next and previous, Escape.
    set_cursor(&editor, cx, 0);
    cx.simulate_keystrokes(&s("f"));
    assert!(editor.read_with(cx, |e, _| e.search.is_some()));
    cx.simulate_input("two");
    cx.simulate_keystrokes(&s("g"));
    assert_eq!(selection(&editor, cx), [(4, 7)]);
    cx.simulate_keystrokes(&s("g"));
    let second = selection(&editor, cx);
    assert_ne!(second, [(4, 7)]);
    cx.simulate_keystrokes(&s("shift-g"));
    assert_eq!(selection(&editor, cx), [(4, 7)]);
    cx.simulate_keystrokes("escape");
    assert!(editor.read_with(cx, |e, _| e.search.is_none()));
    // Formatting through the registry (Bold).
    set_cursor(&editor, cx, 1);
    cx.simulate_keystrokes(&s("b"));
    assert!(doc(&editor, cx).starts_with("**one**"));
    // Shift+Tab reaches the list outdent.
    editor.update(cx, |editor, cx| {
        editor.update_state(cx, |state| {
            let len = state.text().len();
            let _ = state.apply(
                Transaction::new().change(cerata_markdown::Change::new(0..len, "- a\n  - b\n")),
            );
        });
    });
    set_cursor(&editor, cx, 9);
    cx.simulate_keystrokes("shift-tab");
    assert_eq!(doc(&editor, cx), "- a\n- b\n");
}

#[gpui::test]
fn every_cursor_motion_keeps_the_caret_inside_the_safe_margins(cx: &mut TestAppContext) {
    let note: String = (0..400).map(|i| format!("line {i}\n")).collect();
    let (editor, cx) = open(cx, &note);
    let check = |editor: &Entity<Editor>, cx: &mut VisualTestContext, what: &str| {
        editor.update_in(cx, |editor, window, cx| {
            let caret = editor
                .caret_bounds(editor.cursor(), window, cx)
                .expect("caret");
            let viewport = editor.viewport.expect("painted");
            let top = viewport.bounds.top() + px(super::TOP_INSET);
            let bottom = viewport.bounds.bottom();
            let margin = px(SAFE_MARGIN as f32);
            assert!(
                caret.top() >= top + margin - px(1.0),
                "{what}: {caret:?} above the safe zone"
            );
            assert!(
                caret.bottom() <= bottom - margin + px(1.0),
                "{what}: {caret:?} below the safe zone"
            );
        });
    };
    for keys in ["pagedown", "pagedown", "down", "pageup", "up"] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        check(&editor, cx, keys);
    }
    let end = if cfg!(target_os = "macos") {
        "cmd-down"
    } else {
        "ctrl-end"
    };
    let start = if cfg!(target_os = "macos") {
        "cmd-up"
    } else {
        "ctrl-home"
    };
    cx.simulate_keystrokes(end);
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        // The last line can sit lower than the margin: the note ends there.
        let caret = editor
            .caret_bounds(editor.cursor(), window, cx)
            .expect("caret");
        assert!(caret.bottom() <= editor.viewport.expect("painted").bounds.bottom());
    });
    cx.simulate_keystrokes(start);
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        // The note cannot scroll above its start: the first line shows at
        // the top, inside the margin.
        assert_eq!(editor.scroll_top(), 0.0);
        let caret = editor
            .caret_bounds(editor.cursor(), window, cx)
            .expect("caret");
        assert!(caret.top() >= editor.viewport.expect("painted").bounds.top());
    });
}

/// Writer's `keyboard-scroll.spec.js`: holding Down then Up through a long
/// note keeps the caret on screen at every step and never edits.
#[gpui::test]
fn holding_down_then_up_follows_the_caret_through_a_long_note(cx: &mut TestAppContext) {
    let mut lines = vec!["# Keyboard scroll check".to_owned(), String::new()];
    for s in 1..=8 {
        lines.push(format!("## Section {s}"));
        lines.push(String::new());
        for i in 1..=8 {
            lines.push(format!("Line {s}.{i}: scroll check paragraph."));
        }
        lines.extend([
            String::new(),
            format!("- item {s}a"),
            format!("- item {s}b"),
            String::new(),
        ]);
    }
    let note = lines.join("\n");
    let (editor, cx) = open(cx, &note);
    set_cursor(&editor, cx, 0);
    let mut max_scroll = 0.0f64;
    let mut steps = 0;
    loop {
        let before = editor.read_with(cx, |e, _| e.cursor());
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        let (after, len) = editor.read_with(cx, |e, _| (e.cursor(), e.state.text().len()));
        editor.update_in(cx, |editor, window, cx| {
            let caret = editor.caret_bounds(after, window, cx).expect("caret");
            let bounds = editor.viewport.expect("painted").bounds;
            assert!(
                caret.top() >= bounds.top() && caret.bottom() <= bounds.bottom(),
                "step {steps}: caret left the view"
            );
            max_scroll = max_scroll.max(editor.scroll_top());
        });
        steps += 1;
        if after == len || after == before || steps > 500 {
            break;
        }
    }
    assert_eq!(
        doc(&editor, cx),
        note,
        "arrow navigation edited the document"
    );
    let height = editor.read_with(cx, |e, _| e.viewport_height());
    assert!(max_scroll > height, "never scrolled a full viewport");
    let bottom_scroll = editor.read_with(cx, |e, _| e.scroll_top());
    for _ in 0..steps + 2 {
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        if editor.read_with(cx, |e, _| e.cursor()) == 0 {
            break;
        }
    }
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.cursor(), 0);
        assert!(
            editor.scroll_top() < bottom_scroll,
            "Up did not scroll back"
        );
    });
}

/// Writer's `list-enter-exit.spec.js`: Enter, Enter, type ends the list;
/// the empty item is cleared in place and the text is a paragraph.
#[gpui::test]
fn enter_enter_type_ends_a_list(cx: &mut TestAppContext) {
    let note = "# List enter check\n\n- first item\n- im creating a sub item!!\n- last item\n\nPlain paragraph after the list.\n";
    for backspace in [false, true] {
        let (editor, cx) = open(cx, note);
        let at = note.find("sub item!!").unwrap() + "sub item!!".len();
        set_cursor(&editor, cx, at);
        cx.simulate_keystrokes("enter");
        if backspace {
            cx.simulate_keystrokes("backspace");
        } else {
            cx.simulate_keystrokes("enter");
        }
        cx.simulate_input("typed");
        let text = doc(&editor, cx);
        let lines: Vec<&str> = text.lines().collect();
        let i = lines
            .iter()
            .position(|l| *l == "typed")
            .unwrap_or_else(|| unreachable_line(&lines));
        assert!(lines[i - 1].starts_with("- "), "{lines:?}");
        assert_eq!(
            lines.len(),
            note.lines().count() + 1,
            "nothing below moved: {lines:?}"
        );
        editor.update_in(cx, |editor, window, cx| {
            let line = editor.layout_line(i, window, cx);
            assert_eq!(line.text_left, 0.0, "the typed text sits at the margin");
            assert!(line.prefix.is_none());
        });
    }
}

fn unreachable_line(lines: &[&str]) -> usize {
    assert_eq!(lines, ["typed"], "typed line not found");
    0
}

// US-018 ---------------------------------------------------------------

#[gpui::test]
fn clicks_map_through_hidden_syntax_and_list_prefix_zones(cx: &mut TestAppContext) {
    // At depth 0 the prefix has no line-start zone (Writer's
    // `listPrefixClickPosition`): the nested item has all three.
    let (editor, cx) = open(cx, "**bold** text\n\n- a\n  - item body\n");
    set_cursor(&editor, cx, 14);
    // The `**` are hidden: the left edge of "bold" is offset 2.
    let at = point_at(&editor, cx, 2);
    press(cx, at, 1, false);
    release(cx, at);
    assert_eq!(selection(&editor, cx), [(2, 2)]);

    set_cursor(&editor, cx, 0);
    let (line_start, column_left, ch, prefix_left, text_left) =
        editor.update_in(cx, |editor, window, cx| {
            let line = editor.layout_line(3, window, cx);
            let prefix = line.prefix.clone().expect("list prefix");
            (
                editor.line_screen_top(3),
                editor.viewport.expect("painted").column_left,
                editor.ch(window),
                prefix.left,
                line.text_left,
            )
        });
    let y = line_start + px(10.0);
    let unit = cerata_markdown::list::LIST_UNIT_CH as f32 * ch;
    let marker_start = text_left - unit;
    for (x, expected, what) in [
        (prefix_left + 0.5, 19, "line start"),
        (marker_start + 1.0, 21, "marker"),
        (text_left - 1.0, 23, "body"),
    ] {
        let position = point(column_left + px(x), y);
        press(cx, position, 1, false);
        release(cx, position);
        assert_eq!(selection(&editor, cx), [(expected, expected)], "{what}");
    }
}

#[gpui::test]
fn a_drag_from_a_list_prefix_and_its_rectangles_stay_on_the_text(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "- first item\n- second\n");
    set_cursor(&editor, cx, 22);
    let start = editor.update_in(cx, |editor, window, cx| {
        let line = editor.layout_line(0, window, cx);
        point(
            editor.viewport.expect("painted").column_left + px(line.text_left - 1.0),
            editor.line_screen_top(0) + px(10.0),
        )
    });
    press(cx, start, 1, false);
    let first = selection(&editor, cx);
    assert_eq!(
        first[0].0, first[0].1,
        "the press does not select the whole line"
    );
    let to = point_at(&editor, cx, 8);
    drag_to(cx, to, true);
    release(cx, to);
    let (anchor, head) = selection(&editor, cx)[0];
    assert_eq!((anchor.min(head), anchor.max(head)), (2, 8));
    // The rectangle ends at the selected text, not at the column edge.
    editor.update_in(cx, |editor, window, cx| {
        let line = editor.layout_line(0, window, cx);
        let column_left = editor.viewport.expect("painted").column_left;
        let rects =
            super::element::range_rects(&line, editor.line_screen_top(0), column_left, &(2..8));
        let end = editor.caret_bounds(8, window, cx).expect("caret").origin.x;
        assert_eq!(rects.len(), 1);
        assert!(
            rects[0].right() <= end + px(0.5),
            "{:?} past {end:?}",
            rects[0]
        );
    });
}

#[gpui::test]
fn a_click_on_a_widget_selects_its_decoration_anchored_at_its_end(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "intro\n\n---\n\nafter\n");
    set_cursor(&editor, cx, 0);
    let position = editor.read_with(cx, |editor, _| {
        point(
            editor.viewport.expect("painted").column_left + px(40.0),
            editor.line_screen_top(2) + px(15.0),
        )
    });
    press(cx, position, 1, false);
    release(cx, position);
    assert_eq!(
        selection(&editor, cx),
        [(10, 7)],
        "anchor at the end of `---`"
    );
}

#[gpui::test]
fn double_click_selects_a_word_and_triple_click_a_line(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "alpha beta gamma\nnext\n");
    let at = point_at(&editor, cx, 8);
    press(cx, at, 2, false);
    release(cx, at);
    assert_eq!(selection(&editor, cx), [(6, 10)]);
    press(cx, at, 3, false);
    release(cx, at);
    assert_eq!(selection(&editor, cx), [(0, 17)]);
}

#[gpui::test]
fn a_drag_released_outside_the_window_stops_at_its_last_position(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "some **bold** words here\n");
    let start = point_at(&editor, cx, 0);
    press(cx, start, 1, false);
    assert!(
        editor.read_with(cx, |e, _| e.state.is_dragging()),
        "reveal frozen while dragging"
    );
    let to = point_at(&editor, cx, 12);
    drag_to(cx, to, true);
    // The button is released outside: the next move has no button down.
    drag_to(cx, point(px(5000.0), px(5000.0)), false);
    editor.read_with(cx, |editor, _| {
        assert!(!editor.state.is_dragging(), "the freeze is lifted");
        // Offset 12 sits inside the hidden closing `**`: the selection
        // takes the markup that follows the visible text.
        assert_eq!(editor.state.selection().main().range(), 0..13);
    });
    // A drag that leaves the window keeps following, and a release out
    // there reaches the editor.
    let start = point_at(&editor, cx, 16);
    press(cx, start, 1, false);
    let anchor = selection(&editor, cx)[0].0;
    drag_to(cx, point(px(-400.0), start.y), true);
    release(cx, point(px(-400.0), start.y));
    editor.read_with(cx, |editor, _| {
        assert!(!editor.state.is_dragging());
        assert_eq!(editor.state.selection().main().range(), 0..anchor);
    });
}

#[gpui::test]
fn a_click_on_a_checkbox_toggles_it(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "- [ ] task\n\nend\n");
    set_cursor(&editor, cx, 13);
    let position = editor.update_in(cx, |editor, window, cx| {
        let line = editor.layout_line(0, window, cx);
        let prefix = line.prefix.clone().expect("task prefix");
        point(
            editor.viewport.expect("painted").column_left + px(prefix.x + prefix.width / 2.0),
            editor.line_screen_top(0) + px(line.metrics.row_height / 2.0),
        )
    });
    press(cx, position, 1, false);
    release(cx, position);
    assert_eq!(doc(&editor, cx), "- [x] task\n\nend\n");
}

// US-019 ---------------------------------------------------------------

/// Writer's `code-block-scroll.spec.js`, plus the frontmatter.
#[gpui::test]
fn code_blocks_scroll_horizontally_as_one_block(cx: &mut TestAppContext) {
    let long = format!("const long = \"{}\";", "x".repeat(200));
    let note = format!(
        "---\ntitle: {}\n---\n\n# Code scroll check\n\n```ts\nconst short = 1;\n{long}\n```\n\nafter\n",
        "y".repeat(200)
    );
    let (editor, cx) = open(cx, &note);
    let fence_line = 6;
    let (short_line, long_line) = (fence_line + 1, fence_line + 2);
    editor.update_in(cx, |editor, window, cx| {
        let line = editor.layout_line(long_line, window, cx);
        assert_eq!(line.rows(), 1, "the long line does not wrap");
    });
    // A mostly vertical gesture scrolls the note, not the block.
    let over = editor.read_with(cx, |e, _| {
        point(
            e.viewport.expect("painted").column_left + px(100.0),
            e.line_screen_top(long_line) + px(10.0),
        )
    });
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(-10.0), px(-30.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.code_scroll.offset_of_line(fence_line)),
        0.0
    );
    // A horizontal one scrolls every line of the block together.
    let over = editor.read_with(cx, |e, _| {
        point(
            e.viewport.expect("painted").column_left + px(100.0),
            e.line_screen_top(long_line) + px(10.0),
        )
    });
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(-300.0), px(0.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.code_scroll.offset_of_line(fence_line), 300.0);
        let short = editor.layout_line(short_line, window, cx);
        assert_eq!(
            short.code.as_ref().expect("code").scroll_x,
            300.0,
            "same offset on every line"
        );
    });
    // Clamped at the widest line, and back to zero.
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(-100_000.0), px(0.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    let max = editor.update_in(cx, |editor, window, cx| {
        let line = editor.layout_line(long_line, window, cx);
        let code = line.code.expect("code");
        let visible = editor.column_width() - super::layout::CODE_PADDING * 2.0;
        assert!((code.scroll_x - (code.text_width - visible).max(0.0)).abs() < 0.01);
        code.scroll_x
    });
    assert!(max > 0.0);
    // The drawn thumb has a minimum width and drags the block back.
    let thumb = editor.read_with(cx, |e, _| {
        e.thumbs
            .iter()
            .find(|t| t.first_line == fence_line)
            .copied()
            .expect("the block's thumb")
    });
    assert!(thumb.bounds.size.width >= px(super::code_scroll::MIN_THUMB_WIDTH));
    let grab = thumb.bounds.center();
    press(cx, grab, 1, false);
    drag_to(cx, point(grab.x - px(2000.0), grab.y), true);
    release(cx, point(grab.x - px(2000.0), grab.y));
    assert_eq!(
        editor.read_with(cx, |e, _| e.code_scroll.offset_of_line(fence_line)),
        0.0
    );
    // The caret at the end of the long line is revealed by the smallest move.
    let end = note.find(&long).unwrap() + long.len();
    set_cursor(&editor, cx, end);
    editor.update_in(cx, |editor, window, cx| {
        let offset = editor.code_scroll.offset_of_line(fence_line);
        assert!(offset > 0.0 && offset <= max, "{offset}");
        let caret = editor.caret_bounds(end, window, cx).expect("caret");
        let column = editor.viewport.expect("painted");
        let right = column.column_left + px(column.column_width - super::layout::CODE_PADDING);
        assert!(
            caret.left() <= right && caret.left() >= right - px(4.0),
            "just inside the right edge: {caret:?}"
        );
    });
    // The frontmatter scrolls the same way.
    set_cursor(&editor, cx, 0);
    let over_fm = editor.read_with(cx, |e, _| {
        point(
            e.viewport.expect("painted").column_left + px(100.0),
            e.line_screen_top(1) + px(10.0),
        )
    });
    cx.simulate_event(ScrollEvent {
        position: over_fm,
        delta: ScrollDelta::Pixels(point(px(-50.0), px(0.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.code_scroll.offset_of_line(0)),
        50.0
    );
}

#[gpui::test]
fn a_block_made_narrower_than_the_view_goes_back_to_zero(cx: &mut TestAppContext) {
    let long = "z".repeat(300);
    let note = format!("intro\n```\n{long}\n```\nafter\n");
    let (editor, cx) = open(cx, &note);
    let over = editor.read_with(cx, |e, _| {
        point(
            e.viewport.expect("painted").column_left + px(50.0),
            e.line_screen_top(2) + px(10.0),
        )
    });
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(-200.0), px(0.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.code_scroll.offset_of_line(1)),
        200.0
    );
    // Typing above the block keeps its offset (remapped).
    set_cursor(&editor, cx, 0);
    cx.simulate_input("x\n");
    cx.run_until_parked();
    // The typed line goes before the fence: the block starts on line 2.
    assert_eq!(
        editor.read_with(cx, |e, _| e.code_scroll.offset_of_line(2)),
        200.0
    );
    // Select most of the long line: the block scrolls to its end.
    let from = "x\nintro\n```\n".len();
    editor.update(cx, |editor, cx| {
        editor.select(Selection::single(from + 40, from + 300), cx);
    });
    cx.run_until_parked();
    // The selection of the scrolled line stays inside the block's box.
    editor.read_with(cx, |editor, _| {
        let viewport = editor.viewport.expect("painted");
        let left = viewport.column_left + px(super::layout::CODE_PADDING);
        let right = viewport.column_left + px(viewport.column_width - super::layout::CODE_PADDING);
        let frame = editor.frames.last().expect("a frame");
        assert!(!frame.selections.is_empty());
        // At the largest offset the caret ends the widest line, on the
        // box's edge: it is still drawn.
        assert_eq!(frame.carets.len(), 1, "{:?}", frame.carets);
        for rect in &frame.selections {
            assert!(rect.left() >= left && rect.right() <= right, "{rect:?}");
        }
    });
    // Delete it with the main caret outside the block, so no caret reveal
    // moves the offset first: the block fits, offset and bar go.
    let after = from + 300 + "\n```\n".len() + 3;
    editor.update(cx, |editor, cx| {
        assert!(editor.code_scroll.offset_of_line(2) > 0.0);
        let ranges = vec![
            cerata_markdown::SelectionRange::new(from + 40, from + 300),
            cerata_markdown::SelectionRange::cursor(after),
        ];
        editor.select(Selection::new(ranges, 1), cx);
    });
    cx.run_until_parked();
    editor.update(cx, |editor, _| editor.frames.clear());
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.code_scroll.offset_of_line(2), 0.0);
        assert!(editor.thumbs.is_empty());
        // The first frame that shows the edit already paints at offset 0.
        let frame = editor.frames.first().expect("a frame after the edit");
        assert!(!frame.code_scroll.is_empty());
        assert!(
            frame.code_scroll.iter().all(|(_, x)| *x == 0.0),
            "{:?}",
            frame.code_scroll
        );
    });
}

// US-020 ---------------------------------------------------------------

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::new(width, height)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[gpui::test]
fn local_and_data_images_render_at_their_measured_height(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("img dir")).unwrap();
    std::fs::write(dir.path().join("img dir/a.png"), png(400, 150)).unwrap();
    let data = {
        let bytes = png(20, 10);
        let b64: String = base64_encode(&bytes);
        format!("data:image/png;base64,{b64}")
    };
    let note = format!(
        "![a](img%20dir/a.png)\n\n![d]({data})\n\n![h](http://example.com/x.png)\n\n![m](missing.png)\n"
    );
    let path = dir.path().join("note.md");
    let (editor, cx) = open_at(cx, &note, Some(path), EditorConfig::default());
    // The caret away from the images, which it would reveal as source.
    set_cursor(&editor, cx, note.len());
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        let local = editor.layout_line(0, window, cx);
        let block = local.block.clone().expect("local image");
        assert!(block.replaces);
        assert_eq!((block.width, block.height), (400.0, 150.0));
        assert_eq!(local.height, 150.0);
        let inline = editor.layout_line(2, window, cx);
        assert_eq!(inline.block.expect("data image").height, 10.0);
        // `http:` is never loaded; a missing file shows its source muted.
        for ix in [4, 6] {
            let line = editor.layout_line(ix, window, cx);
            assert!(line.block.is_none(), "line {ix}");
            assert!(!line.display.text.is_empty(), "source shown");
        }
    });
    assert_eq!(
        cx.debug_bounds("editor-image-0")
            .expect("painted")
            .size
            .height,
        px(150.0)
    );
}

fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[gpui::test]
fn a_pasted_image_is_saved_next_to_the_note(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("my note.md");
    let (editor, cx) = open_at(cx, "start\n", Some(path), EditorConfig::default());
    set_cursor(&editor, cx, 5);
    let image = Image::from_bytes(ImageFormat::Png, png(4, 4));
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![ClipboardEntry::Image(image)],
    });
    let before = chrono::Local::now().format("%Y%m%d").to_string();
    cx.simulate_keystrokes(&secondary("v"));
    let text = doc(&editor, cx);
    assert!(
        text.starts_with("start![image.png](<my note-assets/"),
        "{text}"
    );
    let assets: Vec<_> = std::fs::read_dir(dir.path().join("my note-assets"))
        .unwrap()
        .collect();
    assert_eq!(assets.len(), 1);
    let name = assets[0]
        .as_ref()
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();
    assert!(
        name.starts_with(&before) && name.ends_with(".png"),
        "{name}"
    );
    assert_eq!(name.len(), "YYYYMMDD-HHMMSS-xxxx.png".len());
}

#[gpui::test]
fn an_image_over_5_mb_is_refused_with_a_notice(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    let (editor, cx) = open_at(cx, "start\n", Some(path), EditorConfig::default());
    let mut bytes = png(4, 4);
    bytes.resize(5 * 1024 * 1024 + 300_000, 0);
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![ClipboardEntry::Image(Image::from_bytes(
            ImageFormat::Png,
            bytes,
        ))],
    });
    cx.simulate_keystrokes(&secondary("v"));
    assert_eq!(doc(&editor, cx), "start\n");
    assert!(!dir.path().join("note-assets").exists(), "nothing written");
    let notice = editor
        .read_with(cx, |e, _| e.notice.clone())
        .expect("notice");
    assert_eq!(
        notice.as_ref(),
        "Image not pasted: 5.3 MB is over the 5 MB limit"
    );
}

// US-021 ---------------------------------------------------------------

fn search_counter(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> String {
    editor.read_with(cx, |e, cx| {
        e.search.as_ref().expect("open").read(cx).counter()
    })
}

#[gpui::test]
fn find_is_prefilled_literal_and_case_insensitive(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "Alpha a.b alpha ALPHA\nnext alpha\n");
    editor.update(cx, |e, cx| e.select(Selection::single(0, 5), cx));
    cx.simulate_keystrokes(&secondary("f"));
    assert_eq!(search_counter(&editor, cx), "1/4");
    // Typing replaces the prefilled (selected) query.
    cx.simulate_input("a.b");
    assert_eq!(
        search_counter(&editor, cx),
        "1/1",
        "literal: `.` is not a wildcard"
    );
    // A multi-line selection does not prefill.
    cx.simulate_keystrokes("escape");
    editor.update(cx, |e, cx| e.select(Selection::single(0, 25), cx));
    cx.simulate_keystrokes(&secondary("f"));
    let query = editor.read_with(cx, |e, cx| {
        e.search.as_ref().expect("open").read(cx).query(cx)
    });
    assert_eq!(query, "a.b");
}

#[gpui::test]
fn enter_steps_and_replace_all_is_one_undo(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "cat dog cat dog cat\n");
    set_cursor(&editor, cx, 0);
    cx.simulate_keystrokes(&secondary("f"));
    cx.simulate_input("cat");
    cx.simulate_keystrokes("enter");
    assert_eq!(selection(&editor, cx), [(0, 3)]);
    cx.simulate_keystrokes("enter");
    assert_eq!(selection(&editor, cx), [(8, 11)]);
    assert_eq!(search_counter(&editor, cx), "2/3");
    cx.simulate_keystrokes("shift-enter");
    assert_eq!(selection(&editor, cx), [(0, 3)]);
    // A click in the replace field focuses it and leaves the text alone;
    // Enter there replaces the selected result.
    let replace = editor.read_with(cx, |e, cx| {
        e.search.as_ref().expect("open").read(cx).replace.clone()
    });
    let field = cx
        .debug_bounds("text-field-Replace")
        .expect("replace field drawn");
    press(cx, field.center(), 1, false);
    release(cx, field.center());
    cx.run_until_parked();
    assert!(cx.update(|window, cx| replace.read(cx).focus_handle.is_focused(window)));
    assert_eq!(
        selection(&editor, cx),
        [(0, 3)],
        "the click stays in the bar"
    );
    cx.simulate_input("cow");
    cx.simulate_keystrokes("enter");
    assert_eq!(doc(&editor, cx), "cow dog cat dog cat\n");
    assert_eq!(selection(&editor, cx), [(8, 11)], "on to the next result");
    editor.update(cx, |e, cx| e.replace_all(cx));
    assert_eq!(doc(&editor, cx), "cow dog cow dog cow\n");
    editor.update(cx, |e, cx| {
        e.update_state(cx, |s| s.undo());
    });
    assert_eq!(
        doc(&editor, cx),
        "cow dog cat dog cat\n",
        "one undo for replace all"
    );
}

#[gpui::test]
fn no_result_shows_0_of_0_flags_the_field_and_enter_does_nothing(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "some text\n");
    set_cursor(&editor, cx, 2);
    cx.simulate_keystrokes(&secondary("f"));
    cx.simulate_input("zzz");
    assert_eq!(search_counter(&editor, cx), "0/0");
    let invalid = editor.read_with(cx, |e, cx| {
        e.search
            .as_ref()
            .expect("open")
            .read(cx)
            .find
            .read(cx)
            .invalid
    });
    assert!(invalid);
    cx.simulate_keystrokes("enter");
    assert_eq!(selection(&editor, cx), [(2, 2)]);
    assert_eq!(doc(&editor, cx), "some text\n");
}

#[gpui::test]
fn marks_are_capped_merged_and_a_jump_lands_140_px_from_the_edge(cx: &mut TestAppContext) {
    let note: String = (0..6000).map(|i| format!("hit {i}\n")).collect();
    let (editor, cx) = open(cx, &note);
    set_cursor(&editor, cx, 0);
    cx.simulate_keystrokes(&secondary("f"));
    cx.simulate_input("hit");
    cx.run_until_parked();
    editor.read_with(cx, |editor, cx| {
        let search = editor.search.as_ref().expect("open").read(cx);
        assert_eq!(search.matches().len(), 6000);
        assert_eq!(search.marks().count(), super::search::MAX_MARKS);
        // Merged per pixel: at most one mark per pixel row of the bar.
        let height = editor
            .viewport
            .expect("painted")
            .bounds
            .size
            .height
            .as_f32() as usize;
        assert!(
            editor.mark_hits.len() <= height + 1,
            "{}",
            editor.mark_hits.len()
        );
    });
    // A click on a mark far down jumps there, 140 px from the bottom edge.
    let (mark, pos) = editor.read_with(cx, |e, _| e.mark_hits[e.mark_hits.len() / 2]);
    press(cx, mark.center(), 1, false);
    release(cx, mark.center());
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.state.selection().main().from(), pos);
        let caret = editor.caret_bounds(pos, window, cx).expect("caret");
        let bottom = editor.viewport.expect("painted").bounds.bottom();
        let gap = (bottom - caret.bottom()).as_f32();
        assert!((gap - SAFE_MARGIN as f32).abs() <= 1.0, "{gap}");
    });
}

#[gpui::test]
fn escape_and_an_inactive_tab_close_the_search(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "text\n");
    cx.simulate_keystrokes(&secondary("f"));
    cx.simulate_keystrokes("escape");
    editor.update_in(cx, |editor, window, _| {
        assert!(editor.search.is_none());
        assert!(
            editor.focus_handle.is_focused(window),
            "focus back to the text"
        );
    });
    cx.simulate_keystrokes(&secondary("f"));
    editor.update(cx, |editor, cx| editor.set_active(false, cx));
    assert!(editor.read_with(cx, |e, _| e.search.is_none()));
}

// US-022 ---------------------------------------------------------------

#[gpui::test]
fn cut_copy_and_paste_work_in_plain_text(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "**bold** rest\n");
    editor.update(cx, |e, cx| e.select(Selection::single(0, 8), cx));
    cx.simulate_keystrokes(&secondary("c"));
    assert_eq!(
        cx.read_from_clipboard().and_then(|i| i.text()).as_deref(),
        Some("**bold**")
    );
    cx.simulate_keystrokes(&secondary("x"));
    assert_eq!(doc(&editor, cx), " rest\n");
    set_cursor(&editor, cx, 5);
    cx.simulate_keystrokes(&secondary("v"));
    assert_eq!(doc(&editor, cx), " rest**bold**\n");
}

#[gpui::test]
fn pasting_a_document_with_frontmatter_moves_its_yaml_to_the_top(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "intro\n");
    set_cursor(&editor, cx, 6);
    cx.write_to_clipboard(ClipboardItem::new_string(
        "---\ntitle: a\n---\nbody text".to_owned(),
    ));
    cx.simulate_keystrokes(&secondary("v"));
    assert_eq!(doc(&editor, cx), "---\ntitle: a\n---\nintro\nbody text");
    // A note that already has one gets the paste as is.
    cx.simulate_keystrokes(&secondary("v"));
    assert!(doc(&editor, cx).ends_with("body text---\ntitle: a\n---\nbody text"));
    // An empty frontmatter stays empty, with no blank line inside.
    let (editor, cx) = open(cx, "intro\n");
    set_cursor(&editor, cx, 6);
    cx.write_to_clipboard(ClipboardItem::new_string("---\n---\nbody".to_owned()));
    cx.simulate_keystrokes(&secondary("v"));
    assert_eq!(doc(&editor, cx), "---\n---\nintro\nbody");
}

#[gpui::test]
fn the_context_menu_is_drawn_in_the_app_with_the_registry(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "see [site](https://a.b) here\n");
    cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    let on_link = point_at(&editor, cx, 6);
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: on_link,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        let menu = editor.menu.as_ref().expect("menu open");
        let has = |pred: &dyn Fn(&super::menu::MenuItem) -> bool| menu.items.iter().any(pred);
        assert!(has(&|i| matches!(i, super::menu::MenuItem::Action { action: super::menu::MenuAction::OpenLink(l), .. } if l == "https://a.b")));
        assert!(has(&|i| matches!(i, super::menu::MenuItem::Action { action: super::menu::MenuAction::Paste, enabled: false, .. })), "empty clipboard: Paste disabled");
    });
    // Run Bold from the Format submenu.
    editor.update(cx, |editor, cx| {
        editor.select(Selection::single(24, 28), cx);
        editor.run_menu_action(super::menu::MenuAction::Command("format.bold"), cx);
    });
    assert_eq!(doc(&editor, cx), "see [site](https://a.b) **here**\n");
    assert!(editor.read_with(cx, |e, _| e.menu.is_none()));
}
