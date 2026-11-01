//! Cerata desktop entry point.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod editor;
mod window;

use cerata_core::cli;
use cerata_core::open_target;
use cerata_core::startup::{CoreApp, startup};
use cerata_core::watcher::WatchEnvelope;
use gpui::App;
use gpui_platform::application;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(cli_argv) = cli::cli_args(&argv) {
        return ExitCode::from(cli::run(&cli_argv, &cwd, &cli::SystemLauncher));
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // The app takes at most one target: a folder or a Markdown file, as
    // handed over by the CLI, the OS or a shell.
    let target = argv
        .get(1)
        .and_then(|arg| open_target::resolve(&cwd.join(arg)));

    application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        editor::actions::init(cx);
        start(cx, target.clone());
    });
    ExitCode::SUCCESS
}

fn start(cx: &mut App, target: Option<open_target::OpenTarget>) {
    let core = match CoreApp::open_default() {
        Ok(core) => core,
        Err(error) => {
            log::error!("cannot start: {error}");
            cx.quit();
            return;
        }
    };
    let (events, receiver) = std::sync::mpsc::channel::<WatchEnvelope>();
    let mut document = window::Document::default();
    let mut config = editor::EditorConfig::default();
    match startup(&core, target, events) {
        Ok((window_state, bundle)) => {
            config = editor::EditorConfig::from_settings(&bundle.settings);
            if let Some(file) = bundle.standalone_file.clone() {
                document = window::Document {
                    text: file.content,
                    path: Some(file.path),
                    root: None,
                };
            } else if let Some(file) = bundle.active_file.clone() {
                document = window::Document {
                    text: file.content,
                    path: Some(file.path),
                    root: bundle.workspace.as_ref().map(|info| info.root.clone()),
                };
            }
            log::info!(
                "startup: workspace {:?}, {} entries, {} recent workspaces, {} recent files",
                bundle.workspace.as_ref().map(|info| &info.root),
                bundle.entries.len(),
                bundle.recent_workspaces.len(),
                bundle.recent_files.len(),
            );
            // The shell (EP-004) consumes these; until then they are logged
            // so the index and watcher pipeline stays observable.
            let spawned = std::thread::Builder::new()
                .name("cerata-events".into())
                .spawn(move || {
                    for envelope in receiver {
                        let current = window_state
                            .snapshot()
                            .is_some_and(|s| envelope.is_for(&s.root, s.epoch));
                        if current {
                            log::info!("workspace event: {:?}", envelope.event);
                        }
                    }
                });
            if let Err(error) = spawned {
                log::warn!("workspace events not logged: {error}");
            }
        }
        Err(error) => log::error!("startup failed: {error}"),
    }
    if let Err(error) = window::open_main_window(cx, document, config) {
        log::error!("failed to open the main window: {error:#}");
        cx.quit();
    }
}
