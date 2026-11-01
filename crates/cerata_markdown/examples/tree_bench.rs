//! US-010 reparse budget: `MarkdownTree::reparse` after one keystroke on
//! the 3 000-item list of `list-enter-exit.spec.js` (budget 2 ms) and on a
//! 1 MiB note (budget 16 ms), each keystroke reparsed from the previous
//! tree as `EditorState` does. US-013: Enter after `sub item!!` on the same
//! list, the whole `EditorState::insert_newline` path (budget 16 ms).
//! Run with `--release`.

#[path = "parser_spike/corpus.rs"]
#[allow(dead_code)]
mod corpus;
#[path = "parser_spike/pd.rs"]
#[allow(dead_code)]
mod pd;
#[path = "parser_spike/probes.rs"]
#[allow(dead_code)]
mod probes;
#[path = "parser_spike/ts.rs"]
#[allow(dead_code)]
mod ts;

use cerata_markdown::{Change, ChangeSet, EditorState, MarkdownTree, Selection, Text};
use std::time::{Duration, Instant};

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index].as_secs_f64() * 1000.0
}

fn bench(name: &str, text: &str, pos: usize, budget_ms: f64) {
    let mut samples = Vec::new();
    let mut doc = text.to_string();
    let mut base = MarkdownTree::parse(&doc);
    for i in 0..510 {
        for edit in ["x", "\n"] {
            let before = Text::new(doc.clone());
            let insert =
                ChangeSet::new(&before, vec![Change::insert(pos, edit)]).unwrap_or_default();
            doc.insert_str(pos, edit);
            let start = Instant::now();
            let tree = base.reparse(before.as_str(), &doc, &insert);
            let elapsed = start.elapsed();
            let after = Text::new(doc.clone());
            let delete = ChangeSet::new(&after, vec![Change::delete(pos..pos + edit.len())])
                .unwrap_or_default();
            doc.replace_range(pos..pos + edit.len(), "");
            let start2 = Instant::now();
            base = tree.reparse(after.as_str(), &doc, &delete);
            let elapsed2 = start2.elapsed();
            if i >= 10 {
                samples.push(elapsed);
                samples.push(elapsed2);
            }
        }
    }
    samples.sort();
    let (p50, p99) = (percentile(&samples, 0.5), percentile(&samples, 0.99));
    println!(
        "{name}: {} bytes, {} samples, p50 {p50:.3} ms, p99 {p99:.3} ms, budget {budget_ms} ms: {}",
        text.len(),
        samples.len(),
        if p99 < budget_ms { "ok" } else { "OVER" }
    );
}

fn bench_enter(text: &str, pos: usize, budget_ms: f64) {
    let mut samples = Vec::new();
    let mut state = EditorState::new(text);
    for i in 0..210 {
        let _ = state.select(Selection::cursor(pos));
        let start = Instant::now();
        let handled = state.insert_newline();
        let elapsed = start.elapsed();
        assert!(handled && state.doc()[pos..].starts_with('\n'));
        state.undo();
        if i >= 10 {
            samples.push(elapsed);
        }
    }
    samples.sort();
    let (p50, p99) = (percentile(&samples, 0.5), percentile(&samples, 0.99));
    println!(
        "list-3000 Enter: {} samples, p50 {p50:.3} ms, p99 {p99:.3} ms, budget {budget_ms} ms: {}",
        samples.len(),
        if p99 < budget_ms { "ok" } else { "OVER" }
    );
}

fn main() {
    if cfg!(debug_assertions) {
        println!("debug build: numbers are meaningless, use --release");
    }
    let list = corpus::list_3000();
    let list_pos = list
        .find("sub item!!")
        .map_or(0, |i| i + "sub item!!".len());
    bench("list-3000", &list, list_pos, 2.0);
    bench_enter(&list, list_pos, 16.0);
    let note = corpus::note_1mb();
    let note_pos = note[note.len() / 2..]
        .find("Paragraph text")
        .map_or(0, |i| note.len() / 2 + i + "Paragraph".len());
    bench("note-1mb", &note, note_pos, 16.0);
}
