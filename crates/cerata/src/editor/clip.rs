use cerata_markdown::table::Align;
use cerata_markdown::table_clip::{CellBlock, PASTE_LIMIT};
use serde::{Deserialize, Serialize};

const FORMAT: &str = "cerata.table-cells";
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellClip {
    format: String,
    version: u32,
    rows: Vec<Vec<String>>,
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
