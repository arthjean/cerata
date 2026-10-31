//! US-003 throwaway spike: tree-sitter-md 0.5.4 vs pulldown-cmark 0.13.6.
//! Decision and numbers: `docs/adr/0002-markdown-parser.md`.
//!
//! `cargo run --release -p cerata_markdown --example parser_spike -- <mode>`
//! with mode `trees <fixture>..`, `coverage`, `bench` or `all` (default).
//! `cargo test -p cerata_markdown --example parser_spike` pins the coverage
//! matrix, the marker inventory and incremental correctness.

#[path = "parser_spike/bench.rs"]
mod bench;
#[path = "parser_spike/corpus.rs"]
mod corpus;
#[path = "parser_spike/pd.rs"]
mod pd;
#[path = "parser_spike/probes.rs"]
mod probes;
#[path = "parser_spike/ts.rs"]
mod ts;

use std::path::PathBuf;

use corpus::{CASES, Case, Rule};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parser_spike")
}

fn load(id: &str) -> Result<String, String> {
    let path = fixture_dir().join(format!("{id}.md"));
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn print_trees(id: &str, text: &str) -> Result<(), String> {
    println!("=== {id} {text:?}");
    let doc = ts::TsDoc::new(text.to_string(), false)?;
    println!(
        "-- tree-sitter-md, raw input (ERROR nodes: {})",
        doc.has_error()
    );
    for n in doc.nodes() {
        let slice = &text[n.start..n.end];
        let shown = if n.named && slice.len() > 40 {
            "…"
        } else {
            slice
        };
        let kind = if n.named {
            n.kind.clone()
        } else {
            format!("{:?}", n.kind)
        };
        let mark = if n.inline { "~" } else { "" };
        println!(
            "{}{mark}{kind} {}..{} {shown:?}",
            "  ".repeat(n.depth),
            n.start,
            n.end
        );
    }
    println!("-- pulldown-cmark");
    for (event, range) in pd::events(text, pd::options()) {
        println!("{event:?} {range:?} {:?}", &text[range.clone()]);
    }
    Ok(())
}

/// Native outcome of one case: tree-sitter on the raw buffer, tree-sitter
/// with the virtual trailing newline, pulldown-cmark.
struct Outcome {
    ts_raw: bool,
    ts: bool,
    pd: bool,
    lines: Vec<String>,
}

fn evaluate(case: &Case) -> Result<Outcome, String> {
    let text = load(case.id)?;
    let raw = ts::TsDoc::new(text.clone(), false)?;
    let nl = ts::TsDoc::new(text.clone(), true)?;
    let mut out = Outcome {
        ts_raw: true,
        ts: true,
        pd: true,
        lines: Vec::new(),
    };
    for (probe, expected) in case.checks {
        let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        let got_raw = probes::ts(&raw, *probe);
        let got_ts = probes::ts(&nl, *probe);
        let got_pd = probes::pd(&text, *probe);
        let mark = |ok: bool| if ok { "ok" } else { "KO" };
        out.ts_raw &= got_raw == expected;
        out.ts &= got_ts == expected;
        out.pd &= got_pd == expected;
        out.lines.push(format!(
            "  {:<34} {:<11} writer={expected:?}\n      ts-raw {} {got_raw:?} | ts {} {got_ts:?} | pd {} {got_pd:?}",
            case.id,
            format!("{probe:?}"),
            mark(got_raw == expected),
            mark(got_ts == expected),
            mark(got_pd == expected),
        ));
    }
    Ok(out)
}

fn coverage() -> Result<(), String> {
    println!("# Native coverage against Writer's expected results");
    println!("ts-raw: tree-sitter-md on the raw buffer; ts: with the virtual trailing newline;");
    println!(
        "pd: pulldown-cmark with pd::options(). Url/Title for pd are values (no source range).\n"
    );
    let mut summary = Vec::new();
    let mut drift = Vec::new();
    for rule in Rule::ALL {
        println!("## {}", rule.label());
        let (mut n, mut raw, mut t, mut p) = (0, 0, 0, 0);
        for case in CASES.iter().filter(|c| c.rule == rule) {
            let o = evaluate(case)?;
            for line in &o.lines {
                println!("{line}");
            }
            if (o.ts, o.pd) != (case.ts_native, case.pd_native) {
                drift.push(case.id);
            }
            n += 1;
            raw += usize::from(o.ts_raw);
            t += usize::from(o.ts);
            p += usize::from(o.pd);
        }
        summary.push((rule, n, raw, t, p));
    }
    println!("\n## Summary (cases matching Writer natively)");
    println!(
        "  {:<78} {:>5} {:>6} {:>4} {:>4}",
        "rule", "cases", "ts-raw", "ts", "pd"
    );
    for (rule, n, raw, t, p) in summary {
        println!("  {:<78} {n:>5} {raw:>6} {t:>4} {p:>4}", rule.label());
    }
    println!("  matches the matrix recorded in corpus.rs (and the ADR): {drift:?} differ");
    let markers = load("markers")?;
    let doc = ts::TsDoc::new(markers, true)?;
    let kinds: Vec<String> = doc.nodes().into_iter().map(|n| n.kind).collect();
    println!("\n## tree-sitter-md marker-level nodes in markers.md");
    for kind in corpus::TS_MARKER_KINDS {
        let present = kinds.iter().any(|k| k == kind);
        println!(
            "  {kind:<28} {}",
            if present { "present" } else { "MISSING" }
        );
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map_or("all", String::as_str) {
        "trees" => {
            for id in &args[1..] {
                print_trees(id, &load(id)?)?;
            }
        }
        "coverage" => coverage()?,
        "bench" => bench::run()?,
        "all" => {
            coverage()?;
            println!();
            bench::run()?;
        }
        mode => return Err(format!("unknown mode {mode}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_has_a_case_and_loads() {
        let mut files: Vec<String> = std::fs::read_dir(fixture_dir())
            .unwrap()
            .filter_map(|e| e.unwrap().file_name().into_string().ok())
            .filter_map(|f| f.strip_suffix(".md").map(str::to_string))
            .filter(|f| f != "markers")
            .collect();
        files.sort();
        let mut ids: Vec<String> = CASES.iter().map(|c| c.id.to_string()).collect();
        ids.sort();
        assert_eq!(files, ids);
        for id in &ids {
            assert!(!load(id).unwrap().contains('\r'), "{id} must stay LF");
        }
    }

    /// Pins the matrix the ADR reports.
    #[test]
    fn native_coverage_matches_the_recorded_matrix() {
        for case in CASES {
            let o = evaluate(case).unwrap();
            assert_eq!(
                (o.ts, o.pd),
                (case.ts_native, case.pd_native),
                "{}:\n{}",
                case.id,
                o.lines.join("\n")
            );
        }
    }

    /// Without the virtual newline, tree-sitter-md misses EOF line ends.
    #[test]
    fn raw_tree_sitter_misses_eof_line_ends() {
        for id in [
            "setext-double-dash",
            "setext-single-equal",
            "frontmatter-eof-no-newline",
        ] {
            let case = CASES.iter().find(|c| c.id == id).unwrap();
            let o = evaluate(case).unwrap();
            assert!(!o.ts_raw && o.ts, "{id}");
        }
        let raw = ts::TsDoc::new(load("setext-single-dash-space").unwrap(), false).unwrap();
        assert!(raw.has_error());
        let nl = ts::TsDoc::new(load("setext-single-dash-space").unwrap(), true).unwrap();
        assert!(!nl.has_error());
    }

    #[test]
    fn tree_sitter_exposes_marker_nodes() {
        let doc = ts::TsDoc::new(load("markers").unwrap(), true).unwrap();
        let kinds: Vec<String> = doc.nodes().into_iter().map(|n| n.kind).collect();
        for kind in corpus::TS_MARKER_KINDS {
            assert!(kinds.iter().any(|k| k == kind), "missing {kind}");
        }
    }

    /// Range-scoped incremental reparse must equal a fresh parse after every
    /// edit, including edits that change block structure.
    #[test]
    fn incremental_reparse_equals_fresh_parse() {
        let mut text = load("markers").unwrap();
        text.push_str("Para *a* [l](u)\nnext line\n\n- one\n- two\n");
        let mut doc = ts::TsDoc::new(text, true).unwrap();
        let inserts = ["x", "\n", "-", " ", "*", "`", "$", "# ", "> ", "\n\n", "|"];
        let mut seed = 7usize;
        for step in 0..400 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345) % (1 << 31);
            let len = doc.text.len();
            let mut pos = seed % (len + 1);
            while !doc.text.is_char_boundary(pos) {
                pos -= 1;
            }
            let edit = if step % 3 == 2 && pos < len {
                let mut end = pos + 1;
                while !doc.text.is_char_boundary(end) {
                    end += 1;
                }
                ts::edit_text(&mut doc.text, pos, end, "")
            } else {
                ts::edit_text(&mut doc.text, pos, pos, inserts[seed % inserts.len()])
            };
            doc.reparse(&edit).unwrap();
            let fresh = ts::TsDoc::new(doc.text.clone(), true).unwrap();
            assert_eq!(
                doc.fingerprint(),
                fresh.fingerprint(),
                "step {step}: {:?}",
                doc.text
            );
        }
    }
}
