//! Cerata desktop entry point.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// A keystroke with the OS's command modifier: Cmd on macOS, Ctrl
/// elsewhere, as a static string for `KeyBinding::new`. `editor` keeps its
/// own copy for the examples that compile it on its own.
macro_rules! secondary {
    ($keys:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $keys)
        } else {
            concat!("ctrl-", $keys)
        }
    };
}

mod a11y;
mod app;
mod app_menu;
mod documents;
mod editor;
mod icons;
mod inbox;
mod menu;
mod palette;
mod routing;
mod settings_panel;
mod shell;
mod sidebar;
mod single_instance;
mod tabs;
mod theme;
mod zoom;

use cerata_core::cli;
use cerata_core::open_target;
use cerata_core::startup::CoreApp;
use futures::StreamExt as _;
use gpui::App;
use gpui_platform::application;
use single_instance::{Claim, Endpoint, LaunchRequest};
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
    let core = match CoreApp::open_default() {
        Ok(core) => core,
        Err(error) => {
            log::error!("cannot start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let request = LaunchRequest::from_env(argv.into_iter().skip(1), cwd);
    let listener = match single_instance::claim(&Endpoint::for_data_dir(&core.data_dir), &request) {
        Ok(Claim::Forwarded) => return ExitCode::SUCCESS,
        Ok(Claim::Primary(listener)) => Some(listener),
        Err(error) => {
            log::warn!("single instance unavailable, running on its own: {error}");
            None
        }
    };

    application().run(move |cx: &mut App| {
        editor::actions::init(cx);
        cx.bind_keys(shell::bindings());
        let positional = shell::positional_bindings(cx);
        cx.bind_keys(positional);
        cx.bind_keys(a11y::bindings());
        cx.bind_keys(sidebar::bindings());
        cx.bind_keys(palette::bindings());
        app_menu::init(cx);
        shell::title_bar::init(cx);
        app::init(cx, core);
        if let Some(listener) = listener {
            serve_launches(cx, listener);
        }
        start(cx, request);
    });
    ExitCode::SUCCESS
}

/// Later launches reach the main thread in arrival order.
fn serve_launches(cx: &mut App, listener: interprocess::local_socket::Listener) {
    let (sender, mut launches) = futures::channel::mpsc::unbounded();
    if let Err(error) = single_instance::serve(listener, move |request| {
        let _ = sender.unbounded_send(request);
    }) {
        log::warn!("second launches will not be received: {error}");
        return;
    }
    cx.spawn(async move |cx| {
        while let Some(request) = launches.next().await {
            cx.update(|cx| app::handle_launch(cx, request));
        }
    })
    .detach();
}

/// The first window opens on the first target that resolves (or restores
/// the last workspace); the other targets follow in order.
fn start(cx: &mut App, request: LaunchRequest) {
    let paths = request.paths();
    let first = paths
        .iter()
        .position(|path| open_target::resolve(path).is_some());
    let target = first.and_then(|ix| open_target::resolve(&paths[ix]));
    if let Err(error) = app::open_window(cx, target, None) {
        log::error!("failed to open the main window: {error:#}");
        cx.quit();
        return;
    }
    let rest = LaunchRequest {
        cwd: request.cwd.clone(),
        args: request
            .args
            .iter()
            .enumerate()
            .filter(|(ix, _)| Some(*ix) != first)
            .map(|(_, arg)| arg.clone())
            .collect(),
    };
    if !rest.args.is_empty() {
        app::handle_launch(cx, rest);
    }
}
