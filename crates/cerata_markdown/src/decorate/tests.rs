// Expected values are written as lists of ranges.
#![allow(clippy::single_range_in_vec_init)]

use super::*;
use crate::Change;
use crate::motion::Vertical;
use crate::selection::SelectionRange;
use crate::state::{EditorState, Transaction};

fn at(doc: &str, caret: usize) -> Vec<Decoration> {
    let state = EditorState::with_filters(doc, Vec::new());
    decorate(state.tree(), state.text(), &Selection::cursor(caret), None)
}

fn hidden(decorations: &[Decoration]) -> Vec<Range<usize>> {
    decorations
        .iter()
        .filter_map(|d| match d {
            Decoration::Hide { range, .. } => Some(range.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn atx_marks_hide_with_their_space_unless_touched() {
    let doc = "# Title\n\nbody";
    assert_eq!(hidden(&at(doc, 12)), [0..2]);
    assert!(
        hidden(&at(doc, 7)).is_empty(),
        "the caret at the end touches the heading"
    );
    // Contact is inclusive on every range of the selection.
    let state = EditorState::with_filters(doc, Vec::new());
    let two = Selection::new(
        vec![SelectionRange::cursor(12), SelectionRange::cursor(0)],
        0,
    );
    assert!(hidden(&decorate(state.tree(), state.text(), &two, None)).is_empty());
}

#[test]
fn setext_underline_hides_as_a_block() {
    let doc = "Title\n===\n\nbody";
    assert!(at(doc, 14).contains(&Decoration::Hide {
        range: 6..9,
        mode: HideMode::Block
    }));
}

#[test]
fn emphasis_strike_and_code_keep_their_style() {
    let doc = "**b** ~~s~~ `c`\n\nx";
    let d = at(doc, 17);
    assert!(d.contains(&Decoration::Mark {
        range: 0..5,
        style: MarkStyle::Emphasis
    }));
    assert!(d.contains(&Decoration::Mark {
        range: 6..11,
        style: MarkStyle::Strikethrough
    }));
    assert!(d.contains(&Decoration::Mark {
        range: 12..15,
        style: MarkStyle::InlineCode
    }));
    assert_eq!(hidden(&d), [0..2, 3..5, 6..8, 9..11, 12..13, 14..15]);
    // Touched: marks shown, styles kept.
    let touched = at(doc, 5);
    assert!(touched.contains(&Decoration::Mark {
        range: 0..5,
        style: MarkStyle::Emphasis
    }));
    assert_eq!(hidden(&touched), [6..8, 9..11, 12..13, 14..15]);
}

// Ported from Writer `tests/hide-links.test.ts`.
#[test]
fn hides_the_title_of_a_titled_link_along_with_its_marks_and_url() {
    let doc = "see [text](http://x.y \"the title\") ok";
    let d = at(doc, 0);
    let title = doc.find("\"the title\"").unwrap();
    assert!(hidden(&d).contains(&(title..title + "\"the title\"".len())));
    assert!(hidden(&d).contains(&(11..21)), "URL");
    assert!(d.contains(&Decoration::Mark {
        range: 4..34,
        style: MarkStyle::RenderedLink
    }));
    assert!(!at(doc, 6).iter().any(|d| matches!(
        d,
        Decoration::Mark {
            style: MarkStyle::RenderedLink,
            ..
        }
    )));
}

#[test]
fn autolinks_hide_only_their_angle_brackets() {
    let doc = "a <https://x.y> b";
    assert_eq!(hidden(&at(doc, 0)), [2..3, 14..15]);
}

#[test]
fn an_escape_shows_while_its_word_is_touched() {
    let doc = "foo\\*bar baz";
    assert_eq!(hidden(&at(doc, 10)), [3..4]);
    assert!(hidden(&at(doc, 7)).is_empty(), "same WORD");
    // A lone escaped space: the zone is the whole line.
    let lone = "a \\  b\nnext";
    assert!(hidden(&at(lone, 6)).is_empty());
    assert_eq!(hidden(&at(lone, 9)), [2..3]);
}

#[test]
fn fence_marks_and_quote_marks_keep_their_width() {
    let doc = "```rust\nx\n```\n\n> q\n\nend";
    let d = at(doc, doc.len());
    for range in [0..3, 3..7, 10..13, 15..16] {
        assert!(
            d.contains(&Decoration::Hide {
                range,
                mode: HideMode::KeepSpace
            }),
            "{d:?}"
        );
    }
}

// Ported from Writer `tests/heading-decorations.test.ts`.
#[test]
fn heading_lines_get_their_level_scale_and_weight() {
    let doc = "# a\n## b\n### c\n#### d\n##### e\n###### f\ng\n-\nh\n=";
    let lines: Vec<(usize, LineStyle)> = at(doc, 0)
        .into_iter()
        .filter_map(|d| match d {
            Decoration::Line { line_start, style } => Some((line_start, style)),
            _ => None,
        })
        .collect();
    let levels: Vec<u8> = lines
        .iter()
        .filter_map(|(_, s)| match s {
            LineStyle::Heading(l) => Some(*l),
            _ => None,
        })
        .collect();
    assert_eq!(levels, [1, 2, 3, 4, 5, 6, 1]);
    let scales: Vec<f32> = lines.iter().take(6).map(|(_, s)| s.scale()).collect();
    assert_eq!(scales, [1.6, 1.4, 1.2, 1.1, 1.0, 0.9]);
    assert!(lines.iter().all(|(_, s)| s.weight() == 600));
    assert_eq!(NodeKind::Paragraph.heading_level(), None);
}

#[test]
fn a_selection_past_the_end_is_clamped() {
    let doc = "# T\n\n**b**";
    let state = EditorState::with_filters(doc, Vec::new());
    let far = Selection::new(vec![SelectionRange::new(4, 1_000)], 0);
    let clamped = Selection::new(vec![SelectionRange::new(4, doc.len())], 0);
    assert_eq!(
        decorate(state.tree(), state.text(), &far, None),
        decorate(state.tree(), state.text(), &clamped, None)
    );
}

// Ported from Writer `tests/image-fold.test.ts:52-70`.
#[test]
fn image_previews_wait_for_the_caret_to_leave_the_destination() {
    let doc = "before\n\n![diagram](assets/diagram.png)\n\nafter";
    let widgets = |caret| {
        at(doc, caret)
            .iter()
            .filter(|d| matches!(d.widget(), Some(Widget::Image { .. })))
            .count()
    };
    let url = doc.find("assets").unwrap();
    let end = doc.find(')').unwrap() + 1;
    assert_eq!(widgets(url + 3), 0);
    assert_eq!(widgets(end), 1);
    assert_eq!(widgets(doc.find("diagram").unwrap() + 2), 1);
    // Untouched, it replaces its line; touched, it follows the source.
    let image = 8..end;
    let url_string = "assets/diagram.png".to_owned();
    assert!(at(doc, 0).contains(&Decoration::Replace {
        range: image.clone(),
        widget: Widget::Image {
            url: url_string.clone()
        },
        block: true
    }));
    assert!(at(doc, end).contains(&Decoration::Widget {
        pos: end,
        widget: Widget::Image { url: url_string },
        block: true
    }));
    // Inline when it shares its line.
    let inline = "see ![a](b.png) here";
    assert!(at(inline, 0).iter().any(|d| matches!(
        d,
        Decoration::Replace {
            block: false,
            widget: Widget::Image { .. },
            ..
        }
    )));
}

#[test]
fn rules_dashes_and_emoji_fold_and_unknown_codes_stay_text() {
    let doc = "a -- b --- c :smile: :nope:\n\n---\n\nz";
    let d = at(doc, doc.len());
    let glyphs: Vec<&str> = d
        .iter()
        .filter_map(|d| d.widget().and_then(Widget::glyph))
        .collect();
    assert_eq!(glyphs, ["\u{2013}", "\u{2014}", "\u{1f604}"]);
    assert!(d.contains(&Decoration::Replace {
        range: 29..32,
        widget: Widget::HorizontalRule,
        block: true
    }));
    assert!(
        !d.iter().any(|d| d.range() == (21..27)),
        ":nope: is not an emoji"
    );
    assert_eq!(emoji("smile"), Some("\u{1f604}"));
    assert_eq!(emoji(":smile:"), Some("\u{1f604}"));
    assert_eq!(emoji("nope"), None);
    // Touched: the source shows.
    assert!(!at(doc, 3).iter().any(|d| d.range() == (2..4)));
}

// Ported from Writer `tests/drag-selection-gate.test.ts:38-79`.
#[test]
fn a_drag_freezes_what_is_revealed_until_it_ends() {
    let doc = "# heading\nplain";
    let mut state = EditorState::with_filters(doc, Vec::new());
    state.select(Selection::cursor(12)).unwrap();
    let baseline = state.decorations();
    assert_eq!(hidden(&baseline), [0..2]);
    assert!(!state.is_dragging());

    state.begin_drag();
    assert!(state.is_dragging());
    // The live selection enters the heading: nothing is revealed yet.
    state.select(Selection::cursor(1)).unwrap();
    assert_eq!(state.decorations(), baseline);

    // Edits during the drag remap the frozen set.
    state
        .apply(Transaction::new().change(Change::insert(0, "xyz\n")))
        .unwrap();
    assert_eq!(hidden(&state.decorations()), [4..6]);

    // Release: decorations follow the live selection.
    state.end_drag();
    assert!(hidden(&state.decorations()).is_empty());
}

#[test]
fn arrows_enter_an_adjacent_block_widget_in_the_same_column() {
    let doc = "abcdef\n\n---\n\nxyz";
    let mut state = EditorState::with_filters(doc, Vec::new());
    state.select(Selection::cursor(2)).unwrap();
    assert!(state.reveal_block_on_arrow(Vertical::Down));
    assert_eq!(
        state.selection().main().head,
        10,
        "column 2 of the rule's line"
    );
    // From below, upward, onto the rule's last line.
    state.select(Selection::cursor(14)).unwrap();
    assert!(state.reveal_block_on_arrow(Vertical::Up));
    assert_eq!(state.selection().main().head, 9);
    // No widget nearby: the regular vertical move runs.
    state.select(Selection::cursor(16)).unwrap();
    assert!(!state.reveal_block_on_arrow(Vertical::Down));
}

#[test]
fn arrows_right_outside_a_widget_enter_it() {
    let doc = "a\n![x](y.png)\nb";
    let mut state = EditorState::with_filters(doc, Vec::new());
    state.select(Selection::cursor(1)).unwrap();
    assert!(state.reveal_block_on_arrow(Vertical::Down));
    assert_eq!(
        state.selection().main().head,
        3,
        "column 1 of the image line"
    );
}
