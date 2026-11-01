//! The virtualized editor element: lays out only the lines around the
//! viewport, each at its own font size, then paints the chrome, the
//! selection, the search results, the text, the block widgets, the carets
//! and the scrollbars.

use std::ops::Range;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Corners, CursorStyle, DispatchPhase,
    Edges, ElementId, ElementInputHandler, Entity, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, LayoutId, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Style,
    TextAlign, Window, fill, point, prelude::*, px, quad, relative, size,
};

use super::code_scroll::{THUMB_HEIGHT, THUMB_INSET};
use super::layout::{BlockKind, CODE_PADDING, LaidLine, Marker, QUOTE_BAR_WIDTH};
use super::mouse::ThumbHit;
use super::{Editor, LAYOUT_MARGIN, SCROLLBAR_WIDTH, theme};

pub struct EditorElement {
    editor: Entity<Editor>,
}

impl EditorElement {
    pub fn new(editor: Entity<Editor>) -> Self {
        Self { editor }
    }
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What a frame drew, for the interaction tests: each code line's
/// horizontal offset, the selection rects and the carets.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct FrameRecord {
    pub code_scroll: Vec<(usize, f32)>,
    pub selections: Vec<Bounds<Pixels>>,
    pub carets: Vec<Bounds<Pixels>>,
}

struct Painted {
    line: LaidLine,
    top: Pixels,
}

pub struct PrepaintState {
    lines: Vec<Painted>,
    blocks: Vec<AnyElement>,
    selections: Vec<Bounds<Pixels>>,
    matches: Vec<(Bounds<Pixels>, bool)>,
    carets: Vec<Bounds<Pixels>>,
    column_left: Pixels,
    column_width: f32,
    scrollbar: Option<Bounds<Pixels>>,
    marks: Vec<(Bounds<Pixels>, bool)>,
    focused: bool,
    hitbox: Hitbox,
}

/// Column-relative x and row of display offset `display` in `line`.
fn x_in_line(line: &LaidLine, display: usize) -> Option<Point<Pixels>> {
    let wrapped = line.wrapped.as_ref()?;
    let position = wrapped.position_for_index(display, px(line.metrics.row_height))?;
    let scroll = line.code.as_ref().map_or(0.0, |c| c.scroll_x);
    Some(point(px(line.text_left - scroll) + position.x, position.y))
}

/// The caret at buffer offset `pos` of `line`, whose top is at `top`.
pub fn caret_rect(
    line: &LaidLine,
    top: Pixels,
    column_left: Pixels,
    pos: usize,
) -> Option<Bounds<Pixels>> {
    if pos < line.start || pos > line.end() {
        return None;
    }
    let Some(_) = &line.wrapped else {
        let height = line.height.max(1.0);
        return Some(Bounds::new(
            point(column_left, top),
            size(px(2.0), px(height)),
        ));
    };
    let position = x_in_line(line, line.display.to_display(pos - line.start))?;
    Some(Bounds::new(
        point(
            column_left + position.x,
            top + px(line.text_top) + position.y,
        ),
        size(px(2.0), px(line.metrics.row_height)),
    ))
}

/// Selection rectangles over the text of one line: each row from the first
/// to the last selected glyph, never past the text (a selected line break
/// adds a narrow mark).
pub fn range_rects(
    line: &LaidLine,
    top: Pixels,
    column_left: Pixels,
    range: &Range<usize>,
) -> Vec<Bounds<Pixels>> {
    let mut rects = Vec::new();
    if range.is_empty() || range.end < line.start || range.start > line.end() {
        return rects;
    }
    let Some(wrapped) = &line.wrapped else {
        return rects;
    };
    let start = line
        .display
        .to_display(range.start.max(line.start) - line.start);
    let end = line
        .display
        .to_display(range.end.min(line.end()) - line.start);
    let selects_break = range.end > line.end();
    let row_height = px(line.metrics.row_height);
    let layout = &wrapped.unwrapped_layout;
    let rows = line.row_starts.len();
    let scroll = line.code.as_ref().map_or(0.0, |c| c.scroll_x);
    for (row, &row_start) in line.row_starts.iter().enumerate() {
        let row_end = line
            .row_starts
            .get(row + 1)
            .copied()
            .unwrap_or(line.display.text.len());
        let s = start.max(row_start);
        let e = end.min(row_end);
        let last = row + 1 == rows;
        if s >= e && !(last && selects_break && end >= row_start) {
            continue;
        }
        let origin_x = layout.x_for_index(row_start);
        let x0 = layout.x_for_index(s.min(e)) - origin_x;
        let mut x1 = layout.x_for_index(e.max(s)) - origin_x;
        if last && selects_break {
            x1 += px(line.metrics.font_size * 0.3);
        }
        let left = px(line.text_left - scroll);
        let y = top + px(line.text_top) + row_height * row as f32;
        rects.push(Bounds::from_corners(
            point(column_left + left + x0, y),
            point(column_left + left + x1, y + row_height),
        ));
    }
    rects
}

/// The left and right edges of a code line's padded box, where its text
/// is clipped when it scrolls.
fn code_box(line: &LaidLine, column_left: Pixels, column_width: f32) -> Option<(Pixels, Pixels)> {
    line.code.as_ref().map(|_| {
        (
            column_left + px(CODE_PADDING),
            column_left + px(column_width - CODE_PADDING),
        )
    })
}

/// `rects` of a code line cut to its box; a rect wholly outside it is
/// dropped.
fn clip_to_code(
    line: &LaidLine,
    column_left: Pixels,
    column_width: f32,
    rects: Vec<Bounds<Pixels>>,
) -> Vec<Bounds<Pixels>> {
    let Some((left, right)) = code_box(line, column_left, column_width) else {
        return rects;
    };
    rects
        .into_iter()
        .filter_map(|rect| {
            let x0 = rect.left().max(left);
            let x1 = rect.right().min(right);
            (x1 > x0).then(|| Bounds::from_corners(point(x0, rect.top()), point(x1, rect.bottom())))
        })
        .collect()
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let focused = self.editor.read(cx).focus_handle.is_focused(window);
        let (
            lines,
            blocks,
            selections,
            matches,
            carets,
            column_left,
            column_width,
            scrollbar,
            marks,
        ) = self.editor.update(cx, |editor, cx| {
            editor.set_viewport(bounds);
            editor.code_scroll.begin_frame();
            editor.autoscroll(window, cx);
            let max = editor.max_scroll();
            if editor.scroll_top() > max {
                editor.set_scroll_top(max);
            }
            // Normalize the anchor into the line it points in.
            let top = editor.scroll_top();
            editor.set_scroll_top(top);

            let text = editor.state.text();
            let anchor = editor.scroll_line();
            let offset = editor.state.scroll_anchor().offset as f64;
            let height = editor.viewport_height();
            let (last_guess, _) = editor.heights.find(top + height + LAYOUT_MARGIN);
            let (first_guess, _) = editor.heights.find(top - LAYOUT_MARGIN);
            let range =
                text.line(first_guess).from..text.line(last_guess.min(text.line_count() - 1)).to;
            let inputs = editor.frame_inputs(range);
            let mut jobs = Vec::new();

            // From the anchor down to the bottom plus margin, then up to
            // the margin above: corrections above the anchor do not
            // move it on screen.
            let count = editor.state.text().line_count();
            let budget = offset + height + LAYOUT_MARGIN;
            let mut below = Vec::new();
            let mut ix = anchor;
            while ix < count {
                below.push(editor.layout_line_with(ix, Some(&inputs), &mut jobs, window));
                if editor.heights.prefix(ix + 1) - editor.heights.prefix(anchor) > budget {
                    break;
                }
                ix += 1;
            }
            let mut above = Vec::new();
            let mut ix = anchor;
            let mut height_above = 0.0;
            while ix > 0 && height_above < LAYOUT_MARGIN - offset {
                ix -= 1;
                let line = editor.layout_line_with(ix, Some(&inputs), &mut jobs, window);
                height_above += line.height as f64;
                above.push(line);
            }
            above.reverse();
            above.extend(below);
            editor.code_scroll.clamp_measured();
            // Lines were laid out with the offsets of the last frame: a
            // block that got narrower paints at its bounded offset now.
            for line in &mut above {
                if let Some(code) = &mut line.code {
                    let first = editor.state.text().line_at(code.block_start).number;
                    code.scroll_x = editor.code_scroll.offset_of_line(first);
                }
            }
            editor.start_loads(jobs, cx);

            let viewport = editor.viewport.unwrap_or(super::Viewport {
                bounds,
                column_left: bounds.left(),
                column_width: bounds.size.width.as_f32(),
            });
            let column_left = viewport.column_left;
            let code_clip = |line: &LaidLine, rects: Vec<Bounds<Pixels>>| {
                clip_to_code(line, column_left, viewport.column_width, rects)
            };
            let lines: Vec<Painted> = above
                .into_iter()
                .map(|line| {
                    let top = editor.line_screen_top(line.ix);
                    Painted { line, top }
                })
                .collect();
            editor.laid_out =
                lines.first().map_or(0, |p| p.line.ix)..lines.last().map_or(0, |p| p.line.ix + 1);

            let ranges: Vec<Range<usize>> = editor
                .state
                .selection()
                .ranges()
                .iter()
                .map(|r| r.range())
                .collect();
            let mut selections = Vec::new();
            let mut carets = Vec::new();
            for painted in &lines {
                for range in &ranges {
                    selections.extend(code_clip(
                        &painted.line,
                        range_rects(&painted.line, painted.top, column_left, range),
                    ));
                    if let Some(block) = &painted.line.block
                        && !range.is_empty()
                        && range.start <= painted.line.start
                        && range.end >= painted.line.end()
                    {
                        selections.push(Bounds::new(
                            point(column_left, painted.top + px(block.top)),
                            size(px(block.width), px(block.height)),
                        ));
                    }
                }
                for r in editor.state.selection().ranges() {
                    if editor.marked_range.is_none() || r.is_empty() {
                        let on_this_line =
                            painted.line.start <= r.head && r.head <= painted.line.end();
                        let next_starts_here = r.head == painted.line.end()
                            && lines
                                .iter()
                                .any(|p| p.line.start == r.head && p.line.ix != painted.line.ix);
                        if on_this_line
                            && !next_starts_here
                            && let Some(caret) =
                                caret_rect(&painted.line, painted.top, column_left, r.head)
                        {
                            // A caret is kept whole while it overlaps the
                            // box or touches its right edge: at the largest
                            // (rounded) offset, the end of the widest line
                            // sits on that edge.
                            let in_box =
                                code_box(&painted.line, column_left, viewport.column_width)
                                    .is_none_or(|(left, right)| {
                                        caret.right() > left
                                            && caret.left() < right + caret.size.width
                                    });
                            if in_box {
                                carets.push(caret);
                            }
                        }
                    }
                }
            }

            // Search results on screen, and their marks on the bar.
            let mut matches = Vec::new();
            let mut marks = Vec::new();
            if let Some(search) = editor.search.clone() {
                let search = search.read(cx);
                let visible = lines.first().map_or(0, |p| p.line.start)
                    ..lines.last().map_or(0, |p| p.line.end());
                for (i, m) in search.matches_in(visible) {
                    let current = search.current() == Some(i);
                    for painted in &lines {
                        let rects = range_rects(&painted.line, painted.top, column_left, &m);
                        for rect in code_clip(&painted.line, rects) {
                            matches.push((rect, current));
                        }
                    }
                }
                let total = editor.heights.total().max(1.0);
                let track = bounds.size.height.as_f32();
                let mut last_y = -1.0f32;
                let mut hits = Vec::new();
                for (i, m) in search.marks() {
                    let line = editor.state.text().line_at(m.start).number;
                    let y = (editor.heights.prefix(line) / total * track as f64).floor() as f32;
                    if y == last_y {
                        continue;
                    }
                    last_y = y;
                    let rect = Bounds::new(
                        point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.top() + px(y)),
                        size(px(SCROLLBAR_WIDTH), px(2.0)),
                    );
                    marks.push((rect, search.current() == Some(i)));
                    hits.push((rect, m.start));
                }
                editor.mark_hits = hits;
            } else {
                editor.mark_hits.clear();
            }

            // Code block scrollbars, on the last laid-out line of each
            // overflowing block.
            let mut thumbs = Vec::new();
            for painted in &lines {
                let Some(code) = &painted.line.code else {
                    continue;
                };
                let first = editor.state.text().line_at(code.block_start).number;
                let is_last =
                    code.last || lines.last().is_some_and(|p| p.line.ix == painted.line.ix);
                if !is_last {
                    continue;
                }
                if let Some(thumb) = editor.code_scroll.thumb(first) {
                    let y = painted.top + px(painted.line.height - THUMB_HEIGHT - THUMB_INSET);
                    let rect = Bounds::new(
                        point(column_left + px(CODE_PADDING + thumb.left), y),
                        size(px(thumb.width), px(THUMB_HEIGHT)),
                    );
                    thumbs.push(ThumbHit {
                        first_line: first,
                        bounds: rect,
                        per_pixel: thumb.per_pixel,
                    });
                }
            }
            editor.thumbs = thumbs;

            let total = editor.heights.total() + height / 2.0;
            let track = bounds.size.height.as_f32();
            let scrollbar = (total > height).then(|| {
                let thumb = (height / total * track as f64).max(24.0) as f32;
                let max = editor.max_scroll().max(1.0);
                let y = (editor.scroll_top() / max) as f32 * (track - thumb);
                Bounds::new(
                    point(
                        bounds.right() - px(SCROLLBAR_WIDTH - 2.0),
                        bounds.top() + px(y),
                    ),
                    size(px(SCROLLBAR_WIDTH - 4.0), px(thumb)),
                )
            });

            #[cfg(test)]
            {
                editor.frames.push(FrameRecord {
                    code_scroll: lines
                        .iter()
                        .filter_map(|p| p.line.code.as_ref().map(|c| (p.line.ix, c.scroll_x)))
                        .collect(),
                    selections: selections.clone(),
                    carets: carets.clone(),
                });
            }

            let mut blocks = Vec::new();
            for painted in &lines {
                let Some(block) = &painted.line.block else {
                    continue;
                };
                if let BlockKind::Image { url } = &block.kind {
                    let id = format!("editor-image-{}", painted.line.ix);
                    blocks.push((
                        editor.images.element(url, block.width, block.height, id),
                        point(column_left, painted.top + px(block.top)),
                        size(px(block.width), px(block.height)),
                    ));
                }
            }
            (
                lines,
                blocks,
                selections,
                matches,
                carets,
                column_left,
                viewport.column_width,
                scrollbar,
                marks,
            )
        });

        let mut elements = Vec::new();
        for (mut element, origin, block_size) in blocks {
            element.layout_as_root(
                size(
                    AvailableSpace::Definite(block_size.width),
                    AvailableSpace::Definite(block_size.height),
                ),
                window,
                cx,
            );
            element.prepaint_at(origin, window, cx);
            elements.push(element);
        }

        PrepaintState {
            lines,
            blocks: elements,
            selections,
            matches,
            carets,
            column_left,
            column_width,
            scrollbar,
            marks,
            focused,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        if self.editor.read(cx).drag.is_some() {
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    editor.update(cx, |editor, cx| editor.on_mouse_move(event, window, cx));
                }
            });
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    editor.update(cx, |editor, cx| editor.on_mouse_up(event, window, cx));
                }
            });
        }
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        let left = prepaint.column_left;
        let width = px(prepaint.column_width);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for painted in &prepaint.lines {
                paint_chrome(painted, left, width, window);
            }
            for rect in prepaint.selections.drain(..) {
                window.paint_quad(fill(rect, theme::selection()));
            }
            for (rect, current) in prepaint.matches.drain(..) {
                let color = if current {
                    theme::search_current()
                } else {
                    theme::search_match()
                };
                window.paint_quad(fill(rect, color));
            }
            for painted in &prepaint.lines {
                paint_text(painted, left, width, window, cx);
            }
            for block in &mut prepaint.blocks {
                block.paint(window, cx);
            }
            if prepaint.focused {
                for caret in prepaint.carets.drain(..) {
                    window.paint_quad(fill(caret, theme::accent()));
                }
            }
            let thumbs = self.editor.read(cx).thumbs.clone();
            for thumb in thumbs {
                window.paint_quad(fill(thumb.bounds, theme::scrollbar()).corner_radii(px(3.0)));
            }
            if let Some(bar) = prepaint.scrollbar.take() {
                window.paint_quad(fill(bar, theme::scrollbar()).corner_radii(px(3.0)));
            }
            for (mark, current) in prepaint.marks.drain(..) {
                let color = if current {
                    theme::search_current()
                } else {
                    theme::accent()
                };
                window.paint_quad(fill(mark, color));
            }
        });
    }
}

/// Code background, quote bars, rules and list markers of one line.
fn paint_chrome(painted: &Painted, left: Pixels, width: Pixels, window: &mut Window) {
    let line = &painted.line;
    let top = painted.top;
    let height = px(line.height);
    if let Some(code) = &line.code {
        let radius = px(6.0);
        let corners = Corners {
            top_left: if code.first { radius } else { px(0.0) },
            top_right: if code.first { radius } else { px(0.0) },
            bottom_left: if code.last { radius } else { px(0.0) },
            bottom_right: if code.last { radius } else { px(0.0) },
        };
        window.paint_quad(quad(
            Bounds::new(point(left, top), size(width, height)),
            corners,
            theme::code_background(),
            Edges::default(),
            theme::transparent(),
            Default::default(),
        ));
    }
    for x in &line.quote_bars {
        window.paint_quad(fill(
            Bounds::new(point(left + px(*x), top), size(px(QUOTE_BAR_WIDTH), height)),
            theme::quote_bar(),
        ));
    }
    if let Some(block) = &line.block
        && block.kind == BlockKind::Rule
    {
        let y = top + px(block.top + block.height / 2.0);
        window.paint_quad(fill(
            Bounds::new(point(left, y), size(width, px(1.0))),
            theme::rule(),
        ));
    }
}

fn paint_text(painted: &Painted, left: Pixels, width: Pixels, window: &mut Window, cx: &mut App) {
    let line = &painted.line;
    let row_height = px(line.metrics.row_height);
    let text_top = painted.top + px(line.text_top);
    if let Some(prefix) = &line.prefix {
        let x = left + px(prefix.x);
        let center_y = text_top + row_height / 2.0;
        match &prefix.marker {
            Marker::Bullet => {
                let d = px((line.metrics.font_size * 0.35).round());
                let cx_ = x + px(prefix.width) / 2.0;
                window.paint_quad(
                    fill(
                        Bounds::new(point(cx_ - d / 2.0, center_y - d / 2.0), size(d, d)),
                        theme::text(),
                    )
                    .corner_radii(d / 2.0),
                );
            }
            Marker::Task { checked } => {
                let s = px(18.0);
                let bx = x + (px(prefix.width) - s) / 2.0;
                let rect = Bounds::new(point(bx, center_y - s / 2.0), size(s, s));
                let (bg, border) = if *checked {
                    (theme::accent(), theme::accent())
                } else {
                    (theme::transparent(), theme::muted())
                };
                window.paint_quad(quad(rect, px(5.0), bg, px(1.5), border, Default::default()));
                if *checked {
                    let check = crate::icons::Icon::Check;
                    let g = px(14.0);
                    let glyph =
                        Bounds::new(point(bx + (s - g) / 2.0, center_y - g / 2.0), size(g, g));
                    let _ = window.paint_svg(
                        glyph,
                        check.key().into(),
                        Some(check.data()),
                        Default::default(),
                        gpui::white(),
                        cx,
                    );
                }
            }
            Marker::Ordered(shaped) => {
                let _ = shaped.paint(
                    point(x, text_top),
                    row_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
    }
    let Some(wrapped) = &line.wrapped else {
        return;
    };
    let scroll = line.code.as_ref().map_or(0.0, |c| c.scroll_x);
    let origin = point(left + px(line.text_left - scroll), text_top);
    let paint = |window: &mut Window, cx: &mut App| {
        if let Err(error) = wrapped.paint(origin, row_height, TextAlign::Left, None, window, cx) {
            log::error!("failed to paint line {}: {error:#}", line.ix);
        }
    };
    if line.code.is_some() {
        // Code scrolls inside its padded box.
        let clip = Bounds::new(
            point(left + px(CODE_PADDING), painted.top),
            size(width - px(CODE_PADDING * 2.0), px(line.height)),
        );
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            paint(window, cx)
        });
    } else {
        paint(window, cx);
    }
}
