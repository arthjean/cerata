//! EP-003 benchmark of the real editor in a real window
//! (`docs/adr/0001-editor-engine.md`, retained latency criterion): opening a
//! 1 MB note, typing, and scrolling, timed on the platform's own frames.
//!
//! ```text
//! cargo run -p cerata --example editor_bench --release
//! ```
//!
//! Timestamps per event (keystroke, scroll jump, or the open):
//! - `input`: right before the event is applied. A keystroke goes through
//!   `EntityInputHandler::replace_text_in_range`, the entry point of every
//!   keyboard and IME path; its cost is reported on its own.
//! - `frame`: the next frame callback starts (a `Window::on_next_frame`
//!   callback, which GPUI runs right before `draw`).
//! - `render`: the root view renders, proof that this frame draws.
//! - `presented`: a task spawned from the frame callback, which runs once
//!   the callback has drawn and returned from `present` (DXGI, Metal, wgpu).
//!
//! App work is `presented - frame`; the wait for the refresh is excluded.
//! Each keystroke must be drawn by the first frame after it.
//!
//! A second scenario (US-048, ADR 0007) types 200 characters in a cell in
//! the middle of a drawn table of 200 rows and 8 columns: the editor lays
//! the whole table out in the frame of each keystroke. US-057 adds, on the
//! same table, 60 column insertions ("Insert column right", each undone
//! off the clock), then 200 characters typed in the last cell of a table of
//! 1 000 rows.
//!
//! US-066 resizes an image in the middle of the 1 MB note: 3 x 60 widths
//! of the preview of a handle's gesture, the buffer untouched, each
//! drawn by the frame after it (the lines below reflow in it).

#![allow(clippy::single_range_in_vec_init)]

#[allow(dead_code)]
#[path = "../src/a11y.rs"]
mod a11y;
#[allow(dead_code, unused_imports)]
#[path = "../src/editor/mod.rs"]
mod editor;
#[allow(dead_code)]
#[path = "editor_spike/fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "../src/icons.rs"]
mod icons;
#[allow(dead_code)]
#[path = "../src/input.rs"]
mod input;
#[allow(dead_code)]
#[path = "../src/menu.rs"]
mod menu;
#[allow(dead_code)]
#[path = "../src/select.rs"]
mod select;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, EntityInputHandler,
    IntoElement, ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_platform::application;

use editor::{Editor, EditorConfig};

const CHARS: usize = 200;
const SCROLL_JUMPS: usize = 60;
const TABLE_ROWS: usize = 200;
const TABLE_COLUMNS: usize = 8;
const COLUMN_INSERTS: usize = 60;
const BIG_TABLE_ROWS: usize = 1_000;
const RESIZE_RUNS: usize = 3;
const RESIZE_STEPS: usize = 60;

#[derive(Default)]
struct Timings {
    open: Option<Duration>,
    handler: Vec<Duration>,
    typing: Vec<Duration>,
    scrolling: Vec<Duration>,
    /// Keystrokes in a cell of the 200 x 8 table.
    table: Vec<Duration>,
    /// "Insert column right" in the 200 x 8 table.
    column: Vec<Duration>,
    /// Keystrokes in the last cell of the 1 000 x 8 table.
    big: Vec<Duration>,
    /// Width changes of an image's resize preview.
    resize: Vec<Duration>,
    /// Events whose first frame after them did not render.
    missed: usize,
}

/// A note holding a table of `rows` body rows and `TABLE_COLUMNS`
/// columns, with inline marks in some cells.
fn table_note(rows: usize) -> String {
    let mut out = format!("# Table\n\nA table of {rows} rows.\n\n|");
    for c in 0..TABLE_COLUMNS {
        out.push_str(&format!(" Column {c} |"));
    }
    out.push_str("\n|");
    for _ in 0..TABLE_COLUMNS {
        out.push_str(" --- |");
    }
    out.push('\n');
    for r in 0..rows {
        out.push('|');
        for c in 0..TABLE_COLUMNS {
            match (r + c) % 4 {
                0 => out.push_str(&format!(" row {r} cell {c} |")),
                1 => out.push_str(&format!(" **bold {r}** |")),
                2 => out.push_str(&format!(
                    " `code {c}` and [link](https://example.com/{r}) |"
                )),
                _ => out.push_str(&format!(" {} |", "word ".repeat(1 + r % 5).trim_end())),
            }
        }
        out.push('\n');
    }
    out.push_str("\nAfter the table.\n");
    out
}

struct Root {
    editor: Option<Entity<Editor>>,
    rendered: Rc<RefCell<Option<Instant>>>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The first render after an event: later frames (an image that
        // finished loading) must not hide a missed one.
        self.rendered.borrow_mut().get_or_insert_with(Instant::now);
        div().size_full().children(self.editor.clone())
    }
}

#[derive(Clone, Copy)]
enum Event {
    Open,
    Key(char),
    Scroll,
    /// A registry command, from the caret.
    Command(&'static str),
    Undo,
    /// The resize gesture in flight moves to this window x.
    ResizeTo(f32),
}

struct Bench {
    window: AnyWindowHandle,
    root: Entity<Root>,
    note: String,
    path: std::path::PathBuf,
    rendered: Rc<RefCell<Option<Instant>>>,
    timings: Timings,
    rng: u64,
}

impl Bench {
    /// Uniform in [0, 16.7 ms): events land at every phase of the refresh.
    fn next_delay(&mut self) -> Duration {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        Duration::from_micros(self.rng % 16_667)
    }
}

/// Applies `event`, then waits for the frame that shows it; returns the
/// app work of that frame (`presented - frame`).
async fn measure(bench: &Rc<RefCell<Bench>>, event: Event, cx: &mut AsyncApp) -> Option<Duration> {
    let (window, root) = {
        let b = bench.borrow();
        (b.window, b.root.clone())
    };
    let (tx, rx) = slot();
    let input = Instant::now();
    let rendered = bench.borrow().rendered.clone();
    *rendered.borrow_mut() = None;
    window
        .update(cx, |_, window, cx| {
            match event {
                Event::Open => {
                    let (note, path) = {
                        let b = bench.borrow();
                        (b.note.clone(), b.path.clone())
                    };
                    let editor =
                        cx.new(|cx| Editor::new(note, Some(path), EditorConfig::default(), cx));
                    root.update(cx, |root, cx| {
                        root.editor = Some(editor);
                        cx.notify();
                    });
                }
                Event::Key(ch) => {
                    if let Some(editor) = root.read(cx).editor.clone() {
                        let start = Instant::now();
                        editor.update(cx, |editor, cx| {
                            let mut buf = [0; 4];
                            editor.replace_text_in_range(
                                None,
                                ch.encode_utf8(&mut buf),
                                window,
                                cx,
                            );
                        });
                        bench.borrow_mut().timings.handler.push(start.elapsed());
                    }
                }
                Event::Command(id) => {
                    if let Some(editor) = root.read(cx).editor.clone() {
                        editor.update(cx, |editor, cx| {
                            let clock = editor::actions::clock();
                            editor.update_state(cx, |state| state.run_command(id, &clock));
                        });
                    }
                }
                Event::Undo => {
                    if let Some(editor) = root.read(cx).editor.clone() {
                        editor.update(cx, |editor, cx| {
                            editor.update_state(cx, |state| state.undo());
                        });
                    }
                }
                Event::ResizeTo(x) => {
                    if let Some(editor) = root.read(cx).editor.clone() {
                        editor.update(cx, |editor, cx| editor.resize_to(px(x), cx));
                    }
                }
                Event::Scroll => {
                    if let Some(editor) = root.read(cx).editor.clone() {
                        editor.update(cx, |editor, cx| {
                            let target = editor.scroll_top() + 2.5 * editor.viewport_height();
                            editor.set_scroll_top(target);
                            cx.notify();
                        });
                    }
                }
            }
            window.on_next_frame(move |_, cx| {
                let frame = Instant::now();
                cx.spawn(async move |_| {
                    tx.send((frame, Instant::now()));
                })
                .detach();
            });
        })
        .ok()?;
    let (frame, presented) = rx.recv(cx).await?;
    let drew = rendered
        .borrow()
        .is_some_and(|at| at >= frame && at <= presented);
    if !drew {
        bench.borrow_mut().timings.missed += 1;
    }
    if matches!(event, Event::Open) {
        bench.borrow_mut().timings.open = Some(presented - input);
    }
    Some(presented - frame)
}

/// A one-shot slot the frame task fills after `present`.
fn slot() -> (Slot, Slot) {
    let slot = Slot(Rc::new(RefCell::new(None)));
    (Slot(slot.0.clone()), slot)
}

struct Slot(Rc<RefCell<Option<(Instant, Instant)>>>);

impl Slot {
    fn send(self, value: (Instant, Instant)) {
        *self.0.borrow_mut() = Some(value);
    }

    /// Polls the slot on a short timer: the timestamps are taken by the
    /// frame task, so how soon this notices does not matter.
    async fn recv(self, cx: &AsyncApp) -> Option<(Instant, Instant)> {
        for _ in 0..10_000 {
            if let Some(value) = self.0.borrow_mut().take() {
                return Some(value);
            }
            cx.background_executor()
                .timer(Duration::from_micros(500))
                .await;
        }
        None
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn summary(name: &str, values: &[Duration]) -> String {
    let mut ms: Vec<f64> = values.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    format!(
        "{name:<32} p50={:7.3} p95={:7.3} p99={:7.3} max={:7.3} ms (n={})",
        percentile(&ms, 50.0),
        percentile(&ms, 95.0),
        percentile(&ms, 99.0),
        ms.last().copied().unwrap_or(f64::NAN),
        ms.len()
    )
}

async fn run(bench: Rc<RefCell<Bench>>, cx: &mut AsyncApp) -> anyhow::Result<String> {
    // Let the empty window present its first frame.
    cx.background_executor()
        .timer(Duration::from_millis(500))
        .await;
    measure(&bench, Event::Open, cx)
        .await
        .context("no frame after the open")?;

    // Caret at the end of a long paragraph in the middle of the note.
    let root = bench.borrow().root.clone();
    let window = bench.borrow().window;
    window.update(cx, |_, _, cx| {
        if let Some(editor) = root.read(cx).editor.clone() {
            editor.update(cx, |editor, cx| {
                let text = editor.state.text();
                let middle = text.line_count() / 2;
                let line = (middle..text.line_count())
                    .map(|n| text.line(n))
                    .find(|line| {
                        line.range().len() > 200 && !text.slice(line.range()).starts_with('#')
                    })
                    .map_or(text.len(), |line| line.to);
                editor.select(cerata_markdown::Selection::cursor(line), cx);
            });
        }
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;

    let typed: Vec<char> = "the quick brown fox jumps over a lazy dog while typing "
        .chars()
        .cycle()
        .take(CHARS)
        .collect();
    for ch in &typed {
        let delay = bench.borrow_mut().next_delay();
        cx.background_executor().timer(delay).await;
        let work = measure(&bench, Event::Key(*ch), cx)
            .await
            .context("no frame after a keystroke")?;
        bench.borrow_mut().timings.typing.push(work);
    }
    for _ in 0..SCROLL_JUMPS {
        let delay = bench.borrow_mut().next_delay();
        cx.background_executor().timer(delay).await;
        let work = measure(&bench, Event::Scroll, cx)
            .await
            .context("no frame after a scroll jump")?;
        bench.borrow_mut().timings.scrolling.push(work);
    }

    // US-066: a gesture on the figure nearest the middle of the note.
    let missed_before_resize = bench.borrow().timings.missed;
    let started = window.update(cx, |_, window, cx| {
        let editor = root.read(cx).editor.clone()?;
        editor.update(cx, |editor, cx| {
            let text = editor.state.text();
            let middle = text.line_count() / 2;
            let ix = (middle..text.line_count())
                .find(|n| text.slice(text.line(*n).range()).starts_with("![Figure"))?;
            let range = text.line(ix).range();
            let top = editor.heights.prefix(ix);
            editor.set_scroll_top(top - 100.0);
            let line = editor.layout_line(ix, window, cx);
            let block = line.block?;
            let grip = editor::resize::Grip {
                range,
                url: String::new(),
                ix,
                width: block.width,
                height: block.height,
            };
            let press = gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: gpui::point(px(0.0), px(0.0)),
                modifiers: gpui::Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            };
            editor.start_resize(&grip, editor::resize::Side::Right, &press, window, cx);
            Some((editor.state.revision(), block.width))
        })
    })?;
    let (revision, start_width) = started.context("no figure to resize")?;
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    for run in 0..RESIZE_RUNS {
        for step in 0..RESIZE_STEPS {
            // Narrower then back (a move is half the width change, the
            // image staying centered); every step changes the width.
            let half = RESIZE_STEPS as f32 / 2.0;
            let s = (step + 1) as f32;
            let triangle = if s <= half {
                s / half
            } else {
                (RESIZE_STEPS as f32 - s) / half
            };
            let x = -(3.0 * run as f32 + (start_width / 2.0 - 40.0) * triangle);
            let delay = bench.borrow_mut().next_delay();
            cx.background_executor().timer(delay).await;
            let work = measure(&bench, Event::ResizeTo(x), cx)
                .await
                .context("no frame after a width change")?;
            bench.borrow_mut().timings.resize.push(work);
        }
    }
    let untouched = window.update(cx, |_, _, cx| {
        root.read(cx).editor.clone().is_some_and(|editor| {
            editor.update(cx, |editor, cx| {
                let same = editor.state.revision() == revision;
                editor.cancel_resize(cx);
                editor.release_resize(cx);
                same && editor.state.revision() == revision
            })
        })
    })?;
    let missed_resize = bench.borrow().timings.missed - missed_before_resize;

    let typed: String = typed.into_iter().collect();
    let found = window.update(cx, |_, _, cx| {
        root.read(cx)
            .editor
            .as_ref()
            .is_some_and(|editor| editor.read(cx).state.doc().contains(&typed))
    })?;

    // US-048: the caret at the end of a cell in the middle of the table.
    let handled = bench.borrow().timings.handler.len();
    let missed_before_table = bench.borrow().timings.missed;
    window.update(cx, |_, _, cx| {
        if let Some(editor) = root.read(cx).editor.clone() {
            editor.update(cx, |editor, cx| {
                editor.set_text(&table_note(TABLE_ROWS), cx);
                let doc = editor.state.doc();
                let row = format!("| row {} cell 0 |", TABLE_ROWS / 2);
                let at = doc.find(&row).map_or(0, |i| i + row.len() - 2);
                editor.select(cerata_markdown::Selection::cursor(at), cx);
            });
        }
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    let cell_typed: Vec<char> = "in a cell ".chars().cycle().take(CHARS).collect();
    for ch in &cell_typed {
        let delay = bench.borrow_mut().next_delay();
        cx.background_executor().timer(delay).await;
        let work = measure(&bench, Event::Key(*ch), cx)
            .await
            .context("no frame after a keystroke in a cell")?;
        bench.borrow_mut().timings.table.push(work);
    }
    let cell_typed: String = cell_typed.into_iter().collect();
    let missed_table = bench.borrow().timings.missed;
    let cell_handled = bench.borrow().timings.handler.len();
    let (in_cell, columns) = window.update(cx, |_, _, cx| {
        root.read(cx).editor.as_ref().map_or((false, 0), |editor| {
            let state = &editor.read(cx).state;
            let head = state.selection().main().head;
            let line = state.text().slice(state.text().line_at(head).range());
            let row = format!("| row {} cell 0{cell_typed} |", TABLE_ROWS / 2);
            let line = if line.starts_with(&row) { line } else { "" };
            let drawn = cerata_markdown::cells::cell_at(state, head).is_some();
            (
                drawn,
                cerata_markdown::table::pipe_count(line).saturating_sub(1),
            )
        })
    })?;

    // US-057: "Insert column right" from that cell, each one undone off
    // the clock, so every insertion meets the same 200 x 8 table.
    for _ in 0..COLUMN_INSERTS {
        let delay = bench.borrow_mut().next_delay();
        cx.background_executor().timer(delay).await;
        let work = measure(&bench, Event::Command("table.columnRight"), cx)
            .await
            .context("no frame after a column insertion")?;
        bench.borrow_mut().timings.column.push(work);
        measure(&bench, Event::Undo, cx)
            .await
            .context("no frame after an undo")?;
    }
    let columns_after = window.update(cx, |_, _, cx| {
        root.read(cx).editor.as_ref().map_or(0, |editor| {
            let state = &editor.read(cx).state;
            cerata_markdown::cells::grid_at(state, state.selection().main().head)
                .map_or(0, |g| g.columns())
        })
    })?;
    let missed_columns = bench.borrow().timings.missed;

    // US-057: 200 characters in the last cell of a 1 000-row table.
    window.update(cx, |_, _, cx| {
        if let Some(editor) = root.read(cx).editor.clone() {
            editor.update(cx, |editor, cx| {
                editor.set_text(&table_note(BIG_TABLE_ROWS), cx);
                let doc = editor.state.doc();
                let end = doc.find("\n\nAfter the table.").unwrap_or(doc.len());
                editor.select(cerata_markdown::Selection::cursor(end - 2), cx);
            });
        }
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(500))
        .await;
    for ch in &cell_typed.chars().collect::<Vec<char>>() {
        let delay = bench.borrow_mut().next_delay();
        cx.background_executor().timer(delay).await;
        let work = measure(&bench, Event::Key(*ch), cx)
            .await
            .context("no frame after a keystroke in the big table")?;
        bench.borrow_mut().timings.big.push(work);
    }
    let big_in_cell = window.update(cx, |_, _, cx| {
        root.read(cx).editor.as_ref().is_some_and(|editor| {
            let state = &editor.read(cx).state;
            let head = state.selection().main().head;
            state.doc()[..head].ends_with(&cell_typed)
                && cerata_markdown::cells::cell_at(state, head).is_some()
        })
    })?;

    let b = bench.borrow();
    let t = &b.timings;
    let mut work: Vec<f64> = t.typing.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    work.sort_by(f64::total_cmp);
    let mut scroll: Vec<f64> = t
        .scrolling
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect();
    scroll.sort_by(f64::total_cmp);
    let open = t.open.map_or(f64::NAN, |d| d.as_secs_f64() * 1000.0);
    let mut table: Vec<f64> = t.table.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    table.sort_by(f64::total_cmp);
    let p99 = |values: &[Duration]| {
        let mut ms: Vec<f64> = values.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        ms.sort_by(f64::total_cmp);
        percentile(&ms, 99.0)
    };
    let verdict = |p: f64| if p < 16.0 { "<" } else { ">=" };
    Ok(format!(
        "note: {} bytes\n\
         open -> first frame presented: {open:.1} ms\n\
         {}\n{}\n{}\n\
         frames that missed their event: {}\n\
         typed text in the buffer: {found}\n\
         table {TABLE_ROWS} x {TABLE_COLUMNS}:\n{}\n{}\n\
         caret in a drawn cell: {in_cell}, cells in the row: {columns}\n\
         frames that missed their event, table typing: {}, column insertions: {}, big table: {}\n\
         {}\n\
         columns after the insertions were undone: {columns_after}\n\
         table {BIG_TABLE_ROWS} x {TABLE_COLUMNS}:\n{}\n{}\n\
         caret after the typed text in a drawn cell: {big_in_cell}\n\
         image resize, {RESIZE_RUNS} x {RESIZE_STEPS} widths:\n{}\n\
         buffer untouched by the gesture: {untouched}, frames that missed their width: {missed_resize}\n\
         verdict: image resize p99 {:.3} ms {} 16 ms\n\
         verdict: column insertion p99 {:.3} ms {} 16 ms; big table typing p99 {:.3} ms {} 16 ms\n\
         verdict: typing p99 {:.3} ms {} 16 ms; scroll p99 {:.3} ms {} 16.7 ms; open {open:.1} ms {} 100 ms; table typing p99 {:.3} ms {} 16 ms",
        b.note.len(),
        summary("keystroke: input handler", &t.handler[..handled]),
        summary("keystroke: frame work", &t.typing),
        summary("scroll jump: frame work (cold)", &t.scrolling),
        missed_before_table - missed_resize,
        summary(
            "cell keystroke: input handler",
            &t.handler[handled..cell_handled]
        ),
        summary("cell keystroke: frame work", &t.table),
        missed_table - missed_before_table,
        missed_columns - missed_table,
        t.missed - missed_columns,
        summary("insert column: frame work", &t.column),
        summary(
            "big table keystroke: input handler",
            &t.handler[cell_handled..]
        ),
        summary("big table keystroke: frame work", &t.big),
        summary("image resize: frame work", &t.resize),
        p99(&t.resize),
        verdict(p99(&t.resize)),
        p99(&t.column),
        verdict(p99(&t.column)),
        p99(&t.big),
        verdict(p99(&t.big)),
        percentile(&work, 99.0),
        if percentile(&work, 99.0) < 16.0 {
            "<"
        } else {
            ">="
        },
        percentile(&scroll, 99.0),
        if percentile(&scroll, 99.0) < 16.7 {
            "<"
        } else {
            ">="
        },
        if open < 100.0 { "<" } else { ">=" },
        percentile(&table, 99.0),
        if percentile(&table, 99.0) < 16.0 {
            "<"
        } else {
            ">="
        },
    ))
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let dir = std::env::temp_dir().join("cerata-editor-bench");
    std::fs::create_dir_all(&dir)?;
    let path = fixture::write(&dir)?;
    let note = std::fs::read_to_string(&path)?;
    application().run(move |cx: &mut App| {
        let rendered = Rc::new(RefCell::new(None));
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        let opened = cx.open_window(options, |_, cx| {
            cx.new(|_| Root {
                editor: None,
                rendered: rendered.clone(),
            })
        });
        let Ok(handle) = opened else {
            eprintln!("failed to open the window");
            cx.quit();
            return;
        };
        let Ok(root) = handle.entity(cx) else {
            cx.quit();
            return;
        };
        let bench = Rc::new(RefCell::new(Bench {
            window: handle.into(),
            root,
            note: note.clone(),
            path: path.clone(),
            rendered,
            timings: Timings::default(),
            rng: 0x9e37_79b9_7f4a_7c15,
        }));
        cx.spawn(async move |cx| {
            match run(bench, cx).await {
                Ok(report) => println!("{report}"),
                Err(error) => eprintln!("bench failed: {error:#}"),
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}
