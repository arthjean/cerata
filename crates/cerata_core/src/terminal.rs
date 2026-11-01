//! "Open in Terminal" for a workspace folder.
//!
//! Ported from Writer `commands/workspace.rs:61-342`: the launch commands
//! per OS (`workspace.default-terminal` overrides them and is taken as one
//! opaque program name), the validation of the target (a folder inside the
//! workspace, symlinks resolved) and the guard that refuses to launch once
//! the window has moved to another workspace or the folder was replaced.

use crate::workspace::{WindowState, WorkspaceSnapshot};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const DEFAULT_TERMINAL_KEY: &str = "workspace.default-terminal";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchPlatform {
    MacOs,
    Windows,
    Linux,
}

impl LaunchPlatform {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Self::MacOs,
            "windows" => Self::Windows,
            _ => Self::Linux,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalExecution {
    /// Started and left running.
    Detached,
    /// Run to completion (macOS `open`); a failure exit is an error.
    WaitForSuccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
    /// Windows: a console of its own (`CREATE_NEW_CONSOLE`).
    pub new_console: bool,
    pub execution: TerminalExecution,
}

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("no such folder: {}", .0.display())]
    NotFound(PathBuf),
    #[error("{0}")]
    InvalidPath(String),
    #[error("{0}")]
    Io(String),
}

/// The commands to try, in order, to open a terminal in `target_dir`.
pub fn terminal_launch_specs(
    platform: LaunchPlatform,
    target_dir: &Path,
    preferred_terminal: Option<&str>,
    terminal_env: Option<&str>,
) -> Vec<TerminalLaunchSpec> {
    let spec = |program: &str, args: Vec<String>, new_console, execution| TerminalLaunchSpec {
        program: program.to_owned(),
        args,
        current_dir: target_dir.to_path_buf(),
        new_console,
        execution,
    };
    let dir = target_dir.to_string_lossy().into_owned();
    match (platform, preferred_terminal) {
        (LaunchPlatform::MacOs, program) => vec![spec(
            "open",
            vec!["-a".into(), program.unwrap_or("Terminal").into(), dir],
            false,
            TerminalExecution::WaitForSuccess,
        )],
        (LaunchPlatform::Windows, Some(program)) => {
            vec![spec(program, Vec::new(), true, TerminalExecution::Detached)]
        }
        (LaunchPlatform::Windows, None) => vec![spec(
            "cmd.exe",
            vec!["/K".into()],
            true,
            TerminalExecution::Detached,
        )],
        (LaunchPlatform::Linux, Some(program)) => {
            vec![spec(
                program,
                Vec::new(),
                false,
                TerminalExecution::Detached,
            )]
        }
        (LaunchPlatform::Linux, None) => {
            let mut programs: Vec<String> = terminal_env
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .into_iter()
                .collect();
            for fallback in [
                "x-terminal-emulator",
                "ghostty",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal",
            ] {
                if !programs.iter().any(|program| program == fallback) {
                    programs.push(fallback.to_owned());
                }
            }
            programs
                .iter()
                .map(|program| spec(program, Vec::new(), false, TerminalExecution::Detached))
                .collect()
        }
    }
}

/// The canonical form of an existing folder.
pub fn validate_workspace_launch_root(path: &Path) -> Result<PathBuf, TerminalError> {
    if !path.is_dir() {
        return Err(TerminalError::NotFound(path.to_path_buf()));
    }
    dunce::canonicalize(path).map_err(|error| TerminalError::Io(error.to_string()))
}

/// The folder to open: `requested` (or the root), which must resolve inside
/// a root that still canonicalizes to `captured_root`.
pub fn validate_terminal_target(
    captured_root: &Path,
    requested: Option<&Path>,
) -> Result<PathBuf, TerminalError> {
    let canonical_root = validate_workspace_launch_root(captured_root)?;
    if canonical_root != captured_root {
        return Err(TerminalError::InvalidPath(
            captured_root.display().to_string(),
        ));
    }
    let requested = requested.unwrap_or(captured_root);
    let target = validate_workspace_launch_root(requested)?;
    if !target.starts_with(&canonical_root) {
        return Err(TerminalError::InvalidPath(requested.display().to_string()));
    }
    Ok(target)
}

/// Runs `launch` only while `window` still shows the workspace of
/// `snapshot` and `requested` still resolves to `captured_target`, holding
/// the workspace lock so no switch can interleave.
pub fn with_terminal_launch_snapshot<T>(
    window: &WindowState,
    snapshot: &WorkspaceSnapshot,
    requested: Option<&Path>,
    captured_target: &Path,
    launch: impl FnOnce() -> Result<T, TerminalError>,
) -> Result<T, TerminalError> {
    window
        .with_snapshot(snapshot, || {
            let live_target = validate_terminal_target(&snapshot.root, requested)?;
            if live_target != captured_target {
                return Err(TerminalError::InvalidPath(
                    "Selected folder changed before the terminal could be opened".into(),
                ));
            }
            launch()
        })
        .ok_or_else(|| {
            TerminalError::InvalidPath(
                "Workspace changed before the terminal could be opened".into(),
            )
        })?
}

/// "Open folder" (the workspace root in the file manager), guarded the same
/// way.
pub fn open_workspace_in_file_manager_for_snapshot<T>(
    window: &WindowState,
    snapshot: &WorkspaceSnapshot,
    open: impl FnOnce(&Path) -> Result<T, TerminalError>,
) -> Result<T, TerminalError> {
    window
        .with_snapshot(snapshot, || {
            let live_root = validate_workspace_launch_root(&snapshot.root)?;
            if live_root != snapshot.root {
                return Err(TerminalError::InvalidPath(
                    "Workspace folder changed before it could be opened".into(),
                ));
            }
            open(&live_root)
        })
        .ok_or_else(|| {
            TerminalError::InvalidPath("Workspace changed before it could be opened".into())
        })?
}

pub fn terminal_launch_error(preferred_terminal: Option<&str>, reason: &str) -> TerminalError {
    match preferred_terminal {
        Some(terminal) => TerminalError::Io(format!(
            "Could not launch configured terminal {terminal:?}: {reason}. Clear or reset Default Terminal in Preferences."
        )),
        None => TerminalError::Io(reason.to_owned()),
    }
}

/// Tries each launch spec in order until one starts. `spawn` runs the
/// command (behind the snapshot guard in the app, directly in tests); its
/// inner `Err` is a failed start, its outer one aborts.
pub fn launch_terminal(
    target_dir: &Path,
    preferred_terminal: &str,
    mut spawn: impl FnMut(&mut Command) -> Result<Result<Child, String>, TerminalError>,
) -> Result<(), TerminalError> {
    let preferred = (!preferred_terminal.trim().is_empty()).then_some(preferred_terminal.trim());
    let terminal_env = std::env::var("TERMINAL").ok();
    let specs = terminal_launch_specs(
        LaunchPlatform::current(),
        target_dir,
        preferred,
        terminal_env.as_deref(),
    );
    let mut last_error = None;
    for launch in specs {
        let mut command = Command::new(&launch.program);
        command.args(&launch.args).current_dir(&launch.current_dir);
        #[cfg(windows)]
        if launch.new_console {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            command.creation_flags(CREATE_NEW_CONSOLE);
        }
        match launch.execution {
            TerminalExecution::Detached => match spawn(&mut command)? {
                Ok(_) => return Ok(()),
                Err(error) => last_error = Some(error),
            },
            TerminalExecution::WaitForSuccess => {
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
                let output = match spawn(&mut command)? {
                    Ok(child) => child.wait_with_output(),
                    Err(error) => {
                        last_error = Some(error);
                        continue;
                    }
                };
                match output {
                    Ok(output) if output.status.success() => return Ok(()),
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let stderr = stderr.trim();
                        last_error = Some(if stderr.is_empty() {
                            format!("{} exited with {}", launch.program, output.status)
                        } else {
                            stderr.to_owned()
                        });
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
        }
    }
    Err(terminal_launch_error(
        preferred,
        last_error
            .as_deref()
            .unwrap_or("No terminal launcher is available"),
    ))
}

/// Opens a terminal in `requested` (the root when `None`) of the workspace
/// `window` shows now.
pub fn open_in_terminal(
    window: &WindowState,
    requested: Option<&Path>,
) -> Result<(), TerminalError> {
    let snapshot = window
        .snapshot()
        .ok_or_else(|| TerminalError::InvalidPath("No workspace is open".into()))?;
    let target = validate_terminal_target(&snapshot.root, requested)?;
    let preferred = window
        .settings()
        .get(DEFAULT_TERMINAL_KEY)
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    launch_terminal(&target, &preferred, |command| {
        with_terminal_launch_snapshot(window, &snapshot, requested, &target, || {
            Ok(command.spawn().map_err(|error| error.to_string()))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::workspace::{WindowRegistry, open_workspace};
    use std::sync::Arc;
    use std::sync::mpsc;
    use tempfile::TempDir;

    // Ported from Writer `commands/workspace.rs` tests.

    #[test]
    fn terminal_launch_specs_cover_defaults_and_explicit_preferences_exactly() {
        let root = Path::new("/workspace with spaces");

        let mac = terminal_launch_specs(LaunchPlatform::MacOs, root, None, None);
        assert_eq!(mac.len(), 1);
        assert_eq!(mac[0].program, "open");
        assert_eq!(mac[0].args, ["-a", "Terminal", "/workspace with spaces"]);
        assert_eq!(mac[0].current_dir, root);
        assert!(!mac[0].new_console);
        assert_eq!(mac[0].execution, TerminalExecution::WaitForSuccess);

        let custom_mac = terminal_launch_specs(LaunchPlatform::MacOs, root, Some("iTerm"), None);
        assert_eq!(custom_mac.len(), 1);
        assert_eq!(custom_mac[0].program, "open");
        assert_eq!(
            custom_mac[0].args,
            ["-a", "iTerm", "/workspace with spaces"]
        );
        assert_eq!(custom_mac[0].execution, TerminalExecution::WaitForSuccess);

        let windows = terminal_launch_specs(LaunchPlatform::Windows, root, None, None);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].program, "cmd.exe");
        assert_eq!(windows[0].args, ["/K"]);
        assert_eq!(windows[0].current_dir, root);
        assert!(windows[0].new_console);
        assert_eq!(windows[0].execution, TerminalExecution::Detached);

        let custom_windows = terminal_launch_specs(
            LaunchPlatform::Windows,
            root,
            Some("C:\\Apps\\wt.exe"),
            None,
        );
        assert_eq!(custom_windows.len(), 1);
        assert_eq!(custom_windows[0].program, "C:\\Apps\\wt.exe");
        assert!(custom_windows[0].args.is_empty());
        assert_eq!(custom_windows[0].current_dir, root);
        assert!(custom_windows[0].new_console);
        assert_eq!(custom_windows[0].execution, TerminalExecution::Detached);

        let linux = terminal_launch_specs(LaunchPlatform::Linux, root, None, Some(" ghostty "));
        assert_eq!(
            linux.iter().map(|s| s.program.as_str()).collect::<Vec<_>>(),
            [
                "ghostty",
                "x-terminal-emulator",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal"
            ]
        );
        assert!(linux.iter().all(|s| s.args.is_empty()));
        assert!(linux.iter().all(|s| s.current_dir == root));
        assert!(linux.iter().all(|s| !s.new_console));
        assert!(
            linux
                .iter()
                .all(|s| s.execution == TerminalExecution::Detached)
        );

        let linux_without_env = terminal_launch_specs(LaunchPlatform::Linux, root, None, None);
        assert_eq!(
            linux_without_env
                .iter()
                .map(|s| s.program.as_str())
                .collect::<Vec<_>>(),
            [
                "x-terminal-emulator",
                "ghostty",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal"
            ]
        );

        let opaque = "ghostty --execute 'not a flag'";
        let custom_linux =
            terminal_launch_specs(LaunchPlatform::Linux, root, Some(opaque), Some("other"));
        assert_eq!(custom_linux.len(), 1);
        assert_eq!(custom_linux[0].program, opaque);
        assert!(custom_linux[0].args.is_empty());
    }

    #[test]
    fn custom_terminal_errors_identify_the_preference_and_recovery() {
        let message =
            terminal_launch_error(Some("Missing Terminal"), "application could not be found")
                .to_string();
        assert!(message.contains("Missing Terminal"));
        assert!(message.contains("Default Terminal"));
        assert!(message.contains("Preferences"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_invalid_terminal_app_surfaces_the_open_helper_failure() {
        let dir = TempDir::new().unwrap();
        let preference = "Cerata Definitely Missing Terminal 8B112EE0";
        let message = launch_terminal(dir.path(), preference, |command| {
            Ok(command.spawn().map_err(|error| error.to_string()))
        })
        .unwrap_err()
        .to_string();
        assert!(message.contains(preference));
        assert!(message.contains("Default Terminal"));
        assert!(message.contains("Preferences"));
    }

    /// The same contract checked on every OS: a configured terminal that
    /// cannot start names the preference and the way out.
    #[test]
    fn a_missing_configured_terminal_names_the_preference() {
        let dir = TempDir::new().unwrap();
        let preference = "cerata-definitely-missing-terminal-8b112ee0";
        let message = launch_terminal(dir.path(), preference, |command| {
            Ok(command.spawn().map_err(|error| error.to_string()))
        })
        .unwrap_err()
        .to_string();
        assert!(message.contains(preference));
        assert!(message.contains("Default Terminal"));
    }

    #[test]
    fn launch_root_validation_rejects_files_and_missing_paths() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("note.md");
        std::fs::write(&file, "# Note").unwrap();
        assert!(matches!(
            validate_workspace_launch_root(&file),
            Err(TerminalError::NotFound(_))
        ));
        assert!(matches!(
            validate_workspace_launch_root(&dir.path().join("missing")),
            Err(TerminalError::NotFound(_))
        ));
    }

    #[test]
    fn terminal_target_validation_accepts_root_and_nested_directories() {
        let workspace = TempDir::new().unwrap();
        let nested = workspace.path().join("drafts").join("chapter one");
        std::fs::create_dir_all(&nested).unwrap();
        let root = dunce::canonicalize(workspace.path()).unwrap();
        assert_eq!(validate_terminal_target(&root, None).unwrap(), root);
        assert_eq!(
            validate_terminal_target(&root, Some(&nested)).unwrap(),
            dunce::canonicalize(&nested).unwrap()
        );
    }

    #[test]
    fn terminal_target_validation_rejects_files_missing_paths_and_siblings() {
        let workspace = TempDir::new().unwrap();
        let sibling = TempDir::new().unwrap();
        let file = workspace.path().join("note.md");
        std::fs::write(&file, "# Note").unwrap();
        let root = dunce::canonicalize(workspace.path()).unwrap();
        assert!(matches!(
            validate_terminal_target(&root, Some(&file)),
            Err(TerminalError::NotFound(_))
        ));
        assert!(matches!(
            validate_terminal_target(&root, Some(&workspace.path().join("missing"))),
            Err(TerminalError::NotFound(_))
        ));
        assert!(matches!(
            validate_terminal_target(&root, Some(sibling.path())),
            Err(TerminalError::InvalidPath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_target_validation_accepts_internal_symlinks_and_rejects_external_ones() {
        use std::os::unix::fs::symlink;
        let workspace = TempDir::new().unwrap();
        let nested = workspace.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let internal = workspace.path().join("internal-link");
        symlink(&nested, &internal).unwrap();
        let outside = TempDir::new().unwrap();
        let external = workspace.path().join("external-link");
        symlink(outside.path(), &external).unwrap();
        let root = dunce::canonicalize(workspace.path()).unwrap();
        assert_eq!(
            validate_terminal_target(&root, Some(&internal)).unwrap(),
            dunce::canonicalize(&nested).unwrap()
        );
        assert!(matches!(
            validate_terminal_target(&root, Some(&external)),
            Err(TerminalError::InvalidPath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_target_validation_rejects_a_replaced_workspace_root() {
        use std::os::unix::fs::symlink;
        let parent = TempDir::new().unwrap();
        let root_path = parent.path().join("workspace");
        std::fs::create_dir(&root_path).unwrap();
        let captured = dunce::canonicalize(&root_path).unwrap();
        std::fs::rename(&root_path, parent.path().join("moved-workspace")).unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), &root_path).unwrap();
        assert!(matches!(
            validate_terminal_target(&captured, None),
            Err(TerminalError::InvalidPath(_))
        ));
    }

    fn window() -> (TempDir, Arc<WindowState>) {
        let config = TempDir::new().unwrap();
        let (settings, _) = Settings::open(config.path()).unwrap();
        let registry = WindowRegistry::new();
        (config, registry.create(settings))
    }

    fn open(window: &Arc<WindowState>, root: &Path) -> WorkspaceSnapshot {
        let (sender, _events) = mpsc::channel();
        open_workspace(window, root, sender).unwrap();
        window.snapshot().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn terminal_launch_boundary_rejects_delayed_target_and_root_replacement() {
        use std::os::unix::fs::symlink;
        let parent = TempDir::new().unwrap();
        let root_path = parent.path().join("workspace");
        let selected = root_path.join("drafts");
        std::fs::create_dir_all(&selected).unwrap();
        let (_config, state) = window();
        let snapshot = open(&state, &root_path);
        let target = validate_terminal_target(&snapshot.root, Some(&selected)).unwrap();

        let outside = TempDir::new().unwrap();
        std::fs::rename(&selected, root_path.join("moved-drafts")).unwrap();
        symlink(outside.path(), &selected).unwrap();
        assert!(matches!(
            with_terminal_launch_snapshot(&state, &snapshot, Some(&selected), &target, || Ok(())),
            Err(TerminalError::InvalidPath(_))
        ));

        std::fs::remove_file(&selected).unwrap();
        let root_target = validate_terminal_target(&snapshot.root, None).unwrap();
        std::fs::rename(&root_path, parent.path().join("moved-workspace")).unwrap();
        symlink(outside.path(), &root_path).unwrap();
        assert!(matches!(
            with_terminal_launch_snapshot(&state, &snapshot, None, &root_target, || Ok(())),
            Err(TerminalError::InvalidPath(_))
        ));
    }

    #[test]
    fn terminal_launch_snapshot_rejects_workspace_switches_and_epoch_aba() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let (_config, state) = window();
        let snapshot = open(&state, first.path());
        let target = snapshot.root.clone();
        with_terminal_launch_snapshot(&state, &snapshot, None, &target, || Ok(())).unwrap();

        open(&state, second.path());
        assert!(matches!(
            with_terminal_launch_snapshot(&state, &snapshot, None, &target, || Ok(())),
            Err(TerminalError::InvalidPath(_))
        ));
        // Back on the first root, but in a newer epoch: still refused.
        open(&state, first.path());
        assert!(matches!(
            with_terminal_launch_snapshot(&state, &snapshot, None, &target, || Ok(())),
            Err(TerminalError::InvalidPath(_))
        ));
    }

    #[test]
    fn guarded_launch_helpers_reject_stale_callbacks_and_hold_the_workspace_read_guard() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let (_config, state) = window();
        let snapshot = open(&state, first.path());
        let root = snapshot.root.clone();

        let ran = std::cell::Cell::new(false);
        with_terminal_launch_snapshot(&state, &snapshot, None, &root, || {
            ran.set(true);
            assert!(
                state.workspace_is_locked(),
                "launch runs under the read guard"
            );
            Ok(())
        })
        .unwrap();
        assert!(ran.get());

        open(&state, second.path());
        ran.set(false);
        assert!(matches!(
            with_terminal_launch_snapshot(&state, &snapshot, None, &root, || {
                ran.set(true);
                Ok(())
            }),
            Err(TerminalError::InvalidPath(_))
        ));
        assert!(!ran.get());

        let opened = std::cell::Cell::new(false);
        assert!(matches!(
            open_workspace_in_file_manager_for_snapshot(&state, &snapshot, |_| {
                opened.set(true);
                Ok(())
            }),
            Err(TerminalError::InvalidPath(_))
        ));
        assert!(!opened.get());
    }
}
