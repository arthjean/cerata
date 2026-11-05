use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const RENAME_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(100), Duration::from_millis(200)];

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with(path, bytes, |file, bytes| file.write_all(bytes))
}

pub(crate) fn atomic_write_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let target = resolve_write_target(path)?;
    let existing = fs::metadata(&target).ok();
    if existing
        .as_ref()
        .is_some_and(|meta| meta.permissions().readonly())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is read-only", target.display()),
        ));
    }
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let temp = temp_path(dir, &target);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;

    let result = (|| {
        if let Some(meta) = &existing {
            file.set_permissions(meta.permissions())?;
        }
        write(&mut file, bytes)?;
        file.sync_all()?;
        drop(file);
        rename_with_retry(&temp, &target)?;
        sync_dir(dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn atomic_create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let temp = temp_path(dir, path);
    let linked = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temp, path)
    })();
    let _ = fs::remove_file(&temp);
    match linked {
        Ok(()) => {
            sync_dir(dir);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(error),
        Err(error) if fs::symlink_metadata(path).is_ok() => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists ({error})", path.display()),
        )),
        Err(link_error) => {
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            let written = file.write_all(bytes).and_then(|()| file.sync_all());
            if let Err(error) = written {
                drop(file);
                let _ = fs::remove_file(path);
                return Err(error);
            }
            log::debug!(
                "{}: created without a hard link ({link_error})",
                path.display()
            );
            sync_dir(dir);
            Ok(())
        }
    }
}

fn resolve_write_target(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => dunce::canonicalize(path),
        _ => Ok(path.to_path_buf()),
    }
}

fn temp_path(dir: &Path, target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        ".{name}.{}-{unique}.cerata-tmp",
        std::process::id()
    ))
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut delays = RENAME_RETRY_DELAYS.iter();
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if is_transient_lock(&error) => match delays.next() {
                Some(delay) => std::thread::sleep(*delay),
                None => return Err(error),
            },
            Err(error) => return Err(error),
        }
    }
}

fn is_transient_lock(error: &io::Error) -> bool {
    if !cfg!(windows) {
        return false;
    }
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".cerata-tmp"))
            .collect()
    }

    #[test]
    fn atomic_create_never_replaces_and_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("id");
        atomic_create(&path, b"first\n").unwrap();
        let error = atomic_create(&path, b"second\n").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn writes_and_replaces_atomically() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn failed_write_keeps_original_and_leaves_no_partial_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        fs::write(&path, "original").unwrap();
        let error = atomic_write_with(&path, b"new content", |file, bytes| {
            file.write_all(&bytes[..3])?;
            Err(io::Error::other("No space left on device"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "No space left on device");
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn read_only_file_is_refused_and_untouched() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("locked.md");
        fs::write(&path, "original").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions.clone()).unwrap();
        let error = atomic_write(&path, b"new").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert!(leftovers(dir.path()).is_empty());
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_written_through_and_permissions_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.md");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        atomic_write(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[cfg(windows)]
    #[test]
    fn symlink_is_written_through_when_the_os_allows_creating_one() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.md");
        fs::write(&target, "old").unwrap();
        let link = dir.path().join("link.md");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            eprintln!("skipped: this account cannot create symlinks");
            return;
        }
        atomic_write(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[cfg(windows)]
    fn open_without_delete_sharing(path: &Path) -> File {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn rename_blocked_by_another_process_is_retried_three_times_over_300ms() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("held.md");
        fs::write(&path, "original").unwrap();
        let holder = open_without_delete_sharing(&path);
        let started = std::time::Instant::now();
        let error = atomic_write(&path, b"new").unwrap_err();
        let elapsed = started.elapsed();
        drop(holder);
        assert!(is_transient_lock(&error), "{error:?}");
        assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn rename_succeeds_once_the_other_process_lets_go() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("held.md");
        fs::write(&path, "original").unwrap();
        let holder = open_without_delete_sharing(&path);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(holder);
        });
        atomic_write(&path, b"new").unwrap();
        release.join().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }
}
