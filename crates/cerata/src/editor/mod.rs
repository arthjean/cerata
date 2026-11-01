//! The editor element: a dedicated GPUI view over a `cerata_markdown`
//! [`EditorState`], with a display map of variable-height lines, a height
//! map in a sum tree, and virtualized layout (`docs/adr/0001-editor-engine.md`,
//! option C, built on the US-002 spike).
//!
//! Every position is computed from the model, never from the last frame:
//! hit-testing, IME bounds and scrolling lay the line out on demand, which
//! GPUI's shaping cache makes free for lines already on screen.

/// A keystroke with the OS's command modifier: Cmd on macOS, Ctrl
/// elsewhere, as a static string for `KeyBinding::new`.
macro_rules! secondary {
    ($keys:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $keys)
        } else {
            concat!("ctrl-", $keys)
        }
    };
}

pub mod actions;
pub mod blocks;
mod code_scroll;
pub mod complete;
pub mod config;
mod display;
mod element;
pub mod frontmatter;
mod height_map;
mod html;
pub mod images;
mod input;
mod layout;
mod links;
mod math;
mod menu;
pub mod mermaid;
mod mouse;
mod rail;
pub mod search;
pub mod text_field;
pub mod theme;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use cerata_markdown::{Change, EditorState, LineStyle, Selection, Transaction, UserEvent};
use cerata_render::HighlightSpan;
use gpui::{
    App, AppContext as _, Bounds, Context, CursorStyle, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, div, px,
};

pub use code_scroll::CodeScroll;
pub use config::EditorConfig;
use element::EditorElement;
use height_map::HeightMap;
use images::{ImageHost, LoadJob};
use layout::{Fonts, FrameInputs, LaidLine, LayoutCx};
pub use links::EditorEvent;
use menu::ContextMenu;
use search::SearchBar;

/// Layout margin above and below the viewport, in pixels.
pub const LAYOUT_MARGIN: f64 = 300.0;
/// Space kept clear above and below a caret or a search result: the fade
/// distance plus 20 px (`editor-scroll-container.tsx:3-11`).
pub const SAFE_MARGIN: f64 = 140.0;
/// Horizontal padding around the text column, gutter included.
pub const SIDE_PADDING: f32 = 40.0;
/// The side inset of a source-only editor.
const SOURCE_PADDING: f32 = 12.0;
/// Space above the first line.
pub const TOP_INSET: f32 = 32.0;
/// Width of the drawn vertical scrollbar.
pub const SCROLLBAR_WIDTH: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub bounds: Bounds<Pixels>,
    /// Window x of the text column.
    pub column_left: Pixels,
    pub column_width: f32,
}

/// What the next frame scrolls to.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Autoscroll {
    /// The main caret, kept inside the safe margins.
    Caret,
    /// The line holding a position, `SAFE_MARGIN` from the top or bottom
    /// edge, whichever is nearer (search results).
    Reveal(usize),
    /// The line holding a position at the top of the safe zone (heading
    /// anchors, `scrollPosToSafeTop`).
    Top(usize),
}

/// The state saved when an IME composition starts: a cancel restores it, a
/// commit restores it and applies the committed text as one edit, so the
/// buffer and the undo history only ever see the result.
struct Composition {
    state: EditorState,
    heights: HeightMap,
    /// The range the composition replaces, in the saved state.
    replaced: Range<usize>,
}

pub struct Editor {
    pub focus_handle: FocusHandle,
    pub state: EditorState,
    /// The note's path: images resolve against its folder, pastes save next
    /// to it.
    pub path: Option<PathBuf>,
    pub config: EditorConfig,
    fonts: Option<Fonts>,
    /// `ch` of the body font at its current size.
    ch: Option<(f32, f32)>,
    pub heights: HeightMap,
    /// Column width the estimated heights were computed for.
    estimated_for: f32,
    pub viewport: Option<Viewport>,
    pub marked_range: Option<Range<usize>>,
    composition: Option<Composition>,
    autoscroll: Option<Autoscroll>,
    /// Pixel x a vertical motion aims for, kept across Up and Down.
    goal_x: Option<f32>,
    pub images: ImageHost,
    pub rich: blocks::RichBlocks,
    /// The open workspace: link resolution and wiki completion.
    pub workspace: Option<std::sync::Arc<cerata_core::workspace::WorkspaceRuntime>>,
    /// An anchor to scroll to once the note has loaded.
    pending_anchor: Option<String>,
    /// The open wiki link completion.
    pub completion: Option<complete::Completion>,
    /// The change being applied was typed (opens the completion).
    typed: bool,
    /// The properties panel drawn in place of the frontmatter.
    pub frontmatter: Entity<frontmatter::FrontmatterPanel>,
    /// Formulas asked of the renderer, and their images.
    math: math::MathHost,
    /// A canvas per ` ```mermaid ` fence, by its index in the document.
    canvases: HashMap<usize, (Entity<mermaid::MermaidCanvas>, Subscription)>,
    /// The diagram open full screen.
    fullscreen: Option<mermaid::Fullscreen>,
    /// Focus this at the next frame (a closed full-screen diagram's canvas).
    refocus_to: Option<FocusHandle>,
    /// The section rail's headings and hover state.
    rail: rail::Rail,
    _frontmatter_events: Subscription,
    /// Take the focus back at the next frame (the panel let it go).
    refocus: bool,
    highlights: HashMap<(String, String), Rc<Vec<HighlightSpan>>>,
    pub code_scroll: CodeScroll,
    drag: Option<mouse::Drag>,
    /// Code block scrollbars drawn by the last frame.
    thumbs: Vec<mouse::ThumbHit>,
    /// Search marks on the scrollbar and the result each one jumps to.
    mark_hits: Vec<(Bounds<Pixels>, usize)>,
    pub search: Option<Entity<SearchBar>>,
    /// The query of the last closed search, shown again on the next open.
    last_query: String,
    pub menu: Option<ContextMenu>,
    pub notice: Option<SharedString>,
    /// Lines laid out by the last frame.
    pub laid_out: Range<usize>,
    /// Every frame drawn, oldest first (tests clear it).
    #[cfg(test)]
    pub frames: Vec<element::FrameRecord>,
    epoch: Instant,
    /// Source text only, filling its width: a diagram's edit panel
    /// (Writer's inner editor has no prosemark decorations nor page inset).
    source_only: bool,
    /// Tab and Shift+Tab move the focus instead of indenting.
    pub(crate) tab_focus: bool,
    /// The focus the view last scrolled into sight.
    revealed_focus: Option<FocusHandle>,
}

impl Editor {
    pub fn new(
        text: impl Into<String>,
        path: Option<PathBuf>,
        config: EditorConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = EditorState::new(text);
        let panel = cx.new(|_| frontmatter::FrontmatterPanel::new());
        let frontmatter_events = cx.subscribe(&panel, |editor: &mut Self, _, event, cx| {
            editor.on_panel(event, cx);
        });
        let note_dir = path.as_ref().and_then(|p| p.parent().map(PathBuf::from));
        let mut editor = Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            state,
            path,
            config,
            fonts: None,
            ch: None,
            heights: HeightMap::new(Vec::new()),
            estimated_for: 0.0,
            viewport: None,
            marked_range: None,
            composition: None,
            autoscroll: None,
            goal_x: None,
            images: ImageHost::new(note_dir),
            rich: blocks::RichBlocks::default(),
            workspace: None,
            pending_anchor: None,
            completion: None,
            typed: false,
            frontmatter: panel,
            math: math::MathHost::default(),
            canvases: HashMap::new(),
            fullscreen: None,
            refocus_to: None,
            rail: rail::Rail::default(),
            _frontmatter_events: frontmatter_events,
            refocus: false,
            highlights: HashMap::new(),
            code_scroll: CodeScroll::default(),
            drag: None,
            thumbs: Vec::new(),
            mark_hits: Vec::new(),
            search: None,
            last_query: String::new(),
            menu: None,
            notice: None,
            laid_out: 0..0,
            #[cfg(test)]
            frames: Vec::new(),
            epoch: Instant::now(),
            source_only: false,
            tab_focus: false,
            revealed_focus: None,
        };
        // Writer keeps the frontmatter out of the editor: a note opens with
        // the caret in its body, so the properties show as a panel.
        if let Some(frontmatter) = cerata_markdown::frontmatter::detect(editor.state.doc()) {
            let _ = editor
                .state
                .select(Selection::cursor(frontmatter.body_start));
        }
        editor.reestimate_heights();
        editor.sync_frontmatter(cx);
        editor
    }

    fn sync_frontmatter(&mut self, cx: &mut Context<Self>) {
        if self.state.is_loading() {
            return;
        }
        let yaml = cerata_markdown::frontmatter::split(self.state.doc())
            .0
            .map(str::to_owned);
        self.frontmatter
            .update(cx, |panel, cx| panel.sync(yaml.as_deref(), cx));
    }

    /// The panel rewrote the YAML, or removed the frontmatter.
    fn on_panel(&mut self, event: &frontmatter::PanelEvent, cx: &mut Context<Self>) {
        use frontmatter::PanelEvent;
        if *event == PanelEvent::FocusEditor {
            self.refocus = true;
            cx.notify();
            return;
        }
        let Some(fm) = cerata_markdown::frontmatter::detect(self.state.doc()) else {
            return;
        };
        let yaml = match event {
            PanelEvent::FocusEditor => return,
            PanelEvent::Write(yaml) => yaml.as_deref(),
        };
        // Writer's `serializeFile`: the block written whole, the body kept.
        let eol = if self.state.doc()[..fm.body_start].contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let change = Change::new(
            0..fm.body_start,
            cerata_markdown::frontmatter::join(yaml, "", eol),
        );
        self.update_state(cx, |state| {
            let _ = state.apply(Transaction::new().change(change).event(UserEvent::Type));
        });
    }

    /// Milliseconds since the editor opened, never 0 (0 means "unset" to
    /// [`EditorState::set_time`]).
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    fn column_width(&self) -> f32 {
        self.viewport
            .map_or(self.config.content_width, |viewport| viewport.column_width)
    }

    /// New settings (size, zoom, fonts, width): fonts resolve again and
    /// every line height is estimated afresh on the next frame.
    pub fn set_config(&mut self, config: EditorConfig, cx: &mut Context<Self>) {
        if self.config == config {
            return;
        }
        self.config = config;
        self.fonts = None;
        self.ch = None;
        self.estimated_for = 0.0;
        cx.notify();
    }

    fn reestimate_heights(&mut self) {
        let heights = (0..self.state.text().line_count())
            .map(|ix| self.estimate_height(ix))
            .collect();
        self.heights.reset(heights);
        self.estimated_for = self.column_width();
    }

    /// Height of a line before it is laid out: half an em per character
    /// (CodeMirror-like estimates). Exact once the line is laid out.
    fn estimate_height(&self, ix: usize) -> f64 {
        let text = self.state.text();
        let line = text.slice(text.line(ix).range());
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        let style = ((1..=6).contains(&hashes) && line[hashes..].starts_with(' '))
            .then_some(LineStyle::Heading(hashes as u8));
        let metrics = self.config.metrics(style);
        let chars = line.chars().count() as f32;
        let rows = (chars * 0.5 * metrics.font_size / self.column_width())
            .ceil()
            .max(1.0);
        (metrics.pad_top + rows * metrics.row_height + metrics.pad_bottom) as f64
    }

    /// Applies the line splices of the edits since the last call to the
    /// height map, then remembers the time for undo grouping.
    fn sync_heights(&mut self) {
        let splices = self.state.take_line_splices();
        let (tree, text) = (self.state.tree(), self.state.text());
        self.code_scroll.remap(&splices, |line| {
            let from = text.line(line.min(text.line_count() - 1)).from;
            cerata_markdown::code::code_block_at(tree, from).is_some_and(|n| n.from() == from)
        });
        for splice in splices {
            let heights = (splice.first..splice.first + splice.new)
                .map(|ix| self.estimate_height(ix))
                .collect();
            self.heights.splice(splice.first, splice.old, heights);
        }
        debug_assert_eq!(self.heights.len(), self.state.text().line_count());
    }

    /// Runs `f` on the state as one user action: the clock is set first,
    /// then heights, scroll and the view follow what changed.
    pub fn update_state<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditorState) -> R,
    ) -> R {
        let now = self.now_ms();
        self.state.set_time(now);
        let selection = self.state.selection().clone();
        let doc_len = self.state.text().len();
        let result = f(&mut self.state);
        self.sync_heights();
        if *self.state.selection() != selection || self.state.text().len() != doc_len {
            self.autoscroll = Some(Autoscroll::Caret);
        }
        self.notice = None;
        self.on_changed(cx);
        result
    }

    /// The view follows a change of the document or selection.
    fn on_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search.clone() {
            search.update(cx, |search, cx| search.document_changed(&self.state, cx));
        }
        if !self.state.is_loading()
            && let Some(anchor) = self.pending_anchor.take()
        {
            self.go_to_anchor(&anchor, cx);
        }
        let typed = std::mem::take(&mut self.typed);
        self.refresh_completion(typed, cx);
        self.sync_frontmatter(cx);
        cx.notify();
    }

    /// Replaces the whole text (a Mermaid edit panel following its fence).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let all = 0..self.state.text().len();
        let text = text.to_owned();
        self.update_state(cx, |state| {
            let _ = state.apply(Transaction::new().change(Change::new(all, text)));
        });
    }

    pub fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.update_state(cx, |state| {
            let _ = state.apply(
                Transaction::new()
                    .select(selection)
                    .event(UserEvent::Select)
                    .scroll(),
            );
        });
    }

    pub fn cursor(&self) -> usize {
        self.state.selection().main().head
    }

    pub fn viewport_height(&self) -> f64 {
        self.viewport.map_or(600.0, |viewport| {
            (viewport.bounds.size.height.as_f32() - TOP_INSET) as f64
        })
    }

    /// Scroll position: the anchor line's top plus the offset into it.
    pub fn scroll_top(&self) -> f64 {
        let anchor = self.state.scroll_anchor();
        let line = self.state.text().line_at(anchor.pos).number;
        self.heights.prefix(line) + anchor.offset as f64
    }

    pub fn max_scroll(&self) -> f64 {
        (self.heights.total() - self.viewport_height() / 2.0).max(0.0)
    }

    /// Scrolls to `y`, stored as (line, offset into it): a later height
    /// change above that line moves nothing on screen.
    pub fn set_scroll_top(&mut self, y: f64) {
        let (line, offset) = self.heights.find(y.clamp(0.0, self.max_scroll()));
        let pos = self.state.text().line(line).from;
        self.state.set_scroll_anchor(cerata_markdown::ScrollAnchor {
            pos,
            offset: offset as f32,
        });
    }

    pub fn scroll_line(&self) -> usize {
        self.state
            .text()
            .line_at(self.state.scroll_anchor().pos)
            .number
    }

    /// Window y of a scroll-space y.
    pub fn screen_y(&self, scroll_y: f64) -> Pixels {
        let top = self.viewport.map_or(px(0.0), |v| v.bounds.top());
        top + px(TOP_INSET + (scroll_y - self.scroll_top()) as f32)
    }

    pub fn line_screen_top(&self, ix: usize) -> Pixels {
        self.screen_y(self.heights.prefix(ix))
    }

    fn set_viewport(&mut self, bounds: Bounds<Pixels>) {
        let (padding, max) = if self.source_only {
            (SOURCE_PADDING, f32::INFINITY)
        } else {
            (SIDE_PADDING, self.config.content_width)
        };
        let available = (bounds.size.width.as_f32() - 2.0 * padding).max(120.0);
        let width = available.min(max).floor();
        self.viewport = Some(Viewport {
            bounds,
            column_left: bounds.left() + px(((bounds.size.width.as_f32() - width) / 2.0).floor()),
            column_width: width,
        });
        if (self.estimated_for - width).abs() > 0.5 {
            // Keep the first visible line where it is across the reflow.
            let anchor = self.state.scroll_anchor();
            self.reestimate_heights();
            self.state.set_scroll_anchor(cerata_markdown::ScrollAnchor {
                pos: self.state.text().line_at(anchor.pos).from,
                offset: 0.0,
            });
        }
    }

    fn fonts(&mut self, window: &Window) -> Fonts {
        if let Some(fonts) = &self.fonts {
            return fonts.clone();
        }
        let installed = window.text_system().all_font_names();
        let fonts = Fonts {
            body: config::resolve_family(&self.config.font_families, &installed, false),
            mono: config::resolve_family(&self.config.mono_families, &installed, true),
        };
        self.fonts = Some(fonts.clone());
        fonts
    }

    fn ch(&mut self, window: &Window) -> f32 {
        let size = self.config.text_size();
        if let Some((for_size, ch)) = self.ch
            && for_size == size
        {
            return ch;
        }
        let fonts = self.fonts(window);
        let run = gpui::TextRun {
            len: 1,
            font: gpui::font(fonts.body),
            color: theme::text(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = window
            .text_system()
            .shape_line("0".into(), px(size), &[run], None)
            .width()
            .as_f32();
        let ch = if width > 0.0 { width } else { size * 0.5 };
        self.ch = Some((size, ch));
        ch
    }

    /// Inputs for the lines touching `range`, code highlighting cached per
    /// block content.
    fn frame_inputs(&mut self, range: Range<usize>) -> FrameInputs {
        let cache = &mut self.highlights;
        if cache.len() > 512 {
            cache.clear();
        }
        let mut highlight = |language: &str, code: &str| {
            cache
                .entry((language.to_owned(), code.to_owned()))
                .or_insert_with(|| {
                    Rc::new(cerata_render::highlight(language, code).unwrap_or_default())
                })
                .clone()
        };
        let mut inputs = FrameInputs::collect(&self.state, range, &mut highlight);
        if self.source_only {
            inputs.decorations.clear();
        }
        inputs
    }

    /// The editor's keys act on the text: it has the focus, or its find
    /// bar does (whose shortcuts drive the text). Any other control drawn
    /// in or beside the text keeps its keys.
    pub(crate) fn keys_reach_text(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            || self
                .search
                .as_ref()
                .is_some_and(|search| search.focus_handle(cx).contains_focused(window, cx))
    }

    /// A control drawn in the text that takes the keyboard focus (a
    /// property field, a diagram or its toolbar) scrolls into sight once:
    /// a focused control is never left off screen (WCAG 2.4.11).
    fn reveal_focused_block(&mut self, window: &Window, cx: &App) {
        let focused = window.focused(cx);
        if focused == self.revealed_focus {
            return;
        }
        self.revealed_focus = focused;
        let block = if self.frontmatter.read(cx).has_focus(window, cx) {
            Some(0)
        } else {
            let fences = mermaid::fences(self.state.tree(), self.state.text());
            self.canvases.iter().find_map(|(index, (canvas, _))| {
                canvas
                    .focus_handle(cx)
                    .contains_focused(window, cx)
                    .then(|| fences.get(*index).map(|r| r.start))
                    .flatten()
            })
        };
        let Some(pos) = block else {
            return;
        };
        let line = self.state.text().line_at(pos).number;
        let (top, bottom) = (self.heights.prefix(line), self.heights.prefix(line + 1));
        let scroll = self.scroll_top();
        if top < scroll || bottom > scroll + self.viewport_height() {
            self.autoscroll = Some(Autoscroll::Top(pos));
        }
    }

    /// Shows the source as plain text across the whole width.
    pub fn source_only(mut self) -> Self {
        self.source_only = true;
        self.state.set_atomic_blocks(false);
        self
    }

    /// Lays out line `ix` from `inputs` (gathered for it when `None`) and
    /// records its exact height. Image loads it needs go to `jobs`.
    fn layout_line_with(
        &mut self,
        ix: usize,
        inputs: Option<&FrameInputs>,
        jobs: &mut Vec<LoadJob>,
        window: &Window,
    ) -> LaidLine {
        let range = self.state.text().line(ix).range();
        let own;
        let inputs = match inputs {
            Some(inputs) if inputs.covers(&range) => inputs,
            _ => {
                own = self.frame_inputs(range);
                &own
            }
        };
        let fonts = self.fonts(window);
        let ch = self.ch(window);
        let column_width = self.column_width();
        let row_height = self.config.row_height();
        let code_scroll = &self.code_scroll;
        let text = self.state.text();
        let scroll_for = |block: usize| code_scroll.offset_of_line(text.line_at(block).number);
        let cx = LayoutCx {
            config: &self.config,
            fonts: &fonts,
            ch,
            column_width,
            marked: self.marked_range.clone(),
            code_scroll: &scroll_for,
        };
        let mut sizer = Sizer {
            images: &mut self.images,
            rich: &mut self.rich,
            math: &mut self.math,
            jobs,
            row_height,
            font_size: self.config.text_size(),
        };
        let line = layout::layout_line(&self.state, ix, inputs, &cx, &mut sizer, window);
        if (self.heights.get(ix) - line.height as f64).abs() > 1e-6 {
            self.heights.set(ix, line.height as f64);
        }
        if let Some(code) = &line.code {
            let first = self.state.text().line_at(code.block_start).number;
            let visible = column_width - layout::CODE_PADDING * 2.0;
            self.code_scroll.measure(first, code.text_width, visible);
        }
        line
    }

    /// What rich blocks are drawn with.
    pub(crate) fn block_style(&mut self, window: &Window) -> blocks::BlockStyle {
        blocks::BlockStyle {
            fonts: self.fonts(window),
            font_size: self.config.text_size(),
        }
    }

    /// Lays out one line on demand (hit-testing, IME, scrolling).
    pub fn layout_line(&mut self, ix: usize, window: &Window, cx: &mut Context<Self>) -> LaidLine {
        let mut jobs = Vec::new();
        let line = self.layout_line_with(ix, None, &mut jobs, window);
        self.start_loads(jobs, cx);
        self.start_math(cx);
        line
    }

    /// Runs image loads off the main thread; a measured image updates its
    /// line's height, which the scroll anchor keeps from moving the view.
    fn start_loads(&mut self, jobs: Vec<LoadJob>, cx: &mut Context<Self>) {
        for job in jobs {
            let task = cx.background_spawn(async move { job.run() });
            cx.spawn(async move |this, cx| {
                let (url, result) = task.await;
                this.update(cx, |editor, cx| {
                    if editor.images.finish(url, result) {
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// Window bounds of the caret at `offset`.
    pub fn caret_bounds(
        &mut self,
        offset: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let viewport = self.viewport?;
        let offset = self.state.text().floor_char_boundary(offset);
        let ix = self.state.text().line_at(offset).number;
        let line = self.layout_line(ix, window, cx);
        let top = self.line_screen_top(ix);
        let x0 = viewport.column_left + px(line.text_left);
        match &line.wrapped {
            None => Some(Bounds::new(
                gpui::point(x0, top),
                gpui::size(px(2.0), px(line.height.max(line.metrics.row_height))),
            )),
            Some(wrapped) => {
                let display = line.display.to_display(offset - line.start);
                let row_height = px(line.metrics.row_height);
                let position = wrapped.position_for_index(display, row_height)?;
                let scroll = line.code.as_ref().map_or(0.0, |c| c.scroll_x);
                Some(Bounds::new(
                    gpui::point(
                        x0 + position.x - px(scroll),
                        top + px(line.text_top) + position.y,
                    ),
                    gpui::size(px(2.0), row_height),
                ))
            }
        }
    }

    /// The wrapped row of `pos`: its buffer range, its index and its count.
    fn row_at(&mut self, pos: usize, window: &Window, cx: &mut Context<Self>) -> (LaidLine, usize) {
        let ix = self.state.text().line_at(pos).number;
        let line = self.layout_line(ix, window, cx);
        let display = line.display.to_display(pos - line.start);
        let row = line
            .row_starts
            .iter()
            .rposition(|start| *start <= display)
            .unwrap_or(0);
        (line, row)
    }

    /// Buffer range of a wrapped row of `line`.
    fn row_range(line: &LaidLine, row: usize) -> Range<usize> {
        let start = line.row_starts.get(row).copied().unwrap_or(0);
        let end = line
            .row_starts
            .get(row + 1)
            .copied()
            .unwrap_or(line.display.text.len());
        let to_buffer = |d: usize| line.start + line.display.to_buffer(d);
        let end = if row + 1 < line.row_starts.len() {
            // The wrap point belongs to the next row.
            line.start + line.display.to_buffer(end).min(line.len)
        } else {
            line.end()
        };
        to_buffer(start).min(end)..end
    }

    /// Buffer offset at pixel `x` (column-relative) on row `row` of `line`.
    fn offset_in_row(line: &LaidLine, row: usize, x: f32) -> usize {
        let Some(wrapped) = &line.wrapped else {
            return line.start;
        };
        let row_height = line.metrics.row_height;
        let scroll = line.code.as_ref().map_or(0.0, |c| c.scroll_x);
        let point = gpui::point(
            px(x - line.text_left + scroll),
            px(row as f32 * row_height + row_height / 2.0),
        );
        let display = match wrapped.closest_index_for_position(point, px(row_height)) {
            Ok(ix) | Err(ix) => ix,
        };
        let pos = line.start + line.display.to_buffer(display);
        // A wrap boundary maps to the next row: stay on this one.
        let range = Self::row_range(line, row);
        pos.clamp(range.start, range.end)
    }

    /// Scrolls the main caret inside the safe margins, or the requested
    /// line to its safe edge.
    fn autoscroll(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(request) = self.autoscroll.take() else {
            return;
        };
        let height = self.viewport_height();
        let margin = SAFE_MARGIN.min((height / 2.0 - self.config.row_height() as f64).max(0.0));
        let scroll_top = self.scroll_top();
        let (top, bottom) = match request {
            Autoscroll::Caret => {
                self.reveal_caret_in_code(window, cx);
                let head = self.cursor();
                let (line, row) = self.row_at(head, window, cx);
                let line_top = self.heights.prefix(line.ix);
                if line.wrapped.is_none() {
                    (line_top, line_top + line.height as f64)
                } else {
                    let row_top =
                        line_top + (line.text_top + row as f32 * line.metrics.row_height) as f64;
                    (row_top, row_top + line.metrics.row_height as f64)
                }
            }
            Autoscroll::Reveal(pos) => {
                let ix = self.state.text().line_at(pos).number;
                self.layout_line(ix, window, cx);
                (self.heights.prefix(ix), self.heights.prefix(ix + 1))
            }
            Autoscroll::Top(pos) => {
                let ix = self.state.text().line_at(pos).number;
                self.layout_line(ix, window, cx);
                self.set_scroll_top(self.heights.prefix(ix) - margin);
                return;
            }
        };
        if top < scroll_top + margin {
            self.set_scroll_top(top - margin);
        } else if bottom > scroll_top + height - margin {
            self.set_scroll_top(bottom - height + margin);
        }
    }

    /// Asks the next frame to bring `pos` into the safe zone.
    pub fn reveal(&mut self, pos: usize, cx: &mut Context<Self>) {
        self.autoscroll = Some(Autoscroll::Reveal(pos));
        cx.notify();
    }

    /// Shows a short message under the text (paste refusals).
    pub fn show_notice(&mut self, notice: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        cx.notify();
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus_handle, cx);
        }
        if let Some(handle) = self.refocus_to.take() {
            window.focus(&handle, cx);
        }
        self.reveal_focused_block(window, cx);
        let mut root = div()
            .id("editor")
            .relative()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text())
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam);
        root = actions::register(root, cx);
        root = mouse::register(root, cx);
        let mut root = root.child(EditorElement::new(cx.entity()));
        if let Some(search) = &self.search {
            root = root.child(
                // Occluding: a click in the bar never reaches the text
                // behind it, which would move the caret and take the focus.
                div()
                    .absolute()
                    .top(px(8.0))
                    .right(px(24.0))
                    .occlude()
                    .child(search.clone()),
            );
        }
        if let Some(notice) = &self.notice {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px(16.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("editor-notice")
                            .role(gpui::Role::Status)
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(theme::overlay())
                            .border_1()
                            .border_color(theme::border())
                            .text_size(px(13.0))
                            .child(notice.clone()),
                    ),
            );
        }
        if let Some(menu) = &self.menu {
            root = root.child(menu.render(window, cx));
        }
        if let Some(list) = self.render_completion(window, cx) {
            root = root.child(list);
        }
        if let Some(rail) = self.render_rail(cx) {
            root = root.child(rail);
        }
        if let Some(overlay) = self.render_fullscreen(window, cx) {
            root = root.child(overlay);
        }
        root
    }
}

/// Sizes the block widgets of the lines being laid out.
struct Sizer<'a> {
    images: &'a mut ImageHost,
    rich: &'a mut blocks::RichBlocks,
    math: &'a mut math::MathHost,
    jobs: &'a mut Vec<LoadJob>,
    row_height: f32,
    font_size: f32,
}

impl layout::BlockSizer for Sizer<'_> {
    fn image_box(&mut self, url: &str, width: f32) -> layout::ImageBox {
        let (image_box, job) = self.images.image_box(url, width, self.row_height);
        self.jobs.extend(job);
        image_box
    }

    fn embed_box(&mut self, target: &str, width: f32) -> layout::ImageBox {
        let (image_box, job) = self.images.embed_box(target, width, self.row_height);
        self.jobs.extend(job);
        image_box
    }

    fn math(&mut self, tex: &str, display: bool) -> math::MathState {
        self.math.state(tex, display, self.font_size)
    }

    fn rich_height(&mut self, widget: &cerata_markdown::Widget, width: f32) -> f32 {
        // A formula's height is its image's, known without measuring.
        if let cerata_markdown::Widget::Math { tex, display } = widget {
            let state = self.math.state(tex, *display, self.font_size);
            return math::block_height(&state, tex, self.font_size, self.row_height);
        }
        self.rich.height(widget, width, self.font_size)
    }
}
