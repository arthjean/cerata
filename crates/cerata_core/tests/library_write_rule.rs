#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use cerata_core::fs::text::TextFormat;
use cerata_core::images::{ImageKind, ImportSource, plan_import, run_import};
use cerata_core::markdown::is_markdown_path;
use cerata_core::paths::{CONFIG_FILE, LIBRARY_ID_FILE, WORKSPACE_DIR};
use cerata_core::settings::{ConfigValue, Layer};
use cerata_core::startup::CoreApp;
use tempfile::TempDir;

fn violations(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                stack.push(path);
            } else if !allowed(&relative) {
                found.push(relative);
            }
        }
    }
    found.sort();
    found
}

fn allowed(relative: &Path) -> bool {
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [dir, file] if dir == WORKSPACE_DIR => file == CONFIG_FILE || file == LIBRARY_ID_FILE,
        [dir, ..] if dir == WORKSPACE_DIR => false,
        _ => {
            let in_assets = parts[..parts.len() - 1]
                .iter()
                .any(|part| part.ends_with("-assets") && !part.starts_with('.'));
            let hidden = parts.iter().any(|part| part.starts_with('.'));
            !hidden && (in_assets || is_markdown_path(relative))
        }
    }
}

fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::new(2, 2)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[test]
fn library_write_rule() {
    let config = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let library = TempDir::new().unwrap();
    let app = CoreApp::new(config.path().to_path_buf(), data.path().to_path_buf());
    let (window, _) = app.create_window().unwrap();
    let (sender, _events) = mpsc::channel();
    let (info, _) = app.open_workspace(&window, library.path(), sender).unwrap();
    let runtime = window.runtime().unwrap();
    let fs = &runtime.fs;
    let root = info.root.clone();

    let a = fs.create_file(&root.join("a.md")).unwrap().path;
    fs.write_file(&a, "# A\n\nbody", TextFormat::default())
        .unwrap();
    fs.create_directory(&root.join("dir")).unwrap();
    let b = fs.create_file(&root.join("dir").join("b.md")).unwrap().path;
    fs.write_file(&b, "# B", TextFormat::default()).unwrap();

    let planned = plan_import(
        Some(&a),
        &root,
        vec![ImportSource::Bytes {
            bytes: png(),
            declared: ImageKind::Png,
            stem: "20261103-120000-abcd".into(),
        }],
    )
    .remove(0)
    .unwrap();
    let markdown = run_import(fs, planned).unwrap();
    fs.write_file(&a, &format!("# A\n\n{markdown}\n"), TextFormat::default())
        .unwrap();
    assert!(root.join("a-assets").is_dir());

    let renamed = root.join("renamed.md");
    fs.rename_note(&a, &renamed).unwrap();
    fs.duplicate_note(&renamed, None).unwrap();
    window.settings().load_workspace(&root).unwrap();
    window
        .settings()
        .set(
            "editor.font-size",
            ConfigValue::Number(17.0),
            Layer::Workspace,
        )
        .unwrap();

    if let Err(error) = fs.delete(&b) {
        eprintln!("delete skipped: no trash available here: {error}");
    }

    assert!(root.join(WORKSPACE_DIR).join(LIBRARY_ID_FILE).is_file());
    assert!(root.join(WORKSPACE_DIR).join(CONFIG_FILE).is_file());
    let outside = violations(&root);
    assert!(
        outside.is_empty(),
        "Cerata wrote outside the ADR 0009 rule: {outside:?}"
    );
}

#[test]
fn library_write_rule_names_a_stray_file() {
    let root = TempDir::new().unwrap();
    std::fs::create_dir_all(root.path().join(WORKSPACE_DIR)).unwrap();
    std::fs::write(root.path().join(WORKSPACE_DIR).join("store.sqlite"), "").unwrap();
    std::fs::write(root.path().join("note.md"), "").unwrap();
    std::fs::write(root.path().join(".lock"), "").unwrap();
    std::fs::create_dir_all(root.path().join("x-assets")).unwrap();
    std::fs::write(root.path().join("x-assets").join("p.png"), "").unwrap();
    std::fs::write(root.path().join("index.json"), "").unwrap();
    assert_eq!(
        violations(root.path()),
        [
            PathBuf::from(".cerata").join("store.sqlite"),
            PathBuf::from(".lock"),
            PathBuf::from("index.json"),
        ]
    );
}
