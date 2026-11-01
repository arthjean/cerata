//! The sidebar filter (Paneflow's workspace filter): while a query is set,
//! the sidebar lists every indexed file whose label or relative path
//! contains it, case-insensitively, instead of its sections. Titles are
//! read in the background the first time a filter starts and dropped when
//! it ends, so a new filter sees edited titles.
//!
//! Each change of the matches animates as Paneflow's rail does
//! (`SidebarRowMotion`): over 180 ms with a cubic ease-out, entering rows
//! fade in while their height grows, leaving rows fade out and collapse in
//! place, and a new query retargets from what is shown. Reduced motion
//! applies the matches at once.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cerata_core::fs::DirEntry;
use cerata_core::fs::titles::extract_title;
use gpui::{Context, FontWeight, HighlightStyle, ScrollStrategy, StyledText};

use super::Sidebar;
use super::tree::{LabelMode, stem};
use crate::editor::theme;

/// Titles read per background chunk.
const TITLE_CHUNK: usize = 512;

/// Length of a change of the matches (Paneflow's rail rows).
pub const MOTION: Duration = Duration::from_millis(180);

#[derive(Default)]
pub struct Filter {
    /// Trimmed and lowercased.
    pub query: String,
    pub results: Vec<DirEntry>,
    pub motion: RowMotion,
    titles: HashMap<PathBuf, String>,
    titles_requested: bool,
}

/// A row shown while the matches change: its sort key, its entry, and how
/// far it is shown (0 gone, 1 settled).
pub type MotionRow = (String, DirEntry, f32);

/// The rows of a change of the matches in flight.
#[derive(Default)]
pub struct RowMotion {
    pub rows: Vec<MotionRow>,
    from: HashMap<PathBuf, f32>,
    target: HashSet<PathBuf>,
    started: Option<Instant>,
}

pub(super) fn ease_out_cubic(progress: f32) -> f32 {
    1. - (1. - progress).powi(3)
}

impl RowMotion {
    pub fn animating(&self) -> bool {
        self.started.is_some()
    }

    /// Moves every row to its amount at `now`; at the end, the rows that
    /// left are dropped.
    pub fn sample(&mut self, now: Instant) {
        let Some(started) = self.started else { return };
        let progress = (now.duration_since(started).as_secs_f32() / MOTION.as_secs_f32()).min(1.);
        let eased = ease_out_cubic(progress);
        for (_, entry, amount) in &mut self.rows {
            let from = self.from.get(&entry.path).copied().unwrap_or(0.);
            let to = if self.target.contains(&entry.path) {
                1.
            } else {
                0.
            };
            *amount = from + (to - from) * eased;
        }
        if progress >= 1. {
            self.rows.retain(|(_, _, amount)| *amount > 0.);
            self.started = None;
        }
    }

    /// Heads for `matches` (sorted by key) from what is shown at `now`.
    /// Leaving rows keep their place among them until they are gone.
    fn retarget(&mut self, matches: &[(String, DirEntry)], now: Instant, animate: bool) {
        self.sample(now);
        if !animate {
            self.rows = matches
                .iter()
                .map(|(key, entry)| (key.clone(), entry.clone(), 1.))
                .collect();
            self.from.clear();
            self.target.clear();
            self.started = None;
            return;
        }
        self.from = self
            .rows
            .iter()
            .map(|(_, entry, amount)| (entry.path.clone(), *amount))
            .collect();
        self.target = matches
            .iter()
            .map(|(_, entry)| entry.path.clone())
            .collect();
        let target = &self.target;
        let leaving: Vec<MotionRow> = self
            .rows
            .drain(..)
            .filter(|(_, entry, amount)| *amount > 0. && !target.contains(&entry.path))
            .collect();
        let mut rows: Vec<MotionRow> = matches
            .iter()
            .map(|(key, entry)| {
                let amount = self.from.get(&entry.path).copied().unwrap_or(0.);
                (key.clone(), entry.clone(), amount)
            })
            .chain(leaving)
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        self.rows = rows;
        self.started = Some(now);
    }
}

impl Sidebar {
    pub fn set_filter(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim().to_lowercase();
        if query == self.filter.query {
            return;
        }
        if query.is_empty() {
            self.filter = Filter::default();
        } else {
            self.filter.query = query;
            if !self.filter.titles_requested && self.options.label == LabelMode::Title {
                self.filter.titles_requested = true;
                self.load_filter_titles(cx);
            }
            self.refilter(!cx.reduce_motion());
        }
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub fn filtering(&self) -> bool {
        !self.filter.query.is_empty()
    }

    /// Ends the filter of a previous workspace.
    pub fn reset_filter(&mut self) {
        self.filter = Filter::default();
    }

    fn refilter(&mut self, animate: bool) {
        let Some(runtime) = self.runtime.clone() else {
            self.filter.results.clear();
            self.filter.motion = RowMotion::default();
            return;
        };
        let query = self.filter.query.as_str();
        let titles = &self.filter.titles;
        let by_title = self.options.label == LabelMode::Title;
        let mut results: Vec<(String, DirEntry)> = runtime
            .index
            .lock()
            .files()
            .filter_map(|file| {
                // The row's label (`tree::label`), without building the row.
                let title = titles
                    .get(&file.path)
                    .filter(|title| by_title && !title.trim().is_empty());
                let shown = title
                    .map_or_else(|| stem(&file.name), String::as_str)
                    .to_lowercase();
                let matches =
                    shown.contains(query) || file.relative_path.to_lowercase().contains(query);
                matches.then(|| {
                    let entry = DirEntry {
                        name: file.name.clone(),
                        path: file.path.clone(),
                        is_dir: false,
                        is_markdown: true,
                        modified_at: file.modified_at,
                        created_at: file.modified_at,
                        title: title.cloned(),
                    };
                    (shown, entry)
                })
            })
            .collect();
        results.sort_by(|a, b| a.0.cmp(&b.0));
        self.filter
            .motion
            .retarget(&results, Instant::now(), animate);
        self.filter.results = results.into_iter().map(|(_, entry)| entry).collect();
    }

    /// Reads every indexed file's title, in chunks, refiltering after each.
    fn load_filter_titles(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let paths: Vec<PathBuf> = runtime
            .index
            .lock()
            .files()
            .map(|file| file.path.clone())
            .collect();
        let task = cx.spawn(async move |this, cx| {
            for chunk in paths.chunks(TITLE_CHUNK) {
                let chunk = chunk.to_vec();
                let titles: Vec<(PathBuf, String)> = cx
                    .background_executor()
                    .spawn(async move {
                        chunk
                            .into_iter()
                            .filter_map(|path| extract_title(&path).map(|title| (path, title)))
                            .collect()
                    })
                    .await;
                let applied = this.update(cx, |this, cx| {
                    // The filter ended (or the workspace changed) meanwhile.
                    if !this.filter.titles_requested {
                        return false;
                    }
                    this.filter.titles.extend(titles);
                    this.refilter(!cx.reduce_motion());
                    cx.notify();
                    true
                });
                if !matches!(applied, Ok(true)) {
                    return;
                }
            }
        });
        self.tasks.push(task);
    }
}

/// `text` with every occurrence of `query` (lowercased) in the filter's
/// match color and semibold (`ui_primitives.rs` `highlight_matches` in
/// Paneflow).
pub fn highlight(text: String, query: &str) -> StyledText {
    let ranges = match_ranges(&text, query);
    let style = HighlightStyle {
        color: Some(theme::filter_match()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    StyledText::new(text).with_highlights(ranges.into_iter().map(move |range| (range, style)))
}

/// Byte ranges of `text` where the lowercased `query` occurs, without
/// overlaps.
fn match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    if !query.is_empty() {
        // Lowercasing can change byte lengths: map each lowered byte back
        // to the source character it came from.
        let mut lowered = String::new();
        let mut sources = Vec::new();
        for (start, ch) in text.char_indices() {
            let lower = ch.to_lowercase().to_string();
            sources.extend(std::iter::repeat_n(
                start..start + ch.len_utf8(),
                lower.len(),
            ));
            lowered.push_str(&lower);
        }
        for (start, matched) in lowered.match_indices(query) {
            let range = sources[start].start..sources[start + matched.len() - 1].end;
            if ranges
                .last()
                .is_none_or(|previous: &std::ops::Range<usize>| previous.end <= range.start)
            {
                ranges.push(range);
            }
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> DirEntry {
        DirEntry {
            name: name.into(),
            path: PathBuf::from(name),
            is_dir: false,
            is_markdown: true,
            modified_at: 0,
            created_at: 0,
            title: None,
        }
    }

    fn keyed(names: &[&str]) -> Vec<(String, DirEntry)> {
        names.iter().map(|n| (n.to_string(), entry(n))).collect()
    }

    fn amounts(motion: &RowMotion) -> Vec<(String, f32)> {
        motion
            .rows
            .iter()
            .map(|(key, _, amount)| (key.clone(), *amount))
            .collect()
    }

    #[test]
    fn leaving_rows_fade_in_place_and_go_at_the_end() {
        let now = Instant::now();
        let mut motion = RowMotion::default();
        motion.retarget(&keyed(&["a", "b", "c"]), now, false);
        motion.retarget(&keyed(&["a", "c"]), now, true);
        assert_eq!(
            amounts(&motion),
            [("a".into(), 1.), ("b".into(), 1.), ("c".into(), 1.)]
        );
        // Halfway in time, the ease is 7/8 of the way: b shows at 0.125.
        motion.sample(now + MOTION / 2);
        assert!((amounts(&motion)[1].1 - 0.125).abs() < 0.001);
        motion.sample(now + MOTION);
        assert_eq!(amounts(&motion), [("a".into(), 1.), ("c".into(), 1.)]);
        assert!(!motion.animating());
    }

    #[test]
    fn a_new_query_retargets_from_what_is_shown() {
        let now = Instant::now();
        let mut motion = RowMotion::default();
        motion.retarget(&keyed(&["a"]), now, true);
        let midway = now + Duration::from_millis(60);
        motion.sample(midway);
        let shown = amounts(&motion)[0].1;
        assert!(shown > 0. && shown < 1.);
        // Leaving starts from where it was, not from 1.
        motion.retarget(&[], midway, true);
        assert_eq!(amounts(&motion)[0].1, shown);
        motion.sample(midway + MOTION / 2);
        assert!(amounts(&motion)[0].1 < shown);
        motion.sample(midway + MOTION);
        assert!(motion.rows.is_empty());
    }

    #[test]
    fn reduced_motion_applies_the_matches_at_once() {
        let mut motion = RowMotion::default();
        motion.retarget(&keyed(&["b", "a"]), Instant::now(), false);
        assert_eq!(amounts(&motion), [("b".into(), 1.), ("a".into(), 1.)]);
        assert!(!motion.animating());
    }

    #[test]
    fn matches_ignore_case_and_map_back_to_the_source_bytes() {
        assert_eq!(match_ranges("ADR 0002 : Parser", "parser"), vec![11..17]);
        // "É" is two bytes, lowered or not; "İ" lowers to three.
        assert_eq!(match_ranges("Édition", "éd"), vec![0..3]);
        assert_eq!(match_ranges("İa", "a"), vec![2..3]);
    }

    #[test]
    fn occurrences_do_not_overlap() {
        assert_eq!(match_ranges("aaaa", "aa"), [0..2, 2..4]);
        assert!(match_ranges("note", "").is_empty());
    }
}
