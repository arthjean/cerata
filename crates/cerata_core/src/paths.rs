//! Application identity and the directories Cerata reads and writes.
//!
//! Cerata's identifiers are its own so it never collides with a Writer
//! install (PLAN.md section 3.3).

use std::path::{Path, PathBuf};

/// Directory name under the OS config and data directories.
pub const APP_ID: &str = "cerata";

/// Name of the per-library directory, which holds `config` and `id`
/// (ADR 0009 write rule).
pub const WORKSPACE_DIR: &str = ".cerata";

/// File name of both the global and the workspace config.
pub const CONFIG_FILE: &str = "config";

/// File name of a library's identity marker (`{root}/.cerata/id`).
pub const LIBRARY_ID_FILE: &str = "id";

/// `{root}/.cerata/id`.
pub fn library_marker_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_DIR).join(LIBRARY_ID_FILE)
}

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

/// OS local data directory for Cerata, never the roaming one: the library
/// stores live there (ADR 0009). `%LOCALAPPDATA%\cerata` on Windows.
pub fn local_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(APP_ID))
}

/// `{root}/.cerata/config`.
pub fn workspace_config_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_DIR).join(CONFIG_FILE)
}

/// The folder the welcome form proposes for a first library (`~`).
pub fn default_library_parent() -> Option<PathBuf> {
    dirs::home_dir()
}

/// `path` for display, with the home folder shown as `~`.
pub fn display_path(path: &Path) -> String {
    display_path_from(path, dirs::home_dir().as_deref())
}

fn display_path_from(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_shows_as_a_tilde() {
        let home = Path::new(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        });
        assert_eq!(display_path_from(home, Some(home)), "~");
        let notes = home.join("Cerata");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            display_path_from(&notes, Some(home)),
            format!("~{sep}Cerata")
        );
        let other = Path::new(if cfg!(windows) {
            r"D:\Notes"
        } else {
            "/srv/notes"
        });
        assert_eq!(
            display_path_from(other, Some(home)),
            other.display().to_string()
        );
        assert_eq!(display_path_from(other, None), other.display().to_string());
    }
}
