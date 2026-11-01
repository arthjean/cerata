//! Markdown model for Cerata: buffer, syntax tree, decoration rules and
//! editing commands. This crate must never depend on GPUI (checked in CI by
//! `scripts/check-core-deps.sh`), so every rule stays testable without a window.
//!
//! Ported from writer-computer's `prosemark-core` (MIT, J. Simon Richard,
//! see `NOTICE`) and editor extensions (GPL-3.0-or-later), commit `79b744c`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod change;
pub mod code;
pub mod commands;
pub mod decorate;
pub mod editing;
pub mod frontmatter;
pub mod history;
pub mod list;
pub mod motion;
pub mod selection;
pub mod state;
pub mod text;
pub mod tree;

pub use change::{Bias, Change, ChangeError, ChangeSet};
pub use decorate::{Decoration, HideMode, LineStyle, MarkStyle, Widget, decorate};
pub use history::UserEvent;
pub use motion::Vertical;
pub use selection::{Selection, SelectionRange};
pub use state::{Anchor, Applied, EditorState, ScrollAnchor, Transaction, TransactionError};
pub use text::{Line, Text};
pub use tree::{MarkdownTree, Node, NodeKind};
