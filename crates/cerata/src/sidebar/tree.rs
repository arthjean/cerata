//! The sidebar tree as plain data: sort modes, labels, the flattened rows
//! of the expanded folders and the selection.
//!
//! Ported from Writer `sidebar-sort.ts:13-89`, `use-file-tree-label.ts`,
//! `flatten-tree.ts` and the selection half of `file-tree.tsx:113-191`.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cerata_core::fs::DirEntry;

/// The six orders of `appearance.sidebar-sort`, in menu order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    #[default]
    NameAsc,
    NameDesc,
    ModifiedDesc,
    ModifiedAsc,
    CreatedDesc,
    CreatedAsc,
}

impl SortMode {
    pub const ALL: [Self; 6] = [
        Self::NameAsc,
        Self::NameDesc,
        Self::ModifiedDesc,
        Self::ModifiedAsc,
        Self::CreatedDesc,
        Self::CreatedAsc,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::NameAsc => "name-asc",
            Self::NameDesc => "name-desc",
            Self::ModifiedDesc => "modified-desc",
            Self::ModifiedAsc => "modified-asc",
            Self::CreatedDesc => "created-desc",
            Self::CreatedAsc => "created-asc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NameAsc => "Name (A to Z)",
            Self::NameDesc => "Name (Z to A)",
            Self::ModifiedDesc => "Modified time (new to old)",
            Self::ModifiedAsc => "Modified time (old to new)",
            Self::CreatedDesc => "Created time (new to old)",
            Self::CreatedAsc => "Created time (old to new)",
        }
    }

    /// An unknown or missing mode sorts by name.
    pub fn parse(id: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| Some(mode.id()) == id)
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelMode {
    #[default]
    Title,
    Filename,
}

impl LabelMode {
    pub fn parse(id: Option<&str>) -> Self {
        if id == Some("filename") {
            Self::Filename
        } else {
            Self::Title
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortOptions {
    pub mode: SortMode,
    pub folders_first: bool,
    pub label: LabelMode,
}

impl Default for SortOptions {
    fn default() -> Self {
        Self {
            mode: SortMode::NameAsc,
            folders_first: true,
            label: LabelMode::Title,
        }
    }
}

pub fn stem(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
}

/// A row's text: a folder's name, a file's stem in filename mode, else its
/// title, falling back to the stem.
pub fn label(entry: &DirEntry, mode: LabelMode) -> String {
    if entry.is_dir {
        return entry.name.clone();
    }
    match (mode, &entry.title) {
        (LabelMode::Title, Some(title)) if !title.trim().is_empty() => title.clone(),
        _ => stem(&entry.name).to_owned(),
    }
}

/// Case- and accent-insensitive key, digits kept for the natural compare.
fn fold(text: &str) -> Vec<char> {
    text.chars().flat_map(fold_char).collect()
}

fn fold_char(c: char) -> impl Iterator<Item = char> {
    let base = match c {
        'À'..='Å' | 'à'..='å' | 'Ā'..='ą' => 'a',
        'Ç' | 'ç' | 'Ć'..='č' => 'c',
        'Ď'..='đ' => 'd',
        'È'..='Ë' | 'è'..='ë' | 'Ē'..='ě' => 'e',
        'Ĝ'..='ģ' => 'g',
        'Ĥ'..='ħ' => 'h',
        'Ì'..='Ï' | 'ì'..='ï' | 'Ĩ'..='ı' => 'i',
        'Ĵ' | 'ĵ' => 'j',
        'Ķ' | 'ķ' => 'k',
        'Ĺ'..='ł' => 'l',
        'Ñ' | 'ñ' | 'Ń'..='ŋ' => 'n',
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' | 'Ō'..='ő' => 'o',
        'Ŕ'..='ř' => 'r',
        'Ś'..='š' => 's',
        'Ţ'..='ŧ' => 't',
        'Ù'..='Ü' | 'ù'..='ü' | 'Ũ'..='ų' => 'u',
        'Ŵ' | 'ŵ' => 'w',
        'Ý' | 'ý' | 'ÿ' | 'Ŷ'..='Ÿ' => 'y',
        'Ź'..='ž' => 'z',
        _ => '\0',
    };
    let folded: Vec<char> = match c {
        'Æ' | 'æ' => vec!['a', 'e'],
        'Œ' | 'œ' => vec!['o', 'e'],
        'ß' => vec!['s', 's'],
        _ if base != '\0' => vec![base],
        _ => c.to_lowercase().collect(),
    };
    folded.into_iter()
}

/// Natural order: digit runs compare by value ("2" before "10"), the rest
/// ignores case and accents (`Intl.Collator` with `numeric`, `base`).
fn natural_cmp_folded(a: &[char], b: &[char]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let start_a = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let start_b = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let digits_a = trim_zeros(&a[start_a..i]);
            let digits_b = trim_zeros(&b[start_b..j]);
            let order = digits_a
                .len()
                .cmp(&digits_b.len())
                .then_with(|| digits_a.cmp(digits_b));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].cmp(&b[j]);
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(digits: &[char]) -> &[char] {
    let first = digits
        .iter()
        .position(|c| *c != '0')
        .unwrap_or(digits.len());
    &digits[first..]
}

/// An entry with its folded label, computed once per sort.
struct Keyed<'a> {
    entry: &'a DirEntry,
    label: Vec<char>,
}

fn by_label(a: &Keyed<'_>, b: &Keyed<'_>) -> Ordering {
    natural_cmp_folded(&a.label, &b.label).then_with(|| a.entry.name.cmp(&b.entry.name))
}

fn compare(a: &Keyed<'_>, b: &Keyed<'_>, mode: SortMode) -> Ordering {
    let (x, y) = (a.entry, b.entry);
    let label_order = || by_label(a, b);
    match mode {
        SortMode::NameAsc => label_order(),
        SortMode::NameDesc => label_order().reverse(),
        SortMode::ModifiedDesc => y.modified_at.cmp(&x.modified_at).then_with(label_order),
        SortMode::ModifiedAsc => x.modified_at.cmp(&y.modified_at).then_with(label_order),
        SortMode::CreatedDesc => y.created_at.cmp(&x.created_at).then_with(label_order),
        SortMode::CreatedAsc => x.created_at.cmp(&y.created_at).then_with(label_order),
    }
}

/// A sorted copy of one folder's entries. With folders first, folders lead
/// and always go A to Z; only files follow `mode`.
pub fn sort_entries(entries: &[DirEntry], options: SortOptions) -> Vec<DirEntry> {
    let mut keyed: Vec<Keyed<'_>> = entries
        .iter()
        .map(|entry| Keyed {
            entry,
            label: fold(&label(entry, options.label)),
        })
        .collect();
    keyed.sort_by(|a, b| {
        if options.folders_first {
            match (a.entry.is_dir, b.entry.is_dir) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (true, true) => by_label(a, b),
                (false, false) => compare(a, b, options.mode),
            }
        } else {
            compare(a, b, options.mode)
        }
    });
    keyed.into_iter().map(|k| k.entry.clone()).collect()
}

/// What the sidebar knows about a folder's listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Loading,
    Loaded(Vec<DirEntry>),
    /// The read failed; the next expansion tries again.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub entry: DirEntry,
    pub depth: usize,
    pub expanded: bool,
    /// The folder's read failed (shown on its row).
    pub error: Option<String>,
}

/// Depth-first rows of the root listing, each expanded folder's children
/// right after it, every level sorted (`flatten-tree.ts`).
pub fn flatten(
    root: &[DirEntry],
    expanded: &HashSet<PathBuf>,
    listings: &HashMap<PathBuf, Listing>,
    options: SortOptions,
) -> Vec<Row> {
    let mut rows = Vec::new();
    push_level(root, 0, expanded, listings, options, &mut rows);
    rows
}

fn push_level(
    entries: &[DirEntry],
    depth: usize,
    expanded: &HashSet<PathBuf>,
    listings: &HashMap<PathBuf, Listing>,
    options: SortOptions,
    rows: &mut Vec<Row>,
) {
    for entry in sort_entries(entries, options) {
        let is_expanded = entry.is_dir && expanded.contains(&entry.path);
        let listing = listings.get(&entry.path);
        let error = match listing {
            Some(Listing::Failed(error)) if entry.is_dir => Some(error.clone()),
            _ => None,
        };
        let children = match (is_expanded, listing) {
            (true, Some(Listing::Loaded(children))) => Some(children.clone()),
            _ => None,
        };
        rows.push(Row {
            entry,
            depth,
            expanded: is_expanded && error.is_none(),
            error,
        });
        if let Some(children) = children {
            push_level(&children, depth + 1, expanded, listings, options, rows);
        }
    }
}

/// Multi-selection with an anchor (`file-tree.tsx:113-191`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub paths: Vec<PathBuf>,
    pub anchor: Option<PathBuf>,
}

impl Selection {
    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Escape, or a plain click.
    pub fn clear(&mut self) {
        self.paths.clear();
        self.anchor = None;
    }

    /// A plain press: the anchor moves here, nothing stays selected.
    pub fn press(&mut self, path: &Path) {
        self.paths.clear();
        self.anchor = Some(path.to_path_buf());
    }

    /// Cmd or Ctrl: toggles `path`, which becomes the anchor.
    pub fn toggle(&mut self, path: &Path) {
        if let Some(ix) = self.paths.iter().position(|p| p == path) {
            self.paths.remove(ix);
        } else {
            self.paths.push(path.to_path_buf());
        }
        self.anchor = Some(path.to_path_buf());
    }

    /// Shift: the rows from the anchor (or the first row) to `path`
    /// replace the selection; the anchor stays.
    pub fn extend(&mut self, path: &Path, rows: &[Row]) {
        let Some(target) = rows.iter().position(|row| row.entry.path == path) else {
            return;
        };
        let from = self
            .anchor
            .as_ref()
            .and_then(|anchor| rows.iter().position(|row| &row.entry.path == anchor))
            .unwrap_or(0);
        let (start, end) = if from <= target {
            (from, target)
        } else {
            (target, from)
        };
        self.paths = rows[start..=end]
            .iter()
            .map(|row| row.entry.path.clone())
            .collect();
    }

    /// Selected paths that are still shown.
    pub fn retain_visible(&mut self, rows: &[Row]) {
        let visible: HashSet<&Path> = rows.iter().map(|row| row.entry.path.as_path()).collect();
        self.paths.retain(|path| visible.contains(path.as_path()));
    }
}

/// Bounds of the sidebar width (`app-layout.tsx`): at least 220 px, at most
/// `max(280, min(420, 35 % of the window))`.
pub fn clamp_width(width: f32, window_width: f32) -> f32 {
    let max = 280.0_f32.max(420.0_f32.min((window_width * 0.35).floor()));
    220.0_f32.max(max.min(width.round()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, title: Option<&str>, modified: u64, created: u64) -> DirEntry {
        DirEntry {
            name: name.into(),
            path: PathBuf::from("/v").join(name),
            is_dir: false,
            is_markdown: true,
            modified_at: modified,
            created_at: created,
            title: title.map(str::to_owned),
        }
    }

    fn folder(name: &str, modified: u64) -> DirEntry {
        DirEntry {
            name: name.into(),
            path: PathBuf::from("/v").join(name),
            is_dir: true,
            is_markdown: false,
            modified_at: modified,
            created_at: modified,
            title: None,
        }
    }

    fn names(entries: &[DirEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    fn opts(mode: SortMode) -> SortOptions {
        SortOptions {
            mode,
            ..SortOptions::default()
        }
    }

    #[test]
    fn files_sort_by_title_with_the_stem_as_fallback() {
        let entries = [
            file("a.md", Some("Zebra"), 0, 0),
            file("b.md", None, 0, 0),
            file("c.md", Some("Apple"), 0, 0),
        ];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::NameAsc))),
            ["c.md", "b.md", "a.md"]
        );
    }

    #[test]
    fn filename_mode_sorts_by_stem() {
        let entries = [file("b.md", Some("A"), 0, 0), file("a.md", Some("Z"), 0, 0)];
        let options = SortOptions {
            label: LabelMode::Filename,
            ..SortOptions::default()
        };
        assert_eq!(names(&sort_entries(&entries, options)), ["a.md", "b.md"]);
        assert_eq!(label(&entries[0], LabelMode::Filename), "b");
        assert_eq!(label(&entries[0], LabelMode::Title), "A");
    }

    #[test]
    fn folders_come_first_by_name_whatever_the_titles() {
        let entries = [
            file("a.md", Some("A"), 0, 0),
            folder("zeta", 0),
            folder("alpha", 0),
        ];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::NameAsc))),
            ["alpha", "zeta", "a.md"]
        );
    }

    #[test]
    fn the_sort_is_natural_and_ignores_case_and_accents() {
        let entries = [
            file("Chapter 10.md", None, 0, 0),
            file("chapter 2.md", None, 0, 0),
            file("Étude.md", None, 0, 0),
            file("dessert.md", None, 0, 0),
        ];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::NameAsc))),
            ["chapter 2.md", "Chapter 10.md", "dessert.md", "Étude.md"]
        );
    }

    #[test]
    fn label_ties_break_by_file_name() {
        let entries = [
            file("b.md", Some("Same"), 0, 0),
            file("a.md", Some("Same"), 0, 0),
        ];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::NameAsc))),
            ["a.md", "b.md"]
        );
    }

    #[test]
    fn name_desc_reverses_files_and_keeps_folders_first_a_to_z() {
        let entries = [
            file("a.md", None, 0, 0),
            file("b.md", None, 0, 0),
            folder("y", 0),
            folder("x", 0),
        ];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::NameDesc))),
            ["x", "y", "b.md", "a.md"]
        );
    }

    #[test]
    fn modified_modes_order_files_and_leave_folders_alphabetical() {
        let entries = [
            file("old.md", None, 1, 0),
            file("new.md", None, 3, 0),
            folder("b", 9),
            folder("a", 1),
        ];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::ModifiedDesc))),
            ["a", "b", "new.md", "old.md"]
        );
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::ModifiedAsc))),
            ["a", "b", "old.md", "new.md"]
        );
    }

    #[test]
    fn created_modes_order_by_creation_time() {
        let entries = [file("x.md", None, 0, 5), file("y.md", None, 0, 1)];
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::CreatedAsc))),
            ["y.md", "x.md"]
        );
        assert_eq!(
            names(&sort_entries(&entries, opts(SortMode::CreatedDesc))),
            ["x.md", "y.md"]
        );
    }

    #[test]
    fn equal_times_fall_back_to_label_ascending_in_both_directions() {
        let entries = [file("b.md", None, 1, 1), file("a.md", None, 1, 1)];
        for mode in [
            SortMode::ModifiedDesc,
            SortMode::ModifiedAsc,
            SortMode::CreatedDesc,
            SortMode::CreatedAsc,
        ] {
            assert_eq!(
                names(&sort_entries(&entries, opts(mode))),
                ["a.md", "b.md"],
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_missing_or_bogus_mode_sorts_by_name() {
        assert_eq!(SortMode::parse(None), SortMode::NameAsc);
        assert_eq!(SortMode::parse(Some("bogus")), SortMode::NameAsc);
        assert_eq!(SortMode::parse(Some("created-asc")), SortMode::CreatedAsc);
    }

    #[test]
    fn without_folders_first_names_interleave_folders_and_files() {
        let entries = [
            file("b.md", None, 0, 0),
            folder("c", 0),
            file("a.md", None, 0, 0),
        ];
        let options = SortOptions {
            folders_first: false,
            ..SortOptions::default()
        };
        assert_eq!(
            names(&sort_entries(&entries, options)),
            ["a.md", "b.md", "c"]
        );
        let desc = SortOptions {
            mode: SortMode::NameDesc,
            ..options
        };
        assert_eq!(names(&sort_entries(&entries, desc)), ["c", "b.md", "a.md"]);
    }

    #[test]
    fn without_folders_first_time_modes_use_folder_times() {
        let entries = [
            file("a.md", None, 2, 0),
            folder("f", 3),
            file("b.md", None, 1, 0),
        ];
        let options = SortOptions {
            mode: SortMode::ModifiedDesc,
            folders_first: false,
            ..SortOptions::default()
        };
        assert_eq!(
            names(&sort_entries(&entries, options)),
            ["f", "a.md", "b.md"]
        );
    }

    #[test]
    fn folders_first_is_the_default_and_matches_the_schema() {
        assert!(SortOptions::default().folders_first);
        let ids: Vec<&str> = SortMode::ALL.iter().map(|m| m.id()).collect();
        assert_eq!(
            ids,
            [
                "name-asc",
                "name-desc",
                "modified-desc",
                "modified-asc",
                "created-desc",
                "created-asc"
            ]
        );
        assert_eq!(SortMode::default(), SortMode::ALL[0]);
    }

    /// Ports Writer `flatten-tree.test.ts:178` and `:187`: the folders-first
    /// default and the sort modes come from the settings schema itself.
    #[test]
    fn the_sort_registry_and_folders_first_follow_the_schema() {
        use cerata_core::settings::{ConfigValue, Schema};
        let schema = Schema::embedded().unwrap();
        let folders_first = schema.get(super::super::FOLDERS_FIRST_KEY).unwrap();
        assert_eq!(folders_first.default, ConfigValue::Bool(true));
        assert_eq!(
            SortOptions::default().folders_first,
            folders_first.default.as_bool().unwrap()
        );
        let entries = [file("a.md", Some("Apple"), 0, 0), folder("zoo", 0)];
        assert_eq!(
            names(&sort_entries(&entries, SortOptions::default())),
            ["zoo", "a.md"]
        );

        let sort = schema.get(super::super::SORT_KEY).unwrap();
        let ids: Vec<String> = SortMode::ALL.iter().map(|m| m.id().to_owned()).collect();
        assert_eq!(sort.options.as_deref(), Some(ids.as_slice()));
        assert_eq!(sort.default.as_str(), Some(SortMode::default().id()));
        assert_eq!(SortMode::default(), SortMode::ALL[0]);
    }

    #[test]
    fn sorting_does_not_touch_the_input() {
        let entries = vec![file("b.md", None, 0, 0), file("a.md", None, 0, 0)];
        let before = entries.clone();
        let _ = sort_entries(&entries, opts(SortMode::NameAsc));
        assert_eq!(entries, before);
    }

    #[test]
    fn flatten_sorts_every_expanded_level_with_depths() {
        let root = [file("z.md", None, 0, 0), folder("dir", 0)];
        let child = DirEntry {
            path: PathBuf::from("/v/dir/inner"),
            ..folder("inner", 0)
        };
        let leaf = DirEntry {
            path: PathBuf::from("/v/dir/inner/leaf.md"),
            ..file("leaf.md", None, 0, 0)
        };
        let child_file = DirEntry {
            path: PathBuf::from("/v/dir/a.md"),
            ..file("a.md", None, 0, 0)
        };
        let expanded: HashSet<PathBuf> =
            [PathBuf::from("/v/dir"), PathBuf::from("/v/dir/inner")].into();
        let listings: HashMap<PathBuf, Listing> = [
            (
                PathBuf::from("/v/dir"),
                Listing::Loaded(vec![child_file, child]),
            ),
            (PathBuf::from("/v/dir/inner"), Listing::Loaded(vec![leaf])),
        ]
        .into();
        let rows = flatten(&root, &expanded, &listings, SortOptions::default());
        let shape: Vec<(&str, usize)> = rows
            .iter()
            .map(|r| (r.entry.name.as_str(), r.depth))
            .collect();
        assert_eq!(
            shape,
            [
                ("dir", 0),
                ("inner", 1),
                ("leaf.md", 2),
                ("a.md", 1),
                ("z.md", 0)
            ]
        );
    }

    #[test]
    fn a_failed_folder_shows_its_error_and_no_children() {
        let root = [folder("dir", 0)];
        let expanded: HashSet<PathBuf> = [PathBuf::from("/v/dir")].into();
        let listings: HashMap<PathBuf, Listing> =
            [(PathBuf::from("/v/dir"), Listing::Failed("denied".into()))].into();
        let rows = flatten(&root, &expanded, &listings, SortOptions::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].error.as_deref(), Some("denied"));
        assert!(!rows[0].expanded);
    }

    fn rows(paths: &[&str]) -> Vec<Row> {
        paths
            .iter()
            .map(|name| Row {
                entry: file(name, None, 0, 0),
                depth: 0,
                expanded: false,
                error: None,
            })
            .collect()
    }

    #[test]
    fn shift_selects_a_range_from_the_anchor_and_cmd_toggles() {
        let rows = rows(&["a.md", "b.md", "c.md", "d.md"]);
        let p = |n: &str| PathBuf::from("/v").join(n);
        let mut selection = Selection::default();
        selection.extend(&p("b.md"), &rows);
        assert_eq!(
            selection.paths,
            [p("a.md"), p("b.md")],
            "no anchor: from the first row"
        );
        selection.press(&p("c.md"));
        selection.extend(&p("a.md"), &rows);
        assert_eq!(selection.paths, [p("a.md"), p("b.md"), p("c.md")]);
        assert_eq!(selection.anchor, Some(p("c.md")));
        selection.toggle(&p("b.md"));
        assert_eq!(selection.paths, [p("a.md"), p("c.md")]);
        selection.toggle(&p("d.md"));
        assert_eq!(selection.anchor, Some(p("d.md")));
        selection.clear();
        assert!(selection.is_empty() && selection.anchor.is_none());
    }

    #[test]
    fn fifty_thousand_entries_sort_and_flatten_within_a_frame_budget() {
        let entries: Vec<DirEntry> = (0..50_000)
            .map(|i| {
                file(
                    &format!("Note {i} \u{e9}t\u{e9}.md"),
                    None,
                    i as u64 % 97,
                    0,
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let rows = flatten(
            &entries,
            &HashSet::new(),
            &HashMap::new(),
            SortOptions::default(),
        );
        let elapsed = started.elapsed();
        assert_eq!(rows.len(), 50_000);
        assert_eq!(rows[1].entry.name, "Note 1 \u{e9}t\u{e9}.md");
        assert_eq!(rows[2].entry.name, "Note 2 \u{e9}t\u{e9}.md");
        // Debug-build budget. Rows are rebuilt only when the tree changes,
        // never per frame: scrolling lays out the visible rows only.
        assert!(elapsed.as_millis() < 1500, "{elapsed:?}");
    }

    #[test]
    fn the_width_stays_between_220_and_the_window_bound() {
        assert_eq!(clamp_width(100.0, 1200.0), 220.0);
        assert_eq!(clamp_width(1000.0, 1200.0), 420.0);
        assert_eq!(clamp_width(1000.0, 800.0), 280.0);
        assert_eq!(clamp_width(300.4, 1000.0), 300.0);
        assert_eq!(clamp_width(1000.0, 1000.0), 350.0);
    }
}
