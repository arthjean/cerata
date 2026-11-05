use std::path::{Path, PathBuf};

pub const APP_ID: &str = "cerata";

pub const WORKSPACE_DIR: &str = ".cerata";

pub const CONFIG_FILE: &str = "config";

pub const LIBRARY_ID_FILE: &str = "id";

pub fn library_marker_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_DIR).join(LIBRARY_ID_FILE)
}

pub fn global_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_ID))
}

pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join(APP_ID))
}

pub fn local_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(APP_ID))
}

pub fn workspace_config_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_DIR).join(CONFIG_FILE)
}

pub fn default_library_parent() -> Option<PathBuf> {
    dirs::home_dir()
}

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
