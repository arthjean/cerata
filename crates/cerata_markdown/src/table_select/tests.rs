use super::*;
use crate::commands::Clock;

const CLOCK: Clock = Clock {
    year: 2026,
    month: 11,
    day: 2,
    hour: 12,
    minute: 0,
};

const DOC: &str = "| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 |  | 6 |\n\nend";

fn grid(s: &EditorState) -> Grid {
    cells::grid_at(s, 0).expect("a table")
}

fn state() -> EditorState {
    let mut s = EditorState::new(DOC);
    s.select(Selection::cursor(DOC.find('2').unwrap())).unwrap();
    s
}

fn selected(s: &EditorState) -> Vec<&str> {
    s.selection()
        .ranges()
        .iter()
        .map(|r| &s.doc()[r.range()])
        .collect()
}

#[test]
fn a_rectangle_projects_one_range_per_non_empty_cell() {
    let mut s = state();
    let g = grid(&s);
    assert!(s.select_cells(&g, (2, 1), (3, 2)));
    let ts = *s.table_selection().unwrap();
    assert_eq!(ts.rows(), 2..=3);
    assert_eq!(ts.columns(), 1..=2);
    assert_eq!(
        selected(&s),
        vec!["2", "3", "6"],
        "the empty cell has no range"
    );
    assert_eq!(
        &s.doc()[s.selection().main().range()],
        "6",
        "the head cell is main"
    );
    let g = grid(&s);
    assert!(s.select_cells(&g, (0, 0), (2, 0)));
    assert_eq!(selected(&s), vec!["a", "1"]);
    s.select(Selection::cursor(0)).unwrap();
    assert!(s.table_selection().is_none());
}

#[test]
fn formatting_applies_to_each_non_empty_cell_in_one_step() {
    let mut s = state();
    let g = grid(&s);
    s.select_cells(&g, (2, 0), (3, 2));
    assert!(s.run_command("format.bold", &CLOCK));
    assert_eq!(
        s.doc(),
        "| a | b | c |\n|---|---|---|\n| **1** | **2** | **3** |\n| **4** |  | **6** |\n\nend"
    );
    assert!(s.undo());
    assert_eq!(s.doc(), DOC);
    for id in ["format.italic", "format.strikethrough", "format.code"] {
        let mut s = state();
        let g = grid(&s);
        s.select_cells(&g, (2, 0), (2, 1));
        assert!(s.run_command(id, &CLOCK), "{id}");
        let g = grid(&s);
        let cells: Vec<&str> = g.rows[2].cells[..2]
            .iter()
            .map(|c| &s.doc()[c.content.clone()])
            .collect();
        assert!(
            cells.iter().all(|c| c.len() > 1 && c != &"1" && c != &"2"),
            "{id}: {cells:?}"
        );
        assert!(s.undo());
        assert_eq!(s.doc(), DOC, "{id}");
    }
    let bold = "| a | b |\n|---|---|\n| **x** | *y* |\n";
    let mut s = EditorState::new(bold);
    let g = cells::grid_at(&s, 0).unwrap();
    s.select_cells(&g, (2, 0), (2, 1));
    assert!(s.run_command("format.clearFormatting", &CLOCK));
    assert_eq!(s.doc(), "| a | b |\n|---|---|\n| x | y |\n");
}

#[test]
fn backspace_and_delete_empty_the_cells_and_keep_the_grid() {
    for forward in [false, true] {
        let mut s = state();
        let g = grid(&s);
        s.select_cells(&g, (0, 1), (2, 2));
        let done = if forward {
            s.delete_forward()
        } else {
            s.delete_backward()
        };
        assert!(done);
        assert_eq!(
            s.doc(),
            "| a |  |  |\n|---|---|---|\n| 1 |  |  |\n| 4 |  | 6 |\n\nend"
        );
        assert_eq!(cells::grid_at(&s, 0).unwrap().columns(), 3);
        assert!(s.undo());
        assert_eq!(s.doc(), DOC);
        assert!(!s.undo(), "one step");
    }
}

#[test]
fn arrows_collapse_and_shift_arrows_extend() {
    let mut s = state();
    let g = grid(&s);
    s.select_cells(&g, (2, 1), (3, 2));
    assert!(s.collapse_cells(false));
    assert_eq!(
        s.selection().main().head,
        DOC.find('2').unwrap(),
        "first cell's start"
    );
    let mut s = state();
    let g = grid(&s);
    s.select_cells(&g, (2, 1), (3, 2));
    assert!(s.collapse_cells(true));
    assert_eq!(
        s.selection().main().head,
        DOC.find('6').unwrap() + 1,
        "last cell's end"
    );
    let mut s = state();
    s.select(Selection::cursor(DOC.find('2').unwrap() + 1))
        .unwrap();
    assert!(s.extend_cells(0, 1, false));
    assert_eq!(selected(&s), vec!["2", "3"]);
    assert!(s.extend_cells(0, 1, false));
    assert_eq!(s.table_selection().unwrap().head, (2, 2));
    assert!(s.extend_cells(1, 0, false));
    assert_eq!(selected(&s), vec!["2", "3", "6"]);
    assert!(s.extend_cells(-2, 0, false));
    assert_eq!(s.table_selection().unwrap().head, (0, 2));
    let mut s = state();
    s.select(Selection::cursor(DOC.find('2').unwrap() + 1))
        .unwrap();
    assert!(!s.extend_cells(0, -1, false));
}

#[test]
fn typing_replaces_the_anchor_cell_and_collapses() {
    let mut s = state();
    let g = grid(&s);
    s.select_cells(&g, (2, 0), (3, 1));
    assert!(s.type_text(None, "x|", UserEvent::Type));
    assert_eq!(
        s.doc(),
        "| a | b | c |\n|---|---|---|\n| x\\| | 2 | 3 |\n| 4 |  | 6 |\n\nend"
    );
    assert!(s.table_selection().is_none());
    assert_eq!(s.selection().ranges().len(), 1);
    let head = s.selection().main().head;
    assert_eq!(&s.doc()[head - 3..head], "x\\|");
}
