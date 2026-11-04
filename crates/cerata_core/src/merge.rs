//! Three-way merge of a note changed outside Cerata while it had unsaved
//! edits (US-092): base = what Cerata last read or wrote, ours = the
//! buffer, theirs = the disk.
//!
//! The three texts are normalized first (BOM dropped, CRLF and CR turned
//! into LF). A line diff (histogram, `imara-diff`, as Zed does) of each side
//! against the base gives the changed regions; regions that touch or
//! overlap form one block. A block one side alone changed takes that side.
//! A block both sides changed the same way takes it once. Otherwise the
//! block is merged again by words, and if the word changes still overlap
//! it is a conflict for the user to resolve: keep mine, keep theirs, or
//! keep both. Nothing is dropped silently.

use imara_diff::{Algorithm, Diff, InternedInput};
use std::ops::Range;

/// The text with no BOM and LF line endings only.
pub fn normalize(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.contains('\r') {
        return text.to_owned();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// One block of a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Text both sides kept from the base, or a change applied cleanly.
    Merged(String),
    /// Both sides changed this text differently.
    Conflict {
        base: String,
        ours: String,
        theirs: String,
    },
}

/// How a conflict is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Mine,
    Theirs,
    /// Mine, then theirs.
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    pub blocks: Vec<Block>,
    /// The result ends without a final newline: the blocks were merged
    /// with one, which the result drops (see [`merge`]).
    pub open_end: bool,
}

impl Merge {
    pub fn conflicts(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| matches!(block, Block::Conflict { .. }))
            .count()
    }

    pub fn is_clean(&self) -> bool {
        self.conflicts() == 0
    }

    /// The merged text, when nothing conflicts.
    pub fn text(&self) -> Option<String> {
        self.is_clean().then(|| self.resolve(&[]))
    }

    /// The text with conflict `n` resolved by `choices[n]` (mine when
    /// missing).
    pub fn resolve(&self, choices: &[Choice]) -> String {
        let mut text = String::new();
        let mut n = 0;
        for block in &self.blocks {
            match block {
                Block::Merged(merged) => text.push_str(merged),
                Block::Conflict { ours, theirs, .. } => {
                    match choices.get(n).copied().unwrap_or(Choice::Mine) {
                        Choice::Mine => text.push_str(ours),
                        Choice::Theirs => text.push_str(theirs),
                        Choice::Both => {
                            text.push_str(ours);
                            if !ours.is_empty() && !ours.ends_with('\n') && !theirs.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(theirs);
                        }
                    }
                    n += 1;
                }
            }
        }
        if self.open_end && text.ends_with('\n') {
            text.pop();
        }
        text
    }
}

/// One line of a line diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    Removed(String),
    Added(String),
}

/// The lines of `old` and `new` (normalized), changes marked, for "Show
/// Changes" (US-091). Lines carry no line ending.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let (old, new) = (normalize(old), normalize(new));
    let lines =
        |text: &str| -> Vec<String> { text.split_inclusive('\n').map(str::to_owned).collect() };
    let (a, b) = (lines(&old), lines(&new));
    let bare = |line: &String| line.strip_suffix('\n').unwrap_or(line).to_owned();
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    let mut at = 0;
    for change in changes(&a, &b, true) {
        out.extend(
            a[at..change.base.start]
                .iter()
                .map(|l| DiffLine::Same(bare(l))),
        );
        out.extend(
            a[change.base.clone()]
                .iter()
                .map(|l| DiffLine::Removed(bare(l))),
        );
        out.extend(
            b[change.side.clone()]
                .iter()
                .map(|l| DiffLine::Added(bare(l))),
        );
        at = change.base.end;
    }
    out.extend(a[at..].iter().map(|l| DiffLine::Same(bare(l))));
    out
}

/// Merges `ours` and `theirs`, both derived from `base`.
pub fn merge(base: &str, ours: &str, theirs: &str) -> Merge {
    let (base, ours, theirs) = (normalize(base), normalize(ours), normalize(theirs));
    // A save ends the file with a newline that the buffer may lack: the
    // final newline is merged on its own, so its absence alone overlaps no
    // change of the last line.
    let open = |text: &str| !text.is_empty() && !text.ends_with('\n');
    let (base_open, theirs_open) = (open(&base), open(&theirs));
    let result_open = if open(&ours) == base_open {
        theirs_open
    } else {
        open(&ours)
    };
    let closed = |text: String| if open(&text) { text + "\n" } else { text };
    let (base, ours, theirs) = (closed(base), closed(ours), closed(theirs));
    let lines =
        |text: &str| -> Vec<String> { text.split_inclusive('\n').map(str::to_owned).collect() };
    let (b, o, t) = (lines(&base), lines(&ours), lines(&theirs));
    let mut blocks: Vec<Block> = Vec::new();
    let push = |blocks: &mut Vec<Block>, text: String| {
        if text.is_empty() {
            return;
        }
        if let Some(Block::Merged(last)) = blocks.last_mut() {
            last.push_str(&text);
        } else {
            blocks.push(Block::Merged(text));
        }
    };
    for region in diff3(&b, &o, &t, true) {
        match region {
            Region::Merged(text) => push(&mut blocks, text),
            Region::Conflict {
                base: cb,
                ours: co,
                theirs: ct,
            } => {
                // Merge the block again, word by word.
                let (wb, wo, wt) = (words(&cb), words(&co), words(&ct));
                let refined = diff3(&wb, &wo, &wt, false);
                if refined.iter().all(|r| matches!(r, Region::Merged(_))) {
                    for region in refined {
                        if let Region::Merged(text) = region {
                            push(&mut blocks, text);
                        }
                    }
                } else {
                    blocks.push(Block::Conflict {
                        base: cb,
                        ours: co,
                        theirs: ct,
                    });
                }
            }
        }
    }
    Merge {
        blocks,
        open_end: result_open,
    }
}

/// Words, runs of spaces and single other characters: their
/// concatenation is the text.
fn words(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut kind = 0u8;
    for ch in text.chars() {
        let this = if ch.is_alphanumeric() || ch == '_' {
            1
        } else if ch == ' ' || ch == '\t' {
            2
        } else {
            3
        };
        if this != kind || this == 3 {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            kind = this;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

enum Region {
    Merged(String),
    Conflict {
        base: String,
        ours: String,
        theirs: String,
    },
}

/// A change of one side: `base` replaced by `side` (token ranges).
#[derive(Debug, Clone)]
struct Change {
    base: Range<usize>,
    side: Range<usize>,
}

fn changes(base: &[String], side: &[String], lines: bool) -> Vec<Change> {
    let mut input: InternedInput<&str> = InternedInput::default();
    input.update_before(base.iter().map(String::as_str));
    input.update_after(side.iter().map(String::as_str));
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    if lines {
        diff.postprocess_lines(&input);
    } else {
        diff.postprocess_no_heuristic(&input);
    }
    diff.hunks()
        .map(|hunk| Change {
            base: hunk.before.start as usize..hunk.before.end as usize,
            side: hunk.after.start as usize..hunk.after.end as usize,
        })
        .collect()
}

/// Where `base` position `at` falls in the side, given its changes, all
/// of which end at or before `at` or start at or after it.
fn map_position(changes: &[Change], at: usize) -> usize {
    let mut delta: isize = 0;
    for change in changes {
        if change.base.end <= at && !(change.base.is_empty() && change.base.start == at) {
            delta += change.side.len() as isize - change.base.len() as isize;
        }
    }
    (at as isize + delta).max(0) as usize
}

fn join(tokens: &[String]) -> String {
    tokens.concat()
}

/// The diff3 regions of `ours` and `theirs` against `base`.
fn diff3(base: &[String], ours: &[String], theirs: &[String], lines: bool) -> Vec<Region> {
    let a = changes(base, ours, lines);
    let b = changes(base, theirs, lines);
    // All changes by base start; group those that touch or overlap.
    let mut all: Vec<(usize, &Change)> = a
        .iter()
        .map(|c| (0, c))
        .chain(b.iter().map(|c| (1, c)))
        .collect();
    all.sort_by_key(|(side, change)| (change.base.start, change.base.end, *side));
    let mut regions = Vec::new();
    let mut at = 0;
    let mut i = 0;
    while i < all.len() {
        let mut range = all[i].1.base.clone();
        let mut sides = [false, false];
        sides[all[i].0] = true;
        let mut j = i + 1;
        while j < all.len() && all[j].1.base.start <= range.end {
            range.end = range.end.max(all[j].1.base.end);
            sides[all[j].0] = true;
            j += 1;
        }
        if range.start > at {
            regions.push(Region::Merged(join(&base[at..range.start])));
        }
        let group: Vec<&(usize, &Change)> = all[i..j].iter().collect();
        let side_text = |side: usize, tokens: &[String], all_changes: &[Change]| {
            let mine: Vec<Change> = group
                .iter()
                .filter(|(s, _)| *s == side)
                .map(|(_, c)| (*c).clone())
                .collect();
            if mine.is_empty() {
                return join(&base[range.clone()]);
            }
            let start = map_position(all_changes, range.start);
            let end = map_position(all_changes, range.end);
            // Pure insertions at the end of the range belong to it.
            let end = end.max(mine.iter().map(|c| c.side.end).max().unwrap_or(end));
            let start = start.min(mine.iter().map(|c| c.side.start).min().unwrap_or(start));
            join(&tokens[start.min(tokens.len())..end.min(tokens.len())])
        };
        let ours_text = side_text(0, ours, &a);
        let theirs_text = side_text(1, theirs, &b);
        match sides {
            [true, false] => regions.push(Region::Merged(ours_text)),
            [false, true] => regions.push(Region::Merged(theirs_text)),
            _ if ours_text == theirs_text => regions.push(Region::Merged(ours_text)),
            _ => regions.push(Region::Conflict {
                base: join(&base[range.clone()]),
                ours: ours_text,
                theirs: theirs_text,
            }),
        }
        at = range.end;
        i = j;
    }
    if at < base.len() {
        regions.push(Region::Merged(join(&base[at..])));
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_changes_merge_cleanly() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let ours = "ONE\ntwo\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfour\nFIVE\n";
        let merge = merge(base, ours, theirs);
        assert_eq!(
            merge.text().as_deref(),
            Some("ONE\ntwo\nthree\nfour\nFIVE\n")
        );
    }

    #[test]
    fn overlapping_changes_conflict_and_keep_every_side() {
        let base = "# Plan\n\nShip on Monday.\n";
        let ours = "# Plan\n\nShip on Tuesday after review.\n";
        let theirs = "# Plan\n\nShip on Friday, once tested.\n";
        let merge = merge(base, ours, theirs);
        assert_eq!(merge.conflicts(), 1);
        assert_eq!(merge.text(), None);
        assert_eq!(merge.resolve(&[Choice::Mine]), ours);
        assert_eq!(merge.resolve(&[Choice::Theirs]), theirs);
        assert_eq!(
            merge.resolve(&[Choice::Both]),
            "# Plan\n\nShip on Tuesday after review.\nShip on Friday, once tested.\n"
        );
        let Block::Conflict {
            base: b,
            ours: o,
            theirs: t,
        } = &merge.blocks[1]
        else {
            panic!("{:?}", merge.blocks);
        };
        assert_eq!(b, "Ship on Monday.\n");
        assert_eq!(o, "Ship on Tuesday after review.\n");
        assert_eq!(t, "Ship on Friday, once tested.\n");
    }

    #[test]
    fn the_same_change_on_both_sides_is_kept_once() {
        let base = "a\nb\nc\n";
        let same = "a\nB, fixed\nc\n";
        let merge = merge(base, same, same);
        assert_eq!(merge.text().as_deref(), Some(same));
    }

    #[test]
    fn crlf_on_one_side_and_lf_on_the_other_is_no_change() {
        let base = "\u{feff}one\r\ntwo\r\nthree\r\n";
        let ours = "one\ntwo, mine\nthree\n";
        let theirs = "one\r\ntwo\r\nthree, theirs\r\n";
        let merge = merge(base, ours, theirs);
        assert_eq!(
            merge.text().as_deref(),
            Some("one\ntwo, mine\nthree, theirs\n")
        );
    }

    #[test]
    fn additions_at_the_end_on_both_sides_are_never_lost() {
        let base = "# Notes\n";
        let ours = "# Notes\n- mine\n";
        let theirs = "# Notes\n- theirs\n";
        let merge = merge(base, ours, theirs);
        assert_eq!(merge.conflicts(), 1);
        assert_eq!(
            merge.resolve(&[Choice::Both]),
            "# Notes\n- mine\n- theirs\n"
        );
        // The same addition on both sides: once.
        assert_eq!(merge_text(base, ours, ours), ours);
    }

    #[test]
    fn a_missing_final_newline_is_no_change() {
        // The saved file ends with a newline, the buffer does not.
        let base = "Thanks.\nLove, A\n";
        let ours = "Thanks a lot.\nLove, A";
        let theirs = "Thanks.\nLove, A\n\nP.S. The beans too.\n";
        assert_eq!(
            merge_text(base, ours, theirs),
            "Thanks a lot.\nLove, A\n\nP.S. The beans too."
        );
        assert_eq!(merge_text(base, ours, base), ours, "the buffer as it was");
        // Also when the last block is a conflict.
        let merge = merge(base, "Thanks.\nLove, Arthur", "Thanks.\nCheers\n");
        assert_eq!(merge.conflicts(), 1);
        assert_eq!(merge.resolve(&[Choice::Mine]), "Thanks.\nLove, Arthur");
    }

    fn merge_text(base: &str, ours: &str, theirs: &str) -> String {
        merge(base, ours, theirs).text().unwrap()
    }

    #[test]
    fn changes_to_different_words_of_a_line_merge_by_words() {
        let base = "The quick brown fox jumps.\n";
        let ours = "The slow brown fox jumps.\n";
        let theirs = "The quick brown fox leaps.\n";
        assert_eq!(
            merge_text(base, ours, theirs),
            "The slow brown fox leaps.\n"
        );
    }

    #[test]
    fn insertions_next_to_each_other_stay_in_order() {
        let base = "a\nb\n";
        let ours = "a\nmine\nb\n";
        let theirs = "a\nb\ntheirs\n";
        assert_eq!(merge_text(base, ours, theirs), "a\nmine\nb\ntheirs\n");
    }

    #[test]
    fn a_deletion_against_an_edit_conflicts() {
        let base = "keep\ndrop me\nkeep\n";
        let ours = "keep\nkeep\n";
        let theirs = "keep\ndrop me, edited\nkeep\n";
        let merge = merge(base, ours, theirs);
        assert_eq!(merge.conflicts(), 1);
        assert_eq!(merge.resolve(&[Choice::Theirs]), theirs);
        assert_eq!(merge.resolve(&[Choice::Mine]), ours);
    }

    /// Random edits of random texts: a side left as the base never
    /// changes the other side's text, whatever the edits.
    #[test]
    fn a_side_left_unchanged_yields_the_other_side() {
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        const WORDS: [&str; 6] = ["alpha", "beta", "gamma", " ", "\n", "delta epsilon"];
        for _ in 0..300 {
            let base: String = (0..next(40)).map(|_| WORDS[next(WORDS.len())]).collect();
            let mut side = base.clone();
            for _ in 0..next(4) {
                let at = side
                    .char_indices()
                    .map(|(i, _)| i)
                    .nth(next(side.len() + 1))
                    .unwrap_or(side.len());
                match next(3) {
                    0 => side.insert_str(at, WORDS[next(WORDS.len())]),
                    1 => {
                        let end = side[at..]
                            .char_indices()
                            .map(|(i, _)| at + i)
                            .nth(next(6))
                            .unwrap_or(side.len());
                        side.replace_range(at..end, "");
                    }
                    _ => side.insert_str(at, "new line\n"),
                }
            }
            assert_eq!(
                merge(&base, &side, &base).text().as_deref(),
                Some(side.as_str()),
                "{base:?} {side:?}"
            );
            assert_eq!(
                merge(&base, &base, &side).text().as_deref(),
                Some(side.as_str()),
                "{base:?} {side:?}"
            );
            assert_eq!(
                merge(&base, &side, &side).text().as_deref(),
                Some(side.as_str()),
                "{base:?} {side:?}"
            );
        }
    }

    #[test]
    fn a_line_diff_marks_what_changed() {
        assert_eq!(
            line_diff("a\nb\nc\n", "a\nB\nc\nd\n"),
            [
                DiffLine::Same("a".into()),
                DiffLine::Removed("b".into()),
                DiffLine::Added("B".into()),
                DiffLine::Same("c".into()),
                DiffLine::Added("d".into()),
            ]
        );
    }

    #[test]
    fn identical_texts_and_empty_ones_merge() {
        assert_eq!(merge_text("", "", ""), "");
        assert_eq!(merge_text("x\n", "x\n", "x\n"), "x\n");
        assert_eq!(merge_text("", "mine\n", ""), "mine\n");
        assert_eq!(
            merge_text("no newline", "no newline", "no newline!"),
            "no newline!"
        );
    }
}
