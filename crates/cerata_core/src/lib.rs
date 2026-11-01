//! Cerata core: everything Cerata does with files, without a window.
//!
//! Workspace files, ignore rules, the file index and search, the watcher,
//! settings, sessions and recents. This crate must never depend on GPUI
//! (checked in CI by `scripts/check-core-deps.sh`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cli;
pub mod epoch;
pub mod fs;
pub mod ignore;
pub mod index;
pub mod json_store;
pub mod links;
pub mod markdown;
pub mod open_target;
pub mod paths;
pub mod pins;
pub mod recents;
pub mod save;
pub mod self_writes;
pub mod session;
pub mod settings;
pub mod startup;
pub mod terminal;
pub mod watcher;
pub mod workspace;
