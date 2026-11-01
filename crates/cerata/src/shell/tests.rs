//! Window-level behavior through the real entry points: launches (first
//! and forwarded), drops, tab actions, session save and restore, and disk
//! changes delivered as watcher events.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cerata_core::open_target::{self, OpenTarget};
use cerata_core::startup::CoreApp;
use cerata_core::watcher::{FileChange, WatchEvent};
use gpui::{ExternalPaths, TestAppContext, VisualTestContext, WindowHandle};
use tempfile::TempDir;

use super::*;
use crate::app;
use crate::documents::Conflict;
use crate::single_instance::LaunchRequest;
use crate::tabs::Location;

struct Env {
    _config: TempDir,
    _data: TempDir,
    dir: TempDir,
}

impl Env {
    fn root(&self) -> PathBuf {
        match open_target::resolve(self.dir.path()) {
            Some(OpenTarget::Workspace(root)) => root,
            _ => panic!("temp dir resolves"),
        }
    }

    fn folder(&self, name: &str) -> PathBuf {
        let path = self.root().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }
}

fn setup(cx: &mut TestAppContext) -> Env {
    let config = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let core = CoreApp::new(config.path().to_path_buf(), data.path().to_path_buf());
    cx.update(|cx| {
        crate::editor::actions::init(cx);
        cx.bind_keys(bindings());
        cx.bind_keys(crate::a11y::bindings());
        cx.bind_keys(crate::palette::bindings());
        app::init(cx, core);
    });
    Env {
        _config: config,
        _data: data,
        dir: TempDir::new().unwrap(),
    }
}

fn launch(cx: &mut TestAppContext, cwd: &Path, args: &[&str]) {
    let request = LaunchRequest {
        cwd: cwd.to_path_buf(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    };
    cx.update(|cx| app::handle_launch(cx, request));
    cx.run_until_parked();
}

fn windows(cx: &mut TestAppContext) -> Vec<WindowHandle<WindowView>> {
    cx.update(|cx| app::windows(cx))
}

fn read<R>(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    f: impl FnOnce(&WindowView, &App) -> R,
) -> R {
    cx.update(|cx| handle.read_with(cx, |view, cx| f(view, cx)).unwrap())
}

fn update<R>(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    f: impl FnOnce(&mut WindowView, &mut Window, &mut Context<WindowView>) -> R,
) -> R {
    let result = handle.update(cx, f).unwrap();
    cx.run_until_parked();
    result
}

/// Waits for work on real threads (autosave), then applies its results to
/// every window, as the production wake-up does.
fn wait_until(cx: &mut TestAppContext, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for handle in windows(cx) {
            let _ = handle.update(cx, |view, _, cx| view.drain_saves(cx));
        }
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn editor_text(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) -> String {
    read(cx, handle, |view, cx| {
        view.active_doc()
            .unwrap()
            .editor
            .read(cx)
            .state
            .doc()
            .to_owned()
    })
}

fn type_text(cx: &mut TestAppContext, handle: WindowHandle<WindowView>, text: &str) {
    update(cx, handle, |view, _, cx| {
        let editor = view.active_doc().unwrap().editor.clone();
        editor.update(cx, |editor, cx| {
            let end = editor.state.text().len();
            editor.update_state(cx, |state| {
                let _ = state.apply(
                    cerata_markdown::Transaction::new()
                        .change(cerata_markdown::Change::insert(end, text))
                        .event(cerata_markdown::UserEvent::Type),
                );
            });
        });
    });
}

// ---- US-023: windows, single instance, open flow ----

#[gpui::test]
fn one_window_per_workspace_and_reopening_reveals_it(cx: &mut TestAppContext) {
    let env = setup(cx);
    let notes = env.folder("notes");
    launch(cx, &env.root(), &["notes"]);
    assert_eq!(windows(cx).len(), 1);
    launch(cx, &notes, &["."]);
    assert_eq!(
        windows(cx).len(),
        1,
        "the same workspace is not opened twice"
    );
    let other = env.folder("other");
    launch(cx, &env.root(), &[other.to_str().unwrap()]);
    assert_eq!(windows(cx).len(), 2, "another folder opens a new window");
}

#[gpui::test]
fn forwarded_targets_resolve_against_the_launch_cwd_in_order(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.folder("a");
    env.folder("b");
    launch(cx, &env.root(), &["a", "b"]);
    let roots: Vec<PathBuf> = windows(cx)
        .into_iter()
        .map(|handle| {
            read(cx, handle, |view, _| {
                view.workspace_root().unwrap().to_path_buf()
            })
        })
        .collect();
    assert_eq!(roots, [env.root().join("a"), env.root().join("b")]);
}

#[gpui::test]
fn a_missing_target_shows_a_notice_and_opens_no_window(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.folder("a");
    launch(cx, &env.root(), &["a"]);
    launch(cx, &env.root(), &["gone.md"]);
    let handles = windows(cx);
    assert_eq!(handles.len(), 1);
    let notice = read(cx, handles[0], |view, _| view.notice.clone());
    assert_eq!(notice.as_deref(), Some("gone.md not found"));
}

#[gpui::test]
fn a_file_alone_opens_a_compact_window_once(cx: &mut TestAppContext) {
    let env = setup(cx);
    let file = env.file("solo.md", "# Solo");
    env.folder("vault");
    launch(cx, &env.root(), &["vault"]);
    launch(cx, &env.root(), &["solo.md"]);
    let handles = windows(cx);
    assert_eq!(handles.len(), 2);
    let (chrome, path) = read(cx, handles[1], |view, _| {
        (view.chrome, view.tabs.active_path().map(Path::to_path_buf))
    });
    assert_eq!(chrome, Chrome::Compact);
    assert_eq!(path, Some(file));
    launch(cx, &env.root(), &["solo.md"]);
    assert_eq!(windows(cx).len(), 2, "the file's window is revealed");
}

#[gpui::test]
fn drops_follow_the_same_routing(cx: &mut TestAppContext) {
    let env = setup(cx);
    let vault = env.folder("vault");
    let inside = env.file("vault/a.md", "# A");
    let other = env.folder("other");
    launch(cx, &env.root(), &["vault"]);
    let main = windows(cx)[0];

    let drop = |cx: &mut TestAppContext, paths: Vec<PathBuf>| {
        update(cx, main, |view, window, cx| {
            view.on_drop(&ExternalPaths(paths.into_iter().collect()), window, cx)
        });
    };
    drop(cx, vec![inside.clone()]);
    assert_eq!(
        read(cx, main, |view, _| view
            .tabs
            .active_path()
            .map(Path::to_path_buf)),
        Some(inside),
        "a file of the same workspace opens in place"
    );
    drop(cx, vec![vault.clone(), other.clone()]);
    assert_eq!(windows(cx).len(), 2, "the other folder opens a new window");
    drop(cx, vec![other]);
    assert_eq!(
        windows(cx).len(),
        2,
        "its window is revealed, not duplicated"
    );
}

#[gpui::test]
fn an_empty_window_receives_a_dropped_folder(cx: &mut TestAppContext) {
    let env = setup(cx);
    let vault = env.folder("vault");
    let handle = cx.update(|cx| app::open_window(cx, None, None)).unwrap();
    cx.run_until_parked();
    assert!(matches!(
        read(cx, handle, |v, cx| v.page(cx)),
        Page::Welcome
    ));
    update(cx, handle, |view, window, cx| {
        view.on_drop(
            &ExternalPaths([vault.clone()].into_iter().collect()),
            window,
            cx,
        )
    });
    assert_eq!(windows(cx).len(), 1);
    assert_eq!(
        read(cx, handle, |view, _| view
            .workspace_root()
            .map(Path::to_path_buf)),
        Some(vault)
    );
}

#[gpui::test]
fn the_first_frame_already_shows_the_session_file(cx: &mut TestAppContext) {
    let env = setup(cx);
    let root = env.folder("vault");
    let file = env.file("vault/a.md", "# Restored");
    let mut tabs = Tabs::new();
    tabs.open(&file);
    cx.update(|cx| app::core(cx).sessions.save(&root, tabs.to_session()))
        .unwrap();
    let handle = cx
        .update(|cx| app::open_window(cx, Some(OpenTarget::Workspace(root)), None))
        .unwrap();
    // No executor turn: the bundle's prefetched file is already shown.
    let text = read(cx, handle, |view, cx| match view.page(cx) {
        Page::Editor(editor) => editor.read(cx).state.doc().to_owned(),
        _ => String::new(),
    });
    assert_eq!(text, "# Restored");
}

#[gpui::test]
fn main_window_opens_at_writer_geometry(cx: &mut TestAppContext) {
    let _env = setup(cx);
    let options = cx.update(|cx| window_options(cx));
    assert_eq!(options.window_min_size, Some(size(px(800.0), px(500.0))));
    let handle = cx.update(|cx| app::open_window(cx, None, None)).unwrap();
    let bounds = handle.update(cx, |_, window, _| window.bounds()).unwrap();
    assert_eq!(bounds.size, size(px(1200.0), px(800.0)));
}

// ---- US-024: tabs, history, session ----

fn workspace(cx: &mut TestAppContext, env: &Env) -> WindowHandle<WindowView> {
    let root = env.root();
    let handle = cx
        .update(|cx| app::open_window(cx, Some(OpenTarget::Workspace(root)), None))
        .unwrap();
    cx.run_until_parked();
    handle
}

#[gpui::test]
fn back_and_forward_keys_work_outside_the_text(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    update(cx, handle, |view, window, cx| {
        view.open_file(&b, window, cx)
    });
    assert_eq!(editor_text(cx, handle), "b");

    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    // The editor has the focus: Alt+Left stays in the text.
    vcx.simulate_keystrokes("alt-left");
    vcx.run_until_parked();
    assert_eq!(editor_text(cx, handle), "b");
    // Outside the text the tab goes back, then forward.
    update(cx, handle, |view, window, cx| {
        window.focus(&view.focus_handle, cx)
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_keystrokes("alt-left");
    vcx.run_until_parked();
    assert_eq!(editor_text(cx, handle), "a");
    update(cx, handle, |view, window, cx| {
        window.focus(&view.focus_handle, cx)
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_keystrokes("alt-right");
    vcx.run_until_parked();
    assert_eq!(editor_text(cx, handle), "b");
}

#[gpui::test]
fn opening_replaces_a_launcher_and_new_tab_adds_one(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    assert_eq!(read(cx, handle, |view, _| view.tabs.tabs().len()), 1);
    update(cx, handle, |view, window, cx| {
        view.open_file_in_new_tab(&a, window, cx)
    });
    let tabs = read(cx, handle, |view, _| view.tabs.tabs().len());
    assert_eq!(tabs, 2);
    let shared = read(cx, handle, |view, _| view.docs.iter().count());
    assert_eq!(shared, 1, "both tabs share one document");
}

#[gpui::test]
fn a_tab_on_a_missing_file_is_rolled_back(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    let gone = env.root().join("gone.md");
    update(cx, handle, |view, window, cx| {
        view.open_file_in_new_tab(&gone, window, cx)
    });
    let (tabs, notice) = read(cx, handle, |view, _| {
        (view.tabs.tabs().len(), view.notice.clone())
    });
    assert_eq!(tabs, 1);
    assert_eq!(notice.as_deref(), Some("Failed to open gone.md"));
}

#[gpui::test]
fn the_session_is_saved_500_ms_after_a_tab_change(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let handle = workspace(cx, &env);
    let saved = |cx: &mut TestAppContext| {
        cx.update(|cx| app::core(cx).sessions.load(&env.root()))
            .unwrap()
            .map(|s| s.tabs.len())
    };
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    cx.executor().advance_clock(Duration::from_millis(400));
    assert_eq!(saved(cx), None);
    cx.executor().advance_clock(Duration::from_millis(150));
    cx.run_until_parked();
    assert_eq!(saved(cx), Some(1));
    update(cx, handle, |view, window, cx| view.new_tab(window, cx));
    update(cx, handle, |view, _, _| view.save_session_now());
    assert_eq!(saved(cx), Some(1), "launchers are not saved");
}

#[gpui::test]
fn a_deleted_session_file_loses_its_tab(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let mut tabs = Tabs::new();
    tabs.open(&a);
    tabs.open_in_new_tab(&b);
    tabs.activate(0);
    cx.update(|cx| app::core(cx).sessions.save(&env.root(), tabs.to_session()))
        .unwrap();
    std::fs::remove_file(&b).unwrap();
    let handle = workspace(cx, &env);
    let locations: Vec<Location> = read(cx, handle, |view, _| {
        view.tabs
            .tabs()
            .iter()
            .map(|t| t.location.clone())
            .collect()
    });
    assert_eq!(locations, [Location::file(&a)]);
}

#[gpui::test]
fn the_other_restored_tabs_load_after_the_active_one(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let mut tabs = Tabs::new();
    tabs.open(&a);
    tabs.open_in_new_tab(&b);
    cx.update(|cx| app::core(cx).sessions.save(&env.root(), tabs.to_session()))
        .unwrap();
    let root = env.root();
    let handle = cx
        .update(|cx| app::open_window(cx, Some(OpenTarget::Workspace(root)), None))
        .unwrap();
    let loading = read(cx, handle, |view, _| {
        (
            view.docs.get(&b).is_some_and(|d| d.loading_since.is_none()),
            view.docs.get(&a).is_some_and(|d| d.loading_since.is_some()),
        )
    });
    assert_eq!(loading, (true, true), "b (active) first, a still loading");
    cx.run_until_parked();
    assert!(read(cx, handle, |view, _| view
        .docs
        .get(&a)
        .unwrap()
        .loading_since
        .is_none()));
}

#[gpui::test]
fn renaming_rewrites_tabs_and_keeps_the_document(cx: &mut TestAppContext) {
    let env = setup(cx);
    let old = env.file("dir/old.md", "x");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&old, window, cx)
    });
    let new = env.root().join("moved/old.md");
    update(cx, handle, |view, _, cx| {
        view.path_renamed(&env.root().join("dir"), &env.root().join("moved"), cx)
    });
    assert_eq!(
        read(cx, handle, |view, _| view
            .tabs
            .active_path()
            .map(Path::to_path_buf)),
        Some(new.clone())
    );
    assert!(read(cx, handle, |view, _| view.docs.contains(&new)));
}

#[gpui::test]
fn an_inactive_tab_closes_its_search(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    update(cx, handle, |view, window, cx| {
        let editor = view.active_doc().unwrap().editor.clone();
        editor.update(cx, |editor, cx| editor.open_search(window, cx));
    });
    update(cx, handle, |view, window, cx| {
        view.open_file_in_new_tab(&b, window, cx)
    });
    let search_open = read(cx, handle, |view, cx| {
        view.docs.get(&a).unwrap().editor.read(cx).search.is_some()
    });
    assert!(!search_open);
}

#[test]
fn the_spinner_waits_40_ms() {
    let since = Instant::now();
    let shows = |elapsed: u64| since + Duration::from_millis(elapsed) >= since + LOAD_GRACE;
    assert!(!shows(39));
    assert!(shows(40));
}

// ---- US-025: conflicts with changes on disk ----

fn file_event(path: &Path, change: FileChange) -> WatchEvent {
    WatchEvent::FileChanged {
        path: path.to_path_buf(),
        change,
    }
}

/// Leaves the active document with unsaved text and no write in flight:
/// a first edit is saved (autosave writes at once), a second one waits for
/// the one-second throttle.
fn make_dirty(cx: &mut TestAppContext, handle: WindowHandle<WindowView>, path: &Path) {
    type_text(cx, handle, " mine");
    wait_until(cx, |cx| {
        std::fs::read_to_string(path).is_ok_and(|t| t.contains("mine"))
            && read(cx, handle, |v, cx| {
                !v.active_doc().unwrap().editor.read(cx).state.is_dirty()
            })
    });
    type_text(cx, handle, " more");
}

fn conflict(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) -> Option<Conflict> {
    read(cx, handle, |view, _| {
        view.active_doc().unwrap().conflict.clone()
    })
}

/// Also ports Writer `stores.test.ts:481`: a reload with shorter text keeps
/// the caret inside the text.
#[gpui::test]
fn a_clean_file_reloads_and_keeps_the_caret(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "hello world");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    update(cx, handle, |view, _, cx| {
        let editor = view.active_doc().unwrap().editor.clone();
        editor.update(cx, |editor, cx| {
            editor.select(cerata_markdown::Selection::cursor(5), cx)
        });
    });
    std::fs::write(&a, "hi").unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&a, FileChange::Modified), window, cx)
    });
    assert_eq!(editor_text(cx, handle), "hi");
    let caret = read(cx, handle, |view, cx| {
        view.active_doc().unwrap().editor.read(cx).cursor()
    });
    assert_eq!(caret, 2, "kept, bounded by the new text");
    assert_eq!(conflict(cx, handle), None);
}

#[gpui::test]
fn a_dirty_file_is_neither_reloaded_nor_saved(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    make_dirty(cx, handle, &a);
    std::fs::write(&a, "theirs").unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&a, FileChange::Modified), window, cx)
    });
    assert_eq!(editor_text(cx, handle), "base mine more");
    assert_eq!(
        conflict(cx, handle),
        Some(Conflict::Modified {
            disk: "theirs".into()
        })
    );
    std::thread::sleep(Duration::from_millis(1200));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "theirs",
        "no autosave"
    );

    update(cx, handle, |view, _, cx| view.toggle_compare(cx));
    assert!(read(cx, handle, |view, _| view
        .active_doc()
        .unwrap()
        .compare));

    update(cx, handle, |view, _, cx| view.keep_mine(cx));
    wait_until(cx, |_| {
        std::fs::read_to_string(&a)
            .unwrap()
            .starts_with("base mine more")
    });
    assert_eq!(conflict(cx, handle), None);
}

#[gpui::test]
fn reload_takes_the_disk_version(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    make_dirty(cx, handle, &a);
    std::fs::write(&a, "theirs").unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&a, FileChange::Modified), window, cx)
    });
    update(cx, handle, |view, _, cx| view.reload_from_disk(cx));
    assert_eq!(editor_text(cx, handle), "theirs");
    assert_eq!(conflict(cx, handle), None);
    let dirty = read(cx, handle, |v, cx| {
        v.active_doc().unwrap().editor.read(cx).state.is_dirty()
    });
    assert!(!dirty);
}

#[gpui::test]
fn a_deleted_dirty_file_offers_recreate_and_close(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    make_dirty(cx, handle, &a);
    std::fs::remove_file(&a).unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&a, FileChange::Removed), window, cx)
    });
    assert_eq!(conflict(cx, handle), Some(Conflict::Deleted));
    update(cx, handle, |view, _, cx| view.recreate(cx));
    wait_until(cx, |_| a.is_file());
    assert!(
        std::fs::read_to_string(&a)
            .unwrap()
            .starts_with("base mine more")
    );

    make_dirty(cx, handle, &a);
    std::fs::remove_file(&a).unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&a, FileChange::Removed), window, cx)
    });
    update(cx, handle, |view, window, cx| {
        view.discard_deleted(window, cx)
    });
    let tabs: Vec<Location> = read(cx, handle, |view, _| {
        view.tabs
            .tabs()
            .iter()
            .map(|t| t.location.clone())
            .collect()
    });
    assert_eq!(tabs, [Location::Launcher]);
}

#[gpui::test]
fn a_change_during_a_save_is_compared_with_the_last_write(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    // The change event arrives while a write is in flight: it is held.
    update(cx, handle, |view, window, cx| {
        view.docs.get_mut(&a).unwrap().change_pending = true;
        let _ = window;
        let _ = cx;
    });
    // The save completes with what it wrote, which is what is on disk.
    std::fs::write(&a, "written\n").unwrap();
    let revision = read(cx, handle, |v, cx| {
        v.active_doc().unwrap().editor.read(cx).state.revision()
    });
    update(cx, handle, |view, _, cx| {
        view.on_save_result(
            SaveResult::Saved {
                path: a.clone(),
                revision,
                disk_text: "written\n".into(),
                modified_at: 0,
            },
            cx,
        )
    });
    assert_eq!(conflict(cx, handle), None, "our own write is not a change");
    assert_eq!(editor_text(cx, handle), "base");

    // This time another program wrote after us.
    type_text(cx, handle, "!");
    update(cx, handle, |view, _, _| {
        view.docs.get_mut(&a).unwrap().change_pending = true
    });
    std::fs::write(&a, "external").unwrap();
    update(cx, handle, |view, _, cx| {
        view.on_save_result(
            SaveResult::Saved {
                path: a.clone(),
                revision: 0,
                disk_text: "written\n".into(),
                modified_at: 0,
            },
            cx,
        )
    });
    assert_eq!(
        conflict(cx, handle),
        Some(Conflict::Modified {
            disk: "external".into()
        })
    );
}

// ---- Shared by US-027 to US-030 ----

fn keys(cx: &mut TestAppContext, handle: WindowHandle<WindowView>, keystrokes: &str) {
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_keystrokes(keystrokes);
    vcx.run_until_parked();
}

fn focus_shell(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) {
    update(cx, handle, |view, window, cx| {
        window.focus(&view.focus_handle, cx)
    });
}

fn wait_for_index(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) {
    let runtime = read(cx, handle, |view, _| view.core.runtime()).expect("a workspace");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime
        .index_ready
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        assert!(Instant::now() < deadline, "index timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn setting(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    key: &str,
) -> Option<cerata_core::settings::ConfigValue> {
    read(cx, handle, |view, _| view.settings().get(key))
}

fn choices(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
) -> Vec<crate::palette::Choice> {
    read(cx, handle, |view, cx| {
        view.palette
            .as_ref()
            .expect("palette open")
            .read(cx)
            .choices(cx)
    })
}

fn palette_intent(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
) -> Option<crate::palette::Intent> {
    read(cx, handle, |view, cx| {
        view.palette.as_ref().map(|p| p.read(cx).intent)
    })
}

// ---- US-027: tab menu, delete, duplicate ----

#[gpui::test]
fn the_tab_menu_closes_reveals_and_copies(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("deep/b.md", "b");
    let c = env.file("c.md", "c");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx);
        view.open_file_in_new_tab(&b, window, cx);
        view.open_file_in_new_tab(&c, window, cx);
    });
    let at = gpui::point(px(10.0), px(10.0));
    update(cx, handle, |view, _, cx| view.open_tab_menu(1, at, cx));
    let labels = read(cx, handle, |view, _| {
        view.tab_menu.as_ref().unwrap().labels()
    });
    assert_eq!(
        labels,
        [
            "Close",
            "Close others",
            "Close all",
            "-",
            "Reveal in sidebar",
            "Copy path"
        ]
    );

    let run = |cx: &mut TestAppContext, label: &str| {
        update(cx, handle, |view, _, cx| view.open_tab_menu(1, at, cx));
        let action = read(cx, handle, |view, _| {
            view.tab_menu.as_ref().unwrap().find(label).unwrap()
        });
        update(cx, handle, |view, window, cx| action(view, window, cx));
    };
    run(cx, "Copy path");
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some(b.to_string_lossy().as_ref()));
    run(cx, "Reveal in sidebar");
    let sidebar = read(cx, handle, |view, _| view.sidebar.clone().unwrap());
    let revealed = sidebar.read_with(cx, |s, _| s.rows.iter().any(|r| r.entry.path == b));
    assert!(revealed, "the tab's folder is expanded down to its row");
    run(cx, "Close others");
    let open: Vec<PathBuf> = read(cx, handle, |view, _| {
        view.tabs
            .referenced_paths()
            .map(Path::to_path_buf)
            .collect()
    });
    assert_eq!(open, std::slice::from_ref(&b));
    run(cx, "Close all");
    assert!(read(cx, handle, |view, _| view.tabs.only_launchers()));
}

#[gpui::test]
fn delete_asks_only_for_unsaved_work_and_uses_the_trash(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    let ask = |cx: &mut TestAppContext, paths: Vec<PathBuf>| {
        read(cx, handle, move |view, cx| {
            view.delete_confirmation(&paths, cx)
        })
    };
    assert_eq!(
        ask(cx, vec![a.clone()]),
        None,
        "a saved file goes without asking"
    );
    assert_eq!(
        ask(cx, vec![a.clone(), b.clone()]).as_deref(),
        Some("Delete 2 items?")
    );
    make_dirty(cx, handle, &a);
    assert_eq!(
        ask(cx, vec![a.clone()]).as_deref(),
        Some("\"a.md\" has unsaved changes. Delete anyway?")
    );
    let folder = format!(
        "\"{}\" contains 1 unsaved file(s). Delete anyway?",
        file_name(&env.root())
    );
    assert_eq!(ask(cx, vec![env.root()]), Some(folder));

    update(cx, handle, |view, _, cx| {
        view.delete_now(std::slice::from_ref(&b), cx)
    });
    let notice = read(cx, handle, |view, _| view.notice.clone());
    match notice {
        // Headless sessions may have no trash: the file must survive.
        Some(notice) if notice.contains("Failed to delete") => assert!(b.exists()),
        _ => assert!(!b.exists(), "moved to the trash"),
    }
}

#[gpui::test]
fn duplicate_copies_unsaved_text_into_a_new_tab(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "saved");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    make_dirty(cx, handle, &a);
    update(cx, handle, |view, window, cx| {
        view.duplicate(&a, window, cx)
    });
    let copy = env.root().join("a copy.md");
    assert!(std::fs::read_to_string(&copy).unwrap().contains("more"));
    let active = read(cx, handle, |view, _| view.tabs.active_path().map(file_name));
    assert_eq!(active.as_deref(), Some("a copy.md"));
    assert_eq!(read(cx, handle, |view, _| view.tabs.tabs().len()), 2);
}

// ---- US-028: palette, welcome, new tab, compact window ----

#[gpui::test]
fn the_palette_keys_open_it_and_files_follow_50_ms_after_typing(cx: &mut TestAppContext) {
    use crate::palette::{Choice, Intent};
    let env = setup(cx);
    env.file("alpha.md", "# Alpha");
    env.file("beta.md", "# Beta");
    let handle = workspace(cx, &env);
    wait_for_index(cx, handle);
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("p"));
    assert_eq!(palette_intent(cx, handle), Some(Intent::Search));

    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_input("alp");
    vcx.run_until_parked();
    let files = |cx: &mut TestAppContext| -> Vec<String> {
        choices(cx, handle)
            .into_iter()
            .filter_map(|c| match c {
                Choice::File(path) => Some(file_name(&path)),
                _ => None,
            })
            .collect()
    };
    assert!(files(cx).is_empty(), "nothing before the debounce");
    cx.executor().advance_clock(crate::palette::SEARCH_DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(files(cx), ["alpha.md"]);
    let highlighted = read(cx, handle, |view, cx| {
        view.palette.as_ref().unwrap().read(cx).files[0]
            .match_indices
            .clone()
    });
    assert!(
        !highlighted.is_empty(),
        "the matched characters are highlighted"
    );

    keys(cx, handle, "escape");
    assert!(read(cx, handle, |view, _| view.palette.is_none()));
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("n"));
    assert_eq!(palette_intent(cx, handle), Some(Intent::CreateFile));
    keys(cx, handle, "escape");
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("o"));
    assert_eq!(palette_intent(cx, handle), Some(Intent::Search));
}

#[gpui::test]
fn the_palette_commands_are_writers_filtered_by_substring(cx: &mut TestAppContext) {
    use crate::palette::{Choice, Command};
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("shift-p"));
    let all: Vec<Choice> = Command::ALL.into_iter().map(Choice::Command).collect();
    assert_eq!(
        choices(cx, handle),
        all,
        "with a workspace and a file every command applies"
    );
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_input("ZOOM");
    vcx.run_until_parked();
    let commands: Vec<Choice> = choices(cx, handle)
        .into_iter()
        .filter(|c| matches!(c, Choice::Command(_)))
        .collect();
    assert_eq!(
        commands,
        [Command::ZoomIn, Command::ZoomOut, Command::ResetZoom].map(Choice::Command)
    );
    // Enter runs the first one.
    keys(cx, handle, "enter");
    assert!(read(cx, handle, |view, _| view.palette.is_none()));
    assert_eq!(
        setting(cx, handle, crate::zoom::ZOOM_KEY),
        Some(ConfigValue::Number(110.0))
    );
}

#[gpui::test]
fn without_a_workspace_the_palette_offers_commands_and_says_why(cx: &mut TestAppContext) {
    use crate::palette::Choice;
    let _env = setup(cx);
    let handle = cx.update(|cx| app::open_window(cx, None, None)).unwrap();
    cx.run_until_parked();
    assert!(matches!(
        read(cx, handle, |view, cx| view.page(cx)),
        Page::Welcome
    ));
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("p"));
    assert!(
        read(cx, handle, |view, _| view.palette.is_none()),
        "Cmd+P needs a folder"
    );
    keys(cx, handle, secondary!("shift-p"));
    let hint = read(cx, handle, |view, cx| {
        view.palette.as_ref().unwrap().read(cx).no_workspace_hint()
    });
    assert_eq!(hint, Some("Open a folder to search its files."));
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_input("o");
    vcx.run_until_parked();
    cx.executor().advance_clock(crate::palette::SEARCH_DEBOUNCE);
    cx.run_until_parked();
    let list = choices(cx, handle);
    assert!(!list.is_empty());
    assert!(list.iter().all(|c| matches!(c, Choice::Command(_))));
}

#[gpui::test]
fn the_welcome_screen_opens_a_folder_or_a_file_in_its_folder(cx: &mut TestAppContext) {
    let env = setup(cx);
    let note = env.file("notes/note.md", "note");
    let other = env.folder("other");
    let handle = cx.update(|cx| app::open_window(cx, None, None)).unwrap();
    cx.run_until_parked();
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("welcome-add-folder").is_some());
    assert!(vcx.debug_bounds("welcome-open-file").is_some());

    // "Add Folder" once the folder is picked: it opens in this window.
    cx.update(|cx| super::commands::open_picked_folder(cx, other.clone(), Some(handle)));
    cx.run_until_parked();
    let root = |cx: &mut TestAppContext| {
        read(cx, handle, |view, _| {
            view.workspace_root().map(Path::to_path_buf)
        })
    };
    assert_eq!(root(cx), Some(other));
    assert_eq!(windows(cx).len(), 1);

    // "Open File" from the welcome screen: its folder, then the file.
    update(cx, handle, |view, window, cx| {
        view.close_workspace(window, cx)
    });
    assert!(matches!(
        read(cx, handle, |view, cx| view.page(cx)),
        Page::Welcome
    ));
    cx.update(|cx| super::commands::open_picked_file(cx, note.clone(), Some(handle)));
    cx.run_until_parked();
    assert_eq!(root(cx), note.parent().map(Path::to_path_buf));
    let active = read(cx, handle, |view, _| {
        view.tabs.active_path().map(Path::to_path_buf)
    });
    assert_eq!(active, Some(note));
}

#[gpui::test]
fn a_new_tab_offers_create_and_search(cx: &mut TestAppContext) {
    use crate::palette::Intent;
    let env = setup(cx);
    let handle = workspace(cx, &env);
    assert!(matches!(
        read(cx, handle, |view, cx| view.page(cx)),
        Page::Launcher
    ));
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let create = vcx
        .debug_bounds("launcher-create")
        .expect("Create new note");
    vcx.simulate_click(create.center(), gpui::Modifiers::none());
    assert_eq!(palette_intent(cx, handle), Some(Intent::CreateFile));
    update(cx, handle, |view, window, cx| {
        view.close_palette(window, cx)
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let search = vcx.debug_bounds("launcher-search").expect("Search");
    vcx.simulate_click(search.center(), gpui::Modifiers::none());
    assert_eq!(palette_intent(cx, handle), Some(Intent::Search));
}

#[gpui::test]
fn the_compact_window_has_no_tabs_sidebar_close_key_or_settings(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    launch(cx, &env.root(), &["a.md"]);
    let handle = windows(cx)[0];
    assert_eq!(read(cx, handle, |view, _| view.chrome), Chrome::Compact);
    assert!(read(cx, handle, |view, _| view.sidebar.is_none()));
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("w"));
    keys(cx, handle, secondary!("t"));
    keys(cx, handle, secondary!(","));
    assert_eq!(windows(cx).len(), 1, "Cmd+W does not close it");
    let tabs = read(cx, handle, |view, _| {
        view.tabs
            .tabs()
            .iter()
            .map(|t| t.location.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(tabs, [Location::File { path: a }]);
    // Cmd+N still works, on the file's folder.
    keys(cx, handle, secondary!("n"));
    assert_eq!(
        palette_intent(cx, handle),
        Some(crate::palette::Intent::CreateFile)
    );
}

#[gpui::test]
fn the_compact_title_lists_global_recents_without_the_active_file(cx: &mut TestAppContext) {
    let env = setup(cx);
    let files: Vec<PathBuf> = (0..30)
        .map(|i| env.file(&format!("n{i:02}.md"), "x"))
        .collect();
    let core = cx.update(|cx| app::core(cx));
    for file in &files {
        core.recents.record_file(file).unwrap();
    }
    launch(cx, &env.root(), &["n05.md"]);
    let handle = windows(cx)[0];
    assert_eq!(read(cx, handle, |view, _| view.compact_title()), "n05");
    update(cx, handle, |view, window, cx| {
        view.toggle_recents_picker(window, cx)
    });
    let entries = read(cx, handle, |view, _| view.recents_picker.clone().unwrap());
    assert_eq!(
        entries.len(),
        29,
        "the 30 global recents, the active one left out"
    );
    assert!(entries.iter().all(|e| file_name(&e.path) != "n05.md"));

    let removed = entries[0].path.clone();
    update(cx, handle, |view, _, cx| view.remove_recent(&removed, cx));
    let entries = read(cx, handle, |view, _| view.recents_picker.clone().unwrap());
    assert!(entries.iter().all(|e| e.path != removed));
    let stored = core.recents.existing_files().unwrap();
    assert!(
        stored.iter().all(|p| file_name(p) != file_name(&removed)),
        "removed from the store"
    );

    // Escape closes the picker and gives the note its focus back; a recent
    // opens in the same window.
    keys(cx, handle, "escape");
    assert!(read(cx, handle, |view, _| view.recents_picker.is_none()));
    let editor_focused = update(cx, handle, |view, window, cx| {
        view.active_doc()
            .unwrap()
            .editor
            .read(cx)
            .focus_handle
            .is_focused(window)
    });
    assert!(editor_focused);
    let next = entries[0].path.clone();
    update(cx, handle, |view, window, cx| {
        view.toggle_recents_picker(window, cx);
        view.choose_recent(next.clone(), window, cx);
    });
    let active = read(cx, handle, |view, _| view.tabs.active_path().map(file_name));
    assert_eq!(active, Some(file_name(&next)));
    assert_eq!(windows(cx).len(), 1);
}

// ---- US-029: settings panel ----

fn settings_panel(
    cx: &mut TestAppContext,
    env: &Env,
) -> (
    WindowHandle<WindowView>,
    Entity<crate::settings_panel::SettingsPanel>,
) {
    let handle = workspace(cx, env);
    focus_shell(cx, handle);
    keys(cx, handle, secondary!(","));
    let vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let panel = read(cx, handle, |view, _| view.settings_panel.clone())
        .expect("the Settings tab shows the panel");
    (handle, panel)
}

#[gpui::test]
fn settings_write_through_the_panel_and_reset_to_the_default(cx: &mut TestAppContext) {
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let (handle, panel) = settings_panel(cx, &env);
    assert!(matches!(
        read(cx, handle, |view, cx| view.page(cx)),
        Page::Settings(_)
    ));
    assert!(!panel.read_with(cx, |p, _| p.is_modified("editor.font-size")));
    let field = panel.read_with(cx, |p, _| p.fields["editor.font-size"].clone());
    field.update(cx, |field, cx| field.set_text("18", cx));
    panel.update(cx, |p, cx| p.commit_field("editor.font-size", cx));
    cx.run_until_parked();
    assert_eq!(
        setting(cx, handle, "editor.font-size"),
        Some(ConfigValue::Number(18.0))
    );
    assert!(
        panel.read_with(cx, |p, _| p.is_modified("editor.font-size")),
        "Reset shows"
    );
    assert_eq!(read(cx, handle, |view, _| view.config.font_size), 18.0);

    panel.update(cx, |p, cx| p.reset("editor.font-size", cx));
    wait_until(cx, |cx| {
        setting(cx, handle, "editor.font-size") == Some(ConfigValue::Number(16.0))
    });
    assert!(!panel.read_with(cx, |p, _| p.is_modified("editor.font-size")));
}

#[gpui::test]
fn an_invalid_hex_color_turns_red_and_is_not_saved(cx: &mut TestAppContext) {
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let (handle, panel) = settings_panel(cx, &env);
    let key = "theme.light.accent";
    let before = setting(cx, handle, key);
    let field = panel.read_with(cx, |p, _| p.fields[key].clone());
    field.update(cx, |field, cx| field.set_text("#12", cx));
    panel.update(cx, |p, cx| p.commit_field(key, cx));
    cx.run_until_parked();
    assert!(field.read_with(cx, |f, _| f.invalid), "shown in red");
    assert_eq!(setting(cx, handle, key), before, "not saved");
    field.update(cx, |field, cx| field.set_text("#00ff00", cx));
    panel.update(cx, |p, cx| p.commit_field(key, cx));
    cx.run_until_parked();
    assert!(!field.read_with(cx, |f, _| f.invalid));
    assert_eq!(
        setting(cx, handle, key),
        Some(ConfigValue::String("#00FF00".into()))
    );
}

#[gpui::test]
fn a_font_pick_replaces_only_the_first_family_and_presets_apply(cx: &mut TestAppContext) {
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let (handle, panel) = settings_panel(cx, &env);
    let text = |cx: &mut TestAppContext, key: &str| {
        setting(cx, handle, key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap()
    };
    let before = text(cx, "fonts.mono");
    panel.update(cx, |p, cx| p.pick_font("fonts.mono", "Fira Code", cx));
    cx.run_until_parked();
    let after = text(cx, "fonts.mono");
    assert_eq!(
        after,
        crate::settings_panel::stack_with_family("Fira Code", &before)
    );
    assert!(after.starts_with("\"Fira Code\", "));

    panel.update(cx, |p, cx| {
        p.apply_preset(crate::theme::Mode::Dark, "Warm Paper", cx)
    });
    cx.run_until_parked();
    assert_eq!(
        setting(cx, handle, "theme.dark.preset"),
        Some(ConfigValue::String("Warm Paper".into()))
    );
    assert_eq!(text(cx, "theme.dark.background"), "#1F1A14");
}

// ---- US-030: theme, zoom, status bar, title ----

#[gpui::test]
fn zoom_keys_walk_the_ladder_without_touching_the_font_size(cx: &mut TestAppContext) {
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    focus_shell(cx, handle);
    let zoom = |cx: &mut TestAppContext| setting(cx, handle, crate::zoom::ZOOM_KEY);
    keys(cx, handle, secondary!("="));
    assert_eq!(zoom(cx), Some(ConfigValue::Number(110.0)));
    let editor_zoom = read(cx, handle, |view, cx| {
        view.active_doc().unwrap().editor.read(cx).config.zoom
    });
    assert!(
        (editor_zoom - 1.1).abs() < 1e-6,
        "the editor takes the zoom"
    );
    keys(cx, handle, secondary!("-"));
    keys(cx, handle, secondary!("-"));
    assert_eq!(zoom(cx), Some(ConfigValue::Number(90.0)));
    keys(cx, handle, secondary!("0"));
    assert_eq!(zoom(cx), Some(ConfigValue::Number(100.0)));
    // Persisted in the global config, so it survives a relaunch.
    let path = read(cx, handle, |view, _| view.settings().global_path());
    wait_until(cx, |_| {
        std::fs::read_to_string(&path).is_ok_and(|text| text.contains("editor.zoom = 100"))
    });
    assert_eq!(
        setting(cx, handle, "editor.font-size"),
        Some(ConfigValue::Number(16.0))
    );
}

#[gpui::test]
fn the_status_bar_counts_and_toggles_its_metrics(cx: &mut TestAppContext) {
    use super::status_bar::{METRICS, Stats};
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let a = env.file("a.md", "# Title\n\none two three\n");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    let stats = update(cx, handle, |view, _, cx| view.active_stats(cx));
    assert_eq!(
        stats,
        Some(Stats {
            words: 4,
            characters: 19,
            paragraphs: 2
        })
    );
    let at = gpui::point(px(0.0), px(0.0));
    for (_, _, label) in METRICS {
        update(cx, handle, |view, _, cx| view.open_status_menu(at, cx));
        let labels = read(cx, handle, |view, _| {
            view.tab_menu.as_ref().unwrap().labels()
        });
        assert_eq!(labels, ["Words", "Characters", "Paragraphs"]);
        let action = read(cx, handle, |view, _| {
            view.tab_menu.as_ref().unwrap().find(label).unwrap()
        });
        update(cx, handle, |view, window, cx| action(view, window, cx));
    }
    assert_eq!(
        setting(cx, handle, "statusbar.show-words"),
        Some(ConfigValue::Bool(false))
    );
    let shown = update(cx, handle, |view, _, cx| {
        super::status_bar::render(view, cx).is_some()
    });
    assert!(!shown, "hidden when every metric is off");
}

#[gpui::test]
fn the_window_title_follows_the_active_tab(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("note.md", "a");
    let handle = workspace(cx, &env);
    assert_eq!(
        read(cx, handle, |view, cx| view.title(cx)),
        "New Tab - Cerata"
    );
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    assert_eq!(
        read(cx, handle, |view, cx| view.title(cx)),
        "note.md - Cerata"
    );
    make_dirty(cx, handle, &a);
    assert_eq!(
        read(cx, handle, |view, cx| view.title(cx)),
        "note.md (unsaved) - Cerata"
    );
    update(cx, handle, |view, window, cx| {
        view.close_workspace(window, cx)
    });
    assert_eq!(read(cx, handle, |view, cx| view.title(cx)), "Cerata");
}

#[gpui::test]
fn the_theme_preference_sets_the_palette(cx: &mut TestAppContext) {
    use cerata_core::settings::ConfigValue;
    let env = setup(cx);
    let handle = workspace(cx, &env);
    update(cx, handle, |view, _, cx| {
        view.write_setting(
            crate::theme::THEME_KEY,
            ConfigValue::String("dark".into()),
            cx,
        );
        view.setting_written(cx);
    });
    let palette = crate::editor::theme::palette();
    assert!(palette.dark);
    assert_eq!(palette.text, gpui::Hsla::from(gpui::rgb(0xFCFCFC)));
    // "Toggle Dark Mode" cycles system, light, dark.
    update(cx, handle, |view, _, cx| view.cycle_theme(cx));
    assert_eq!(
        setting(cx, handle, crate::theme::THEME_KEY),
        Some(ConfigValue::String("system".into()))
    );
    update(cx, handle, |view, _, cx| view.cycle_theme(cx));
    assert_eq!(
        setting(cx, handle, crate::theme::THEME_KEY),
        Some(ConfigValue::String("light".into()))
    );
    assert!(!crate::editor::theme::palette().dark);
}

/// An edit the autosave throttle still holds reaches the disk when the
/// window closes from its menu.
#[gpui::test]
fn closing_the_window_writes_the_edit_the_throttle_holds(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    make_dirty(cx, handle, &a);
    focus_shell(cx, handle);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.dispatch_action(crate::app_menu::CloseWindow);
    vcx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "base mine more
"
    );
}

/// Quitting skips `on_window_should_close`; the quit hook writes the edit
/// the throttle still holds.
#[gpui::test]
fn quitting_writes_the_edit_the_throttle_holds(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    make_dirty(cx, handle, &a);
    cx.update(|cx| cx.shutdown());
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "base mine more
"
    );
}

/// The keyboard runs a shell menu: it takes the focus, arrows pick an item,
/// Enter runs it, Escape closes another one and gives the focus back.
#[gpui::test]
fn a_menu_takes_the_keyboard_and_gives_the_focus_back(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx);
        view.open_file_in_new_tab(&b, window, cx);
    });
    focus_shell(cx, handle);
    let at = gpui::point(px(10.0), px(10.0));
    update(cx, handle, |view, _, cx| view.open_tab_menu(1, at, cx));
    keys(cx, handle, "escape");
    assert!(read(cx, handle, |view, _| view.tab_menu.is_none()));
    let shell_focused = handle
        .update(cx, |view, window, _| view.focus_handle.is_focused(window))
        .unwrap();
    assert!(shell_focused, "the focus went back to the shell");

    // Close (the first item) on the second tab.
    update(cx, handle, |view, _, cx| view.open_tab_menu(1, at, cx));
    keys(cx, handle, "down enter");
    assert!(read(cx, handle, |view, _| view.tab_menu.is_none()));
    assert_eq!(read(cx, handle, |view, _| view.tabs.tabs().len()), 1);
}

/// A failed save says so in words, with what happens to the changes; the
/// tab's red dot alone carried it by color.
#[gpui::test]
fn a_failed_save_shows_a_notice(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    let mut permissions = std::fs::metadata(&a).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&a, permissions.clone()).unwrap();
    type_text(cx, handle, " mine");
    wait_until(cx, |cx| read(cx, handle, |view, _| view.notice.is_some()));
    let notice = read(cx, handle, |view, _| view.notice.clone().unwrap());
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&a, permissions).unwrap();
    assert!(notice.starts_with("Cannot save a.md:"), "{notice}");
    assert!(
        notice.ends_with("Your changes stay in the editor; the next edit tries again."),
        "{notice}"
    );
}

/// Every shortcut on a digit or punctuation key also has a binding by key
/// position (AZERTY gives `ctrl-à` for Ctrl+0).
#[gpui::test]
fn digit_and_punctuation_shortcuts_also_bind_by_position(cx: &mut TestAppContext) {
    let _env = setup(cx);
    let count = cx.update(|cx| positional_bindings(cx).len());
    assert_eq!(count, 13);
}

// ---- EP-004: Writer store invariants ----

/// Ports Writer `stores.test.ts:321`: closing a tab activates its neighbor
/// and drops the tab's clean document.
#[gpui::test]
fn closing_a_tab_drops_its_clean_document(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx);
        view.open_file_in_new_tab(&b, window, cx);
    });
    update(cx, handle, |view, window, cx| view.close_tab(1, window, cx));
    let (paths, active, has_b) = read(cx, handle, |view, _| {
        (
            view.tabs
                .referenced_paths()
                .map(Path::to_path_buf)
                .collect::<Vec<_>>(),
            view.tabs.active_path().map(Path::to_path_buf),
            view.docs.contains(&b),
        )
    });
    assert_eq!(paths, std::slice::from_ref(&a));
    assert_eq!(active, Some(a));
    assert!(!has_b, "the clean document of the closed tab is dropped");
}

/// Ports Writer `stores.test.ts:500`: a caret left beyond the text by a
/// reload while its tab was inactive is inside the text on switching back.
#[gpui::test]
fn a_stale_caret_is_clamped_when_its_tab_comes_back(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", &"a".repeat(100));
    let b = env.file("b.md", "b");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    update(cx, handle, |view, _, cx| {
        let editor = view.active_doc().unwrap().editor.clone();
        editor.update(cx, |editor, cx| {
            editor.select(cerata_markdown::Selection::cursor(80), cx)
        });
    });
    update(cx, handle, |view, window, cx| {
        view.open_file_in_new_tab(&b, window, cx)
    });
    std::fs::write(&a, "short").unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&a, FileChange::Modified), window, cx)
    });
    update(cx, handle, |view, window, cx| {
        view.activate_tab(0, window, cx)
    });
    assert_eq!(editor_text(cx, handle), "short");
    let caret = read(cx, handle, |view, cx| {
        view.active_doc().unwrap().editor.read(cx).cursor()
    });
    assert!(caret <= "short".len(), "caret {caret} is inside the text");
}

/// Ports Writer `stores.test.ts:847`: a closed palette keeps no search
/// text; the next one opens empty.
#[gpui::test]
fn reopening_the_palette_shows_an_empty_query(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("a.md", "a");
    let handle = workspace(cx, &env);
    let query = |cx: &mut TestAppContext| {
        read(cx, handle, |view, cx| {
            view.palette
                .as_ref()
                .expect("palette open")
                .read(cx)
                .query(cx)
        })
    };
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("p"));
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_input("hello");
    vcx.run_until_parked();
    assert_eq!(query(cx), "hello");
    keys(cx, handle, "escape");
    assert!(read(cx, handle, |view, _| view.palette.is_none()));
    focus_shell(cx, handle);
    keys(cx, handle, secondary!("p"));
    assert_eq!(query(cx), "");
}

/// Ports Writer `stores.test.ts:1005`: a compact window that navigates to
/// another file now stands for that file (the one routing, reads and saves
/// follow) and no longer for the first.
#[gpui::test]
fn a_compact_window_moves_its_standalone_file_on_navigation(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("a.md", "a");
    let b = env.file("b.md", "b");
    launch(cx, &env.root(), &["a.md"]);
    let handle = windows(cx)[0];
    update(cx, handle, |view, window, cx| {
        view.open_elsewhere(crate::routing::OpenRequest::File(b.clone()), window, cx)
    });
    let (chrome, tabs, standalone) = read(cx, handle, |view, _| {
        (
            view.chrome,
            view.tabs
                .tabs()
                .iter()
                .map(|t| t.location.clone())
                .collect::<Vec<_>>(),
            view.core.standalone_file(),
        )
    });
    assert_eq!(chrome, Chrome::Compact);
    assert_eq!(tabs, [Location::file(&b)]);
    assert_eq!(standalone, Some(b));
    launch(cx, &env.root(), &["b.md"]);
    assert_eq!(windows(cx).len(), 1, "the window on b is revealed");
    launch(cx, &env.root(), &["a.md"]);
    assert_eq!(windows(cx).len(), 2, "a is no longer this window's file");
}

/// Ports Writer `stores.test.ts:1188`: closing a workspace tells the core
/// first (the session is saved, the window state lets go of the
/// workspace), then clears the root, the listings, the documents and the
/// tabs.
#[gpui::test]
fn closing_the_workspace_tells_the_core_then_clears_the_window(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("dir/a.md", "a");
    let handle = workspace(cx, &env);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    let sidebar = read(cx, handle, |view, _| view.sidebar.clone().unwrap());
    sidebar.update(cx, |s, cx| s.expand(&env.root().join("dir"), cx));
    cx.run_until_parked();
    assert!(sidebar.read_with(cx, |s, _| !s.listings.is_empty()));

    update(cx, handle, |view, window, cx| {
        view.close_workspace(window, cx)
    });
    let saved = cx
        .update(|cx| app::core(cx).sessions.load(&env.root()))
        .unwrap()
        .map(|s| s.tabs.len());
    assert_eq!(saved, Some(1), "the session was saved before the tabs went");
    let (runtime, root, docs, launchers, sidebar) = read(cx, handle, |view, _| {
        (
            view.core.runtime().is_some(),
            view.workspace_root().map(Path::to_path_buf),
            view.docs.iter().count(),
            view.tabs.only_launchers(),
            view.sidebar.is_some(),
        )
    });
    assert!(!runtime, "the core no longer holds the workspace");
    assert_eq!(root, None);
    assert_eq!(docs, 0);
    assert!(launchers, "only the launcher every window keeps");
    assert!(!sidebar, "the sidebar and its listings are gone");
}

/// Ports Writer `stores.test.ts:1213`: closing with no workspace open does
/// nothing.
#[gpui::test]
fn closing_without_a_workspace_does_nothing(cx: &mut TestAppContext) {
    let _env = setup(cx);
    let handle = cx.update(|cx| app::open_window(cx, None, None)).unwrap();
    cx.run_until_parked();
    let state = |cx: &mut TestAppContext| {
        read(cx, handle, |view, _| {
            (
                view.tabs.tabs().iter().map(|t| t.id).collect::<Vec<_>>(),
                view.workspace_root().map(Path::to_path_buf),
                view.notice.clone(),
                view.chrome,
            )
        })
    };
    let before = state(cx);
    update(cx, handle, |view, window, cx| {
        view.close_workspace(window, cx)
    });
    assert_eq!(state(cx), before);
    assert!(read(cx, handle, |view, _| view.core.runtime().is_none()));
}
