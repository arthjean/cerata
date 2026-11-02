use super::*;
use crate::commands::{Clock, command};
use crate::table::Grid;

const CLOCK: Clock = Clock {
    year: 2026,
    month: 11,
    day: 2,
    hour: 12,
    minute: 0,
};

const DOC: &str =
    "intro\n\n| Name | Qty |\n|:--|--:|\n| **pear** | 10 |\n| Apple | 2 |\n| `fig` | 33 |\n\nend";

fn state_at(doc: &str, needle: &str) -> EditorState {
    let mut s = EditorState::new(doc);
    let at = doc.find(needle).unwrap();
    s.select(Selection::cursor(at)).unwrap();
    s
}

fn run_id(s: &mut EditorState, id: &str) -> bool {
    s.run_command(id, &CLOCK)
}

/// The grid at `pos`, its rows' drawn contents.
fn cells_of(s: &EditorState) -> Vec<Vec<String>> {
    let grid = table_grid(s);
    grid.text_rows()
        .map(|(_, r)| {
            r.cells
                .iter()
                .map(|c| s.doc()[c.content.clone()].to_owned())
                .collect()
        })
        .collect()
}

fn table_grid(s: &EditorState) -> Grid {
    let at = s.doc().find('|').unwrap();
    cells::grid_in(s.tree(), s.text(), at, None).expect("a table")
}

/// The caret's row and column in the drawn table.
fn caret_cell(s: &EditorState) -> (usize, usize) {
    let head = s.selection().main().head;
    let (_, r, c) = cells::cell_at(s, head).expect("the caret in a cell");
    (r, c)
}

/// Every row has the header's number of cells (a valid GFM table).
fn assert_valid(s: &EditorState, columns: usize) {
    let grid = table_grid(s);
    assert_eq!(grid.columns(), columns, "{}", s.doc());
    for row in &grid.rows {
        assert!(
            row.cells.iter().all(|c| c.kind == CellKind::InGrid),
            "{:?}",
            s.doc()
        );
        assert_eq!(row.cells.len(), columns, "{:?}", s.doc());
    }
}

#[test]
fn the_registry_lists_every_table_command() {
    for id in IDS {
        let c = command(id).unwrap_or_else(|| panic!("{id} not registered"));
        assert!(!c.label.is_empty());
    }
    assert_eq!(IDS.len(), 21);
}

#[test]
fn table_commands_need_the_caret_in_a_drawn_table() {
    let mut s = state_at(DOC, "intro");
    for id in IDS {
        assert!(!enabled(&s, id), "{id}");
        assert!(!command(id).unwrap().enabled(&s));
        assert!(!run_id(&mut s, id), "{id}");
    }
    assert_eq!(s.doc(), DOC);
    let s = state_at(DOC, "Apple");
    for id in IDS {
        // The first column cannot move left.
        assert_eq!(enabled(&s, id), *id != "table.columnMoveLeft", "{id}");
    }
    assert!(command("format.bold").unwrap().enabled(&s));
}

/// Each command is one undo step that puts the caret back in its cell,
/// and leaves a table with as many cells in every row.
#[test]
fn every_command_is_one_undo_step_back_to_the_cell() {
    for id in IDS {
        if *id == "table.editSource" {
            continue;
        }
        // Column 0 cannot move left and is already left-aligned.
        let at = if matches!(*id, "table.columnMoveLeft" | "table.alignLeft") {
            "2 |"
        } else {
            "pple"
        };
        let mut s = state_at(DOC, at);
        let before = s.selection().clone();
        assert!(run_id(&mut s, id), "{id}");
        assert_ne!(s.doc(), DOC, "{id}");
        if *id != "table.delete" {
            let columns = match *id {
                "table.columnLeft" | "table.columnRight" | "table.columnDuplicate" => 3,
                "table.columnDelete" => 1,
                _ => 2,
            };
            assert_valid(&s, columns);
            caret_cell(&s);
        }
        assert!(s.undo(), "{id}");
        assert_eq!(s.doc(), DOC, "{id}");
        assert_eq!(s.selection(), &before, "{id}");
        assert!(!s.undo(), "{id}: more than one step");
    }
}

#[test]
fn rows_are_inserted_duplicated_moved_and_deleted() {
    let mut s = state_at(DOC, "Apple");
    assert!(run_id(&mut s, "table.rowAbove"));
    assert!(
        s.doc()
            .contains("| **pear** | 10 |\n|       |   |\n| Apple | 2 |"),
        "{:?}",
        s.doc()
    );
    assert_eq!(caret_cell(&s), (3, 0));
    let mut s = state_at(DOC, "Apple");
    assert!(run_id(&mut s, "table.rowBelow"));
    assert!(
        s.doc().contains("| Apple | 2 |\n|       |   |\n| `fig`"),
        "{:?}",
        s.doc()
    );
    assert_eq!(caret_cell(&s), (4, 0));
    let mut s = state_at(DOC, "pple");
    assert!(run_id(&mut s, "table.rowDuplicate"));
    assert!(s.doc().contains("| Apple | 2 |\n| Apple | 2 |\n"));
    assert_eq!(caret_cell(&s), (4, 0));
    assert_eq!(
        s.selection().main().head,
        s.doc().rfind("Apple").unwrap() + 1
    );
    let mut s = state_at(DOC, "Apple");
    assert!(run_id(&mut s, "table.rowUp"));
    assert_eq!(
        cells_of(&s)[1..3],
        [vec!["Apple", "2"], vec!["**pear**", "10"]]
    );
    assert_eq!(caret_cell(&s), (2, 0));
    assert!(run_id(&mut s, "table.rowDown"));
    assert_eq!(s.doc(), DOC);
    let mut s = state_at(DOC, "Apple");
    assert!(run_id(&mut s, "table.rowDelete"));
    assert!(!s.doc().contains("Apple"));
    assert_eq!(
        cells_of(&s)[2],
        vec!["`fig`", "33"],
        "the row below takes the caret"
    );
    assert_eq!(caret_cell(&s), (3, 0));
    // The last row: the caret goes up.
    let mut s = state_at(DOC, "`fig`");
    assert!(run_id(&mut s, "table.rowDelete"));
    assert_eq!(caret_cell(&s), (3, 0));
    assert!(s.doc().contains("| Apple | 2 |\n\nend"));
}

#[test]
fn a_row_above_the_header_becomes_the_header() {
    let mut s = state_at(DOC, "Name");
    assert!(run_id(&mut s, "table.rowAbove"));
    assert!(
        s.doc()
            .contains("|      |     |\n|:--|--:|\n| Name | Qty |\n| **pear**"),
        "{:?}",
        s.doc()
    );
    assert_eq!(caret_cell(&s), (0, 0));
    assert_valid(&s, 2);
    // Below the header: the first body row.
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.rowBelow"));
    assert!(
        s.doc().contains("|--:|\n|      |     |\n| **pear**"),
        "{:?}",
        s.doc()
    );
    assert_eq!(caret_cell(&s), (2, 1));
}

#[test]
fn columns_take_the_reference_alignment() {
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.columnLeft"));
    let grid = table_grid(&s);
    assert_eq!(
        grid.alignments,
        vec![Some(Align::Left), Some(Align::Right), Some(Align::Right)]
    );
    assert!(s.doc().contains("| Name |     | Qty |"), "{:?}", s.doc());
    assert_eq!(caret_cell(&s), (0, 1));
    let mut s = state_at(DOC, "Name");
    assert!(run_id(&mut s, "table.columnRight"));
    assert_eq!(table_grid(&s).alignments[1], Some(Align::Left));
    assert_eq!(caret_cell(&s), (0, 1));
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.columnDuplicate"));
    assert_eq!(cells_of(&s)[2], vec!["Apple", "2", "2"]);
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.columnMoveLeft"));
    assert_eq!(cells_of(&s)[0], vec!["Qty", "Name"]);
    assert_eq!(
        table_grid(&s).alignments,
        vec![Some(Align::Right), Some(Align::Left)]
    );
    assert_eq!(caret_cell(&s), (0, 0));
    assert!(run_id(&mut s, "table.columnMoveRight"));
    assert_eq!(s.doc(), DOC);
    let mut s = state_at(DOC, "Name");
    assert!(run_id(&mut s, "table.columnDelete"));
    assert_eq!(cells_of(&s)[2], vec!["2"]);
    assert_valid(&s, 1);
}

/// Commands other than "Format table" rewrite only the cells they create,
/// move or delete: here, an inserted column leaves every other byte.
#[test]
fn structure_edits_are_minimal() {
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.columnRight"));
    let lines: Vec<&str> = s.doc().lines().collect();
    assert_eq!(lines[2], "| Name | Qty |     |");
    assert_eq!(lines[3], "|:--|--:|--:|");
    assert_eq!(lines[4], "| **pear** | 10 |    |");
    let mut s = state_at(DOC, "Apple");
    assert!(run_id(&mut s, "table.alignCenter"));
    assert_eq!(s.doc(), DOC.replace("|:--|--:|", "|:-:|--:|"));
    assert!(run_id(&mut s, "table.alignNone"));
    assert_eq!(s.doc(), DOC.replace("|:--|--:|", "|---|--:|"));
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.alignLeft"));
    assert_eq!(s.doc(), DOC.replace("|:--|--:|", "|:--|:--|"));
}

#[test]
fn sorting_compares_the_drawn_text_and_keeps_the_header() {
    let mut s = state_at(DOC, "Name");
    assert!(run_id(&mut s, "table.sortAscending"));
    let names: Vec<String> = cells_of(&s)[1..].iter().map(|r| r[0].clone()).collect();
    // Marks ignored, case ignored: Apple, `fig`, **pear**.
    assert_eq!(names, vec!["Apple", "`fig`", "**pear**"]);
    assert_eq!(cells_of(&s)[0], vec!["Name", "Qty"]);
    assert_eq!(caret_cell(&s), (0, 0));
    let mut s = state_at(DOC, "Qty");
    assert!(run_id(&mut s, "table.sortAscending"));
    let qty: Vec<String> = cells_of(&s)[1..].iter().map(|r| r[1].clone()).collect();
    assert_eq!(qty, vec!["2", "10", "33"], "numbers in numeric order");
    assert!(run_id(&mut s, "table.sortDescending"));
    let qty: Vec<String> = cells_of(&s)[1..].iter().map(|r| r[1].clone()).collect();
    assert_eq!(qty, vec!["33", "10", "2"]);
    // The caret follows its row.
    let mut s = state_at(DOC, "pear");
    assert!(run_id(&mut s, "table.sortAscending"));
    let head = s.selection().main().head;
    assert_eq!(&s.doc()[head..head + 4], "pear");
    assert!(natural_cmp("item 9", "item 10").is_lt());
    assert!(natural_cmp("a", "b").is_lt());
    assert_eq!(sort_key("**Big** [link](x) a<br>b"), "big link a b");
}

#[test]
fn format_pads_by_display_width_and_completes_short_rows() {
    let doc = "| a | 漢字 |\n|---|:-:|\n| longer text | x |\n| y |";
    let mut s = state_at(doc, "x");
    assert!(run_id(&mut s, "table.format"));
    assert_eq!(
        s.doc(),
        "| a           | 漢字 |\n| ----------- | :--: |\n| longer text |  x   |\n| y           |      |"
    );
    assert_valid(&s, 2);
    let head = s.selection().main().head;
    assert_eq!(&s.doc()[head..head + 1], "x");
    assert!(s.undo());
    assert_eq!(s.doc(), doc);
}

#[test]
fn header_and_edges_disable_what_cannot_apply() {
    let s = state_at(DOC, "Name");
    for id in ["table.rowDelete", "table.rowUp", "table.rowDown"] {
        assert!(!enabled(&s, id), "{id} in the header");
    }
    assert!(!enabled(&s, "table.columnMoveLeft"));
    assert!(enabled(&s, "table.columnMoveRight"));
    let s = state_at(DOC, "pear");
    assert!(!enabled(&s, "table.rowUp"), "the first body row");
    assert!(enabled(&s, "table.rowDown"));
    let s = state_at(DOC, "fig");
    assert!(!enabled(&s, "table.rowDown"), "the last body row");
    assert!(enabled(&s, "table.rowUp"));
    let one = "| a |\n|---|\n| b |\n";
    let s = state_at(one, "b");
    assert!(!enabled(&s, "table.columnDelete"));
    assert!(enabled(&s, "table.delete"));
    let mut s = state_at(one, "b");
    assert!(run_id(&mut s, "table.delete"));
    assert_eq!(s.doc(), "");
    let mut s = state_at(DOC, "Apple");
    assert!(run_id(&mut s, "table.delete"));
    assert_eq!(s.doc(), "intro\n\n\nend");
}

#[test]
fn rows_added_keep_the_quote_or_list_prefix() {
    let quoted = "> | a | b |\n> |---|---|\n> | 1 | 2 |\n\nx";
    let mut s = state_at(quoted, "1");
    assert!(run_id(&mut s, "table.rowBelow"));
    assert_eq!(
        s.doc(),
        "> | a | b |\n> |---|---|\n> | 1 | 2 |\n> |   |   |\n\nx"
    );
    assert_valid(&s, 2);
    let listed = "- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n";
    let mut s = state_at(listed, "1");
    assert!(run_id(&mut s, "table.rowAbove"));
    assert!(
        s.doc().contains("  |---|---|\n  |   |   |\n  | 1 | 2 |"),
        "{:?}",
        s.doc()
    );
    assert_valid(&s, 2);
    // In the quote, a new header takes the header line's prefix.
    let mut s = state_at(quoted, "a");
    assert!(run_id(&mut s, "table.rowAbove"));
    assert!(
        s.doc()
            .starts_with("> |   |   |\n> |---|---|\n> | a | b |\n"),
        "{:?}",
        s.doc()
    );
}

#[test]
fn a_malformed_table_is_completed_in_the_same_step() {
    let doc = "| a | b | c |\n|---|---|---|\n| 1\n| 4 | 5 |\n\nx";
    let mut s = state_at(doc, "4");
    assert!(run_id(&mut s, "table.rowDuplicate"));
    assert_valid(&s, 3);
    assert!(
        s.doc().contains("| 1 |  |  |\n| 4 | 5 |  |\n| 4 | 5 |  |"),
        "{:?}",
        s.doc()
    );
    assert!(s.undo());
    assert_eq!(s.doc(), doc);
    let mut s = state_at(doc, "b");
    assert!(run_id(&mut s, "table.columnDelete"));
    assert_valid(&s, 2);
}

#[test]
fn crlf_tables_keep_their_line_breaks() {
    let doc = "| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n";
    let mut s = state_at(doc, "1");
    assert!(run_id(&mut s, "table.rowBelow"));
    assert_eq!(
        s.doc(),
        "| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n|   |   |\r\n"
    );
    assert_eq!(caret_cell(&s), (3, 0));
}

/// US-056: "Edit as Markdown" shows the source and edits it as text until
/// the selection leaves the table.
#[test]
fn edit_as_markdown_holds_until_the_selection_leaves() {
    let drawn = |s: &EditorState| {
        s.decorations().iter().any(|d| {
            matches!(
                d,
                crate::decorate::Decoration::Replace {
                    widget: crate::decorate::Widget::Table { .. },
                    ..
                }
            )
        })
    };
    let mut s = state_at(DOC, "Apple");
    assert!(drawn(&s));
    assert!(run_id(&mut s, "table.editSource"));
    assert!(!drawn(&s));
    assert!(cells::grid_at(&s, s.selection().main().head).is_none());
    // A pipe takes the caret and typing is not escaped.
    let pipe = DOC.find("| Apple").unwrap();
    s.select(Selection::cursor(pipe)).unwrap();
    assert_eq!(s.selection().main().head, pipe);
    assert!(s.type_text(None, "|", UserEvent::Type));
    assert!(s.doc().contains("|| Apple"));
    assert!(!drawn(&s));
    // Out of the table: drawn again, and again on the way back.
    s.select(Selection::cursor(0)).unwrap();
    assert!(drawn(&s));
    assert!(s.undo());
    s.select(Selection::cursor(DOC.find("Apple").unwrap()))
        .unwrap();
    assert!(drawn(&s));
}

/// US-056: a range from one cell to another keeps its ends (a search
/// result across a `|`), and its table shows its source.
#[test]
fn a_range_across_cells_keeps_its_ends() {
    let mut s = state_at(DOC, "Apple");
    let from = DOC.find("le | 2").unwrap();
    let to = from + 4;
    s.select(Selection::single(from, to)).unwrap();
    assert_eq!(s.selection().main().range(), from..to);
    assert!(!s.decorations().iter().any(|d| matches!(
        d,
        crate::decorate::Decoration::Replace {
            widget: crate::decorate::Widget::Table { .. },
            ..
        }
    )));
}

/// Review of EP-007: a table written without its edge pipes stays a table
/// when a column is added at an edge or all but one are deleted.
#[test]
fn columns_at_pipe_less_edges_stay_cells() {
    let doc = "| a | b\n|---|---\n| 1 | 2";
    let mut s = state_at(doc, "b");
    assert!(run_id(&mut s, "table.columnRight"));
    assert_valid(&s, 3);
    assert_eq!(caret_cell(&s), (0, 2));
    let doc = "a | b |\n--- | --- |\n1 | 2 |";
    let mut s = state_at(doc, "a");
    assert!(run_id(&mut s, "table.columnLeft"));
    assert_valid(&s, 3);
    assert_eq!(cells_of(&s)[1], vec!["", "1", "2"]);
    let doc = "a | b\n--- | ---\n1 | 2";
    let mut s = state_at(doc, "a");
    assert!(run_id(&mut s, "table.columnDelete"));
    assert_valid(&s, 1);
    assert_eq!(cells_of(&s), vec![vec!["b"], vec!["2"]]);
    assert!(s.undo());
    assert_eq!(s.doc(), doc);
}

/// Review of EP-007: decimals and negatives sort by value, numbers before
/// text.
#[test]
fn sorting_reads_decimals_and_negatives() {
    let doc = "| v |\n|---|\n| 1.5 |\n| tea |\n| -5 |\n| 1.25 |\n| -10 |\n| item 10 |\n| item 9 |";
    let mut s = state_at(doc, "v");
    assert!(run_id(&mut s, "table.sortAscending"));
    let values: Vec<String> = cells_of(&s)[1..].iter().map(|r| r[0].clone()).collect();
    assert_eq!(
        values,
        vec!["-10", "-5", "1.25", "1.5", "item 9", "item 10", "tea"]
    );
    assert_eq!(number("nan"), None);
    assert_eq!(number("inf"), None);
    assert_eq!(number("-"), None);
}
