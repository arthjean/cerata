use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::{FsError, WorkspaceFs};

pub fn assets_dir(note: &Path) -> Option<PathBuf> {
    let stem = note.file_stem()?.to_string_lossy();
    Some(note.with_file_name(format!("{stem}-assets")))
}

fn existing_assets(note: &Path) -> Option<PathBuf> {
    assets_dir(note).filter(|dir| fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetsOutcome {
    None,
    Followed { from: PathBuf, to: PathBuf },
    Stayed { folder: PathBuf, taken: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteOperation {
    pub path: PathBuf,
    pub assets: AssetsOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRewrite {
    pub from: String,
    pub to: String,
}

impl NoteOperation {
    pub fn links(&self, old: &Path) -> Option<LinkRewrite> {
        let actual = match &self.assets {
            AssetsOutcome::None => return None,
            AssetsOutcome::Followed { to, .. } => to,
            AssetsOutcome::Stayed { folder, .. } => folder,
        };
        let from = assets_dir(old)?.file_name()?.to_string_lossy().into_owned();
        let to = relative_between(self.path.parent()?, actual)?;
        (from != to).then_some(LinkRewrite { from, to })
    }

    pub fn notice(&self) -> Option<String> {
        let AssetsOutcome::Stayed { folder, taken } = &self.assets else {
            return None;
        };
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        Some(format!(
            "Images stay in \"{}\": \"{}\" already exists",
            name(folder),
            name(taken)
        ))
    }
}

pub fn relative_between(from: &Path, to: &Path) -> Option<String> {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    if from.first() != to.first() {
        return None;
    }
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl WorkspaceFs {
    pub fn rename_note(&self, old: &Path, new: &Path) -> Result<NoteOperation, FsError> {
        let note = self.confine_entry(old)?;
        let Some(folder) = existing_assets(&note) else {
            return Ok(NoteOperation {
                path: self.rename(old, new)?,
                assets: AssetsOutcome::None,
            });
        };
        let target = assets_dir(&self.confine_new(new)?)
            .ok_or_else(|| FsError::NotFound(new.to_path_buf()))?;
        match self.rename(&folder, &target) {
            Ok(moved) => match self.rename(&note, new) {
                Ok(path) => Ok(NoteOperation {
                    path,
                    assets: AssetsOutcome::Followed {
                        from: folder,
                        to: moved,
                    },
                }),
                Err(error) => {
                    if let Err(back) = fs::rename(&moved, &folder) {
                        log::error!("assets folder {} not moved back: {back}", moved.display());
                    }
                    Err(error)
                }
            },
            Err(FsError::AlreadyExists(taken)) => Ok(NoteOperation {
                path: self.rename(&note, new)?,
                assets: AssetsOutcome::Stayed { folder, taken },
            }),
            Err(error) => Err(error),
        }
    }

    pub fn duplicate_note(
        &self,
        source: &Path,
        unsaved: Option<&str>,
    ) -> Result<NoteOperation, FsError> {
        self.duplicate_note_with(source, unsaved, &|from, to| fs::copy(from, to))
    }

    fn duplicate_note_with(
        &self,
        source: &Path,
        unsaved: Option<&str>,
        copy_file: &dyn Fn(&Path, &Path) -> io::Result<u64>,
    ) -> Result<NoteOperation, FsError> {
        let source_file = self.read_file(source)?;
        let Some(folder) = existing_assets(&source_file.path) else {
            return Ok(NoteOperation {
                path: self.duplicate(source, unsaved)?,
                assets: AssetsOutcome::None,
            });
        };
        let candidate = self.reserve_copy(&source_file.path)?;
        let target = assets_dir(&candidate).ok_or_else(|| FsError::NotFound(candidate.clone()))?;
        let assets = match self.copy_folder(&folder, &target, copy_file) {
            Ok(()) => AssetsOutcome::Followed {
                from: folder,
                to: target,
            },
            Err(FsError::AlreadyExists(taken)) if taken == target => AssetsOutcome::Stayed {
                folder,
                taken: target,
            },
            Err(error) => {
                let _ = fs::remove_file(&candidate);
                return Err(error);
            }
        };
        let operation = NoteOperation {
            path: candidate.clone(),
            assets,
        };
        let content = unsaved.unwrap_or(&source_file.content);
        let rewritten = operation.links(&source_file.path).and_then(|links| {
            cerata_markdown::asset_links::rewrite_text(content, &links.from, &links.to)
        });
        if let Err(error) = self.fill_copy(
            &candidate,
            rewritten.as_deref().unwrap_or(content),
            source_file.format,
        ) {
            if let AssetsOutcome::Followed { to, .. } = &operation.assets {
                let _ = fs::remove_dir_all(to);
            }
            return Err(error);
        }
        Ok(operation)
    }

    fn copy_folder(
        &self,
        from: &Path,
        to: &Path,
        copy_file: &dyn Fn(&Path, &Path) -> io::Result<u64>,
    ) -> Result<(), FsError> {
        let to = self.confine_new(to)?;
        fs::create_dir(&to).map_err(|e| FsError::io(&to, e))?;
        let copied = self.copy_tree(from, &to, copy_file);
        if copied.is_err() {
            let _ = fs::remove_dir_all(&to);
        }
        copied
    }

    fn copy_tree(
        &self,
        from: &Path,
        to: &Path,
        copy_file: &dyn Fn(&Path, &Path) -> io::Result<u64>,
    ) -> Result<(), FsError> {
        for entry in fs::read_dir(from).map_err(|e| FsError::io(from, e))? {
            let entry = entry.map_err(|e| FsError::io(from, e))?;
            let kind = entry
                .file_type()
                .map_err(|e| FsError::io(&entry.path(), e))?;
            let destination = self.confine_new(&to.join(entry.file_name()))?;
            if kind.is_dir() {
                fs::create_dir(&destination).map_err(|e| FsError::io(&destination, e))?;
                self.copy_tree(&entry.path(), &destination, copy_file)?;
            } else if kind.is_file() {
                copy_file(&entry.path(), &destination).map_err(|e| FsError::io(&destination, e))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ws(dir: &TempDir) -> WorkspaceFs {
        WorkspaceFs::new(dir.path()).unwrap()
    }

    fn note(root: &Path, dir: &str) -> PathBuf {
        let parent = root.join(dir);
        fs::create_dir_all(parent.join("foo-assets/sub")).unwrap();
        fs::write(parent.join("foo-assets/x.png"), "x").unwrap();
        fs::write(parent.join("foo-assets/sub/y.png"), "y").unwrap();
        let note = parent.join("foo.md");
        fs::write(&note, "![x](foo-assets/x.png)").unwrap();
        note
    }

    #[test]
    fn a_rename_takes_the_folder_along() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let old = note(&root, "");
        let done = ws.rename_note(&old, &root.join("bar.md")).unwrap();
        assert_eq!(done.path, root.join("bar.md"));
        assert!(root.join("bar-assets/x.png").is_file());
        assert!(root.join("bar-assets/sub/y.png").is_file());
        assert!(!root.join("foo-assets").exists());
        assert_eq!(
            done.links(&old),
            Some(LinkRewrite {
                from: "foo-assets".into(),
                to: "bar-assets".into()
            })
        );
        assert_eq!(done.notice(), None);
    }

    #[test]
    fn a_move_keeps_the_folder_name_and_the_text() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let old = note(&root, "a");
        fs::create_dir(root.join("b")).unwrap();
        let done = ws.rename_note(&old, &root.join("b/foo.md")).unwrap();
        assert!(root.join("b/foo.md").is_file());
        assert!(root.join("b/foo-assets/x.png").is_file());
        assert!(!root.join("a/foo-assets").exists());
        assert_eq!(done.links(&old), None, "a move rewrites nothing");
    }

    #[test]
    fn a_taken_folder_name_leaves_the_folder_and_points_the_links_at_it() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let old = note(&root, "a");
        fs::create_dir_all(root.join("b/foo-assets")).unwrap();
        let done = ws.rename_note(&old, &root.join("b/foo.md")).unwrap();
        assert!(root.join("b/foo.md").is_file(), "the note moves");
        assert!(
            root.join("a/foo-assets/x.png").is_file(),
            "the folder stays"
        );
        assert_eq!(
            done.links(&old),
            Some(LinkRewrite {
                from: "foo-assets".into(),
                to: "../a/foo-assets".into()
            })
        );
        assert_eq!(
            done.notice().as_deref(),
            Some("Images stay in \"foo-assets\": \"foo-assets\" already exists")
        );
        let other = note(&root, "c");
        fs::create_dir(root.join("c/bar-assets")).unwrap();
        let done = ws.rename_note(&other, &root.join("c/bar.md")).unwrap();
        assert_eq!(done.links(&other), None);
        assert_eq!(
            done.notice().as_deref(),
            Some("Images stay in \"foo-assets\": \"bar-assets\" already exists")
        );
    }

    #[test]
    fn a_note_that_cannot_follow_brings_the_folder_back() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let old = note(&root, "");
        fs::write(root.join("bar.md"), "other").unwrap();
        let error = ws.rename_note(&old, &root.join("bar.md")).unwrap_err();
        assert!(matches!(error, FsError::AlreadyExists(_)), "{error}");
        assert!(old.is_file());
        assert!(
            root.join("foo-assets/x.png").is_file(),
            "the folder came back"
        );
        assert!(!root.join("bar-assets").exists());
        assert_eq!(fs::read_to_string(root.join("bar.md")).unwrap(), "other");
    }

    #[test]
    fn a_case_only_rename_changes_the_folder_case() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        fs::create_dir(root.join("Foo-assets")).unwrap();
        fs::write(root.join("Foo-assets/x.png"), "x").unwrap();
        fs::write(root.join("Foo.md"), "![x](Foo-assets/x.png)").unwrap();
        let done = ws
            .rename_note(&root.join("Foo.md"), &root.join("foo.md"))
            .unwrap();
        let names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"foo-assets".to_owned()), "{names:?}");
        assert!(names.contains(&"foo.md".to_owned()), "{names:?}");
        assert_eq!(
            done.links(&root.join("Foo.md")).map(|l| l.to),
            Some("foo-assets".to_owned())
        );
    }

    #[test]
    fn a_note_without_a_folder_renames_as_before() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        fs::write(root.join("a.md"), "a").unwrap();
        let done = ws
            .rename_note(&root.join("a.md"), &root.join("b.md"))
            .unwrap();
        assert_eq!(
            done,
            NoteOperation {
                path: root.join("b.md"),
                assets: AssetsOutcome::None
            }
        );
        assert_eq!(done.links(&root.join("a.md")), None);
    }

    #[test]
    fn a_duplicate_copies_the_folder_and_rewrites_its_links() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let source = note(&root, "");
        let done = ws
            .duplicate_note(&source, Some("![x](foo-assets/x.png) unsaved"))
            .unwrap();
        assert_eq!(done.path, root.join("foo copy.md"));
        assert_eq!(
            fs::read_to_string(&done.path).unwrap(),
            "![x](<foo copy-assets/x.png>) unsaved"
        );
        assert!(root.join("foo copy-assets/x.png").is_file());
        assert!(root.join("foo copy-assets/sub/y.png").is_file());
        assert!(
            root.join("foo-assets/x.png").is_file(),
            "the source keeps its own"
        );
        fs::remove_file(root.join("foo copy-assets/x.png")).unwrap();
        assert!(
            root.join("foo-assets/x.png").is_file(),
            "two independent copies"
        );
    }

    #[test]
    fn a_duplicate_over_a_taken_folder_name_keeps_the_links() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let source = note(&root, "");
        fs::create_dir(root.join("foo copy-assets")).unwrap();
        let done = ws.duplicate_note(&source, None).unwrap();
        assert_eq!(
            fs::read_to_string(&done.path).unwrap(),
            "![x](foo-assets/x.png)"
        );
        assert_eq!(
            fs::read_dir(root.join("foo copy-assets")).unwrap().count(),
            0
        );
        assert_eq!(
            done.notice().as_deref(),
            Some("Images stay in \"foo-assets\": \"foo copy-assets\" already exists")
        );
    }

    #[test]
    fn a_failed_copy_leaves_neither_folder_nor_note() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        let source = note(&root, "");
        let calls = std::cell::Cell::new(0);
        let failing = |from: &Path, to: &Path| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                return Err(io::Error::other("disk full"));
            }
            fs::copy(from, to)
        };
        let error = ws.duplicate_note_with(&source, None, &failing).unwrap_err();
        assert!(error.to_string().contains("disk full"), "{error}");
        assert!(!root.join("foo copy-assets").exists());
        assert!(!root.join("foo copy.md").exists());
        assert!(root.join("foo-assets/x.png").is_file());
    }

    #[test]
    fn a_note_without_a_folder_duplicates_as_before() {
        let dir = TempDir::new().unwrap();
        let ws = ws(&dir);
        let root = ws.root().to_path_buf();
        fs::write(root.join("a.md"), "![x](a-assets/x.png)").unwrap();
        let done = ws.duplicate_note(&root.join("a.md"), None).unwrap();
        assert_eq!(done.assets, AssetsOutcome::None);
        assert_eq!(
            fs::read_to_string(root.join("a copy.md")).unwrap(),
            "![x](a-assets/x.png)"
        );
    }

    #[test]
    fn relative_paths_between_folders() {
        let base = if cfg!(windows) { "C:/w" } else { "/w" };
        let p = |s: &str| PathBuf::from(format!("{base}/{s}"));
        assert_eq!(
            relative_between(&p("a"), &p("a/x-assets")).as_deref(),
            Some("x-assets")
        );
        assert_eq!(
            relative_between(&p("b/c"), &p("a/x-assets")).as_deref(),
            Some("../../a/x-assets")
        );
    }
}
