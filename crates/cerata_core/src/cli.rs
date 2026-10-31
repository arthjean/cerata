//! The `cerata` command-line launcher.
//!
//! Ported from Writer `writer_cli.rs` (hand-rolled argv parsing, launcher
//! trait, exit codes 0, 2 and 3) with Writer defect 10 fixed: Writer chose
//! CLI mode when `argv[0]` matched `writer` case-insensitively, so the app
//! binary `Writer` and the CLI `writer` were indistinguishable on
//! case-insensitive file systems. Cerata enters CLI mode only through an
//! exact, case-sensitive binary name ([`CLI_BINARY`]) or an explicit
//! subcommand ([`CLI_SUBCOMMAND`]).

use crate::open_target::{self, OpenTarget};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_RUNTIME: u8 = 3;

/// File stem of a binary (or link) that always runs the CLI.
pub const CLI_BINARY: &str = "cerata-cli";
/// First argument of the app binary that runs the CLI: `cerata cli PATH`.
pub const CLI_SUBCOMMAND: &str = "cli";
/// Overrides the app binary the CLI launches.
pub const APP_PATH_ENV: &str = "CERATA_APP_PATH";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "\
Usage: cerata cli [PATH]
       cerata-cli [PATH]

Open a folder or Markdown file in Cerata.

Arguments:
  PATH              Folder or .md/.markdown file to open, relative to the
                    current directory. If omitted, Cerata starts with no target.

Options:
  -h, --help        Print this help and exit.
  -V, --version     Print version and exit.

Environment:
  CERATA_APP_PATH   Path of the Cerata app binary to launch.
";

/// The CLI arguments (program name first) when this invocation is a CLI
/// call, `None` when it should start the app. Matching is exact and
/// case-sensitive.
pub fn cli_args(argv: &[OsString]) -> Option<Vec<OsString>> {
    let program = argv.first()?;
    let stem = Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str());
    if stem == Some(CLI_BINARY) {
        return Some(argv.to_vec());
    }
    if argv.get(1).and_then(|arg| arg.to_str()) == Some(CLI_SUBCOMMAND) {
        let mut rest = vec![program.clone()];
        rest.extend(argv.iter().skip(2).cloned());
        return Some(rest);
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedArgs {
    Help,
    Version,
    Open { path: Option<PathBuf> },
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
enum ParseError {
    #[error("unknown option: {0}")]
    UnknownFlag(String),
    #[error("expected at most one path argument")]
    TooManyArgs,
}

fn parse_args(argv: &[OsString]) -> Result<ParsedArgs, ParseError> {
    let mut positional: Option<PathBuf> = None;
    for arg in argv.iter().skip(1) {
        if let Some(flag) = arg.to_str() {
            match flag {
                "--help" | "-h" => return Ok(ParsedArgs::Help),
                "--version" | "-V" => return Ok(ParsedArgs::Version),
                _ if flag.starts_with('-') => return Err(ParseError::UnknownFlag(flag.to_owned())),
                _ => {}
            }
        }
        if positional.is_some() {
            return Err(ParseError::TooManyArgs);
        }
        positional = Some(PathBuf::from(arg));
    }
    Ok(ParsedArgs::Open { path: positional })
}

/// Relative paths resolve against the calling process's directory, so the
/// app receives an absolute path whatever its own working directory is.
fn resolve_input_path(input: &Path, cwd: &Path) -> PathBuf {
    if input.is_absolute() {
        input.to_path_buf()
    } else {
        cwd.join(input)
    }
}

/// Starts the app; abstracted so tests observe the target without spawning.
pub trait Launcher {
    fn launch(&self, target: Option<&Path>) -> Result<(), LaunchError>;
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("{0}")]
    AppNotFound(String),
    #[error("could not launch Cerata: {0}")]
    Io(#[from] std::io::Error),
}

/// Spawns the Cerata app binary (this executable, or `CERATA_APP_PATH`)
/// with the target as its only argument and does not wait for it.
pub struct SystemLauncher;

impl Launcher for SystemLauncher {
    fn launch(&self, target: Option<&Path>) -> Result<(), LaunchError> {
        let program = match std::env::var_os(APP_PATH_ENV) {
            Some(path) => PathBuf::from(path),
            None => std::env::current_exe()?,
        };
        let mut command = std::process::Command::new(&program);
        if let Some(path) = target {
            command.arg(path);
        }
        match command.spawn() {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(LaunchError::AppNotFound(format!(
                    "could not find the Cerata app ({}); set {APP_PATH_ENV}",
                    program.display()
                )))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// Runs the CLI. Returns the process exit code: 0 on success, 2 for a usage
/// error, 3 for a runtime error (missing or unsupported path, launch
/// failure), as in Writer.
pub fn run<L: Launcher>(argv: &[OsString], cwd: &Path, launcher: &L) -> u8 {
    match parse_args(argv) {
        Ok(ParsedArgs::Help) => {
            println!("{USAGE}");
            EXIT_SUCCESS
        }
        Ok(ParsedArgs::Version) => {
            println!("cerata {VERSION}");
            EXIT_SUCCESS
        }
        Ok(ParsedArgs::Open { path }) => run_open(path, cwd, launcher),
        Err(error) => {
            eprintln!("cerata: {error}\n");
            eprint!("{USAGE}");
            EXIT_USAGE
        }
    }
}

fn run_open<L: Launcher>(path: Option<PathBuf>, cwd: &Path, launcher: &L) -> u8 {
    let target = match path {
        None => None,
        Some(input) => {
            let resolved = resolve_input_path(&input, cwd);
            match open_target::validate_and_resolve(&resolved) {
                Ok(OpenTarget::Workspace(path) | OpenTarget::File(path)) => Some(path),
                Err(error) => {
                    eprintln!("cerata: {error}");
                    return EXIT_RUNTIME;
                }
            }
        }
    };
    if let Err(error) = launcher.launch(target.as_deref()) {
        eprintln!("cerata: {error}");
        return EXIT_RUNTIME;
    }
    EXIT_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use tempfile::tempdir;

    struct FakeLauncher {
        calls: RefCell<Vec<Option<PathBuf>>>,
        fail: Option<String>,
    }

    impl FakeLauncher {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fail: None,
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fail: Some(message.to_owned()),
            }
        }
    }

    impl Launcher for FakeLauncher {
        fn launch(&self, target: Option<&Path>) -> Result<(), LaunchError> {
            self.calls.borrow_mut().push(target.map(Path::to_path_buf));
            match &self.fail {
                Some(message) => Err(LaunchError::AppNotFound(message.clone())),
                None => Ok(()),
            }
        }
    }

    fn argv(parts: &[&str]) -> Vec<OsString> {
        std::iter::once("cerata-cli")
            .chain(parts.iter().copied())
            .map(OsString::from)
            .collect()
    }

    // Ported from Writer `writer_cli.rs` tests.

    #[test]
    fn parse_help_flags() {
        assert_eq!(parse_args(&argv(&["--help"])), Ok(ParsedArgs::Help));
        assert_eq!(parse_args(&argv(&["-h"])), Ok(ParsedArgs::Help));
    }

    #[test]
    fn parse_version_flags() {
        assert_eq!(parse_args(&argv(&["--version"])), Ok(ParsedArgs::Version));
        assert_eq!(parse_args(&argv(&["-V"])), Ok(ParsedArgs::Version));
    }

    #[test]
    fn parse_no_args() {
        assert_eq!(parse_args(&argv(&[])), Ok(ParsedArgs::Open { path: None }));
    }

    #[test]
    fn parse_single_path() {
        assert_eq!(
            parse_args(&argv(&["notes"])),
            Ok(ParsedArgs::Open {
                path: Some(PathBuf::from("notes"))
            })
        );
    }

    #[test]
    fn parse_rejects_multiple_positional() {
        assert_eq!(parse_args(&argv(&["a", "b"])), Err(ParseError::TooManyArgs));
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        assert_eq!(
            parse_args(&argv(&["--nope"])),
            Err(ParseError::UnknownFlag("--nope".into()))
        );
    }

    #[test]
    fn run_no_args_launches_with_none() {
        let launcher = FakeLauncher::new();
        assert_eq!(run(&argv(&[]), Path::new("."), &launcher), EXIT_SUCCESS);
        assert_eq!(launcher.calls.borrow().as_slice(), &[None]);
    }

    #[test]
    fn run_directory_target_passes_canonical_workspace() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("notes");
        fs::create_dir(&target).unwrap();
        let launcher = FakeLauncher::new();
        assert_eq!(run(&argv(&["notes"]), dir.path(), &launcher), EXIT_SUCCESS);
        let calls = launcher.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].as_ref().unwrap(),
            &dunce::canonicalize(&target).unwrap()
        );
    }

    #[test]
    fn run_markdown_target_passes_file_path() {
        let dir = tempdir().unwrap();
        let md = dir.path().join("note.MD");
        fs::write(&md, "").unwrap();
        let launcher = FakeLauncher::new();
        assert_eq!(
            run(&argv(&["note.MD"]), dir.path(), &launcher),
            EXIT_SUCCESS
        );
        assert_eq!(
            launcher.calls.borrow()[0].as_ref().unwrap(),
            &dunce::canonicalize(&md).unwrap()
        );
    }

    #[test]
    fn run_unsupported_file_is_runtime_error_without_launch() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("image.png"), "").unwrap();
        let launcher = FakeLauncher::new();
        assert_eq!(
            run(&argv(&["image.png"]), dir.path(), &launcher),
            EXIT_RUNTIME
        );
        assert!(launcher.calls.borrow().is_empty());
    }

    #[test]
    fn run_missing_path_is_runtime_error() {
        let dir = tempdir().unwrap();
        let launcher = FakeLauncher::new();
        assert_eq!(
            run(&argv(&["missing"]), dir.path(), &launcher),
            EXIT_RUNTIME
        );
        assert!(launcher.calls.borrow().is_empty());
    }

    #[test]
    fn run_bad_flag_is_usage_error() {
        let launcher = FakeLauncher::new();
        assert_eq!(
            run(&argv(&["--bad"]), Path::new("."), &launcher),
            EXIT_USAGE
        );
        assert!(launcher.calls.borrow().is_empty());
    }

    #[test]
    fn run_propagates_launcher_failure_as_runtime_error() {
        let launcher = FakeLauncher::failing("not installed");
        assert_eq!(run(&argv(&[]), Path::new("."), &launcher), EXIT_RUNTIME);
    }

    #[test]
    fn resolve_input_path_joins_relative_against_cwd() {
        let cwd = Path::new("/home/me/work");
        assert_eq!(resolve_input_path(Path::new("foo"), cwd), cwd.join("foo"));
        let absolute = std::env::temp_dir().join("abs");
        assert_eq!(resolve_input_path(&absolute, cwd), absolute);
    }

    // Cerata addition: CLI detection is exact and case-sensitive.

    #[test]
    fn cli_mode_needs_the_exact_binary_name_or_subcommand() {
        let os = |parts: &[&str]| parts.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            cli_args(&os(&["/usr/bin/cerata-cli", "notes"])),
            Some(os(&["/usr/bin/cerata-cli", "notes"]))
        );
        assert_eq!(
            cli_args(&os(&["C:\\Apps\\cerata.exe", "cli", "notes"])),
            Some(os(&["C:\\Apps\\cerata.exe", "notes"]))
        );
        // The app binary itself, whatever its case, opens the app.
        assert_eq!(
            cli_args(&os(&["/Applications/Cerata.app/Contents/MacOS/Cerata"])),
            None
        );
        assert_eq!(cli_args(&os(&["cerata", "notes"])), None);
        assert_eq!(cli_args(&os(&["Cerata-CLI", "notes"])), None);
        assert_eq!(cli_args(&os(&["cerata", "CLI", "notes"])), None);
    }
}
