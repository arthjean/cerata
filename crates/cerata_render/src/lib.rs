#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod highlight;
pub mod js;

pub use highlight::{
    HighlightKind, HighlightSpan, highlight, highlight_code_blocks, language_for, languages,
};
