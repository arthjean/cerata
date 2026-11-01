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

    const DOC: &str =
        "intro\n\n| Link | N |\n|:--|--:|\n| [site](https://example.test) | 2 |\n\nend";

    fn table_from() -> usize {
        DOC.find("| Link").unwrap()
    }

    /// US-032: folded while the caret is away, at its measured height, with
    /// its other lines hidden; a link in a cell opens without moving the
    /// caret; a click elsewhere selects the table, which shows its source.
    #[gpui::test]
    fn a_table_folds_measures_and_reveals_its_source(cx: &mut TestAppContext) {
        let (editor, cx) = open(cx, DOC);
        set_cursor(&editor, cx, DOC.len());
        cx.run_until_parked();
        let (first, rest) = editor.update_in(cx, |editor, window, cx| {
            let first = editor.layout_line(2, window, cx);
            let rest: Vec<f32> = (3..5)
                .map(|ix| editor.layout_line(ix, window, cx).height)
                .collect();
            (first, rest)
        });
        let block = first.block.as_ref().expect("table block");
        assert!(matches!(
            block.kind,
            BlockKind::Rich(cerata_markdown::Widget::Table { .. })
        ));
        assert!(block.replaces);
        assert_eq!(rest, vec![0.0, 0.0], "the other rows are folded away");
        let header = cx
            .debug_bounds("table-header-0")
            .expect("header cell drawn");
        let cell = cx.debug_bounds("table-cell-0").expect("body cell drawn");
        assert!(cell.top() >= header.bottom() - px(1.0));
        // The line takes the height the frame measured, not the estimate.
        let measured = cx.debug_bounds("table-cell-1").expect("cell").bottom() - header.top();
        assert!(
            first.height >= measured.as_f32() - 1.0,
            "{} < {measured:?}",
            first.height
        );

        // A link in a cell opens; the caret stays where it was.
        let inside = point(cell.left() + px(16.0), cell.top() + cell.size.height / 2.0);
        press(cx, inside, 1, false);
        release(cx, inside);
        cx.run_until_parked();
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.test"));
        assert_eq!(selection(&editor, cx), vec![(DOC.len(), DOC.len())]);

        // Anywhere else selects the whole table and reveals its source.
        let header = cx.debug_bounds("table-header-1").expect("header");
        let blank = point(header.right() - px(4.0), header.top() + px(4.0));
        press(cx, blank, 1, false);
        release(cx, blank);
        cx.run_until_parked();
        let table_to = DOC.find("\n\nend").unwrap();
        assert_eq!(selection(&editor, cx), vec![(table_to, table_from())]);
        let revealed = editor.update_in(cx, |editor, window, cx| {
            (2..5)
                .map(|ix| editor.layout_line(ix, window, cx).table_source)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            revealed,
            vec![
                Some((true, false)),
                Some((false, false)),
                Some((false, true))
            ]
        );
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
