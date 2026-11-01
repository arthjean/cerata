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

#[test]
fn ranged_decorations_match_the_whole_document_line_by_line() {
    let doc = "# Title\n\nSome **bold** and `code`, a [link](u) --\n\n> quote\n> > nested\n\n```rust\nfn a() {}\n\tx\n```\n\n- [ ] task\n- item :smile:\n\n![img](a.png)\n\n---\ntail\twith tab\n";
    let tree = MarkdownTree::parse(doc);
    let text = Text::new(doc);
    let selection = Selection::cursor(10);
    let all = decorate(&tree, &text, &selection, None);
    let on_line = |decorations: &[Decoration], line: crate::text::Line| -> Vec<Decoration> {
        decorations
            .iter()
            .filter(|d| {
                let r = d.range();
                r.start <= line.to && r.end >= line.from
            })
            .cloned()
            .collect()
    };
    for n in 0..text.line_count() {
        let line = text.line(n);
        let ranged = decorate_range(&tree, &text, &selection, None, line.range());
        assert_eq!(on_line(&ranged, line), on_line(&all, line), "line {n}");
    }
}

/// `table-decorations.test.ts:134-190`.
#[test]
fn tables_fold_to_a_widget_and_unfold_as_source_lines() {
    let before = "before\n";
    let table = "| Name | Count |\n| --- | ---: |\n| Tea | 2 |";
    let doc = format!("{before}{table}\n\nafter");
    let (from, to) = (before.len(), before.len() + table.len());
    let is_table = |d: &Decoration| matches!(d.widget(), Some(Widget::Table { .. }));
    let source_lines = |d: &[Decoration]| -> Vec<LineStyle> {
        d.iter()
            .filter_map(|d| match d {
                Decoration::Line {
                    style: s @ LineStyle::TableSource { .. },
                    ..
                } => Some(*s),
                _ => None,
            })
            .collect()
    };

    let folded = at(&doc, 0);
    let widget = folded.iter().find(|d| is_table(d)).unwrap();
    assert_eq!(widget.range(), from..to);
    assert!(matches!(widget, Decoration::Replace { block: true, .. }));
    assert!(source_lines(&folded).is_empty());

    let touched = at(&doc, from + 2);
    assert!(!touched.iter().any(is_table));
    assert_eq!(
        source_lines(&touched),
        [
            LineStyle::TableSource {
                first: true,
                last: false
            },
            LineStyle::TableSource {
                first: false,
                last: false
            },
            LineStyle::TableSource {
                first: false,
                last: true
            },
        ]
    );

    // Selecting the whole table keeps its source.
    let state = EditorState::with_filters(&doc, Vec::new());
    let all = decorate(
        state.tree(),
        state.text(),
        &Selection::single(to, from),
        None,
    );
    assert!(!all.iter().any(is_table));
    assert_eq!(source_lines(&all).len(), 3);
}

#[test]
fn a_table_in_a_quote_keeps_its_rows_without_the_marks() {
    let doc = "> | a | b |\n> | - | - |\n> | 1 | 2 |\n\nx";
    let d = at(doc, doc.len());
    let Some(Widget::Table { source }) = d.iter().find_map(|d| d.widget()).cloned() else {
        panic!("no table widget");
    };
    assert_eq!(source, "| a | b |\n| - | - |\n| 1 | 2 |");
}

/// `wiki-link-extension.ts:195-238`: the alias (else the target) while the
/// selection is outside the brackets; the source while it is inside.
#[test]
fn wiki_links_fold_to_their_text_unless_the_selection_is_inside() {
    let doc = "see [[Roadmap|the plan]] and [[Notes/Plan.md#Goals]] x";
    let folded = at(doc, doc.len());
    let texts: Vec<(Range<usize>, &str)> = folded
        .iter()
        .filter_map(|d| match d {
            Decoration::Replace {
                range,
                widget: Widget::WikiLink { display },
                ..
            } => Some((range.clone(), display.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [(4..24, "the plan"), (29..52, "Notes/Plan")]);
    let editing = at(doc, 10);
    assert!(
        !editing
            .iter()
            .any(|d| matches!(d, Decoration::Replace { range, .. } if range.start == 4)),
        "the caret inside shows the source"
    );
    assert!(at("`[[code]]`", 0).iter().all(|d| d.widget().is_none()));
}

/// US-033: an image embed folds to its image (a block alone on its line,
/// `!` included) unless the caret is inside; another embed keeps its `!`
/// and folds as a link (`wiki-link-extension.ts:205-232`).
#[test]
fn image_embeds_fold_to_their_image() {
    let doc = "![[assets/pic.png|400]]\n\nsee ![[pic.png]] and ![[Note]]\n\nx";
    let folded = at(doc, doc.len());
    let images: Vec<(Range<usize>, &str, bool)> = folded
        .iter()
        .filter_map(|d| match d {
            Decoration::Replace {
                range,
                widget: Widget::WikiImage { target },
                block,
            } => Some((range.clone(), target.as_str(), *block)),
            _ => None,
        })
        .collect();
    let inline = doc.find("![[pic").unwrap();
    assert_eq!(
        images,
        [
            (0..23, "assets/pic.png", true),
            (inline..inline + 12, "pic.png", false)
        ]
    );
    let note = doc.find("![[Note").unwrap();
    assert!(folded.iter().any(|d| matches!(
        d,
        Decoration::Replace { range, widget: Widget::WikiLink { .. }, .. } if range.start == note + 1
    )));
    assert!(
        at(doc, 5)
            .iter()
            .all(|d| !matches!(d, Decoration::Replace { range, .. } if range.start == 0)),
        "the caret inside shows the source"
    );
}

/// US-034: the properties panel while the caret is away, the YAML as a
/// code block once it touches the frontmatter.
#[test]
fn frontmatter_folds_to_its_panel_unless_touched() {
    let doc = "---\ntitle: a\n---\nbody";
    let code_lines = |d: &[Decoration]| {
        d.iter()
            .filter(|d| {
                matches!(
                    d,
                    Decoration::Line {
                        style: LineStyle::Code { .. },
                        ..
                    }
                )
            })
            .count()
    };
    let folded = at(doc, doc.len());
    let panel = folded
        .iter()
        .find(|d| d.widget() == Some(&Widget::Frontmatter))
        .expect("panel");
    assert_eq!(panel.range(), 0..16);
    assert!(matches!(panel, Decoration::Replace { block: true, .. }));
    assert_eq!(code_lines(&folded), 0);

    let touched = at(doc, 5);
    assert!(
        touched
            .iter()
            .all(|d| d.widget() != Some(&Widget::Frontmatter))
    );
    assert_eq!(code_lines(&touched), 3);
}

/// US-035: an HTML block is drawn while the selection is away, its source
/// once touched, and stays drawn while a drag frozen outside it passes
/// over it; `script` and `style` blocks stay source.
#[test]
fn html_blocks_fold_unless_touched_or_script() {
    let doc = "intro\n\n<div>\n<b>hi</b>\n</div>\n\nend";
    let is_html = |d: &Decoration| matches!(d.widget(), Some(Widget::Html { .. }));
    let block = doc.find("<div>").unwrap();
    let folded = at(doc, 0);
    let widget = folded.iter().find(|d| is_html(d)).expect("html widget");
    assert_eq!(widget.range(), block..doc.find("\n\nend").unwrap());
    assert!(!at(doc, block + 3).iter().any(is_html));

    // A drag that started at the intro keeps the block drawn.
    let state = EditorState::with_filters(doc, Vec::new());
    let frozen = Selection::cursor(0);
    let live = Selection::single(0, block + 3);
    let dragged = decorate(state.tree(), state.text(), &live, Some(&frozen));
    assert!(dragged.iter().any(is_html));

    assert!(
        !at("<script>\nalert(1)\n</script>\n\nx", 20)
            .iter()
            .any(is_html)
    );
}

/// `<details>` runs to its `</details>` over blank lines, as Writer's
/// block parser reads it; an unclosed one does not.
#[test]
fn details_blocks_span_blank_lines() {
    let doc = "<details>\n<summary>More</summary>\n\nHidden **text**\n\n</details>\n\nafter";
    let html: Vec<_> = at(doc, doc.len())
        .into_iter()
        .filter(|d| matches!(d.widget(), Some(Widget::Html { .. })))
        .collect();
    assert_eq!(html.len(), 1);
    assert_eq!(html[0].range(), 0..doc.find("\n\nafter").unwrap());
    let unclosed = "<details>\n<summary>More</summary>\n\nHidden";
    let html: Vec<_> = at(unclosed, unclosed.len())
        .into_iter()
        .filter(|d| matches!(d.widget(), Some(Widget::Html { .. })))
        .collect();
    assert_eq!(html[0].range(), 0..unclosed.find("\n\nHidden").unwrap());
}

/// US-036: `$...$` folds inline, `$$...$$` alone on its lines as a block;
/// currency stays text; the caret reveals the source.
#[test]
fn formulas_fold_inline_or_as_blocks_and_currency_stays_text() {
    let math = |d: &[Decoration]| -> Vec<(Range<usize>, String, bool, bool)> {
        d.iter()
            .filter_map(|d| match d {
                Decoration::Replace {
                    range,
                    widget: Widget::Math { tex, display },
                    block,
                } => Some((range.clone(), tex.clone(), *display, *block)),
                _ => None,
            })
            .collect()
    };
    let doc = "Inline $a^2$ here.\n\n$$\n\\frac{1}{2}\n$$\n\nend";
    let found = math(&at(doc, doc.len()));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0], (7..12, "a^2".into(), false, false));
    let block_from = doc.find("$$").unwrap();
    assert_eq!(found[1].0, block_from..doc.rfind("$$").unwrap() + 2);
    assert!(found[1].2 && found[1].3);
    // The caret inside a formula shows its source.
    assert_eq!(math(&at(doc, 9)).len(), 1);
    // So does a range covering it (`math.test.ts:142`).
    let state = EditorState::with_filters(doc, Vec::new());
    let covering = Selection::single(13, 5);
    assert_eq!(
        math(&decorate(state.tree(), state.text(), &covering, None)).len(),
        1
    );

    for text in [
        "5 $ and 10 $",
        "I paid $5 and $10 more",
        r"costs \$5 and \$10",
    ] {
        assert!(math(&at(text, 0)).is_empty(), "{text}");
    }
    // Display math inside a line flows with the text.
    let inline_display = "see $$x$$ now";
    assert_eq!(
        math(&at(inline_display, 0)),
        vec![(4..9, "x".into(), true, false)]
    );
}

/// `math.test.ts:29-63`: Pandoc's rules for `$` (no space after the
/// opener or before the closer, no digit after the closer, `\$` escaped).
#[test]
fn math_parses_with_pandocs_guards() {
    let nodes = |doc: &str| -> Vec<String> {
        let state = EditorState::with_filters(doc, Vec::new());
        state
            .tree()
            .root()
            .descendants()
            .filter(|n| n.kind() == crate::tree::NodeKind::Math)
            .map(|n| doc[n.range()].to_owned())
            .collect()
    };
    assert_eq!(
        nodes(r"Euler: $e^{i\pi} + 1 = 0$ wow"),
        [r"$e^{i\pi} + 1 = 0$"]
    );
    assert_eq!(
        nodes(r"Sum: $$\sum_{i=1}^n i$$ done"),
        [r"$$\sum_{i=1}^n i$$"]
    );
    assert!(nodes(r"costs \$5 and \$10").is_empty());
    assert!(nodes("I paid $5 and $10 more").is_empty());
    assert!(nodes("a $ b$ c").is_empty());
    assert!(nodes("a $b $ c").is_empty());
    assert_eq!(nodes("pay $x+y$ dollars"), ["$x+y$"]);
    assert_eq!(nodes("$$ x^2 $$"), ["$$ x^2 $$"]);
}

/// US-037: only a closed, non-empty ` ```mermaid ` fence becomes a canvas,
/// whose code chrome is not drawn, whatever the selection.
#[test]
fn closed_mermaid_fences_fold_to_a_canvas() {
    let canvas = |d: &[Decoration]| -> Vec<(Range<usize>, String)> {
        d.iter()
            .filter_map(|d| match d {
                Decoration::Replace {
                    range,
                    widget: Widget::Mermaid { source },
                    block: true,
                } => Some((range.clone(), source.clone())),
                _ => None,
            })
            .collect()
    };
    let doc = "intro\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\nend";
    let fence = doc.find("```").unwrap()..doc.rfind("```").unwrap() + 3;
    let folded = at(doc, 0);
    assert_eq!(
        canvas(&folded),
        vec![(fence.clone(), "flowchart LR\n  A --> B".into())]
    );
    assert!(!folded.iter().any(|d| matches!(
        d,
        Decoration::Line {
            style: LineStyle::Code { .. },
            ..
        }
    )));
    // `mermaid.test.ts:501-511`: the canvas stays whatever the selection.
    let state = EditorState::with_filters(doc, Vec::new());
    for selection in [
        Selection::cursor(fence.start + 5),
        Selection::cursor(fence.start),
        Selection::cursor(fence.end),
        Selection::single(fence.end, fence.start),
        Selection::single(0, doc.len()),
    ] {
        assert_eq!(
            canvas(&decorate(state.tree(), state.text(), &selection, None)).len(),
            1,
            "{selection:?}"
        );
    }

    let unclosed = "```mermaid\nflowchart LR\n  A --> B\n";
    assert!(canvas(&at(unclosed, unclosed.len())).is_empty());
    let empty = "```mermaid\n\n```\n\nx";
    assert!(canvas(&at(empty, empty.len())).is_empty());
    let other = "```rust\nfn main() {}\n```\n\nx";
    assert!(canvas(&at(other, other.len())).is_empty());
}
