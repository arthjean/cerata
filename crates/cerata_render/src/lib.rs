//! Rendering helpers whose dependencies are too heavy for `cerata_markdown`
//! (tree-sitter grammars, compiled C), extracted as `PLAN.md` foresees once
//! they arrive. No GPUI: checked in CI by `scripts/check-core-deps.sh`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod highlight;

pub use highlight::{
    HighlightKind, HighlightSpan, highlight, highlight_code_blocks, language_for, languages,
};
