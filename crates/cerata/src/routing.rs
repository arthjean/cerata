//! Where an open request lands: Writer's `handleOpenPayload`
//! (`use-open-drop.ts:23-62`) and its Rust side (`lib.rs:101-180`,
//! `:461-490`) as one pure decision, so every case is testable without a
//! window.

use std::path::{Path, PathBuf};

use cerata_core::markdown::is_markdown_path;
use cerata_core::open_target::{self, OpenTarget};

/// What to open: a workspace, optionally with a file in it, or a file on its
/// own (Writer's `{workspace, file}` payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenRequest {
    Workspace {
        root: PathBuf,
        file: Option<PathBuf>,
    },
    File(PathBuf),
}

impl OpenRequest {
    /// A launch target (argv, CLI, second instance): a folder is a
    /// workspace, a Markdown file opens on its own (`lib.rs:461-490`).
    pub fn from_launch(target: OpenTarget) -> Self {
        match target {
            OpenTarget::Workspace(root) => Self::Workspace { root, file: None },
            OpenTarget::File(file) => Self::File(file),
        }
    }

    /// A path dropped on a window: a folder is a workspace, a Markdown file
    /// opens in its folder's workspace (`open-flow.md:22-31`). `None` for
    /// anything else.
    pub fn from_drop(path: &Path) -> Option<Self> {
        match open_target::resolve(path)? {
            OpenTarget::Workspace(root) => Some(Self::Workspace { root, file: None }),
            OpenTarget::File(file) if is_markdown_path(&file) => {
                let root = file.parent()?.to_path_buf();
                Some(Self::Workspace {
                    root,
                    file: Some(file),
                })
            }
            OpenTarget::File(_) => None,
        }
    }
}

/// The window chrome (`compact-mode.ts`): a workspace window with tabs and
/// sidebar, or a compact window on one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chrome {
    Workspace,
    Compact,
}

/// The window a request comes from, when it has one (drops do, launches
/// do not).
#[derive(Clone, Copy, Debug)]
pub struct Origin<'a> {
    pub id: u64,
    pub root: Option<&'a Path>,
    pub chrome: Chrome,
}

/// The other windows, looked up by what they show.
pub trait Hosts {
    fn workspace_window(&self, root: &Path) -> Option<u64>;
    fn file_window(&self, file: &Path) -> Option<u64>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Bring window `id` forward and open `file` in it, if any.
    Reveal {
        id: u64,
        file: Option<PathBuf>,
    },
    /// Open the workspace (and file) in the origin window.
    WorkspaceHere {
        root: PathBuf,
        file: Option<PathBuf>,
    },
    /// Open the file in the origin window, which already shows its workspace.
    FileHere(PathBuf),
    /// Turn the origin window into a compact window on the file.
    CompactHere(PathBuf),
    NewWorkspaceWindow {
        root: PathBuf,
        file: Option<PathBuf>,
    },
    NewCompactWindow(PathBuf),
}

pub fn route(request: OpenRequest, origin: Option<Origin<'_>>, hosts: &impl Hosts) -> Route {
    match request {
        OpenRequest::File(file) => {
            if let Some(id) = hosts.file_window(&file) {
                return Route::Reveal { id, file: None };
            }
            match origin {
                Some(origin) if origin.root.is_none() => Route::CompactHere(file),
                _ => Route::NewCompactWindow(file),
            }
        }
        OpenRequest::Workspace { root, file } => {
            if let Some(origin) = origin
                && origin.root == Some(root.as_path())
            {
                return match file {
                    Some(file) => Route::FileHere(file),
                    None => Route::Reveal {
                        id: origin.id,
                        file: None,
                    },
                };
            }
            if let Some(id) = hosts.workspace_window(&root) {
                return Route::Reveal { id, file };
            }
            match origin {
                Some(origin) if origin.root.is_none() && origin.chrome == Chrome::Workspace => {
                    Route::WorkspaceHere { root, file }
                }
                _ => Route::NewWorkspaceWindow { root, file },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        workspaces: HashMap<PathBuf, u64>,
        files: HashMap<PathBuf, u64>,
    }

    impl Hosts for Fake {
        fn workspace_window(&self, root: &Path) -> Option<u64> {
            self.workspaces.get(root).copied()
        }
        fn file_window(&self, file: &Path) -> Option<u64> {
            self.files.get(file).copied()
        }
    }

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn origin(root: Option<&Path>, chrome: Chrome) -> Option<Origin<'_>> {
        Some(Origin {
            id: 1,
            root,
            chrome,
        })
    }

    fn ws(root: &str, file: Option<&str>) -> OpenRequest {
        OpenRequest::Workspace {
            root: p(root),
            file: file.map(p),
        }
    }

    #[test]
    fn a_file_alone_on_a_workspace_window_opens_a_standalone_window() {
        let root = p("/vault");
        let route = route(
            OpenRequest::File(p("/x/a.md")),
            origin(Some(&root), Chrome::Workspace),
            &Fake::default(),
        );
        assert_eq!(route, Route::NewCompactWindow(p("/x/a.md")));
    }

    #[test]
    fn a_file_alone_on_a_rootless_window_turns_it_compact() {
        for chrome in [Chrome::Workspace, Chrome::Compact] {
            let route = route(
                OpenRequest::File(p("/x/a.md")),
                origin(None, chrome),
                &Fake::default(),
            );
            assert_eq!(route, Route::CompactHere(p("/x/a.md")));
        }
    }

    #[test]
    fn a_file_already_shown_on_its_own_reveals_its_window() {
        let mut hosts = Fake::default();
        hosts.files.insert(p("/x/a.md"), 7);
        let route = route(OpenRequest::File(p("/x/a.md")), None, &hosts);
        assert_eq!(route, Route::Reveal { id: 7, file: None });
    }

    #[test]
    fn a_launch_without_origin_opens_new_windows() {
        let hosts = Fake::default();
        assert_eq!(
            route(OpenRequest::File(p("/a.md")), None, &hosts),
            Route::NewCompactWindow(p("/a.md"))
        );
        assert_eq!(
            route(ws("/v", None), None, &hosts),
            Route::NewWorkspaceWindow {
                root: p("/v"),
                file: None
            }
        );
    }

    #[test]
    fn an_open_workspace_brings_its_window_forward_with_the_file() {
        let mut hosts = Fake::default();
        hosts.workspaces.insert(p("/v"), 3);
        let other = p("/other");
        let route = route(
            ws("/v", Some("/v/a.md")),
            origin(Some(&other), Chrome::Workspace),
            &hosts,
        );
        assert_eq!(
            route,
            Route::Reveal {
                id: 3,
                file: Some(p("/v/a.md"))
            }
        );
    }

    #[test]
    fn the_same_workspace_opens_the_file_in_place() {
        let root = p("/v");
        let here = origin(Some(&root), Chrome::Workspace);
        assert_eq!(
            route(ws("/v", Some("/v/a.md")), here, &Fake::default()),
            Route::FileHere(p("/v/a.md"))
        );
        assert_eq!(
            route(ws("/v", None), here, &Fake::default()),
            Route::Reveal { id: 1, file: None }
        );
    }

    #[test]
    fn another_folder_opens_a_new_window() {
        let root = p("/v");
        assert_eq!(
            route(
                ws("/w", None),
                origin(Some(&root), Chrome::Workspace),
                &Fake::default()
            ),
            Route::NewWorkspaceWindow {
                root: p("/w"),
                file: None
            }
        );
    }

    #[test]
    fn a_folder_on_a_compact_window_opens_a_new_window() {
        assert_eq!(
            route(
                ws("/w", None),
                origin(None, Chrome::Compact),
                &Fake::default()
            ),
            Route::NewWorkspaceWindow {
                root: p("/w"),
                file: None
            }
        );
    }

    #[test]
    fn a_folder_on_an_empty_workspace_window_opens_there() {
        assert_eq!(
            route(
                ws("/w", Some("/w/a.md")),
                origin(None, Chrome::Workspace),
                &Fake::default()
            ),
            Route::WorkspaceHere {
                root: p("/w"),
                file: Some(p("/w/a.md"))
            }
        );
    }

    #[test]
    fn a_dropped_markdown_file_opens_in_its_folder_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let Some(OpenTarget::Workspace(root)) = open_target::resolve(dir.path()) else {
            panic!("a folder resolves to a workspace");
        };
        std::fs::write(root.join("a.md"), "").unwrap();
        std::fs::write(root.join("b.txt"), "").unwrap();
        assert_eq!(
            OpenRequest::from_drop(&root.join("a.md")),
            Some(OpenRequest::Workspace {
                root: root.clone(),
                file: Some(root.join("a.md"))
            })
        );
        assert_eq!(
            OpenRequest::from_drop(&root),
            Some(OpenRequest::Workspace {
                root: root.clone(),
                file: None
            })
        );
        assert_eq!(OpenRequest::from_drop(&root.join("b.txt")), None);
        assert_eq!(OpenRequest::from_drop(&root.join("missing.md")), None);
    }
}
