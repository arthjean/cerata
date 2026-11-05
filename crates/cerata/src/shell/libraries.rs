use std::path::PathBuf;

use cerata_core::library::{CreateError, Library, LocateOutcome, name_problem};
use cerata_core::open_target::{self, OpenTarget};
use gpui::{
    AppContext as _, Context, Entity, Focusable as _, PathPromptOptions, SharedString,
    Subscription, Window, WindowHandle,
};

use super::WindowView;
use crate::editor::text_field::{FieldEvent, TextField};
use crate::palette::{Intent, LibraryAction};
use crate::routing::{Chrome, OpenRequest};

pub const MAX_RECENT_KEY: &str = "workspace.max-recent-workspaces";
const MAX_RECENT_DEFAULT: usize = 10;

pub const DEFAULT_NOT_FOUND: &str =
    "Your default library wasn't found. Locate it or create a new one.";

pub struct Welcome {
    pub name: Entity<TextField>,
    pub location: Option<PathBuf>,
    pub synced_by: Option<String>,
    pub problem: Option<WelcomeProblem>,
    pub libraries: Vec<Library>,
    _events: Subscription,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WelcomeProblem {
    Exists { path: PathBuf, name: String },
    Failed(String),
}

impl WelcomeProblem {
    pub fn message(&self) -> String {
        match self {
            Self::Exists { name, .. } => {
                format!("A folder named {name} already exists here. Open it as a library?")
            }
            Self::Failed(message) => format!(
                "{}. Choose another location or name.",
                message.trim_end_matches('.')
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    Locate(String),
    Remove(String),
    OpenAsNew(PathBuf),
}

impl NoticeAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Locate(_) => LibraryAction::Locate.label(),
            Self::Remove(_) => LibraryAction::Remove.label(),
            Self::OpenAsNew(_) => "Open as New Library",
        }
    }
}

impl Welcome {
    pub fn new(window: &mut Window, cx: &mut Context<WindowView>) -> Self {
        let name = cx.new(|cx| {
            let mut field = TextField::new("Library name", cx).labeled("Name");
            field.set_text(cerata_core::library::DEFAULT_LIBRARY_NAME, cx);
            field
        });
        let events = cx.subscribe_in(&name, window, |this, _, event, window, cx| match event {
            FieldEvent::Changed => {
                this.welcome.problem = None;
                cx.notify();
            }
            FieldEvent::Submit { .. } => this.create_library(window, cx),
            FieldEvent::Escape => {}
        });
        let location = cerata_core::paths::default_library_parent();
        Self {
            synced_by: location.as_deref().and_then(cerata_core::cloud::synced_by),
            name,
            location,
            problem: None,
            libraries: Vec::new(),
            _events: events,
        }
    }
}

fn os_error(error: &CreateError) -> String {
    match error {
        CreateError::Fs(cerata_core::fs::FsError::Io { source, .. }) => source.to_string(),
        other => other.to_string(),
    }
}

impl WindowView {
    pub fn refresh_libraries(&mut self, cx: &mut Context<Self>) {
        match crate::app::core(cx).libraries.list() {
            Ok(libraries) => self.welcome.libraries = libraries,
            Err(error) => log::warn!("libraries unavailable: {error}"),
        }
        cx.notify();
    }

    pub fn listed_libraries(&self) -> &[Library] {
        let cap = self
            .core
            .settings()
            .get(MAX_RECENT_KEY)
            .and_then(|value| value.as_f64())
            .map_or(MAX_RECENT_DEFAULT, |n| n.max(0.0) as usize);
        let libraries = &self.welcome.libraries;
        &libraries[..libraries.len().min(cap)]
    }

    pub fn welcome_name(&self, cx: &gpui::App) -> String {
        self.welcome.name.read(cx).text.trim().to_owned()
    }

    pub fn welcome_name_problem(&self, cx: &gpui::App) -> Option<String> {
        name_problem(&self.welcome_name(cx))
    }

    pub fn can_create_library(&self, cx: &gpui::App) -> bool {
        self.welcome.location.is_some() && self.welcome_name_problem(cx).is_none()
    }

    pub fn create_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_create_library(cx) {
            cx.notify();
            return;
        }
        let Some(location) = self.welcome.location.clone() else {
            return;
        };
        let name = self.welcome_name(cx);
        match crate::app::core(cx).libraries.create(&location, &name) {
            Ok(library) => {
                self.welcome.problem = None;
                self.open_library_here(&library, window, cx);
            }
            Err(CreateError::FolderExists(path)) => {
                self.welcome.problem = Some(WelcomeProblem::Exists { path, name });
                cx.notify();
            }
            Err(error) => {
                log::warn!("library not created: {error}");
                self.welcome.problem = Some(WelcomeProblem::Failed(format!(
                    "Couldn't create the library: {}",
                    os_error(&error)
                )));
                cx.notify();
            }
        }
    }

    pub fn adopt_existing_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(WelcomeProblem::Exists { path, .. }) = self.welcome.problem.clone() else {
            return;
        };
        match crate::app::core(cx).libraries.adopt(&path) {
            Ok(library) => {
                self.welcome.problem = None;
                self.open_library_here(&library, window, cx);
            }
            Err(error) => {
                self.welcome.problem = Some(WelcomeProblem::Failed(format!(
                    "Couldn't create the library: {}",
                    os_error(&error)
                )));
                cx.notify();
            }
        }
    }

    fn open_library_here(
        &mut self,
        library: &Library,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = OpenRequest::Workspace {
            root: library.path.clone(),
            file: None,
        };
        self.open_elsewhere(request, window, cx);
    }

    pub fn pick_library_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        let handle = window.window_handle().downcast::<WindowView>();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let (Some(folder), Some(handle)) = (paths.into_iter().next(), handle) else {
                return;
            };
            let _ = handle.update(cx, |view, _, cx| view.set_library_location(folder, cx));
        })
        .detach();
    }

    pub fn set_library_location(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.welcome.synced_by = cerata_core::cloud::synced_by(&folder);
        self.welcome.location = Some(folder);
        self.welcome.problem = None;
        cx.notify();
    }

    pub fn new_note_without_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default = match crate::app::core(cx).libraries.default_library() {
            Ok(default) => default,
            Err(error) => {
                self.show_notice(format!("Libraries unavailable: {error}"), cx);
                return;
            }
        };
        match default {
            Some(library) if library.exists() => {
                if let Some(handle) = crate::app::workspace_window(cx, &library.path) {
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |view, window, cx| {
                            window.activate_window();
                            view.open_palette(Intent::CreateFile, window, cx);
                        });
                    });
                    return;
                }
                match self.open_workspace(&library.path, None, window, cx) {
                    Ok(()) => self.open_palette(Intent::CreateFile, window, cx),
                    Err(error) => self.show_notice(format!("Cannot open: {error}"), cx),
                }
            }
            Some(library) => self.show_notice_with(
                DEFAULT_NOT_FOUND,
                vec![
                    NoticeAction::Locate(library.id.clone()),
                    NoticeAction::Remove(library.id),
                ],
                cx,
            ),
            None => {
                let focus = self.welcome.name.focus_handle(cx);
                window.focus(&focus, cx);
                cx.notify();
            }
        }
    }

    pub fn open_library(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match crate::app::core(cx).libraries.get(id) {
            Ok(Some(library)) if library.exists() => self.open_library_here(&library, window, cx),
            Ok(Some(library)) => self.show_notice_with(
                format!("{} wasn't found.", library.name),
                vec![
                    NoticeAction::Locate(library.id.clone()),
                    NoticeAction::Remove(library.id),
                ],
                cx,
            ),
            Ok(None) => self.refresh_libraries(cx),
            Err(error) => self.show_notice(format!("Libraries unavailable: {error}"), cx),
        }
    }

    pub fn run_library_action(
        &mut self,
        id: &str,
        action: LibraryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            LibraryAction::Open => self.open_library(id, window, cx),
            LibraryAction::SetDefault => self.set_default_library(id, cx),
            LibraryAction::Remove => self.remove_library(id, cx),
            LibraryAction::Locate => self.locate_library(id, window, cx),
        }
    }

    pub fn set_default_library(&mut self, id: &str, cx: &mut Context<Self>) {
        let core = crate::app::core(cx);
        let name = core.libraries.get(id).ok().flatten().map(|l| l.name);
        match core.libraries.set_default(id) {
            Ok(true) => {
                if let Some(name) = name {
                    self.show_notice(format!("{name} is now your default library"), cx);
                }
            }
            Ok(false) => {}
            Err(error) => self.show_notice(format!("Default not changed: {error}"), cx),
        }
        self.refresh_libraries(cx);
    }

    pub fn remove_library(&mut self, id: &str, cx: &mut Context<Self>) {
        let core = crate::app::core(cx);
        let name = core.libraries.get(id).ok().flatten().map(|l| l.name);
        match core.libraries.remove(id) {
            Ok(true) => {
                if let Some(name) = name {
                    self.show_notice(
                        format!("{name} was removed from the list. Its folder is unchanged."),
                        cx,
                    );
                }
            }
            Ok(false) => {}
            Err(error) => self.show_notice(format!("Library not removed: {error}"), cx),
        }
        self.refresh_libraries(cx);
    }

    pub fn locate_library(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Locate".into()),
        });
        let origin = window.window_handle().downcast::<WindowView>();
        let id = id.to_owned();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            cx.update(|cx| located(cx, &id, folder, origin));
        })
        .detach();
    }

    pub fn run_notice_action(
        &mut self,
        action: NoticeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notice = None;
        self.notice_actions = None;
        match action {
            NoticeAction::Locate(id) => self.locate_library(&id, window, cx),
            NoticeAction::Remove(id) => self.remove_library(&id, cx),
            NoticeAction::OpenAsNew(folder) => {
                let request = OpenRequest::Workspace {
                    root: folder,
                    file: None,
                };
                self.open_elsewhere(request, window, cx);
            }
        }
        cx.notify();
    }

    pub fn show_notice_with(
        &mut self,
        notice: impl Into<SharedString>,
        actions: Vec<NoticeAction>,
        cx: &mut Context<Self>,
    ) {
        let notice = notice.into();
        self.notice_actions = Some((notice.clone(), actions));
        self.show_notice(notice, cx);
    }

    pub fn current_notice_actions(&self) -> &[NoticeAction] {
        match (&self.notice, &self.notice_actions) {
            (Some(notice), Some((text, actions))) if notice == text => actions,
            _ => &[],
        }
    }

    pub fn needs_a_library(&self) -> bool {
        !self.has_root() && self.chrome == Chrome::Workspace
    }
}

pub fn located(
    cx: &mut gpui::App,
    id: &str,
    folder: PathBuf,
    origin: Option<WindowHandle<WindowView>>,
) {
    let outcome = crate::app::core(cx).libraries.locate(id, &folder);
    let notify = |cx: &mut gpui::App, text: String, actions: Vec<NoticeAction>| match origin {
        Some(handle) => {
            let _ = handle.update(cx, |view, _, cx| {
                view.refresh_libraries(cx);
                view.show_notice_with(text, actions, cx);
            });
        }
        None => log::warn!("{text}"),
    };
    match outcome {
        Ok(LocateOutcome::Located(library)) => {
            if let Some(handle) = origin {
                let _ = handle.update(cx, |view, _, cx| view.refresh_libraries(cx));
            }
            let request = OpenRequest::Workspace {
                root: library.path,
                file: None,
            };
            crate::app::open(cx, request, origin);
        }
        Ok(LocateOutcome::NotThisLibrary { name }) => notify(
            cx,
            format!("This folder isn't {name}. Open it as a new library instead?"),
            vec![NoticeAction::OpenAsNew(folder)],
        ),
        Ok(LocateOutcome::Unknown) => {
            notify(cx, "This library is no longer listed".into(), Vec::new())
        }
        Err(error) => notify(cx, format!("Libraries unavailable: {error}"), Vec::new()),
    }
}

pub fn open_picked_file(
    cx: &mut gpui::App,
    file: PathBuf,
    origin: Option<WindowHandle<WindowView>>,
) {
    let file = match open_target::resolve(&file) {
        Some(OpenTarget::File(file)) => file,
        _ => file,
    };
    let library = crate::app::core(cx)
        .libraries
        .containing(&file)
        .unwrap_or_else(|error| {
            log::warn!("libraries unavailable: {error}");
            None
        });
    let request = match library {
        Some(library) => OpenRequest::Workspace {
            root: library.path,
            file: Some(file),
        },
        None => OpenRequest::File(file),
    };
    crate::app::open(cx, request, origin);
}
