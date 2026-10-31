//! Markdown model for Cerata: buffer, syntax tree, decoration rules and
//! editing commands. This crate must never depend on GPUI (checked in CI by
//! `scripts/check-core-deps.sh`), so every rule stays testable without a window.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
