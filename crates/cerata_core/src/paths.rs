//! Application identity and the directories Cerata reads and writes.
//!
//! Cerata's identifiers are its own so it never collides with a Writer
//! install (PLAN.md section 3.3).

use std::path::{Path, PathBuf};

/// Directory name under the OS config and data directories.
pub const APP_ID: &str = "cerata";

/// Name of the per-workspace directory, the only thing Cerata writes into a
/// workspace (`{root}/.cerata/config`).
pub const WORKSPACE_DIR: &str = ".cerata";

/// File name of both the global and the workspace config.
pub const CONFIG_FILE: &str = "config";

/// OS config directory for Cerata: `~/Library/Application Support/cerata`
/// on macOS, `%APPDATA%\cerata` on Windows, `$XDG_CONFIG_HOME/cerata` on
/// Linux. `None` when the OS reports no config directory.
pub fn global_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_ID))
}

/// OS data directory for Cerata (sessions, recents).
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join(APP_ID))
}

/// `{root}/.cerata/config`.
pub fn workspace_config_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_DIR).join(CONFIG_FILE)
}
