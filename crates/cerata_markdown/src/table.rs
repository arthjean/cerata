//! GFM tables folded into a rendered grid (US-032). Ported from
//! `components/editor-area/table-decorations.ts`: the cell split and
//! alignment parse (`:110-162`), the height estimate (`:169-176`) and the
//! sizing that `tableTheme` asks the browser for (`:485-534`, cells capped
//! at 48ch, columns never narrower than their longest word).

use crate::preview::{self, Inline};

/// Ceiling on the width one column may demand, in `ch`.
pub const CELL_MAX_WIDTH_CH: f32 = 48.0;
/// Line height of a cell, relative to the font size.
pub const CELL_LINE_HEIGHT: f32 = 1.4;
/// Vertical padding of a cell (both sides together), in em.
pub const CELL_VERTICAL_PADDING_EM: f32 = 1.0;
/// Horizontal padding of a cell, per side, in em.
pub const CELL_HORIZONTAL_PADDING_EM: f32 = 0.8;
/// Padding above and below the table (both together), in em.
pub const WIDGET_VERTICAL_PADDING_EM: f32 = 0.5;
pub const BORDER_WIDTH: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Table {
    /// One per column, from the delimiter row.
    pub alignments: Vec<Option<Align>>,
    pub header: Vec<Vec<Inline>>,
    /// Body rows, each exactly as wide as the header: extra cells are
    /// dropped and missing ones are empty, as in GFM.
    pub rows: Vec<Vec<Vec<Inline>>>,
}

impl Table {
    pub fn columns(&self) -> usize {
        self.header.len()
    }
}

fn escaped_at(text: &str, index: usize) -> bool {
    text.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|b| **b == b'\\')
        .count()
        % 2
        == 1
}

/// `parseCells`: the cells of a row, trimmed; a leading and a trailing
/// unescaped `|` are dropped. `\|` stays in the cell.
pub fn cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let start = usize::from(trimmed.starts_with('|'));
    let end = if trimmed.len() > start
        && trimmed.ends_with('|')
        && !escaped_at(trimmed, trimmed.len() - 1)
    {
        trimmed.len() - 1
    } else {
        trimmed.len()
    };
    let mut out = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in trimmed[start..end.max(start)].chars() {
        if c == '|' && !escaped {
            out.push(current.trim().to_owned());
            current.clear();
        } else {
            current.push(c);
        }
        escaped = !escaped && c == '\\';
    }
    out.push(current.trim().to_owned());
    out
}

fn alignment(cell: &str) -> Option<Align> {
    match (cell.starts_with(':'), cell.ends_with(':')) {
        (true, true) => Some(Align::Center),
        (false, true) => Some(Align::Right),
        (true, false) => Some(Align::Left),
        (false, false) => None,
    }
}

fn is_delimiter(cell: &str) -> bool {
    let inner = cell.strip_prefix(':').unwrap_or(cell);
    let inner = inner.strip_suffix(':').unwrap_or(inner);
    !inner.is_empty() && inner.bytes().all(|b| b == b'-')
}

/// Parses a table's source (`parseMarkdownTable`), `None` when its second
/// line is not a delimiter row. Lines may carry a quote prefix, which the
/// tree already accounted for: `strip` removes it.
pub fn parse(source: &str, strip: impl Fn(&str) -> &str) -> Option<Table> {
    let lines: Vec<&str> = source
        .lines()
        .map(strip)
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let header = cells(lines[0]);
    let delimiter = cells(lines[1]);
    if !delimiter.iter().all(|c| is_delimiter(c)) {
        return None;
    }
    let columns = header.len();
    let alignments = (0..columns)
        .map(|i| delimiter.get(i).and_then(|c| alignment(c)))
        .collect();
    let rows = lines[2..]
        .iter()
        .map(|line| {
            let mut row = cells(line);
            row.resize(columns, String::new());
            row.iter().map(|c| preview::render(c)).collect()
        })
        .collect();
    Some(Table {
        alignments,
        header: header.iter().map(|c| preview::render(c)).collect(),
        rows,
    })
}

/// `estimateTableWidgetHeight`: one text line per row, before layout.
pub fn estimate_height(table: &Table, font_size: f32) -> f32 {
    let visual_rows = 1 + table.rows.len();
    let row_height = font_size * (CELL_LINE_HEIGHT + CELL_VERTICAL_PADDING_EM);
    let padding = font_size * WIDGET_VERTICAL_PADDING_EM;
    let borders = (visual_rows + 1) as f32 * BORDER_WIDTH;
    (padding + visual_rows as f32 * row_height + borders).ceil()
}

/// Column widths for an `auto` table layout: each column gets at least its
/// longest word (`min`) and asks for its content capped at `cap`
/// (`max`, never below `min`). When the asks fit in `available` they are
/// granted; else the room above the minimums is shared in proportion to
/// what each column asks beyond its minimum. Widths include padding.
pub fn size_columns(min: &[f32], max: &[f32], available: f32, cap: f32) -> Vec<f32> {
    let asks: Vec<f32> = min
        .iter()
        .zip(max)
        .map(|(lo, hi)| hi.min(cap).max(*lo))
        .collect();
    let total_ask: f32 = asks.iter().sum();
    if total_ask <= available {
        return asks;
    }
    let total_min: f32 = min.iter().sum();
    if total_min >= available {
        return min.to_vec();
    }
    let spare = available - total_min;
    let extra: f32 = asks.iter().zip(min).map(|(a, m)| a - m).sum();
    asks.iter()
        .zip(min)
        .map(|(a, m)| {
            if extra > 0.0 {
                m + spare * (a - m) / extra
            } else {
                *m
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(source: &str) -> Table {
        parse(source, |l| l).unwrap()
    }

    #[test]
    fn splits_cells_and_reads_alignments() {
        assert_eq!(cells("| a | b \\| c | d |"), vec!["a", "b \\| c", "d"]);
        assert_eq!(cells("a|b"), vec!["a", "b"]);
        assert_eq!(cells("| a \\|"), vec!["a \\|"]);
        let t = table("| L | C | R | N |\n|:--|:-:|--:|---|\n| 1 | 2 | 3 | 4 |");
        assert_eq!(
            t.alignments,
            vec![
                Some(Align::Left),
                Some(Align::Center),
                Some(Align::Right),
                None
            ]
        );
        assert!(parse("| a |\n| b |", |l| l).is_none());
    }

    /// GFM: extra body cells are dropped, missing ones are empty.
    #[test]
    fn rows_take_the_header_width() {
        let t = table("| a | b |\n| - | - |\n| 1 | 2 | 3 |\n| only |");
        assert_eq!(t.rows[0].len(), 2);
        assert_eq!(preview::plain_text(&t.rows[0][1]), "2");
        assert_eq!(t.rows[1].len(), 2);
        assert!(t.rows[1][1].is_empty());
    }

    /// `table-decorations.test.ts:148-163`.
    #[test]
    fn height_estimate_is_positive_and_grows_with_rows() {
        let short = table("| Name | Count |\n| --- | ---: |\n| Tea | 2 |");
        assert!(estimate_height(&short, 16.0) > 70.0);
        let mut long = String::from("| Name | Count |\n| --- | ---: |\n");
        for i in 1..=12 {
            long.push_str(&format!("| Row {i} | {i} |\n"));
        }
        assert!(estimate_height(&table(&long), 16.0) > estimate_height(&short, 16.0));
    }

    #[test]
    fn columns_size_between_longest_word_and_capped_content() {
        // Everything fits: each column gets its content.
        assert_eq!(
            size_columns(&[10.0, 20.0], &[30.0, 40.0], 100.0, 500.0),
            vec![30.0, 40.0]
        );
        // A long prose column asks for at most the cap.
        assert_eq!(
            size_columns(&[10.0, 20.0], &[30.0, 900.0], 1000.0, 300.0),
            vec![30.0, 300.0]
        );
        // Too narrow: the spare room is shared, nobody below its word.
        let w = size_columns(&[10.0, 20.0], &[110.0, 220.0], 130.0, 500.0);
        assert!(w[0] >= 10.0 && w[1] >= 20.0);
        assert!((w.iter().sum::<f32>() - 130.0).abs() < 1e-3);
        // Narrower than the words: words win, no word is cut.
        assert_eq!(
            size_columns(&[50.0, 60.0], &[80.0, 90.0], 40.0, 500.0),
            vec![50.0, 60.0]
        );
    }
}
