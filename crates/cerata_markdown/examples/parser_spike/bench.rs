//! Reparse latency after a one-character edit, both parsers.
//!
//! Each iteration inserts one character at the edit point, reparses (timed),
//! deletes it, reparses again (timed): two samples per iteration. Buffer work
//! (string splice, row/column computation, which a rope does in O(log n))
//! stays outside the timed region; only the parser work is measured.

use std::hint::black_box;
use std::time::{Duration, Instant};

use crate::corpus;
use crate::pd;
use crate::ts::{self, TsDoc, TsNaive};

struct Stats {
    n: usize,
    p50: Duration,
    p99: Duration,
    max: Duration,
}

fn stats(mut samples: Vec<Duration>) -> Stats {
    samples.sort_unstable();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    Stats {
        n: samples.len(),
        p50: at(0.50),
        p99: at(0.99),
        max: samples[samples.len() - 1],
    }
}

fn ms(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64() * 1e3)
}

fn row(label: &str, s: &Stats, extra: &str) {
    println!(
        "  {label:<44} n={:<5} p50={:>8} ms  p99={:>8} ms  max={:>8} ms  {extra}",
        s.n,
        ms(s.p50),
        ms(s.p99),
        ms(s.max)
    );
}

fn median_of<F: FnMut() -> Result<(), String>>(runs: usize, mut f: F) -> Result<Duration, String> {
    let mut v = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        f()?;
        v.push(t.elapsed());
    }
    Ok(stats(v).p50)
}

/// Returns total reparse stats, block-tree share stats, mean inline trees
/// reparsed per edit.
fn bench_ts_smart(
    text: &str,
    pos: usize,
    ch: &str,
    iters: usize,
) -> Result<(Stats, Stats, f64), String> {
    let mut doc = TsDoc::new(text.to_string(), true)?;
    let mut samples = Vec::with_capacity(iters * 2);
    let mut block = Vec::with_capacity(iters * 2);
    let mut reparsed = 0usize;
    for i in 0..iters + 10 {
        for (start, end, new) in [(pos, pos, ch), (pos, pos + ch.len(), "")] {
            let edit = ts::edit_text(&mut doc.text, start, end, new);
            let t = Instant::now();
            let n = doc.reparse(&edit)?;
            let d = t.elapsed();
            if i >= 10 {
                samples.push(d);
                block.push(doc.last_block_time);
                reparsed += n;
            }
        }
    }
    // Correctness: incremental result == fresh parse, after an insert and
    // after the matching delete.
    let insert = ts::edit_text(&mut doc.text, pos, pos, ch);
    doc.reparse(&insert)?;
    if doc.fingerprint() != TsDoc::new(doc.text.clone(), true)?.fingerprint() {
        return Err("tree-sitter incremental tree differs from a fresh parse (insert)".into());
    }
    let delete = ts::edit_text(&mut doc.text, pos, pos + ch.len(), "");
    doc.reparse(&delete)?;
    if doc.fingerprint() != TsDoc::new(doc.text.clone(), true)?.fingerprint() {
        return Err("tree-sitter incremental tree differs from a fresh parse (delete)".into());
    }
    Ok((
        stats(samples),
        stats(block),
        reparsed as f64 / (iters * 2) as f64,
    ))
}

fn bench_ts_naive(text: &str, pos: usize, ch: &str, iters: usize) -> Result<Stats, String> {
    let mut doc = TsNaive::new(text.to_string())?;
    let mut samples = Vec::with_capacity(iters * 2);
    for i in 0..iters + 3 {
        let insert = ts::edit_text(&mut doc.text, pos, pos, ch);
        let t = Instant::now();
        doc.reparse(&insert)?;
        let d1 = t.elapsed();
        let delete = ts::edit_text(&mut doc.text, pos, pos + ch.len(), "");
        let t = Instant::now();
        doc.reparse(&delete)?;
        let d2 = t.elapsed();
        if i >= 3 {
            samples.extend([d1, d2]);
        }
    }
    Ok(stats(samples))
}

fn bench_pd(text: &str, pos: usize, ch: &str, iters: usize) -> Stats {
    let mut text = text.to_string();
    let mut buf = Vec::new();
    let mut samples = Vec::with_capacity(iters * 2);
    for i in 0..iters + 10 {
        text.insert_str(pos, ch);
        let t = Instant::now();
        pd::parse_flat(&text, &mut buf);
        black_box(&buf);
        let d1 = t.elapsed();
        text.replace_range(pos..pos + ch.len(), "");
        let t = Instant::now();
        pd::parse_flat(&text, &mut buf);
        black_box(&buf);
        let d2 = t.elapsed();
        if i >= 10 {
            samples.extend([d1, d2]);
        }
    }
    stats(samples)
}

pub fn run() -> Result<(), String> {
    println!("# Reparse benchmark");
    println!(
        "os={} arch={} threads={} cpu_env={:?} profile={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default(),
        if cfg!(debug_assertions) {
            "debug (numbers meaningless)"
        } else {
            "release"
        }
    );
    let list = corpus::list_3000();
    let list_pos = list
        .find("sub item!!")
        .map(|i| i + "sub item!!".len())
        .ok_or("edit point missing")?;
    let note = corpus::note_1mb();
    let note_pos = note[note.len() / 2..]
        .find("Paragraph text")
        .map(|i| note.len() / 2 + i + "Paragraph".len())
        .ok_or("edit point missing")?;

    for (name, text, pos, budget) in [
        (
            "list-3000 (list-enter-exit.spec.js)",
            &list,
            list_pos,
            "2 ms",
        ),
        ("note-1mb", &note, note_pos, "16 ms"),
    ] {
        println!(
            "\n## {name}: {} bytes, {} lines, edit at byte {pos}, budget {budget}",
            text.len(),
            text.lines().count()
        );
        let fresh = TsDoc::new(text.to_string(), true)?;
        println!(
            "  tree-sitter: {} inline trees, ERROR nodes: {}",
            fresh.inlines.len(),
            fresh.has_error()
        );
        let t_ts = median_of(5, || TsDoc::new(text.to_string(), true).map(drop))?;
        let mut block_parser = tree_sitter::Parser::new();
        block_parser
            .set_language(&tree_sitter_md::LANGUAGE.into())
            .map_err(|e| e.to_string())?;
        let t_block = median_of(5, || {
            block_parser
                .parse(text, None)
                .map(drop)
                .ok_or("parse".into())
        })?;
        let t_naive = median_of(5, || TsNaive::new(text.to_string()).map(drop))?;
        let mut buf = Vec::new();
        let t_pd = median_of(5, || {
            pd::parse_flat(text, &mut buf);
            Ok(())
        })?;
        println!(
            "  initial full parse (median of 5): tree-sitter two-level {} ms (block tree alone {} ms), MarkdownParser {} ms, pulldown-cmark {} ms ({} events)",
            ms(t_ts),
            ms(t_block),
            ms(t_naive),
            ms(t_pd),
            buf.len()
        );
        for (ch, label) in [("x", "insert 'x'/delete"), ("\n", "insert LF/delete")] {
            println!("  edit: {label}");
            let (s, block, avg) = bench_ts_smart(text, pos, ch, 500)?;
            row(
                "tree-sitter incremental (range-scoped)",
                &s,
                &format!("avg inline trees reparsed {avg:.1}; equals fresh parse: yes"),
            );
            row("  of which block tree edit+reparse", &block, "");
            let s = bench_ts_naive(text, pos, ch, 40)?;
            row("tree-sitter-md MarkdownParser (crate wrapper)", &s, "");
            let s = bench_pd(text, pos, ch, 500);
            row("pulldown-cmark full reparse (events+ranges)", &s, "");
        }
    }
    Ok(())
}
