//! The virtualized editor element: lays out only the lines around the
//! viewport, each at its own font size, and paints selection, text, block
//! images, IME marked text and the caret.

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, CursorStyle, ElementId,
    ElementInputHandler, Entity, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId,
    LayoutId, Pixels, Style, TextAlign, Window, div, fill, img, point, prelude::*, px, relative,
    rgb, rgba, size,
};

use crate::editor::{Editor, LAYOUT_MARGIN, LaidLine};

pub struct EditorElement {
    editor: Entity<Editor>,
}

impl EditorElement {
    pub fn new(editor: Entity<Editor>) -> Self {
        Self { editor }
    }
}

pub struct PrepaintState {
    lines: Vec<(LaidLine, Pixels)>,
    blocks: Vec<AnyElement>,
    selections: Vec<Bounds<Pixels>>,
    /// Selected image widgets, painted over the image.
    block_selections: Vec<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
    hitbox: Hitbox,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The selection overlay of a block widget fully inside the selection.
pub fn block_selection_rect(
    line: &LaidLine,
    top: Pixels,
    column_left: Pixels,
    selection: &std::ops::Range<usize>,
) -> Option<Bounds<Pixels>> {
    let block = line.block.as_ref()?;
    (!selection.is_empty() && selection.start <= line.start && selection.end >= line.end()).then(
        || {
            Bounds::new(
                point(column_left, top + px(block.top)),
                size(px(block.width), px(block.height)),
            )
        },
    )
}

/// Selection rectangles for the text of one laid-out line whose top is at
/// `top`.
pub fn selection_rects(
    line: &LaidLine,
    top: Pixels,
    column_left: Pixels,
    selection: &std::ops::Range<usize>,
) -> Vec<Bounds<Pixels>> {
    let mut rects = Vec::new();
    if selection.is_empty() || selection.end < line.start || selection.start > line.end() {
        return rects;
    }
    let Some(wrapped) = &line.wrapped else {
        return rects;
    };
    let start = line
        .display
        .to_display(selection.start.max(line.start) - line.start);
    let end = line
        .display
        .to_display(selection.end.min(line.end()) - line.start);
    let selects_newline = selection.end > line.end();
    let row_height = px(line.metrics.row_height);
    let layout = &wrapped.unwrapped_layout;
    let rows = line.row_starts.len();
    for (row, &row_start) in line.row_starts.iter().enumerate() {
        let row_end = line
            .row_starts
            .get(row + 1)
            .copied()
            .unwrap_or(line.display.text.len());
        let s = start.max(row_start);
        let e = end.min(row_end);
        let last = row + 1 == rows;
        if s >= e && !(last && selects_newline && end >= row_start) {
            continue;
        }
        let origin_x = layout.x_for_index(row_start);
        let x0 = layout.x_for_index(s.min(e)) - origin_x;
        let mut x1 = layout.x_for_index(e.max(s)) - origin_x;
        if last && selects_newline {
            x1 += px(line.metrics.font_size * 0.4);
        }
        let y = top + px(line.text_top) + row_height * row as f32;
        rects.push(Bounds::from_corners(
            point(column_left + x0, y),
            point(column_left + x1, y + row_height),
        ));
    }
    rects
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
        let (lines, selections, block_selections, caret, column_left) = self.editor.update(cx, |editor, _| {
            editor.set_viewport(bounds);
            if editor.autoscroll {
                editor.autoscroll = false;
                editor.scroll_to_caret(window);
            }
            let max = editor.max_scroll();
            if editor.scroll_top() > max {
                editor.set_scroll_top(max);
            }

            // Lines from the anchor down to the viewport bottom plus margin.
            let anchor = editor.scroll_line;
            let budget = editor.scroll_offset + editor.viewport_height() + LAYOUT_MARGIN;
            let mut below = Vec::new();
            let mut ix = anchor;
            let line_count = editor.buffer.line_count();
            while ix < line_count {
                below.push(editor.layout_line(ix, window));
                if editor.heights.prefix(ix + 1) - editor.heights.prefix(anchor) > budget {
                    break;
                }
                ix += 1;
            }
            // Then up to the margin above the anchor; their height
            // corrections do not move the anchor on screen.
            let mut above = Vec::new();
            let mut ix = anchor;
            let mut height_above = 0.0;
            while ix > 0 && height_above < LAYOUT_MARGIN - editor.scroll_offset {
                ix -= 1;
                let line = editor.layout_line(ix, window);
                height_above += line.height as f64;
                above.push(line);
            }
            above.reverse();
            above.extend(below);

            let column_left = editor.viewport.map_or(bounds.left(), |v| v.column_left);
            let selection = editor.selected_range.clone();
            let lines: Vec<(LaidLine, Pixels)> = above
                .into_iter()
                .map(|line| {
                    let top = editor.line_screen_top(line.ix);
                    (line, top)
                })
                .collect();
            let selections = lines
                .iter()
                .flat_map(|(line, top)| selection_rects(line, *top, column_left, &selection))
                .collect();
            let block_selections = lines
                .iter()
                .filter_map(|(line, top)| {
                    block_selection_rect(line, *top, column_left, &selection)
                })
                .collect();
            let caret = if selection.is_empty() {
                editor.caret_bounds(editor.cursor_offset(), window)
            } else {
                None
            };
            let first = lines.first().map_or(0, |(line, _)| line.ix);
            let last = lines.last().map_or(0, |(line, _)| line.ix);
            editor.laid_out = first..last + 1;
            editor.status = format!(
                "{:.2} MB, {} lines, laid out {}..={} ({} lines), caret line {}, height map total {:.0}px",
                editor.buffer.len() as f64 / (1024.0 * 1024.0),
                line_count,
                first,
                last,
                lines.len(),
                editor.buffer.line_for_offset(editor.cursor_offset()),
                editor.heights.total(),
            )
            .into();
            (lines, selections, block_selections, caret, column_left)
        });

        let mut blocks = Vec::new();
        for (line, top) in &lines {
            let Some(block) = &line.block else {
                continue;
            };
            let mut element = div()
                .debug_selector(|| format!("spike-image-{}", line.ix))
                .w(px(block.width))
                .h(px(block.height))
                .bg(rgb(0xeef1f5))
                .child(img(block.path.clone()).size_full())
                .into_any_element();
            let measured = element.layout_as_root(
                size(
                    AvailableSpace::Definite(px(block.width)),
                    AvailableSpace::Definite(px(block.height)),
                ),
                window,
                cx,
            );
            // Criterion 4: the widget's laid-out height is the height the
            // height map reserved for the line.
            debug_assert_eq!(measured.height, px(block.height));
            debug_assert_eq!(line.height, block.top + block.height);
            element.prepaint_at(point(column_left, *top + px(block.top)), window, cx);
            blocks.push(element);
        }

        PrepaintState {
            lines,
            blocks,
            selections,
            block_selections,
            caret,
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
        let (focus_handle, column_left) = {
            let editor = self.editor.read(cx);
            (
                editor.focus_handle.clone(),
                editor.viewport.map_or(bounds.left(), |v| v.column_left),
            )
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selection in prepaint.selections.drain(..) {
                window.paint_quad(fill(selection, rgba(0x3b82f640)));
            }
            for (line, top) in &prepaint.lines {
                if let Some(wrapped) = &line.wrapped
                    && let Err(error) = wrapped.paint(
                        point(column_left, *top + px(line.text_top)),
                        px(line.metrics.row_height),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                {
                    log::error!("failed to paint line {}: {error:#}", line.ix);
                }
            }
            for block in &mut prepaint.blocks {
                block.paint(window, cx);
            }
            for selection in prepaint.block_selections.drain(..) {
                window.paint_quad(fill(selection, rgba(0x3b82f640)));
            }
            if let Some(caret) = prepaint.caret.take() {
                window.paint_quad(fill(caret, rgb(0x0969da)));
            }
        });

        let bench = self.editor.read(cx).bench.clone();
        if let Some(bench) = bench {
            crate::bench::on_paint_end(&bench, cx);
        }
    }
}
