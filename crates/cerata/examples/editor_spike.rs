//! US-002 spike (throwaway): a dedicated GPUI editor element with variable
//! line heights, PLAN.md section 3.4 "Option C". See
//! `docs/adr/0001-editor-engine.md` for what it proves and the measurements.
//!
//! ```text
//! cargo run -p cerata --example editor_spike --release -- [<note.md>]
//! cargo run -p cerata --example editor_spike --release -- --bench-typing [--chars N] [<note.md>]
//! cargo run -p cerata --example editor_spike --release -- --write-fixture <dir>
//! ```
//!
//! Without a note, the deterministic 1 MB fixture is generated into the
//! system temp directory.

// Hidden ranges and selections are `Vec<Range>`/`&[Range]`; one-range
// literals are intended, not a mistaken `(a..b).collect()`.
#![allow(clippy::single_range_in_vec_init)]

#[path = "editor_spike/bench.rs"]
mod bench;
#[path = "editor_spike/buffer.rs"]
mod buffer;
#[path = "editor_spike/display.rs"]
mod display;
#[path = "editor_spike/editor.rs"]
mod editor;
#[path = "editor_spike/element.rs"]
mod element;
#[path = "editor_spike/fixture.rs"]
mod fixture;
#[path = "editor_spike/height_map.rs"]
mod height_map;
#[path = "editor_spike/markup.rs"]
mod markup;
#[cfg(test)]
#[path = "editor_spike/tests.rs"]
mod tests;

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};
use gpui::{App, AppContext as _, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;

use crate::{
    bench::BenchState,
    editor::{
        Backspace, Delete, Editor, Left, Newline, Quit, Right, SelectAll, SelectLeft, SelectRight,
    },
    markup::{LineKind, scan_line},
};

struct Args {
    bench_typing: bool,
    chars: usize,
    note: Option<PathBuf>,
    write_fixture: Option<PathBuf>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        bench_typing: false,
        chars: 200,
        note: None,
        write_fixture: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bench-typing" => args.bench_typing = true,
            "--chars" => {
                args.chars = iter
                    .next()
                    .context("--chars needs a number")?
                    .parse()
                    .context("--chars needs a number")?;
            }
            "--write-fixture" => {
                args.write_fixture =
                    Some(iter.next().context("--write-fixture needs a dir")?.into());
            }
            other if other.starts_with("--") => bail!("unknown flag {other}"),
            other => args.note = Some(other.into()),
        }
    }
    Ok(args)
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = parse_args()?;
    if let Some(dir) = &args.write_fixture {
        let path = fixture::write(dir)?;
        println!("wrote {}", path.display());
        return Ok(());
    }
    let note = match &args.note {
        Some(note) => note.clone(),
        None => fixture::write(&std::env::temp_dir().join("cerata-editor-spike"))?,
    };
    let started = Instant::now();
    let text =
        std::fs::read_to_string(&note).with_context(|| format!("reading {}", note.display()))?;
    let image_dir = note.parent().map(Path::to_path_buf).unwrap_or_default();
    let header = format!(
        "bench-typing: {} ({} bytes, {} lines), {} chars\n",
        note.display(),
        text.len(),
        text.lines().count(),
        args.chars
    );

    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("enter", Newline, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new(
                if cfg!(target_os = "macos") {
                    "cmd-a"
                } else {
                    "ctrl-a"
                },
                SelectAll,
                None,
            ),
            KeyBinding::new(
                if cfg!(target_os = "macos") {
                    "cmd-q"
                } else {
                    "ctrl-q"
                },
                Quit,
                None,
            ),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let mut editor = Editor::new(text, image_dir, cx);
                    window.focus(&editor.focus_handle, cx);
                    if args.bench_typing {
                        place_bench_caret(&mut editor);
                    }
                    editor
                })
            },
        );
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                log::error!("failed to open the spike window: {error:#}");
                cx.quit();
                return;
            }
        };
        cx.activate(true);

        if args.bench_typing {
            let Ok(editor) = window.root(cx) else {
                cx.quit();
                return;
            };
            let mut state = BenchState::new(args.chars, window.into(), editor.downgrade(), started);
            state.report_header = header;
            let state = Rc::new(RefCell::new(state));
            editor.update(cx, |editor, _| editor.bench = Some(state.clone()));
            cx.spawn(async move |cx| {
                // Let the window show, fonts load and the first frames settle.
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                bench::inject(&state, cx);
            })
            .detach();
        }
    });
    Ok(())
}

/// Puts the caret at the end of a paragraph in the middle of the note, a
/// few lines below a heading and with strong text and links nearby, so
/// every keystroke runs the hide decisions of a busy viewport.
fn place_bench_caret(editor: &mut Editor) {
    let lines = editor.buffer.line_count();
    let target = (lines / 2..lines)
        .find(|&ix| {
            let text = editor.buffer.line_text(ix);
            text.len() > 200 && scan_line(text).kind == LineKind::Paragraph
        })
        .unwrap_or(0);
    let end = editor.buffer.line_range(target).end;
    editor.selected_range = end..end;
    editor.scroll_line = target.saturating_sub(6);
    editor.scroll_offset = 0.0;
}
