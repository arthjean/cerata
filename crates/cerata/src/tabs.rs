//! Tabs and their own back and forward history, as plain data.
//!
//! Ported from Writer `editor-store.ts:342-1106` (tab half; the documents
//! are in [`crate::documents`]): there is always at least one tab, opening a
//! file replaces a launcher or navigates the active tab, closing activates
//! the right neighbor, and path rewrites and removals match whole path
//! components, so `/a` never touches `/a-b`.

use std::path::{Path, PathBuf};

use cerata_core::session::{SerializedLocation, SessionData, SessionTab};
use serde::{Deserialize, Serialize};

/// What a tab shows. Serialized with a `kind` tag, Writer's session format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Location {
    File { path: PathBuf },
    Launcher,
    Settings,
}

impl Location {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File { path: path.into() }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path } => Some(path),
            Self::Launcher | Self::Settings => None,
        }
    }

    fn to_serialized(&self) -> Option<SerializedLocation> {
        serde_json::to_value(self)
            .and_then(serde_json::from_value)
            .ok()
    }

    /// `None` for a kind this version does not know (dropped on restore,
    /// `page-kinds/index.ts:13-47`).
    fn from_serialized(location: &SerializedLocation) -> Option<Self> {
        serde_json::to_value(location)
            .and_then(serde_json::from_value)
            .ok()
    }

    /// `path` itself or anything under it, by whole components.
    fn is_under(&self, prefix: &Path) -> bool {
        self.path().is_some_and(|path| path.starts_with(prefix))
    }

    /// Moves `old` (or anything under it) to `new`; `true` if it matched.
    fn rewrite(&mut self, old: &Path, new: &Path) -> bool {
        let Self::File { path } = self else {
            return false;
        };
        let Ok(rest) = path.strip_prefix(old) else {
            return false;
        };
        *path = if rest.as_os_str().is_empty() {
            new.to_path_buf()
        } else {
            new.join(rest)
        };
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: u64,
    pub location: Location,
    pub back: Vec<Location>,
    pub forward: Vec<Location>,
    /// Increases on every navigation of this tab: a load that finishes
    /// after a newer navigation may not roll it back.
    pub version: u64,
}

impl Tab {
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

/// The state of a tab before a navigation, to put back if the file of the
/// navigation cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undo {
    pub id: u64,
    pub version: u64,
    /// `None`: the tab did not exist before (it is closed on failure).
    pub previous: Option<Tab>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

impl Tabs {
    /// One launcher tab.
    pub fn new() -> Self {
        let mut tabs = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
        };
        tabs.ensure_launcher();
        tabs
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Tab {
        // Invariant: never empty, `active` always in range.
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    fn active_mut(&mut self) -> &mut Tab {
        let ix = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[ix]
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active().location.path()
    }

    fn tab(&mut self, location: Location) -> Tab {
        let id = self.next_id;
        self.next_id += 1;
        Tab {
            id,
            location,
            back: Vec::new(),
            forward: Vec::new(),
            version: 0,
        }
    }

    fn ensure_launcher(&mut self) {
        if self.tabs.is_empty() {
            let tab = self.tab(Location::Launcher);
            self.tabs.push(tab);
            self.active = 0;
        }
    }

    pub fn activate(&mut self, index: usize) -> bool {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            return true;
        }
        false
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab, wrapping around.
    pub fn cycle(&mut self, forward: bool) -> bool {
        let len = self.tabs.len();
        if len < 2 {
            return false;
        }
        self.active = if forward {
            (self.active + 1) % len
        } else {
            (self.active + len - 1) % len
        };
        true
    }

    /// Opens `path` the way a click does: replaces an active launcher (the
    /// tab keeps its id), navigates an active file tab, and adds a tab next
    /// to anything else. `None` when the active tab already shows `path`.
    pub fn open(&mut self, path: &Path) -> Option<Undo> {
        let location = Location::file(path);
        match &self.active().location {
            Location::Launcher => {
                let previous = self.active().clone();
                let tab = self.active_mut();
                tab.location = location;
                tab.version += 1;
                Some(Undo {
                    id: tab.id,
                    version: tab.version,
                    previous: Some(previous),
                })
            }
            Location::File { path: current } if current == path => None,
            Location::File { .. } => Some(self.navigate(location)),
            Location::Settings => Some(self.open_in_new_tab(path)),
        }
    }

    /// "Open in new tab": always a fresh tab, even for an open file.
    pub fn open_in_new_tab(&mut self, path: &Path) -> Undo {
        let mut tab = self.tab(Location::file(path));
        tab.version = 1;
        let undo = Undo {
            id: tab.id,
            version: tab.version,
            previous: None,
        };
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        undo
    }

    pub fn open_launcher(&mut self) {
        let tab = self.tab(Location::Launcher);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    /// Activates the settings tab, adding one if none exists.
    pub fn open_settings(&mut self) {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.location == Location::Settings)
        {
            self.active = ix;
            return;
        }
        let tab = self.tab(Location::Settings);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    fn navigate(&mut self, location: Location) -> Undo {
        let previous = self.active().clone();
        let tab = self.active_mut();
        let current = std::mem::replace(&mut tab.location, location);
        tab.back.push(current);
        tab.forward.clear();
        tab.version += 1;
        Undo {
            id: tab.id,
            version: tab.version,
            previous: Some(previous),
        }
    }

    pub fn back(&mut self) -> Option<Undo> {
        let previous = self.active().clone();
        let tab = self.active_mut();
        let target = tab.back.pop()?;
        let current = std::mem::replace(&mut tab.location, target);
        tab.forward.insert(0, current);
        tab.version += 1;
        Some(Undo {
            id: tab.id,
            version: tab.version,
            previous: Some(previous),
        })
    }

    pub fn forward(&mut self) -> Option<Undo> {
        let previous = self.active().clone();
        let tab = self.active_mut();
        if tab.forward.is_empty() {
            return None;
        }
        let target = tab.forward.remove(0);
        let current = std::mem::replace(&mut tab.location, target);
        tab.back.push(current);
        tab.version += 1;
        Some(Undo {
            id: tab.id,
            version: tab.version,
            previous: Some(previous),
        })
    }

    /// Puts a tab back as it was before a navigation whose file failed to
    /// load, unless the tab navigated again since. `true` if applied.
    pub fn rollback(&mut self, undo: &Undo) -> bool {
        let Some(ix) = self.index_of(undo.id) else {
            return false;
        };
        if self.tabs[ix].version != undo.version {
            return false;
        }
        match &undo.previous {
            Some(previous) => {
                let version = self.tabs[ix].version + 1;
                self.tabs[ix] = Tab {
                    version,
                    ..previous.clone()
                };
            }
            None => {
                self.close(ix);
            }
        }
        true
    }

    /// Closes the tab at `index`. The active tab passes to the right
    /// neighbor, else the left one; closing the last tab leaves a launcher.
    pub fn close(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let closed = self.tabs.remove(index);
        if index < self.active {
            self.active -= 1;
        } else if index == self.active && self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        self.ensure_launcher();
        Some(closed)
    }

    pub fn close_others(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let keep = self.tabs.remove(index);
        self.tabs = vec![keep];
        self.active = 0;
    }

    pub fn close_all(&mut self) {
        self.tabs.clear();
        self.ensure_launcher();
    }

    /// Replaces every tab with one tab on `path` (compact windows).
    pub fn replace_all(&mut self, path: &Path) {
        let mut tab = self.tab(Location::file(path));
        tab.version = 1;
        self.tabs = vec![tab];
        self.active = 0;
    }

    /// `true` if only launchers are open (Cmd+W then closes the window).
    pub fn only_launchers(&self) -> bool {
        self.tabs.len() <= 1 && self.active().location == Location::Launcher
    }

    /// A file or folder was renamed or moved: rewrites tab locations and
    /// histories at `old` or under it. Returns `true` if anything changed.
    pub fn rewrite_prefix(&mut self, old: &Path, new: &Path) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            changed |= tab.location.rewrite(old, new);
            for location in tab.back.iter_mut().chain(tab.forward.iter_mut()) {
                changed |= location.rewrite(old, new);
            }
        }
        changed
    }

    /// A file or folder is gone: closes the tabs showing it or anything
    /// under it and strips it from every history. `true` if anything changed.
    pub fn remove_prefix(&mut self, prefix: &Path) -> bool {
        let before = self.clone();
        let active_id = self.active().id;
        self.tabs.retain(|tab| !tab.location.is_under(prefix));
        for tab in &mut self.tabs {
            tab.back.retain(|location| !location.is_under(prefix));
            tab.forward.retain(|location| !location.is_under(prefix));
        }
        self.active = self.index_of(active_id).unwrap_or(0);
        self.ensure_launcher();
        *self != before
    }

    /// Every file a tab shows or can go back or forward to.
    pub fn referenced_paths(&self) -> impl Iterator<Item = &Path> {
        self.tabs.iter().flat_map(|tab| {
            std::iter::once(&tab.location)
                .chain(&tab.back)
                .chain(&tab.forward)
                .filter_map(Location::path)
        })
    }

    pub fn references(&self, path: &Path) -> bool {
        self.referenced_paths().any(|p| p == path)
    }

    /// The session to save: launchers are left out, and the active index
    /// is `None` when the active tab is a launcher.
    pub fn to_session(&self) -> SessionData {
        let mut tabs = Vec::new();
        let mut active_index = None;
        for (ix, tab) in self.tabs.iter().enumerate() {
            if tab.location == Location::Launcher {
                continue;
            }
            let Some(location) = tab.location.to_serialized() else {
                continue;
            };
            if ix == self.active {
                active_index = Some(tabs.len());
            }
            tabs.push(SessionTab {
                location,
                back: tab
                    .back
                    .iter()
                    .filter_map(Location::to_serialized)
                    .collect(),
                forward: tab
                    .forward
                    .iter()
                    .filter_map(Location::to_serialized)
                    .collect(),
            });
        }
        SessionData { tabs, active_index }
    }

    /// Tabs from a saved session, with fresh ids; unknown kinds dropped.
    pub fn from_session(session: &SessionData) -> Self {
        let mut tabs = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
        };
        let mut active = 0;
        for (ix, saved) in session.tabs.iter().enumerate() {
            let Some(location) = Location::from_serialized(&saved.location) else {
                continue;
            };
            if Some(ix) == session.active_index {
                active = tabs.tabs.len();
            }
            let mut tab = tabs.tab(location);
            tab.back = saved
                .back
                .iter()
                .filter_map(Location::from_serialized)
                .collect();
            tab.forward = saved
                .forward
                .iter()
                .filter_map(Location::from_serialized)
                .collect();
            tabs.tabs.push(tab);
        }
        tabs.active = active.min(tabs.tabs.len().saturating_sub(1));
        tabs.ensure_launcher();
        tabs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn locations(tabs: &Tabs) -> Vec<Location> {
        tabs.tabs().iter().map(|t| t.location.clone()).collect()
    }

    fn with_files(paths: &[&str]) -> Tabs {
        let mut tabs = Tabs::new();
        tabs.open(&p(paths[0]));
        for path in &paths[1..] {
            tabs.open_in_new_tab(&p(path));
        }
        tabs
    }

    #[test]
    fn there_is_always_a_tab_and_closing_the_last_leaves_a_launcher() {
        let mut tabs = Tabs::new();
        assert_eq!(locations(&tabs), [Location::Launcher]);
        tabs.close(0);
        assert_eq!(locations(&tabs), [Location::Launcher]);
        let mut tabs = with_files(&["/a.md"]);
        tabs.close(0);
        assert_eq!(locations(&tabs), [Location::Launcher]);
    }

    #[test]
    fn open_launcher_appends_and_activates_it() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open_launcher();
        assert_eq!(tabs.tabs().len(), 2);
        assert_eq!(tabs.active().location, Location::Launcher);
    }

    #[test]
    fn closing_the_active_tab_activates_the_right_neighbor_else_the_left() {
        let mut tabs = with_files(&["/a.md", "/b.md", "/c.md"]);
        tabs.activate(1);
        tabs.close(1);
        assert_eq!(tabs.active_path(), Some(p("/c.md").as_path()));
        tabs.close(1);
        assert_eq!(tabs.active_path(), Some(p("/a.md").as_path()));
    }

    #[test]
    fn closing_a_tab_left_of_the_active_one_keeps_it_active() {
        let mut tabs = with_files(&["/a.md", "/b.md", "/c.md"]);
        tabs.activate(2);
        tabs.close(0);
        assert_eq!(tabs.active_path(), Some(p("/c.md").as_path()));
    }

    #[test]
    fn opening_from_a_launcher_reuses_its_tab_id() {
        let mut tabs = Tabs::new();
        let id = tabs.active().id;
        tabs.open(&p("/a.md"));
        assert_eq!(tabs.tabs().len(), 1);
        assert_eq!(tabs.active().id, id);
        assert_eq!(tabs.active_path(), Some(p("/a.md").as_path()));
    }

    #[test]
    fn opening_an_already_open_file_from_a_launcher_makes_a_second_tab() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open_launcher();
        tabs.open(&p("/a.md"));
        assert_eq!(
            locations(&tabs),
            [Location::file("/a.md"), Location::file("/a.md")]
        );
    }

    #[test]
    fn opening_navigates_the_active_file_tab_and_records_history() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open(&p("/b.md"));
        assert_eq!(tabs.tabs().len(), 1);
        let tab = tabs.active();
        assert_eq!(tab.back, [Location::file("/a.md")]);
        assert!(tab.forward.is_empty());
        assert!(tabs.open(&p("/b.md")).is_none(), "same path is a no-op");
    }

    #[test]
    fn back_and_forward_are_per_tab() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open(&p("/b.md"));
        tabs.open(&p("/c.md"));
        tabs.open_in_new_tab(&p("/x.md"));
        assert!(tabs.back().is_none(), "the new tab has no history");
        tabs.activate(0);
        tabs.back();
        assert_eq!(tabs.active_path(), Some(p("/b.md").as_path()));
        assert_eq!(tabs.active().forward, [Location::file("/c.md")]);
        tabs.back();
        assert_eq!(tabs.active_path(), Some(p("/a.md").as_path()));
        assert_eq!(
            tabs.active().forward,
            [Location::file("/b.md"), Location::file("/c.md")]
        );
        tabs.forward();
        assert_eq!(tabs.active_path(), Some(p("/b.md").as_path()));
        tabs.open(&p("/d.md"));
        assert!(
            tabs.active().forward.is_empty(),
            "a new navigation drops forward"
        );
        assert_eq!(tabs.tabs()[1].location, Location::file("/x.md"));
    }

    #[test]
    fn open_in_new_tab_always_adds_a_tab() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open_in_new_tab(&p("/a.md"));
        assert_eq!(tabs.tabs().len(), 2);
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn a_failed_new_tab_is_closed_and_a_failed_navigation_rolled_back() {
        let mut tabs = with_files(&["/a.md"]);
        let undo = tabs.open_in_new_tab(&p("/gone.md"));
        assert!(tabs.rollback(&undo));
        assert_eq!(locations(&tabs), [Location::file("/a.md")]);

        let undo = tabs.open(&p("/gone.md")).unwrap();
        assert!(tabs.rollback(&undo));
        assert_eq!(tabs.active_path(), Some(p("/a.md").as_path()));
        assert!(tabs.active().back.is_empty());
    }

    #[test]
    fn a_stale_load_failure_cannot_undo_a_newer_navigation() {
        let mut tabs = with_files(&["/a.md"]);
        let slow = tabs.open(&p("/slow.md")).unwrap();
        tabs.open(&p("/c.md"));
        assert!(!tabs.rollback(&slow));
        assert_eq!(tabs.active_path(), Some(p("/c.md").as_path()));
    }

    #[test]
    fn a_failed_launcher_replacement_restores_the_launcher() {
        let mut tabs = Tabs::new();
        let undo = tabs.open(&p("/gone.md")).unwrap();
        assert!(tabs.rollback(&undo));
        assert_eq!(locations(&tabs), [Location::Launcher]);
    }

    #[test]
    fn settings_has_a_single_tab() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open_settings();
        tabs.activate(0);
        tabs.open_settings();
        assert_eq!(tabs.tabs().len(), 2);
        assert_eq!(tabs.active().location, Location::Settings);
        tabs.open(&p("/b.md"));
        assert_eq!(tabs.tabs().len(), 3, "a file never replaces settings");
    }

    #[test]
    fn cycling_wraps_around() {
        let mut tabs = with_files(&["/a.md", "/b.md"]);
        assert!(tabs.cycle(true));
        assert_eq!(tabs.active_index(), 0);
        assert!(tabs.cycle(false));
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn replace_all_leaves_one_file_tab() {
        let mut tabs = with_files(&["/a.md", "/b.md"]);
        tabs.replace_all(&p("/c.md"));
        assert_eq!(locations(&tabs), [Location::file("/c.md")]);
    }

    #[test]
    fn removing_a_path_closes_its_tabs_and_strips_histories() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open(&p("/gone.md"));
        tabs.open(&p("/b.md"));
        tabs.open_in_new_tab(&p("/gone.md"));
        tabs.open_in_new_tab(&p("/c.md"));
        assert!(tabs.remove_prefix(&p("/gone.md")));
        assert_eq!(
            locations(&tabs),
            [Location::file("/b.md"), Location::file("/c.md")]
        );
        assert_eq!(tabs.tabs()[0].back, [Location::file("/a.md")]);
        assert_eq!(tabs.active_path(), Some(p("/c.md").as_path()));
    }

    #[test]
    fn removing_the_last_file_tab_leaves_a_launcher() {
        let mut tabs = with_files(&["/dir/a.md", "/dir/sub/b.md"]);
        assert!(tabs.remove_prefix(&p("/dir")));
        assert_eq!(locations(&tabs), [Location::Launcher]);
    }

    #[test]
    fn prefixes_match_whole_components_only() {
        let mut tabs = with_files(&["/a/x.md", "/a-b/y.md"]);
        tabs.open(&p("/a/z.md"));
        assert!(tabs.rewrite_prefix(&p("/a"), &p("/n")));
        assert_eq!(
            locations(&tabs),
            [Location::file("/n/x.md"), Location::file("/n/z.md")]
        );
        assert_eq!(tabs.tabs()[1].back, [Location::file("/a-b/y.md")]);
        assert!(!tabs.remove_prefix(&p("/a")));
        assert_eq!(tabs.tabs().len(), 2);
    }

    #[test]
    fn renaming_rewrites_locations_and_histories() {
        let mut tabs = with_files(&["/v/old.md"]);
        tabs.open(&p("/v/other.md"));
        tabs.open_in_new_tab(&p("/v/old.md"));
        tabs.open_in_new_tab(&p("/v/old.md.bak"));
        assert!(tabs.rewrite_prefix(&p("/v/old.md"), &p("/v/new.md")));
        assert_eq!(tabs.tabs()[0].back, [Location::file("/v/new.md")]);
        assert_eq!(tabs.tabs()[1].location, Location::file("/v/new.md"));
        assert_eq!(tabs.tabs()[2].location, Location::file("/v/old.md.bak"));
        assert!(!tabs.rewrite_prefix(&p("/nowhere"), &p("/x")));
    }

    #[test]
    fn the_session_leaves_launchers_out() {
        let mut tabs = with_files(&["/a.md"]);
        tabs.open(&p("/b.md"));
        tabs.open_launcher();
        let session = tabs.to_session();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.active_index, None);
        assert_eq!(session.tabs[0].location.kind, "file");
        assert_eq!(session.tabs[0].back.len(), 1);
        tabs.activate(0);
        assert_eq!(tabs.to_session().active_index, Some(0));
    }

    #[test]
    fn a_session_round_trips_and_unknown_kinds_are_dropped() {
        let mut tabs = with_files(&["/a.md", "/b.md"]);
        tabs.open(&p("/c.md"));
        tabs.open_settings();
        tabs.activate(1);
        let mut session = tabs.to_session();
        session.tabs.insert(
            0,
            SessionTab {
                location: SerializedLocation {
                    kind: "graph".into(),
                    payload: serde_json::Map::new(),
                },
                back: Vec::new(),
                forward: Vec::new(),
            },
        );
        session.active_index = Some(2);
        let restored = Tabs::from_session(&session);
        assert_eq!(
            locations(&restored),
            [
                Location::file("/a.md"),
                Location::file("/c.md"),
                Location::Settings
            ]
        );
        assert_eq!(restored.active_index(), 1);
        assert_eq!(restored.tabs()[1].back, [Location::file("/b.md")]);
        assert_eq!(Tabs::from_session(&SessionData::default()), Tabs::new());
    }

    /// Ports Writer `stores.test.ts:426`: back and forward on a launcher
    /// change nothing.
    #[test]
    fn back_and_forward_on_a_launcher_change_nothing() {
        let mut tabs = Tabs::new();
        tabs.open_launcher();
        let before = tabs.clone();
        assert!(tabs.back().is_none());
        assert!(tabs.forward().is_none());
        assert_eq!(tabs, before);
        assert_eq!(tabs.active().location, Location::Launcher);
        assert_eq!(tabs.active_path(), None);
    }

    /// Ports Writer `stores.test.ts:438` (`setActiveFile`): Cerata activates
    /// a tab by index; activating another tab switches the active file.
    #[test]
    fn activating_a_tab_switches_the_active_file() {
        let mut tabs = with_files(&["/a.md", "/b.md"]);
        assert_eq!(tabs.active_path(), Some(p("/b.md").as_path()));
        assert!(tabs.activate(0));
        assert_eq!(tabs.active_path(), Some(p("/a.md").as_path()));
        assert!(!tabs.activate(0), "the active tab is a no-op");
        assert!(!tabs.activate(2), "out of range is a no-op");
        assert_eq!(tabs.active_path(), Some(p("/a.md").as_path()));
    }
}
