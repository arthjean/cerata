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

/// Rewritten for ADR 0008: a rule still needs an empty selection; a block
/// image is selected as an object whatever is selected
/// (`a_click_on_an_image_selects_it_as_an_object_without_moving_it`).
#[gpui::test]
fn a_click_on_a_widget_selects_its_decoration_anchored_at_its_end(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "intro\n\n---\n\nafter\n");
    // A selection: the click on the rule places the caret instead.
    editor.update(cx, |e, cx| e.select(Selection::single(0, 3), cx));
    cx.run_until_parked();
    let rule = editor.read_with(cx, |editor, _| {
        point(
            editor.viewport.expect("painted").column_left + px(40.0),
            editor.line_screen_top(2) + px(15.0),
        )
    });
    press(cx, rule, 1, false);
    release(cx, rule);
    assert_ne!(selection(&editor, cx), [(10, 7)], "not the rule");
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

/// US-033: `![[pic.png]]` shows the image found anywhere in the workspace
/// (Obsidian's shortest-path links); an embed found nowhere shows its
/// source muted (`ImageEmbedWidget`).
#[gpui::test]
fn image_embeds_render_the_file_they_name(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("deep/attachments")).unwrap();
    std::fs::write(dir.path().join("deep/attachments/pic.png"), png(300, 120)).unwrap();
    std::fs::create_dir(dir.path().join("notes")).unwrap();
    let note = "![[pic.png]]\n\n![[nowhere.png]]\n\nend";
    let path = dir.path().join("notes/note.md");
    let (editor, cx) = open_at(cx, note, Some(path), EditorConfig::default());
    editor.update(cx, |e, _| e.images.set_root(dir.path()));
    set_cursor(&editor, cx, note.len());
    for _ in 0..50 {
        cx.run_until_parked();
        let done = editor.update_in(cx, |e, window, cx| {
            e.layout_line(0, window, cx)
                .block
                .is_some_and(|b| b.height == 120.0)
                && e.layout_line(2, window, cx).block.is_none()
        });
        if done {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    editor.update_in(cx, |e, window, cx| {
        let found = e.layout_line(0, window, cx).block.expect("embed drawn");
        assert_eq!((found.width, found.height), (300.0, 120.0));
        let missing = e.layout_line(2, window, cx);
        assert!(missing.block.is_none());
        assert!(missing.display.text.contains("nowhere.png"), "source shown");
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("editor-image-0").is_some());
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
    cx.run_until_parked();
    let text = doc(&editor, cx);
    assert!(
        text.starts_with(
            "start

![image.png](<my note-assets/"
        ) && text.ends_with(
            ">)
"
        ),
        "{text}"
    );
    assert_eq!(
        selection(&editor, cx),
        [(text.len(), text.len())],
        "the caret is on the blank line after the image"
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
    editor.update_in(cx, |editor, window, cx| {
        editor.select(Selection::single(24, 28), cx);
        editor.run_menu_action(super::menu::MenuAction::Command("format.bold"), window, cx);
    });
    assert_eq!(doc(&editor, cx), "see [site](https://a.b) **here**\n");
    assert!(editor.read_with(cx, |e, _| e.menu.is_none()));
}

mod tables {
    use super::*;
    use crate::editor::layout::BlockKind;

    /// Header, a link row, a row with an empty cell.
    const DOC: &str = "intro\n\n| Link | N |\n|:--|--:|\n| [site](https://example.test) | 2 |\n| alpha beta | |\n\nend";

    fn pos(needle: &str) -> usize {
        DOC.find(needle).unwrap()
    }

    /// The last frame's bounds of cell `row`-`column` (row 0 is the header).
    fn cell(
        editor: &Entity<Editor>,
        cx: &mut VisualTestContext,
        row: usize,
        column: usize,
    ) -> gpui::Bounds<Pixels> {
        let name = format!("table-cell-{row}-{column}");
        editor.read_with(cx, |editor, _| {
            editor
                .frames
                .last()
                .and_then(|f| f.cells.iter().find(|(n, _)| *n == name).map(|(_, b)| *b))
                .unwrap_or_else(|| panic!("{name} not drawn"))
        })
    }

    fn table_drawn(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> bool {
        editor.update_in(cx, |editor, window, cx| {
            let line = editor.layout_line(2, window, cx);
            matches!(
                line.block.as_ref().map(|b| &b.kind),
                Some(BlockKind::Table(_))
            )
        })
    }

    fn click(cx: &mut VisualTestContext, at: Point<Pixels>, clicks: usize, shift: bool) {
        press(cx, at, clicks, shift);
        release(cx, at);
        cx.run_until_parked();
    }

    fn last_caret(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> gpui::Bounds<Pixels> {
        editor.read_with(cx, |editor, _| {
            *editor
                .frames
                .last()
                .and_then(|f| f.carets.last())
                .expect("a caret drawn")
        })
    }

    /// A row with a cell past the header: the grid while untouched, then a
    /// click in a cell reveals the source, so the extra cell shows.
    #[gpui::test]
    fn a_click_in_a_table_with_an_extra_cell_reveals_its_source(cx: &mut TestAppContext) {
        let src = "intro\n\n| a | b |\n|---|---|\n| 1 | 2 | extra |\n\nend";
        let (editor, cx) = open(cx, src);
        set_cursor(&editor, cx, src.len());
        assert!(table_drawn(&editor, cx));
        let one = cell(&editor, cx, 1, 0);
        click(cx, one.center(), 1, false);
        assert!(!table_drawn(&editor, cx), "the source is shown");
        assert_eq!(doc(&editor, cx), src);
        let head = selection(&editor, cx)[0].1;
        assert!((src.find("| 1").unwrap()..src.find("extra").unwrap()).contains(&head));
    }

    /// The table's layout is cached per state; a palette change that keeps
    /// the mode (a new accent) still invalidates it, since the cells' runs
    /// carry their colors.
    #[gpui::test]
    fn a_theme_change_in_the_same_mode_lays_the_table_out_again(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let table =
            |editor: &mut Editor, window: &mut gpui::Window, cx: &mut gpui::Context<Editor>| {
                match editor.layout_line(2, window, cx).block.map(|b| b.kind) {
                    Some(BlockKind::Table(table)) => table,
                    other => panic!("not a table: {other:?}"),
                }
            };
        let (first, again) = editor.update_in(cx, |editor, window, cx| {
            (table(editor, window, cx), table(editor, window, cx))
        });
        assert!(std::rc::Rc::ptr_eq(&first, &again), "one layout per state");
        let mut palette = super::super::theme::palette();
        palette.accent = gpui::rgb(0x00aa00).into();
        super::super::theme::set(palette);
        let recolored = editor.update_in(cx, |editor, window, cx| table(editor, window, cx));
        assert!(
            !std::rc::Rc::ptr_eq(&first, &recolored),
            "the links take the new accent"
        );
    }

    /// US-049: the editor lays the table out in the frame (no measuring
    /// frame), its rows on the first line, the others folded away; cells
    /// are named by row and column.
    #[gpui::test]
    fn a_table_is_laid_out_by_the_editor_in_its_frame(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let (first, rest) = editor.update_in(cx, |editor, window, cx| {
            let first = editor.layout_line(2, window, cx);
            let rest: Vec<f32> = (3..6)
                .map(|ix| editor.layout_line(ix, window, cx).height)
                .collect();
            (first, rest)
        });
        let block = first.block.as_ref().expect("table block");
        let BlockKind::Table(table) = &block.kind else {
            panic!("not a table: {:?}", block.kind);
        };
        assert!(block.replaces);
        assert_eq!(rest, vec![0.0, 0.0, 0.0], "the other rows are folded away");
        assert_eq!(first.height, table.height, "the height is the layout's");
        assert_eq!(table.rows.len(), 3);
        let header = cell(&editor, cx, 0, 0);
        let body = cell(&editor, cx, 1, 0);
        assert!(body.top() >= header.bottom() - px(1.0));
        // Right-aligned column: its text ends at the cell's right padding.
        let n = table
            .cells
            .iter()
            .find(|c| c.row == 0 && c.column == 1)
            .unwrap();
        let segment = &n.segments[0];
        let right = segment.text_left + segment.wrapped.as_ref().unwrap().width().as_f32();
        let (x, w) = table.columns[1];
        let em = editor.read_with(cx, |e, _| e.config.text_size());
        let inner_right = x + w
            - cerata_markdown::table::BORDER_WIDTH
            - cerata_markdown::table::CELL_HORIZONTAL_PADDING_EM * em;
        assert!(
            (inner_right - right).abs() < 1.5,
            "{right} vs {inner_right}"
        );
        // The empty cell is drawn and holds an insertion point.
        let empty = table
            .cells
            .iter()
            .find(|c| c.row == 2 && c.column == 1)
            .unwrap();
        assert!(empty.content.is_empty() && !empty.missing);
    }

    /// US-048: the keystroke that wraps a cell to a second row shows the
    /// taller row in the first frame drawn after it, with the layout's
    /// height: there is no measuring frame.
    #[gpui::test]
    fn a_wrapping_keystroke_grows_the_row_in_the_next_frame(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, pos("alpha beta") + 10);
        let row_height = cell(&editor, cx, 2, 0).size.height;
        let mut grown = None;
        for step in 0..120 {
            let before = editor.read_with(cx, |e, _| e.frames.len());
            cx.simulate_input(if step % 2 == 0 { "word" } else { " " });
            let first = editor.read_with(cx, |e, _| {
                assert!(e.frames.len() > before, "the keystroke drew a frame");
                let frame = &e.frames[before];
                frame
                    .cells
                    .iter()
                    .find(|(n, _)| n == "table-cell-2-0")
                    .map(|(_, b)| *b)
                    .expect("the cell drawn")
            });
            if first.size.height > row_height {
                grown = Some(first);
                break;
            }
        }
        let first = grown.expect("the cell wrapped");
        assert!(table_drawn(&editor, cx));
        let (block_height, table) = editor.update_in(cx, |editor, window, cx| {
            let line = editor.layout_line(2, window, cx);
            let Some(BlockKind::Table(table)) = line.block.map(|b| b.kind) else {
                panic!("not a table");
            };
            (line.height, table)
        });
        let (_, layout_row) = table.rows[2];
        assert!(
            (first.size.height.as_f32() - layout_row).abs() < 0.5,
            "{first:?} vs {layout_row}"
        );
        assert_eq!(
            block_height, table.height,
            "the line is as tall as the table"
        );
        let caret = last_caret(&editor, cx);
        assert!(
            first.contains(&caret.center()),
            "the caret stays in the grown cell"
        );
    }

    /// US-051: a click puts the caret in the cell, a click on a padding in
    /// the nearest cell, never a whole-table selection; the caret is drawn
    /// in the cell, one text row tall, even in an empty cell.
    #[gpui::test]
    fn clicks_put_the_caret_in_cells(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let alpha = pos("alpha");
        let at = point_at(&editor, cx, alpha + 2);
        click(cx, at, 1, false);
        assert_eq!(selection(&editor, cx), vec![(alpha + 2, alpha + 2)]);
        assert!(table_drawn(&editor, cx), "a click keeps the table drawn");
        let caret = last_caret(&editor, cx);
        let alpha_cell = cell(&editor, cx, 2, 0);
        assert!(
            alpha_cell.contains(&caret.center()),
            "{caret:?} in {alpha_cell:?}"
        );
        let row_height = editor.read_with(cx, |e, _| {
            e.config.text_size() * cerata_markdown::table::CELL_LINE_HEIGHT
        });
        assert!((caret.size.height.as_f32() - row_height).abs() < 0.5);

        // On the right padding of the header's first cell: its content's end.
        let link = cell(&editor, cx, 0, 0);
        click(cx, point(link.right() - px(3.0), link.center().y), 1, false);
        assert_eq!(
            selection(&editor, cx),
            vec![(pos("Link") + 4, pos("Link") + 4)]
        );

        // In the empty cell: its insertion point, the caret drawn there.
        let empty = cell(&editor, cx, 2, 1);
        click(cx, empty.center(), 1, false);
        let point_in = DOC.rfind("| |").unwrap() + 2;
        assert_eq!(selection(&editor, cx), vec![(point_in, point_in)]);
        let caret = last_caret(&editor, cx);
        assert!(empty.contains(&caret.center()));
        assert!((caret.size.height.as_f32() - row_height).abs() < 0.5);
        assert!(table_drawn(&editor, cx));
    }

    /// US-051: a link in a cell opens and the caret stays; a double click
    /// takes a word of the cell, a triple click the table, whose source
    /// then shows.
    #[gpui::test]
    fn links_words_and_the_whole_table(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let link = cell(&editor, cx, 1, 0);
        click(cx, point(link.left() + px(16.0), link.center().y), 1, false);
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.test"));
        assert_eq!(selection(&editor, cx), vec![(DOC.len(), DOC.len())]);

        let beta = pos("beta");
        let at = point_at(&editor, cx, beta + 1);
        press(cx, at, 1, false);
        release(cx, at);
        press(cx, at, 2, false);
        release(cx, at);
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(beta, beta + 4)]);
        assert!(table_drawn(&editor, cx));

        press(cx, at, 3, false);
        release(cx, at);
        cx.run_until_parked();
        let table = pos("| Link")..pos("\n\nend");
        assert_eq!(selection(&editor, cx), vec![(table.start, table.end)]);
        let revealed = editor.update_in(cx, |editor, window, cx| {
            (2..6)
                .map(|ix| editor.layout_line(ix, window, cx).table_source.is_some())
                .collect::<Vec<_>>()
        });
        assert_eq!(revealed, vec![true; 4]);
    }

    /// US-051: dragging in a cell selects its text with the usual rects;
    /// Shift+click extends in the cell; a drag out of the table crosses its
    /// edge and shows the source on release.
    #[gpui::test]
    fn drags_select_in_the_cell_or_across_the_edge(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let alpha = pos("alpha");
        let from = point_at(&editor, cx, alpha);
        let to = point_at(&editor, cx, alpha + 5);
        press(cx, from, 1, false);
        drag_to(cx, to, true);
        release(cx, to);
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(alpha, alpha + 5)]);
        assert!(table_drawn(&editor, cx));
        let rects = editor.read_with(cx, |e, _| e.frames.last().unwrap().selections.clone());
        let alpha_cell = cell(&editor, cx, 2, 0);
        assert!(!rects.is_empty() && rects.iter().all(|r| alpha_cell.contains(&r.center())));

        let extend = point_at(&editor, cx, alpha + 8);
        click(cx, extend, 1, true);
        assert_eq!(selection(&editor, cx), vec![(alpha, alpha + 8)]);

        // Out of the table, below it: the source is revealed on release.
        let end = point_at(&editor, cx, DOC.len());
        press(cx, from, 1, false);
        drag_to(cx, end, true);
        assert!(table_drawn(&editor, cx), "frozen while dragging");
        release(cx, end);
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(alpha, DOC.len())]);
        assert!(!table_drawn(&editor, cx));
    }

    /// US-051 and ADR 0007: typed `|` is escaped, the IME's marked text
    /// shows in the cell and its window follows the cell's caret.
    #[gpui::test]
    fn typing_and_composing_in_a_cell(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        let alpha = pos("alpha");
        set_cursor(&editor, cx, alpha + 5);
        cx.simulate_input("|x");
        assert!(doc(&editor, cx).contains("| alpha\\|x beta | |"));
        assert!(table_drawn(&editor, cx));
        let caret_pos = alpha + 8;
        assert_eq!(selection(&editor, cx), vec![(caret_pos, caret_pos)]);
        editor.update_in(cx, |editor, window, cx| {
            editor.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
        });
        cx.run_until_parked();
        assert!(doc(&editor, cx).contains("alpha\\|xにほ beta"));
        let alpha_cell = cell(&editor, cx, 2, 0);
        let bounds = editor.update_in(cx, |editor, window, cx| {
            let utf16 = super::super::input::to_utf16(editor.state.text(), caret_pos);
            editor
                .bounds_for_range(utf16..utf16 + 2, Default::default(), window, cx)
                .expect("bounds")
        });
        assert!(
            alpha_cell.contains(&bounds.origin),
            "{bounds:?} in {alpha_cell:?}"
        );
        // Enter commits the composition and stays in the cell.
        cx.simulate_keystrokes("enter");
        let text = doc(&editor, cx);
        assert!(text.contains("alpha\\|xにほ beta"), "{text}");
        assert_eq!(selection(&editor, cx), vec![(caret_pos + 6, caret_pos + 6)]);
        assert!(editor.read_with(cx, |e, _| e.marked_range.is_none()));
        // Tab likewise: committed in place, no move to the next cell.
        editor.update_in(cx, |editor, window, cx| {
            editor.replace_and_mark_text_in_range(None, "ん", None, window, cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        let text = doc(&editor, cx);
        assert!(text.contains("alpha\\|xにほん beta"), "{text}");
        assert_eq!(selection(&editor, cx), vec![(caret_pos + 9, caret_pos + 9)]);
        assert!(editor.read_with(cx, |e, _| e.marked_range.is_none()));
    }

    /// US-052: Tab, Shift+Tab and Enter go from cell to cell, selecting
    /// them; Tab in the last cell adds a row; Shift+Enter writes `<br>`.
    #[gpui::test]
    fn tab_and_enter_walk_the_cells(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, pos("Link") + 1);
        cx.simulate_keystrokes("tab");
        assert_eq!(selection(&editor, cx), vec![(pos("N |"), pos("N |") + 1)]);
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(selection(&editor, cx), vec![(pos("Link"), pos("Link") + 4)]);
        cx.simulate_keystrokes("enter");
        let site = pos("[site]");
        assert_eq!(selection(&editor, cx), vec![(site, pos(" | 2"))]);
        let empty = DOC.rfind("| |").unwrap() + 2;
        set_cursor(&editor, cx, empty);
        cx.simulate_keystrokes("tab");
        let text = doc(&editor, cx);
        assert!(
            text.contains("| alpha beta | |\n|            |  |\n\nend"),
            "{text:?}"
        );
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), DOC);
        set_cursor(&editor, cx, pos("alpha") + 5);
        cx.simulate_keystrokes("shift-enter");
        assert!(doc(&editor, cx).contains("| alpha<br> beta |"));
        assert!(table_drawn(&editor, cx));
    }

    /// US-052: Up and Down keep the pixel column from cell to cell, leave
    /// the table by its edges and enter it from the lines around without
    /// revealing it; Home, End and Ctrl+A stay in the cell.
    #[gpui::test]
    fn arrows_home_end_and_select_all_in_cells(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, pos("N |") + 1);
        cx.simulate_keystrokes("down");
        assert_eq!(
            selection(&editor, cx),
            vec![(pos(" 2 |") + 2, pos(" 2 |") + 2)]
        );
        cx.simulate_keystrokes("down");
        let empty = DOC.rfind("| |").unwrap() + 2;
        assert_eq!(selection(&editor, cx), vec![(empty, empty)]);
        assert!(table_drawn(&editor, cx));
        // Down from the last row: the line below the table.
        cx.simulate_keystrokes("down");
        let below = pos("\n\nend") + 1;
        assert_eq!(selection(&editor, cx), vec![(below, below)]);
        // Up from the blank line below: into the last row, source hidden.
        cx.simulate_keystrokes("up");
        let (_, head) = selection(&editor, cx)[0];
        assert!(
            cerata_markdown::cells::cell_at(&editor.read_with(cx, |e, _| e.state.clone()), head)
                .is_some()
        );
        assert!(table_drawn(&editor, cx));
        // Up from the header: out by the top.
        set_cursor(&editor, cx, pos("Link") + 2);
        editor.update(cx, |e, _| e.goal_x = None);
        cx.simulate_keystrokes("up");
        assert_eq!(selection(&editor, cx), vec![(6, 6)]);
        cx.simulate_keystrokes("down");
        let (_, head) = selection(&editor, cx)[0];
        assert!((pos("Link")..=pos("Link") + 4).contains(&head), "{head}");
        assert!(table_drawn(&editor, cx));

        let alpha = pos("alpha");
        set_cursor(&editor, cx, alpha + 3);
        cx.simulate_keystrokes("end");
        assert_eq!(selection(&editor, cx), vec![(alpha + 10, alpha + 10)]);
        cx.simulate_keystrokes("end");
        assert_eq!(selection(&editor, cx), vec![(alpha + 10, alpha + 10)]);
        cx.simulate_keystrokes("home");
        assert_eq!(selection(&editor, cx), vec![(alpha, alpha)]);
        cx.simulate_keystrokes(&secondary("a"));
        assert_eq!(selection(&editor, cx), vec![(alpha, alpha + 10)]);
        cx.simulate_keystrokes(&secondary("a"));
        assert_eq!(selection(&editor, cx), vec![(0, DOC.len())]);
        // Left from a content's start: the previous content's end.
        set_cursor(&editor, cx, pos("2 |"));
        cx.simulate_keystrokes("left");
        let site_end = pos(" | 2");
        assert_eq!(selection(&editor, cx), vec![(site_end, site_end)]);
    }

    /// US-053, review: the Table submenu works with the pointer. Hovering
    /// "Table" opens it, a click on one of its items (outside the menu's
    /// own bounds) runs the command instead of closing the menu.
    #[gpui::test]
    fn a_click_in_the_table_submenu_runs_its_command(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        let two = pos(" 2 |") + 1;
        let at = point_at(&editor, cx, two);
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        let table = cx.debug_bounds("submenu-Table").expect("the Table row");
        cx.simulate_mouse_move(table.center(), None, Modifiers::default());
        cx.run_until_parked();
        let item = cx
            .debug_bounds("menu-Insert row below")
            .expect("the submenu is open");
        assert!(!table.contains(&item.center()));
        cx.simulate_mouse_move(item.center(), None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(item.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(editor.read_with(cx, |e, _| e.menu.is_none()));
        let text = doc(&editor, cx);
        assert!(
            text.contains("| 2 |\n|                              |   |\n| alpha"),
            "{text:?}"
        );
        // A press outside both lists still closes the menu.
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        let table = cx.debug_bounds("submenu-Table").expect("the Table row");
        cx.simulate_mouse_move(table.center(), None, Modifiers::default());
        cx.run_until_parked();
        let end = point_at(&editor, cx, DOC.len());
        cx.simulate_mouse_move(end, None, Modifiers::default());
        cx.simulate_click(end, Modifiers::default());
        cx.run_until_parked();
        assert!(editor.read_with(cx, |e, _| e.menu.is_none()));
    }

    /// US-053: a right click in a cell offers the Table submenu; its row
    /// command edits the table, keeps it drawn with the caret in the new
    /// row, and one undo puts the text and the caret back.
    #[gpui::test]
    fn the_context_menu_restructures_the_table(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let two = pos(" 2 |") + 1;
        let at = point_at(&editor, cx, two);
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        let before = selection(&editor, cx);
        assert_eq!(before, vec![(two, two)]);
        let action = editor.read_with(cx, |e, _| {
            let menu = e.menu.as_ref().expect("menu open");
            let table = menu
                .items
                .iter()
                .find_map(|i| match i {
                    super::super::menu::MenuItem::Submenu { label, items }
                        if label.as_ref() == "Table" =>
                    {
                        Some(items.clone())
                    }
                    _ => None,
                })
                .expect("a Table submenu");
            table
                .into_iter()
                .find_map(|i| match i {
                    super::super::menu::MenuItem::Action {
                        label,
                        enabled: true,
                        action,
                        ..
                    } if label.as_ref() == "Insert row below" => Some(action),
                    _ => None,
                })
                .expect("Insert row below, enabled")
        });
        editor.update_in(cx, |e, window, cx| e.run_menu_action(action, window, cx));
        cx.run_until_parked();
        let text = doc(&editor, cx);
        assert!(
            text.contains(
                "| [site](https://example.test) | 2 |
|                              |   |
| alpha"
            ),
            "{text:?}"
        );
        assert!(table_drawn(&editor, cx));
        let (_, head) = selection(&editor, cx)[0];
        let state = editor.read_with(cx, |e, _| e.state.clone());
        let (_, row, column) = cerata_markdown::cells::cell_at(&state, head).expect("in a cell");
        assert_eq!((row, column), (3, 1));
        assert_eq!(table_at(&editor, cx, 2).rows.len(), 4);
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), DOC);
        assert_eq!(selection(&editor, cx), before);
    }

    /// Eight columns of long words: wider than any text column.
    fn wide_doc() -> String {
        let header: Vec<String> = (0..8).map(|c| format!("Heading{c}")).collect();
        let body: Vec<String> = (0..8)
            .map(|c| format!("Unbreakablewordnumber{c}xxxxxxxx"))
            .collect();
        format!(
            "intro\n\n| {} |\n|{}\n| {} |\n\nend",
            header.join(" | "),
            "---|".repeat(8),
            body.join(" | ")
        )
    }

    fn table_offset(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> f32 {
        editor.read_with(cx, |e, _| e.code_scroll.offset_of_line(2))
    }

    /// The text column's horizontal bounds.
    fn column(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> (Pixels, Pixels) {
        editor.read_with(cx, |e, _| {
            let v = e.viewport.expect("painted");
            (v.column_left, v.column_left + px(v.column_width))
        })
    }

    /// US-056: a table wider than the text column scrolls horizontally with
    /// a thin bar (Shift+wheel arrives as a horizontal delta); the caret
    /// entering a hidden cell or typing there brings it into sight, and the
    /// offset holds from one keystroke to the next.
    #[gpui::test]
    fn a_wide_table_scrolls_and_follows_the_caret(cx: &mut TestAppContext) {
        let src = wide_doc();
        let (editor, cx) = open(cx, &src);
        set_cursor(&editor, cx, src.len());
        let table = table_at(&editor, cx, 2);
        let (left, right) = column(&editor, cx);
        assert!(
            table.width > (right - left).as_f32(),
            "wider than the column"
        );
        let thumbs = editor.read_with(cx, |e, _| e.thumbs.clone());
        assert_eq!(thumbs.len(), 1, "a thin bar");
        assert_eq!(thumbs[0].first_line, 2);
        assert!(thumbs[0].bounds.size.height <= px(6.0));
        let first = cell(&editor, cx, 1, 0);
        cx.simulate_event(ScrollEvent {
            position: first.center(),
            delta: ScrollDelta::Pixels(point(px(-120.0), px(0.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(table_offset(&editor, cx), 120.0);
        assert_eq!(cell(&editor, cx, 1, 0).left(), first.left() - px(120.0));
        assert!(table_drawn(&editor, cx));
        // The caret in the last cell, out of sight: the table follows it.
        let last = src.find("Unbreakablewordnumber7").unwrap() + 3;
        set_cursor(&editor, cx, last);
        let caret = last_caret(&editor, cx);
        assert!(
            caret.left() >= left && caret.right() <= right,
            "{caret:?} in {left:?}..{right:?}"
        );
        let offset = table_offset(&editor, cx);
        assert!(offset > 120.0);
        cx.simulate_input("ab");
        let typed = table_offset(&editor, cx);
        assert!(typed >= offset, "kept while typing");
        let caret = last_caret(&editor, cx);
        assert!(caret.left() >= left && caret.right() <= right);
        // Back to the first cell: the table scrolls back.
        set_cursor(&editor, cx, src.find("Unbreakablewordnumber0").unwrap());
        assert!(table_offset(&editor, cx) < typed);
        let caret = last_caret(&editor, cx);
        assert!(caret.left() >= left);
    }

    /// US-056: search marks its results in the cells; going to one selects
    /// it in the cell, keeps the table drawn and scrolls the table to it.
    /// A result across a `|` reveals the source.
    #[gpui::test]
    fn search_finds_text_in_cells(cx: &mut TestAppContext) {
        let src = wide_doc();
        let (editor, cx) = open(cx, &src);
        set_cursor(&editor, cx, 0);
        cx.simulate_keystrokes(&secondary("f"));
        cx.simulate_input("number7");
        cx.simulate_keystrokes("enter");
        let at = src.find("number7").unwrap();
        assert_eq!(selection(&editor, cx), [(at, at + 7)]);
        assert!(table_drawn(&editor, cx), "the source stays hidden");
        let (left, right) = column(&editor, cx);
        let marks = editor.read_with(cx, |e, _| e.frames.last().unwrap().matches.clone());
        let hidden = cell(&editor, cx, 1, 7);
        assert!(!marks.is_empty());
        assert!(
            marks.iter().all(|(r, _)| hidden.contains(&r.center())
                && r.left() >= left
                && r.right() <= right),
            "{marks:?} in {hidden:?}"
        );
        assert!(table_offset(&editor, cx) > 0.0, "scrolled to the result");
        // Across a pipe: selected as is, the source shows.
        cx.simulate_keystrokes("escape");
        set_cursor(&editor, cx, 0);
        cx.simulate_keystrokes(&secondary("f"));
        cx.simulate_keystrokes(&secondary("a"));
        cx.simulate_input("xx | Unb");
        cx.simulate_keystrokes("enter");
        let across = src.find("xx | Unb").unwrap();
        assert_eq!(selection(&editor, cx), [(across, across + 8)]);
        assert!(!table_drawn(&editor, cx));
    }

    /// US-056 review: a result to the left of a table scrolled right comes
    /// into sight whole, its start included, not just up to its end.
    #[gpui::test]
    fn a_result_left_of_the_scrolled_table_shows_whole(cx: &mut TestAppContext) {
        // A second row whose first cell wraps: the result starts its
        // second visual row.
        let mut src = wide_doc();
        let row: Vec<&str> = (0..8).map(|_| "x").collect();
        let at_row = src
            .find(
                "

end",
            )
            .unwrap();
        src.insert_str(
            at_row,
            &format!(
                "
| short Unbreakablewordnumber9xxxxxxxx | {} |",
                row[1..].join(" | ")
            ),
        );
        let (editor, cx) = open(cx, &src);
        set_cursor(&editor, cx, src.find("Unbreakablewordnumber7").unwrap() + 3);
        assert!(table_offset(&editor, cx) > 0.0);
        let wrapped = table_at(&editor, cx, 2)
            .cells
            .iter()
            .find(|c| c.row == 2 && c.column == 0)
            .map(|c| c.segments[0].row_starts.len());
        assert_eq!(wrapped, Some(2), "the cell wraps before the result");
        // The caret leaves the table, which stays scrolled; the query is
        // typed a letter at a time.
        set_cursor(&editor, cx, 0);
        assert!(table_offset(&editor, cx) > 0.0);
        cx.simulate_keystrokes(&secondary("f"));
        for c in "Unbreakablewordnumber9".chars() {
            cx.simulate_input(&c.to_string());
        }
        cx.simulate_keystrokes("enter");
        let at = src.find("Unbreakablewordnumber9").unwrap();
        assert_eq!(selection(&editor, cx), [(at, at + 22)]);
        let (left, right) = column(&editor, cx);
        let marks = editor.read_with(cx, |e, _| e.frames.last().unwrap().matches.clone());
        assert!(!marks.is_empty());
        assert!(
            marks
                .iter()
                .all(|(r, _)| r.left() >= left && r.right() <= right),
            "{marks:?} in {left:?}..{right:?}"
        );
        // A caret at that wrap boundary, the table scrolled right again.
        cx.simulate_keystrokes("escape");
        set_cursor(&editor, cx, src.find("Unbreakablewordnumber7").unwrap() + 3);
        set_cursor(&editor, cx, at);
        let caret = last_caret(&editor, cx);
        assert!(
            caret.left() >= left && caret.right() <= right,
            "{caret:?} in {left:?}..{right:?}"
        );
    }

    /// US-056: replacing a result in a cell escapes the replacement's `|`
    /// and line breaks, so the row keeps its cells.
    #[gpui::test]
    fn replacing_in_a_cell_escapes_the_replacement(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, 0);
        cx.simulate_keystrokes(&secondary("f"));
        cx.simulate_input("beta");
        cx.simulate_keystrokes("enter");
        let field = cx
            .debug_bounds("text-field-Replace")
            .expect("replace field drawn");
        press(cx, field.center(), 1, false);
        release(cx, field.center());
        cx.run_until_parked();
        cx.simulate_input("a|b");
        cx.simulate_keystrokes("enter");
        let text = doc(&editor, cx);
        assert!(text.contains("| alpha a\\|b | |"), "{text:?}");
    }

    /// US-056: "Edit as Markdown" shows the table's source until the
    /// selection leaves it; then the table is drawn again.
    #[gpui::test]
    fn edit_as_markdown_reveals_the_source_until_the_selection_leaves(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, pos("alpha") + 2);
        editor.update_in(cx, |e, window, cx| {
            e.run_menu_action(
                super::super::menu::MenuAction::Command("table.editSource"),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(!table_drawn(&editor, cx), "the source shows");
        // The caret moves freely in the source, a pipe included.
        let pipe = pos("| alpha");
        set_cursor(&editor, cx, pipe);
        assert_eq!(selection(&editor, cx), [(pipe, pipe)]);
        assert!(!table_drawn(&editor, cx));
        cx.simulate_input("|");
        assert!(doc(&editor, cx).contains("|| alpha"), "typed as source");
        cx.simulate_keystrokes(&secondary("z"));
        set_cursor(&editor, cx, DOC.len());
        assert!(table_drawn(&editor, cx), "drawn once the selection left");
        set_cursor(&editor, cx, pos("alpha"));
        assert!(
            table_drawn(&editor, cx),
            "and stays drawn when it comes back"
        );
    }

    fn rect(
        editor: &Entity<Editor>,
        cx: &mut VisualTestContext,
    ) -> Option<((usize, usize), (usize, usize))> {
        editor.read_with(cx, |e, _| {
            e.state.table_selection().map(|ts| (ts.anchor, ts.head))
        })
    }

    /// US-054: a drag from one cell to another selects the rectangle
    /// between them, painted as cell backgrounds with the table drawn;
    /// Shift+click extends it from the caret's cell; a drag out of the
    /// table is a text selection that shows the source on release.
    #[gpui::test]
    fn dragging_across_cells_selects_a_rectangle(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let link = cell(&editor, cx, 0, 0);
        let two = cell(&editor, cx, 1, 1);
        press(cx, link.center(), 1, false);
        drag_to(cx, two.center(), true);
        assert_eq!(rect(&editor, cx), Some(((0, 0), (2, 1))));
        release(cx, two.center());
        cx.run_until_parked();
        assert_eq!(rect(&editor, cx), Some(((0, 0), (2, 1))));
        assert!(table_drawn(&editor, cx));
        let ranges: Vec<String> = editor.read_with(cx, |e, _| {
            e.state
                .selection()
                .ranges()
                .iter()
                .map(|r| e.state.doc()[r.range()].to_owned())
                .collect()
        });
        assert_eq!(ranges, ["Link", "N", "[site](https://example.test)", "2"]);
        let painted = editor.read_with(cx, |e, _| e.frames.last().unwrap().selections.clone());
        assert_eq!(painted.len(), 4, "one background per cell");
        for (r, c) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let b = cell(&editor, cx, r, c);
            assert!(painted.iter().any(|p| b.contains(&p.center())), "{r}-{c}");
        }
        // Shift+click from the caret's cell.
        set_cursor(&editor, cx, pos("alpha"));
        let empty = cell(&editor, cx, 2, 1);
        click(cx, empty.center(), 1, true);
        assert_eq!(rect(&editor, cx), Some(((3, 0), (3, 1))));
        // Out of the table: a text selection; the source shows on release.
        let alpha = point_at(&editor, cx, pos("alpha"));
        let end = point_at(&editor, cx, DOC.len());
        press(cx, alpha, 1, false);
        let into = cell(&editor, cx, 2, 1).center();
        drag_to(cx, into, true);
        assert!(rect(&editor, cx).is_some());
        drag_to(cx, end, true);
        assert!(rect(&editor, cx).is_none());
        release(cx, end);
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), [(pos("alpha"), DOC.len())]);
        assert!(!table_drawn(&editor, cx));
    }

    /// US-054: Shift+arrows at a content's edge grow a rectangle cell by
    /// cell; formatting applies to each non-empty cell and Backspace empties
    /// them, one undo step each; a plain arrow collapses it; typing replaces
    /// the anchor cell.
    #[gpui::test]
    fn the_keyboard_drives_a_rectangle(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, pos("Link") + 4);
        cx.simulate_keystrokes("shift-right");
        assert_eq!(rect(&editor, cx), Some(((0, 0), (0, 1))));
        cx.simulate_keystrokes("shift-down");
        assert_eq!(rect(&editor, cx), Some(((0, 0), (2, 1))));
        assert!(table_drawn(&editor, cx));
        cx.simulate_keystrokes(&secondary("b"));
        let text = doc(&editor, cx);
        assert!(text.contains("| **Link** | **N** |"), "{text:?}");
        assert!(
            text.contains("| **[site](https://example.test)** | **2** |"),
            "{text:?}"
        );
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), DOC);
        // The rectangle again, then Backspace.
        set_cursor(&editor, cx, pos("Link") + 4);
        cx.simulate_keystrokes("shift-right shift-down");
        cx.simulate_keystrokes("backspace");
        let text = doc(&editor, cx);
        assert!(
            text.contains("|  |  |\n|:--|--:|\n|  |  |\n| alpha beta | |"),
            "{text:?}"
        );
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), DOC);
        // Left collapses to the first cell, Right to the last.
        set_cursor(&editor, cx, pos("Link") + 4);
        cx.simulate_keystrokes("shift-right shift-down");
        cx.simulate_keystrokes("left");
        assert_eq!(selection(&editor, cx), [(pos("Link"), pos("Link"))]);
        assert!(rect(&editor, cx).is_none());
        set_cursor(&editor, cx, pos("Link") + 4);
        cx.simulate_keystrokes("shift-right shift-down");
        cx.simulate_keystrokes("down");
        assert_eq!(selection(&editor, cx), [(pos(" 2 |") + 2, pos(" 2 |") + 2)]);
        // Typing over it: the anchor cell's content goes.
        set_cursor(&editor, cx, pos("Link") + 4);
        cx.simulate_keystrokes("shift-right shift-down");
        cx.simulate_input("H");
        let text = doc(&editor, cx);
        assert!(text.contains("| H | N |"), "{text:?}");
        assert_eq!(selection(&editor, cx), [(pos("Link") + 1, pos("Link") + 1)]);
        assert!(rect(&editor, cx).is_none());
    }

    const TWO: &str = "| a | b |\n|:--|--:|\n| 1 | 2 |\n| 3 | 4 |\n\nmid\n\n| x | y |\n|---|---|\n| p | q |\n\nend";

    fn in_two(needle: &str) -> usize {
        TWO.find(needle).unwrap()
    }

    /// US-055: a rectangle copies as a GFM table with the grid as metadata
    /// and pastes into another table from the caret's cell, adding the rows
    /// it needs, in one undo step; out of a table it pastes as Markdown;
    /// cut empties the cells in one step.
    #[gpui::test]
    fn cells_copy_cut_and_paste_between_tables(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, TWO);
        set_cursor(&editor, cx, in_two("1 |") + 1);
        cx.simulate_keystrokes("shift-right shift-down");
        cx.simulate_keystrokes(&secondary("c"));
        let item = cx.read_from_clipboard().expect("copied");
        assert_eq!(
            item.text().as_deref(),
            Some("| 1 | 2 |\n| :-- | --: |\n| 3 | 4 |")
        );
        assert!(
            item.metadata()
                .is_some_and(|m| m.contains("cerata.table-cells"))
        );
        // Into the second table's last cell: one row added.
        set_cursor(&editor, cx, in_two("q |"));
        cx.simulate_keystrokes(&secondary("v"));
        let text = doc(&editor, cx);
        assert!(
            text.ends_with("| x | y |   |\n|---|---| --- |\n| p | 1 | 2 |\n|   | 3 | 4 |\n\nend"),
            "{text:?}"
        );
        let drawn = editor.update_in(cx, |e, window, cx| {
            matches!(
                e.layout_line(7, window, cx).block.map(|b| b.kind),
                Some(BlockKind::Table(_))
            )
        });
        assert!(drawn, "the second table stays drawn");
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), TWO, "one step");
        // Out of a table: the Markdown as is.
        set_cursor(&editor, cx, in_two("mid") + 3);
        cx.simulate_keystrokes(&secondary("v"));
        assert!(doc(&editor, cx).contains("mid| 1 | 2 |\n| :-- | --: |\n| 3 | 4 |\n"));
        cx.simulate_keystrokes(&secondary("z"));
        // Cut: copied, then emptied in one step.
        set_cursor(&editor, cx, in_two("1 |") + 1);
        cx.simulate_keystrokes("shift-right");
        cx.simulate_keystrokes(&secondary("x"));
        assert!(doc(&editor, cx).starts_with("| a | b |\n|:--|--:|\n|  |  |\n"));
        assert_eq!(
            cx.read_from_clipboard().and_then(|i| i.text()).as_deref(),
            Some("| 1 | 2 |\n| :-- | --: |")
        );
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), TWO);
    }

    /// US-055: spreadsheet TSV fills cells with its `|` and line breaks
    /// escaped; unknown or broken metadata pastes the text; a grid over
    /// the limit pastes nothing and says why.
    #[gpui::test]
    fn spreadsheet_cells_bad_metadata_and_the_limit(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, TWO);
        set_cursor(&editor, cx, in_two("p |"));
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("a|b\tc\n".into()));
        cx.simulate_keystrokes(&secondary("v"));
        assert!(
            doc(&editor, cx).contains("| a\\|b | c |"),
            "{:?}",
            doc(&editor, cx)
        );
        cx.simulate_keystrokes(&secondary("z"));
        cx.write_to_clipboard(gpui::ClipboardItem::new_string_with_metadata(
            "hello".into(),
            "{broken".into(),
        ));
        set_cursor(&editor, cx, in_two("p |") + 1);
        cx.simulate_keystrokes(&secondary("v"));
        assert!(doc(&editor, cx).contains("| phello | q |"));
        cx.simulate_keystrokes(&secondary("z"));
        let mut big = String::new();
        for _ in 0..101 {
            big.push_str(&"v\t".repeat(100));
            big.push_str("v\n");
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(big));
        cx.simulate_keystrokes(&secondary("v"));
        assert_eq!(doc(&editor, cx), TWO, "nothing pasted");
        let notice = editor.read_with(cx, |e, _| e.notice.clone());
        assert_eq!(
            notice.as_deref(),
            Some("Paste canceled: 10,201 cells are over the 10,000 limit")
        );
    }

    /// The table on line `ix` laid out afresh, every cell measured again,
    /// with the column widths the caret's table keeps.
    fn fresh_table_at(
        editor: &Entity<Editor>,
        cx: &mut VisualTestContext,
        ix: usize,
    ) -> std::rc::Rc<crate::editor::table::TableLayout> {
        editor.update(cx, |editor, _| {
            let frozen = editor.tables.frozen.take();
            editor.tables = crate::editor::TableCache {
                frozen,
                ..Default::default()
            };
        });
        table_at(editor, cx, ix)
    }

    /// What a table layout shows, as comparable values.
    fn table_geometry(t: &crate::editor::table::TableLayout) -> Vec<String> {
        let mut out = vec![format!(
            "{} {} {:?} {:?}",
            t.width, t.height, t.columns, t.rows
        )];
        for c in &t.cells {
            out.push(format!(
                "{} {} {:?} {}",
                c.row, c.column, c.content, c.missing
            ));
            for s in &c.segments {
                out.push(format!(
                    "  {} {} {} {:?} {:?} {} {} {} {:?}",
                    s.ix,
                    s.start,
                    s.len,
                    s.display.text,
                    s.row_starts,
                    s.text_left,
                    s.text_top,
                    s.height,
                    s.wrapped.as_ref().map(|w| w.text.to_string()),
                ));
            }
        }
        out
    }

    const STYLED: &str = "intro

| Name | Kind | Note |
|---|:-:|--:|
| **bold** cell | `code` | [link](https://example.com) |
| plain | *it* | a<br>b |
| last | x |

end";

    /// US-057: after an edit in a table, the cells it left alone are taken
    /// from the previous layout, and the table looks exactly as if every
    /// cell were laid out again: typing, revealing syntax, moving between
    /// cells, a new line in a cell, undo.
    #[gpui::test]
    fn reused_cells_match_a_fresh_layout(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, STYLED);
        let check = |editor: &Entity<Editor>, cx: &mut VisualTestContext, step: &str| {
            let reused = table_at(editor, cx, 2);
            let fresh = fresh_table_at(editor, cx, 2);
            assert_eq!(table_geometry(&reused), table_geometry(&fresh), "{step}");
            fresh
        };
        let plain = STYLED.find("plain").unwrap() + 5;
        set_cursor(&editor, cx, plain);
        let first = check(&editor, cx, "enter");
        cx.simulate_input("ly");
        let typed = table_at(&editor, cx, 2);
        // A cell away from the caret kept its shaped line.
        let link = |t: &crate::editor::table::TableLayout| {
            t.cells
                .iter()
                .find(|c| c.row == 1 && c.column == 2)
                .and_then(|c| c.segments[0].wrapped.clone())
                .unwrap()
        };
        assert!(std::rc::Rc::ptr_eq(&link(&first), &link(&typed)));
        check(&editor, cx, "typed");
        // Into the bold cell (its syntax revealed), then typing there.
        let bold = doc(&editor, cx).find("bold**").unwrap() + 2;
        set_cursor(&editor, cx, bold);
        check(&editor, cx, "into bold");
        cx.simulate_input("er");
        check(&editor, cx, "typed in bold");
        // To the next cell by Tab, a new line in it, then undo twice.
        cx.simulate_keystrokes("tab");
        check(&editor, cx, "tab");
        cx.simulate_keystrokes("enter");
        check(&editor, cx, "new line");
        cx.simulate_keystrokes(&secondary("z"));
        check(&editor, cx, "undo");
        cx.simulate_keystrokes(&secondary("z"));
        check(&editor, cx, "undo typing");
        assert!(table_drawn(&editor, cx));
    }

    /// The column edges (left, right) of drawn row `row` in the last frame.
    fn column_edges(
        editor: &Entity<Editor>,
        cx: &mut VisualTestContext,
        row: usize,
        columns: usize,
    ) -> Vec<(Pixels, Pixels)> {
        (0..columns)
            .map(|c| {
                let b = cell(editor, cx, row, c);
                (b.left(), b.right())
            })
            .collect()
    }

    /// The table on line `ix`, laid out for the current state.
    fn table_at(
        editor: &Entity<Editor>,
        cx: &mut VisualTestContext,
        ix: usize,
    ) -> std::rc::Rc<crate::editor::table::TableLayout> {
        editor.update_in(cx, |editor, window, cx| {
            match editor.layout_line(ix, window, cx).block.map(|b| b.kind) {
                Some(BlockKind::Table(table)) => table,
                other => panic!("not a table: {other:?}"),
            }
        })
    }

    const NARROW: &str = "intro

| Feature | N | Note |
|---|---|---|
| A long cell that wraps across several lines of text because its column reaches the cap | x | mid |

end";

    /// US-058: typing a sentence in a narrow column keeps every column's
    /// edges while the caret stays in the table: the cell wraps and only
    /// its row grows. Leaving the table sizes the columns again.
    #[gpui::test]
    fn column_widths_hold_while_typing_in_the_table(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, NARROW);
        let x = NARROW.find("x |").unwrap();
        set_cursor(&editor, cx, x + 1);
        let before = column_edges(&editor, cx, 1, 3);
        let row_before = cell(&editor, cx, 1, 1).size.height;
        for word in "a sentence typed in the narrow column".split_inclusive(' ') {
            cx.simulate_input(word);
            assert_eq!(column_edges(&editor, cx, 1, 3), before, "after {word:?}");
            assert!(table_drawn(&editor, cx));
        }
        assert!(
            cell(&editor, cx, 1, 1).size.height > row_before,
            "the cell wraps in its column"
        );
        // Out of the table: the columns are sized again, the typed column
        // now asks for more room.
        let end = doc(&editor, cx).len();
        set_cursor(&editor, cx, end);
        let after = column_edges(&editor, cx, 1, 3);
        assert_ne!(after, before);
        assert!(after[1].1 - after[1].0 > before[1].1 - before[1].0);
        // Back in the table: the widths it is entered with hold again.
        let x = doc(&editor, cx).find("xa sentence").unwrap();
        set_cursor(&editor, cx, x + 1);
        let entered = column_edges(&editor, cx, 1, 3);
        assert_eq!(entered, after);
        cx.simulate_input(" more");
        assert_eq!(column_edges(&editor, cx, 1, 3), entered);
    }

    /// US-058: a word longer than its frozen column wraps inside it without
    /// running into the next column; out of the table, the column is at
    /// least as wide as that word again (US-032).
    #[gpui::test]
    fn a_long_word_wraps_in_its_frozen_column(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, NARROW);
        let x = NARROW.find("x |").unwrap();
        set_cursor(&editor, cx, x + 1);
        let frozen = table_at(&editor, cx, 2).columns.clone();
        cx.simulate_input("Supercalifragilisticexpialidocious");
        let table = table_at(&editor, cx, 2);
        assert_eq!(table.columns, frozen);
        let n = table
            .cells
            .iter()
            .find(|c| c.row == 1 && c.column == 1)
            .unwrap();
        let (left, width) = table.columns[1];
        let line = n.segments[0].wrapped.as_ref().unwrap();
        assert!(!line.wrap_boundaries().is_empty(), "the word is cut");
        let right = n.segments[0].text_left + line.width().as_f32();
        assert!(right <= left + width, "{right} past {}", left + width);
        let end = doc(&editor, cx).len();
        set_cursor(&editor, cx, end);
        let out = table_at(&editor, cx, 2);
        let n = out
            .cells
            .iter()
            .find(|c| c.row == 1 && c.column == 1)
            .unwrap();
        assert_eq!(
            n.segments[0].rows(),
            1,
            "out of the table, the column holds the word"
        );
    }

    /// US-058: a change of text column width (or of text size) sizes the
    /// columns again, even with the caret in the table; the kept widths
    /// live in the editor only.
    #[gpui::test]
    fn a_new_column_width_sizes_the_frozen_table_again(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, NARROW);
        let x = NARROW.find("x |").unwrap();
        set_cursor(&editor, cx, x + 1);
        cx.simulate_input(" typed words");
        let wide = table_at(&editor, cx, 2).columns.clone();
        cx.simulate_resize(size(px(700.0), px(800.0)));
        cx.run_until_parked();
        let narrow = table_at(&editor, cx, 2).columns.clone();
        assert_ne!(narrow, wide);
        let mut config = editor.read_with(cx, |e, _| e.config.clone());
        config.zoom = 1.5;
        editor.update(cx, |e, cx| e.set_config(config, cx));
        cx.run_until_parked();
        assert_ne!(table_at(&editor, cx, 2).columns, narrow);
        assert!(doc(&editor, cx).contains("| x typed words |"));
    }
}

mod frontmatter {
    use super::*;
    use crate::editor::layout::BlockKind;

    fn rows(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> Vec<(String, String)> {
        editor.update(cx, |e, cx| e.frontmatter.read(cx).rows_text(cx))
    }

    fn focus_field(editor: &Entity<Editor>, cx: &mut VisualTestContext, row: usize, value: bool) {
        editor.update_in(cx, |e, window, cx| {
            let handle = e.frontmatter.read(cx).field_handle(row, value, cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
    }

    /// An active window, as focus-out events need.
    fn open_active<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<Editor>, &'a mut VisualTestContext) {
        let (editor, cx) = open(cx, text);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (editor, cx)
    }

    fn keys(cx: &mut VisualTestContext, keys: &str) {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
    }

    fn type_in(cx: &mut VisualTestContext, text: &str) {
        cx.simulate_input(text);
        cx.run_until_parked();
    }

    const DOC: &str = "---\ntitle: Old\ntags:\n- a\n- b\n---\nbody";

    /// One row per property in place of the YAML; editing a value rewrites
    /// only its entry, the list keeps its text; keys stay in the panel.
    #[gpui::test]
    fn the_panel_replaces_the_yaml_and_edits_one_entry(cx: &mut TestAppContext) {
        let (editor, cx) = open_active(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let (first, hidden) = editor.update_in(cx, |editor, window, cx| {
            let first = editor.layout_line(0, window, cx);
            let hidden: Vec<f32> = (1..6)
                .map(|ix| editor.layout_line(ix, window, cx).height)
                .collect();
            (first, hidden)
        });
        let block = first.block.as_ref().expect("panel block");
        assert_eq!(
            block.kind,
            BlockKind::Rich(cerata_markdown::Widget::Frontmatter)
        );
        assert_eq!(hidden, vec![0.0; 5]);
        assert_eq!(
            rows(&editor, cx),
            vec![
                ("title".into(), "Old".into()),
                ("tags".into(), "- a - b".into())
            ]
        );

        focus_field(&editor, cx, 0, true);
        type_in(cx, "er");
        assert_eq!(
            doc(&editor, cx),
            "---\ntitle: Older\ntags:\n- a\n- b\n---\nbody"
        );
        // Up stays in the panel: the caret in the text does not move.
        keys(cx, "up");
        assert_eq!(selection(&editor, cx), vec![(DOC.len() + 2, DOC.len() + 2)]);
    }

    /// A note opens with the caret in its body, its properties as a panel.
    #[gpui::test]
    fn a_note_opens_with_its_properties_as_a_panel(cx: &mut TestAppContext) {
        let (editor, cx) = open_active(cx, DOC);
        let body = DOC.find("body").expect("body");
        assert_eq!(selection(&editor, cx), vec![(body, body)]);
        let first = editor.update_in(cx, |editor, window, cx| editor.layout_line(0, window, cx));
        assert_eq!(
            first.block.as_ref().map(|b| &b.kind),
            Some(&BlockKind::Rich(cerata_markdown::Widget::Frontmatter))
        );
    }

    /// Enter on the last value adds a row; Backspace in an empty row
    /// removes it; a row left without a key is removed.
    #[gpui::test]
    fn rows_are_added_removed_and_reaped(cx: &mut TestAppContext) {
        let doc_text = "---\na: 1\n---\nbody";
        let (editor, cx) = open_active(cx, doc_text);
        set_cursor(&editor, cx, doc_text.len());
        cx.run_until_parked();
        focus_field(&editor, cx, 0, true);
        keys(cx, "enter");
        assert_eq!(rows(&editor, cx).len(), 2);
        assert!(editor.update_in(cx, |e, window, cx| {
            e.frontmatter
                .read(cx)
                .field_handle(1, false, cx)
                .is_focused(window)
        }));
        keys(cx, "backspace");
        assert_eq!(rows(&editor, cx).len(), 1);
        assert_eq!(doc(&editor, cx), doc_text);

        // A new row with a key is written; one left without is reaped.
        keys(cx, "enter");
        type_in(cx, "draft");
        assert_eq!(doc(&editor, cx), "---\na: 1\ndraft: \"\"\n---\nbody");
        // Enter goes from the name to the value, then adds a row.
        keys(cx, "enter");
        keys(cx, "enter");
        type_in(cx, "x");
        keys(cx, "enter");
        keys(cx, "enter");
        assert_eq!(rows(&editor, cx).len(), 4);
        editor.update_in(cx, |e, window, cx| window.focus(&e.focus_handle, cx));
        cx.run_until_parked();
        assert_eq!(rows(&editor, cx).len(), 3);
        assert_eq!(
            doc(&editor, cx),
            "---\na: 1\ndraft: \"\"\nx: \"\"\n---\nbody"
        );
    }

    /// Tab past the last field leaves the panel instead of stopping there.
    #[gpui::test]
    fn tab_leaves_the_panel_past_its_last_field(cx: &mut TestAppContext) {
        let doc_text = "---\na: 1\n---\nbody";
        let (editor, cx) = open_active(cx, doc_text);
        focus_field(&editor, cx, 0, true);
        keys(cx, "tab");
        let in_panel = editor.update_in(cx, |e, window, cx| {
            let panel = e.frontmatter.read(cx);
            [false, true]
                .iter()
                .any(|&value| panel.field_handle(0, value, cx).is_focused(window))
        });
        assert!(!in_panel, "the focus moved on");
        assert_eq!(doc(&editor, cx), doc_text);
    }

    /// Emptying the last row and pressing Backspace removes the block and
    /// gives the focus back to the text.
    #[gpui::test]
    fn removing_the_last_row_removes_the_frontmatter(cx: &mut TestAppContext) {
        let doc_text = "---\na: 1\n---\nbody";
        let (editor, cx) = open_active(cx, doc_text);
        set_cursor(&editor, cx, doc_text.len());
        cx.run_until_parked();
        focus_field(&editor, cx, 0, true);
        keys(cx, "backspace");
        keys(cx, "shift-tab");
        keys(cx, "backspace");
        assert_eq!(doc(&editor, cx), "---\n\n---\nbody");
        keys(cx, "backspace");
        assert_eq!(doc(&editor, cx), "body");
        assert!(editor.update_in(cx, |e, window, _| e.focus_handle.is_focused(window)));
    }

    /// `---` typed on the first line opens an empty panel, its name field
    /// focused.
    #[gpui::test]
    fn typing_three_dashes_on_the_first_line_opens_an_empty_panel(cx: &mut TestAppContext) {
        let (editor, cx) = open_active(cx, "--\nhello");
        set_cursor(&editor, cx, 2);
        type_in(cx, "-");
        assert_eq!(doc(&editor, cx), "---\n\n---\nhello");
        assert_eq!(rows(&editor, cx), vec![(String::new(), String::new())]);
        assert!(editor.update_in(cx, |e, window, cx| {
            e.frontmatter
                .read(cx)
                .field_handle(0, false, cx)
                .is_focused(window)
        }));
        type_in(cx, "title");
        assert_eq!(doc(&editor, cx), "---\ntitle: \"\"\n---\nhello");
    }

    /// Elsewhere `---` stays a rule.
    #[gpui::test]
    fn three_dashes_elsewhere_stay_text(cx: &mut TestAppContext) {
        let (editor, cx) = open_active(cx, "text\n--");
        set_cursor(&editor, cx, 7);
        type_in(cx, "-");
        assert_eq!(doc(&editor, cx), "text\n---");
    }

    /// An invalid YAML is shown raw with its error and never written.
    #[gpui::test]
    fn an_invalid_yaml_is_shown_raw_and_left_alone(cx: &mut TestAppContext) {
        let doc_text = "---\ntitle: [x\n---\nbody";
        let (editor, cx) = open_active(cx, doc_text);
        set_cursor(&editor, cx, doc_text.len());
        cx.run_until_parked();
        let error = editor.update(cx, |e, cx| {
            e.frontmatter.read(cx).error().map(str::to_owned)
        });
        assert!(error.is_some_and(|e| !e.is_empty()));
        assert!(rows(&editor, cx).is_empty());
        assert!(cx.debug_bounds("frontmatter-raw-text").is_some());
        assert_eq!(doc(&editor, cx), doc_text);
    }
}

mod html_blocks {
    use super::*;
    use crate::editor::layout::BlockKind;

    /// US-035: `<details>` drawn closed and opened by its summary, a link
    /// opening with the URL rules, an image outside the note's folder never
    /// loaded, and a click elsewhere revealing the source.
    #[gpui::test]
    fn html_blocks_render_toggle_and_reveal(cx: &mut TestAppContext) {
        let dir = tempfile::TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(dir.path().join("outside.png"), png(4, 4)).unwrap();
        let doc_text = "intro\n\n<details>\n<summary>More</summary>\n\n<a href=\"https://example.test\">site</a> <img src=\"../outside.png\" alt=\"out\"> <span onclick=\"x()\">safe</span>\n\n</details>\n\nend";
        let (editor, cx) = open_at(
            cx,
            doc_text,
            Some(vault.join("note.md")),
            EditorConfig::default(),
        );
        set_cursor(&editor, cx, doc_text.len());
        let line = editor.update_in(cx, |e, window, cx| e.layout_line(2, window, cx));
        let block = line.block.as_ref().expect("html block");
        assert!(matches!(
            &block.kind,
            BlockKind::Rich(cerata_markdown::Widget::Html { .. })
        ));

        let summary = cx.debug_bounds("html-summary-0").expect("summary drawn");
        assert!(cx.debug_bounds("html-details-body-0").is_none(), "closed");
        let at = point(summary.left() + px(24.0), summary.center().y);
        press(cx, at, 1, false);
        cx.run_until_parked();
        // The frame that draws it open measures it; the next one makes room:
        // the caret on the following line is painted below its body.
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let body = cx.debug_bounds("html-details-body-0").expect("opened");
        let caret = editor.read_with(cx, |e, _| {
            e.frames.last().and_then(|f| f.carets.first().copied())
        });
        let caret = caret.expect("caret painted");
        assert!(caret.top() > body.bottom(), "{caret:?} under {body:?}");
        release(cx, at);
        cx.run_until_parked();
        assert_eq!(
            selection(&editor, cx),
            vec![(doc_text.len(), doc_text.len())]
        );
        assert!(
            cx.debug_bounds("html-image-0").is_none(),
            "outside image not loaded"
        );

        // The link opens with the URL rules.
        let link = point(body.left() + px(8.0), body.top() + px(8.0));
        press(cx, link, 1, false);
        release(cx, link);
        cx.run_until_parked();
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.test"));

        // A click on the frame selects the block, which shows its source.
        let frame = point(summary.right() - px(4.0), summary.top() - px(2.0));
        press(cx, frame, 1, false);
        release(cx, frame);
        cx.run_until_parked();
        let line = editor.update_in(cx, |e, window, cx| e.layout_line(2, window, cx));
        assert!(line.block.is_none(), "source revealed");
    }
}

mod math {
    use super::*;
    use crate::editor::layout::BlockKind;
    use crate::editor::math::MathState;
    use gpui::{Context, Window};

    /// Lays the lines out until `done`, letting the renderer's thread answer.
    fn wait_for(
        editor: &Entity<Editor>,
        cx: &mut VisualTestContext,
        mut done: impl FnMut(&mut Editor, &mut Window, &mut Context<Editor>) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            cx.run_until_parked();
            if editor.update_in(cx, |e, window, cx| done(e, window, cx)) {
                cx.run_until_parked();
                return;
            }
            assert!(std::time::Instant::now() < deadline, "renderer timed out");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    const DOC: &str = "Inline $a^2$ here.\n\n$$\n\\frac{1}{2}\n$$\n\nBad $\\frac{a$ one.\n\nend";

    /// US-036: inline and block formulas drawn from MathJax's SVG once the
    /// renderer answers, an invalid one in red with its error, and a click
    /// on a formula revealing its source.
    #[gpui::test]
    fn formulas_render_off_thread_and_errors_show_their_source(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        let block_line = 2;
        let bad_line = 6;
        wait_for(&editor, cx, |e, window, cx| {
            let ready = |l: &crate::editor::layout::LaidLine| {
                l.inline_math
                    .iter()
                    .all(|m| !matches!(m.state, MathState::Pending))
            };
            let first = e.layout_line(0, window, cx);
            let bad = e.layout_line(bad_line, window, cx);
            let block = e.layout_line(block_line, window, cx);
            let block_ready = matches!(
                e.math.state("\n\\frac{1}{2}\n", true, e.config.text_size()),
                MathState::Ready(_)
            );
            ready(&first) && ready(&bad) && block.block.is_some() && block_ready
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let inline = cx
            .debug_bounds("math-inline-0")
            .expect("inline formula drawn");
        assert!(inline.size.width > px(4.0) && inline.size.height > px(4.0));
        let block = cx.debug_bounds("math-block").expect("block formula drawn");
        assert!(block.size.height > px(16.0));
        let (laid, kind) = editor.update_in(cx, |e, window, cx| {
            let l = e.layout_line(block_line, window, cx);
            let kind = l.block.as_ref().map(|b| b.kind.clone());
            (l, kind)
        });
        assert!(matches!(
            kind,
            Some(BlockKind::Rich(cerata_markdown::Widget::Math {
                display: true,
                ..
            }))
        ));
        assert!(laid.height >= block.size.height.as_f32());

        let bad = editor.update_in(cx, |e, window, cx| e.layout_line(bad_line, window, cx));
        let MathState::Error(message) = &bad.inline_math[0].state else {
            panic!("the invalid formula is an error");
        };
        assert!(message.contains("Missing close brace"), "{message}");
        assert!(
            cx.debug_bounds("math-inline-error-0").is_some(),
            "tooltip box drawn"
        );

        // A click on the block formula selects it: its source shows.
        let at = block.center();
        press(cx, at, 1, false);
        release(cx, at);
        cx.run_until_parked();
        let from = DOC.find("$$").unwrap();
        let to = DOC.rfind("$$").unwrap() + 2;
        assert_eq!(selection(&editor, cx), vec![(to, from)]);
        let line = editor.update_in(cx, |e, window, cx| e.layout_line(block_line, window, cx));
        assert!(line.block.is_none(), "source revealed");
    }

    /// Currency stays text: nothing is sent to the renderer.
    #[gpui::test]
    fn currency_is_not_a_formula(cx: &mut TestAppContext) {
        let text = "I paid $5 and $10 more, 5 $ and 10 $.";
        let (editor, cx) = open(cx, text);
        set_cursor(&editor, cx, text.len());
        let line = editor.update_in(cx, |e, window, cx| e.layout_line(0, window, cx));
        assert!(line.inline_math.is_empty());
    }
}

mod mermaid {
    use super::*;
    use crate::editor::layout::BlockKind;
    use crate::editor::mermaid::{CANVAS_HEIGHT, MermaidCanvas, PANEL_SHARE};
    use gpui::{Bounds, Focusable as _};

    const DOC: &str = "intro\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\nend";

    fn canvas(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> Entity<MermaidCanvas> {
        editor.read_with(cx, |e, _| {
            e.canvases.get(&0).map(|(c, _)| c.clone()).expect("canvas")
        })
    }

    /// Waits for the renderer's thread, then draws again.
    fn wait_drawn(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            if let Some(bounds) = cx.debug_bounds(selector) {
                return bounds;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{selector} never drawn"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn view(canvas: &Entity<MermaidCanvas>, cx: &mut VisualTestContext) -> (f32, f32, f32) {
        canvas.read_with(cx, |c, _| (c.view.zoom, c.view.pan.x, c.view.pan.y))
    }

    /// US-037: a 480 px canvas fitted with 16 px around the diagram; drag
    /// pans, Ctrl+wheel zooms around the pointer within 0.25 and 4, keys pan,
    /// zoom and fit.
    #[gpui::test]
    fn the_canvas_pans_zooms_and_fits(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, 0);
        let line = editor.update_in(cx, |e, window, cx| e.layout_line(2, window, cx));
        assert!(matches!(
            line.block.as_ref().map(|b| &b.kind),
            Some(BlockKind::Rich(cerata_markdown::Widget::Mermaid { .. }))
        ));
        let frame = wait_drawn(cx, "mermaid-canvas");
        assert!((frame.size.height.as_f32() - CANVAS_HEIGHT).abs() < 1.0);
        let diagram = wait_drawn(cx, "mermaid-diagram");
        // Fitted: centered, at least 16 px around (this small diagram stops
        // at the 4x cap before filling an axis).
        let margin_x = (diagram.left() - frame.left()).as_f32();
        let margin_y = (diagram.top() - frame.top()).as_f32();
        assert!(margin_x.min(margin_y) >= 15.0, "{margin_x} {margin_y}");
        assert!(((frame.right() - diagram.right()).as_f32() - margin_x).abs() < 2.0);
        assert!(((frame.bottom() - diagram.bottom()).as_f32() - margin_y).abs() < 2.0);
        let canvas = canvas(&editor, cx);
        let fitted = view(&canvas, cx);

        // Drag pans.
        let start = diagram.center();
        press(cx, start, 1, false);
        drag_to(cx, point(start.x + px(40.0), start.y + px(10.0)), true);
        release(cx, point(start.x + px(40.0), start.y + px(10.0)));
        cx.run_until_parked();
        let panned = view(&canvas, cx);
        assert!(
            (panned.1 - fitted.1 - 40.0).abs() < 0.5 && (panned.2 - fitted.2 - 10.0).abs() < 0.5
        );
        assert_eq!(selection(&editor, cx), vec![(0, 0)], "the caret stays");

        // Ctrl+wheel zooms, bounded.
        // GPUI's wheel up is a positive delta: it zooms in.
        for _ in 0..40 {
            cx.simulate_event(ScrollEvent {
                position: start,
                delta: ScrollDelta::Pixels(point(px(0.0), px(200.0))),
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                ..Default::default()
            });
        }
        cx.run_until_parked();
        assert!((view(&canvas, cx).0 - 4.0).abs() < 1e-3, "capped at 4");
        for _ in 0..80 {
            cx.simulate_event(ScrollEvent {
                position: start,
                delta: ScrollDelta::Pixels(point(px(0.0), px(-200.0))),
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                ..Default::default()
            });
        }
        cx.run_until_parked();
        let zoomed_out = view(&canvas, cx).0;
        assert!(zoomed_out >= 0.25f32.min(fitted.0) - 1e-3 && zoomed_out < fitted.0);
        // Without Ctrl the wheel is the note's, not the diagram's.
        cx.simulate_event(ScrollEvent {
            position: start,
            delta: ScrollDelta::Pixels(point(px(0.0), px(-60.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(view(&canvas, cx).0, zoomed_out);

        // Keys: `0` fits, `+` zooms in, arrows pan.
        cx.simulate_keystrokes("0");
        assert_eq!(view(&canvas, cx), fitted);
        // A small diagram fits at the cap: zoom out first.
        cx.simulate_keystrokes("-");
        let smaller = view(&canvas, cx).0;
        assert!(smaller < fitted.0);
        cx.simulate_keystrokes("+");
        assert!(view(&canvas, cx).0 > smaller);
        cx.simulate_keystrokes("0 right");
        let moved = view(&canvas, cx);
        assert!((moved.1 - (fitted.1 - 24.0)).abs() < 0.5);
        // Editing keys the canvas does not bind stop there too.
        cx.simulate_keystrokes("backspace tab delete shift-tab");
        assert_eq!(doc(&editor, cx), DOC, "keys never reach the text");
    }

    /// The edit panel takes a third of the width and writes the fence back
    /// 150 ms after the last keystroke.
    #[gpui::test]
    fn the_edit_panel_rewrites_the_fence_after_150_ms(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, 0);
        let frame = wait_drawn(cx, "mermaid-canvas");
        let canvas = canvas(&editor, cx);
        canvas.update_in(cx, |c, window, cx| {
            window.focus(&c.focus_handle(cx), cx);
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let panel = cx.debug_bounds("mermaid-panel").expect("panel open");
        let share = panel.size.width.as_f32() / frame.size.width.as_f32();
        assert!((share - PANEL_SHARE).abs() < 0.02, "{share}");

        // Typing in the panel: the fence follows 150 ms after the last key.
        let fence_end = DOC.find("B\n```").unwrap() + 1;
        let inner = canvas.read_with(cx, |c, _| c.panel_editor().expect("panel editor"));
        // Plain source across the panel: no code block, no page inset.
        let (code, column) = inner.update_in(cx, |e, window, cx| {
            let line = e.layout_line(1, window, cx);
            (line.code.is_some(), e.viewport.map(|v| v.column_width))
        });
        assert!(!code, "the fence is not drawn as a code block");
        let column = column.expect("panel laid out");
        assert!(
            (column - (panel.size.width.as_f32() - 24.0)).abs() < 2.0,
            "{column} in {panel:?}"
        );
        inner.update(cx, |e, cx| {
            e.select(
                cerata_markdown::Selection::cursor(fence_end - DOC.find("```").unwrap()),
                cx,
            )
        });
        cx.simulate_input(" --> C");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(doc(&editor, cx), DOC, "not before 150 ms");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(60));
        cx.run_until_parked();
        assert_eq!(doc(&editor, cx), DOC.replace("A --> B", "A --> B --> C"));
        // Digits typed in the panel stay text.
        cx.simulate_input("0");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        let text = doc(&editor, cx);
        let panel_text = inner.read_with(cx, |e, _| e.state.text().as_str().to_owned());
        assert!(text.contains("B --> C0"), "{text:?} panel {panel_text:?}");
    }

    /// Full screen opens from its button and closes with Escape, the close
    /// button or the backdrop, the focus back on the canvas.
    #[gpui::test]
    fn full_screen_closes_and_gives_the_focus_back(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, 0);
        wait_drawn(cx, "mermaid-diagram");
        let canvas = canvas(&editor, cx);
        let open_full = |cx: &mut VisualTestContext| {
            let expand = cx.debug_bounds("mermaid-expand").expect("expand button");
            press(cx, expand.center(), 1, false);
            release(cx, expand.center());
            cx.run_until_parked();
            assert!(cx.debug_bounds("mermaid-fullscreen").is_some(), "open");
        };
        let focused_canvas = |cx: &mut VisualTestContext| {
            canvas.update_in(cx, |c, window, cx| c.focus_handle(cx).is_focused(window))
        };

        open_full(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("mermaid-fullscreen").is_none());
        assert!(focused_canvas(cx));

        open_full(cx);
        let backdrop = cx.debug_bounds("mermaid-fullscreen").unwrap();
        let corner = point(backdrop.left() + px(4.0), backdrop.top() + px(4.0));
        press(cx, corner, 1, false);
        release(cx, corner);
        cx.run_until_parked();
        assert!(cx.debug_bounds("mermaid-fullscreen").is_none());
        assert!(focused_canvas(cx));
        assert_eq!(doc(&editor, cx), DOC);
    }

    /// Tab moves from the canvas to its toolbar; Enter clicks the focused
    /// button once (not the canvas's own Enter as well).
    #[gpui::test]
    fn the_toolbar_works_from_the_keyboard(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, 0);
        wait_drawn(cx, "mermaid-diagram");
        let canvas = canvas(&editor, cx);
        canvas.update_in(cx, |c, window, cx| window.focus(&c.focus_handle(cx), cx));
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        let on_button = canvas.update_in(cx, |c, window, cx| {
            let handle = c.focus_handle(cx);
            !handle.is_focused(window) && handle.contains_focused(window, cx)
        });
        assert!(on_button, "Tab reached a toolbar button");
        let enter = gpui::Keystroke::parse("enter").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: enter.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent { keystroke: enter });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("mermaid-panel").is_some(),
            "Edit code opened once"
        );
        assert_eq!(doc(&editor, cx), DOC);
    }

    /// A canvas taking the keyboard focus off screen scrolls into sight.
    #[gpui::test]
    fn a_focused_canvas_scrolls_into_sight(cx: &mut TestAppContext) {
        let text = format!("{DOC}\n\n{}", "filler line\n\n".repeat(200));
        let (editor, cx) = open(cx, &text);
        set_cursor(&editor, cx, 0);
        wait_drawn(cx, "mermaid-diagram");
        let canvas = canvas(&editor, cx);
        editor.update(cx, |e, cx| {
            e.set_scroll_top(4000.0);
            cx.notify();
        });
        cx.run_until_parked();
        canvas.update_in(cx, |c, window, cx| window.focus(&c.focus_handle(cx), cx));
        cx.run_until_parked();
        let (scroll, top) = editor.read_with(cx, |e, _| {
            let fence = text.find("```").unwrap();
            let line = e.state.text().line_at(fence).number;
            (e.scroll_top(), e.heights.prefix(line))
        });
        assert!(
            (scroll - (top - SAFE_MARGIN).max(0.0)).abs() < 1.0,
            "{scroll} {top}"
        );
    }

    /// ADR 0006: the caret steps over a diagram, which stays drawn: Down
    /// stops before it, then after it, then on the next line; Backspace
    /// after it selects it, then deletes it; typing at a boundary takes a
    /// line of its own.
    #[gpui::test]
    fn the_caret_steps_over_a_diagram(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, 0);
        let frame = wait_drawn(cx, "mermaid-canvas");
        let start = DOC.find("```").unwrap();
        let end = DOC.rfind("```").unwrap() + 3;
        let last_caret = |editor: &Entity<Editor>, cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            editor.read_with(cx, |e, _| {
                e.frames
                    .last()
                    .and_then(|f| f.carets.first().copied())
                    .expect("a caret drawn")
            })
        };
        let canvas_drawn = |editor: &Entity<Editor>, cx: &mut VisualTestContext| {
            editor.update_in(cx, |e, window, cx| {
                let line = e.layout_line(2, window, cx);
                line.block.is_some_and(|b| b.height >= CANVAS_HEIGHT)
            })
        };

        let mut heads = Vec::new();
        for _ in 0..4 {
            cx.simulate_keystrokes("down");
            cx.run_until_parked();
            heads.push(selection(&editor, cx)[0].1);
            assert!(canvas_drawn(&editor, cx), "the diagram stays drawn");
        }
        assert_eq!(heads, vec![start, end, end + 1, end + 2]);
        // Up from the line under it: straight to its end.
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(end, end)]);
        // After the diagram: a caret on its right edge, its full height.
        let caret = last_caret(&editor, cx);
        assert!(
            (caret.right() - frame.right()).abs() < px(3.0),
            "{caret:?} {frame:?}"
        );
        assert!(caret.size.height >= px(CANVAS_HEIGHT));
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(start, start)]);
        let caret = last_caret(&editor, cx);
        assert!(
            (caret.left() - frame.left()).abs() < px(3.0),
            "{caret:?} {frame:?}"
        );
        // Right and Left never stop inside.
        cx.simulate_keystrokes("right");
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(end, end)]);
        cx.simulate_keystrokes("left");
        cx.run_until_parked();
        assert_eq!(selection(&editor, cx), vec![(start, start)]);

        // Typed at the start: a line of its own above the diagram.
        cx.simulate_input("x");
        cx.run_until_parked();
        let typed = DOC.replace("```mermaid", "x\n```mermaid");
        assert_eq!(doc(&editor, cx), typed);
        assert!(canvas_drawn_at(&editor, cx, 3));
        cx.simulate_keystrokes(&secondary("z"));
        cx.run_until_parked();
        assert_eq!(doc(&editor, cx), DOC);

        // Backspace after it selects it, then deletes it.
        set_cursor(&editor, cx, end);
        cx.simulate_keystrokes("backspace");
        cx.run_until_parked();
        assert_eq!(doc(&editor, cx), DOC);
        assert_eq!(selection(&editor, cx), vec![(start, end)]);
        cx.simulate_keystrokes("backspace");
        cx.run_until_parked();
        assert_eq!(doc(&editor, cx), "intro\n\n\n\nend");
    }

    fn canvas_drawn_at(editor: &Entity<Editor>, cx: &mut VisualTestContext, line: usize) -> bool {
        editor.update_in(cx, |e, window, cx| {
            let laid = e.layout_line(line, window, cx);
            laid.block.is_some_and(|b| b.height >= CANVAS_HEIGHT)
        })
    }

    /// A diagram Pele rejects shows the message and the source.
    #[gpui::test]
    fn a_syntax_error_shows_its_message_and_source(cx: &mut TestAppContext) {
        let text =
            "```mermaid\nflowchart TD\n  A --> B -->\n  C[[[ <img src=x onerror=1>\n```\n\nend";
        let (editor, cx) = open(cx, text);
        set_cursor(&editor, cx, text.len());
        wait_drawn(cx, "mermaid-error");
        let canvas = canvas(&editor, cx);
        let error = canvas.read_with(cx, |c, _| c.rendered().cloned());
        // `mermaid.test.ts:300-312`: the message is text, its markup shown
        // as typed.
        let Some(Err(message)) = error else {
            panic!("an error");
        };
        assert!(message.contains("<img src=x onerror=1>"), "{message}");

        // `mermaid.test.ts:576-587`: full screen opens on the error, and
        // Escape closes it.
        let expand = cx.debug_bounds("mermaid-expand").expect("expand button");
        press(cx, expand.center(), 1, false);
        release(cx, expand.center());
        cx.run_until_parked();
        assert!(cx.debug_bounds("mermaid-fullscreen").is_some());
        let full = editor.read_with(cx, |e, _| {
            e.fullscreen
                .as_ref()
                .map(|f| f.canvas.clone())
                .expect("full screen")
        });
        assert!(matches!(
            full.read_with(cx, |c, _| c.rendered().cloned()),
            Some(Err(_))
        ));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("mermaid-fullscreen").is_none());

        // `mermaid.test.ts:332-344`: fixing the source swaps the error for
        // the diagram.
        let broken = text.find("  A --> B -->").unwrap();
        let line_end = broken + text[broken..].find("```").unwrap();
        editor.update(cx, |e, cx| {
            e.update_state(cx, |s| {
                let _ = s.apply(cerata_markdown::Transaction::new().change(
                    cerata_markdown::Change::new(broken..line_end, "  A --> B --> C\n"),
                ));
            })
        });
        wait_drawn(cx, "mermaid-diagram");
        assert!(cx.debug_bounds("mermaid-error").is_none());
        assert!(matches!(
            canvas.read_with(cx, |c, _| c.rendered().cloned()),
            Some(Ok(_))
        ));
    }
}

mod rail {
    use super::*;
    use crate::editor::menu::{MenuAction, MenuItem};
    use crate::editor::rail::{MIN_HEADINGS, POPOVER_WIDTH};

    /// `count` h2 sections, each followed by 30 paragraphs.
    fn sections(count: usize) -> String {
        let mut out = String::new();
        for i in 0..count {
            out.push_str(&format!("## Section {i}\n\n"));
            for j in 0..30 {
                out.push_str(&format!("Paragraph {j} of section {i}.\n\n"));
            }
        }
        out
    }

    fn hover(cx: &mut VisualTestContext, position: Point<Pixels>) {
        drag_to(cx, position, false);
        cx.run_until_parked();
    }

    fn line_of(editor: &Entity<Editor>, cx: &mut VisualTestContext, i: usize) -> f64 {
        editor.read_with(cx, |e, _| {
            let headings = e.rail.headings.clone();
            e.heights.prefix(headings[i].line)
        })
    }

    fn active(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> Option<usize> {
        editor.update(cx, |e, cx| {
            let headings = e.rail_headings(cx);
            e.active_heading(&headings)
        })
    }

    /// US-038: hidden under 5 headings; one 8 x 1 mark per heading, 6 px
    /// apart, on the right edge.
    #[gpui::test]
    fn the_rail_shows_from_five_headings(cx: &mut TestAppContext) {
        let (_editor, cx) = open(cx, &sections(MIN_HEADINGS - 1));
        assert!(
            cx.debug_bounds("section-rail").is_none(),
            "4 headings: no rail"
        );

        let (_editor, cx) = open(cx, &sections(MIN_HEADINGS));
        let rail = cx.debug_bounds("section-rail").expect("rail");
        let window = cx.update(|window, _| window.viewport_size());
        assert!(rail.right() <= window.width && rail.right() >= window.width - px(24.0));
        let first = cx.debug_bounds("rail-tick-0").expect("tick 0");
        let second = cx.debug_bounds("rail-tick-1").expect("tick 1");
        assert_eq!(first.size.width, px(10.0), "8 px mark in a 1 px focus ring");
        assert_eq!(
            (second.top() - first.top()).as_f32(),
            7.0,
            "1 px mark + 6 px gap"
        );
        assert!(cx.debug_bounds("rail-tick-4").is_some());
        assert!(cx.debug_bounds("rail-popover").is_none());
    }

    /// A note with thousands of headings builds only the marks that fit in
    /// 70 % of the view, around the active one, and reparses its headings
    /// only once typing pauses (`use-document-headings.ts:85-87`).
    #[gpui::test]
    fn a_long_note_builds_only_the_marks_that_fit(cx: &mut TestAppContext) {
        use crate::editor::rail::HEADINGS_DEBOUNCE;
        let note: String = (0..2000).map(|i| format!("## Heading {i}\n\n")).collect();
        let (editor, cx) = open(cx, &note);
        let rail = cx.debug_bounds("section-rail").expect("rail");
        let window = cx.update(|window, _| window.viewport_size());
        assert!(rail.size.height < window.height, "{rail:?}");
        let drawn = (0..2000)
            .filter(|i| {
                cx.debug_bounds(Box::leak(format!("rail-tick-{i}").into_boxed_str()))
                    .is_some()
            })
            .count();
        assert!(drawn > 10 && drawn <= 200, "{drawn} marks built");
        assert!(cx.debug_bounds("rail-tick-0").is_some());

        // Far down the note, the marks around the active heading are drawn.
        let target = editor.update(cx, |e, _| e.rail.headings[1500].pos);
        editor.update(cx, |e, cx| e.rail_go_to(target, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let shown = active(&editor, cx).expect("an active heading");
        assert!(shown >= 1490, "{shown}");
        assert!(
            cx.debug_bounds(Box::leak(format!("rail-tick-{shown}").into_boxed_str()))
                .is_some()
        );

        // Typing keeps the parsed headings until the pause.
        set_cursor(&editor, cx, 0);
        cx.simulate_input("# New\n\n");
        cx.run_until_parked();
        let before = editor.read_with(cx, |e, _| e.rail.headings.len());
        assert_eq!(before, 2000);
        cx.executor().advance_clock(HEADINGS_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |e, _| e.rail.headings.len()), 2001);
    }

    /// The active heading is the last one at or above the safe line, else
    /// the first; it follows the scroll.
    #[gpui::test]
    fn the_active_heading_follows_the_scroll(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, &sections(6));
        assert_eq!(active(&editor, cx), Some(0));
        let third = line_of(&editor, cx, 2);
        editor.update(cx, |e, cx| {
            e.set_scroll_top(third - SAFE_MARGIN);
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(active(&editor, cx), Some(2));
        editor.update(cx, |e, _| e.set_scroll_top(third - SAFE_MARGIN - 10.0));
        assert_eq!(active(&editor, cx), Some(1), "not past the line yet");
    }

    /// Hovering opens the 260 px outline; a row scrolls its heading to the
    /// safe line and makes it active; leaving or Escape closes it.
    #[gpui::test]
    fn the_outline_opens_on_hover_and_jumps(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, &sections(6));
        set_cursor(&editor, cx, 0);
        let tick = cx.debug_bounds("rail-tick-2").expect("tick");
        hover(cx, tick.center());
        let popover = cx.debug_bounds("rail-popover").expect("outline open");
        assert_eq!(popover.size.width, px(POPOVER_WIDTH));

        let row = cx.debug_bounds("rail-row-4").expect("row 4");
        press(cx, row.center(), 1, false);
        release(cx, row.center());
        cx.run_until_parked();
        let target = line_of(&editor, cx, 4);
        let scroll = editor.read_with(cx, |e, _| e.scroll_top());
        assert!(
            (scroll - (target - SAFE_MARGIN)).abs() < 1.0,
            "{scroll} {target}"
        );
        assert_eq!(active(&editor, cx), Some(4));
        assert_eq!(selection(&editor, cx), vec![(0, 0)], "the caret stays");

        hover(cx, point(px(300.0), px(300.0)));
        assert!(cx.debug_bounds("rail-popover").is_none(), "closed on leave");

        hover(cx, tick.center());
        assert!(cx.debug_bounds("rail-popover").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("rail-popover").is_none(),
            "closed by Escape"
        );
    }

    /// A right click on a mark offers "Copy heading link", which copies
    /// `[text](#slug)`.
    #[gpui::test]
    fn a_right_click_copies_the_heading_link(cx: &mut TestAppContext) {
        let (editor, cx) = open(
            cx,
            &sections(6).replace("## Section 3", "## Sources & Notes"),
        );
        // With the outline closed the marks take the click.
        let tick = cx.debug_bounds("rail-tick-3").expect("tick");
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: tick.center(),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        let action = editor.read_with(cx, |e, _| {
            let menu = e.menu.as_ref().expect("menu open");
            assert_eq!(menu.items.len(), 1);
            match &menu.items[0] {
                MenuItem::Action { label, action, .. } => {
                    assert_eq!(label.as_ref(), "Copy heading link");
                    action.clone()
                }
                _ => panic!("an action"),
            }
        });
        assert!(
            matches!(&action, MenuAction::CopyLink(l) if l == "[Sources & Notes](#sources-notes)"),
            "{action:?}"
        );
        editor.update_in(cx, |e, window, cx| e.run_menu_action(action, window, cx));
        assert_eq!(
            cx.read_from_clipboard().and_then(|i| i.text()).as_deref(),
            Some("[Sources & Notes](#sources-notes)")
        );
    }
}

// EP-012: images added by paste, drop and picker ----------------------

/// A note in a fresh folder, and a folder outside it holding `files`.
fn note_with_files<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    files: &[(&str, &[u8])],
) -> (
    Entity<Editor>,
    &'a mut VisualTestContext,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    for (name, bytes) in files {
        std::fs::write(outside.path().join(name), bytes).unwrap();
    }
    let path = dir.path().join("note.md");
    std::fs::write(&path, text).unwrap();
    let (editor, cx) = open_at(cx, text, Some(path), EditorConfig::default());
    (editor, cx, dir, outside)
}

fn notice(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> Option<String> {
    editor.read_with(cx, |e, _| e.notice.as_ref().map(|n| n.to_string()))
}

fn add_file(editor: &Entity<Editor>, cx: &mut VisualTestContext, path: PathBuf) {
    editor.update(cx, |editor, cx| {
        let source = cerata_core::images::ImportSource::File(path);
        editor.add_images(vec![source], super::import::ImageTarget::Caret, cx);
    });
}

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"></svg>"#;

/// Files copied in the file manager paste as images; one undo removes them
/// all, the copies stay, and one notice names the file that is no image.
#[gpui::test]
fn pasted_files_insert_image_blocks_in_one_undo_step(cx: &mut TestAppContext) {
    let png = png(4, 3);
    let (editor, cx, dir, outside) = note_with_files(
        cx,
        "Intro",
        &[
            ("a.png", &png[..]),
            ("b.png", &png[..]),
            ("notes.txt", b"hello"),
        ],
    );
    set_cursor(&editor, cx, 5);
    let paths = ["a.png", "b.png", "notes.txt"].map(|name| outside.path().join(name));
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![
            ClipboardEntry::ExternalPaths(gpui::ExternalPaths(paths.into_iter().collect())),
            ClipboardEntry::String(gpui::ClipboardString::new("ignored".into())),
        ],
    });
    cx.simulate_keystrokes(&secondary("v"));
    cx.run_until_parked();
    assert_eq!(
        doc(&editor, cx),
        "Intro\n\n![a](note-assets/a.png)\n\n![b](note-assets/b.png)\n"
    );
    assert_eq!(
        notice(&editor, cx).as_deref(),
        Some("notes.txt is not an image")
    );
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), "Intro", "one undo step");
    assert!(
        dir.path().join("note-assets/a.png").is_file(),
        "copies stay"
    );
    assert!(dir.path().join("note-assets/b.png").is_file());
}

/// A 6 MB file copied in the file manager has no paste limit.
#[gpui::test]
fn a_pasted_file_has_no_byte_limit(cx: &mut TestAppContext) {
    let mut big = png(4, 3);
    big.resize(6 * 1024 * 1024, 0);
    let (editor, cx, dir, outside) = note_with_files(cx, "", &[("big.png", &big[..])]);
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
            [outside.path().join("big.png")].into_iter().collect(),
        ))],
    });
    cx.simulate_keystrokes(&secondary("v"));
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "![big](note-assets/big.png)\n");
    assert_eq!(
        std::fs::metadata(dir.path().join("note-assets/big.png"))
            .unwrap()
            .len(),
        6 * 1024 * 1024
    );
}

/// Inside a list item the images go inline at the caret.
#[gpui::test]
fn images_added_in_a_list_item_stay_inline(cx: &mut TestAppContext) {
    let png = png(4, 3);
    let (editor, cx, _dir, outside) = note_with_files(cx, "- item", &[("a.png", &png[..])]);
    set_cursor(&editor, cx, 6);
    add_file(&editor, cx, outside.path().join("a.png"));
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "- item![a](note-assets/a.png)");
}

/// The insertion point follows what is typed while the files are copied,
/// a batch still copying after 300 ms shows a notice, and the notice goes
/// with the insertion.
#[gpui::test]
fn a_slow_batch_shows_a_notice_and_lands_where_it_was_asked(cx: &mut TestAppContext) {
    let (editor, cx, _dir, _outside) = note_with_files(cx, "Intro", &[]);
    use gpui::AppContext as _;
    let (done, gate) = futures::channel::oneshot::channel::<()>();
    editor.update(cx, |editor, cx| {
        let work = cx.background_spawn(async move {
            let _ = gate.await;
            vec![Ok("![a](a.png)".to_owned()), Ok("![b](b.png)".to_owned())]
        });
        editor.insert_when_done(super::import::ImageTarget::At(5), 2, work, cx);
    });
    cx.run_until_parked();
    assert_eq!(notice(&editor, cx), None, "not before 300 ms");
    cx.executor()
        .advance_clock(super::import::LOADING_NOTICE_DELAY);
    cx.run_until_parked();
    assert_eq!(
        notice(&editor, cx).as_deref(),
        Some("Adding 2 images\u{2026}")
    );
    // Typed while copying, before the insertion point.
    editor.update(cx, |editor, cx| {
        editor.update_state(cx, |state| {
            let _ =
                state.apply(Transaction::new().change(cerata_markdown::Change::insert(0, "So ")));
        });
    });
    done.send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "So Intro\n\n![a](a.png)\n\n![b](b.png)\n");
    assert_eq!(notice(&editor, cx), None);
}

/// A tab closed during the copy inserts nothing and keeps the copies.
#[gpui::test]
fn a_closed_note_keeps_the_copies_and_inserts_nothing(cx: &mut TestAppContext) {
    let png = png(4, 3);
    let (editor, cx, dir, outside) = note_with_files(cx, "Intro", &[("a.png", &png[..])]);
    add_file(&editor, cx, outside.path().join("a.png"));
    cx.update(|window, _| window.remove_window());
    drop(editor);
    cx.run_until_parked();
    assert!(dir.path().join("note-assets/a.png").is_file());
}

/// A write error inserts nothing for the file and names the OS error.
#[gpui::test]
fn a_write_error_inserts_nothing(cx: &mut TestAppContext) {
    let png = png(4, 3);
    let (editor, cx, dir, outside) = note_with_files(cx, "Intro", &[("a.png", &png[..])]);
    // A file where the assets folder should be.
    std::fs::write(dir.path().join("note-assets"), b"").unwrap();
    add_file(&editor, cx, outside.path().join("a.png"));
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "Intro");
    let notice = notice(&editor, cx).unwrap_or_default();
    assert!(notice.starts_with("Image not added: "), "{notice}");
}

#[gpui::test]
fn a_note_without_a_path_adds_no_image(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "text");
    editor.update(cx, |editor, cx| editor.pick_images(cx));
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths(), "no picker");
    assert_eq!(
        notice(&editor, cx).as_deref(),
        Some("Image not added: the note is not saved in a folder")
    );
}

/// The context menu's Insert > Image… opens the picker; canceling it
/// changes nothing.
#[gpui::test]
fn the_menu_entry_opens_the_picker(cx: &mut TestAppContext) {
    use super::menu::{MenuAction, MenuItem, body_items};
    let (editor, cx, _dir, _outside) = note_with_files(cx, "Intro", &[]);
    let insert = editor.read_with(cx, |editor, _| {
        body_items(false, None, &editor.state, None)
            .into_iter()
            .find_map(|item| match item {
                MenuItem::Submenu { label, items } if label.as_ref() == "Insert" => Some(items),
                _ => None,
            })
    });
    assert!(insert.unwrap().iter().any(|item| matches!(
        item,
        MenuItem::Action { label, action: MenuAction::InsertImage, .. }
            if label.as_ref() == "Image\u{2026}"
    )));
    editor.update_in(cx, |editor, window, cx| {
        editor.run_menu_action(MenuAction::InsertImage, window, cx)
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "Intro", "canceled: nothing changes");
}

/// A file drag carrying an image shows the bar at the nearest block
/// boundary, starts no selection, and inserts there on drop.
#[gpui::test]
fn a_file_drag_shows_a_bar_and_drops_at_the_boundary(cx: &mut TestAppContext) {
    let png = png(4, 3);
    let text = "# Title\n\nPara one\n\nPara two\n";
    let (editor, cx, dir, outside) =
        note_with_files(cx, text, &[("a.png", &png[..]), ("b.svg", SVG)]);
    set_cursor(&editor, cx, 3);
    let before = selection(&editor, cx);
    let over = point_at(&editor, cx, text.find("Para two").unwrap() + 3);
    let paths = gpui::ExternalPaths(
        [outside.path().join("a.png"), outside.path().join("b.svg")]
            .into_iter()
            .collect(),
    );
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: over,
        paths: paths.clone(),
    });
    cx.simulate_event(gpui::FileDropEvent::Pending { position: over });
    let bar = editor.read_with(cx, |e, _| e.drop_bar);
    assert_eq!(bar.map(|b| b.pos), text.find("Para two"));
    assert_eq!(selection(&editor, cx), before, "no drag selection");
    // Leaving the window hides the bar and keeps the selection.
    cx.simulate_event(gpui::FileDropEvent::Exited);
    assert_eq!(editor.read_with(cx, |e, _| e.drop_bar), None);
    assert_eq!(selection(&editor, cx), before);
    // Back in, then dropped.
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: over,
        paths,
    });
    cx.simulate_event(gpui::FileDropEvent::Submit { position: over });
    cx.run_until_parked();
    assert_eq!(
        doc(&editor, cx),
        "# Title\n\nPara one\n\n![a](note-assets/a.png)\n\n![b](note-assets/b.svg)\n\nPara two\n"
    );
    assert_eq!(editor.read_with(cx, |e, _| e.drop_bar), None);
    assert!(dir.path().join("note-assets/b.svg").is_file());
}

/// No bar for a drag without an image; inside a code block the bar snaps
/// to one of its boundaries.
#[gpui::test]
fn the_drop_bar_snaps_to_block_boundaries(cx: &mut TestAppContext) {
    let text = "Intro\n\n```\ncode\nmore\n```\n\nEnd\n";
    let (editor, cx, _dir, outside) = note_with_files(cx, text, &[]);
    let in_code = point_at(&editor, cx, text.find("more").unwrap() + 1);
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: in_code,
        paths: gpui::ExternalPaths([outside.path().join("notes.md")].into_iter().collect()),
    });
    cx.simulate_event(gpui::FileDropEvent::Pending { position: in_code });
    assert_eq!(editor.read_with(cx, |e, _| e.drop_bar), None, "no image");
    cx.simulate_event(gpui::FileDropEvent::Exited);
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: in_code,
        paths: gpui::ExternalPaths([outside.path().join("a.png")].into_iter().collect()),
    });
    cx.simulate_event(gpui::FileDropEvent::Pending { position: in_code });
    let pos = editor.read_with(cx, |e, _| e.drop_bar.map(|b| b.pos));
    assert!(
        pos == text.find("```") || pos == text.find("End"),
        "{pos:?} is a boundary of the code block"
    );
    cx.simulate_event(gpui::FileDropEvent::Exited);
}

/// A screenshot copied as a bitmap (`CF_DIB`, a BMP once read) over 5 MB
/// is pasted as a PNG under the limit.
#[gpui::test]
fn a_pasted_bitmap_over_the_limit_is_saved_as_png(cx: &mut TestAppContext) {
    let (editor, cx, dir, _outside) = note_with_files(cx, "", &[]);
    let mut bmp = Vec::new();
    image::RgbImage::from_pixel(1400, 1400, image::Rgb([40, 90, 160]))
        .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
        .unwrap();
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![ClipboardEntry::Image(Image::from_bytes(
            ImageFormat::Bmp,
            bmp,
        ))],
    });
    cx.simulate_keystrokes(&secondary("v"));
    cx.run_until_parked();
    let text = doc(&editor, cx);
    assert!(text.starts_with("![image.png](note-assets/"), "{text}");
    let saved: Vec<_> = std::fs::read_dir(dir.path().join("note-assets"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].extension().is_some_and(|e| e == "png"));
    assert!(std::fs::metadata(&saved[0]).unwrap().len() < 5 * 1024 * 1024);
}

/// A drop at the boundary that starts a list or a table inserts blocks
/// before it, never inline in its first item or row.
#[gpui::test]
fn a_drop_before_a_list_or_a_table_inserts_blocks(cx: &mut TestAppContext) {
    let png = png(4, 3);
    let text = "Intro\n\n- item\n\n| A | B |\n| --- | --- |\n| x | y |\n";
    let (editor, cx, _dir, outside) = note_with_files(cx, text, &[("a.png", &png[..])]);
    let drop_over = |editor: &Entity<Editor>, cx: &mut VisualTestContext, needle: &str| {
        let text = doc(editor, cx);
        let over = point_at(editor, cx, text.find(needle).unwrap() + 1);
        cx.simulate_event(gpui::FileDropEvent::Entered {
            position: over,
            paths: gpui::ExternalPaths([outside.path().join("a.png")].into_iter().collect()),
        });
        cx.simulate_event(gpui::FileDropEvent::Pending { position: over });
        let bar = editor.read_with(cx, |e, _| e.drop_bar.map(|b| b.pos));
        assert_eq!(bar, text.find(needle), "the bar is at the block's start");
        cx.simulate_event(gpui::FileDropEvent::Submit { position: over });
        cx.run_until_parked();
    };
    drop_over(&editor, cx, "- item");
    assert_eq!(
        doc(&editor, cx),
        "Intro\n\n![a](note-assets/a.png)\n\n- item\n\n| A | B |\n| --- | --- |\n| x | y |\n"
    );
    drop_over(&editor, cx, "| A |");
    assert_eq!(
        doc(&editor, cx),
        "Intro\n\n![a](note-assets/a.png)\n\n- item\n\n![a](<note-assets/a 1.png>)\n\n| A | B |\n| --- | --- |\n| x | y |\n"
    );
}

// EP-013: image widths, object selection and resizing ------------------

/// A note next to `wide.png` (1600 x 900), `small.png` (200 x 120) and an
/// unreadable `bad.png`, the caret at its end and its images measured.
fn image_editor<'a>(
    cx: &'a mut TestAppContext,
    note: &str,
) -> (Entity<Editor>, &'a mut VisualTestContext, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wide.png"), png(1600, 900)).unwrap();
    std::fs::write(dir.path().join("small.png"), png(200, 120)).unwrap();
    std::fs::write(dir.path().join("bad.png"), b"not a png").unwrap();
    let path = dir.path().join("note.md");
    let (editor, cx) = open_at(cx, note, Some(path), EditorConfig::default());
    set_cursor(&editor, cx, note.len());
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (editor, cx, dir)
}

fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// The painted bounds of the block image on line `ix`.
fn image_bounds(cx: &mut VisualTestContext, ix: usize) -> gpui::Bounds<Pixels> {
    cx.debug_bounds(selector(format!("editor-image-{ix}")))
        .expect("image painted")
}

fn handle_bounds(
    cx: &mut VisualTestContext,
    side: &str,
    ix: usize,
) -> Option<gpui::Bounds<Pixels>> {
    cx.debug_bounds(selector(format!("image-handle-{side}-{ix}")))
}

fn viewport(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> super::Viewport {
    editor.read_with(cx, |e, _| e.viewport.expect("painted"))
}

fn hover(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_mouse_move(position, None, Modifiers::default());
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, position: Point<Pixels>, clicks: usize) {
    press(cx, position, clicks, false);
    release(cx, position);
    cx.run_until_parked();
}

fn last_frame(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> super::element::FrameRecord {
    editor.read_with(cx, |e, _| e.frames.last().cloned().expect("a frame"))
}

/// US-064: the width a block image's source asks for, bounded by the
/// column (and enlarging a small image), centered when narrower than the
/// column; no size shows the natural width.
#[gpui::test]
fn block_images_show_at_their_width_and_narrow_ones_are_centered(cx: &mut TestAppContext) {
    let note = "![a|300](wide.png)\n\n![b|2000](wide.png)\n\n![c](small.png)\n\n![d|400](small.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let v = viewport(&editor, cx);
    let column = v.column_width;
    let centered = |width: f32| v.column_left + px((column - width) / 2.0);
    let a = image_bounds(cx, 0);
    assert_eq!((a.size.width, a.size.height), (px(300.0), px(169.0)));
    assert_eq!(a.origin.x, centered(300.0));
    let b = image_bounds(cx, 2);
    assert_eq!(b.size.width, px(column), "bounded by the column");
    assert_eq!(b.origin.x, v.column_left);
    let c = image_bounds(cx, 4);
    assert_eq!((c.size.width, c.size.height), (px(200.0), px(120.0)));
    assert_eq!(c.origin.x, centered(200.0));
    let d = image_bounds(cx, 6);
    assert_eq!(
        (d.size.width, d.size.height),
        (px(400.0), px(240.0)),
        "enlarged past its natural width"
    );
    assert_eq!(d.origin.x, centered(400.0));
}

/// US-064: the frame after a width change already has the lines below the
/// image at their new place.
#[gpui::test]
fn a_new_width_moves_the_lines_below_in_the_next_frame(cx: &mut TestAppContext) {
    let note = "![a|300](wide.png)\nafter\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    assert_eq!(image_bounds(cx, 0).size.height, px(169.0));
    editor.update(cx, |e, cx| {
        e.frames.clear();
        e.update_state(cx, |s| s.set_image_width(0..18, Some(150)));
    });
    cx.run_until_parked();
    let frames = editor.read_with(cx, |e, _| e.frames.len());
    assert_eq!(frames, 1, "one frame drew the change");
    let image = image_bounds(cx, 0);
    assert_eq!(image.size.height, px(84.0));
    let below = editor.read_with(cx, |e, _| e.line_screen_top(1));
    assert_eq!(
        below,
        image.bottom(),
        "the next line follows the new height"
    );
}

/// US-064: an unreadable image with a width reserves a box that wide at
/// its estimated height; a missing one shows its source, muted.
#[gpui::test]
fn an_image_that_cannot_be_drawn_keeps_its_width_or_its_source(cx: &mut TestAppContext) {
    let note = "![x|300](bad.png)\n\n![m|300](missing.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    editor.update_in(cx, |e, window, cx| {
        let bad = e.layout_line(0, window, cx);
        let block = bad.block.expect("a placeholder");
        assert_eq!((block.width, block.height), (300.0, 30.0));
        assert_eq!(bad.height, 30.0, "its estimated height");
        let column = e.column_width();
        assert_eq!(block.left, (column - 300.0) / 2.0, "centered");
        let missing = e.layout_line(2, window, cx);
        assert!(missing.block.is_none(), "no box: its source shows");
        assert!(missing.display.text.contains("missing.png"));
    });
}

/// US-064: an embed and a `data:` image honor their width.
#[gpui::test]
fn embeds_and_data_images_honor_their_width(cx: &mut TestAppContext) {
    let data = format!("data:image/png;base64,{}", base64_encode(&png(20, 10)));
    let note = format!("![[small.png|120]]\n\n![d|10]({data})\n\nend");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("small.png"), png(200, 120)).unwrap();
    let path = dir.path().join("note.md");
    let (editor, cx) = open_at(cx, &note, Some(path), EditorConfig::default());
    editor.update(cx, |e, _| e.images.set_root(dir.path()));
    set_cursor(&editor, cx, note.len());
    for _ in 0..50 {
        cx.run_until_parked();
        let done = editor.update_in(cx, |e, window, cx| {
            e.layout_line(0, window, cx)
                .block
                .is_some_and(|b| b.height == 72.0)
        });
        if done {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    editor.update_in(cx, |e, window, cx| {
        let embed = e.layout_line(0, window, cx).block.expect("embed drawn");
        assert_eq!((embed.width, embed.height), (120.0, 72.0));
        let inline = e
            .layout_line(2, window, cx)
            .block
            .expect("data image drawn");
        assert_eq!((inline.width, inline.height), (10.0, 5.0));
    });
}

/// US-065 (rewrites the click test of US-018 for ADR 0008): a click on a
/// block image selects it as an object, anchored at its end, whatever was
/// selected; it stays drawn where it was, with a ring and no caret.
#[gpui::test]
fn a_click_on_an_image_selects_it_as_an_object_without_moving_it(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    editor.update(cx, |e, cx| e.select(Selection::single(0, 3), cx));
    cx.run_until_parked();
    let before = image_bounds(cx, 2);
    click(cx, before.center(), 1);
    assert_eq!(selection(&editor, cx), [(25, 7)], "anchor at the end");
    let after = image_bounds(cx, 2);
    assert_eq!(after, before, "the image did not move");
    let frame = last_frame(&editor, cx);
    assert_eq!(frame.rings.len(), 1, "a ring");
    let ring = frame.rings[0];
    assert_eq!(ring.origin.x, before.origin.x - px(4.0));
    assert_eq!(ring.size.width, before.size.width + px(8.0));
    assert!(frame.carets.is_empty(), "no caret");
    assert!(frame.selections.is_empty(), "no highlight");
}

/// US-065: an image taller than the room left above the bottom margin
/// does not scroll the note when clicked (found in the app on Windows).
#[gpui::test]
fn a_click_on_an_image_near_the_bottom_does_not_scroll(cx: &mut TestAppContext) {
    let mut note = String::new();
    for i in 0..10 {
        note.push_str(&format!("line {i}\n"));
    }
    note.push_str("\n![a](wide.png)\n\nend");
    let (editor, cx, _dir) = image_editor(cx, &note);
    set_cursor(&editor, cx, 0);
    let before = image_bounds(cx, 11);
    let viewport = viewport(&editor, cx);
    assert!(
        before.bottom() > viewport.bounds.bottom() - px(SAFE_MARGIN as f32),
        "the image reaches into the bottom margin"
    );
    click(cx, before.center(), 1);
    let start = note.find("![a").unwrap_or(0);
    assert_eq!(selection(&editor, cx), [(start + 14, start)]);
    assert_eq!(image_bounds(cx, 11), before, "the note did not scroll");
}

/// US-065: a right click selects the image before the menu opens, whose
/// Image width submenu comes first.
#[gpui::test]
fn a_right_click_on_an_image_selects_it_before_the_menu(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let center = image_bounds(cx, 2).center();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: center,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
    assert_eq!(selection(&editor, cx), [(25, 7)]);
    let first = editor.read_with(cx, |e, _| e.menu.as_ref().map(|m| m.items[0].clone()));
    let Some(super::menu::MenuItem::Submenu { label, items }) = first else {
        panic!("the Image width submenu first: {first:?}");
    };
    assert_eq!(label.as_ref(), "Image width");
    let labels: Vec<String> = items
        .iter()
        .map(|i| match i {
            super::menu::MenuItem::Action { label, enabled, .. } => format!("{label} {enabled}"),
            _ => String::new(),
        })
        .collect();
    assert_eq!(
        labels,
        [
            "25% true",
            "50% true",
            "75% true",
            "Full width true",
            "Original size true"
        ]
    );
}

/// US-065: a double click shows the source, the caret at the end of the
/// alt text, before its size.
#[gpui::test]
fn a_double_click_on_an_image_edits_its_alt_text(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let center = image_bounds(cx, 2).center();
    click(cx, center, 1);
    click(cx, center, 2);
    assert_eq!(
        selection(&editor, cx),
        [(10, 10)],
        "after `a`, before `|300`"
    );
    editor.update_in(cx, |e, window, cx| {
        let line = e.layout_line(2, window, cx);
        assert!(line.display.text.contains("![a|300]"), "source shown");
        assert!(!line.block.expect("the image below").replaces);
    });
}

/// US-065: on an image selected as an object, Backspace, Delete and Ctrl+X
/// remove its Markdown, Ctrl+C copies it and a typed character replaces
/// it, each in one undo step that restores the text byte for byte.
#[gpui::test]
fn keys_act_on_an_image_selected_as_an_object(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let center = image_bounds(cx, 2).center();
    for key in ["backspace", "delete"] {
        click(cx, center, 1);
        cx.simulate_keystrokes(key);
        assert_eq!(doc(&editor, cx), "intro\n\n\n\nend", "{key}");
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), note, "{key} undone");
    }
    click(cx, center, 1);
    cx.simulate_keystrokes(&secondary("x"));
    assert_eq!(doc(&editor, cx), "intro\n\n\n\nend");
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("![a|300](wide.png)")
    );
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), note);
    cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    click(cx, center, 1);
    cx.simulate_keystrokes(&secondary("c"));
    assert_eq!(doc(&editor, cx), note, "copy changes nothing");
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("![a|300](wide.png)")
    );
    click(cx, center, 1);
    cx.simulate_input("x");
    assert_eq!(doc(&editor, cx), "intro\n\nx\n\nend");
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), note);
}

/// US-065: arrows onto an image's line show its source as before; Shift+End
/// from its start, or Shift+Home from its end, select it as an object.
#[gpui::test]
fn arrows_reveal_an_image_and_shift_home_end_select_it(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    set_cursor(&editor, cx, 0);
    // Past the blank line, onto the image (`revealBlockOnArrow`).
    cx.simulate_keystrokes("down");
    let head = selection(&editor, cx)[0].1;
    assert!((7..=25).contains(&head), "on the image's line: {head}");
    let replaced = |editor: &Entity<Editor>, cx: &mut VisualTestContext| {
        editor.update_in(cx, |e, window, cx| {
            e.layout_line(2, window, cx)
                .block
                .is_some_and(|b| b.replaces)
        })
    };
    assert!(!replaced(&editor, cx), "the source shows");
    set_cursor(&editor, cx, 7);
    cx.simulate_keystrokes("shift-end");
    assert_eq!(selection(&editor, cx), [(7, 25)]);
    assert!(replaced(&editor, cx), "selected as an object");
    set_cursor(&editor, cx, 25);
    cx.simulate_keystrokes("shift-home");
    assert_eq!(selection(&editor, cx), [(25, 7)]);
    assert!(replaced(&editor, cx));
}

/// US-067 (and the US-066 assumption): a handle shows on hover, a press on
/// it neither selects the image nor moves the caret, the gesture changes
/// the view only, and the release writes the width in one undo step.
#[gpui::test]
fn dragging_a_handle_resizes_the_image_in_one_undo_step(cx: &mut TestAppContext) {
    let note = "intro\n\n![a](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let v = viewport(&editor, cx);
    let column = v.column_width;
    let image = image_bounds(cx, 2);
    assert_eq!(image.size.width, px(column));
    hover(cx, image.center());
    let handle = handle_bounds(cx, "right", 2).expect("a right handle");
    assert!(handle.size.width >= px(24.0) && handle.size.height >= px(24.0));
    assert!(handle.right() <= image.right() && handle.left() >= image.left());
    let left = handle_bounds(cx, "left", 2).expect("a left handle");
    assert_eq!(left.left(), image.left());
    let (before, revision) =
        editor.read_with(cx, |e, _| (e.state.selection().clone(), e.state.revision()));
    let start = handle.center();
    press(cx, start, 1, false);
    cx.run_until_parked();
    editor.read_with(cx, |e, _| {
        assert_eq!(*e.state.selection(), before, "the press selects nothing");
        assert!(matches!(e.drag, Some(super::mouse::Drag::Resize(_))));
    });
    let shrink = (column - 300.0) / 2.0;
    drag_to(cx, point(start.x - px(shrink / 2.0), start.y), true);
    drag_to(cx, point(start.x - px(shrink), start.y + px(40.0)), true);
    cx.run_until_parked();
    let preview = image_bounds(cx, 2);
    assert_eq!(preview.size.width, px(300.0), "twice the move");
    assert_eq!(preview.origin.x, v.column_left + px((column - 300.0) / 2.0));
    editor.read_with(cx, |e, _| {
        assert_eq!(e.state.doc(), note, "the buffer is untouched");
        assert_eq!(e.state.revision(), revision);
        assert!(!e.state.is_dirty());
    });
    release(cx, point(start.x - px(shrink), start.y));
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "intro\n\n![a|300](wide.png)\n\nend");
    assert_eq!(image_bounds(cx, 2).size.width, px(300.0));
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), note, "one undo step");
}

/// US-067: Escape during the gesture puts the width back, and the release
/// writes nothing.
#[gpui::test]
fn escape_cancels_a_resize(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let start = handle_bounds(cx, "right", 2).expect("handle").center();
    press(cx, start, 1, false);
    drag_to(cx, point(start.x + px(60.0), start.y), true);
    cx.run_until_parked();
    assert_eq!(image_bounds(cx, 2).size.width, px(420.0));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(image_bounds(cx, 2).size.width, px(300.0), "back");
    drag_to(cx, point(start.x + px(90.0), start.y), true);
    release(cx, point(start.x + px(90.0), start.y));
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), note);
    assert_eq!(image_bounds(cx, 2).size.width, px(300.0));
}

/// US-067: a release within 8 px of the default width removes the size;
/// the width stops at 64 px.
#[gpui::test]
fn a_resize_snaps_to_the_default_width_and_stops_at_64_px(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let column = viewport(&editor, cx).column_width;
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let start = handle_bounds(cx, "right", 2).expect("handle").center();
    // To 4 px short of the column, the default width of a wide image.
    let to = point(start.x + px((column - 4.0 - 300.0) / 2.0), start.y);
    press(cx, start, 1, false);
    drag_to(cx, to, true);
    release(cx, to);
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "intro\n\n![a](wide.png)\n\nend");
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), note);
    // The left handle, dragged right past the center: 64 px.
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let start = handle_bounds(cx, "left", 2).expect("handle").center();
    let to = point(start.x + px(500.0), start.y);
    press(cx, start, 1, false);
    drag_to(cx, to, true);
    cx.run_until_parked();
    assert_eq!(image_bounds(cx, 2).size.width, px(64.0));
    release(cx, to);
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), "intro\n\n![a|64](wide.png)\n\nend");
}

/// US-067: a source changed during the gesture (a reload from disk) is
/// left alone by the release.
#[gpui::test]
fn a_resize_over_a_changed_source_writes_nothing(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let start = handle_bounds(cx, "right", 2).expect("handle").center();
    press(cx, start, 1, false);
    drag_to(cx, point(start.x + px(40.0), start.y), true);
    let reloaded = "intro, reloaded\n\n![a|300](wide.png)\n\nend";
    editor.update(cx, |e, cx| e.set_text(reloaded, cx));
    release(cx, point(start.x + px(40.0), start.y));
    cx.run_until_parked();
    assert_eq!(doc(&editor, cx), reloaded);
}

/// US-067: an image selected as an object shows its handles without the
/// pointer on it; a hidden handle takes no press (US-066: the handle is
/// a child of the image that the pointer's arrival shows).
#[gpui::test]
fn handles_show_on_hover_or_while_the_image_is_selected(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let image = image_bounds(cx, 2);
    let start = handle_bounds(cx, "right", 2).expect("laid out").center();
    // Not hovered yet: the press reaches the image, which it selects.
    hover(cx, point(px(5.0), px(5.0)));
    press(cx, start, 1, false);
    release(cx, start);
    cx.run_until_parked();
    assert_eq!(selection(&editor, cx), [(25, 7)]);
    assert!(editor.read_with(cx, |e, _| e.drag.is_none()));
    // Selected, the pointer away: the handle takes the press.
    hover(cx, point(px(5.0), px(5.0)));
    press(cx, start, 1, false);
    assert!(editor.read_with(cx, |e, _| matches!(
        e.drag,
        Some(super::mouse::Drag::Resize(_))
    )));
    release(cx, start);
    cx.run_until_parked();
    assert_eq!(
        doc(&editor, cx),
        note,
        "a press without a move writes nothing"
    );
    let _ = image;
}

/// US-067: no handles on an inline image, a missing image, an image whose
/// source shows, during a text drag or a file drag.
#[gpui::test]
fn handles_are_absent_where_they_do_not_apply(cx: &mut TestAppContext) {
    let note = "see ![c](small.png) here\n\n![m](missing.png)\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    assert!(handle_bounds(cx, "right", 0).is_none(), "inline");
    assert!(handle_bounds(cx, "right", 2).is_none(), "missing");
    assert!(handle_bounds(cx, "right", 4).is_some(), "a drawn image");
    let image = note.find("![a").unwrap_or(0);
    set_cursor(&editor, cx, image + 2);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(handle_bounds(cx, "right", 4).is_none(), "its source shows");
    set_cursor(&editor, cx, note.len());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let from = point_at(&editor, cx, note.len() - 3);
    press(cx, from, 1, false);
    let to = point_at(&editor, cx, note.len());
    drag_to(cx, to, true);
    cx.run_until_parked();
    assert!(handle_bounds(cx, "right", 4).is_none(), "a text drag");
    release(cx, to);
    cx.run_until_parked();
    editor.update(cx, |e, cx| {
        e.drop_bar = Some(super::import::DropBar { pos: 0, y: 0.0 });
        cx.notify();
    });
    cx.run_until_parked();
    assert!(handle_bounds(cx, "right", 4).is_none(), "a file drag");
}

/// US-068: each preset width writes `|W` on the column (Full width the
/// column, Original size removes the size and is disabled without one), in
/// one undo step; a width near the default one removes the size.
#[gpui::test]
fn preset_widths_write_a_share_of_the_column(cx: &mut TestAppContext) {
    use super::resize::Preset;
    let note = "intro\n\n![s](small.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let column = viewport(&editor, cx).column_width;
    set_cursor(&editor, cx, 9);
    let presets = editor.read_with(cx, |e, _| e.image_presets());
    assert!(presets.contains(&(Preset::Original, false)), "no size yet");
    for (preset, share) in [
        (Preset::Quarter, 0.25),
        (Preset::Half, 0.5),
        (Preset::ThreeQuarters, 0.75),
        (Preset::Full, 1.0),
    ] {
        editor.update(cx, |e, cx| assert!(e.set_image_width_preset(preset, cx)));
        let width = (column.floor() * share).round() as u32;
        assert_eq!(
            doc(&editor, cx),
            format!("intro\n\n![s|{width}](small.png)\n\nend"),
            "{preset:?}"
        );
        cx.simulate_keystrokes(&secondary("z"));
        assert_eq!(doc(&editor, cx), note, "{preset:?} undone");
    }
    // Original size, from the menu, on an image that has one.
    let sized = "intro\n\n![s|120](small.png)\n\nend";
    editor.update(cx, |e, cx| e.set_text(sized, cx));
    set_cursor(&editor, cx, 9);
    let presets = editor.read_with(cx, |e, _| e.image_presets());
    assert!(presets.contains(&(Preset::Original, true)));
    editor.update_in(cx, |e, window, cx| {
        e.run_menu_action(
            super::menu::MenuAction::ImageWidth(Preset::Original, 7..26),
            window,
            cx,
        )
    });
    assert_eq!(doc(&editor, cx), note);
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), sized);
    // 200 px is the default width of `small.png`: within 8 px, no size.
    editor.update(cx, |e, cx| e.set_text("![s|120](small.png)\n\nend", cx));
    set_cursor(&editor, cx, 2);
    let narrow = column.floor() * 0.25;
    if ((narrow).round() - 200.0).abs() < 8.0 {
        editor.update(cx, |e, cx| e.set_image_width_preset(Preset::Quarter, cx));
        assert_eq!(doc(&editor, cx), "![s](small.png)\n\nend");
    }
}

/// US-068: an image whose natural size is unknown (unreadable) takes its
/// share of the column all the same.
#[gpui::test]
fn a_preset_width_applies_to_an_image_of_unknown_size(cx: &mut TestAppContext) {
    use super::resize::Preset;
    let note = "![x](bad.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let column = viewport(&editor, cx).column_width;
    set_cursor(&editor, cx, 2);
    editor.update(cx, |e, cx| {
        assert!(e.set_image_width_preset(Preset::Half, cx))
    });
    let half = (column.floor() * 0.5).round() as u32;
    assert_eq!(doc(&editor, cx), format!("![x|{half}](bad.png)\n\nend"));
}

// EP-014: image actions and the hover toolbar --------------------------

/// The labels of the open menu's top level, `---` for a separator, with
/// `false` after a disabled entry.
fn menu_labels(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> Vec<String> {
    editor.read_with(cx, |e, _| {
        e.menu
            .as_ref()
            .expect("menu open")
            .items
            .iter()
            .map(|i| match i {
                super::menu::MenuItem::Action { label, enabled, .. } if !enabled => {
                    format!("{label} false")
                }
                super::menu::MenuItem::Action { label, .. }
                | super::menu::MenuItem::Submenu { label, .. } => label.to_string(),
                super::menu::MenuItem::Separator => "---".to_owned(),
            })
            .collect()
    })
}

fn right_click(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
}

fn image_action(
    editor: &Entity<Editor>,
    cx: &mut VisualTestContext,
    action: super::image_actions::ImageAction,
    range: std::ops::Range<usize>,
) {
    editor.update_in(cx, |e, window, cx| {
        e.run_menu_action(super::menu::MenuAction::Image(action, range), window, cx)
    });
    cx.run_until_parked();
}

/// The format and bytes of the image on the clipboard.
fn copied(cx: &mut VisualTestContext) -> Option<(ImageFormat, Vec<u8>)> {
    match cx
        .read_from_clipboard()
        .and_then(|item| item.entries().first().cloned())
    {
        Some(ClipboardEntry::Image(image)) => Some((image.format, image.bytes().to_vec())),
        _ => None,
    }
}

/// US-069: on an image the menu starts with the Image section, then the
/// text entries; Copy and Open are off for a missing file.
#[gpui::test]
fn the_image_menu_starts_with_its_section(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\n![m](missing.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let center = image_bounds(cx, 2).center();
    right_click(cx, center);
    let labels = menu_labels(&editor, cx);
    assert_eq!(
        labels[..10],
        [
            "Image width",
            "Copy image",
            "Open image",
            "Edit source",
            "Delete image",
            "---",
            "Cut",
            "Copy",
            "Paste false",
            "---"
        ]
    );
    // The missing image shows its source: the caret in it.
    let missing = note.find("![m").unwrap_or(0);
    set_cursor(&editor, cx, missing + 2);
    let at = point_at(&editor, cx, missing + 2);
    right_click(cx, at);
    let labels = menu_labels(&editor, cx);
    assert_eq!(
        labels[..5],
        [
            "Image width",
            "Copy image false",
            "Open image false",
            "Edit source",
            "Delete image"
        ]
    );
}

/// US-069: Copy image reads the file off the main thread and puts it on
/// the clipboard in its format, SVG included; a file deleted since the
/// menu opened gives a notice.
#[gpui::test]
fn copy_image_puts_the_file_on_the_clipboard(cx: &mut TestAppContext) {
    use super::image_actions::ImageAction;
    let note = "![a](small.png)\n\n![s](pic.svg)\n\nend";
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"></svg>"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("small.png"), png(200, 120)).unwrap();
    std::fs::write(dir.path().join("pic.svg"), svg).unwrap();
    let (editor, cx) = open_at(
        cx,
        note,
        Some(dir.path().join("note.md")),
        EditorConfig::default(),
    );
    set_cursor(&editor, cx, note.len());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    image_action(&editor, cx, ImageAction::Copy, 0..15);
    assert_eq!(
        copied(cx),
        Some((ImageFormat::Png, png(200, 120))),
        "the PNG file"
    );
    image_action(&editor, cx, ImageAction::Copy, 17..30);
    assert_eq!(copied(cx), Some((ImageFormat::Svg, svg.to_vec())), "SVG");
    assert_eq!(doc(&editor, cx), note, "the text stays");
    std::fs::remove_file(dir.path().join("small.png")).unwrap();
    image_action(&editor, cx, ImageAction::Copy, 0..15);
    assert_eq!(
        notice(&editor, cx).as_deref(),
        Some("Image not found: small.png. Restore the file or edit the image's source.")
    );
    assert_eq!(copied(cx).map(|c| c.0), Some(ImageFormat::Svg), "unchanged");
    // A file that no longer reads as an image.
    std::fs::write(dir.path().join("pic.svg"), b"garbage").unwrap();
    image_action(&editor, cx, ImageAction::Copy, 17..30);
    assert_eq!(
        notice(&editor, cx).as_deref(),
        Some(
            "Can't copy pic.svg: the file can't be read as an image. Check it or edit the image's source."
        )
    );
}

/// US-069: Open image confirms before opening a local file (FR-14), says
/// so when the file is gone, opens a remote image in the browser and is
/// off for a `data:` URI, whose bytes Copy image still takes.
#[gpui::test]
fn open_image_follows_fr_14(cx: &mut TestAppContext) {
    use super::image_actions::ImageAction;
    let (editor, cx, dir) = image_editor(cx, "");
    let url = "https://example.com/a.png";
    let data = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    // Measured before the note draws it: no request leaves the test.
    let remote = std::sync::Arc::new(Image::from_bytes(ImageFormat::Png, png(3, 2)));
    editor.update(cx, |e, _| {
        e.images.finish(
            url.to_owned(),
            Some((
                (3.0, 2.0),
                super::images::Loaded::Bytes {
                    image: remote,
                    file: None,
                },
            )),
        );
    });
    let note = format!("![a](wide.png)\n\n![r]({url})\n\n![d]({data})\n\nend");
    editor.update(cx, |e, cx| e.set_text(&note, cx));
    set_cursor(&editor, cx, note.len());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let range = |name: &str| {
        let start = note.find(name).unwrap_or(0);
        start..note[start..].find(')').map_or(start, |end| start + end + 1)
    };
    image_action(&editor, cx, ImageAction::Open, range("![a"));
    assert!(cx.has_pending_prompt(), "a local file asks first");
    cx.simulate_prompt_answer("Cancel");
    image_action(&editor, cx, ImageAction::Open, range("![r"));
    assert_eq!(cx.opened_url().as_deref(), Some(url));
    let d = range("![d");
    set_cursor(&editor, cx, d.start + 2);
    let actions = editor.read_with(cx, |e, _| e.image_actions());
    assert_eq!(
        actions,
        [
            (ImageAction::Copy, true),
            (ImageAction::Open, false),
            (ImageAction::EditSource, true),
            (ImageAction::Delete, true)
        ]
    );
    image_action(&editor, cx, ImageAction::Copy, d);
    assert_eq!(copied(cx).map(|c| c.0), Some(ImageFormat::Png));
    std::fs::remove_file(dir.path().join("wide.png")).unwrap();
    image_action(&editor, cx, ImageAction::Open, range("![a"));
    assert!(!cx.has_pending_prompt());
    assert_eq!(
        notice(&editor, cx).as_deref(),
        Some("Image not found: wide.png. Restore the file or edit the image's source.")
    );
}

/// US-069: Edit source shows the source, the caret at the end of the alt
/// text; Delete image removes its Markdown in one undo step and leaves its
/// file.
#[gpui::test]
fn edit_source_and_delete_image_act_on_the_text(cx: &mut TestAppContext) {
    use super::image_actions::ImageAction;
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, dir) = image_editor(cx, note);
    image_action(&editor, cx, ImageAction::EditSource, 7..25);
    assert_eq!(
        selection(&editor, cx),
        [(10, 10)],
        "after `a`, before `|300`"
    );
    assert_eq!(doc(&editor, cx), note);
    image_action(&editor, cx, ImageAction::Delete, 7..25);
    assert_eq!(doc(&editor, cx), "intro\n\n\n\nend");
    assert!(dir.path().join("wide.png").is_file(), "the file stays");
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), note);
    image_action(&editor, cx, ImageAction::Delete, 7..12);
    assert_eq!(doc(&editor, cx), note, "not an image's node");
}

fn toolbar_bounds(cx: &mut VisualTestContext, ix: usize) -> Option<gpui::Bounds<Pixels>> {
    cx.debug_bounds(selector(format!("image-toolbar-{ix}")))
}

fn toolbar_button(
    cx: &mut VisualTestContext,
    name: &str,
    ix: usize,
) -> Option<gpui::Bounds<Pixels>> {
    cx.debug_bounds(selector(format!("image-toolbar-{name}-{ix}")))
}

/// The toolbar buttons drawn on line `ix`, in order.
fn toolbar_buttons(cx: &mut VisualTestContext, ix: usize) -> Vec<&'static str> {
    let mut buttons: Vec<(Pixels, &'static str)> = ["width", "copy", "open", "edit", "delete"]
        .into_iter()
        .filter_map(|name| toolbar_button(cx, name, ix).map(|b| (b.left(), name)))
        .collect();
    buttons.sort_by(|a, b| a.0.as_f32().total_cmp(&b.0.as_f32()));
    buttons.into_iter().map(|(_, name)| name).collect()
}

fn away(cx: &mut VisualTestContext) {
    hover(cx, point(px(5.0), px(5.0)));
}

/// US-070: hovering a block image shows its toolbar in its top right
/// corner, 8 px from its edges, five 28 px buttons; it stays with the
/// pointer on it, goes when the pointer leaves, shows while the image is
/// selected as an object, and Escape closes it.
#[gpui::test]
fn the_toolbar_shows_on_hover_and_on_an_object_selection(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    assert!(toolbar_bounds(cx, 2).is_none(), "not hovered");
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let bar = toolbar_bounds(cx, 2).expect("shown on hover");
    assert_eq!(bar.right(), image.right() - px(8.0));
    assert_eq!(bar.top(), image.top() + px(8.0));
    assert_eq!(
        toolbar_buttons(cx, 2),
        ["width", "copy", "open", "edit", "delete"]
    );
    for name in ["width", "copy", "open", "edit", "delete"] {
        let button = toolbar_button(cx, name, 2).expect(name);
        assert_eq!(button.size, size(px(28.0), px(28.0)), "{name}");
    }
    // On the toolbar, then away.
    let edit = toolbar_button(cx, "edit", 2).expect("edit").center();
    hover(cx, edit);
    assert!(toolbar_bounds(cx, 2).is_some(), "the pointer is on it");
    away(cx);
    assert!(toolbar_bounds(cx, 2).is_none(), "the pointer left");
    // Selected as an object, the pointer away.
    click(cx, image.center(), 1);
    away(cx);
    assert_eq!(selection(&editor, cx), [(25, 7)]);
    assert!(toolbar_bounds(cx, 2).is_some(), "selected as an object");
    cx.simulate_keystrokes("escape");
    assert!(toolbar_bounds(cx, 2).is_none(), "Escape closes it");
    assert_eq!(
        selection(&editor, cx),
        [(25, 7)],
        "the image stays selected"
    );
    // The pointer coming onto the image shows it again.
    hover(cx, image.center());
    assert!(toolbar_bounds(cx, 2).is_some(), "the pointer came back");
    // Escape with the pointer on the image: closed while it stays there.
    cx.simulate_keystrokes("escape");
    hover(cx, image.center() + point(px(3.0), px(3.0)));
    assert!(toolbar_bounds(cx, 2).is_none(), "the pointer did not leave");
    away(cx);
    hover(cx, image.center());
    assert!(toolbar_bounds(cx, 2).is_some(), "left, then back");
}

/// US-070: a toolbar button runs its action on its image and leaves the
/// caret where it was: Copy image, the width submenu under its button,
/// Delete image.
#[gpui::test]
fn toolbar_buttons_act_without_moving_the_caret(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    set_cursor(&editor, cx, 2);
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let copy = toolbar_button(cx, "copy", 2).expect("copy").center();
    hover(cx, copy);
    click(cx, copy, 1);
    assert_eq!(selection(&editor, cx), [(2, 2)], "the caret stays");
    assert_eq!(copied(cx), Some((ImageFormat::Png, png(1600, 900))));
    assert!(editor.update_in(cx, |e, window, _| e.focus_handle.is_focused(window)));
    // Image width: its submenu opens under the button.
    let width = toolbar_button(cx, "width", 2).expect("width");
    click(cx, width.center(), 1);
    let at = editor
        .read_with(cx, |e, _| e.menu.as_ref().map(|m| m.popup.position))
        .expect("submenu");
    assert_eq!(at.x, width.left());
    assert!(at.y > width.bottom(), "under the button");
    let labels = menu_labels(&editor, cx);
    assert_eq!(labels, ["25%", "50%", "75%", "Full width", "Original size"]);
    let half = cx.debug_bounds("menu-50%").expect("drawn").center();
    hover(cx, half);
    click(cx, half, 1);
    let column = viewport(&editor, cx).column_width;
    let w = (column.floor() * 0.5).round() as u32;
    let sized = format!("intro\n\n![a|{w}](wide.png)\n\nend");
    assert_eq!(doc(&editor, cx), sized);
    assert_eq!(selection(&editor, cx), [(2, 2)]);
    // Delete image: the Markdown goes, in one undo step.
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let delete = toolbar_button(cx, "delete", 2).expect("delete").center();
    hover(cx, delete);
    click(cx, delete, 1);
    assert_eq!(doc(&editor, cx), "intro\n\n\n\nend");
    assert_eq!(selection(&editor, cx), [(2, 2)]);
    cx.simulate_keystrokes(&secondary("z"));
    assert_eq!(doc(&editor, cx), sized);
}

/// US-070: on an image narrower than the toolbar, the toolbar keeps to
/// the image's right edge and grows to the left, inside the editor.
#[gpui::test]
fn the_toolbar_of_a_narrow_image_grows_to_the_left(cx: &mut TestAppContext) {
    let note = "intro\n\n![a|64](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let image = image_bounds(cx, 2);
    assert_eq!(image.size.width, px(64.0));
    hover(cx, image.center());
    let bar = toolbar_bounds(cx, 2).expect("shown");
    assert_eq!(bar.right(), image.right() - px(8.0));
    assert!(bar.left() < image.left(), "wider than the image");
    // The pointer on the part past the image keeps it.
    hover(cx, point(bar.left() + px(4.0), bar.center().y));
    assert!(toolbar_bounds(cx, 2).is_some());
    // An editor narrower than the toolbar: it stops at the editor's edge.
    away(cx);
    cx.simulate_resize(size(px(150.0), px(800.0)));
    cx.run_until_parked();
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let bar = toolbar_bounds(cx, 2).expect("shown");
    let left = viewport(&editor, cx).bounds.left();
    assert!(bar.left() >= left, "{:?} inside {left:?}", bar.left());
}

/// US-070: an image not measured keeps Edit source and Delete image; no
/// toolbar on an inline image, a revealed source, during a resize, a text
/// drag or a file drag.
#[gpui::test]
fn the_toolbar_is_absent_where_it_does_not_apply(cx: &mut TestAppContext) {
    let note = "see ![c](small.png) here\n\n![x](bad.png)\n\n![a|300](wide.png)\n\nend";
    let (editor, cx, _dir) = image_editor(cx, note);
    let bad = image_bounds(cx, 2).center();
    hover(cx, bad);
    assert_eq!(toolbar_buttons(cx, 2), ["edit", "delete"], "unreadable");
    let inline = point_at(&editor, cx, 5);
    hover(cx, inline);
    assert!(toolbar_bounds(cx, 0).is_none(), "inline");
    let image = image_bounds(cx, 4);
    hover(cx, image.center());
    assert!(toolbar_bounds(cx, 4).is_some(), "a drawn image");
    // During a resize.
    let start = handle_bounds(cx, "right", 4).expect("handle").center();
    press(cx, start, 1, false);
    drag_to(cx, point(start.x - px(20.0), start.y), true);
    assert!(toolbar_bounds(cx, 4).is_none(), "resizing");
    cx.simulate_keystrokes("escape");
    release(cx, start);
    cx.run_until_parked();
    // Its source revealed.
    let at = note.find("![a").unwrap_or(0) + 2;
    set_cursor(&editor, cx, at);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(toolbar_bounds(cx, 4).is_none(), "its source shows");
    set_cursor(&editor, cx, note.len());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    hover(cx, image.center());
    assert!(toolbar_bounds(cx, 4).is_some());
    // A file drag.
    editor.update(cx, |e, cx| {
        e.drop_bar = Some(super::import::DropBar { pos: 0, y: 0.0 });
        cx.notify();
    });
    cx.run_until_parked();
    assert!(toolbar_bounds(cx, 4).is_none(), "a file drag");
    editor.update(cx, |e, cx| {
        e.drop_bar = None;
        cx.notify();
    });
    cx.run_until_parked();
    // A text drag.
    let from = point_at(&editor, cx, note.len() - 3);
    press(cx, from, 1, false);
    drag_to(cx, image.center(), true);
    cx.run_until_parked();
    assert!(toolbar_bounds(cx, 4).is_none(), "a text drag");
    release(cx, image.center());
    cx.run_until_parked();
}

/// US-070: a toolbar whose image goes without the pointer leaving it (a
/// scroll, an edit) does not hold the hover: the next image hovered shows
/// its own toolbar.
#[gpui::test]
fn a_toolbar_gone_under_the_pointer_lets_the_next_image_show_its_own(cx: &mut TestAppContext) {
    let note = "intro

![a|300](wide.png)

![[small.png|100]]

end";
    let (editor, cx, _dir) = image_editor(cx, note);
    let first = image_bounds(cx, 2).center();
    hover(cx, first);
    let copy = toolbar_button(cx, "copy", 2).expect("copy").center();
    hover(cx, copy);
    // The first image goes, its toolbar with it, the pointer still.
    editor.update(cx, |e, cx| {
        e.set_text(
            "intro

plain text

![[small.png|100]]

end",
            cx,
        )
    });
    cx.run_until_parked();
    assert!(toolbar_bounds(cx, 2).is_none());
    let embed = image_bounds(cx, 4).center();
    hover(cx, embed);
    assert!(
        toolbar_bounds(cx, 4).is_some(),
        "the embed shows its toolbar"
    );
}

/// US-070: Escape with the pointer on the toolbar closes it; the image
/// under the pointer does not bring it back until the pointer leaves.
#[gpui::test]
fn escape_on_the_toolbar_closes_it_until_the_pointer_leaves(cx: &mut TestAppContext) {
    let note = "intro

![a|300](wide.png)

end";
    let (_editor, cx, _dir) = image_editor(cx, note);
    let image = image_bounds(cx, 2);
    hover(cx, image.center());
    let copy = toolbar_button(cx, "copy", 2).expect("copy").center();
    hover(cx, copy);
    cx.simulate_keystrokes("escape");
    assert!(toolbar_bounds(cx, 2).is_none(), "Escape closes it");
    hover(cx, copy + point(px(1.0), px(1.0)));
    assert!(toolbar_bounds(cx, 2).is_none(), "the pointer did not leave");
    away(cx);
    hover(cx, image.center());
    assert!(toolbar_bounds(cx, 2).is_some(), "left, then back");
}
