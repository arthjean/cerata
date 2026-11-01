//! Process-wide state: the core services, the open windows, and the entry
//! points every open request goes through (first launch, second launch,
//! drops, the welcome screen and the palette).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cerata_core::open_target::{self, OpenTarget, OpenTargetError};
use cerata_core::startup::{CoreApp, startup};
use cerata_core::watcher::WatchEnvelope;
use gpui::{App, Global, WindowHandle, WindowId};

use crate::inbox::Inbox;
use crate::routing::{self, Hosts, OpenRequest, Origin, Route};
use crate::shell::{self, WindowView};
use crate::single_instance::LaunchRequest;

pub struct Cerata {
    pub core: Arc<CoreApp>,
    windows: HashMap<u64, WindowHandle<WindowView>>,
    by_window_id: HashMap<WindowId, u64>,
    events: std::sync::mpsc::Sender<WatchEnvelope>,
    inbox: Arc<Inbox<WatchEnvelope>>,
}

impl Global for Cerata {}

/// Installs the services and starts routing workspace events to windows.
pub fn init(cx: &mut App, core: CoreApp) {
    let (events, receiver) = std::sync::mpsc::channel::<WatchEnvelope>();
    let (inbox, mut wakeups) = Inbox::new();
    let forward = inbox.clone();
    let spawned = std::thread::Builder::new()
        .name("cerata-events".into())
        .spawn(move || {
            for envelope in receiver {
                forward.push(envelope);
            }
        });
    if let Err(error) = spawned {
        log::error!("workspace events will not reach the windows: {error}");
    }
    cx.set_global(Cerata {
        core: Arc::new(core),
        windows: HashMap::new(),
        by_window_id: HashMap::new(),
        events,
        inbox,
    });
    cx.spawn(async move |cx| {
        while wakeups.next().await.is_some() {
            cx.update(pump_events);
        }
    })
    .detach();
    // Quitting skips `on_window_should_close`: every window writes its
    // pending edits and session here, before the windows are dropped.
    cx.on_app_quit(|cx| {
        for handle in windows(cx) {
            let _ = handle.update(cx, |view, _, _| view.before_close());
        }
        async {}
    })
    .detach();
    cx.on_window_closed(|cx, window_id| {
        forget_window(cx, window_id);
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

pub fn core(cx: &App) -> Arc<CoreApp> {
    cx.global::<Cerata>().core.clone()
}

pub fn events(cx: &App) -> std::sync::mpsc::Sender<WatchEnvelope> {
    cx.global::<Cerata>().events.clone()
}

pub fn windows(cx: &App) -> Vec<WindowHandle<WindowView>> {
    let mut windows: Vec<_> = cx.global::<Cerata>().windows.iter().collect();
    windows.sort_by_key(|(id, _)| **id);
    windows.into_iter().map(|(_, handle)| *handle).collect()
}

fn forget_window(cx: &mut App, window_id: WindowId) {
    let state = cx.global_mut::<Cerata>();
    if let Some(id) = state.by_window_id.remove(&window_id) {
        state.windows.remove(&id);
        state.core.windows.remove(id);
    }
}

/// Hands the queued workspace events to the windows they belong to.
pub fn pump_events(cx: &mut App) {
    let envelopes = cx.global::<Cerata>().inbox.take();
    for envelope in envelopes {
        dispatch_event(cx, envelope);
    }
}

fn dispatch_event(cx: &mut App, envelope: WatchEnvelope) {
    for handle in windows(cx) {
        let _ = handle.update(cx, |view, window, cx| {
            if view.owns(&envelope) {
                view.on_watch_event(envelope.event.clone(), window, cx);
            }
        });
    }
}

struct Registry<'a>(&'a CoreApp);

impl Hosts for Registry<'_> {
    fn workspace_window(&self, root: &Path) -> Option<u64> {
        self.0.windows.find_by_workspace(root)
    }
    fn file_window(&self, file: &Path) -> Option<u64> {
        self.0.windows.find_by_standalone_file(file)
    }
}

/// Opens a new window on `target` (the core startup call: settings,
/// session, first file), then `file` in it. The window is created once
/// everything its first frame shows is read, so it never flashes empty.
pub fn open_window(
    cx: &mut App,
    target: Option<OpenTarget>,
    file: Option<PathBuf>,
) -> anyhow::Result<WindowHandle<WindowView>> {
    let core = core(cx);
    let restore_session = file.is_none();
    let (state, bundle) = startup(&core, target, events(cx))?;
    let id = state.id();
    let handle = match shell::open(cx, state, bundle, restore_session) {
        Ok(handle) => handle,
        Err(error) => {
            core.windows.remove(id);
            return Err(error);
        }
    };
    let window_id = handle.window_id();
    let registry = cx.global_mut::<Cerata>();
    registry.windows.insert(id, handle);
    registry.by_window_id.insert(window_id, id);
    if let Some(file) = file {
        let _ = handle.update(cx, |view, window, cx| view.open_file(&file, window, cx));
    }
    Ok(handle)
}

/// A launch, first or forwarded: every argument is opened in order. A
/// missing one shows "X not found" in the frontmost window and opens
/// nothing; no argument brings the first window forward.
pub fn handle_launch(cx: &mut App, request: LaunchRequest) {
    if request.args.is_empty() {
        match windows(cx).first() {
            Some(handle) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            None => {
                if let Err(error) = open_window(cx, None, None) {
                    log::error!("cannot open a window: {error:#}");
                }
            }
        }
        return;
    }
    for (arg, path) in request.args.iter().zip(request.paths()) {
        match open_target::validate_and_resolve(&path) {
            Ok(target) => open(cx, OpenRequest::from_launch(target), None),
            Err(error) => {
                log::warn!("launch target refused: {error}");
                notify_frontmost(cx, refusal(arg, &path, &error));
            }
        }
    }
}

/// The notice for a target that cannot open.
pub fn refusal(arg: &str, path: &Path, error: &OpenTargetError) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| arg.to_owned());
    match error {
        OpenTargetError::NotFound(_) => format!("{name} not found"),
        OpenTargetError::Unsupported(_) => {
            format!("{name} is not a folder or a Markdown file")
        }
        OpenTargetError::Io { .. } => format!("Cannot open {name}"),
    }
}

fn notify_frontmost(cx: &mut App, notice: String) {
    let active = cx.active_window().and_then(|w| w.downcast::<WindowView>());
    let target = active.or_else(|| windows(cx).first().copied());
    match target {
        Some(handle) => {
            let _ = handle.update(cx, |view, _, cx| view.show_notice(notice, cx));
        }
        None => log::warn!("{notice}"),
    }
}

/// Routes `request` from `origin` (the window it came from, if any) and
/// carries the route out.
pub fn open(cx: &mut App, request: OpenRequest, origin: Option<WindowHandle<WindowView>>) {
    let core = core(cx);
    let origin_state = origin.and_then(|handle| {
        handle
            .read_with(cx, |view, _| {
                (
                    view.core_id(),
                    view.workspace_root().map(Path::to_path_buf),
                    view.chrome,
                )
            })
            .ok()
    });
    let route = routing::route(
        request,
        origin_state.as_ref().map(|(id, root, chrome)| Origin {
            id: *id,
            root: root.as_deref(),
            chrome: *chrome,
        }),
        &Registry(&core),
    );
    log::info!("open: {route:?}");
    let result = match route {
        Route::Reveal { id, file } => {
            let handle = cx.global::<Cerata>().windows.get(&id).copied();
            if let Some(handle) = handle {
                let _ = handle.update(cx, |view, window, cx| {
                    window.activate_window();
                    if let Some(file) = file {
                        view.open_file(&file, window, cx);
                    }
                });
            }
            Ok(())
        }
        Route::WorkspaceHere { root, file } => with_origin(cx, origin, |view, window, cx| {
            view.open_workspace(&root, file.as_deref(), window, cx)
        }),
        Route::FileHere(file) => with_origin(cx, origin, |view, window, cx| {
            view.open_file(&file, window, cx);
            Ok(())
        }),
        Route::CompactHere(file) => with_origin(cx, origin, |view, window, cx| {
            view.make_compact(&file, window, cx)
        }),
        Route::NewWorkspaceWindow { root, file } => {
            open_window(cx, Some(OpenTarget::Workspace(root)), file).map(drop)
        }
        Route::NewCompactWindow(file) => {
            open_window(cx, Some(OpenTarget::File(file)), None).map(drop)
        }
    };
    if let Err(error) = result {
        log::error!("open failed: {error:#}");
        notify_frontmost(cx, format!("Cannot open: {error}"));
    }
}

fn with_origin(
    cx: &mut App,
    origin: Option<WindowHandle<WindowView>>,
    f: impl FnOnce(
        &mut WindowView,
        &mut gpui::Window,
        &mut gpui::Context<WindowView>,
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let handle = origin.ok_or_else(|| anyhow::anyhow!("no window to open in"))?;
    handle.update(cx, f)?
}
