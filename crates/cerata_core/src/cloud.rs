//! Best-effort detection of a folder synced by a cloud service (US-078),
//! for the warning under a library's location. Nothing is blocked on it.
//!
//! Recognized: an ancestor under `%OneDrive%`, `%OneDriveConsumer%`,
//! `%OneDriveCommercial%` or `%USERPROFILE%\iCloudDrive`; under
//! `~/Library/Mobile Documents` or `~/Library/CloudStorage/*`; or an
//! ancestor holding `.stfolder` (Syncthing) or `.dropbox` (Dropbox).

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

const ONEDRIVE_VARIABLES: [&str; 3] = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"];

/// The service that syncs `path`, if one is recognized.
pub fn synced_by(path: &Path) -> Option<String> {
    synced_by_with(
        path,
        &|name| std::env::var_os(name),
        dirs::home_dir().as_deref(),
    )
}

/// [`synced_by`] with the environment and the home folder supplied.
pub fn synced_by_with(
    path: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<String> {
    let folder = |name: &str| {
        env(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    for name in ONEDRIVE_VARIABLES {
        if folder(name).is_some_and(|root| is_under(path, &root)) {
            return Some("OneDrive".into());
        }
    }
    if folder("USERPROFILE").is_some_and(|profile| is_under(path, &profile.join("iCloudDrive"))) {
        return Some("iCloud Drive".into());
    }
    if let Some(home) = home {
        let library = home.join("Library");
        if is_under(path, &library.join("Mobile Documents")) {
            return Some("iCloud Drive".into());
        }
        let storage = library.join("CloudStorage");
        let provider = strip(path, &storage).and_then(|rest| match rest.components().next() {
            Some(Component::Normal(name)) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        });
        if let Some(provider) = provider {
            return Some(cloud_storage_provider(&provider));
        }
    }
    for ancestor in path.ancestors() {
        if ancestor.join(".stfolder").exists() {
            return Some("Syncthing".into());
        }
        if ancestor.join(".dropbox").exists() {
            return Some("Dropbox".into());
        }
    }
    None
}

/// `~/Library/CloudStorage/{Provider}-{account}`, as macOS File Provider
/// names it.
fn cloud_storage_provider(folder: &str) -> String {
    let provider = folder.split('-').next().unwrap_or(folder);
    match provider {
        "OneDrive" => "OneDrive".into(),
        "Dropbox" => "Dropbox".into(),
        "GoogleDrive" => "Google Drive".into(),
        "Box" => "Box".into(),
        "iCloudDrive" | "iCloud" => "iCloud Drive".into(),
        _ => "a cloud service".into(),
    }
}

/// Paths compare without case on Windows and macOS, as their default file
/// systems do.
fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/').to_owned();
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text
    }
}

fn is_under(path: &Path, root: &Path) -> bool {
    strip(path, root).is_some()
}

fn strip(path: &Path, root: &Path) -> Option<PathBuf> {
    let path_text = normalize(path);
    let root_text = normalize(root);
    if root_text.is_empty() {
        return None;
    }
    let rest = path_text.strip_prefix(&root_text)?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    // The remainder in the original case, by component count.
    let skip = Path::new(&root_text).components().count();
    Some(path.components().skip(skip).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn detect(path: &Path, vars: &[(&str, &Path)], home: Option<&Path>) -> Option<String> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.as_os_str().to_owned()))
            .collect();
        synced_by_with(path, &|name| vars.get(name).cloned(), home)
    }

    #[test]
    fn onedrive_folders_from_any_of_the_three_variables() {
        let dir = TempDir::new().unwrap();
        let onedrive = dir.path().join("OneDrive - Contoso");
        for var in ONEDRIVE_VARIABLES {
            assert_eq!(
                detect(&onedrive.join("Documents"), &[(var, &onedrive)], None).as_deref(),
                Some("OneDrive"),
                "{var}"
            );
        }
        // A sibling whose name only starts the same is not inside.
        let sibling = dir.path().join("OneDrive - Contoso Archive");
        assert_eq!(detect(&sibling, &[("OneDrive", &onedrive)], None), None);
    }

    #[test]
    fn icloud_on_windows_and_macos() {
        let dir = TempDir::new().unwrap();
        let profile = dir.path().join("user");
        assert_eq!(
            detect(
                &profile.join("iCloudDrive").join("Notes"),
                &[("USERPROFILE", &profile)],
                None
            )
            .as_deref(),
            Some("iCloud Drive")
        );
        let home = dir.path().join("home");
        let mobile = home.join("Library").join("Mobile Documents").join("x");
        assert_eq!(
            detect(&mobile, &[], Some(&home)).as_deref(),
            Some("iCloud Drive")
        );
    }

    #[test]
    fn cloud_storage_names_its_provider() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        let storage = home.join("Library").join("CloudStorage");
        let cases = [
            ("GoogleDrive-me@example.com", "Google Drive"),
            ("Dropbox", "Dropbox"),
            ("OneDrive-Personal", "OneDrive"),
            ("Acme-Drive", "a cloud service"),
        ];
        for (folder, provider) in cases {
            assert_eq!(
                detect(&storage.join(folder).join("Notes"), &[], Some(&home)).as_deref(),
                Some(provider),
                "{folder}"
            );
        }
    }

    #[test]
    fn syncthing_and_dropbox_markers_in_an_ancestor() {
        let dir = TempDir::new().unwrap();
        let synced = dir.path().join("Sync");
        std::fs::create_dir_all(synced.join(".stfolder")).unwrap();
        assert_eq!(
            detect(&synced.join("a").join("b"), &[], None).as_deref(),
            Some("Syncthing")
        );
        let dropbox = dir.path().join("Dropbox");
        std::fs::create_dir_all(&dropbox).unwrap();
        std::fs::write(dropbox.join(".dropbox"), "{}").unwrap();
        assert_eq!(
            detect(&dropbox.join("Notes"), &[], None).as_deref(),
            Some("Dropbox")
        );
        assert_eq!(detect(&dir.path().join("Plain"), &[], None), None);
    }
}
