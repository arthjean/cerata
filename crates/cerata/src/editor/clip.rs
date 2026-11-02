//! The grid metadata a copied rectangle of cells carries on the clipboard
//! (US-055, ADR 0007 decision 10): GPUI moves text and JSON metadata, so
//! the text is a GFM table and the metadata the cells themselves.

use cerata_markdown::table::Align;
use cerata_markdown::table_clip::{CellBlock, PASTE_LIMIT};
use serde::{Deserialize, Serialize};

/// Names the metadata, so a paste ignores anything else.
const FORMAT: &str = "cerata.table-cells";
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellClip {
    format: String,
    version: u32,
    /// Each cell's source, row by row.
    rows: Vec<Vec<String>>,
    /// `left`, `center`, `right` or `null`, per column.
    alignments: Vec<Option<String>>,
}

impl CellClip {
    pub fn from_block(block: &CellBlock) -> Self {
        Self {
            format: FORMAT.into(),
            version: VERSION,
            rows: block.rows.clone(),
            alignments: block
                .alignments
                .iter()
                .map(|a| {
                    a.map(|a| {
                        match a {
                            Align::Left => "left",
                            Align::Center => "center",
                            Align::Right => "right",
                        }
                        .to_owned()
                    })
                })
                .collect(),
        }
    }

    /// The block in `json`; `None` when it is not valid JSON or not ours,
    /// and the text is pasted instead.
    pub fn parse(json: &str) -> Option<CellBlock> {
        let clip: CellClip = serde_json::from_str(json).ok()?;
        if clip.format != FORMAT || clip.version != VERSION {
            return None;
        }
        let alignments = clip
            .alignments
            .iter()
            .map(|a| match a.as_deref() {
                Some("left") => Some(Align::Left),
                Some("center") => Some(Align::Center),
                Some("right") => Some(Align::Right),
                _ => None,
            })
            .collect();
        Some(
            CellBlock {
                rows: clip.rows,
                alignments,
            }
            .escaped(),
        )
    }
}

/// `12345` as `12,345`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The notice of a paste over the limit.
pub fn too_large(count: usize) -> String {
    format!(
        "Paste canceled: {} cells are over the {} limit",
        grouped(count),
        grouped(PASTE_LIMIT)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metadata_round_trips_and_rejects_strangers() {
        let block = CellBlock {
            rows: vec![vec!["a\\|b".into(), String::new()]],
            alignments: vec![Some(Align::Right), None],
        };
        let json = serde_json::to_string(&CellClip::from_block(&block)).unwrap();
        assert_eq!(CellClip::parse(&json), Some(block));
        assert_eq!(CellClip::parse("{not json"), None);
        assert_eq!(
            CellClip::parse(r#"{"format":"other","version":1,"rows":[],"alignments":[]}"#),
            None
        );
        assert_eq!(
            too_large(10_100),
            "Paste canceled: 10,100 cells are over the 10,000 limit"
        );
    }
}
