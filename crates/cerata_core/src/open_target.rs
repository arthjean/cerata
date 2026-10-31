//! Classifies a path handed to Cerata (argv, CLI, OS "open with").
//!
//! Ported from Writer `open_target.rs`. A folder opens as a workspace, a
//! `.md` or `.markdown` file (any case, the shared predicate) as a standalone
//! file; both are canonicalized first.

use crate::markdown::is_markdown_path;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTarget {
    Workspace(PathBuf),
    File(PathBuf),
}

impl OpenTarget {
    pub fn path(&self) -> &Path {
        match self {
            Self::Workspace(path) | Self::File(path) => path,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenTargetError {
    #[error("no such file or folder: {}", .0.display())]
    NotFound(PathBuf),
    #[error("not a folder or Markdown file: {}", .0.display())]
    Unsupported(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Strict classification, for the CLI: reports why a path cannot open.
pub fn validate_and_resolve(path: &Path) -> Result<OpenTarget, OpenTargetError> {
    if !path.exists() {
        return Err(OpenTargetError::NotFound(path.to_path_buf()));
    }
    let metadata = path.metadata().map_err(|source| OpenTargetError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let canonical = || dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if metadata.is_dir() {
        return Ok(OpenTarget::Workspace(canonical()));
    }
    if metadata.is_file() && is_markdown_path(path) {
        return Ok(OpenTarget::File(canonical()));
    }
    Err(OpenTargetError::Unsupported(path.to_path_buf()))
}

/// Lenient classification, for argv and OS open requests.
pub fn resolve(path: &Path) -> Option<OpenTarget> {
    validate_and_resolve(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // Ported from Writer `open_target.rs` tests.

    #[test]
    fn directory_resolves_to_workspace() {
        let dir = tempdir().unwrap();
        assert_eq!(
            validate_and_resolve(dir.path()).unwrap(),
            OpenTarget::Workspace(dunce::canonicalize(dir.path()).unwrap())
        );
    }

    #[test]
    fn markdown_file_resolves_to_standalone_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("note.md");
        fs::write(&file, "hello").unwrap();
        assert_eq!(
            validate_and_resolve(&file).unwrap(),
            OpenTarget::File(dunce::canonicalize(&file).unwrap())
        );
    }

    #[test]
    fn markdown_extension_matches_both_md_and_markdown_case_insensitively() {
        let dir = tempdir().unwrap();
        for name in ["a.md", "b.MD", "c.markdown", "d.MARKDOWN"] {
            let path = dir.path().join(name);
            fs::write(&path, "").unwrap();
            assert!(
                matches!(validate_and_resolve(&path), Ok(OpenTarget::File(_))),
                "{name}"
            );
        }
    }

    #[test]
    fn non_markdown_file_is_unsupported() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("image.png");
        fs::write(&file, "").unwrap();
        assert!(matches!(
            validate_and_resolve(&file),
            Err(OpenTargetError::Unsupported(_))
        ));
    }

    #[test]
    fn missing_path_is_not_found() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            validate_and_resolve(&dir.path().join("nope.md")),
            Err(OpenTargetError::NotFound(_))
        ));
    }

    #[test]
    fn lenient_resolver_returns_none_for_unsupported() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("image.png");
        fs::write(&file, "").unwrap();
        assert!(resolve(&file).is_none());
    }

    #[test]
    fn lenient_resolver_returns_none_for_missing() {
        let dir = tempdir().unwrap();
        assert!(resolve(&dir.path().join("nope")).is_none());
    }
}
