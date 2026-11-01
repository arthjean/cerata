//! The sidebar of a workspace window: Pinned, Recents and the whole tree
//! (Everything), virtualized.
//!
//! Ported from Writer `sidebar-navigator.tsx`, `file-tree.tsx`,
//! `use-sidebar-files.ts` and `workspace-store.ts:249-421`. Folder reads run
//! in the background and carry the generation of the workspace they were
//! started for: a read that finishes after a workspace switch is dropped.

pub mod ops;
mod render;
pub mod tree;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cerata_core::fs::titles::extract_title;
use cerata_core::fs::{DirEntry, FsError};
use cerata_core::index::IndexedFile;
use cerata_core::workspace::{WindowState, WorkspaceRuntime};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Task, UniformListScrollHandle, actions,
};

use tree::{LabelMode, Listing, Row, Selection, SortMode, SortOptions};

/// Rows per page of Pinned and Recents (`use-sidebar-files.ts`).
pub const PINNED_PAGE: usize = 6;
pub const RECENTS_PAGE: usize = 4;
/// Recents shows only in workspaces with at least this many files.
pub const RECENTS_MIN_FILES: usize = 10;
pub const ROW_HEIGHT: f32 = 32.0;

pub const WIDTH_KEY: &str = "appearance.sidebar-width";
pub const VISIBLE_KEY: &str = "appearance.sidebar-visible";
const LABEL_KEY: &str = "appearance.sidebar-file-label";
const SORT_KEY: &str = "appearance.sidebar-sort";
const FOLDERS_FIRST_KEY: &str = "appearance.sidebar-folders-first";
const SHOW_RECENTS_KEY: &str = "appearance.sidebar-show-recents";
const SHOW_SEARCH_KEY: &str = "appearance.sidebar-show-search";

actions!(sidebar, [ClearSelection]);

pub fn bindings() -> Vec<gpui::KeyBinding> {
    vec![gpui::KeyBinding::new(
        "escape",
        ClearSelection,
        Some("Sidebar"),
    )]
}

/// What the sidebar asks its window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    Open(PathBuf),
    OpenInNewTab(PathBuf),
    Renamed {
        old: PathBuf,
        new: PathBuf,
    },
    Notice(String),
    /// The window decides: it knows which files have unsaved changes.
    Delete(Vec<PathBuf>),
    Duplicate(PathBuf),
    /// A sidebar setting changed (sort, sections).
    SettingsChanged,
    /// The Search button.
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Pinned,
    Recents,
    Everything,
}

pub struct Sidebar {
    window: Arc<WindowState>,
    runtime: Option<Arc<WorkspaceRuntime>>,
    /// Bumped on every workspace change; stale reads compare it.
    generation: u64,
    pub root_listing: Listing,
    pub listings: HashMap<PathBuf, Listing>,
    pub expanded: HashSet<PathBuf>,
    pub rows: Vec<Row>,
    pub options: SortOptions,
    pub selection: Selection,
    pub active_path: Option<PathBuf>,
    pub pinned: Vec<PathBuf>,
    pub pinned_entries: Vec<DirEntry>,
    pub pinned_visible: usize,
    pub recents: Vec<DirEntry>,
    pub recents_visible: usize,
    pub recents_more: bool,
    pub show_recents: bool,
    pub collapsed: HashSet<Section>,
    /// Bumped by every section refresh: only the latest one is applied.
    sections_version: u64,
    /// Latest read of each folder: an older read finishing last is dropped.
    read_versions: HashMap<PathBuf, u64>,
    pub menu: Option<crate::menu::Menu<Sidebar>>,
    pub renaming: Option<ops::Renaming>,
    pub drag: Option<ops::Drag>,
    /// A drop just ended: the click that follows the release is ignored.
    suppress_click: bool,
    /// A created entry to rename once its row is shown.
    pending_rename: Option<PathBuf>,
    reveal_version: u64,
    revealing: Option<(PathBuf, u64)>,
    pub focus_handle: FocusHandle,
    pub scroll: UniformListScrollHandle,
    /// Where each shown row was drawn last frame: the keyboard opens a
    /// row's menu (Shift+F10) under it.
    pub row_bounds: std::rc::Rc<std::cell::RefCell<HashMap<PathBuf, gpui::Bounds<gpui::Pixels>>>>,
    tasks: Vec<Task<()>>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl crate::menu::HasMenu for Sidebar {
    fn menu(&mut self) -> &mut Option<crate::menu::Menu<Self>> {
        &mut self.menu
    }
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Sidebar {
    pub fn new(window: Arc<WindowState>, cx: &mut Context<Self>) -> Self {
        let mut sidebar = Self {
            window,
            runtime: None,
            generation: 0,
            root_listing: Listing::Loading,
            listings: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            options: SortOptions::default(),
            selection: Selection::default(),
            active_path: None,
            pinned: Vec::new(),
            pinned_entries: Vec::new(),
            pinned_visible: PINNED_PAGE,
            recents: Vec::new(),
            recents_visible: RECENTS_PAGE,
            recents_more: false,
            show_recents: true,
            collapsed: HashSet::new(),
            sections_version: 0,
            read_versions: HashMap::new(),
            menu: None,
            renaming: None,
            drag: None,
            suppress_click: false,
            pending_rename: None,
            reveal_version: 0,
            revealing: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            row_bounds: Default::default(),
            tasks: Vec::new(),
        };
        sidebar.read_settings();
        sidebar.workspace_changed(cx);
        sidebar
    }

    pub fn root(&self) -> Option<&Path> {
        self.runtime.as_ref().map(|runtime| runtime.root.as_path())
    }

    pub fn window_state(&self) -> &Arc<WindowState> {
        &self.window
    }

    pub fn runtime(&self) -> Option<&Arc<WorkspaceRuntime>> {
        self.runtime.as_ref()
    }

    fn setting_str(&self, key: &str) -> Option<String> {
        self.window
            .settings()
            .get(key)
            .and_then(|value| value.as_str().map(str::to_owned))
    }

    pub(crate) fn setting_bool(&self, key: &str) -> Option<bool> {
        self.window
            .settings()
            .get(key)
            .and_then(|value| value.as_bool())
    }

    /// Sort, label and section options from the settings.
    pub fn read_settings(&mut self) {
        self.options = SortOptions {
            mode: SortMode::parse(self.setting_str(SORT_KEY).as_deref()),
            folders_first: self.setting_bool(FOLDERS_FIRST_KEY).unwrap_or(true),
            label: LabelMode::parse(self.setting_str(LABEL_KEY).as_deref()),
        };
        self.show_recents = self.setting_bool(SHOW_RECENTS_KEY).unwrap_or(true);
    }

    /// Settings changed: re-sort and re-show without reading the disk.
    pub fn settings_changed(&mut self, cx: &mut Context<Self>) {
        self.read_settings();
        self.rebuild_rows();
        self.refresh_sections(cx);
        cx.notify();
    }

    /// The window opened, switched or closed its workspace.
    pub fn workspace_changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.runtime = self.window.runtime();
        self.root_listing = Listing::Loading;
        self.listings.clear();
        self.read_versions.clear();
        self.expanded.clear();
        self.selection.clear();
        self.rows.clear();
        self.menu = None;
        self.renaming = None;
        self.drag = None;
        self.pending_rename = None;
        self.revealing = None;
        self.pinned_visible = PINNED_PAGE;
        self.recents_visible = RECENTS_PAGE;
        self.tasks.clear();
        self.pinned = match self.root() {
            Some(root) => crate::app::core(cx)
                .pins
                .load(root)
                .unwrap_or_else(|error| {
                    log::warn!("pinned files unavailable: {error}");
                    Vec::new()
                }),
            None => Vec::new(),
        };
        if let Some(root) = self.root().map(Path::to_path_buf) {
            self.load_dir(&root, cx);
        }
        self.refresh_sections(cx);
        cx.notify();
    }

    pub fn set_active_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.active_path != path {
            self.active_path = path;
            cx.notify();
        }
    }

    // ---- Listings ----

    /// Reads `dir` in the background. Titles already known for an unchanged
    /// file are kept; a folder with few unknown titles is shown with all of
    /// them, a larger one is shown at once by file name and its titles fill
    /// in by chunks, so a huge folder never holds the tree back.
    pub fn load_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let generation = self.generation;
        let dir = dir.to_path_buf();
        let is_root = dir == runtime.root;
        let known = known_titles(self.listing(&dir));
        if !is_root && !matches!(self.listings.get(&dir), Some(Listing::Loaded(_))) {
            self.listings.insert(dir.clone(), Listing::Loading);
        }
        let version = {
            let slot = self.read_versions.entry(dir.clone()).or_default();
            *slot += 1;
            *slot
        };
        let task = cx.spawn(async move |this, cx| {
            let read_dir = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move { read_listing(&runtime, &read_dir, &known) })
                .await;
            let current = |this: &Self| {
                this.generation == generation && this.read_versions.get(&dir) == Some(&version)
            };
            let Ok(Some(untitled)) = this.update(cx, |this, cx| {
                if !current(this) {
                    return None;
                }
                let (listing, untitled) = match result {
                    Ok((entries, untitled)) => (Listing::Loaded(entries), untitled),
                    Err(error) => {
                        log::warn!("cannot list {}: {error}", dir.display());
                        (Listing::Failed(error.to_string()), Vec::new())
                    }
                };
                if is_root {
                    this.root_listing = listing;
                } else {
                    this.listings.insert(dir.clone(), listing);
                }
                this.rebuild_rows();
                this.finish_reveal(cx);
                cx.notify();
                Some(untitled)
            }) else {
                return;
            };
            // Chunks double, so a huge folder is re-sorted a handful of times.
            let mut rest = untitled.as_slice();
            let mut size = TITLE_CHUNK;
            while !rest.is_empty() {
                let (chunk, next) = rest.split_at(size.min(rest.len()));
                rest = next;
                size *= 2;
                let paths = chunk.to_vec();
                let titles: HashMap<PathBuf, String> = cx
                    .background_executor()
                    .spawn(async move {
                        paths
                            .into_iter()
                            .filter_map(|path| extract_title(&path).map(|title| (path, title)))
                            .collect()
                    })
                    .await;
                let applied = this.update(cx, |this, cx| {
                    if !current(this) {
                        return false;
                    }
                    if !titles.is_empty() {
                        this.set_titles(&dir, &titles);
                        this.rebuild_rows();
                        cx.notify();
                    }
                    true
                });
                if !matches!(applied, Ok(true)) {
                    return;
                }
            }
        });
        self.tasks.push(task);
    }

    fn listing(&self, dir: &Path) -> Option<&Listing> {
        match self.root() {
            Some(root) if root == dir => Some(&self.root_listing),
            _ => self.listings.get(dir),
        }
    }

    fn set_titles(&mut self, dir: &Path, titles: &HashMap<PathBuf, String>) {
        let listing = match self.root() {
            Some(root) if root == dir => Some(&mut self.root_listing),
            _ => self.listings.get_mut(dir),
        };
        if let Some(Listing::Loaded(entries)) = listing {
            for entry in entries {
                if let Some(title) = titles.get(&entry.path) {
                    entry.title = Some(title.clone());
                }
            }
        }
    }

    pub fn rebuild_rows(&mut self) {
        let root: &[DirEntry] = match &self.root_listing {
            Listing::Loaded(entries) => entries,
            Listing::Loading | Listing::Failed(_) => &[],
        };
        self.rows = tree::flatten(root, &self.expanded, &self.listings, self.options);
        self.selection.retain_visible(&self.rows);
    }

    /// Expands or collapses a folder. Collapsing is immediate; expanding
    /// reads the folder unless it is already loaded, and retries a folder
    /// whose last read failed.
    pub fn toggle_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if matches!(self.listings.get(dir), Some(Listing::Failed(_))) {
            // Shown collapsed with its error: this press expands again.
            self.expanded.remove(dir);
        }
        if self.expanded.remove(dir) {
            self.rebuild_rows();
            cx.notify();
            return;
        }
        self.expanded.insert(dir.to_path_buf());
        if !matches!(self.listings.get(dir), Some(Listing::Loaded(_))) {
            self.load_dir(dir, cx);
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub fn expand(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.expanded.contains(dir) {
            self.toggle_dir(dir, cx);
        }
    }

    /// A folder's listing changed on disk: re-read it if it is shown.
    pub fn refresh_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let shown = self.root() == Some(dir) || self.listings.contains_key(dir);
        if shown {
            self.load_dir(dir, cx);
        }
    }

    /// Re-reads every listing shown (after an index rebuild).
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let mut dirs: Vec<PathBuf> = self.listings.keys().cloned().collect();
        dirs.extend(self.root().map(Path::to_path_buf));
        for dir in dirs {
            self.load_dir(&dir, cx);
        }
        self.refresh_sections(cx);
    }

    // ---- Sections ----

    /// Reloads Pinned and Recents for their current page sizes.
    pub fn refresh_sections(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            self.pinned_entries.clear();
            self.recents.clear();
            self.recents_more = false;
            return;
        };
        let wanted: Vec<PathBuf> = self
            .pinned
            .iter()
            .take(self.pinned_visible + 1)
            .cloned()
            .collect();
        let pinned: HashSet<PathBuf> = self.pinned.iter().cloned().collect();
        let recents_visible = self.recents_visible;
        let generation = self.generation;
        self.sections_version += 1;
        let version = self.sections_version;
        let task = cx.spawn(async move |this, cx| {
            let (entries, recents, more) = cx
                .background_executor()
                .spawn(async move {
                    let entries = with_titles(runtime.fs.file_entries(&wanted));
                    let (recents, more) = load_recents(&runtime, &pinned, recents_visible);
                    (entries, recents, more)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.sections_version != version {
                    return;
                }
                this.pinned_entries = entries;
                this.recents = recents;
                this.recents_more = more;
                cx.notify();
            });
        });
        self.tasks.push(task);
    }

    pub fn pinned_has_more(&self) -> bool {
        self.pinned.len() > self.pinned_visible || self.pinned_entries.len() > self.pinned_visible
    }

    pub fn show_more(&mut self, section: Section, cx: &mut Context<Self>) {
        match section {
            Section::Pinned => self.pinned_visible += PINNED_PAGE,
            Section::Recents => self.recents_visible += RECENTS_PAGE,
            Section::Everything => return,
        }
        self.refresh_sections(cx);
    }

    pub fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
        cx.notify();
    }

    pub fn is_pinned(&self, path: &Path) -> bool {
        self.pinned.iter().any(|p| p == path)
    }

    pub fn toggle_pin(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root().map(Path::to_path_buf) else {
            return;
        };
        match crate::app::core(cx).pins.toggle(&root, path) {
            Ok(pins) => self.pinned = pins,
            Err(error) => {
                cx.emit(SidebarEvent::Notice(format!("Failed to pin: {error}")));
                return;
            }
        }
        self.refresh_sections(cx);
        cx.notify();
    }

    // ---- Selection and activation ----

    /// A press on a row with the modifiers of the event.
    pub fn press_row(&mut self, path: &Path, shift: bool, secondary: bool, cx: &mut Context<Self>) {
        if shift {
            self.selection.extend(path, &self.rows);
        } else if secondary {
            self.selection.toggle(path);
        } else if !(self.selection.len() >= 2 && self.selection.contains(path)) {
            self.selection.press(path);
        }
        cx.notify();
    }

    /// A plain click (no drag): the selection clears, a folder toggles, a
    /// file opens.
    pub fn click_row(&mut self, entry: &DirEntry, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.suppress_click)
            || self.drag.as_ref().is_some_and(|drag| drag.active)
        {
            return;
        }
        self.selection.press(&entry.path);
        if entry.is_dir {
            self.toggle_dir(&entry.path, cx);
        } else {
            cx.emit(SidebarEvent::Open(entry.path.clone()));
        }
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() || self.selection.anchor.is_some() {
            self.selection.clear();
            cx.notify();
        }
    }
}

/// Unknown titles past this count are read after the folder is shown.
const TITLE_CHUNK: usize = 512;

/// Titles of a loaded listing's files, by path and modified time. A file
/// without one is left out: its title may simply not be read yet.
fn known_titles(listing: Option<&Listing>) -> HashMap<PathBuf, (u64, String)> {
    match listing {
        Some(Listing::Loaded(entries)) => entries
            .iter()
            .filter_map(|entry| {
                let title = entry.title.clone()?;
                Some((entry.path.clone(), (entry.modified_at, title)))
            })
            .collect(),
        _ => HashMap::new(),
    }
}

/// One folder's entries. Titles come from `known` while the file is
/// unchanged and settled, else are read here when at most [`TITLE_CHUNK`] are missing;
/// otherwise the paths still to read are returned.
fn read_listing(
    runtime: &WorkspaceRuntime,
    dir: &Path,
    known: &HashMap<PathBuf, (u64, String)>,
) -> Result<(Vec<DirEntry>, Vec<PathBuf>), FsError> {
    let ignore = runtime.ignore.read().clone();
    let mut entries = if runtime
        .index_ready
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        let index = runtime.index.lock();
        runtime.fs.read_directory(dir, &ignore, Some(&*index))
    } else {
        runtime.fs.read_directory(dir, &ignore, None)
    }?;
    // Modified times are in seconds: a file changed within the last two
    // may have changed again since its title was read, so it is read anew.
    let settled = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |now| now.as_secs().saturating_sub(2));
    let mut untitled = Vec::new();
    for entry in entries.iter_mut().filter(|entry| entry.is_markdown) {
        match known.get(&entry.path) {
            Some((modified_at, title))
                if *modified_at == entry.modified_at && *modified_at < settled =>
            {
                entry.title = Some(title.clone());
            }
            _ => untitled.push(entry.path.clone()),
        }
    }
    if untitled.len() <= TITLE_CHUNK {
        let untitled: HashSet<PathBuf> = untitled.drain(..).collect();
        for entry in entries
            .iter_mut()
            .filter(|entry| untitled.contains(&entry.path))
        {
            entry.title = extract_title(&entry.path);
        }
    }
    Ok((entries, untitled))
}

fn with_titles(mut entries: Vec<DirEntry>) -> Vec<DirEntry> {
    for entry in &mut entries {
        if entry.is_markdown && entry.title.is_none() {
            entry.title = extract_title(&entry.path);
        }
    }
    entries
}

/// Recents for `visible` rows, pinned files left out: pages of 100 are
/// read until one more than `visible` rows are collected.
fn load_recents(
    runtime: &WorkspaceRuntime,
    pinned: &HashSet<PathBuf>,
    visible: usize,
) -> (Vec<DirEntry>, bool) {
    let index = runtime.index.lock();
    if index.len() < RECENTS_MIN_FILES {
        return (Vec::new(), false);
    }
    let mut collected: Vec<IndexedFile> = Vec::new();
    let mut offset = 0;
    loop {
        let batch = index.recent(100, offset);
        let count = batch.len();
        collected.extend(
            batch
                .into_iter()
                .filter(|file| !pinned.contains(&file.path)),
        );
        offset += 100;
        if collected.len() > visible || count == 0 || offset >= index.len() {
            break;
        }
    }
    drop(index);
    let more = collected.len() > visible;
    collected.truncate(visible);
    let entries = collected
        .into_iter()
        .map(|file| DirEntry {
            name: file.name.clone(),
            is_dir: false,
            is_markdown: true,
            modified_at: file.modified_at,
            created_at: cerata_core::fs::created_time(&file.path),
            title: extract_title(&file.path),
            path: file.path,
        })
        .collect();
    (entries, more)
}
