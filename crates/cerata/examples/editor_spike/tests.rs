//! GPUI interaction tests on the test platform (deterministic monospace
//! `NoopTextSystem`: every BMP character advances 0.6 em).

use std::path::PathBuf;

use gpui::{
    Entity, EntityInputHandler, Modifiers, MouseButton, ScrollDelta, ScrollEvent, TestAppContext,
    VisualTestContext, point, px,
};

use crate::{
    editor::{BASE_FONT_SIZE, Editor, text_runs},
    fixture,
    markup::{BlockMode, HEADING_SCALES},
};

fn open<'a>(cx: &'a mut TestAppContext, text: &str) -> (Entity<Editor>, &'a mut VisualTestContext) {
    let text = text.to_string();
    let (editor, cx) = cx.add_window_view(|window, cx| {
        let mut editor = Editor::new(text, PathBuf::new(), cx);
        editor.image_sizes.insert("a.svg".into(), (640.0, 251.0));
        editor.image_sizes.insert("b.svg".into(), (1000.0, 397.0));
        editor.reestimate_heights();
        let end = editor.buffer.len();
        editor.selected_range = end..end;
        window.focus(&editor.focus_handle, cx);
        editor
    });
    cx.run_until_parked();
    (editor, cx)
}

const CHAR: f32 = 0.6 * BASE_FONT_SIZE;

#[gpui::test]
fn each_heading_level_is_laid_out_at_its_own_font_size(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "# a\n## b\n### c\n#### d\n##### e\n###### f\nbody\n");
    editor.update_in(cx, |editor, window, _| {
        for level in 1..=6 {
            let line = editor.layout_line(level - 1, window);
            let wrapped = line.wrapped.as_ref().expect("text line");
            let expected = BASE_FONT_SIZE * HEADING_SCALES[level - 1];
            assert_eq!(wrapped.font_size(), px(expected), "H{level}");
            assert_eq!(line.display.text, ["a", "b", "c", "d", "e", "f"][level - 1]);
            let height = line.metrics.pad_top + line.metrics.row_height + line.metrics.pad_bottom;
            assert_eq!(editor.heights.get(level - 1), height as f64);
            let next = editor.line_screen_top(level) - editor.line_screen_top(level - 1);
            assert!((next.as_f32() - height).abs() < 1e-3);
        }
        assert_eq!(editor.heights.get(6), 30.0);
    });
}

#[gpui::test]
fn rendering_is_virtualized_on_a_large_note(cx: &mut TestAppContext) {
    let note = fixture::generate(256 * 1024);
    let (editor, cx) = open(cx, &note);
    editor.update(cx, |editor, cx| {
        editor.selected_range = 0..0;
        cx.notify();
    });
    cx.run_until_parked();
    let (lines, laid_out) = editor.read_with(cx, |editor, _| {
        (editor.buffer.line_count(), editor.laid_out.clone())
    });
    assert!(lines > 2000, "{lines}");
    assert!(laid_out.len() < 120, "laid out {laid_out:?} of {lines}");

    cx.simulate_event(ScrollEvent {
        position: point(px(400.0), px(400.0)),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-50_000.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, _| {
        assert!(editor.laid_out.start > 500, "{:?}", editor.laid_out);
        assert!(editor.laid_out.len() < 120);
        // Hit-testing through the height map lands on the line drawn there.
        let ix = editor.scroll_line + 3;
        let top = editor.line_screen_top(ix);
        let viewport = editor.viewport.expect("painted");
        let offset = editor.offset_for_point(point(viewport.column_left, top + px(1.0)), window);
        assert_eq!(editor.buffer.line_for_offset(offset), ix);
    });
}

#[gpui::test]
fn block_image_is_exactly_as_tall_as_the_image(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "intro line\n![a](a.svg)\nmiddle\n![b](b.svg)\nafter\n");
    let tops = editor.update_in(cx, |editor, window, _| {
        let wrap = editor.viewport.expect("painted").wrap_width.as_f32();
        let mut tops = Vec::new();
        for (ix, (w, h)) in [(1, (640.0f32, 251.0f32)), (3, (1000.0, 397.0))] {
            let line = editor.layout_line(ix, window);
            let block = line.block.as_ref().expect("image block");
            assert_eq!(block.mode, BlockMode::Replace);
            let width = w.min(wrap).floor();
            let expected = (h * width / w).round();
            assert_eq!(block.height, expected);
            assert_eq!(line.height, expected);
            assert_eq!(editor.heights.get(ix), expected as f64);
            assert!(
                (expected / line.metrics.row_height).fract() > 0.01,
                "fixture height must not be a multiple of the row height"
            );
            let top = editor.line_screen_top(ix);
            let next = editor.line_screen_top(ix + 1);
            // Screen positions are f32 sums of f64 prefix sums: compare
            // them at sub-pixel tolerance; the item height itself is exact.
            assert!((next - top - px(expected)).abs() < px(1e-3));
            tops.push((ix, top, expected));
        }
        tops
    });
    for (ix, top, expected) in tops {
        let bounds = cx
            .debug_bounds(Box::leak(format!("spike-image-{ix}").into_boxed_str()))
            .expect("image widget painted");
        assert_eq!(bounds.size.height, px(expected));
        assert!((bounds.origin.y - top).abs() < px(1e-3));
    }
}

#[gpui::test]
fn drag_across_hidden_marks_and_an_image_selects_the_buffer_range(cx: &mut TestAppContext) {
    let line0 = "plain **bold** and [link](https://example.com/a) end";
    let note = format!("{line0}\n![a](a.svg)\nafter image line\n");
    let (editor, cx) = open(cx, &note);
    let (down, over_image, up, image_end, expected_end) =
        editor.update_in(cx, |editor, window, _| {
            let viewport = editor.viewport.expect("painted");
            let line = editor.layout_line(0, window);
            assert_eq!(line.display.text, "plain bold and link end");
            let down = point(
                viewport.column_left + px(1.0),
                editor.line_screen_top(0) + px(5.0),
            );
            let over_image = point(
                viewport.column_left + px(100.0),
                editor.line_screen_top(1) + px(200.0),
            );
            let up = point(
                viewport.column_left + px(5.0 * CHAR + 1.0),
                editor.line_screen_top(2) + px(5.0),
            );
            (
                down,
                over_image,
                up,
                editor.buffer.line_range(1).end,
                editor.buffer.line_range(2).start + 5,
            )
        });

    cx.simulate_mouse_down(down, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(over_image, MouseButton::Left, Modifiers::none());
    editor.update_in(cx, |editor, window, _| {
        assert_eq!(editor.selected_range, 0..image_end);
        // Frozen while dragging: the selection touches every node of line 0
        // but nothing reflows under the pointer.
        let line = editor.layout_line(0, window);
        assert_eq!(line.display.text, "plain bold and link end");
        assert_eq!(
            editor.layout_line(1, window).block.map(|b| b.mode),
            Some(BlockMode::Replace)
        );
    });
    cx.simulate_mouse_move(up, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(up, MouseButton::Left, Modifiers::none());
    editor.update_in(cx, |editor, window, _| {
        assert_eq!(editor.selected_range, 0..expected_end);
        assert_eq!(
            &editor.buffer.text()[editor.selected_range.clone()],
            &format!("{line0}\n![a](a.svg)\nafter")
        );
        // Released: the selection now touches the nodes, so they reveal.
        assert_eq!(editor.layout_line(0, window).display.text, line0);
        assert_eq!(
            editor.layout_line(1, window).block.map(|b| b.mode),
            Some(BlockMode::Below)
        );
    });
}

#[gpui::test]
fn ime_composition_marks_text_at_the_caret_then_commits(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "## Title\nhello world\n");
    editor.update_in(cx, |editor, window, cx| {
        let caret = editor.buffer.line_range(1).start + "hello ".len();
        editor.move_to(caret, window, cx);
        let caret_utf16 = editor.buffer.offset_to_utf16(caret);

        // What MS-IME, macOS and fcitx5/ibus do while composing.
        editor.replace_and_mark_text_in_range(None, "k", Some(1..1), window, cx);
        assert_eq!(editor.buffer.line_text(1), "hello kworld");
        assert_eq!(
            editor.marked_text_range(window, cx),
            Some(caret_utf16..caret_utf16 + 1)
        );
        editor.replace_and_mark_text_in_range(None, "か", Some(1..1), window, cx);
        editor.replace_and_mark_text_in_range(None, "かん", Some(2..2), window, cx);
        assert_eq!(editor.buffer.line_text(1), "hello かんworld");
        assert_eq!(
            editor.marked_text_range(window, cx),
            Some(caret_utf16..caret_utf16 + 2)
        );
        assert_eq!(editor.marked_range, Some(caret..caret + "かん".len()));
        assert_eq!(
            editor
                .selected_text_range(false, window, cx)
                .map(|s| s.range),
            Some(caret_utf16 + 2..caret_utf16 + 2)
        );

        // Marked text is drawn underlined.
        let line = editor.layout_line(1, window);
        let marked = editor
            .marked_range
            .clone()
            .map(|range| range.start - line.start..range.end - line.start);
        let runs = text_runs(&line.syntax, &line.display, &line.metrics, marked);
        let mut offset = 0;
        let mut underlined = Vec::new();
        for run in &runs {
            if run.underline.is_some_and(|u| u.thickness == px(2.0)) {
                underlined.push(offset..offset + run.len);
            }
            offset += run.len;
        }
        assert_eq!(underlined, vec![6..6 + "かん".len()]);

        // The candidate window is positioned from the caret bounds.
        let selection = editor
            .selected_text_range(false, window, cx)
            .expect("selection")
            .range;
        let bounds = editor
            .bounds_for_range(selection, Default::default(), window, cx)
            .expect("caret bounds");
        let viewport = editor.viewport.expect("painted");
        let expected_x = viewport.column_left + px(8.0 * CHAR);
        assert!(
            (bounds.origin.x - expected_x).abs() < px(0.01),
            "{bounds:?}"
        );
        assert_eq!(bounds.origin.y, editor.line_screen_top(1));
        assert_eq!(bounds.size.height, px(30.0));

        // Commit.
        editor.replace_text_in_range(None, "漢", window, cx);
        assert_eq!(editor.buffer.line_text(1), "hello 漢world");
        assert_eq!(editor.marked_text_range(window, cx), None);
        assert_eq!(
            editor.selected_range,
            caret + "漢".len()..caret + "漢".len()
        );
    });
}

#[gpui::test]
fn dead_key_sequences_produce_the_composed_character(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "x\n");
    editor.update_in(cx, |editor, window, cx| {
        editor.move_to(1, window, cx);
        // macOS and Linux show the dead key as marked text, then commit.
        editor.replace_and_mark_text_in_range(None, "^", Some(1..1), window, cx);
        assert_eq!(editor.buffer.line_text(0), "x^");
        editor.replace_text_in_range(None, "ê", window, cx);
        assert_eq!(editor.buffer.line_text(0), "xê");
        // Windows delivers WM_CHAR 'ê' after WM_DEADCHAR: one plain insert.
        editor.replace_text_in_range(None, "ê", window, cx);
        assert_eq!(editor.buffer.line_text(0), "xêê");
        assert_eq!(editor.marked_range, None);
    });
}

#[gpui::test]
fn typing_through_the_input_handler_reveals_and_relayouts(cx: &mut TestAppContext) {
    let (editor, cx) = open(
        cx,
        "intro
some **bold** text
",
    );
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.layout_line(1, window).display.text, "some bold text");
        let inside_bold = editor.buffer.line_range(1).start + "some **bo".len();
        editor.move_to(inside_bold, window, cx);
        editor.replace_text_in_range(None, "x", window, cx);
    });
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        assert_eq!(editor.buffer.line_text(1), "some **boxld** text");
        assert_eq!(
            editor.layout_line(1, window).display.text,
            "some **boxld** text",
            "caret inside the node reveals its marks"
        );
        editor.move_to(0, window, cx);
        assert_eq!(
            editor.layout_line(1, window).display.text,
            "some boxld text"
        );
    });
}
