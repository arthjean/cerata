//! `--bench-typing`: types characters through
//! `EntityInputHandler::replace_text_in_range` (the entry point every
//! platform IME and keyboard path calls) at a random phase relative to the
//! display refresh, one per frame, and records when the frame that shows
//! each one has been presented.
//!
//! Timestamps per keystroke:
//! - `injected`: right before `replace_text_in_range`.
//! - `render`: the editor view's `render`, the start of the frame's work.
//! - `paint_end`: end of the editor element's `paint`.
//! - `presented`: a foreground task spawned at `paint_end`. GPUI's frame
//!   callback draws, then calls `present` (DXGI `Present` on Windows)
//!   before returning to the message loop, so this task runs after the
//!   present call has returned. It does not include compositor latency.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, App, AsyncApp, WeakEntity};

use crate::editor::Editor;

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub to_render: Duration,
    pub to_paint_end: Duration,
    pub to_presented: Duration,
    pub frame_work: Duration,
}

struct Pending {
    injected: Instant,
    render: Option<Instant>,
    paint_end: Option<Instant>,
}

/// Scroll jumps measured after the typing run: each lands on lines never
/// shaped before (cold layout of a whole viewport).
const SCROLL_JUMPS: usize = 60;

pub struct BenchState {
    text: Vec<char>,
    next: usize,
    pending: Option<Pending>,
    pub samples: Vec<Sample>,
    pub scroll_samples: Vec<Sample>,
    scrolls_done: usize,
    rng: u64,
    window: AnyWindowHandle,
    editor: WeakEntity<Editor>,
    pub started: Instant,
    pub first_paint: Option<Duration>,
    pub report_header: String,
}

impl BenchState {
    pub fn new(
        chars: usize,
        window: AnyWindowHandle,
        editor: WeakEntity<Editor>,
        started: Instant,
    ) -> Self {
        let text = "the quick brown fox jumps over a lazy dog while typing "
            .chars()
            .cycle()
            .take(chars)
            .collect();
        Self {
            text,
            next: 0,
            pending: None,
            samples: Vec::new(),
            scroll_samples: Vec::new(),
            scrolls_done: 0,
            rng: 0x9e37_79b9_7f4a_7c15,
            window,
            editor,
            started,
            first_paint: None,
            report_header: String::new(),
        }
    }

    pub fn on_render(&mut self) {
        if let Some(pending) = &mut self.pending
            && pending.render.is_none()
        {
            pending.render = Some(Instant::now());
        }
    }

    /// Delay before the next keystroke: uniform in [0, 16.7 ms) so input
    /// lands at every phase of the refresh cycle, like real typing.
    fn next_delay(&mut self) -> Duration {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        Duration::from_micros(self.rng % 16_667)
    }
}

pub fn on_paint_end(bench: &Rc<RefCell<BenchState>>, cx: &mut App) {
    let now = Instant::now();
    let mut state = bench.borrow_mut();
    if state.first_paint.is_none() {
        state.first_paint = Some(now - state.started);
    }
    let Some(pending) = &mut state.pending else {
        return;
    };
    if pending.render.is_none() || pending.paint_end.is_some() {
        return;
    }
    pending.paint_end = Some(now);
    drop(state);
    let bench = bench.clone();
    cx.spawn(async move |cx| {
        let presented = Instant::now();
        let delay = {
            let mut state = bench.borrow_mut();
            let Some(pending) = state.pending.take() else {
                return;
            };
            let render = pending.render.unwrap_or(pending.injected);
            let paint_end = pending.paint_end.unwrap_or(presented);
            let sample = Sample {
                to_render: render - pending.injected,
                to_paint_end: paint_end - pending.injected,
                to_presented: presented - pending.injected,
                frame_work: presented - render,
            };
            if state.next < state.text.len() || state.samples.len() < state.text.len() {
                state.samples.push(sample);
            } else {
                state.scroll_samples.push(sample);
            }
            if state.scrolls_done >= SCROLL_JUMPS {
                None
            } else {
                Some(state.next_delay())
            }
        };
        match delay {
            Some(delay) => {
                cx.background_executor().timer(delay).await;
                inject(&bench, cx);
            }
            None => finish(&bench, cx),
        }
    })
    .detach();
}

/// Types the next character, or once all are typed, jumps the scroll
/// position 2.5 viewports down.
pub fn inject(bench: &Rc<RefCell<BenchState>>, cx: &mut AsyncApp) {
    let (window, editor, ch) = {
        let mut state = bench.borrow_mut();
        let ch = state.text.get(state.next).copied();
        if ch.is_some() {
            state.next += 1;
        } else {
            state.scrolls_done += 1;
        }
        (state.window, state.editor.clone(), ch)
    };
    let result = window.update(cx, |_, window, cx| {
        if let Some(editor) = editor.upgrade() {
            editor.update(cx, |editor, cx| {
                bench.borrow_mut().pending = Some(Pending {
                    injected: Instant::now(),
                    render: None,
                    paint_end: None,
                });
                match ch {
                    Some(ch) => {
                        let mut buf = [0; 4];
                        gpui::EntityInputHandler::replace_text_in_range(
                            editor,
                            None,
                            ch.encode_utf8(&mut buf),
                            window,
                            cx,
                        );
                    }
                    None => {
                        let target = editor.scroll_top() + 2.5 * editor.viewport_height();
                        editor.set_scroll_top(target);
                        cx.notify();
                    }
                }
            });
        }
    });
    if let Err(error) = result {
        log::error!("bench: window gone: {error:#}");
    }
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn summary(name: &str, values: impl Iterator<Item = Duration>) -> String {
    let mut ms: Vec<f64> = values.map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    format!(
        "{name:<34} p50={:7.3} p95={:7.3} p99={:7.3} max={:7.3} ms (n={})",
        percentile(&ms, 50.0),
        percentile(&ms, 95.0),
        percentile(&ms, 99.0),
        ms.last().copied().unwrap_or(f64::NAN),
        ms.len()
    )
}

fn finish(bench: &Rc<RefCell<BenchState>>, cx: &mut AsyncApp) {
    let state = bench.borrow();
    let samples = &state.samples;
    let mut report = state.report_header.clone();
    if let Some(first_paint) = state.first_paint {
        report.push_str(&format!(
            "first frame painted {:.1} ms after the fixture was read\n",
            first_paint.as_secs_f64() * 1000.0
        ));
    }
    report.push_str(&summary(
        "input -> presented",
        samples.iter().map(|s| s.to_presented),
    ));
    report.push('\n');
    report.push_str(&summary(
        "input -> end of paint",
        samples.iter().map(|s| s.to_paint_end),
    ));
    report.push('\n');
    report.push_str(&summary(
        "wait for frame (input -> render)",
        samples.iter().map(|s| s.to_render),
    ));
    report.push('\n');
    report.push_str(&summary(
        "frame work (render -> presented)",
        samples.iter().map(|s| s.frame_work),
    ));
    report.push('\n');
    report.push_str(&summary(
        "scroll jump: frame work (cold)",
        state.scroll_samples.iter().map(|s| s.frame_work),
    ));
    report.push('\n');
    report.push_str(&summary(
        "scroll jump: input -> presented",
        state.scroll_samples.iter().map(|s| s.to_presented),
    ));
    report.push('\n');
    if let Some(editor) = state.editor.upgrade() {
        let tail = cx.update(|cx| {
            let editor = editor.read(cx);
            let caret = editor.cursor_offset();
            let line = editor.buffer.line_for_offset(caret);
            let range = editor.buffer.line_range(line);
            let start = editor
                .buffer
                .clip_offset(caret.saturating_sub(60).max(range.start));
            editor.buffer.text()[start..caret].to_string()
        });
        report.push_str(&format!("typed text check, end of caret line: {tail:?}\n"));
    }
    let mut sorted: Vec<f64> = samples
        .iter()
        .map(|s| s.to_presented.as_secs_f64() * 1000.0)
        .collect();
    sorted.sort_by(f64::total_cmp);
    let p99 = percentile(&sorted, 99.0);
    report.push_str(&format!(
        "verdict: p99 input -> presented {p99:.3} ms {} 16 ms\n",
        if p99 < 16.0 { "<" } else { ">=" }
    ));
    drop(state);
    println!("{report}");
    cx.update(|cx| cx.quit());
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn nearest_rank_percentiles() {
        let values: Vec<f64> = (1..=200).map(f64::from).collect();
        assert_eq!(percentile(&values, 50.0), 100.0);
        assert_eq!(percentile(&values, 99.0), 198.0);
        assert_eq!(percentile(&values, 100.0), 200.0);
        assert_eq!(percentile(&[3.0], 99.0), 3.0);
    }
}
