#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cerata_markdown::commands::Clock;
use cerata_markdown::table::{CellKind, Grid};
use cerata_markdown::table_clip::CellBlock;
use cerata_markdown::{EditorState, NodeKind, Selection, UserEvent, cells, table_ops};

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

const CLOCK: Clock = Clock {
    year: 2026,
    month: 11,
    day: 2,
    hour: 12,
    minute: 0,
};

const TYPED: &[&str] = &[
    "x", "word ", " ", "|", "\\", "\\|", "<br>", "`", "**", "é", "日本", "😀",
];

const PASTED: &[&str] = &[
    "line one\nline two",
    "a|b\r\nc",
    "tail\\",
    "| x | y |\n| - | - |\n| 1 | 2 |",
    "p\tq\nr\ts",
];

const TABLES: &[&str] = &[
    "intro\n\n| Name | Count |\n| --- | ---: |\n| Tea | 2 |\n| Coffee |  |\n\nend\n",
    "> | a | b | c |\n> |:-|:-:|-:|\n> | 1 | 2 | 3 |\n\nafter",
    "- item\n\n  | k | v |\n  | - | - |\n  | x\\|y | <br> |\n",
    "| a<br>b | `c` |\n| - | - |\n| **d** | [e](f) |\n| short |",
];

fn structure() -> Vec<&'static str> {
    table_ops::IDS
        .iter()
        .copied()
        .filter(|id| !matches!(*id, "table.editSource" | "table.delete"))
        .collect()
}

fn grid(state: &EditorState) -> Option<Grid> {
    let node = state.tree().iter().find(|n| n.kind() == NodeKind::Table)?;
    Grid::from_node(state.text(), node)
}

fn caret_in_cell(rng: &mut Rng, state: &EditorState, grid: &Grid) -> usize {
    let cells: Vec<_> = grid.drawn_cells().collect();
    let (_, _, cell) = cells[rng.below(cells.len())];
    let stretches = cell.stretches();
    let s = &stretches[rng.below(stretches.len())];
    let mut pos = s.start + rng.below(s.end - s.start + 1);
    while !state.text().as_str().is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn random_cell(rng: &mut Rng, grid: &Grid) -> (usize, usize) {
    let rows: Vec<usize> = grid.text_rows().map(|(r, _)| r).collect();
    (rows[rng.below(rows.len())], rng.below(grid.columns()))
}

fn random_block(rng: &mut Rng) -> CellBlock {
    let rows = 1 + rng.below(3);
    let columns = 1 + rng.below(3);
    CellBlock {
        rows: (0..rows)
            .map(|_| {
                (0..columns)
                    .map(|_| TYPED[rng.below(TYPED.len())].to_owned())
                    .collect()
            })
            .collect(),
        alignments: Vec::new(),
    }
}

#[test]
fn random_table_operations_keep_a_valid_table_and_undo_to_the_byte() {
    let mut rng = Rng(0x7ab1_e057_0057_cafe);
    let commands = structure();
    let mut total = 0;
    let mut commands_run = 0;
    let mut rectangles = 0;
    for (t, doc) in TABLES.iter().enumerate() {
        let mut state = EditorState::new(*doc);
        let mut columns = grid(&state).expect("a table").columns();
        let mut undo: Vec<usize> = Vec::new();
        let mut redo: Vec<usize> = Vec::new();
        for step in 0..2_500 {
            let g = grid(&state).unwrap_or_else(|| panic!("table {t} lost at {step}"));
            state.set_time(step as u64 * 1_000 + 1);
            let depth = state.history().undo_depth();
            let before = columns;
            let mut next = columns;
            let big = g.columns() >= 8 || g.rows.len() >= 14;
            let pos = caret_in_cell(&mut rng, &state, &g);
            state.select(Selection::cursor(pos)).unwrap();
            let op = rng.below(10);
            match op {
                0 | 1 => {
                    let text = TYPED[rng.below(TYPED.len())];
                    state.type_text(None, text, UserEvent::Type);
                }
                2 => {
                    state.delete_backward();
                }
                3 => {
                    let text = PASTED[rng.below(PASTED.len())];
                    state.type_text(None, text, UserEvent::Command);
                }
                4 | 5 => {
                    let mut id = commands[rng.below(commands.len())];
                    if big {
                        id = match id {
                            "table.columnLeft" | "table.columnRight" | "table.columnDuplicate" => {
                                "table.columnDelete"
                            }
                            "table.rowAbove" | "table.rowBelow" | "table.rowDuplicate" => {
                                "table.rowDelete"
                            }
                            other => other,
                        };
                    }
                    if table_ops::enabled(&state, id) {
                        match id {
                            "table.columnLeft" | "table.columnRight" | "table.columnDuplicate" => {
                                next += 1;
                            }
                            "table.columnDelete" => next -= 1,
                            _ => {}
                        }
                        commands_run += 1;
                    }
                    state.run_command(id, &CLOCK);
                }
                6 | 7 => {
                    let anchor = random_cell(&mut rng, &g);
                    let head = random_cell(&mut rng, &g);
                    state.select_cells(&g, anchor, head);
                    rectangles += 1;
                    match rng.below(4) {
                        0 => {
                            state.type_text(None, TYPED[rng.below(TYPED.len())], UserEvent::Type);
                        }
                        1 => {
                            state.delete_forward();
                        }
                        2 => {
                            state.run_command("format.bold", &CLOCK);
                        }
                        _ if big => {
                            state.delete_backward();
                        }
                        _ => {
                            let block = random_block(&mut rng);
                            let column = anchor.1.min(head.1);
                            state.paste_cells(&block);
                            next = next.max(column + block.columns());
                        }
                    }
                }
                8 if big => {
                    state.delete_backward();
                }
                8 => {
                    let block = random_block(&mut rng);
                    if let Some((_, _, column)) = cells::cell_at(&state, pos) {
                        state.paste_cells(&block);
                        next = next.max(column + block.columns());
                    }
                }
                _ => {
                    if rng.below(2) == 0 {
                        if state.undo() {
                            redo.push(columns);
                            next = undo.pop().expect("a count per step");
                        }
                    } else if state.redo() {
                        undo.push(columns);
                        next = redo.pop().expect("a count per step");
                    }
                }
            }
            if op != 9 {
                let after = state.history().undo_depth();
                if after == depth + 1 {
                    undo.push(before);
                    redo.clear();
                } else {
                    assert_eq!(after, depth, "table {t}, step {step}: one step at most");
                    next = before;
                }
            }
            columns = next;
            let g = grid(&state)
                .unwrap_or_else(|| panic!("table {t} lost at step {step}:\n{}", state.doc()));
            assert_eq!(
                g.columns(),
                columns,
                "table {t}, step {step} (op {op}):\n{}",
                state.doc()
            );
            assert!(
                g.rows
                    .iter()
                    .all(|r| r.cells.iter().all(|c| c.kind != CellKind::Extra)),
                "table {t}, step {step}: a cell past the header:\n{}",
                state.doc()
            );
            total += 1;
        }
        assert_ne!(state.doc(), *doc, "table {t}: the run edited it");
        while state.undo() {}
        assert_eq!(state.doc(), *doc, "table {t}: undoing everything");
    }
    assert_eq!(total, 10_000);
    assert!(commands_run > 1_000, "{commands_run} structure commands");
    assert!(rectangles > 1_000, "{rectangles} rectangles");
}
