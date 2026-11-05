use std::ops::Range;
use std::path::{Path, PathBuf};

use cerata_core::fs::assets::{AssetsOutcome, LinkRewrite, NoteOperation};
use cerata_core::fs::{DirEntry, EntryKind, FsError, NEW_FILE_CONTENT};
use cerata_core::index::relative_slash_path;
use cerata_core::settings::ConfigValue;
use cerata_core::workspace::WorkspaceRuntime;
use gpui::{AppContext as _, ClipboardItem, Context, Entity, Pixels, Point, Subscription, Window};

use super::tree::{Listing, Row, SortMode, stem};
use super::{Sidebar, SidebarEvent};
use crate::editor::text_field::{FieldEvent, TextField};
use crate::menu::{Item, Menu};

pub const DRAG_THRESHOLD: f32 = 4.0;

pub fn reveal_label() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Reveal in Finder",
        "windows" => "Reveal in Explorer",
        _ => "Show in Folder",
    }
}

pub fn open_folder_label() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Open in Finder",
        "windows" => "Open in Explorer",
        _ => "Open in File Manager",
    }
}

pub fn drop_dest(target: Option<&DirEntry>, root: &Path) -> PathBuf {
    match target {
        Some(entry) if entry.is_dir => entry.path.clone(),
        Some(entry) => entry
            .path
            .parent()
            .map_or_else(|| root.to_path_buf(), Path::to_path_buf),
        None => root.to_path_buf(),
    }
}

pub fn can_move(source: &Path, source_is_dir: bool, dest: &Path) -> bool {
    if source.parent() == Some(dest) {
        return false;
    }
    !(source_is_dir && dest.starts_with(source))
}

pub fn move_path(dest: &Path, source: &Path) -> Option<PathBuf> {
    Some(dest.join(source.file_name()?))
}

pub fn subtree_range(rows: &[Row], dest: &Path) -> Option<Range<usize>> {
    let start = rows.iter().position(|row| row.entry.path == dest)?;
    let depth = rows[start].depth;
    let end = rows[start + 1..]
        .iter()
        .position(|row| row.depth <= depth)
        .map_or(rows.len(), |offset| start + 1 + offset);
    Some(start..end)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub start: Point<Pixels>,
    pub paths: Vec<PathBuf>,
    pub active: bool,
    pub over: Option<DirEntry>,
}

pub struct Renaming {
    pub path: PathBuf,
    pub field: Entity<TextField>,
    pub error: Option<String>,
    pub created: bool,
    _events: Subscription,
}

impl Sidebar {
    fn entry_for(&self, path: &Path) -> Option<DirEntry> {
        self.rows
            .iter()
            .map(|row| &row.entry)
            .chain(&self.pinned_entries)
            .chain(&self.recents)
            .find(|entry| entry.path == path)
            .cloned()
    }

    pub fn open_row_menu(
        &mut self,
        entry: &DirEntry,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = if self.selection.len() >= 2 && self.selection.contains(&entry.path) {
            bulk_menu(self.selection.paths.clone())
        } else {
            self.selection.clear();
            if entry.is_dir {
                folder_menu(entry.path.clone())
            } else {
                file_menu(
                    entry.path.clone(),
                    self.is_pinned(&entry.path),
                    self.conflicted.contains(&entry.path),
                )
            }
        };
        self.menu = Some(Menu::new(position, items));
        cx.notify();
    }

    pub fn open_surface_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let show_search = self.setting_bool(super::SHOW_SEARCH_KEY).unwrap_or(true);
        self.menu = Some(Menu::new(
            position,
            surface_menu(
                self.root().is_some(),
                self.options.mode,
                self.options.folders_first,
                show_search,
                self.show_recents,
            ),
        ));
        cx.notify();
    }

    pub fn set_setting(&mut self, key: &str, value: ConfigValue, cx: &mut Context<Self>) {
        match self.window_state().settings().begin_set(
            key,
            value,
            cerata_core::settings::Layer::Global,
        ) {
            Ok(pending) => {
                let key = key.to_owned();
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = pending.commit() {
                            log::warn!("{key} not saved: {error}");
                        }
                    })
                    .detach();
            }
            Err(error) => log::warn!("{key} not set: {error}"),
        }
        self.settings_changed(cx);
        cx.emit(SidebarEvent::SettingsChanged);
    }

    pub fn copy_paths(&mut self, paths: &[PathBuf], relative: bool, cx: &mut Context<Self>) {
        let Some(root) = self.root().map(Path::to_path_buf) else {
            return;
        };
        let text = paths
            .iter()
            .map(|path| {
                if relative {
                    relative_slash_path(&root, path)
                } else {
                    path.display().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        if path.exists() {
            cx.reveal_path(path);
        } else {
            cx.emit(SidebarEvent::Notice(format!(
                "Failed to reveal: {} no longer exists",
                path.display()
            )));
        }
    }

    pub fn open_root_in_file_manager(&mut self, cx: &mut Context<Self>) {
        let state = self.window_state().clone();
        let Some(snapshot) = state.snapshot() else {
            return;
        };
        let result = cerata_core::terminal::open_workspace_in_file_manager_for_snapshot(
            &state,
            &snapshot,
            |root| Ok(root.to_path_buf()),
        );
        match result {
            Ok(root) => cx.open_with_system(&root),
            Err(error) => cx.emit(SidebarEvent::Notice(format!(
                "Failed to open the folder: {error}"
            ))),
        }
    }

    pub fn open_terminal(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        let state = self.window_state().clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(
                    async move { cerata_core::terminal::open_in_terminal(&state, dir.as_deref()) },
                )
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(SidebarEvent::Notice(format!(
                        "Failed to open terminal: {error}"
                    )))
                });
            }
        });
        task.detach();
    }

    pub fn create_entry(
        &mut self,
        parent: Option<PathBuf>,
        kind: EntryKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.runtime().cloned() else {
            return;
        };
        let parent = parent.unwrap_or_else(|| runtime.root.clone());
        if parent != runtime.root {
            self.expand(&parent, cx);
        } else if kind == EntryKind::File {
            self.open_unfiled();
        }
        let generation = self.generation;
        let window_handle = window.window_handle();
        let task = cx.spawn(async move |this, cx| {
            let target = parent.clone();
            let created = cx
                .background_executor()
                .spawn(async move { runtime.fs.create_untitled(&target, kind) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let entry = match created {
                    Ok(entry) => entry,
                    Err(error) => {
                        let what = if kind == EntryKind::File {
                            "file"
                        } else {
                            "folder"
                        };
                        cx.emit(SidebarEvent::Notice(format!(
                            "Failed to create {what}: {error}"
                        )));
                        return;
                    }
                };
                if this.generation != generation {
                    return;
                }
                this.pending_rename = Some(entry.path.clone());
                this.load_dir(&parent, cx);
            });
            let _ = window_handle;
        });
        task.detach();
    }

    pub fn start_rename(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entry_for(path) else {
            return;
        };
        let text = if entry.is_dir {
            entry.name.clone()
        } else {
            stem(&entry.name).to_owned()
        };
        let field = cx.new(|cx| {
            let mut field = TextField::new("Name", cx).labeled("New name");
            field.set_text(&text, cx);
            field
        });
        let events =
            cx.subscribe_in(
                &field,
                window,
                |this, _, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Submit { .. } => {
                        let created = this.renaming.as_ref().is_some_and(|r| r.created);
                        if let Some(path) = this.commit_rename(window, cx)
                            && created
                        {
                            cx.emit(SidebarEvent::OpenCreated(path));
                        }
                    }
                    FieldEvent::Escape => this.cancel_rename(window, cx),
                    FieldEvent::Changed => {
                        if let Some(renaming) = &mut this.renaming {
                            renaming.error = None;
                        }
                    }
                },
            );
        let focus = field.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.on_blur(&focus, window, |this, window, cx| {
            this.commit_rename(window, cx);
        })
        .detach();
        self.renaming = Some(Renaming {
            path: path.to_path_buf(),
            field,
            error: None,
            created: false,
            _events: events,
        });
        cx.notify();
    }

    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(renaming) = self.renaming.take() {
            if renaming.created {
                self.refresh_sections(cx);
            }
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    pub fn commit_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let renaming = self.renaming.as_ref()?;
        let created = renaming.created;
        let Some(entry) = self.entry_for(&renaming.path) else {
            self.renaming = None;
            return None;
        };
        let typed = renaming.field.read(cx).text.trim().to_owned();
        let name = if entry.is_dir {
            typed.clone()
        } else {
            match Path::new(&entry.name).extension() {
                Some(ext) => format!("{typed}.{}", ext.to_string_lossy()),
                None => typed.clone(),
            }
        };
        if typed.is_empty() {
            self.cancel_rename(window, cx);
            return None;
        }
        if name == entry.name {
            self.cancel_rename(window, cx);
            if created {
                self.name_heading(&entry.path, &typed);
            }
            return Some(entry.path);
        }
        if let Err(FsError::InvalidName { reason, .. }) = cerata_core::fs::validate_file_name(&name)
        {
            if let Some(renaming) = &mut self.renaming {
                renaming.error = Some(format!("\"{typed}\" cannot be used: {reason}."));
            }
            cx.notify();
            return None;
        }
        let runtime = self.runtime().cloned()?;
        let parent = entry.path.parent().map(Path::to_path_buf)?;
        let new = parent.join(&name);
        if new.exists() && !same_entry_ignoring_case(&entry.path, &new) {
            let kind = if entry.is_dir { "folder" } else { "file" };
            if let Some(renaming) = &mut self.renaming {
                renaming.error = Some(format!("A {kind} named \"{name}\" already exists."));
            }
            cx.notify();
            return None;
        }
        let mut committed = None;
        match move_entry(&runtime, &entry.path, &new, entry.is_dir) {
            Ok(done) => {
                self.renaming = None;
                if created {
                    self.name_heading(&done.path, &typed);
                }
                self.after_note_move(&entry.path, &done, cx);
                if let Some(notice) = done.notice() {
                    cx.emit(SidebarEvent::Notice(notice));
                }
                window.focus(&self.focus_handle, cx);
                committed = Some(done.path);
            }
            Err(FsError::AlreadyExists(_)) => {
                let kind = if entry.is_dir { "folder" } else { "file" };
                if let Some(renaming) = &mut self.renaming {
                    renaming.error = Some(format!("A {kind} named \"{name}\" already exists."));
                }
            }
            Err(error) => {
                if let Some(renaming) = &mut self.renaming {
                    renaming.error = Some(format!("Failed to rename: {error}"));
                }
            }
        }
        cx.notify();
        committed
    }

    fn name_heading(&self, path: &Path, title: &str) {
        let Some(runtime) = self.runtime() else {
            return;
        };
        let file = match runtime.fs.read_file(path) {
            Ok(file) if file.content == NEW_FILE_CONTENT => file,
            Ok(_) => return,
            Err(error) => {
                log::warn!("heading not named: {error}");
                return;
            }
        };
        let heading = format!("{NEW_FILE_CONTENT}{title}\n\n");
        if let Err(error) = runtime.fs.write_file(path, &heading, file.format) {
            log::warn!("heading not named: {error}");
        }
    }

    fn after_note_move(&mut self, old: &Path, done: &NoteOperation, cx: &mut Context<Self>) {
        if let AssetsOutcome::Followed { from, to } = &done.assets {
            self.after_move(from, to, None, cx);
        }
        self.after_move(old, &done.path, done.links(old), cx);
    }

    pub fn moved_outside(&mut self, old: &Path, new: &Path, cx: &mut Context<Self>) {
        self.after_move(old, new, None, cx);
    }

    fn after_move(
        &mut self,
        old: &Path,
        new: &Path,
        links: Option<LinkRewrite>,
        cx: &mut Context<Self>,
    ) {
        let rewritten: Vec<PathBuf> = self
            .expanded
            .iter()
            .filter(|dir| dir.starts_with(old))
            .cloned()
            .collect();
        let mut reopened = Vec::new();
        for dir in rewritten {
            self.expanded.remove(&dir);
            if let Ok(rest) = dir.strip_prefix(old) {
                self.expanded.insert(new.join(rest));
                reopened.push(new.join(rest));
            }
        }
        self.listings.retain(|dir, _| !dir.starts_with(old));
        for (dir, version) in &mut self.read_versions {
            if dir.starts_with(old) {
                *version += 1;
            }
        }
        for dir in &reopened {
            self.load_dir(dir, cx);
        }
        if let Some(library) = self.library() {
            match crate::app::core(cx).pins.rewrite_prefix(&library, old, new) {
                Ok(pins) => self.pinned = pins,
                Err(error) => log::warn!("pins not updated: {error}"),
            }
        }
        for dir in [old.parent(), new.parent()].into_iter().flatten() {
            let dir = dir.to_path_buf();
            self.load_dir(&dir, cx);
        }
        self.refresh_sections(cx);
        cx.emit(SidebarEvent::Renamed {
            old: old.to_path_buf(),
            new: new.to_path_buf(),
            links,
        });
    }

    pub fn removed(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.expanded.retain(|dir| !dir.starts_with(path));
        if let Some(library) = self.library() {
            match crate::app::core(cx).pins.remove_prefix(&library, path) {
                Ok(pins) => self.pinned = pins,
                Err(error) => log::warn!("pins not updated: {error}"),
            }
        }
        if let Some(parent) = path.parent().map(Path::to_path_buf) {
            self.load_dir(&parent, cx);
        }
        self.refresh_sections(cx);
    }

    pub fn begin_drag(&mut self, path: &Path, position: Point<Pixels>) {
        let paths = if self.selection.len() >= 2 && self.selection.contains(path) {
            self.selection.paths.clone()
        } else {
            vec![path.to_path_buf()]
        };
        self.drag = Some(Drag {
            start: position,
            paths,
            active: false,
            over: None,
        });
    }

    pub fn drag_moved(
        &mut self,
        position: Point<Pixels>,
        over: Option<DirEntry>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &mut self.drag else { return };
        let distance = (position - drag.start).magnitude();
        if !drag.active && distance > f64::from(DRAG_THRESHOLD) {
            drag.active = true;
        }
        if drag.active && drag.over != over {
            drag.over = over;
            cx.notify();
        }
    }

    pub fn drag_highlight(&self) -> Option<Range<usize>> {
        let drag = self.drag.as_ref().filter(|drag| drag.active)?;
        let dest = drop_dest(drag.over.as_ref(), self.root()?);
        subtree_range(&self.rows, &dest)
    }

    pub fn end_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        if !drag.active {
            return false;
        }
        let Some(runtime) = self.runtime().cloned() else {
            return true;
        };
        let dest = drop_dest(drag.over.as_ref(), &runtime.root);
        let mut failures = Vec::new();
        let mut moved: Vec<(PathBuf, NoteOperation)> = Vec::new();
        let mut notices = Vec::new();
        for source in &drag.paths {
            if moved.iter().any(|(_, done)| {
                matches!(&done.assets, AssetsOutcome::Followed { from, .. } if from == source)
            }) {
                continue;
            }
            let is_dir = source.is_dir();
            if !can_move(source, is_dir, &dest) {
                continue;
            }
            let Some(target) = move_path(&dest, source) else {
                continue;
            };
            if target.exists() {
                failures.push(format!(
                    "• \"{}\": an item with that name already exists",
                    source
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                continue;
            }
            match move_entry(&runtime, source, &target, is_dir) {
                Ok(done) => {
                    notices.extend(done.notice());
                    moved.push((source.clone(), done));
                }
                Err(error) => failures.push(format!(
                    "• \"{}\": {error}",
                    source
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )),
            }
        }
        if dest != runtime.root {
            self.expand(&dest, cx);
        }
        for (old, done) in &moved {
            self.after_note_move(old, done, cx);
        }
        self.selection.clear();
        if !failures.is_empty() {
            cx.emit(SidebarEvent::Notice(format!(
                "Couldn't move {} item(s):\n{}",
                failures.len(),
                failures.join("\n")
            )));
        } else if !notices.is_empty() {
            cx.emit(SidebarEvent::Notice(notices.join("\n")));
        }
        cx.notify();
        true
    }

    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    pub fn reveal_in_sidebar(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root().map(Path::to_path_buf) else {
            return;
        };
        if !path.starts_with(&root) {
            return;
        }
        self.reveal_version += 1;
        self.revealing = Some((path.to_path_buf(), self.reveal_version));
        if path.parent() == Some(root.as_path()) {
            self.open_unfiled();
        }
        let mut dir = path.parent();
        let mut parents = Vec::new();
        while let Some(d) = dir {
            if d == root || !d.starts_with(&root) {
                break;
            }
            parents.push(d.to_path_buf());
            dir = d.parent();
        }
        for parent in parents.into_iter().rev() {
            self.expanded.insert(parent.clone());
            if !matches!(self.listings.get(&parent), Some(Listing::Loaded(_))) {
                self.load_dir(&parent, cx);
            }
        }
        self.rebuild_rows();
        self.finish_reveal(cx);
    }

    pub fn finish_reveal(&mut self, cx: &mut Context<Self>) {
        let Some((path, version)) = self.revealing.clone() else {
            return;
        };
        if version != self.reveal_version {
            return;
        }
        if let Some(ix) = self.rows.iter().position(|row| row.entry.path == path) {
            self.selection.press(&path);
            self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
            self.revealing = None;
            cx.notify();
        }
    }
}

fn same_entry_ignoring_case(old: &Path, new: &Path) -> bool {
    old.to_string_lossy().to_lowercase() == new.to_string_lossy().to_lowercase()
}

fn sep<V>() -> Item<V> {
    Item::Separator
}

pub fn file_menu(path: PathBuf, pinned: bool, conflicted: bool) -> Vec<Item<Sidebar>> {
    let p = || path.clone();
    let (open, new_tab, pin, dup, rel, abs, reveal, rename, delete) =
        (p(), p(), p(), p(), p(), p(), p(), p(), p());
    let (history, review) = (p(), p());
    let mut items = vec![
        Item::action("Open", move |_, _, cx: &mut Context<Sidebar>| {
            cx.emit(SidebarEvent::Open(open.clone()))
        }),
        Item::action("Open in new tab", move |_, _, cx: &mut Context<Sidebar>| {
            cx.emit(SidebarEvent::OpenInNewTab(new_tab.clone()))
        }),
        Item::action(
            if pinned { "Unpin" } else { "Pin" },
            move |s: &mut Sidebar, _, cx| s.toggle_pin(&pin, cx),
        ),
        sep(),
        Item::action("Duplicate", move |_, _, cx: &mut Context<Sidebar>| {
            cx.emit(SidebarEvent::Duplicate(dup.clone()))
        }),
        Item::action(
            "Show Note History",
            move |_, _, cx: &mut Context<Sidebar>| {
                cx.emit(SidebarEvent::ShowHistory(history.clone()))
            },
        ),
    ];
    if conflicted {
        items.push(Item::action(
            "Review Conflict",
            move |_, _, cx: &mut Context<Sidebar>| {
                cx.emit(SidebarEvent::ReviewConflict(review.clone()))
            },
        ));
    }
    items.extend([
        sep(),
        Item::action("Copy relative path", move |s: &mut Sidebar, _, cx| {
            s.copy_paths(std::slice::from_ref(&rel), true, cx)
        }),
        Item::action("Copy absolute path", move |s: &mut Sidebar, _, cx| {
            s.copy_paths(std::slice::from_ref(&abs), false, cx)
        }),
        sep(),
        Item::action(reveal_label(), move |s: &mut Sidebar, _, cx| {
            s.reveal(&reveal, cx)
        }),
        sep(),
        Item::action("Rename…", move |s: &mut Sidebar, window, cx| {
            s.start_rename(&rename, window, cx)
        }),
        Item::action("Delete", move |_, _, cx: &mut Context<Sidebar>| {
            cx.emit(SidebarEvent::Delete(vec![delete.clone()]))
        })
        .destructive(),
    ]);
    items
}

pub fn folder_menu(path: PathBuf) -> Vec<Item<Sidebar>> {
    let p = || path.clone();
    let (new_file, new_folder, rel, abs, term, reveal, rename, delete) =
        (p(), p(), p(), p(), p(), p(), p(), p());
    vec![
        Item::action("New File", move |s: &mut Sidebar, window, cx| {
            s.create_entry(Some(new_file.clone()), EntryKind::File, window, cx)
        }),
        Item::action("New Folder", move |s: &mut Sidebar, window, cx| {
            s.create_entry(Some(new_folder.clone()), EntryKind::Folder, window, cx)
        }),
        sep(),
        Item::action("Copy relative path", move |s: &mut Sidebar, _, cx| {
            s.copy_paths(std::slice::from_ref(&rel), true, cx)
        }),
        Item::action("Copy absolute path", move |s: &mut Sidebar, _, cx| {
            s.copy_paths(std::slice::from_ref(&abs), false, cx)
        }),
        sep(),
        Item::action("Open in Terminal", move |s: &mut Sidebar, _, cx| {
            s.open_terminal(Some(term.clone()), cx)
        }),
        Item::action(reveal_label(), move |s: &mut Sidebar, _, cx| {
            s.reveal(&reveal, cx)
        }),
        sep(),
        Item::action("Rename…", move |s: &mut Sidebar, window, cx| {
            s.start_rename(&rename, window, cx)
        }),
        Item::action("Delete", move |_, _, cx: &mut Context<Sidebar>| {
            cx.emit(SidebarEvent::Delete(vec![delete.clone()]))
        })
        .destructive(),
    ]
}

pub fn bulk_menu(paths: Vec<PathBuf>) -> Vec<Item<Sidebar>> {
    let n = paths.len();
    let (rel, abs, delete) = (paths.clone(), paths.clone(), paths);
    vec![
        Item::action(
            format!("Copy {n} relative paths"),
            move |s: &mut Sidebar, _, cx| s.copy_paths(&rel, true, cx),
        ),
        Item::action(
            format!("Copy {n} absolute paths"),
            move |s: &mut Sidebar, _, cx| s.copy_paths(&abs, false, cx),
        ),
        sep(),
        Item::action(
            format!("Delete {n} items"),
            move |_, _, cx: &mut Context<Sidebar>| cx.emit(SidebarEvent::Delete(delete.clone())),
        )
        .destructive(),
    ]
}

pub fn surface_menu(
    has_workspace: bool,
    mode: SortMode,
    folders_first: bool,
    show_search: bool,
    show_recents: bool,
) -> Vec<Item<Sidebar>> {
    let mut items = Vec::new();
    if has_workspace {
        let mut sort = Vec::new();
        for (ix, sort_mode) in SortMode::ALL.into_iter().enumerate() {
            if ix == 2 || ix == 4 {
                sort.push(sep());
            }
            sort.push(Item::check(
                sort_mode.label(),
                sort_mode == mode,
                move |s: &mut Sidebar, _, cx| {
                    s.set_setting(
                        super::SORT_KEY,
                        ConfigValue::String(sort_mode.id().into()),
                        cx,
                    )
                },
            ));
        }
        sort.push(sep());
        sort.push(Item::check(
            "Folders first",
            folders_first,
            move |s: &mut Sidebar, _, cx| {
                s.set_setting(
                    super::FOLDERS_FIRST_KEY,
                    ConfigValue::Bool(!folders_first),
                    cx,
                )
            },
        ));
        items.extend([
            Item::action("New File", |s: &mut Sidebar, window, cx| {
                s.create_entry(None, EntryKind::File, window, cx)
            }),
            Item::action("New Project", |s: &mut Sidebar, window, cx| {
                s.create_entry(None, EntryKind::Folder, window, cx)
            }),
            sep(),
            Item::action("Open in Terminal", |s: &mut Sidebar, _, cx| {
                s.open_terminal(None, cx)
            }),
            Item::action(open_folder_label(), |s: &mut Sidebar, _, cx| {
                s.open_root_in_file_manager(cx)
            }),
            sep(),
            Item::submenu("Sort by", sort),
            sep(),
        ]);
    }
    items.push(Item::check(
        "Search",
        show_search,
        move |s: &mut Sidebar, _, cx| {
            s.set_setting(super::SHOW_SEARCH_KEY, ConfigValue::Bool(!show_search), cx)
        },
    ));
    items.push(Item::check(
        "Recents",
        show_recents,
        move |s: &mut Sidebar, _, cx| {
            s.set_setting(
                super::SHOW_RECENTS_KEY,
                ConfigValue::Bool(!show_recents),
                cx,
            )
        },
    ));
    items
}

fn move_entry(
    runtime: &WorkspaceRuntime,
    old: &Path,
    new: &Path,
    is_dir: bool,
) -> Result<NoteOperation, FsError> {
    let _gate = runtime.self_writes.write_gate();
    let flushed = if is_dir {
        runtime.flush_history(None, Some(cerata_core::workspace::HISTORY_FLUSH))
    } else {
        runtime.flush_history(Some(old), Some(cerata_core::workspace::HISTORY_FLUSH))
    };
    if !flushed {
        log::warn!("versions of {} not written before its move", old.display());
    }
    runtime.self_writes.record_move(old, new);
    let moved = if !is_dir && cerata_core::markdown::is_markdown_path(old) {
        runtime.fs.rename_note(old, new)
    } else {
        runtime.fs.rename(old, new).map(|path| NoteOperation {
            path,
            assets: AssetsOutcome::None,
        })
    };
    if moved.is_err() {
        runtime.self_writes.forget_move(old, new);
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::tree::RowKind;

    fn entry(path: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            path: PathBuf::from(path),
            is_dir,
            is_markdown: !is_dir,
            modified_at: 0,
            created_at: 0,
            title: None,
        }
    }

    fn row(path: &str, depth: usize) -> Row {
        Row {
            entry: entry(path, true),
            depth,
            expanded: true,
            error: None,
            kind: if depth == 0 {
                RowKind::Project
            } else {
                RowKind::Item
            },
        }
    }

    fn labels(items: &[Item<Sidebar>]) -> Vec<String> {
        items
            .iter()
            .map(|item| item.label().map_or_else(|| "-".to_owned(), str::to_owned))
            .collect()
    }

    #[test]
    fn a_drop_resolves_to_the_folder_its_parent_or_the_root() {
        let root = Path::new("/v");
        assert_eq!(
            drop_dest(Some(&entry("/v/dir", true)), root),
            PathBuf::from("/v/dir")
        );
        assert_eq!(
            drop_dest(Some(&entry("/v/dir/a.md", false)), root),
            PathBuf::from("/v/dir")
        );
        assert_eq!(drop_dest(None, root), PathBuf::from("/v"));
    }

    #[test]
    fn a_move_joins_the_destination_with_the_entry_name() {
        let p = Path::new;
        assert_eq!(
            move_path(p("/vault/notes"), p("/vault/a.md")),
            Some(PathBuf::from("/vault/notes/a.md"))
        );
        assert_eq!(
            move_path(p("/vault/archive"), p("/vault/notes")),
            Some(PathBuf::from("/vault/archive/notes"))
        );
    }

    #[test]
    fn moves_into_the_same_folder_or_a_descendant_are_refused() {
        let p = Path::new;
        assert!(
            !can_move(p("/v/dir/a.md"), false, p("/v/dir")),
            "already there"
        );
        assert!(can_move(p("/v/dir/a.md"), false, p("/v/other")));
        assert!(!can_move(p("/v/dir"), true, p("/v/dir")), "onto itself");
        assert!(
            !can_move(p("/v/dir"), true, p("/v/dir/sub")),
            "into a descendant"
        );
        assert!(can_move(p("/v/dir"), true, p("/v/other")));
        assert!(
            can_move(p("/v/dir"), true, p("/v/dir-b")),
            "a sibling sharing a prefix"
        );
        assert_eq!(
            move_path(p("/v/other"), p("/v/dir/a.md")),
            Some(PathBuf::from("/v/other/a.md"))
        );
    }

    #[test]
    fn the_subtree_range_spans_the_folder_and_its_visible_descendants() {
        let rows = [
            row("/v/a", 0),
            row("/v/a/b", 1),
            row("/v/a/b/c", 2),
            row("/v/a/d", 1),
            row("/v/e", 0),
            Row {
                kind: RowKind::Unfiled,
                ..row("/v", 0)
            },
            row("/v/f.md", 1),
        ];
        let root = Path::new("/v");
        assert_eq!(subtree_range(&rows, Path::new("/v/a")), Some(0..4));
        assert_eq!(subtree_range(&rows, Path::new("/v/a/b")), Some(1..3));
        assert_eq!(subtree_range(&rows, Path::new("/v/e")), Some(4..5));
        assert_eq!(
            subtree_range(&rows, root),
            Some(5..7),
            "the root is Unfiled"
        );
        assert_eq!(subtree_range(&rows[..5], root), None, "no Unfiled shown");
        assert_eq!(subtree_range(&rows, Path::new("/v/hidden")), None);
        assert_eq!(subtree_range(&[], root), None);
    }

    #[test]
    fn menus_follow_writer_order_with_separators() {
        assert_eq!(
            labels(&file_menu(PathBuf::from("/v/a.md"), false, false)),
            [
                "Open",
                "Open in new tab",
                "Pin",
                "-",
                "Duplicate",
                "Show Note History",
                "-",
                "Copy relative path",
                "Copy absolute path",
                "-",
                reveal_label(),
                "-",
                "Rename…",
                "Delete"
            ]
        );
        assert_eq!(
            labels(&file_menu(PathBuf::from("/v/a.md"), true, false))[2],
            "Unpin"
        );
        assert_eq!(
            labels(&file_menu(PathBuf::from("/v/a.md"), false, true))[6],
            "Review Conflict"
        );
        assert_eq!(
            labels(&folder_menu(PathBuf::from("/v/d"))),
            [
                "New File",
                "New Folder",
                "-",
                "Copy relative path",
                "Copy absolute path",
                "-",
                "Open in Terminal",
                reveal_label(),
                "-",
                "Rename…",
                "Delete"
            ]
        );
        assert_eq!(
            labels(&bulk_menu(vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ])),
            [
                "Copy 3 relative paths",
                "Copy 3 absolute paths",
                "-",
                "Delete 3 items"
            ]
        );
    }

    #[test]
    fn the_empty_area_menu_checks_the_sort_and_drops_workspace_items_without_one() {
        let items = surface_menu(true, SortMode::ModifiedAsc, true, true, false);
        assert_eq!(
            labels(&items),
            [
                "New File",
                "New Project",
                "-",
                "Open in Terminal",
                open_folder_label(),
                "-",
                "Sort by",
                "-",
                "Search",
                "Recents"
            ]
        );
        let Item::Submenu { items: sort, .. } = &items[6] else {
            panic!("Sort by is a submenu");
        };
        assert_eq!(
            labels(sort),
            [
                "Name (A to Z)",
                "Name (Z to A)",
                "-",
                "Modified time (new to old)",
                "Modified time (old to new)",
                "-",
                "Created time (new to old)",
                "Created time (old to new)",
                "-",
                "Folders first"
            ]
        );
        let checked: Vec<&str> = sort
            .iter()
            .filter_map(|item| match item {
                Item::Action {
                    label,
                    checked: Some(true),
                    ..
                } => Some(label.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(checked, ["Modified time (old to new)", "Folders first"]);
        assert_eq!(
            labels(&surface_menu(false, SortMode::NameAsc, true, true, true)),
            ["Search", "Recents"]
        );
    }

    #[test]
    fn reveal_labels_depend_on_the_os() {
        let (reveal, open) = (reveal_label(), open_folder_label());
        if cfg!(target_os = "windows") {
            assert_eq!((reveal, open), ("Reveal in Explorer", "Open in Explorer"));
        } else if cfg!(target_os = "macos") {
            assert_eq!((reveal, open), ("Reveal in Finder", "Open in Finder"));
        } else {
            assert_eq!((reveal, open), ("Show in Folder", "Open in File Manager"));
        }
    }
}
