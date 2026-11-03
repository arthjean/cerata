//! The benchmark corpus (docs/benchmarks.md), shared by `index_bench` and
//! `store_bench`: 100 notes per folder on two folder levels, a PNG per
//! folder, a `.gitignore` and an ignored folder of 200 notes.

use std::path::Path;

pub fn generate(root: &Path, count: usize) -> std::io::Result<()> {
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
