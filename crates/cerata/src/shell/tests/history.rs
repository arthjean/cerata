use cerata_core::fs::text::TextFormat;
use cerata_core::history::{self, Cause, VersionInfo};
use cerata_core::merge::Choice;
use cerata_core::reconcile::ReconcileTrigger;

use super::*;
use crate::history_panel::View;
use crate::merge_review::Source;
use crate::shell::history::{MERGED, RESOLVED};

fn wait_for_history(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) {
    wait_until(cx, |cx| {
        read(cx, handle, |view, _| {
            view.history().is_some()
                && view
                    .core
                    .runtime()
                    .is_some_and(|runtime| runtime.store.read().is_some())
        })
    });
}

fn store(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) -> cerata_core::store::Store {
    read(cx, handle, |view, _| {
        view.core.runtime().unwrap().store.read().clone().unwrap()
    })
}

fn versions(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    path: &Path,
) -> Vec<VersionInfo> {
    if let Some(history) = read(cx, handle, |view, _| view.history()) {
        history.flush_and_wait(None, Duration::from_secs(5));
    }
    let relative = read(cx, handle, |view, _| {
        view.core.runtime().unwrap().relative(path).unwrap()
    });
    history::versions_of_path(&store(cx, handle), &relative)
        .unwrap()
        .map_or_else(Vec::new, |(_, versions)| versions)
}

fn text_of(cx: &mut TestAppContext, handle: WindowHandle<WindowView>, v: &VersionInfo) -> String {
    let bytes = history::content(&store(cx, handle), &v.hash)
        .unwrap()
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

fn causes(versions: &[VersionInfo]) -> Vec<Cause> {
    versions.iter().map(|v| v.cause).collect()
}

fn open_panel(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    path: Option<PathBuf>,
    view: View,
) {
    update(cx, handle, move |this, window, cx| {
        this.show_history(path, view, window, cx)
    });
    wait_until(cx, |cx| {
        read(cx, handle, |view, cx| {
            view.history_panel.as_ref().is_some_and(|panel| {
                let panel = panel.read(cx);
                !panel.loading && (panel.selected.is_none() || panel.preview.is_some())
            })
        })
    });
}

fn undo(cx: &mut TestAppContext, handle: WindowHandle<WindowView>) {
    update(cx, handle, |view, _, cx| {
        let editor = view.active_doc().unwrap().editor.clone();
        editor.update(cx, |editor, cx| {
            editor.update_state(cx, |state| state.undo())
        });
    });
}

#[gpui::test]
fn restoring_a_version_is_one_undo_step(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "first draft\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    type_text(cx, handle, "second");
    let before = editor_text(cx, handle);
    assert_eq!(before, "first draft\nsecond");

    open_panel(cx, handle, None, View::Note);
    let listed = read(cx, handle, |view, cx| {
        view.history_panel.as_ref().unwrap().read(cx).versions.len()
    });
    assert!(listed >= 1, "the pre-edit state is listed");

    update(cx, handle, |view, _, cx| {
        let panel = view.history_panel.clone().unwrap();
        panel.update(cx, |panel, cx| panel.select(Some(listed - 1), cx));
    });
    wait_until(cx, |cx| {
        read(cx, handle, |view, cx| {
            view.history_panel.as_ref().unwrap().read(cx).preview
                == Some(Ok("first draft\n".to_owned()))
        })
    });
    let chosen = read(cx, handle, |view, cx| {
        let panel = view.history_panel.as_ref().unwrap().read(cx);
        panel.selected_version().unwrap().seq
    });
    update(cx, handle, |view, _, cx| {
        let panel = view.history_panel.clone().unwrap();
        panel.update(cx, |panel, cx| panel.restore(cx));
    });
    assert_eq!(editor_text(cx, handle), "first draft\n");
    wait_until(cx, |cx| {
        read(cx, handle, |view, cx| {
            let panel = view.history_panel.as_ref().unwrap().read(cx);
            !panel.loading && panel.versions.len() > listed
        })
    });
    let selected = read(cx, handle, |view, cx| {
        let panel = view.history_panel.as_ref().unwrap().read(cx);
        panel.selected_version().map(|v| v.seq)
    });
    assert_eq!(selected, Some(chosen));

    let saved = versions(cx, handle, &a);
    assert_eq!(saved[0].cause, Cause::Restore);
    assert_eq!(text_of(cx, handle, &saved[0]), "first draft\n");
    assert!(
        saved
            .iter()
            .any(|v| v.cause == Cause::Save && text_of(cx, handle, v) == before),
        "the state before the restore is a version: {:?}",
        causes(&saved)
    );

    undo(cx, handle);
    assert_eq!(editor_text(cx, handle), before, "one Ctrl+Z undoes it");
}

#[gpui::test]
fn a_deleted_note_restores_beside_a_taken_path(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "kept words\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    wait_for_index(cx, handle);
    update(cx, handle, |view, _, _| {
        view.record_version(&a, "kept words\n", TextFormat::default(), Cause::Save)
    });
    assert_eq!(versions(cx, handle, &a).len(), 1);

    std::fs::remove_file(&a).unwrap();
    let store_handle = store(cx, handle);
    wait_until(cx, |cx| {
        read(cx, handle, |view, _| {
            view.reconcile(ReconcileTrigger::Focus)
        });
        history::deleted_notes(&store_handle).is_ok_and(|deleted| deleted.len() == 1)
    });
    env.file("a.md", "a new note\n");

    open_panel(cx, handle, None, View::Deleted);
    let deleted = read(cx, handle, |view, cx| {
        view.history_panel.as_ref().unwrap().read(cx).deleted.len()
    });
    assert_eq!(deleted, 1);
    update(cx, handle, |view, _, cx| {
        let panel = view.history_panel.clone().unwrap();
        panel.update(cx, |panel, cx| panel.restore(cx));
    });
    let restored = env.root().join("a (restored).md");
    wait_until(cx, |_| restored.is_file());
    assert_eq!(std::fs::read_to_string(&restored).unwrap(), "kept words\n");
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "a new note\n",
        "not overwritten"
    );
    assert_eq!(
        notice_text(cx, handle).as_deref(),
        Some("Restored a (restored).md")
    );
    let kept = versions(cx, handle, &restored);
    assert!(
        kept.iter().any(|v| v.cause == Cause::Save)
            && kept.iter().any(|v| v.cause == Cause::Restore),
        "the note keeps its history: {:?}",
        causes(&kept)
    );
}

#[gpui::test]
fn the_panel_lists_1000_versions_within_200_ms(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "v\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    let (history, relative) = read(cx, handle, |view, _| {
        let runtime = view.core.runtime().unwrap();
        (view.history().unwrap(), runtime.relative(&a).unwrap())
    });
    for n in 0..1000 {
        let mut request = history::Request::new(
            relative.clone(),
            format!("line {n}\nshared tail of the note\n").into_bytes(),
            Cause::Save,
        );
        request.ts = 1_700_000_000 + n * 60;
        history.record(request);
    }
    assert!(history.flush_and_wait(None, Duration::from_secs(60)));
    open_panel(cx, handle, Some(a.clone()), View::Note);
    let (count, took) = read(cx, handle, |view, cx| {
        let panel = view.history_panel.as_ref().unwrap().read(cx);
        (panel.versions.len(), panel.load_time.unwrap())
    });
    assert_eq!(count, 1000);
    assert!(took <= Duration::from_millis(200), "listed in {took:?}");
}

#[gpui::test]
fn copy_pin_and_delete_before_act_from_the_panel(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "today\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    let (history, relative) = read(cx, handle, |view, _| {
        let runtime = view.core.runtime().unwrap();
        (view.history().unwrap(), runtime.relative(&a).unwrap())
    });
    for (text, ts) in [("old\n", 1_000_000_000), ("today\n", history::unix_now())] {
        let mut request = history::Request::new(relative.clone(), text.into(), Cause::Save);
        request.ts = ts;
        history.record(request);
    }
    assert!(history.flush_and_wait(None, Duration::from_secs(5)));
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    open_panel(cx, handle, None, View::Note);
    let panel = read(cx, handle, |view, _| view.history_panel.clone().unwrap());

    update(cx, handle, |_, _, cx| {
        panel.update(cx, |panel, cx| panel.copy(cx))
    });
    let copied = read(cx, handle, |_, cx| {
        cx.read_from_clipboard().and_then(|item| item.text())
    });
    assert_eq!(copied.as_deref(), Some("today\n"));

    for pinned in [true, false] {
        update(cx, handle, |_, _, cx| {
            panel.update(cx, |panel, cx| panel.toggle_pin(cx))
        });
        wait_until(cx, |cx| {
            read(cx, handle, |_, cx| {
                let panel = panel.read(cx);
                !panel.loading && panel.selected_version().is_some_and(|v| v.pinned == pinned)
            })
        });
    }

    update(cx, handle, |_, window, cx| {
        panel.update(cx, |panel, cx| {
            panel.ask_date(window, cx);
            let field = panel.date_field.clone().unwrap();
            field.update(cx, |field, cx| field.set_text("2020-01-01", cx));
            panel.submit_date(cx);
        })
    });
    wait_until(cx, |cx| {
        VisualTestContext::from_window(handle.into(), cx).has_pending_prompt()
    });
    VisualTestContext::from_window(handle.into(), cx).simulate_prompt_answer("Delete");
    wait_until(cx, |cx| {
        notice_text(cx, handle).as_deref() == Some("Deleted 1 version")
    });
    let left: Vec<String> = versions(cx, handle, &a)
        .iter()
        .map(|v| text_of(cx, handle, v))
        .collect();
    assert_eq!(left, ["today\n"]);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "today\n");
}

#[gpui::test]
fn the_panel_says_why_history_is_unavailable(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "text\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    let reason = "History is unavailable: Cerata keeps no history in a folder synced by OneDrive.";
    update(cx, handle, |view, window, cx| {
        let runtime = view.core.runtime().unwrap();
        *runtime.store.write() = None;
        *runtime.store_unavailable.write() = Some(reason.to_owned());
        view.open_file(&a, window, cx);
    });
    open_panel(cx, handle, None, View::Note);
    let (available, versions) = read(cx, handle, |view, cx| {
        let panel = view.history_panel.as_ref().unwrap().read(cx);
        (panel.is_available(), panel.versions.len())
    });
    assert!(!available, "the footer and \"Deleted Notes\" are off");
    assert_eq!(versions, 0);
    assert_eq!(
        read(cx, handle, |view, cx| view
            .history_panel
            .as_ref()
            .unwrap()
            .read(cx)
            .unavailable
            .clone()),
        Some(reason.to_owned())
    );
}

#[gpui::test]
fn a_note_without_versions_has_an_empty_history(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "untouched\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    open_panel(cx, handle, None, View::Note);
    let (empty, available) = read(cx, handle, |view, cx| {
        let panel = view.history_panel.as_ref().unwrap().read(cx);
        (panel.versions.is_empty(), panel.is_available())
    });
    assert!(empty && available);
    let commands = read(cx, handle, |view, _| view.availability());
    let offered: Vec<_> = crate::palette::HistoryCommand::ALL
        .into_iter()
        .filter(|command| command.available(commands))
        .map(crate::palette::HistoryCommand::label)
        .collect();
    assert!(offered.contains(&"History: Restore Version"), "{offered:?}");
    assert!(offered.contains(&"History: Delete History Before\u{2026}"));
    keys(cx, handle, "escape");
    assert!(read(cx, handle, |view, _| view.history_panel.is_none()));
}

fn change_outside(
    cx: &mut TestAppContext,
    handle: WindowHandle<WindowView>,
    a: &Path,
    theirs: &str,
) {
    make_dirty(cx, handle, a);
    std::fs::write(a, theirs).unwrap();
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(a, FileChange::Modified), window, cx)
    });
}

#[gpui::test]
fn disjoint_outside_changes_merge_into_the_buffer(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "alpha\n\nbeta\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    change_outside(cx, handle, &a, "ALPHA\n\nbeta\n mine\n");
    assert_eq!(editor_text(cx, handle), "ALPHA\n\nbeta\n mine more");
    assert_eq!(conflict(cx, handle), None);
    assert_eq!(notice_text(cx, handle).as_deref(), Some(MERGED));
    wait_until(cx, |_| {
        std::fs::read_to_string(&a).is_ok_and(|text| text.trim_end() == "ALPHA\n\nbeta\n mine more")
    });
    let saved = versions(cx, handle, &a);
    let external = saved.iter().find(|v| v.cause == Cause::External).unwrap();
    assert_eq!(text_of(cx, handle, external), "ALPHA\n\nbeta\n mine\n");
    assert!(saved.iter().any(|v| v.cause == Cause::Merge));

    undo(cx, handle);
    assert_eq!(editor_text(cx, handle), "alpha\n\nbeta\n mine more");
}

#[gpui::test]
fn overlapping_changes_wait_for_the_review(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    change_outside(cx, handle, &a, "theirs");
    assert_eq!(editor_text(cx, handle), "base mine more", "not modified");
    assert!(matches!(
        conflict(cx, handle),
        Some(Conflict::Modified { .. })
    ));

    update(cx, handle, |view, window, cx| {
        view.review_changes(window, cx)
    });
    update(cx, handle, |view, _, cx| {
        let review = view.merge_review.clone().unwrap();
        review.update(cx, |review, cx| {
            assert_eq!(review.source, Source::Disk);
            assert!(!review.ready());
            for n in 0..review.choices.len() {
                review.choose(n, Choice::Both, cx);
            }
            review.apply(cx);
        });
    });
    let text = editor_text(cx, handle);
    assert!(
        text.contains("base mine more") && text.contains("theirs"),
        "{text:?}"
    );
    assert_eq!(conflict(cx, handle), None);
    assert!(read(cx, handle, |view, _| view.merge_review.is_none()));
    wait_until(cx, |_| {
        std::fs::read_to_string(&a).is_ok_and(|disk| disk.trim_end() == text.trim_end())
    });
    let saved = versions(cx, handle, &a);
    assert!(
        saved
            .iter()
            .any(|v| v.cause == Cause::External && text_of(cx, handle, v) == "theirs")
    );
    assert!(
        saved.iter().any(
            |v| v.cause == Cause::Merge && text_of(cx, handle, v).trim_end() == text.trim_end()
        )
    );
}

#[gpui::test]
fn quitting_during_a_conflict_loses_neither_side(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "base");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    update(cx, handle, |view, window, cx| {
        view.open_file(&a, window, cx)
    });
    change_outside(cx, handle, &a, "theirs");
    let saved = versions(cx, handle, &a);
    assert!(
        saved
            .iter()
            .any(|v| v.cause == Cause::External && text_of(cx, handle, v) == "theirs")
    );

    update(cx, handle, |view, _, cx| view.before_close(cx));
    assert_eq!(
        std::fs::read_to_string(&a).unwrap().trim_end(),
        "base mine more"
    );
    let saved = versions(cx, handle, &a);
    let texts: Vec<String> = saved.iter().map(|v| text_of(cx, handle, v)).collect();
    assert!(texts.iter().any(|t| t == "theirs"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.trim_end() == "base mine more"),
        "{texts:?}"
    );
}

const COPY: &str = "a.sync-conflict-20261103-101010-ABCDEFG.md";

fn copy_arrives(
    cx: &mut TestAppContext,
    env: &Env,
    handle: WindowHandle<WindowView>,
    a: &Path,
    theirs: &str,
) -> PathBuf {
    update(cx, handle, |view, window, cx| view.open_file(a, window, cx));
    make_dirty(cx, handle, a);
    let copy = env.file(COPY, theirs);
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&copy, FileChange::Created), window, cx)
    });
    wait_until(cx, |cx| {
        read(cx, handle, |view, cx| {
            view.sidebar
                .as_ref()
                .is_some_and(|sidebar| sidebar.read(cx).conflicted.contains(a))
        })
    });
    copy
}

#[gpui::test]
fn a_conflict_copy_is_reviewed_and_trashed(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "one\ntwo\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    wait_for_index(cx, handle);
    let copy = copy_arrives(cx, &env, handle, &a, "ONE\ntwo\n");
    wait_until(cx, |cx| {
        versions(cx, handle, &a)
            .iter()
            .any(|v| v.cause == Cause::Conflict)
    });
    let conflict_version = versions(cx, handle, &a)
        .into_iter()
        .find(|v| v.cause == Cause::Conflict)
        .unwrap();
    assert_eq!(text_of(cx, handle, &conflict_version), "ONE\ntwo\n");

    update(cx, handle, |view, window, cx| {
        view.review_conflict(&a, window, cx)
    });
    wait_until(cx, |cx| {
        read(cx, handle, |view, _| view.merge_review.is_some())
    });
    update(cx, handle, |view, _, cx| {
        let review = view.merge_review.clone().unwrap();
        review.update(cx, |review, cx| {
            assert_eq!(review.source, Source::Copy(copy.clone()));
            for n in 0..review.choices.len() {
                review.choose(n, Choice::Mine, cx);
            }
            review.apply(cx);
        });
    });
    let text = editor_text(cx, handle);
    assert!(text.starts_with("ONE\ntwo\n"), "theirs merged in: {text:?}");
    assert!(text.ends_with("mine more"), "mine kept: {text:?}");
    wait_until(cx, |cx| {
        notice_text(cx, handle).is_some_and(|n| n != MERGED)
    });
    match notice_text(cx, handle) {
        Some(notice) if notice.starts_with("Conflict not resolved") => assert!(copy.exists()),
        notice => {
            assert_eq!(notice.as_deref(), Some(RESOLVED));
            assert!(!copy.exists(), "moved to the trash");
        }
    }
}

#[gpui::test]
fn resolving_a_copy_keeps_the_original_in_the_history(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "original text\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    wait_for_index(cx, handle);
    let copy = env.file(COPY, "copy text\n");
    update(cx, handle, |view, window, cx| {
        view.on_watch_event(file_event(&copy, FileChange::Created), window, cx)
    });
    update(cx, handle, |view, window, cx| {
        view.review_conflict(&a, window, cx)
    });
    wait_until(cx, |cx| {
        read(cx, handle, |view, _| view.merge_review.is_some())
    });
    update(cx, handle, |view, _, cx| {
        let review = view.merge_review.clone().unwrap();
        review.update(cx, |review, cx| {
            for n in 0..review.choices.len() {
                review.choose(n, Choice::Theirs, cx);
            }
            review.apply(cx);
        });
    });
    wait_until(cx, |_| {
        std::fs::read_to_string(&a).is_ok_and(|text| text == "copy text\n")
    });
    wait_until(cx, |cx| {
        notice_text(cx, handle).is_some_and(|n| n == RESOLVED || n.starts_with("Conflict not"))
    });
    let texts: Vec<String> = versions(cx, handle, &a)
        .iter()
        .map(|v| text_of(cx, handle, v))
        .collect();
    assert!(texts.iter().any(|t| t == "original text\n"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "copy text\n"), "{texts:?}");
}

#[gpui::test]
fn keep_both_renames_the_copy_into_a_note(cx: &mut TestAppContext) {
    let env = setup(cx);
    let a = env.file("a.md", "one\n");
    let handle = workspace(cx, &env);
    wait_for_history(cx, handle);
    wait_for_index(cx, handle);
    let copy = copy_arrives(cx, &env, handle, &a, "other\n");
    update(cx, handle, |view, window, cx| {
        view.review_conflict(&a, window, cx)
    });
    wait_until(cx, |cx| {
        read(cx, handle, |view, _| view.merge_review.is_some())
    });
    update(cx, handle, |view, window, cx| {
        let review = view.merge_review.clone().unwrap();
        review.update(cx, |_, cx| {
            cx.emit(crate::merge_review::ReviewEvent::KeepBothNotes { copy: copy.clone() })
        });
        let _ = window;
    });
    let kept = env.root().join("a (2).md");
    assert!(kept.is_file() && !copy.exists());
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "other\n");
    wait_until(cx, |cx| {
        read(cx, handle, |view, cx| {
            view.sidebar
                .as_ref()
                .is_some_and(|sidebar| !sidebar.read(cx).conflicted.contains(&a))
        })
    });
}

#[gpui::test]
fn a_copy_without_its_original_is_an_ordinary_note(cx: &mut TestAppContext) {
    let env = setup(cx);
    env.file("b.md", "b\n");
    let copy = env.file(COPY, "orphan\n");
    let handle = workspace(cx, &env);
    wait_for_index(cx, handle);
    let marked = read(cx, handle, |view, cx| {
        view.sidebar
            .as_ref()
            .map(|sidebar| sidebar.read(cx).conflicted.len())
    });
    assert_eq!(marked, Some(0));
    assert!(copy.is_file());
}
