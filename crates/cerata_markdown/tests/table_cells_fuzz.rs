//! US-050: 10 000 random edits typed, deleted and pasted in the cells of a
//! drawn table never panic and always leave a table with the same rows and
//! columns (ADR 0007, decision 5).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cerata_markdown::cells;
use cerata_markdown::table::{CellKind, Grid};
use cerata_markdown::{EditorState, NodeKind, Selection, UserEvent};

/// Deterministic xorshift, so a failure replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TYPED: &[&str] = &[
    "x", "word ", " ", "|", "\\", "\\|", "||", "<br>", "<", ">", "`", "**", "é", "日本", "😀",
    "\t", "-", ":",
];

const PASTED: &[&str] = &[
    "line one\nline two",
    "a|b\r\nc",
    "\n",
    "tail\\",
    "| x | y |\n| - | - |",
    "multi\n\nblank",
];

const TABLES: &[&str] = &[
    "intro\n\n| Name | Count |\n| --- | ---: |\n| Tea | 2 |\n| Coffee |  |\n\nend\n",
    "> | a | b | c |\n> |:-|:-:|-:|\n> | 1 | 2 | 3 |\n\nafter",
    "- item\n\n  | k | v |\n  | - | - |\n  | x\\|y | <br> |\n",
    "| a<br>b | `c` |\n| - | - |\n| **d** | [e](f) |",
];

fn grid(state: &EditorState) -> Option<Grid> {
    let node = state.tree().iter().find(|n| n.kind() == NodeKind::Table)?;
    Grid::from_node(state.text(), node)
}

/// Rows and drawn columns per row: what no cell edit may change.
fn shape(grid: &Grid) -> Vec<usize> {
    grid.rows
        .iter()
        .map(|r| {
            r.cells
                .iter()
                .filter(|c| c.kind != CellKind::Missing)
                .count()
        })
        .collect()
}

/// A random caret position in a random drawn cell.
fn caret_in_cell(rng: &mut Rng, grid: &Grid) -> usize {
    let cells: Vec<_> = grid.drawn_cells().collect();
    let (_, _, cell) = cells[rng.below(cells.len())];
    let stretches = cell.stretches();
    let s = &stretches[rng.below(stretches.len())];
    s.start + rng.below(s.end - s.start + 1)
}

#[test]
fn random_cell_edits_keep_the_table() {
    let mut rng = Rng(0x5eed_cafe_f00d_1234);
    let mut total = 0;
    for (t, doc) in TABLES.iter().enumerate() {
        let mut state = EditorState::new(*doc);
        let start = grid(&state).expect("a table");
        let expected = shape(&start);
        for step in 0..2_500 {
            let g = grid(&state).unwrap_or_else(|| panic!("table {t} lost at {step}"));
            let mut pos = caret_in_cell(&mut rng, &g);
            while !state.text().as_str().is_char_boundary(pos) {
                pos -= 1;
            }
            state.set_time(step as u64 * 1_000);
            let selection = if rng.below(4) == 0 {
                // A range inside the caret's cell.
                let (r, c) = g.content_at(pos).unwrap_or((0, 0));
                let cell = g.cell(r, c).unwrap();
                let mut other = cell.content.start + rng.below(cell.content.len() + 1);
                while !state.text().as_str().is_char_boundary(other) {
                    other -= 1;
                }
                Selection::single(pos, other)
            } else {
                Selection::cursor(pos)
            };
            state.select(selection).unwrap();
            match rng.below(6) {
                0 | 1 => {
                    let text = TYPED[rng.below(TYPED.len())];
                    let before = state.selection().main();
                    let cell = cells::cell_at(&state, before.head).map(|(_, r, c)| (r, c));
                    state.type_text(None, text, UserEvent::Type);
                    // The caret stays in its cell, after what it typed.
                    let head = state.selection().main().head;
                    assert_eq!(
                        cells::cell_at(&state, head).map(|(_, r, c)| (r, c)),
                        cell,
                        "table {t}, step {step}: typed {text:?} left the cell:\n{}",
                        state.doc()
                    );
                    assert!(head >= before.from(), "table {t}, step {step}");
                }
                2 => {
                    state.delete_backward();
                }
                3 => {
                    state.delete_forward();
                }
                4 => {
                    state.delete_group(rng.below(2) == 0);
                }
                _ => {
                    let text = PASTED[rng.below(PASTED.len())];
                    state.type_text(None, text, UserEvent::Command);
                }
            }
            let after = grid(&state)
                .unwrap_or_else(|| panic!("table {t} lost at step {step}:\n{}", state.doc()));
            assert_eq!(
                shape(&after),
                expected,
                "table {t}, step {step}:\n{}",
                state.doc()
            );
            // The caret stays where a drawn table can show it.
            let head = state.selection().main().head;
            if cells::grid_at(&state, head).is_some() {
                assert!(
                    after.cell_holding(head).is_some(),
                    "table {t}, step {step}: caret {head} on the structure:\n{}",
                    state.doc()
                );
            }
            total += 1;
        }
        // The edits did land in the cells.
        assert_ne!(state.doc(), *doc, "table {t}");
        assert!(state.doc().len() > doc.len() / 2, "table {t}");
    }
    assert_eq!(total, 10_000);
}
