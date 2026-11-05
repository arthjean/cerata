use super::*;
use crate::selection::Selection;

const DOC: &str = "| a | b |\n|:--|--:|\n| 1 | 2 |\n| 3 | x\\|y |\n\nend";

fn at(doc: &str, needle: &str) -> EditorState {
    let mut s = EditorState::new(doc);
    s.select(Selection::cursor(doc.find(needle).unwrap()))
        .unwrap();
    s
}

fn grid(s: &EditorState) -> Grid {
    cells::grid_at(s, 0).unwrap()
}

#[test]
fn a_rectangle_copies_as_a_gfm_table_and_a_grid() {
    let mut s = at(DOC, "1");
    let g = grid(&s);
    s.select_cells(&g, (2, 0), (3, 1));
    let block = s.copied_cells().unwrap();
    assert_eq!(
        block.rows,
        vec![
            vec!["1".to_owned(), "2".into()],
            vec!["3".into(), "x\\|y".into()]
        ]
    );
    assert_eq!(
        block.alignments,
        vec![Some(Align::Left), Some(Align::Right)]
    );
    assert_eq!(block.to_gfm(), "| 1 | 2 |\n| :-- | --: |\n| 3 | x\\|y |");
    assert_eq!(parse_gfm(&block.to_gfm()), Some(block));
    assert!(at(DOC, "1").copied_cells().is_none(), "no rectangle");
}

#[test]
fn cut_empties_the_cells_in_one_step() {
    let mut s = at(DOC, "1");
    let g = grid(&s);
    s.select_cells(&g, (2, 0), (2, 1));
    assert!(s.cut_cells());
    assert_eq!(
        s.doc(),
        "| a | b |\n|:--|--:|\n|  |  |\n| 3 | x\\|y |\n\nend"
    );
    assert!(s.undo());
    assert_eq!(s.doc(), DOC);
    assert!(!s.undo());
}

#[test]
fn a_grid_fills_cells_from_the_caret_and_grows_the_table() {
    let block = CellBlock {
        rows: vec![
            vec!["p".into(), "q".into(), "r".into()],
            vec!["s".into(), "t".into(), "u".into()],
        ],
        alignments: Vec::new(),
    };
    let mut s = at(DOC, "3");
    assert_eq!(s.paste_cells(&block), PasteCells::Pasted);
    assert_eq!(
        s.doc(),
        "| a | b |   |\n|:--|--:| --- |\n| 1 | 2 |   |\n| p | q | r |\n| s | t | u |\n\nend"
    );
    let g = grid(&s);
    assert_eq!(g.columns(), 3);
    assert!(g.rows.iter().all(|r| r.cells.len() == 3));
    let head = s.selection().main().head;
    assert_eq!(&s.doc()[head - 1..head], "u");
    assert!(s.undo(), "one step");
    assert_eq!(s.doc(), DOC);
    let mut s = at(DOC, "1");
    let g = grid(&s);
    s.select_cells(&g, (3, 1), (2, 1));
    let one = CellBlock {
        rows: vec![vec!["z".into()]],
        alignments: Vec::new(),
    };
    assert_eq!(s.paste_cells(&one), PasteCells::Pasted);
    assert!(s.doc().contains("| 1 | z |"), "{:?}", s.doc());
    assert_eq!(at(DOC, "end").paste_cells(&one), PasteCells::Outside);
}

#[test]
fn pasted_pipes_and_breaks_are_escaped() {
    let tsv = "a|b\tc\nd\te\r\n";
    let block = parse_tsv(tsv).unwrap();
    assert_eq!(
        block.rows,
        vec![
            vec!["a\\|b".to_owned(), "c".into()],
            vec!["d".into(), "e".into()]
        ]
    );
    assert!(parse_tsv("one cell").is_none());
    assert!(
        parse_tsv("two\nlines").is_none(),
        "no tab: text, not a grid"
    );
    let raw = CellBlock {
        rows: vec![vec!["x|y\nz".into(), "\\|".into()]],
        alignments: Vec::new(),
    };
    let mut s = at(DOC, "1");
    assert_eq!(s.paste_cells(&raw), PasteCells::Pasted);
    assert!(s.doc().contains("| x\\|y<br>z | \\| |"), "{:?}", s.doc());
    assert_eq!(grid(&s).columns(), 2);
}

#[test]
fn too_many_cells_paste_nothing() {
    let block = CellBlock {
        rows: vec![vec![String::new(); 101]; 100],
        alignments: Vec::new(),
    };
    let mut s = at(DOC, "1");
    assert_eq!(s.paste_cells(&block), PasteCells::TooLarge(10_100));
    assert_eq!(s.doc(), DOC);
}

#[test]
fn only_a_whole_table_parses_as_one() {
    assert!(parse_gfm("text\n\n| a |\n|---|\n").is_none());
    assert!(parse_gfm("| a |\n|---|\n| b |\n").is_some());
    assert!(parse_gfm("plain").is_none());
}

#[test]
fn the_limit_counts_the_rectangle_filled() {
    let mut rows = vec![vec!["x".to_owned(); 200]];
    rows.extend(std::iter::repeat_n(Vec::new(), 60));
    let block = CellBlock {
        rows,
        alignments: Vec::new(),
    };
    assert_eq!(block.cell_count(), 200);
    let mut s = at(DOC, "1");
    assert_eq!(s.paste_cells(&block), PasteCells::TooLarge(61 * 200));
    assert_eq!(s.doc(), DOC);
}

#[test]
fn quoted_spreadsheet_cells_stay_one_cell() {
    let block = parse_tsv("\"l1\nl2\"\tx\r\n\"say \"\"hi\"\"\"\t\"a\tb\"\r\n").unwrap();
    assert_eq!(
        block.rows,
        vec![
            vec!["l1<br>l2".to_owned(), "x".to_owned()],
            vec!["say \"hi\"".to_owned(), "a\tb".to_owned()],
        ]
    );
    let block = parse_tsv("\"open\tx\ny\tz").unwrap();
    assert_eq!(block.rows[0], vec!["\"open".to_owned(), "x".to_owned()]);
}
