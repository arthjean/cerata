//! The sidebar inside a real workspace window.

use std::path::{Path, PathBuf};

use cerata_core::open_target::{self, OpenTarget};
use cerata_core::startup::CoreApp;
use gpui::{Entity, TestAppContext, VisualTestContext, WindowHandle, px};
use tempfile::TempDir;

use super::tree::Listing;
use super::*;
use crate::app;
use crate::shell::WindowView;

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
        cx.bind_keys(crate::shell::bindings());
        cx.bind_keys(bindings());
        app::init(cx, core);
    });
    Env {
        _config: config,
        _data: data,
        dir: TempDir::new().unwrap(),
    }
}

fn open(cx: &mut TestAppContext, env: &Env) -> (WindowHandle<WindowView>, Entity<Sidebar>) {
    let root = env.root();
    let handle = cx
        .update(|cx| app::open_window(cx, Some(OpenTarget::Workspace(root)), None))
        .unwrap();
    cx.run_until_parked();
    let sidebar = cx
        .update(|cx| handle.read_with(cx, |view, _| view.sidebar.clone()))
        .unwrap()
        .expect("a workspace window has a sidebar");
    (handle, sidebar)
}

/// Waits for the index (a real thread) and lets the sidebar reload.
fn wait_for_index(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let ready = sidebar.read_with(cx, |s, _| {
            s.runtime()
                .is_some_and(|r| r.index_ready.load(std::sync::atomic::Ordering::SeqCst))
        });
        if ready {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "index timed out");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    sidebar.update(cx, |s, cx| s.refresh_all(cx));
    cx.run_until_parked();
}

fn row_names(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>) -> Vec<(String, usize)> {
    sidebar.read_with(cx, |s, _| {
        s.rows
            .iter()
            .map(|r| (r.entry.name.clone(), r.depth))
            .collect()
    })
}

fn shown(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>, path: &Path) -> bool {
    sidebar.read_with(cx, |s, _| s.rows.iter().any(|r| r.entry.path == path))
}

fn row_at(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>, name: &str) -> tree::Row {
    sidebar.read_with(cx, |s, _| {
        s.rows
            .iter()
            .find(|r| r.entry.name == name)
            .cloned()
            .expect("a visible row")
    })
}

#[gpui::test]
fn root_folders_lead_as_projects_and_root_files_close_under_unfiled(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("b.md", "# Beta");
    env.file("a.md", "no title");
    env.file("notes/inner.md", "# Inner");
    env.file("notes/sub/deep.md", "x");
    let (handle, sidebar) = open(cx, &env);
    assert_eq!(
        row_names(cx, &sidebar),
        [
            ("notes".into(), 0),
            (tree::UNFILED.into(), 0),
            ("a.md".into(), 1),
            ("b.md".into(), 1)
        ],
        "projects start collapsed, Unfiled open"
    );

    // A click on a project opens it and selects nothing.
    let notes = row_at(cx, &sidebar, "notes");
    sidebar.update(cx, |s, cx| s.click_header(&notes, cx));
    cx.run_until_parked();
    assert_eq!(
        row_names(cx, &sidebar)[..3],
        [
            ("notes".into(), 0),
            ("sub".into(), 1),
            ("inner.md".into(), 1)
        ]
    );
    assert!(sidebar.read_with(cx, |s, _| s.selection.anchor.is_none()));

    // Unfiled closes and opens on a click.
    let unfiled = row_at(cx, &sidebar, tree::UNFILED);
    assert_eq!(unfiled.entry.path, env.root());
    sidebar.update(cx, |s, cx| s.click_header(&unfiled, cx));
    assert!(!shown(cx, &sidebar, &env.root().join("a.md")));
    sidebar.update(cx, |s, cx| s.click_header(&unfiled, cx));
    assert!(shown(cx, &sidebar, &env.root().join("a.md")));

    // A click on a file opens it in the window.
    let file = row_at(cx, &sidebar, "inner.md").entry;
    sidebar.update(cx, |s, cx| s.click_row(&file, cx));
    cx.run_until_parked();
    let active = cx
        .update(|cx| handle.read_with(cx, |v, _| v.tabs.active_path().map(Path::to_path_buf)))
        .unwrap();
    assert_eq!(active, Some(file.path.clone()));
    let highlighted = sidebar.read_with(cx, |s, _| s.active_path.clone());
    assert_eq!(highlighted, Some(file.path));
}

#[gpui::test]
fn a_file_dropped_on_unfiled_leaves_its_project_for_the_root(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("a.md", "a");
    let x = env.file("work/x.md", "x");
    let (_handle, sidebar) = open(cx, &env);
    let work = env.root().join("work");
    sidebar.update(cx, |s, cx| s.toggle_dir(&work, cx));
    cx.run_until_parked();
    let unfiled = row_at(cx, &sidebar, tree::UNFILED).entry;
    let start = gpui::point(px(10.0), px(10.0));
    let highlight = sidebar.update(cx, |s, cx| {
        s.begin_drag(&x, start);
        s.drag_moved(start + gpui::point(px(0.0), px(8.0)), Some(unfiled), cx);
        s.drag_highlight()
    });
    // Unfiled and its file, not the project above.
    assert_eq!(highlight, Some(2..4));
    sidebar.update(cx, |s, cx| {
        s.end_drag(cx);
    });
    cx.run_until_parked();
    let back = env.root().join("x.md");
    assert!(back.exists() && !x.exists());

    // Dropped on a project's header, it moves into the project.
    let project = row_at(cx, &sidebar, "work").entry;
    sidebar.update(cx, |s, cx| {
        s.begin_drag(&back, start);
        s.drag_moved(start + gpui::point(px(0.0), px(8.0)), Some(project), cx);
        s.end_drag(cx);
    });
    cx.run_until_parked();
    assert!(x.exists() && !back.exists());
}

#[gpui::test]
fn revealing_or_making_a_root_file_reopens_unfiled(cx: &mut TestAppContext) {
    use cerata_core::fs::EntryKind;
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let (handle, sidebar) = open(cx, &env);
    let unfiled = row_at(cx, &sidebar, tree::UNFILED);
    sidebar.update(cx, |s, cx| s.click_header(&unfiled, cx));
    sidebar.update(cx, |s, cx| s.reveal_in_sidebar(&a, cx));
    cx.run_until_parked();
    assert!(shown(cx, &sidebar, &a), "a reveal opens Unfiled");

    sidebar.update(cx, |s, cx| s.click_header(&unfiled, cx));
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.create_entry(None, EntryKind::File, window, cx)
    });
    let renaming = sidebar.read_with(cx, |s, _| s.renaming.as_ref().map(|r| r.path.clone()));
    assert!(renaming.is_some(), "a new root file is named under Unfiled");
}

#[gpui::test]
fn a_new_project_is_named_in_its_header_and_never_opened_as_a_note(cx: &mut TestAppContext) {
    use cerata_core::fs::EntryKind;
    let env = setup(cx);
    env.file("a.md", "a");
    let (handle, sidebar) = open(cx, &env);
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.create_entry(None, EntryKind::Folder, window, cx)
    });
    let (path, created) = sidebar
        .read_with(cx, |s, _| {
            s.renaming.as_ref().map(|r| (r.path.clone(), r.created))
        })
        .expect("the new project is renamed in place");
    assert!(path.is_dir());
    assert!(!created, "Enter must not open a folder as a note");
    assert_eq!(entry(cx, &sidebar, &path).kind, tree::RowKind::Project);
}

#[gpui::test]
fn labels_follow_the_title_setting(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("z.md", "# Alpha");
    env.file("a.md", "# Zulu");
    let (_handle, sidebar) = open(cx, &env);
    assert_eq!(
        row_names(cx, &sidebar)
            .into_iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [tree::UNFILED, "z.md", "a.md"],
        "sorted by title"
    );
}

#[gpui::test]
fn a_failed_folder_shows_an_error_and_retries_on_the_next_expand(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("dir/x.md", "x");
    let (_handle, sidebar) = open(cx, &env);
    let dir = env.root().join("dir");
    std::fs::remove_dir_all(&dir).unwrap();
    sidebar.update(cx, |s, cx| s.toggle_dir(&dir, cx));
    cx.run_until_parked();
    let error = sidebar.read_with(cx, |s, _| s.rows[0].error.clone());
    assert!(error.is_some(), "the row shows the read error");
    env.file("dir/x.md", "x");
    sidebar.update(cx, |s, cx| s.toggle_dir(&dir, cx));
    cx.run_until_parked();
    assert_eq!(
        row_names(cx, &sidebar),
        [("dir".into(), 0), ("x.md".into(), 1)]
    );
    assert!(matches!(
        sidebar.read_with(cx, |s, _| s.listings.get(&dir).cloned()),
        Some(Listing::Loaded(_))
    ));
}

#[gpui::test]
fn recents_appear_from_ten_files_in_pages_of_four(cx: &mut TestAppContext) {
    let env = setup(cx);
    for i in 0..9 {
        env.file(&format!("n{i}.md"), "x");
    }
    let (_handle, sidebar) = open(cx, &env);
    wait_for_index(cx, &sidebar);
    assert!(
        sidebar.read_with(cx, |s, _| s.recents.is_empty()),
        "9 files: no Recents"
    );

    let env2 = setup(cx);
    for i in 0..12 {
        env2.file(&format!("n{i}.md"), "x");
    }
    let (_handle, sidebar) = open(cx, &env2);
    wait_for_index(cx, &sidebar);
    let (count, more) = sidebar.read_with(cx, |s, _| (s.recents.len(), s.recents_more));
    assert_eq!((count, more), (4, true));
    sidebar.update(cx, |s, cx| s.show_more(Section::Recents, cx));
    cx.run_until_parked();
    assert_eq!(sidebar.read_with(cx, |s, _| s.recents.len()), 8);
}

#[gpui::test]
fn a_new_file_being_named_stays_out_of_recents(cx: &mut TestAppContext) {
    use cerata_core::fs::EntryKind;
    let env = setup(cx);
    for i in 0..12 {
        env.file(&format!("n{i}.md"), "x");
    }
    let (handle, sidebar) = open(cx, &env);
    wait_for_index(cx, &sidebar);
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.create_entry(None, EntryKind::File, window, cx)
    });
    let created = sidebar
        .read_with(cx, |s, _| s.renaming.as_ref().map(|r| r.path.clone()))
        .expect("the new file is renamed in place");
    // The index (a real thread) learns of the new file.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !sidebar.read_with(cx, |s, _| {
        s.runtime()
            .is_some_and(|r| r.index.lock().contains(&created))
    }) {
        assert!(std::time::Instant::now() < deadline, "index timed out");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    sidebar.update(cx, |s, cx| s.refresh_sections(cx));
    cx.run_until_parked();
    let in_recents = sidebar.read_with(cx, |s, _| s.recents.iter().any(|e| e.path == created));
    assert!(!in_recents, "the file being named shows only in the tree");

    // Escape keeps the file: it is a recent file like any other.
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.cancel_rename(window, cx)
    });
    let in_recents = sidebar.read_with(cx, |s, _| s.recents.iter().any(|e| e.path == created));
    assert!(in_recents, "once named, the new file is a recent file");
}

#[gpui::test]
fn recents_leave_out_the_active_file_and_keep_their_page_full(cx: &mut TestAppContext) {
    let env = setup(cx);
    for i in 0..12 {
        env.file(&format!("n{i}.md"), "x");
    }
    let (_handle, sidebar) = open(cx, &env);
    wait_for_index(cx, &sidebar);
    let paths = |s: &Sidebar| s.recents.iter().map(|e| e.path.clone()).collect::<Vec<_>>();
    let before = sidebar.read_with(cx, |s, _| paths(s));
    let active = before[0].clone();

    sidebar.update(cx, |s, cx| s.set_active_path(Some(active.clone()), cx));
    cx.run_until_parked();
    let shown = sidebar.read_with(cx, |s, _| paths(s));
    assert!(
        !shown.contains(&active),
        "the active file is not in Recents"
    );
    assert_eq!(shown.len(), 4);
    assert_eq!(shown[..3], before[1..]);

    // Leaving it brings it back.
    sidebar.update(cx, |s, cx| s.set_active_path(None, cx));
    cx.run_until_parked();
    assert_eq!(sidebar.read_with(cx, |s, _| paths(s)), before);
}

#[gpui::test]
fn pinned_files_show_in_pages_of_six(cx: &mut TestAppContext) {
    let env = setup(cx);
    let files: Vec<PathBuf> = (0..8).map(|i| env.file(&format!("p{i}.md"), "x")).collect();
    let (_handle, sidebar) = open(cx, &env);
    for file in &files {
        sidebar.update(cx, |s, cx| s.toggle_pin(file, cx));
    }
    cx.run_until_parked();
    let (count, more) = sidebar.read_with(cx, |s, _| {
        (
            s.pinned_entries.len().min(s.pinned_visible),
            s.pinned_has_more(),
        )
    });
    assert_eq!((count, more), (6, true));
    // Most recently pinned first.
    let first = sidebar.read_with(cx, |s, _| s.pinned_entries[0].path.clone());
    assert_eq!(first, files[7]);
    sidebar.update(cx, |s, cx| s.show_more(Section::Pinned, cx));
    cx.run_until_parked();
    assert!(!sidebar.read_with(cx, |s, _| s.pinned_has_more()));
}

#[gpui::test]
fn shift_and_secondary_select_and_escape_clears(cx: &mut TestAppContext) {
    let env = setup(cx);
    for name in ["a.md", "b.md", "c.md", "d.md"] {
        env.file(name, "x");
    }
    let (handle, sidebar) = open(cx, &env);
    let paths: Vec<PathBuf> = sidebar.read_with(cx, |s, _| {
        s.rows.iter().map(|r| r.entry.path.clone()).collect()
    });
    sidebar.update(cx, |s, cx| {
        s.press_row(&paths[1], false, false, cx);
        s.press_row(&paths[3], true, false, cx);
    });
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection.len()), 3);
    sidebar.update(cx, |s, cx| s.press_row(&paths[0], false, true, cx));
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection.len()), 4);

    handle
        .update(cx, |_, window, cx| {
            let focus = sidebar.read(cx).focus_handle.clone();
            window.focus(&focus, cx)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(sidebar.read_with(cx, |s, _| s.selection.is_empty()));
}

#[gpui::test]
fn the_toggle_key_hides_the_sidebar_and_the_width_is_saved_on_release(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("a.md", "x");
    let (handle, _sidebar) = open(cx, &env);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    handle
        .update(&mut vcx, |view, window, cx| {
            window.focus(&view.focus_handle(cx), cx)
        })
        .unwrap();
    vcx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-\\"
    } else {
        "ctrl-\\"
    });
    vcx.run_until_parked();
    let visible = cx
        .update(|cx| handle.read_with(cx, |v, _| v.sidebar_visible))
        .unwrap();
    assert!(!visible);
    let stored = cx
        .update(|cx| {
            handle.read_with(cx, |v, _| {
                v.settings().get(VISIBLE_KEY).and_then(|x| x.as_bool())
            })
        })
        .unwrap();
    assert_eq!(stored, Some(false));

    handle
        .update(cx, |view, window, cx| {
            view.toggle_sidebar(cx);
            view.begin_resize(cx);
            view.resize_sidebar(10_000.0, window, cx);
            view.end_resize(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let (width, stored) = cx
        .update(|cx| {
            handle.read_with(cx, |v, _| {
                (
                    v.sidebar_width,
                    v.settings()
                        .get(WIDTH_KEY)
                        .and_then(|x| x.as_f64())
                        .map(|w| w as f32),
                )
            })
        })
        .unwrap();
    assert_eq!(width, 420.0, "1200 px window: 35 % is 420");
    assert_eq!(stored, Some(420.0));
}

// ---- US-027: create, rename, move, reveal ----

fn with_window<R>(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    sidebar: &Entity<Sidebar>,
    f: impl FnOnce(&mut Sidebar, &mut gpui::Window, &mut gpui::Context<Sidebar>) -> R,
) -> R {
    let result = handle
        .update(cx, |_, window, cx| {
            sidebar.update(cx, |s, cx| f(s, window, cx))
        })
        .unwrap();
    let vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    result
}

fn entry(
    cx: &mut TestAppContext,
    sidebar: &Entity<Sidebar>,
    path: &Path,
) -> crate::sidebar::tree::Row {
    sidebar.read_with(cx, |s, _| {
        s.rows
            .iter()
            .find(|r| r.entry.path == path)
            .cloned()
            .expect("a visible row")
    })
}

/// A typed `..` or separator is refused in place: a rename never moves
/// the entry to another folder.
#[gpui::test]
fn renaming_refuses_a_name_that_leaves_the_folder(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    env.file("other/x.md", "x");
    let (handle, sidebar) = open(cx, &env);
    for typed in ["other/a", "../a"] {
        with_window(cx, handle, &sidebar, |s, window, cx| {
            s.start_rename(&a, window, cx)
        });
        let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
        field.update(cx, |f, cx| f.set_text(typed, cx));
        with_window(cx, handle, &sidebar, |s, window, cx| {
            s.commit_rename(window, cx)
        });
        let error = sidebar.read_with(cx, |s, _| s.renaming.as_ref().and_then(|r| r.error.clone()));
        assert!(
            error.is_some_and(|e| e.starts_with(&format!("\"{typed}\" cannot be used"))),
            "{typed}"
        );
        assert!(a.exists(), "{typed}");
        with_window(cx, handle, &sidebar, |s, window, cx| {
            s.cancel_rename(window, cx)
        });
    }
    assert!(!env.root().join("other/a.md").exists());
}

#[gpui::test]
fn renaming_edits_the_stem_in_place_and_refuses_a_taken_name(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    let b = env.file("b.md", "b");
    env.file("dir/x.md", "x");
    let (handle, sidebar) = open(cx, &env);
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.start_rename(&a, window, cx)
    });
    let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
    assert_eq!(
        field.read_with(cx, |f, _| f.text.clone()),
        "a",
        "a file shows its stem"
    );

    field.update(cx, |f, cx| f.set_text("b", cx));
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.commit_rename(window, cx)
    });
    let error = sidebar.read_with(cx, |s, _| s.renaming.as_ref().and_then(|r| r.error.clone()));
    assert_eq!(
        error.as_deref(),
        Some("A file named \"b.md\" already exists.")
    );
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "b",
        "the other file is untouched"
    );
    assert!(a.exists());

    // Escape cancels.
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.cancel_rename(window, cx)
    });
    assert!(sidebar.read_with(cx, |s, _| s.renaming.is_none()));

    // A folder shows its full name; Enter renames.
    let dir = env.root().join("dir");
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.start_rename(&dir, window, cx)
    });
    let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
    assert_eq!(field.read_with(cx, |f, _| f.text.clone()), "dir");
    field.update(cx, |f, cx| f.set_text("notes", cx));
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.commit_rename(window, cx)
    });
    assert!(env.root().join("notes/x.md").exists());
    assert!(sidebar.read_with(cx, |s, _| s.renaming.is_none()));
}

#[gpui::test]
fn a_new_file_is_renamed_in_place_unless_the_workspace_changed(cx: &mut TestAppContext) {
    use cerata_core::fs::EntryKind;
    let env = setup(cx);
    env.file("a.md", "a");
    let (handle, sidebar) = open(cx, &env);
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.create_entry(None, EntryKind::File, window, cx)
    });
    let renaming = sidebar.read_with(cx, |s, _| s.renaming.as_ref().map(|r| r.path.clone()));
    let created = renaming.expect("the new file is renamed in place");
    assert!(created.exists());

    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.cancel_rename(window, cx)
    });
    // The workspace changes before the creation finishes: no rename follows.
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.create_entry(None, EntryKind::File, window, cx);
        s.workspace_changed(cx);
    });
    assert!(sidebar.read_with(cx, |s, _| s.renaming.is_none()
        && s.pending_rename.is_none()));
}

#[gpui::test]
fn enter_on_a_new_file_names_its_heading_and_opens_it(cx: &mut TestAppContext) {
    use cerata_core::fs::EntryKind;
    let env = setup(cx);
    env.file("a.md", "a");
    let (handle, sidebar) = open(cx, &env);
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.create_entry(None, EntryKind::File, window, cx)
    });
    let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
    field.update(cx, |f, cx| f.set_text("Test Name", cx));
    handle
        .update(cx, |_, _, cx| {
            field.update(cx, |_, cx| {
                cx.emit(crate::editor::text_field::FieldEvent::Submit { backward: false })
            })
        })
        .unwrap();
    VisualTestContext::from_window(handle.into(), cx).run_until_parked();

    let named = env.root().join("Test Name.md");
    assert_eq!(std::fs::read_to_string(&named).unwrap(), "# Test Name\n\n");
    let (active, caret) = handle
        .read_with(cx, |view, cx| {
            let caret = view
                .docs
                .get(&named)
                .map(|doc| doc.editor.read(cx).cursor());
            (view.tabs.active_path().map(Path::to_path_buf), caret)
        })
        .unwrap();
    assert_eq!(active, Some(named));
    // Ready to write below the heading.
    assert_eq!(caret, Some("# Test Name\n\n".len()));
}

#[gpui::test]
fn renaming_an_existing_file_leaves_its_heading_alone(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "# ");
    let (handle, sidebar) = open(cx, &env);
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.start_rename(&a, window, cx)
    });
    let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
    field.update(cx, |f, cx| f.set_text("b", cx));
    with_window(cx, handle, &sidebar, |s, window, cx| {
        s.commit_rename(window, cx)
    });
    assert_eq!(
        std::fs::read_to_string(env.root().join("b.md")).unwrap(),
        "# "
    );
}

#[gpui::test]
fn a_drag_starts_after_4_px_and_never_drops_a_folder_into_itself(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "a");
    env.file("f/sub/x.md", "x");
    let (handle, sidebar) = open(cx, &env);
    let f = env.root().join("f");
    let sub = f.join("sub");
    sidebar.update(cx, |s, cx| s.toggle_dir(&f, cx));
    cx.run_until_parked();
    let sub_entry = entry(cx, &sidebar, &sub).entry;
    let f_entry = entry(cx, &sidebar, &f).entry;
    let start = gpui::point(px(10.0), px(10.0));

    // 3 px is still a click.
    sidebar.update(cx, |s, cx| {
        s.begin_drag(&f, start);
        s.drag_moved(
            start + gpui::point(px(3.0), px(0.0)),
            Some(sub_entry.clone()),
            cx,
        );
    });
    assert!(sidebar.read_with(cx, |s, _| !s.drag.as_ref().unwrap().active));
    // Into its own descendant: refused, nothing moves.
    sidebar.update(cx, |s, cx| {
        s.drag_moved(
            start + gpui::point(px(5.0), px(0.0)),
            Some(sub_entry.clone()),
            cx,
        );
        assert!(s.drag.as_ref().unwrap().active);
        s.end_drag(cx);
    });
    cx.run_until_parked();
    assert!(f.join("sub/x.md").exists());
    assert!(!sub.join("f").exists());

    // A file dropped on a folder moves into it, and its tab follows.
    handle
        .update(cx, |view, window, cx| view.open_file(&a, window, cx))
        .unwrap();
    cx.run_until_parked();
    sidebar.update(cx, |s, cx| {
        s.begin_drag(&a, start);
        s.drag_moved(
            start + gpui::point(px(0.0), px(8.0)),
            Some(f_entry.clone()),
            cx,
        );
        s.end_drag(cx);
    });
    cx.run_until_parked();
    let moved = f.join("a.md");
    assert!(moved.exists() && !a.exists());
    let active = cx
        .update(|cx| handle.read_with(cx, |v, _| v.tabs.active_path().map(Path::to_path_buf)))
        .unwrap();
    assert_eq!(active, Some(moved));
}

#[gpui::test]
fn reveal_expands_the_parents_and_a_newer_call_wins(cx: &mut TestAppContext) {
    let env = setup(cx);
    let deep = env.file("one/two/deep.md", "x");
    let other = env.file("three/other.md", "x");
    let (_handle, sidebar) = open(cx, &env);
    sidebar.update(cx, |s, cx| {
        s.reveal_in_sidebar(&deep, cx);
        s.reveal_in_sidebar(&other, cx);
    });
    cx.run_until_parked();
    let (deep_shown, anchor, pending) = sidebar.read_with(cx, |s, _| {
        (
            s.rows.iter().any(|r| r.entry.path == deep),
            s.selection.anchor.clone(),
            s.revealing.clone(),
        )
    });
    assert!(deep_shown, "the parents of the first reveal are expanded");
    assert_eq!(anchor, Some(other), "the newer reveal lands on its row");
    assert_eq!(
        pending, None,
        "the older one was canceled, not left waiting"
    );
}

// ---- EP-004: Writer store invariants ----

/// Opens a window on `root`, a folder below the temp dir.
fn open_at(cx: &mut TestAppContext, root: &Path) -> (WindowHandle<WindowView>, Entity<Sidebar>) {
    let target = OpenTarget::Workspace(root.to_path_buf());
    let handle = cx
        .update(|cx| app::open_window(cx, Some(target), None))
        .unwrap();
    cx.run_until_parked();
    let sidebar = cx
        .update(|cx| handle.read_with(cx, |view, _| view.sidebar.clone()))
        .unwrap()
        .expect("a workspace window has a sidebar");
    (handle, sidebar)
}

fn listed_under(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>, prefix: &Path) -> bool {
    sidebar.read_with(cx, |s, _| {
        s.listings.keys().any(|dir| dir.starts_with(prefix))
    })
}

fn rename(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    sidebar: &Entity<Sidebar>,
    path: &Path,
    name: &str,
) {
    with_window(cx, handle, sidebar, |s, window, cx| {
        s.start_rename(path, window, cx)
    });
    let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
    field.update(cx, |f, cx| f.set_text(name, cx));
    with_window(cx, handle, sidebar, |s, window, cx| {
        s.commit_rename(window, cx)
    });
}

/// Ports Writer `stores.test.ts:111`: a folder read that finishes after a
/// workspace switch never reaches the tree.
#[gpui::test]
fn a_folder_read_from_the_previous_workspace_is_dropped(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("one/dir/inner.md", "x");
    env.file("two/b.md", "b");
    let (one, two) = (env.root().join("one"), env.root().join("two"));
    let (handle, sidebar) = open_at(cx, &one);
    let dir = one.join("dir");
    handle
        .update(cx, |view, window, cx| {
            sidebar.update(cx, |s, cx| s.load_dir(&dir, cx));
            view.open_workspace(&two, None, window, cx).unwrap();
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!listed_under(cx, &sidebar, &one), "the old read is dropped");
    assert_eq!(
        sidebar.read_with(cx, |s, _| s.root().map(Path::to_path_buf)),
        Some(two)
    );
    assert_eq!(
        row_names(cx, &sidebar),
        [(tree::UNFILED.into(), 0), ("b.md".into(), 1)]
    );
}

/// Ports Writer `stores.test.ts:130`: a read started before the same root
/// was closed and reopened (A, B, then A again) is thrown away.
#[gpui::test]
fn a_read_from_an_earlier_open_of_the_same_root_is_dropped(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("one/dir/inner.md", "x");
    env.file("two/b.md", "b");
    let (one, two) = (env.root().join("one"), env.root().join("two"));
    let (handle, sidebar) = open_at(cx, &one);
    let dir = one.join("dir");
    handle
        .update(cx, |view, window, cx| {
            sidebar.update(cx, |s, cx| s.load_dir(&dir, cx));
            view.open_workspace(&two, None, window, cx).unwrap();
            view.open_workspace(&one, None, window, cx).unwrap();
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        !listed_under(cx, &sidebar, &dir),
        "the read of the first open does not land in the second"
    );
    assert_eq!(row_names(cx, &sidebar), [("dir".into(), 0)]);
    let expanded = sidebar.read_with(cx, |s, _| s.rows[0].expanded);
    assert!(!expanded);
}

/// Ports Writer `stores.test.ts:773` and `:783`: renaming an expanded
/// folder rewrites it and its expanded subfolders, and their children are
/// listed under the new paths.
#[gpui::test]
fn renaming_an_expanded_folder_keeps_it_and_its_subfolders_open(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("dir/top.md", "t");
    env.file("dir/sub/deep.md", "d");
    let (handle, sidebar) = open(cx, &env);
    wait_for_index(cx, &sidebar);
    let (dir, sub) = (env.root().join("dir"), env.root().join("dir/sub"));
    sidebar.update(cx, |s, cx| s.toggle_dir(&dir, cx));
    cx.run_until_parked();
    sidebar.update(cx, |s, cx| s.toggle_dir(&sub, cx));
    cx.run_until_parked();

    rename(cx, handle, &sidebar, &dir, "renamed");
    let renamed = env.root().join("renamed");
    let expanded = sidebar.read_with(cx, |s, _| s.expanded.clone());
    assert_eq!(
        expanded,
        HashSet::from([renamed.clone(), renamed.join("sub")])
    );
    assert!(
        !listed_under(cx, &sidebar, &dir),
        "no listing under the old path"
    );
    // The index learns the new paths from the watcher, whose events
    // re-read the folders shown.
    let shown = [
        ("renamed".into(), 0),
        ("sub".into(), 1),
        ("deep.md".into(), 2),
        ("top.md".into(), 1),
    ];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while row_names(cx, &sidebar) != shown {
        assert!(
            std::time::Instant::now() < deadline,
            "the children still show: {:?}",
            row_names(cx, &sidebar)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.update(app::pump_events);
        cx.run_until_parked();
    }
    let deep = entry(cx, &sidebar, &renamed.join("sub/deep.md"));
    assert_eq!(deep.depth, 2);
}

/// Ports Writer `stores.test.ts:795`: renaming a folder that is not
/// expanded leaves the expanded set alone.
#[gpui::test]
fn renaming_a_collapsed_folder_leaves_the_expanded_set_alone(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("dir/a.md", "a");
    env.file("other/b.md", "b");
    let (handle, sidebar) = open(cx, &env);
    let other = env.root().join("other");
    sidebar.update(cx, |s, cx| s.toggle_dir(&other, cx));
    cx.run_until_parked();
    rename(cx, handle, &sidebar, &env.root().join("dir"), "renamed");
    assert!(env.root().join("renamed/a.md").exists());
    let expanded = sidebar.read_with(cx, |s, _| s.expanded.clone());
    assert_eq!(expanded, HashSet::from([other]));
}

#[gpui::test]
fn a_huge_folder_shows_before_its_titles_then_fills_them_in(cx: &mut TestAppContext) {
    let env = setup(cx);
    let count = TITLE_CHUNK + 1;
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    for i in 0..count {
        let path = env.file(&format!("n{i:04}.md"), &format!("# Title {i}"));
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(old).unwrap();
    }
    let (_handle, sidebar) = open(cx, &env);
    let root = env.root();
    let runtime = sidebar.read_with(cx, |s, _| s.runtime().unwrap().clone());

    // Past a chunk of unknown titles, the read leaves them for later...
    let (entries, untitled) = read_listing(&runtime, &root, &HashMap::new()).unwrap();
    assert_eq!(untitled.len(), count);
    assert!(entries.iter().all(|entry| entry.title.is_none()));

    // ...and the window fills them in once the folder is shown.
    let titled = sidebar.read_with(cx, |s, _| {
        s.rows
            .iter()
            .filter(|row| row.entry.title.is_some())
            .count()
    });
    assert_eq!(titled, count);

    // A re-read keeps the titles of unchanged files without reading them.
    let first = root.join("n0000.md");
    std::fs::write(&first, "# Changed").unwrap();
    let file = std::fs::File::options().write(true).open(&first).unwrap();
    file.set_modified(old).unwrap();
    let known = sidebar.read_with(cx, |s, _| known_titles(Some(&s.root_listing)));
    let (entries, untitled) = read_listing(&runtime, &root, &known).unwrap();
    assert!(untitled.is_empty());
    let kept = entries.iter().find(|entry| entry.path == first).unwrap();
    assert_eq!(kept.title.as_deref(), Some("Title 0"));
}

// ---- Sidebar filter ----

fn filter_names(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>) -> Vec<String> {
    sidebar.read_with(cx, |s, _| {
        s.filter
            .results
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    })
}

#[gpui::test]
fn the_filter_matches_titles_and_paths_of_every_indexed_file(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("notes/inner.md", "# Moteur d'édition");
    env.file("a.md", "no title");
    env.file("b.md", "# Beta");
    let (_handle, sidebar) = open(cx, &env);
    wait_for_index(cx, &sidebar);

    // A collapsed folder's file is found by its title, in any case.
    sidebar.update(cx, |s, cx| s.set_filter("  MOTEUR ", cx));
    cx.run_until_parked();
    assert_eq!(filter_names(cx, &sidebar), ["inner.md"]);
    // A folder name matches through the relative path.
    sidebar.update(cx, |s, cx| s.set_filter("notes", cx));
    assert_eq!(filter_names(cx, &sidebar), ["inner.md"]);
    sidebar.update(cx, |s, cx| s.set_filter("zzz", cx));
    assert!(filter_names(cx, &sidebar).is_empty());
    assert!(sidebar.read_with(cx, |s, _| s.filtering()));
    sidebar.update(cx, |s, cx| s.set_filter("", cx));
    assert!(!sidebar.read_with(cx, |s, _| s.filtering()));
}

#[gpui::test]
fn typing_in_the_search_field_filters_and_escape_clears(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("a.md", "no title");
    env.file("b.md", "# Beta");
    let (handle, sidebar) = open(cx, &env);
    wait_for_index(cx, &sidebar);
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    handle
        .update(&mut vcx, |view, window, cx| {
            let focus = view.sidebar_filter.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
        })
        .unwrap();
    vcx.simulate_input("bet");
    vcx.run_until_parked();
    assert_eq!(filter_names(&mut vcx, &sidebar), ["b.md"]);

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!sidebar.read_with(&vcx, |s, _| s.filtering()));
    let text = handle
        .read_with(&vcx, |view, cx| view.sidebar_filter.read(cx).text.clone())
        .unwrap();
    assert_eq!(text, "");
}

// ---- US-072: the assets folder follows its note ----

/// `foo.md` linking `foo-assets/x.png`, in `dir` ("" for the root).
fn note_with_assets(env: &Env, dir: &str) -> PathBuf {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    env.file(&format!("{prefix}foo-assets/x.png"), "x");
    env.file(&format!("{prefix}foo.md"), "![x](foo-assets/x.png)\n")
}

fn rename_to(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    sidebar: &Entity<Sidebar>,
    path: &Path,
    typed: &str,
) {
    with_window(cx, handle, sidebar, |s, window, cx| {
        s.start_rename(path, window, cx)
    });
    let field = sidebar.read_with(cx, |s, _| s.renaming.as_ref().unwrap().field.clone());
    field.update(cx, |f, cx| f.set_text(typed, cx));
    with_window(cx, handle, sidebar, |s, window, cx| {
        s.commit_rename(window, cx)
    });
}

fn doc_text(cx: &mut TestAppContext, handle: WindowHandle<WindowView>, path: &Path) -> String {
    cx.update(|cx| {
        handle.read_with(cx, |view, cx| {
            view.docs
                .get(path)
                .map(|doc| doc.editor.read(cx).state.doc().to_owned())
        })
    })
    .unwrap()
    .unwrap_or_default()
}

fn shell_notice(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) -> Option<String> {
    cx.update(|cx| handle.read_with(cx, |view, _| view.notice.as_ref().map(|n| n.to_string())))
        .unwrap()
}

#[gpui::test]
fn renaming_an_open_edited_note_rewrites_its_buffer_and_takes_its_assets(cx: &mut TestAppContext) {
    use cerata_core::watcher::{FileChange, WatchEvent};
    let env = setup(cx);
    let foo = note_with_assets(&env, "");
    let (handle, sidebar) = open(cx, &env);
    handle
        .update(cx, |view, window, cx| view.open_file(&foo, window, cx))
        .unwrap();
    cx.run_until_parked();
    let editor = cx
        .update(|cx| handle.read_with(cx, |v, _| v.docs.get(&foo).unwrap().editor.clone()))
        .unwrap();
    editor.update(cx, |editor, cx| {
        let end = editor.state.text().len();
        editor.update_state(cx, |state| {
            let _ = state.apply(
                cerata_markdown::Transaction::new()
                    .change(cerata_markdown::Change::insert(end, "unsaved"))
                    .event(cerata_markdown::UserEvent::Type),
            );
        });
    });

    rename_to(cx, handle, &sidebar, &foo, "bar");
    let bar = env.root().join("bar.md");
    assert!(env.root().join("bar-assets/x.png").is_file());
    assert!(!env.root().join("foo-assets").exists(), "no orphan folder");
    assert_eq!(
        doc_text(cx, handle, &bar),
        "![x](bar-assets/x.png)\nunsaved",
        "the buffer is rewritten, its unsaved text kept"
    );

    // The rename's own watcher events neither reload nor conflict. The
    // watcher drops an event whose content Cerata just wrote; it is fed the
    // others, as it would be.
    let echo = sidebar.read_with(cx, |s, _| {
        s.runtime().unwrap().self_writes.is_self_write(&bar)
    });
    let events = [(&bar, FileChange::Created), (&foo, FileChange::Removed)];
    for (path, change) in events.into_iter().filter(|(p, _)| !(echo && *p == &bar)) {
        handle
            .update(cx, |view, window, cx| {
                let path = path.to_path_buf();
                view.on_watch_event(WatchEvent::FileChanged { path, change }, window, cx)
            })
            .unwrap();
    }
    cx.run_until_parked();
    let conflicted = cx
        .update(|cx| handle.read_with(cx, |v, _| v.docs.get(&bar).unwrap().conflict.clone()))
        .unwrap();
    assert_eq!(conflicted, None);
    assert_eq!(
        doc_text(cx, handle, &bar),
        "![x](bar-assets/x.png)\nunsaved"
    );

    // The autosave writes the rewrite.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::fs::read_to_string(&bar).is_ok_and(|t| t.contains("bar-assets")) {
        assert!(std::time::Instant::now() < deadline, "never saved");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = handle.update(cx, |view, _, cx| view.drain_saves(cx));
        cx.run_until_parked();
    }

    // One undo step takes the rewrite back, and only it.
    editor.update(cx, |editor, cx| {
        editor.update_state(cx, |state| state.undo());
    });
    assert_eq!(
        doc_text(cx, handle, &bar),
        "![x](foo-assets/x.png)\nunsaved"
    );
}

#[gpui::test]
fn renaming_a_closed_note_rewrites_it_on_disk_as_a_self_write(cx: &mut TestAppContext) {
    let env = setup(cx);
    let foo = note_with_assets(&env, "");
    env.file("other.md", "![o](foo-assets/x.png)\n");
    let (handle, sidebar) = open(cx, &env);
    rename_to(cx, handle, &sidebar, &foo, "bar");
    let bar = env.root().join("bar.md");
    assert_eq!(
        std::fs::read_to_string(&bar).unwrap(),
        "![x](bar-assets/x.png)\n"
    );
    assert!(env.root().join("bar-assets/x.png").is_file());
    assert_eq!(
        std::fs::read_to_string(env.root().join("other.md")).unwrap(),
        "![o](foo-assets/x.png)\n",
        "only the renamed note is rewritten"
    );
    let echo = sidebar.read_with(cx, |s, _| {
        s.runtime().unwrap().self_writes.is_self_write(&bar)
    });
    assert!(echo, "the watcher knows the write is Cerata's");
    assert_eq!(shell_notice(cx, handle), None);
}

#[gpui::test]
fn a_note_without_assets_renames_as_before(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "![x](a-assets/x.png)\n");
    let (handle, sidebar) = open(cx, &env);
    rename_to(cx, handle, &sidebar, &a, "b");
    assert_eq!(
        std::fs::read_to_string(env.root().join("b.md")).unwrap(),
        "![x](a-assets/x.png)\n"
    );
    assert!(!env.root().join("b-assets").exists());
}

#[gpui::test]
fn a_taken_folder_name_keeps_the_folder_and_says_so(cx: &mut TestAppContext) {
    let env = setup(cx);
    let foo = note_with_assets(&env, "");
    env.file("bar-assets/y.png", "y");
    let (handle, sidebar) = open(cx, &env);
    rename_to(cx, handle, &sidebar, &foo, "bar");
    assert!(env.root().join("bar.md").is_file());
    assert!(
        env.root().join("foo-assets/x.png").is_file(),
        "the folder stays"
    );
    assert_eq!(
        std::fs::read_to_string(env.root().join("bar.md")).unwrap(),
        "![x](foo-assets/x.png)\n",
        "still a sibling: the links stay valid"
    );
    assert_eq!(
        shell_notice(cx, handle).as_deref(),
        Some("Images stay in \"foo-assets\": \"bar-assets\" already exists")
    );
}

fn drag_onto(cx: &mut TestAppContext, sidebar: &Entity<Sidebar>, source: &Path, dir: &Path) {
    sidebar.update(cx, |s, cx| s.toggle_dir(dir, cx));
    cx.run_until_parked();
    let target = entry(cx, sidebar, dir).entry;
    let start = gpui::point(px(10.0), px(10.0));
    sidebar.update(cx, |s, cx| {
        s.begin_drag(source, start);
        s.drag_moved(start + gpui::point(px(0.0), px(8.0)), Some(target), cx);
        s.end_drag(cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn a_dragged_note_takes_its_assets_and_keeps_its_text(cx: &mut TestAppContext) {
    let env = setup(cx);
    let foo = note_with_assets(&env, "a");
    env.file("b/keep.md", "k");
    let (handle, sidebar) = open(cx, &env);
    let b = env.root().join("b");
    drag_onto(cx, &sidebar, &foo, &b);
    assert_eq!(
        std::fs::read_to_string(b.join("foo.md")).unwrap(),
        "![x](foo-assets/x.png)\n"
    );
    assert!(b.join("foo-assets/x.png").is_file());
    assert!(
        !env.root().join("a/foo-assets").exists(),
        "no orphan folder"
    );
    assert_eq!(shell_notice(cx, handle), None);
}

#[gpui::test]
fn a_note_dragged_over_a_taken_folder_points_its_links_back(cx: &mut TestAppContext) {
    let env = setup(cx);
    let foo = note_with_assets(&env, "a");
    env.file("b/foo-assets/other.png", "o");
    // A folder shows in the sidebar once it holds a note.
    env.file("b/keep.md", "k");
    let (handle, sidebar) = open(cx, &env);
    let b = env.root().join("b");
    drag_onto(cx, &sidebar, &foo, &b);
    assert_eq!(
        std::fs::read_to_string(b.join("foo.md")).unwrap(),
        "![x](../a/foo-assets/x.png)\n"
    );
    assert!(env.root().join("a/foo-assets/x.png").is_file());
    assert_eq!(
        shell_notice(cx, handle).as_deref(),
        Some("Images stay in \"foo-assets\": \"foo-assets\" already exists")
    );
}
