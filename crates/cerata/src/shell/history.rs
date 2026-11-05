use std::path::{Path, PathBuf};

use cerata_core::fs::FileContent;
use cerata_core::fs::text::{decode, encode};
use cerata_core::history::gc::Purge;
use cerata_core::history::{self, Cause, DeletedNote};
use cerata_core::merge::{Merge, merge};
use gpui::{AppContext as _, ClipboardItem, Context, Focusable as _, Window};

use super::{WindowView, file_name};
use crate::documents::Conflict;
use crate::history_panel::{HistoryEvent, HistoryPanel, View};
use crate::merge_review::{MergeReview, ReviewEvent, Source};
use crate::palette::HistoryCommand;

pub const SYNC_MERGE_LIMIT: usize = 256 * 1024;

pub const MERGED: &str = "Merged changes made outside Cerata";
pub const RESOLVED: &str = "Conflict resolved. The copy was moved to the trash.";

pub fn purge_message(n: usize) -> String {
    let versions = if n == 1 {
        "1 version".to_owned()
    } else {
        format!("{n} versions")
    };
    format!("This permanently deletes {versions}. This can't be undone.")
}

impl WindowView {
    pub fn show_history(
        &mut self,
        path: Option<PathBuf>,
        view: View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.core.runtime();
        let path = path.or_else(|| self.tabs.active_path().map(Path::to_path_buf));
        if view == View::Note
            && let Some(path) = &path
            && self.tabs.active_path() != Some(path.as_path())
        {
            self.open_file(path, window, cx);
        }
        let store = runtime.as_ref().and_then(|r| r.store.read().clone());
        let unavailable =
            match (&runtime, &store) {
                (None, _) => Some("History is kept for notes of a library.".to_owned()),
                (Some(runtime), None) => {
                    Some(runtime.store_unavailable.read().clone().unwrap_or_else(|| {
                        "History is not ready yet. Try again in a moment.".into()
                    }))
                }
                _ => None,
            };
        let relative = path
            .as_ref()
            .and_then(|path| runtime.as_ref().and_then(|r| r.relative(path)));
        let title = path
            .as_ref()
            .map(|path| {
                self.known_title(path)
                    .unwrap_or_else(|| crate::sidebar::tree::stem(&file_name(path)).to_owned())
            })
            .unwrap_or_default();
        let current = path
            .as_ref()
            .and_then(|path| self.docs.get(path))
            .map(|doc| doc.editor.read(cx).state.doc().to_owned())
            .unwrap_or_default();
        if let (Some(runtime), Some(path)) = (&runtime, &path) {
            runtime.flush_history(Some(path), Some(cerata_core::workspace::HISTORY_FLUSH));
        }
        let panel = cx.new(|cx| {
            HistoryPanel::new(store, unavailable, path, relative, title, current, view, cx)
        });
        self._history_events = Some(cx.subscribe_in(
            &panel,
            window,
            |this, _, event, window, cx| this.on_history_event(event.clone(), window, cx),
        ));
        window.focus(&panel.focus_handle(cx), cx);
        self.history_panel = Some(panel);
        cx.notify();
    }

    pub fn close_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_panel.take().is_some() {
            self._history_events = None;
            self.active_editor = None;
            self.sync_active(window, cx);
            cx.notify();
        }
    }

    fn on_history_event(
        &mut self,
        event: HistoryEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HistoryEvent::Close => self.close_history(window, cx),
            HistoryEvent::Restore { path, bytes } => {
                if self.restore_version(&path, &bytes, cx) {
                    self.show_notice("Version restored. Press Ctrl+Z to undo.", cx);
                }
            }
            HistoryEvent::RestoreDeleted { note, bytes } => self.restore_deleted(note, bytes, cx),
            HistoryEvent::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.show_notice("Version copied", cx);
            }
            HistoryEvent::PurgeNote { note } => self.confirm_purge(Purge::Note(note), window, cx),
            HistoryEvent::PurgeBefore { ts } => self.confirm_purge(Purge::Before(ts), window, cx),
            HistoryEvent::Notice(notice) => self.show_notice(notice, cx),
        }
    }

    pub(super) fn replace_text(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(doc) = self
            .docs
            .get(path)
            .filter(|doc| doc.loading_since.is_none())
        else {
            return false;
        };
        doc.editor.update(cx, |editor, cx| {
            let current = editor.state.doc().to_owned();
            if current == text {
                return false;
            }
            let prefix = common_prefix(&current, text);
            let suffix = common_suffix(&current[prefix..], &text[prefix..]);
            let change = cerata_markdown::Change::new(
                prefix..current.len() - suffix,
                &text[prefix..text.len() - suffix],
            );
            editor.update_state(cx, |state| {
                state
                    .apply(
                        cerata_markdown::Transaction::new()
                            .change(change)
                            .event(cerata_markdown::UserEvent::Command),
                    )
                    .is_ok()
            })
        })
    }

    pub fn restore_version(&mut self, path: &Path, bytes: &[u8], cx: &mut Context<Self>) -> bool {
        let Some(doc) = self
            .docs
            .get(path)
            .filter(|doc| doc.loading_since.is_none())
        else {
            self.show_notice("Open the note before restoring one of its versions.", cx);
            return false;
        };
        if doc.conflict.is_some() {
            self.show_notice(
                "Review the change made outside Cerata before restoring.",
                cx,
            );
            return false;
        }
        let Ok((text, format)) = decode(bytes) else {
            self.show_notice("This version is not UTF-8 text.", cx);
            return false;
        };
        let before = doc.editor.read(cx).state.doc().to_owned();
        let before_format = doc.format;
        if let Some(runtime) = self.core.runtime() {
            runtime.flush_history(Some(path), None);
        }
        self.record_version(path, &before, before_format, Cause::Save);
        if let Some(doc) = self.docs.get_mut(path) {
            doc.format = format;
        }
        self.replace_text(path, &text, cx);
        self.record_bytes(path, bytes.to_vec(), Cause::Restore);
        self.refresh_history_panel(path, cx);
        true
    }

    pub(super) fn record_bytes(&self, path: &Path, bytes: Vec<u8>, cause: Cause) -> bool {
        let Some(runtime) = self.core.runtime() else {
            return false;
        };
        let (Some(history), Some(relative)) =
            (runtime.history.read().clone(), runtime.relative(path))
        else {
            return false;
        };
        history.record(history::Request::new(relative, bytes, cause));
        true
    }

    fn refresh_history_panel(&mut self, path: &Path, cx: &mut Context<Self>) {
        let current = self
            .docs
            .get(path)
            .map(|doc| doc.editor.read(cx).state.doc().to_owned());
        let Some(panel) = self.history_panel.clone() else {
            return;
        };
        if let Some(current) = current {
            panel.update(cx, |panel, cx| panel.set_current(current, cx));
        }
        let history = self.history();
        let task = cx.spawn(async move |_, cx| {
            if let Some(history) = history {
                cx.background_executor()
                    .spawn(async move {
                        history.flush_and_wait(None, cerata_core::workspace::HISTORY_FLUSH)
                    })
                    .await;
            }
            panel.update(cx, |panel, cx| panel.reload(cx));
        });
        self._tasks.push(task);
    }

    fn restore_deleted(&mut self, note: DeletedNote, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let Some(runtime) = self.core.runtime() else {
            return;
        };
        let original = cerata_core::reconcile::path_from_relative(&runtime.root, &note.path);
        let target = free_path(&original, "restored");
        let task = cx.spawn(async move |this, cx| {
            let job = runtime.clone();
            let target_for_job = target.clone();
            let written = cx
                .background_executor()
                .spawn(async move {
                    let relative = job
                        .relative(&target_for_job)
                        .ok_or_else(|| "outside the library".to_owned())?;
                    if let Some(store) = job.store.read().clone() {
                        let _ =
                            history::undelete(&store, &job.token, note.id, relative.clone()).recv();
                    }
                    let hash = job.self_writes.record(&target_for_job, &bytes);
                    let written = job.fs.write_new_bytes(&target_for_job, &bytes);
                    match &written {
                        Ok(_) => job.self_writes.written(&target_for_job, hash, &bytes),
                        Err(_) => job.self_writes.abandoned(&target_for_job, hash),
                    }
                    written.map_err(|error| error.to_string())?;
                    if let Some(history) = job.history.read().clone() {
                        history.record(history::Request::new(relative, bytes, Cause::Restore));
                        history.flush_and_wait(None, cerata_core::workspace::HISTORY_FLUSH);
                    }
                    Ok::<_, String>(())
                })
                .await;
            let _ = this.update(cx, |this, cx| match written {
                Ok(()) => {
                    this.show_notice(format!("Restored {}", file_name(&target)), cx);
                    this.refresh_sidebar_for(&target, cx);
                    if let Some(panel) = this.history_panel.clone() {
                        panel.update(cx, |panel, cx| panel.reload(cx));
                    }
                    let path = target.clone();
                    this.with_window(cx, move |this, window, cx| {
                        this.open_file(&path, window, cx)
                    });
                }
                Err(error) => this.show_notice(format!("Note not restored: {error}"), cx),
            });
        });
        self._tasks.push(task);
    }

    fn confirm_purge(&mut self, purge: Purge, window: &mut Window, cx: &mut Context<Self>) {
        let Some(history) = self.history() else {
            return;
        };
        let counting = history.clone();
        let count = cx.background_spawn(async move { counting.purge_count(purge) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let count = match count.await {
                Ok(count) => count,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.show_notice(format!("History not deleted: {error}"), cx)
                    });
                    return;
                }
            };
            if count == 0 {
                let _ = this.update(cx, |this, cx| this.show_notice("No versions to delete", cx));
                return;
            }
            let Ok(answer) = this.update_in(cx, |_, window, cx| {
                window.prompt(
                    gpui::PromptLevel::Warning,
                    &purge_message(count),
                    None,
                    &["Cancel", "Delete"],
                    cx,
                )
            }) else {
                return;
            };
            if answer.await != Ok(1) {
                return;
            }
            let purged = cx
                .background_executor()
                .spawn(async move { history.purge(purge).recv() })
                .await;
            let _ = this.update(cx, |this, cx| {
                let message = match purged {
                    Ok(Ok(removed)) => {
                        let n = removed.versions;
                        if n == 1 {
                            "Deleted 1 version".to_owned()
                        } else {
                            format!("Deleted {n} versions")
                        }
                    }
                    Ok(Err(error)) => format!("History not deleted: {error}"),
                    Err(_) => "History not deleted: the store stopped".to_owned(),
                };
                this.show_notice(message, cx);
                if let Some(panel) = this.history_panel.clone() {
                    panel.update(cx, |panel, cx| {
                        panel.close_date(cx);
                        panel.reload(cx);
                    });
                }
            });
        });
        self._tasks.push(task);
    }

    pub(super) fn merge_disk_change(
        &mut self,
        path: &Path,
        file: FileContent,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.docs.get(path) else {
            return;
        };
        let base = doc.disk_text.clone();
        let ours = doc.editor.read(cx).state.doc().to_owned();
        let format = doc.format;
        let revision = doc.editor.read(cx).state.revision();
        self.record_version(path, &file.content, file.format, Cause::External);
        self.record_version(path, &ours, format, Cause::Save);
        if let Some(doc) = self.docs.get_mut(path) {
            doc.conflict = Some(Conflict::Modified {
                disk: file.content.clone(),
            });
            doc.merge = None;
        }
        self.hold_saves(path);
        let theirs = file.content.clone();
        if base.len() + ours.len() + theirs.len() <= SYNC_MERGE_LIMIT {
            let merged = merge(&base, &ours, &theirs);
            self.finish_merge(path, file, merged, cx);
            return;
        }
        let path = path.to_path_buf();
        let task = cx.spawn(async move |this, cx| {
            let merged = cx
                .background_executor()
                .spawn(async move { merge(&base, &ours, &theirs) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let current = this.docs.get(&path).is_some_and(|doc| {
                    doc.editor.read(cx).state.revision() == revision
                        && doc.conflict
                            == Some(Conflict::Modified {
                                disk: file.content.clone(),
                            })
                });
                if current {
                    this.finish_merge(&path, file, merged, cx);
                }
            });
        });
        self._tasks.push(task);
    }

    fn finish_merge(
        &mut self,
        path: &Path,
        file: FileContent,
        merged: Merge,
        cx: &mut Context<Self>,
    ) {
        match merged.text() {
            Some(text) => {
                if let Some(doc) = self.docs.get_mut(path) {
                    doc.conflict = None;
                    doc.merge = None;
                    doc.disk_text = file.content.clone();
                    doc.modified_at = file.modified_at;
                }
                self.bridge.set_disk(path, Some(file.content));
                self.replace_text(path, &text, cx);
                let format = self.docs.get(path).map_or(file.format, |doc| doc.format);
                self.record_version(path, &text, format, Cause::Merge);
                self.resume_saves(path, cx);
                self.show_notice(MERGED, cx);
            }
            None => {
                if let Some(doc) = self.docs.get_mut(path) {
                    doc.merge = Some(merged);
                }
            }
        }
        cx.notify();
    }

    pub fn review_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_path().map(Path::to_path_buf) else {
            return;
        };
        let Some(doc) = self.docs.get(&path) else {
            return;
        };
        let Some(Conflict::Modified { disk }) = &doc.conflict else {
            return;
        };
        let merged = doc
            .merge
            .clone()
            .unwrap_or_else(|| merge(&doc.disk_text, doc.editor.read(cx).state.doc(), disk));
        self.open_review(path, Source::Disk, merged, window, cx);
    }

    fn open_review(
        &mut self,
        path: PathBuf,
        source: Source,
        merged: Merge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let review = cx.new(|cx| MergeReview::new(path, source, merged, cx));
        self._review_events = Some(cx.subscribe_in(
            &review,
            window,
            |this, _, event, window, cx| this.on_review_event(event.clone(), window, cx),
        ));
        window.focus(&review.focus_handle(cx), cx);
        self.merge_review = Some(review);
        cx.notify();
    }

    pub fn close_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.merge_review.take().is_some() {
            self._review_events = None;
            self.active_editor = None;
            self.sync_active(window, cx);
            cx.notify();
        }
    }

    fn on_review_event(&mut self, event: ReviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ReviewEvent::Close => self.close_review(window, cx),
            ReviewEvent::Apply { path, source, text } => {
                self.close_review(window, cx);
                match source {
                    Source::Disk => self.apply_disk_merge(&path, &text, cx),
                    Source::Copy(copy) => self.apply_copy_merge(&path, &copy, text, cx),
                }
            }
            ReviewEvent::KeepBothNotes { copy } => {
                self.close_review(window, cx);
                self.keep_both_notes(&copy, cx);
            }
        }
    }

    pub fn apply_disk_merge(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(path) else {
            return;
        };
        let Some(Conflict::Modified { disk }) = doc.conflict.take() else {
            return;
        };
        doc.merge = None;
        doc.disk_text = disk.clone();
        let format = doc.format;
        self.bridge.set_disk(path, Some(disk));
        self.replace_text(path, text, cx);
        self.record_version(path, text, format, Cause::Merge);
        self.resume_saves(path, cx);
    }

    pub fn conflict_copy_of(original: &Path) -> Option<PathBuf> {
        let name = original.file_name()?.to_str()?.to_owned();
        let mut copies: Vec<PathBuf> = std::fs::read_dir(original.parent()?)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(cerata_core::conflict_copy::original_name)
                    .is_some_and(|(of, _)| of == name)
            })
            .collect();
        copies.sort();
        copies.into_iter().next()
    }

    pub fn review_conflict(
        &mut self,
        original: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.core.runtime() else {
            return;
        };
        let Some(copy) = Self::conflict_copy_of(original) else {
            self.show_notice("This note has no conflict copy", cx);
            return;
        };
        let mine = self
            .docs
            .get(original)
            .filter(|doc| doc.loading_since.is_none())
            .map(|doc| doc.editor.read(cx).state.doc().to_owned());
        let original = original.to_path_buf();
        let task = cx.spawn_in(window, async move |this, cx| {
            let (job, path, copy_path) = (runtime.clone(), original.clone(), copy.clone());
            let read = cx
                .background_executor()
                .spawn(async move {
                    let theirs = job
                        .fs
                        .read_file(&copy_path)
                        .map_err(|e| e.to_string())?
                        .content;
                    let mine = match mine {
                        Some(mine) => mine,
                        None => job.fs.read_file(&path).map_err(|e| e.to_string())?.content,
                    };
                    let modified = std::fs::metadata(&copy_path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or_else(history::unix_now, |d| d.as_secs() as i64);
                    let base = match (job.store.read().clone(), job.relative(&path)) {
                        (Some(store), Some(relative)) => {
                            history::version_before(&store, &relative, modified)
                                .ok()
                                .flatten()
                                .and_then(|v| history::content(&store, &v.hash).ok().flatten())
                                .and_then(|bytes| decode(&bytes).ok().map(|(text, _)| text))
                                .unwrap_or_default()
                        }
                        _ => String::new(),
                    };
                    Ok::<_, String>(merge(&base, &mine, &theirs))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match read {
                Ok(merged) => this.open_review(original, Source::Copy(copy), merged, window, cx),
                Err(error) => this.show_notice(format!("Conflict not opened: {error}"), cx),
            });
        });
        self._tasks.push(task);
    }

    fn apply_copy_merge(
        &mut self,
        original: &Path,
        copy: &Path,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.core.runtime() else {
            return;
        };
        let open = self
            .docs
            .get(original)
            .filter(|doc| doc.loading_since.is_none() && doc.conflict.is_none())
            .map(|doc| doc.format);
        if let Some(format) = open {
            let before = self
                .docs
                .get(original)
                .map(|doc| doc.editor.read(cx).state.doc().to_owned())
                .unwrap_or_default();
            self.record_version(original, &before, format, Cause::Save);
            self.replace_text(original, &text, cx);
            self.record_version(original, &text, format, Cause::Merge);
        }
        let (original, copy) = (original.to_path_buf(), copy.to_path_buf());
        let task = cx.spawn(async move |this, cx| {
            let job = runtime.clone();
            let (original_path, copy_path) = (original.clone(), copy.clone());
            let done = cx
                .background_executor()
                .spawn(async move {
                    if open.is_none() {
                        let file = job
                            .fs
                            .read_file(&original_path)
                            .map_err(|e| e.to_string())?;
                        let history = job.history.read().clone().zip(job.relative(&original_path));
                        if let Some((history, relative)) = &history {
                            history.record(history::Request::new(
                                relative.clone(),
                                encode(&file.content, file.format),
                                Cause::Save,
                            ));
                        }
                        let bytes = encode(&text, file.format);
                        let hash = job.self_writes.record(&original_path, &bytes);
                        let written = job.fs.write_bytes(&original_path, &bytes);
                        match &written {
                            Ok(_) => job.self_writes.written(&original_path, hash, &bytes),
                            Err(_) => job.self_writes.abandoned(&original_path, hash),
                        }
                        written.map_err(|e| e.to_string())?;
                        if let Some((history, relative)) = history {
                            history.record(history::Request::new(relative, bytes, Cause::Merge));
                        }
                    }
                    job.fs.delete(&copy_path).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(()) => {
                    this.path_removed(&copy, cx);
                    if let Some(sidebar) = this.sidebar.clone() {
                        sidebar.update(cx, |sidebar, cx| sidebar.removed(&copy, cx));
                    }
                    this.refresh_sidebar_for(&original, cx);
                    this.show_notice(RESOLVED, cx);
                }
                Err(error) => this.show_notice(format!("Conflict not resolved: {error}"), cx),
            });
        });
        self._tasks.push(task);
    }

    fn keep_both_notes(&mut self, copy: &Path, cx: &mut Context<Self>) {
        let Some(runtime) = self.core.runtime() else {
            return;
        };
        let Some(original) = copy
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(cerata_core::conflict_copy::original_name)
            .map(|(name, _)| copy.with_file_name(name))
        else {
            return;
        };
        let target = free_numbered(&original);
        match runtime.fs.rename(copy, &target) {
            Ok(_) => {
                self.path_renamed(copy, &target, cx);
                self.refresh_sidebar_for(&target, cx);
                if let Some(sidebar) = self.sidebar.clone() {
                    sidebar.update(cx, |sidebar, cx| sidebar.refresh_all(cx));
                }
                self.show_notice(format!("The copy is now {}", file_name(&target)), cx);
            }
            Err(error) => self.show_notice(format!("Copy not kept: {error}"), cx),
        }
    }

    pub fn run_history_command(
        &mut self,
        command: HistoryCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.history_panel.clone();
        match command {
            HistoryCommand::ShowNoteHistory => self.show_history(None, View::Note, window, cx),
            HistoryCommand::ShowDeletedNotes => self.show_history(None, View::Deleted, window, cx),
            HistoryCommand::ReviewConflict => {
                if let Some(path) = self.tabs.active_path().map(Path::to_path_buf) {
                    self.review_conflict(&path, window, cx);
                }
            }
            action => {
                let Some(panel) = panel else {
                    return;
                };
                panel.update(cx, |panel, cx| match action {
                    HistoryCommand::Restore => panel.restore(cx),
                    HistoryCommand::Copy => panel.copy(cx),
                    HistoryCommand::TogglePin => panel.toggle_pin(cx),
                    HistoryCommand::ShowChanges => panel.toggle_changes(cx),
                    HistoryCommand::DeleteNoteHistory => panel.purge_note(cx),
                    _ => panel.ask_date(window, cx),
                });
            }
        }
    }
}

pub fn free_path(path: &Path, label: &str) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| {
            let suffix = if n == 1 {
                label.to_owned()
            } else {
                format!("{label} {n}")
            };
            path.with_file_name(format!("{stem} ({suffix}){ext}"))
        })
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

pub fn free_numbered(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for ((i, x), y) in a.char_indices().zip(b.chars()) {
        if x != y {
            return i;
        }
        n = i + x.len_utf8();
    }
    n
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    n
}
