#[path = "support/corpus.rs"]
mod corpus;

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
            .and_then(|count| corpus::generate(Path::new(dir), count).map_err(|e| e.to_string())),
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
