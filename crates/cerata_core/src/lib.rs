//! Cerata core: everything Cerata does with files, without a window.
//!
//! Workspace files, ignore rules, the file index and search, the watcher,
//! settings, libraries, sessions and recents, and each library's local
//! store (index and, later, history). This crate must never depend on GPUI
//! (checked in CI by `scripts/check-core-deps.sh`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cli;
pub mod cloud;
pub mod conflict_copy;
pub mod epoch;
pub mod fs;
pub mod history;
pub mod ignore;
pub mod images;
pub mod index;
pub mod json_store;
pub mod library;
pub mod links;
pub mod markdown;
pub mod merge;
pub mod open_target;
pub mod paths;
pub mod pins;
pub mod recents;
pub mod reconcile;
pub mod save;
pub mod self_writes;
pub mod session;
pub mod settings;
pub mod similarity;
pub mod startup;
pub mod store;
pub mod terminal;
pub mod watcher;
pub mod workspace;
