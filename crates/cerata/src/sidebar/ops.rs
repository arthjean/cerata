//! Sidebar file operations and menus (US-027): the file, folder, bulk and
//! empty-area menus (`file-context-menu.ts`, `folder-context-menu.ts`,
//! `bulk-context-menu.ts`, `sidebar-surface-context-menu.ts`), inline
//! rename, creation, moves by drag (`use-tree-drag.ts`, `tree-move.ts`),
//! "Reveal in Sidebar" and "Open in Terminal".

use std::ops::Range;
use std::path::{Path, PathBuf};

use cerata_core::fs::{DirEntry, EntryKind, FsError};
use cerata_core::index::relative_slash_path;
use cerata_core::settings::ConfigValue;
use gpui::{AppContext as _, ClipboardItem, Context, Entity, Pixels, Point, Subscription, Window};

use super::tree::{Listing, Row, SortMode, stem};
use super::{Sidebar, SidebarEvent};
use crate::editor::text_field::{FieldEvent, TextField};
use crate::menu::{Item, Menu};

/// A drag starts once the pointer moves this far (`use-tree-drag.ts:32`).
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

// ---- Moves (`tree-move.ts`) ----

/// Where a drop lands: a folder itself, a file's folder, the root for the
/// empty area.
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

/// `false` for a move into the folder the item is already in, or of a
/// folder into itself or one of its descendants (whole components, so a
/// sibling sharing a name prefix is fine).
pub fn can_move(source: &Path, source_is_dir: bool, dest: &Path) -> bool {
    if source.parent() == Some(dest) {
        return false;
    }
    !(source_is_dir && dest.starts_with(source))
}

pub fn move_path(dest: &Path, source: &Path) -> Option<PathBuf> {
    Some(dest.join(source.file_name()?))
}

/// Rows highlighted while hovering `dest`: the folder's row and its
/// visible descendants, every row for the root, `None` if `dest` is not
/// shown.
pub fn drop_range(rows: &[Row], dest: &Path, root: &Path) -> Option<Range<usize>> {
    if rows.is_empty() {
        return None;
    }
    if dest == root {
        return Some(0..rows.len());
    }
    let start = rows.iter().position(|row| row.entry.path == dest)?;
    let depth = rows[start].depth;
    let end = rows[start + 1..]
        .iter()
        .position(|row| row.depth <= depth)
        .map_or(rows.len(), |offset| start + 1 + offset);
    Some(start..end)
}

/// A drag in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub start: Point<Pixels>,
    pub paths: Vec<PathBuf>,
    /// Past the threshold.
    pub active: bool,
    /// The row under the pointer; `None` is the empty area (the root).
    pub over: Option<DirEntry>,
}

pub struct Renaming {
    pub path: PathBuf,
    pub field: Entity<TextField>,
    pub error: Option<String>,
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

    // ---- Menus ----

    /// The menu of a row: the bulk menu inside a selection of two or more,
    /// otherwise the row's own (the selection clears).
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
                file_menu(entry.path.clone(), self.is_pinned(&entry.path))
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

    // ---- Clipboard, reveal, terminal ----

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

    /// Opens a terminal in `dir` (the root when `None`), off the main thread.
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

    // ---- Create and rename ----

    /// "New File" or "New Folder" in `parent` (the root when `None`): the
    /// folder expands, the entry is created and renamed in place, unless
    /// the workspace changed meanwhile (`create-sidebar-entry-and-rename.ts`).
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

    /// Starts renaming `path` in place: the full name of a folder, the stem
    /// of a file, selected.
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
            let mut field = TextField::new("Name", cx);
            field.set_text(&text, cx);
            field
        });
        let events =
            cx.subscribe_in(
                &field,
                window,
                |this, _, event: &FieldEvent, window, cx| match event {
                    FieldEvent::Submit { .. } => this.commit_rename(window, cx),
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
            this.commit_rename(window, cx)
        })
        .detach();
        self.renaming = Some(Renaming {
            path: path.to_path_buf(),
            field,
            error: None,
            _events: events,
        });
        cx.notify();
    }

    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// Enter or blur: the trimmed name, with a file's extension put back.
    /// An empty or unchanged name does nothing; a taken one shows an error
    /// in place and leaves the file alone.
    pub fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = &self.renaming else {
            return;
        };
        let Some(entry) = self.entry_for(&renaming.path) else {
            self.renaming = None;
            return;
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
        if typed.is_empty() || name == entry.name {
            self.cancel_rename(window, cx);
            return;
        }
        let Some(runtime) = self.runtime().cloned() else {
            return;
        };
        let Some(parent) = entry.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let new = parent.join(&name);
        if new.exists() && !same_entry_ignoring_case(&entry.path, &new) {
            let kind = if entry.is_dir { "folder" } else { "file" };
            if let Some(renaming) = &mut self.renaming {
                renaming.error = Some(format!("A {kind} named \"{name}\" already exists."));
            }
            cx.notify();
            return;
        }
        match runtime.fs.rename(&entry.path, &new) {
            Ok(new) => {
                self.renaming = None;
                self.after_move(&entry.path, &new, cx);
                window.focus(&self.focus_handle, cx);
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
    }

    /// Bookkeeping once `old` became `new` on disk: expanded folders,
    /// pins, listings, and the window's tabs (through the event).
    fn after_move(&mut self, old: &Path, new: &Path, cx: &mut Context<Self>) {
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
        // Listings under `old` hold the old paths: they go, any read still
        // running for them goes stale (versions only grow, so a later read
        // of the same path never matches it), and the reopened folders are
        // read again.
        self.listings.retain(|dir, _| !dir.starts_with(old));
        for (dir, version) in &mut self.read_versions {
            if dir.starts_with(old) {
                *version += 1;
            }
        }
        for dir in &reopened {
            self.load_dir(dir, cx);
        }
        if let Some(root) = self.root().map(Path::to_path_buf) {
            match crate::app::core(cx).pins.rewrite_prefix(&root, old, new) {
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
        });
    }

    /// The window deleted `path`: its pins go, its folder is re-read.
    pub fn removed(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.expanded.retain(|dir| !dir.starts_with(path));
        if let Some(root) = self.root().map(Path::to_path_buf) {
            match crate::app::core(cx).pins.remove_prefix(&root, path) {
                Ok(pins) => self.pinned = pins,
                Err(error) => log::warn!("pins not updated: {error}"),
            }
        }
        if let Some(parent) = path.parent().map(Path::to_path_buf) {
            self.load_dir(&parent, cx);
        }
        self.refresh_sections(cx);
    }

    // ---- Drag and drop ----

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
        let root = self.root()?;
        let dest = drop_dest(drag.over.as_ref(), root);
        drop_range(&self.rows, &dest, root)
    }

    /// The release: an active drag moves its items; a press that never
    /// passed the threshold was a click. Returns `true` for a drop.
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
        let mut moved = Vec::new();
        for source in &drag.paths {
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
            match runtime.fs.rename(source, &target) {
                Ok(new) => moved.push((source.clone(), new)),
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
        for (old, new) in moved {
            self.after_move(&old, &new, cx);
        }
        self.selection.clear();
        if !failures.is_empty() {
            cx.emit(SidebarEvent::Notice(format!(
                "Couldn't move {} item(s):\n{}",
                failures.len(),
                failures.join("\n")
            )));
        }
        cx.notify();
        true
    }

    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    // ---- Reveal in Sidebar ----

    /// Expands the parents of `path` and scrolls to its row. A newer call
    /// cancels an older one still waiting for folders to load.
    pub fn reveal_in_sidebar(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root().map(Path::to_path_buf) else {
            return;
        };
        if !path.starts_with(&root) {
            return;
        }
        self.reveal_version += 1;
        self.revealing = Some((path.to_path_buf(), self.reveal_version));
        self.collapsed.remove(&super::Section::Everything);
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

    /// Scrolls to the pending reveal once its row exists.
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

/// On case-insensitive file systems, renaming `a.md` to `A.md` finds the
/// "new" path already existing: it is the same file.
fn same_entry_ignoring_case(old: &Path, new: &Path) -> bool {
    old.to_string_lossy().to_lowercase() == new.to_string_lossy().to_lowercase()
}

fn sep<V>() -> Item<V> {
    Item::Separator
}

pub fn file_menu(path: PathBuf, pinned: bool) -> Vec<Item<Sidebar>> {
    let p = || path.clone();
    let (open, new_tab, pin, dup, rel, abs, reveal, rename, delete) =
        (p(), p(), p(), p(), p(), p(), p(), p(), p());
    vec![
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
        }),
    ]
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
        }),
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
        ),
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
            Item::action("New Folder", |s: &mut Sidebar, window, cx| {
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
            Item::Submenu {
                label: "Sort by".into(),
                items: sort,
            },
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Ports Writer `tree-move.test.ts:61`: a move joins the destination
    /// folder with the entry's name, for a file and for a folder.
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
    fn the_drop_range_spans_the_folder_and_its_visible_descendants() {
        let rows = [
            row("/v/a", 0),
            row("/v/a/b", 1),
            row("/v/a/b/c", 2),
            row("/v/a/d", 1),
            row("/v/e", 0),
        ];
        let root = Path::new("/v");
        assert_eq!(drop_range(&rows, Path::new("/v/a"), root), Some(0..4));
        assert_eq!(drop_range(&rows, Path::new("/v/a/b"), root), Some(1..3));
        assert_eq!(drop_range(&rows, Path::new("/v/e"), root), Some(4..5));
        assert_eq!(drop_range(&rows, root, root), Some(0..5));
        assert_eq!(drop_range(&rows, Path::new("/v/hidden"), root), None);
        assert_eq!(drop_range(&[], root, root), None);
    }

    #[test]
    fn menus_follow_writer_order_with_separators() {
        assert_eq!(
            labels(&file_menu(PathBuf::from("/v/a.md"), false)),
            [
                "Open",
                "Open in new tab",
                "Pin",
                "-",
                "Duplicate",
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
            labels(&file_menu(PathBuf::from("/v/a.md"), true))[2],
            "Unpin"
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
                "New Folder",
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
