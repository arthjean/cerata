//! History growth benchmark (US-088; docs/benchmarks.md).
//!
//! cargo run -p cerata_core --release --example history_bench -- year [seed]
//! cargo run -p cerata_core --release --example history_bench -- record [versions]
//!
//! cargo run -p cerata_core --release --example history_bench -- merge [runs]
//!
//! `merge` times the three-way merge of a 1 MB note (US-092): the buffer
//! and the disk each changed 50 paragraphs of their own, plus 5 both
//! changed differently (conflicts).
//!
//! `record` times what a version adds to a save of a 100 KB note through
//! the history itself (`History::record`, then the wait for its commit:
//! hash, delta on the previous version, zstd, insert), off the UI thread.
//!
//! Simulates one year of sustained use: 300 notes of 1 to 20 KB, 20 notes
//! edited a day in typing sessions, a version when typing pauses for 30 s
//! and at the end of each session, identical successive contents kept once,
//! and the retention tiers (`history::retention`) applied at the end of
//! each simulated day. The versions left at the end are then stored with
//! three codecs, each in a SQLite file of the history's shape (one row per
//! version, one per distinct blob), vacuumed, and measured:
//!
//! - `zstd-3`: each blob compressed alone, zstd level 3;
//! - `zstd-3+dict`: the same with a dictionary trained on the 300 notes as
//!   first written (more than 100 samples, as zstd's guide asks);
//! - `delta+zstd-3`: each blob compressed with the note's previous kept
//!   version as zstd's dictionary (a delta), the first one alone and one
//!   in every `MAX_CHAIN + 1` alone again, as the history stores them.

use cerata_core::history::retention::{Dated, kept};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Instant;

const NOTES: usize = 300;
const EDITED_PER_DAY: usize = 20;
const DAYS: i64 = 365;
const MIN_NOTE: usize = 1024;
const MAX_NOTE: usize = 20 * 1024;
const PAUSE_FOR_VERSION: i64 = 30;
const LEVEL: i32 = 3;
const DICT_SIZE: usize = 64 * 1024;
/// A delta chain longer than this starts again from a full blob, as the
/// history does (`history::MAX_CHAIN`).
const MAX_CHAIN: usize = cerata_core::history::MAX_CHAIN;
const DAY: i64 = 86_400;
const WARMUPS: usize = 3;
/// 2026-01-01T00:00:00Z.
const START: i64 = 1_767_225_600;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command, rest @ ..] if command == "year" => {
            let seed = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1);
            year(seed)
        }
        [command, rest @ ..] if command == "record" => {
            record(rest.first().and_then(|s| s.parse().ok()).unwrap_or(500))
        }
        [command, rest @ ..] if command == "merge" => {
            merge(rest.first().and_then(|s| s.parse().ok()).unwrap_or(7))
        }
        _ => Err("usage: history_bench year [seed] | record [versions] | merge [runs]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

// Text -------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn range(&mut self, low: usize, high: usize) -> usize {
        low + self.below(high - low + 1)
    }

    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 1_000_000) as f64 / 1_000_000.0 < p
    }
}

/// Sentences of real Markdown prose: those of the repository's ADRs and
/// research notes (French), so the text compresses as notes do.
struct Words(Vec<String>);

impl Words {
    fn new() -> Result<Self, String> {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        let mut files = Vec::new();
        for dir in [docs.join("adr"), docs.join("research")] {
            for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().is_some_and(|ext| ext == "md") {
                    files.push(path);
                }
            }
        }
        files.sort();
        let mut sentences = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            for line in text.lines() {
                let line = line.trim_start_matches(['-', '*', '#', '>', ' ', '|']);
                for sentence in line.split_inclusive(". ") {
                    let sentence = sentence.trim();
                    if sentence.len() >= 20 {
                        sentences.push(sentence.to_owned());
                    }
                }
            }
        }
        if sentences.len() < 500 {
            return Err(format!("only {} sentences found", sentences.len()));
        }
        Ok(Self(sentences))
    }

    fn sentences(&self) -> usize {
        self.0.len()
    }

    fn word<'a>(&'a self, rng: &mut Rng) -> &'a str {
        let sentence = &self.0[rng.below(self.0.len())];
        let words: Vec<&str> = sentence.split_whitespace().collect();
        words[rng.below(words.len())]
    }

    fn sentence(&self, rng: &mut Rng) -> String {
        self.0[rng.below(self.0.len())].clone()
    }

    fn paragraph(&self, rng: &mut Rng) -> String {
        if rng.chance(0.25) {
            let items = rng.range(2, 6);
            return (0..items)
                .map(|_| format!("- {}", self.sentence(rng)))
                .collect::<Vec<_>>()
                .join("\n");
        }
        let count = rng.range(2, 6);
        (0..count)
            .map(|_| self.sentence(rng))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn note(&self, rng: &mut Rng, target: usize) -> Vec<String> {
        let mut blocks = vec![format!("# {}", self.sentence(rng).trim_end_matches('.'))];
        let mut size = blocks[0].len();
        while size < target {
            let block = if rng.chance(0.15) {
                format!("## {}", self.sentence(rng).trim_end_matches('.'))
            } else {
                self.paragraph(rng)
            };
            size += block.len() + 2;
            blocks.push(block);
        }
        blocks
    }
}

fn render(blocks: &[String]) -> String {
    let mut text = blocks.join("\n\n");
    text.push('\n');
    text
}

/// One typing burst: a few edits of the note's blocks.
fn burst(blocks: &mut Vec<String>, words: &Words, rng: &mut Rng, cap: usize) {
    for _ in 0..rng.range(1, 3) {
        let at = rng.range(1, blocks.len().saturating_sub(1).max(1));
        match rng.below(20) {
            0..=6 => {
                // Extend a paragraph with a sentence.
                if let Some(block) = blocks.get_mut(at) {
                    block.push(' ');
                    block.push_str(&words.sentence(rng));
                }
            }
            7..=12 => {
                // Rewrite a word.
                if let Some(block) = blocks.get_mut(at) {
                    let spaces: Vec<usize> = block.match_indices(' ').map(|(i, _)| i).collect();
                    if spaces.len() > 2 {
                        let i = rng.below(spaces.len() - 1);
                        let (start, end) = (spaces[i] + 1, spaces[i + 1]);
                        block.replace_range(start..end, words.word(rng));
                    }
                }
            }
            13 => blocks.insert(at, words.paragraph(rng)),
            14 => {
                if let Some(block) = blocks.get_mut(at) {
                    block.push_str("\n- ");
                    block.push_str(&words.sentence(rng));
                }
            }
            15 => {
                // Delete a paragraph.
                if blocks.len() > 3 {
                    blocks.remove(at);
                }
            }
            _ => {
                // Delete a sentence.
                if let Some(block) = blocks.get_mut(at)
                    && let Some(dot) = block.find(". ")
                {
                    block.replace_range(..dot + 2, "");
                }
            }
        }
    }
    while render(blocks).len() > cap && blocks.len() > 2 {
        let at = rng.range(1, blocks.len() - 1);
        blocks.remove(at);
    }
}

// Simulation -------------------------------------------------------------

struct Version {
    ts: i64,
    hash: [u8; 32],
}

struct Note {
    blocks: Vec<String>,
    versions: Vec<Version>,
}

fn year(seed: u64) -> Result<(), String> {
    let started = Instant::now();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
    let words = Words::new()?;
    let mut contents: HashMap<[u8; 32], String> = HashMap::new();
    let mut notes: Vec<Note> = (0..NOTES)
        .map(|_| {
            let target = rng.range(MIN_NOTE, MAX_NOTE);
            Note {
                blocks: words.note(&mut rng, target),
                versions: Vec::new(),
            }
        })
        .collect();
    let first_texts: Vec<String> = notes.iter().map(|note| render(&note.blocks)).collect();
    // A working set: most days edit the same notes.
    let active: Vec<usize> = (0..40).map(|_| rng.below(NOTES)).collect();

    let mut created = 0usize;
    let mut created_per_day = Vec::new();
    let record = |note: &mut Note, ts: i64, contents: &mut HashMap<[u8; 32], String>| {
        let text = render(&note.blocks);
        let hash = *blake3::hash(text.as_bytes()).as_bytes();
        if note.versions.last().is_some_and(|last| last.hash == hash) {
            return false;
        }
        contents.entry(hash).or_insert(text);
        note.versions.push(Version { ts, hash });
        true
    };

    for day in 0..DAYS {
        let mut today = 0;
        let day_start = START + day * DAY;
        let mut edited = Vec::with_capacity(EDITED_PER_DAY);
        while edited.len() < EDITED_PER_DAY {
            let n = if rng.chance(0.8) {
                active[rng.below(active.len())]
            } else {
                rng.below(NOTES)
            };
            if !edited.contains(&n) {
                edited.push(n);
            }
        }
        // (time, note) of each session, in time order.
        let mut sessions: Vec<(i64, usize)> = Vec::new();
        for &n in &edited {
            for _ in 0..rng.range(1, 3) {
                let at = 9 * 3600 + rng.below(10 * 3600) as i64;
                sessions.push((day_start + at, n));
            }
        }
        sessions.sort_unstable();
        for (mut ts, n) in sessions {
            let note = &mut notes[n];
            let bursts = rng.range(2, 8);
            for b in 0..bursts {
                burst(&mut note.blocks, &words, &mut rng, MAX_NOTE);
                ts += rng.range(20, 120) as i64;
                let last = b + 1 == bursts;
                let long_pause = rng.chance(0.4);
                if last || long_pause {
                    // The version comes 30 s after the last keystroke.
                    if record(note, ts + PAUSE_FOR_VERSION, &mut contents) {
                        created += 1;
                        today += 1;
                    }
                    ts += PAUSE_FOR_VERSION + rng.range(0, 600) as i64;
                } else {
                    ts += rng.range(2, 25) as i64;
                }
            }
        }
        created_per_day.push(today);
        // Retention at the end of the day.
        let now = day_start + DAY - 1;
        for note in &mut notes {
            let dated: Vec<Dated> = note
                .versions
                .iter()
                .map(|v| Dated {
                    ts: v.ts,
                    pinned: false,
                })
                .collect();
            let keep = kept(&dated, now);
            let mut index = 0;
            note.versions.retain(|_| {
                let keep = keep[index];
                index += 1;
                keep
            });
        }
    }

    let kept_versions: usize = notes.iter().map(|note| note.versions.len()).sum();
    let mut referenced: Vec<[u8; 32]> = notes
        .iter()
        .flat_map(|note| note.versions.iter().map(|v| v.hash))
        .collect();
    referenced.sort_unstable();
    referenced.dedup();
    let raw: usize = referenced
        .iter()
        .filter_map(|hash| contents.get(hash))
        .map(String::len)
        .sum();
    let mean = created as f64 / DAYS as f64;
    println!("{} sentences in the pool", words.sentences());
    println!(
        "simulated {DAYS} days in {:?}: {created} versions created ({mean:.1} a day, max {}), \
         {kept_versions} kept, {} distinct blobs, {:.2} MiB uncompressed",
        started.elapsed(),
        created_per_day.iter().max().copied().unwrap_or(0),
        referenced.len(),
        raw as f64 / 1_048_576.0
    );

    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let samples: Vec<&[u8]> = first_texts.iter().map(|text| text.as_bytes()).collect();
    let dict = zstd::dict::from_samples(&samples, DICT_SIZE).map_err(|e| e.to_string())?;

    for codec in [Codec::Plain, Codec::Dict, Codec::Delta] {
        let path = dir.path().join(format!("{}.sqlite", codec.name()));
        let timer = Instant::now();
        let size = store_size(&path, codec, &notes, &contents, &dict)?;
        println!(
            "{:<13} {:>12} bytes ({:.2} MiB) in {:?}",
            codec.name(),
            size,
            size as f64 / 1_048_576.0,
            timer.elapsed()
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Codec {
    Plain,
    Dict,
    Delta,
}

impl Codec {
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "zstd-3",
            Self::Dict => "zstd-3+dict",
            Self::Delta => "delta+zstd-3",
        }
    }
}

/// Size of a vacuumed SQLite file holding the kept versions with `codec`.
fn store_size(
    path: &std::path::Path,
    codec: Codec,
    notes: &[Note],
    contents: &HashMap<[u8; 32], String>,
    dict: &[u8],
) -> Result<u64, String> {
    let error = |e: rusqlite::Error| e.to_string();
    let mut db = Connection::open(path).map_err(error)?;
    db.execute_batch(
        "CREATE TABLE blob (hash BLOB PRIMARY KEY, codec INTEGER NOT NULL, base BLOB,
             bytes BLOB NOT NULL) WITHOUT ROWID;
         CREATE TABLE version (note INTEGER NOT NULL, seq INTEGER NOT NULL, hash BLOB NOT NULL,
             parent BLOB, cause INTEGER NOT NULL, device INTEGER NOT NULL, ts INTEGER NOT NULL,
             pinned INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (note, seq)) WITHOUT ROWID;
         CREATE TABLE dict (bytes BLOB NOT NULL);",
    )
    .map_err(error)?;
    let tx = db.transaction().map_err(error)?;
    if matches!(codec, Codec::Dict) {
        tx.execute("INSERT INTO dict VALUES (?1)", [dict])
            .map_err(error)?;
    }
    let mut plain = zstd::bulk::Compressor::new(LEVEL).map_err(|e| e.to_string())?;
    let mut with_dict =
        zstd::bulk::Compressor::with_dictionary(LEVEL, dict).map_err(|e| e.to_string())?;
    {
        let mut blob = tx
            .prepare("INSERT OR IGNORE INTO blob VALUES (?1, ?2, ?3, ?4)")
            .map_err(error)?;
        let mut version = tx
            .prepare("INSERT INTO version VALUES (?1, ?2, ?3, ?4, 0, 1, ?5, 0)")
            .map_err(error)?;
        for (n, note) in notes.iter().enumerate() {
            let mut parent: Option<[u8; 32]> = None;
            let mut chain = 0;
            for (seq, v) in note.versions.iter().enumerate() {
                let text = contents.get(&v.hash).ok_or("missing content")?;
                let (bytes, base) = match codec {
                    Codec::Plain => (plain.compress(text.as_bytes()), None),
                    Codec::Dict => (with_dict.compress(text.as_bytes()), None),
                    Codec::Delta => match parent
                        .filter(|_| chain < MAX_CHAIN)
                        .and_then(|p| contents.get(&p))
                    {
                        Some(previous) => (
                            zstd::bulk::Compressor::with_dictionary(LEVEL, previous.as_bytes())
                                .and_then(|mut c| c.compress(text.as_bytes())),
                            parent,
                        ),
                        None => (plain.compress(text.as_bytes()), None),
                    },
                };
                let bytes = bytes.map_err(|e| e.to_string())?;
                chain = if base.is_some() { chain + 1 } else { 0 };
                blob.execute(params![
                    v.hash.as_slice(),
                    1,
                    base.map(|b| b.to_vec()),
                    bytes
                ])
                .map_err(error)?;
                version
                    .execute(params![
                        n as i64,
                        seq as i64,
                        v.hash.as_slice(),
                        parent.map(|p| p.to_vec()),
                        v.ts
                    ])
                    .map_err(error)?;
                parent = Some(v.hash);
            }
        }
    }
    tx.commit().map_err(error)?;
    db.execute_batch("VACUUM").map_err(error)?;
    drop(db);
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| e.to_string())
}

/// p50, p99 and max of the time a version of a 100 KB note takes.
fn record(count: usize) -> Result<(), String> {
    use cerata_core::epoch::EpochCounter;
    use cerata_core::history::device::Device;
    use cerata_core::history::{Cause, History, Request, Timing};
    use cerata_core::store::{Store, StoreOpen};
    use std::sync::Arc;
    use std::time::Duration;

    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let StoreOpen::Ready { store, .. } = Store::open(dir.path(), &|_| None) else {
        return Err("no store".into());
    };
    let counter = EpochCounter::new();
    let history = History::start_with(
        store,
        counter.token(),
        Device::load_or_create(dir.path()),
        Arc::new(|message| eprintln!("{message}")),
        Timing {
            save_pause: Duration::from_secs(3600),
            batch_window: Duration::from_millis(1),
            ..Timing::default()
        },
    );
    let words = Words::new()?;
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut blocks = words.note(&mut rng, 100 * 1024);
    let mut times = Vec::with_capacity(count);
    for n in 0..count + WARMUPS {
        burst(&mut blocks, &words, &mut rng, 100 * 1024);
        while render(&blocks).len() > 100 * 1024 && blocks.len() > 2 {
            blocks.remove(1);
        }
        let bytes = render(&blocks).into_bytes();
        let started = Instant::now();
        history.record(Request::new("note.md", bytes, Cause::Save));
        if !history.flush_and_wait(None, Duration::from_secs(10)) {
            return Err("a version took more than 10 s".into());
        }
        if n >= WARMUPS {
            times.push(started.elapsed());
        }
    }
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    println!(
        "{count} versions of a {} KB note: p50 {:?}, p99 {:?}, max {:?}",
        render(&blocks).len() / 1024,
        at(0.5),
        at(0.99),
        times[times.len() - 1]
    );
    Ok(())
}

/// The merge of a 1 MB note, `runs` times after the warm-ups.
fn merge(runs: usize) -> Result<(), String> {
    use cerata_core::merge::merge;
    let words = Words::new()?;
    let mut rng = Rng(0x5ee_d0f3_e26e);
    let base = words.note(&mut rng, 1024 * 1024);
    let (mut ours, mut theirs) = (base.clone(), base.clone());
    let paragraphs = base.len();
    for n in 0..50 {
        let a = 1 + (n * 2 * paragraphs / 110);
        let b = a + paragraphs / 220;
        ours[a] = words.paragraph(&mut rng);
        theirs[b] = words.paragraph(&mut rng);
    }
    for n in 0..5 {
        let c = paragraphs - 2 - n * 7;
        ours[c] = words.paragraph(&mut rng);
        theirs[c] = words.paragraph(&mut rng);
    }
    let (base, ours, theirs) = (render(&base), render(&ours), render(&theirs));
    let mut times = Vec::new();
    let mut conflicts = 0;
    for n in 0..runs + WARMUPS {
        let started = Instant::now();
        let merged = merge(&base, &ours, &theirs);
        let elapsed = started.elapsed();
        conflicts = merged.conflicts();
        if n >= WARMUPS {
            times.push(elapsed);
        }
    }
    times.sort();
    println!(
        "merge of a {} KB note ({} paragraphs), {conflicts} conflicts: median {:?}, max {:?} ({runs} runs)",
        base.len() / 1024,
        paragraphs,
        times[times.len() / 2],
        times[times.len() - 1]
    );
    Ok(())
}
