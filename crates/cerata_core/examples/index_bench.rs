//! Workspace generator and indexing benchmark (US-005, docs/benchmarks.md).
//!
//! cargo run -p cerata_core --release --example index_bench -- generate <dir> <files>
//! cargo run -p cerata_core --release --example index_bench -- index <dir> [runs]
//!
//! `generate` writes `<files>` Markdown notes, 100 per folder, two folder
//! levels deep, plus a `.gitignore`, an ignored folder and non-Markdown
//! files, so the walk exercises the same rules as a real vault.

use cerata_core::epoch::EpochCounter;
use cerata_core::index::index_workspace;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command, dir, count] if command == "generate" => count
            .parse()
            .map_err(|error| format!("invalid file count: {error}"))
            .and_then(|count| generate(Path::new(dir), count).map_err(|e| e.to_string())),
        [command, dir, rest @ ..] if command == "index" => {
            let runs = rest.first().and_then(|r| r.parse().ok()).unwrap_or(5);
            index(Path::new(dir), runs)
        }
        _ => Err("usage: index_bench generate <dir> <files> | index <dir> [runs]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn generate(root: &Path, count: usize) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("ignored-build"))?;
    std::fs::write(root.join(".gitignore"), "ignored-build/\n")?;
    for i in 0..200 {
        std::fs::write(root.join("ignored-build").join(format!("out-{i}.md")), "x")?;
    }
    for i in 0..count {
        let dir = root
            .join(format!("area-{:03}", i / 10_000))
            .join(format!("topic-{:03}", (i / 100) % 100));
        if i % 100 == 0 {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("attachment.png"), [0u8; 16])?;
        }
        std::fs::write(
            dir.join(format!("note {i:06}.md")),
            format!("# Note {i}\n\nBody of note {i}.\n"),
        )?;
    }
    println!("generated {count} notes in {}", root.display());
    Ok(())
}

fn index(root: &Path, runs: usize) -> Result<(), String> {
    let counter = EpochCounter::new();
    let mut timings = Vec::with_capacity(runs);
    let mut file_count = 0;
    for _ in 0..runs {
        let started = Instant::now();
        let index = index_workspace(root, &counter.token()).map_err(|e| e.to_string())?;
        timings.push(started.elapsed());
        file_count = index.files.len();
    }
    timings.sort();
    let first = timings.first().copied().unwrap_or_default();
    let median = timings.get(timings.len() / 2).copied().unwrap_or_default();
    let worst = timings.last().copied().unwrap_or_default();
    println!(
        "indexed {file_count} files in {} runs: min {first:?}, median {median:?}, max {worst:?}",
        timings.len()
    );
    Ok(())
}
