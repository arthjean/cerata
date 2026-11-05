use std::fs;
use std::io;
use std::path::Path;

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    crate::fs::atomic::atomic_write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leftover_temp_files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp"))
            .collect()
    }

    #[test]
    fn atomic_write_replaces_the_whole_file_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config");
        write_atomic(&path, b"editor.font-size = 18\neditor.tab-size = 4\n").unwrap();
        write_atomic(&path, b"editor.zoom = 120\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "editor.zoom = 120\n");
        assert!(leftover_temp_files(&dir.path().join("nested")).is_empty());
    }

    #[test]
    fn atomic_write_failure_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "untouched").unwrap();

        assert!(write_atomic(&path, b"editor.font-size = 18\n").is_err());

        assert!(path.is_dir());
        assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "untouched");
        assert!(leftover_temp_files(dir.path()).is_empty());
    }
}
