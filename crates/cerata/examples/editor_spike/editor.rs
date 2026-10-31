//! The spike editor entity: buffer, selection, IME state, scroll anchor,
//! per-line layout on demand, hit-testing and `EntityInputHandler`.

use std::{cell::RefCell, collections::HashMap, ops::Range, path::PathBuf, rc::Rc};

use gpui::{
    App, Bounds, Context, CursorStyle, EntityInputHandler, FocusHandle, Focusable, FontWeight,
    Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent,
    SharedString, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, font,
    point, prelude::*, px, rgb, white,
};

use crate::{
    bench::BenchState,
    buffer::Buffer,
    display::DisplayLine,
    element::EditorElement,
    fixture::image_natural_size,
    height_map::HeightMap,
    markup::{
        BlockMode, LineKind, LineMetrics, LineSyntax, NodeKind, decorate_line, line_metrics,
        scan_line,
    },
};

actions!(
    editor_spike,
    [
        Backspace,
        Delete,
        Newline,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Quit
    ]
);

pub const BASE_FONT_SIZE: f32 = 16.0;
pub const MAX_COLUMN_WIDTH: f32 = 720.0;
pub const SIDE_PADDING: f32 = 24.0;
pub const TOP_INSET: f32 = 16.0;
/// Layout margin above and below the viewport, in pixels.
pub const LAYOUT_MARGIN: f64 = 300.0;
const FALLBACK_IMAGE: (f32, f32) = (320.0, 180.0);

pub fn text_color() -> Hsla {
    rgb(0x1f2328).into()
}
fn muted_color() -> Hsla {
    rgb(0x8c959f).into()
}
fn link_color() -> Hsla {
    rgb(0x0969da).into()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub bounds: Bounds<Pixels>,
    pub column_left: Pixels,
    pub wrap_width: Pixels,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLayout {
    pub mode: BlockMode,
    /// Top of the widget, relative to the line top.
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub path: PathBuf,
}

/// One buffer line laid out at its own font size.
pub struct LaidLine {
    pub ix: usize,
    /// Absolute byte offset of the line start.
    pub start: usize,
    pub len: usize,
    /// Read by the tests to rebuild the text runs.
    #[cfg_attr(not(test), allow(dead_code))]
    pub syntax: LineSyntax,
    pub metrics: LineMetrics,
    pub display: DisplayLine,
    /// `None` when a block widget replaces the line.
    pub wrapped: Option<WrappedLine>,
    /// Display offsets where each wrapped row starts.
    pub row_starts: Vec<usize>,
    pub text_top: f32,
    pub block: Option<BlockLayout>,
    pub height: f32,
}

impl LaidLine {
    pub fn rows(&self) -> usize {
        if self.wrapped.is_some() {
            self.row_starts.len()
        } else {
            0
        }
    }

    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

pub struct Editor {
    pub focus_handle: FocusHandle,
    pub buffer: Buffer,
    pub heights: HeightMap,
    pub selected_range: Range<usize>,
    pub selection_reversed: bool,
    pub marked_range: Option<Range<usize>>,
    /// Scroll position as an anchor line plus an offset into it, so height
    /// corrections above the anchor never move what is on screen.
    pub scroll_line: usize,
    pub scroll_offset: f64,
    /// Selection used for hide decisions while a drag is in progress
    /// (Writer's `unfurlFreezeFacet`): text does not reflow under the pointer.
    pub frozen_selection: Option<Range<usize>>,
    pub is_selecting: bool,
    pub viewport: Option<Viewport>,
    pub image_dir: PathBuf,
    pub image_sizes: HashMap<String, (f32, f32)>,
    pub autoscroll: bool,
    pub bench: Option<Rc<RefCell<BenchState>>>,
    pub status: SharedString,
    /// Lines laid out by the last prepaint.
    pub laid_out: Range<usize>,
}

impl Editor {
    pub fn new(text: String, image_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::new(text);
        let mut editor = Self {
            focus_handle: cx.focus_handle(),
            buffer,
            heights: HeightMap::new(Vec::new()),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            scroll_line: 0,
            scroll_offset: 0.0,
            frozen_selection: None,
            is_selecting: false,
            viewport: None,
            image_dir,
            image_sizes: HashMap::new(),
            autoscroll: false,
            bench: None,
            status: SharedString::default(),
            laid_out: 0..0,
        };
        editor.reestimate_heights();
        editor
    }

    fn wrap_width(&self) -> f32 {
        self.viewport
            .map_or(MAX_COLUMN_WIDTH, |viewport| viewport.wrap_width.as_f32())
    }

    pub fn reestimate_heights(&mut self) {
        let heights = (0..self.buffer.line_count())
            .map(|ix| self.estimate_height(ix))
            .collect();
        self.heights.reset(heights);
    }

    /// Height of a line before it is shaped: average glyph width of half an
    /// em, Writer-like CodeMirror estimates. Exact once laid out.
    fn estimate_height(&mut self, ix: usize) -> f64 {
        let text = self.buffer.line_text(ix);
        let syntax = scan_line(text);
        let chars = text.chars().count() as f32;
        let metrics = line_metrics(&syntax.kind, BASE_FONT_SIZE);
        if let LineKind::ImageBlock { url } = &syntax.kind {
            return self.image_display_size(url).1 as f64;
        }
        let rows = (chars * 0.5 * metrics.font_size / self.wrap_width())
            .ceil()
            .max(1.0);
        (metrics.pad_top + rows * metrics.row_height + metrics.pad_bottom) as f64
    }

    /// Natural size read from the file header, scaled to fit the column.
    pub fn image_display_size(&mut self, url: &str) -> (f32, f32) {
        let natural = match self.image_sizes.get(url) {
            Some(size) => *size,
            None => {
                let size = std::fs::read(self.image_dir.join(url))
                    .ok()
                    .and_then(|bytes| image_natural_size(&bytes))
                    .unwrap_or(FALLBACK_IMAGE);
                self.image_sizes.insert(url.to_string(), size);
                size
            }
        };
        // GPUI's layout (taffy) rounds element sizes to whole pixels, so the
        // height map takes the size the widget will actually be laid out at:
        // exact, and never rounded to a multiple of the row height.
        let width = natural.0.min(self.wrap_width()).floor();
        (width, (natural.1 * width / natural.0).round())
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn decoration_selection(&self) -> Range<usize> {
        self.frozen_selection
            .clone()
            .unwrap_or_else(|| self.selected_range.clone())
    }

    pub fn set_viewport(&mut self, bounds: Bounds<Pixels>) {
        let available = (bounds.size.width.as_f32() - 2.0 * SIDE_PADDING).max(50.0);
        let wrap = available.min(MAX_COLUMN_WIDTH);
        let viewport = Viewport {
            bounds,
            column_left: bounds.left() + px((bounds.size.width.as_f32() - wrap) / 2.0),
            wrap_width: px(wrap),
        };
        let width_changed = self
            .viewport
            .is_none_or(|old| old.wrap_width != viewport.wrap_width);
        self.viewport = Some(viewport);
        if width_changed {
            self.reestimate_heights();
        }
    }

    pub fn viewport_height(&self) -> f64 {
        self.viewport.map_or(600.0, |viewport| {
            (viewport.bounds.size.height.as_f32() - TOP_INSET) as f64
        })
    }

    pub fn scroll_top(&self) -> f64 {
        self.heights.prefix(self.scroll_line) + self.scroll_offset
    }

    pub fn max_scroll(&self) -> f64 {
        (self.heights.total() - self.viewport_height() / 2.0).max(0.0)
    }

    pub fn set_scroll_top(&mut self, y: f64) {
        let (line, offset) = self.heights.find(y.clamp(0.0, self.max_scroll()));
        self.scroll_line = line;
        self.scroll_offset = offset;
    }

    /// Window y of a scroll-space y.
    pub fn screen_y(&self, scroll_y: f64) -> Pixels {
        let top = self
            .viewport
            .map_or(px(0.0), |viewport| viewport.bounds.top());
        top + px(TOP_INSET + (scroll_y - self.scroll_top()) as f32)
    }

    pub fn line_screen_top(&self, ix: usize) -> Pixels {
        self.screen_y(self.heights.prefix(ix))
    }

    /// Lays out one line from the model (never from a stale frame) and
    /// records its exact height in the height map.
    pub fn layout_line(&mut self, ix: usize, window: &Window) -> LaidLine {
        let range = self.buffer.line_range(ix);
        let text = &self.buffer.text()[range.clone()];
        let syntax = scan_line(text);
        let metrics = line_metrics(&syntax.kind, BASE_FONT_SIZE);
        let selection = self.decoration_selection();
        let decorations = decorate_line(&syntax, range.start, &[selection]);
        let mut block = None;
        if let (Some(mode), LineKind::ImageBlock { url }) = (decorations.block, &syntax.kind) {
            let url = url.clone();
            let (width, height) = self.image_display_size(&url);
            block = Some(BlockLayout {
                mode,
                top: 0.0,
                width,
                height,
                path: self.image_dir.join(url),
            });
        }

        let text = &self.buffer.text()[range.clone()];
        let replaced = matches!(&block, Some(b) if b.mode == BlockMode::Replace);
        let (display, wrapped, row_starts) = if replaced {
            (
                DisplayLine::new(text, vec![0..text.len()]),
                None,
                Vec::new(),
            )
        } else {
            let display = DisplayLine::new(text, decorations.hidden);
            let marked = self.marked_range.as_ref().and_then(|marked| {
                (marked.start >= range.start && marked.end <= range.end)
                    .then(|| marked.start - range.start..marked.end - range.start)
            });
            let runs = text_runs(&syntax, &display, &metrics, marked);
            let shaped = window.text_system().shape_text(
                SharedString::from(display.text.clone()),
                px(metrics.font_size),
                &runs,
                Some(px(self.wrap_width())),
                None,
            );
            let wrapped = shaped
                .ok()
                .and_then(|mut lines| (!lines.is_empty()).then(|| lines.swap_remove(0)));
            let row_starts = wrapped.as_ref().map_or_else(
                || vec![0],
                |line| {
                    std::iter::once(0)
                        .chain(line.wrap_boundaries().iter().map(|boundary| {
                            line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
                                .index
                        }))
                        .collect()
                },
            );
            (display, wrapped, row_starts)
        };

        let rows = if wrapped.is_some() {
            row_starts.len()
        } else {
            0
        };
        let text_height = metrics.pad_top + rows as f32 * metrics.row_height + metrics.pad_bottom;
        let height = match &mut block {
            Some(block) if block.mode == BlockMode::Replace => block.height,
            Some(block) => {
                block.top = text_height;
                text_height + block.height
            }
            None => text_height,
        };
        if replaced {
            // Criterion 4: a replaced image line is exactly as tall as the
            // image, never rounded to a multiple of the row height.
            debug_assert_eq!(Some(height), block.as_ref().map(|b| b.height));
        }
        if (self.heights.get(ix) - height as f64).abs() > 1e-6 {
            self.heights.set(ix, height as f64);
        }
        LaidLine {
            ix,
            start: range.start,
            len: range.len(),
            syntax,
            metrics,
            display,
            wrapped,
            row_starts,
            text_top: if replaced { 0.0 } else { metrics.pad_top },
            block,
            height,
        }
    }

    /// Window position of the top of the caret at `offset`, plus its height.
    pub fn caret_bounds(&mut self, offset: usize, window: &Window) -> Option<Bounds<Pixels>> {
        self.viewport?;
        let offset = self.buffer.clip_offset(offset);
        let ix = self.buffer.line_for_offset(offset);
        let line = self.layout_line(ix, window);
        let top = self.line_screen_top(ix);
        let x0 = self.viewport?.column_left;
        match &line.wrapped {
            None => {
                let block = line.block.as_ref()?;
                Some(Bounds::new(
                    point(x0, top),
                    gpui::size(px(2.0), px(block.height)),
                ))
            }
            Some(wrapped) => {
                let display = line.display.to_display(offset - line.start);
                let row_height = px(line.metrics.row_height);
                let position = wrapped.position_for_index(display, row_height)?;
                Some(Bounds::new(
                    point(x0 + position.x, top + px(line.text_top) + position.y),
                    gpui::size(px(2.0), row_height),
                ))
            }
        }
    }

    /// Buffer offset under a window point.
    pub fn offset_for_point(&mut self, position: Point<Pixels>, window: &Window) -> usize {
        let Some(viewport) = self.viewport else {
            return 0;
        };
        let scroll_y = self.scroll_top()
            + (position.y - viewport.bounds.top() - px(TOP_INSET)).as_f32() as f64;
        if scroll_y < 0.0 {
            return 0;
        }
        if scroll_y >= self.heights.total() {
            return self.buffer.len();
        }
        let (ix, _) = self.heights.find(scroll_y);
        let line = self.layout_line(ix, window);
        let local_y = (scroll_y - self.heights.prefix(ix)) as f32;
        if let Some(block) = &line.block {
            match block.mode {
                BlockMode::Replace => {
                    return if local_y < block.height / 2.0 {
                        line.start
                    } else {
                        line.end()
                    };
                }
                BlockMode::Below if local_y >= block.top => return line.end(),
                BlockMode::Below => {}
            }
        }
        let Some(wrapped) = &line.wrapped else {
            return line.start;
        };
        let row_height = line.metrics.row_height;
        let text_rows = line.rows() as f32 * row_height;
        let y = (local_y - line.text_top).clamp(0.0, (text_rows - 0.01).max(0.0));
        let x = position.x - viewport.column_left;
        let display = match wrapped.closest_index_for_position(point(x, px(y)), px(row_height)) {
            Ok(ix) | Err(ix) => ix,
        };
        line.start + line.display.to_buffer(display)
    }

    pub fn scroll_to_caret(&mut self, window: &Window) {
        let ix = self.buffer.line_for_offset(self.cursor_offset());
        self.layout_line(ix, window);
        let top = self.heights.prefix(ix);
        let bottom = self.heights.prefix(ix + 1);
        let scroll_top = self.scroll_top();
        let height = self.viewport_height();
        if top < scroll_top {
            self.set_scroll_top(top);
        } else if bottom > scroll_top + height {
            self.set_scroll_top(bottom - height);
        }
    }

    fn edit(&mut self, range: Range<usize>, text: &str) {
        let edit = self.buffer.edit(range, text);
        if edit.old_line_count != edit.new_line_count {
            let heights = (edit.first_line..edit.first_line + edit.new_line_count)
                .map(|ix| self.estimate_height(ix))
                .collect();
            self.heights
                .splice(edit.first_line, edit.old_line_count, heights);
            let old_end = edit.first_line + edit.old_line_count;
            if self.scroll_line >= old_end {
                self.scroll_line = self.scroll_line + edit.new_line_count - edit.old_line_count;
            } else if self.scroll_line > edit.first_line {
                self.scroll_line = edit.first_line;
                self.scroll_offset = 0.0;
            }
            self.scroll_line = self.scroll_line.min(self.buffer.line_count() - 1);
        }
        self.autoscroll = true;
    }

    pub fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        let offset = self.buffer.clip_offset(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.autoscroll = true;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    pub fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        let offset = self.buffer.clip_offset(offset);
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let previous = self.buffer.previous_boundary(self.cursor_offset());
            self.select_to(previous, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.buffer.next_boundary(self.cursor_offset());
            self.select_to(next, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected_range.is_empty() {
            self.buffer.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(target, window, cx);
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected_range.is_empty() {
            self.buffer.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(target, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.buffer.previous_boundary(self.cursor_offset());
        self.select_to(target, window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.buffer.next_boundary(self.cursor_offset());
        self.select_to(target, window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.buffer.len();
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    pub fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        // Freeze hide decisions in the shape the user is looking at.
        self.frozen_selection = Some(self.selected_range.clone());
        self.is_selecting = true;
        let offset = self.offset_for_point(event.position, window);
        if event.modifiers.shift {
            self.select_to(offset, window, cx);
        } else {
            self.move_to(offset, window, cx);
        }
        self.autoscroll = false;
    }

    pub fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting && event.pressed_button == Some(MouseButton::Left) {
            let offset = self.offset_for_point(event.position, window);
            self.select_to(offset, window, cx);
        }
    }

    pub fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.is_selecting = false;
            self.frozen_selection = None;
            window.notify_text_selection_changed(&self.focus_handle);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(BASE_FONT_SIZE * 1.875 * 3.0));
        let y = self.scroll_top() - delta.y.as_f32() as f64;
        self.set_scroll_top(y);
        cx.notify();
    }
}

/// Text runs for the visible part of a line: bold headings and strong
/// text, muted revealed markup, link styling, underlined IME marked text.
pub fn text_runs(
    syntax: &LineSyntax,
    display: &DisplayLine,
    metrics: &LineMetrics,
    marked: Option<Range<usize>>,
) -> Vec<TextRun> {
    let mut boundaries = vec![0, display.buffer_len];
    for node in &syntax.nodes {
        boundaries.extend([node.range.start, node.range.end]);
        boundaries.extend([node.content.start, node.content.end]);
        for mark in &node.marks {
            boundaries.extend([mark.start, mark.end]);
        }
    }
    if let Some(marked) = &marked {
        boundaries.extend([marked.start, marked.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let style_at = |offset: usize| {
        let mut bold = metrics.bold;
        let mut color = text_color();
        let mut underline = None;
        for node in &syntax.nodes {
            let in_mark = node.marks.iter().any(|mark| mark.contains(&offset));
            if in_mark {
                color = muted_color();
            } else if node.content.contains(&offset) {
                match node.kind {
                    NodeKind::Strong => bold = true,
                    NodeKind::Link => {
                        color = link_color();
                        underline = Some(UnderlineStyle {
                            thickness: px(1.0),
                            color: Some(link_color()),
                            wavy: false,
                        });
                    }
                    NodeKind::HeadingMark | NodeKind::Image => {}
                }
            }
        }
        if marked
            .as_ref()
            .is_some_and(|marked| marked.contains(&offset))
        {
            underline = Some(UnderlineStyle {
                thickness: px(2.0),
                color: Some(text_color()),
                wavy: false,
            });
        }
        (bold, color, underline)
    };

    let mut runs: Vec<TextRun> = Vec::new();
    let mut last_style = None;
    for (segment, _) in display.visible_segments() {
        let mut start = segment.start;
        for &boundary in boundaries
            .iter()
            .filter(|&&b| b > segment.start && b < segment.end)
            .chain(std::iter::once(&segment.end))
        {
            let style = style_at(start);
            let len = boundary - start;
            if last_style.as_ref() == Some(&style)
                && let Some(run) = runs.last_mut()
            {
                run.len += len;
            } else {
                let (bold, color, underline) = style;
                let mut run_font = font(".SystemUIFont");
                if bold {
                    run_font.weight = FontWeight::BOLD;
                }
                runs.push(TextRun {
                    len,
                    font: run_font,
                    color,
                    background_color: None,
                    underline,
                    strikethrough: None,
                });
                last_style = Some(style);
            }
            start = boundary;
        }
    }
    runs
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        Some(self.buffer.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.buffer.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.buffer.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        log::debug!(target: "spike_input", "replace_text_in_range({range_utf16:?}, {new_text:?}) at bytes {range:?}");
        self.edit(range.clone(), new_text);
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.buffer.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        log::debug!(
            target: "spike_input",
            "replace_and_mark_text_in_range({range_utf16:?}, {new_text:?}, {new_selected_range_utf16:?}) at bytes {range:?}"
        );
        self.edit(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let start_utf16 = self.buffer.offset_to_utf16(range.start);
        let inserted_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.buffer.range_from_utf16(
                    &(start_utf16 + selected.start.min(inserted_utf16)
                        ..start_utf16 + selected.end.min(inserted_utf16)),
                )
            })
            .unwrap_or_else(|| {
                let end = range.start + new_text.len();
                end..end
            });
        self.selection_reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// Caret or range bounds in window coordinates, computed from the model
    /// so it is correct between an edit and the next frame (macOS asks for
    /// it right after setting marked text). The IME positions its
    /// composition and candidate windows from this.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        let start = self.caret_bounds(range.start, window)?;
        let end = self.caret_bounds(range.end, window)?;
        let right = if end.origin.y == start.origin.y {
            end.origin.x.max(start.origin.x)
        } else {
            start.origin.x
        };
        let bounds = Bounds::from_corners(
            start.origin,
            point(right, start.origin.y + start.size.height),
        );
        log::debug!(target: "spike_input", "bounds_for_range({range_utf16:?}) = {bounds:?}");
        Some(bounds)
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, element_bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_point(point, window);
        Some(self.buffer.offset_to_utf16(offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.buffer.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.len_utf16())
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(bench) = &self.bench {
            bench.borrow_mut().on_render();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(white())
            .text_color(text_color())
            .key_context("SpikeEditor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                div()
                    .h(px(24.0))
                    .px(px(8.0))
                    .flex_none()
                    .bg(rgb(0xf6f8fa))
                    .text_size(px(12.0))
                    .text_color(muted_color())
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .size_full()
                    .child(EditorElement::new(cx.entity())),
            )
    }
}
