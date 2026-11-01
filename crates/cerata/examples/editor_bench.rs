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

#![allow(clippy::single_range_in_vec_init)]

#[allow(dead_code, unused_imports)]
#[path = "../src/editor/mod.rs"]
mod editor;
#[allow(dead_code)]
#[path = "editor_spike/fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "../src/icons.rs"]
mod icons;

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

#[derive(Default)]
struct Timings {
    open: Option<Duration>,
    handler: Vec<Duration>,
    typing: Vec<Duration>,
    scrolling: Vec<Duration>,
    /// Events whose first frame after them did not render.
    missed: usize,
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

    let typed: String = typed.into_iter().collect();
    let found = window.update(cx, |_, _, cx| {
        root.read(cx)
            .editor
            .as_ref()
            .is_some_and(|editor| editor.read(cx).state.doc().contains(&typed))
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
    Ok(format!(
        "note: {} bytes\n\
         open -> first frame presented: {open:.1} ms\n\
         {}\n{}\n{}\n\
         frames that missed their event: {}\n\
         typed text in the buffer: {found}\n\
         verdict: typing p99 {:.3} ms {} 16 ms; scroll p99 {:.3} ms {} 16.7 ms; open {open:.1} ms {} 100 ms",
        b.note.len(),
        summary("keystroke: input handler", &t.handler),
        summary("keystroke: frame work", &t.typing),
        summary("scroll jump: frame work (cold)", &t.scrolling),
        t.missed,
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
