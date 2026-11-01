//! Mermaid diagrams (US-037): a closed ` ```mermaid ` fence drawn as a
//! canvas 480 px tall, rendered by Pele on the renderer's thread
//! (`cerata_render::js`). Ported from `mermaid-canvas.ts:24-690`,
//! `mermaid-fullscreen.ts:11-88` and `mermaid-canvas.css:22-53`.
//!
//! Dragging pans; Ctrl or Cmd with the wheel, and a pinch, zoom from 0.25
//! to 4 around the pointer; arrows, `+`, `-` and `0` drive it from the
//! keyboard; a fit keeps 16 px around the diagram. The edit panel (a third
//! of the width) holds the whole fence and writes it back 150 ms after the
//! last keystroke. Diagram links are off (`links: false`).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use cerata_render::js::{DiagramRequest, DiagramTheme, Rendered, Renderer, Svg};
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable,
    Hsla, Image, ImageFormat, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, PinchEvent,
    Pixels, Point, Render, ScrollEvent, SharedString, Subscription, Task, Window, actions, canvas,
    div, img, point, prelude::*, px, relative,
};

use super::config::EditorConfig;
use super::{Editor, theme};
use crate::icons::{Icon, icon};

pub const CANVAS_HEIGHT: f32 = 480.0;
/// Space above and below the canvas in the text column.
pub const CANVAS_MARGIN: f32 = 8.0;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;
const MIN_FIT_ZOOM: f32 = 0.01;
const BUTTON_ZOOM_FACTOR: f32 = 1.2;
const KEY_ZOOM_FACTOR: f32 = 1.15;
const WHEEL_ZOOM_SENSITIVITY: f32 = 0.0015;
const PINCH_ZOOM_SENSITIVITY: f32 = 0.01;
const KEY_PAN_STEP: f32 = 24.0;
const FIT_MARGIN: f32 = 16.0;
const SOURCE_CHANGE_DEBOUNCE: Duration = Duration::from_millis(150);
/// The edit panel's share of the width (`mermaid-canvas.css`).
pub const PANEL_SHARE: f32 = 0.33;

actions!(
    mermaid,
    [
        PanUp, PanDown, PanLeft, PanRight, ZoomIn, ZoomOut, Fit, ToggleEdit, Close
    ]
);

pub const CONTEXT: &str = "MermaidCanvas";

/// Keys of a focused diagram (`mermaid-canvas.ts:323-373`). The edit panel
/// sits outside this context, so its keys stay text.
pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![
        KeyBinding::new("up", PanUp, c),
        KeyBinding::new("down", PanDown, c),
        KeyBinding::new("left", PanLeft, c),
        KeyBinding::new("right", PanRight, c),
        KeyBinding::new("+", ZoomIn, c),
        KeyBinding::new("shift-=", ZoomIn, c),
        KeyBinding::new("=", ZoomIn, c),
        KeyBinding::new("-", ZoomOut, c),
        KeyBinding::new("shift--", ZoomOut, c),
        KeyBinding::new("0", Fit, c),
        KeyBinding::new("enter", ToggleEdit, c),
        KeyBinding::new("escape", Close, c),
    ]
}

/// `mix(a, b, t)`: `t` of `a` over `b`, as CSS `color-mix(in srgb, ...)`.
fn mix(a: Hsla, b: Hsla, t: f32) -> String {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let channel = |x: f32, y: f32| ((x * t + y * (1.0 - t)).clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(a.r, b.r),
        channel(a.g, b.g),
        channel(a.b, b.b)
    )
}

/// A diagram's accessible name: its own `accTitle`, else "Mermaid
/// diagram:" and its first line (`mermaid-decorations.ts:53`,
/// `mermaid-canvas.ts:464-471`).
pub fn accessible_name(source: &str) -> String {
    let own = source.lines().find_map(|l| {
        l.trim()
            .strip_prefix("accTitle")
            .and_then(|r| r.trim_start().strip_prefix(':'))
            .map(str::trim)
            .filter(|t| !t.is_empty())
    });
    match own {
        Some(title) => title.to_owned(),
        None => format!(
            "Mermaid diagram: {}",
            source.lines().next().unwrap_or("").trim()
        ),
    }
}

/// Pele's variables in the active theme (`mermaid-canvas.css:22-53`, at
/// contrast 1).
pub fn diagram_theme() -> DiagramTheme {
    let (bg, fg, accent) = (theme::background(), theme::text(), theme::accent());
    let critical = gpui::rgb(0xe5484d).into();
    let mut vars = vec![
        ("bg", mix(bg, bg, 1.0)),
        ("fg", mix(fg, fg, 1.0)),
        ("muted", mix(fg, bg, 0.55)),
        ("line", mix(fg, bg, 0.45)),
        ("surface", mix(fg, bg, 0.10)),
        ("surface-alt", mix(fg, bg, 0.16)),
        ("border", mix(fg, bg, 0.48)),
        ("accent", mix(accent, accent, 1.0)),
        ("critical", mix(critical, fg, 0.85)),
    ];
    for (i, share) in [1.0, 0.72, 0.5, 0.35, 0.24, 0.16, 0.10, 0.06]
        .iter()
        .enumerate()
    {
        vars.push((
            [
                "series-1", "series-2", "series-3", "series-4", "series-5", "series-6", "series-7",
                "series-8",
            ][i],
            mix(accent, fg, *share),
        ));
    }
    DiagramTheme {
        vars: vars.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

/// What a canvas asks of the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasEvent {
    /// Write this text over the whole fence.
    Rewrite(String),
    /// Open this diagram full screen.
    Expand,
    /// Close the full-screen canvas.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub pan: Point<f32>,
}

pub struct MermaidCanvas {
    focus_handle: FocusHandle,
    /// The fence body, as rendered.
    pub source: String,
    /// The whole fence, as the edit panel shows it.
    pub fence: String,
    request: Option<DiagramRequest>,
    rendered: Option<Rendered>,
    render_task: Option<Task<()>>,
    pub view: View,
    /// The viewport size and diagram the view was last fitted to.
    fitted: Option<(f32, f32, usize)>,
    fit_zoom: f32,
    /// Where the pointer and the pan were when a drag started.
    drag: Option<(Point<Pixels>, Point<f32>)>,
    pub editing: bool,
    panel: Option<(Entity<Editor>, Subscription)>,
    debounce: Option<Task<()>>,
    /// The panel's revision the debounce last started for: a repaint of
    /// the panel is not a keystroke.
    panel_revision: Option<u64>,
    /// The viewport's bounds at the last frame (zoom around the pointer).
    viewport: Rc<Cell<Option<Bounds<Pixels>>>>,
    images: HashMap<(usize, u32), Arc<Image>>,
    /// Full screen: no edit panel, a close button, as tall as given.
    pub fullscreen: bool,
    config: EditorConfig,
}

impl EventEmitter<CanvasEvent> for MermaidCanvas {}

impl Focusable for MermaidCanvas {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MermaidCanvas {
    pub fn new(fullscreen: bool, config: EditorConfig, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            source: String::new(),
            fence: String::new(),
            request: None,
            rendered: None,
            render_task: None,
            view: View {
                zoom: 1.0,
                pan: point(0.0, 0.0),
            },
            fitted: None,
            fit_zoom: ZOOM_MIN,
            drag: None,
            editing: false,
            panel: None,
            debounce: None,
            panel_revision: None,
            viewport: Rc::new(Cell::new(None)),
            images: HashMap::new(),
            fullscreen,
            config,
        }
    }

    /// Follows the document: the fence's body and its whole text. A text
    /// the panel did not write replaces the panel's (undo, other edits).
    pub fn set_source(&mut self, source: &str, fence: &str, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source.to_owned();
            self.fitted = None;
        }
        if self.fence != fence {
            self.fence = fence.to_owned();
            if let Some((panel, _)) = &self.panel {
                let differs = panel.read(cx).state.doc() != fence;
                if differs {
                    self.debounce = None;
                    let fence = fence.to_owned();
                    panel.update(cx, |editor, cx| editor.set_text(&fence, cx));
                }
            }
        }
        self.request_render(cx);
    }

    fn request_render(&mut self, cx: &mut Context<Self>) {
        let request = DiagramRequest {
            source: self.source.clone(),
            theme: diagram_theme(),
            // Writer's `diagramFonts`: the interface face and the code face.
            font: super::config::default_family(false).to_owned(),
            mono: if self.config.mono_families.is_empty() {
                super::config::default_family(true).to_owned()
            } else {
                self.config.mono_families.clone()
            },
        };
        if self.request.as_ref() == Some(&request) {
            return;
        }
        self.request = Some(request.clone());
        if let Some(hit) = Renderer::global().cached_diagram(&request) {
            self.rendered = Some(hit);
            self.render_task = None;
            return;
        }
        let waited = request.clone();
        let task = cx.background_spawn(async move { Renderer::global().diagram_blocking(waited) });
        self.render_task = Some(cx.spawn(async move |this, cx| {
            let rendered = task.await;
            this.update(cx, |canvas, cx| {
                if canvas.request.as_ref() == Some(&request) {
                    canvas.rendered = Some(rendered);
                    canvas.fitted = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn rendered(&self) -> Option<&Rendered> {
        self.rendered.as_ref()
    }

    #[cfg(test)]
    pub fn panel_editor(&self) -> Option<Entity<Editor>> {
        self.panel.as_ref().map(|(p, _)| p.clone())
    }

    fn svg(&self) -> Option<Arc<Svg>> {
        match &self.rendered {
            Some(Ok(svg)) => Some(svg.clone()),
            _ => None,
        }
    }

    fn clamp_zoom(&self, zoom: f32) -> f32 {
        zoom.clamp(ZOOM_MIN.min(self.fit_zoom), ZOOM_MAX)
    }

    /// Fits the diagram in a `width` x `height` viewport, centered.
    fn fit(&mut self, width: f32, height: f32) {
        let Some(svg) = self.svg() else {
            return;
        };
        let Some((fit_zoom, view)) = fit_view(svg.width, svg.height, width, height) else {
            return;
        };
        self.fit_zoom = fit_zoom;
        self.view = view;
        self.fitted = Some((width, height, Arc::as_ptr(&svg) as *const u8 as usize));
    }

    /// Zooms by `factor` keeping the viewport point `at` still.
    fn zoom_at(&mut self, at: Point<f32>, factor: f32, cx: &mut Context<Self>) {
        let next = self.clamp_zoom(self.view.zoom * factor);
        if (next - self.view.zoom).abs() < f32::EPSILON {
            return;
        }
        let stage = point(
            (at.x - self.view.pan.x) / self.view.zoom,
            (at.y - self.view.pan.y) / self.view.zoom,
        );
        self.view = View {
            zoom: next,
            pan: point(at.x - stage.x * next, at.y - stage.y * next),
        };
        cx.notify();
    }

    fn viewport_size(&self) -> (f32, f32) {
        self.viewport.get().map_or((0.0, 0.0), |b| {
            (b.size.width.as_f32(), b.size.height.as_f32())
        })
    }

    fn zoom_center(&mut self, factor: f32, cx: &mut Context<Self>) {
        let (w, h) = self.viewport_size();
        self.zoom_at(point(w / 2.0, h / 2.0), factor, cx);
    }

    fn local(&self, position: Point<Pixels>) -> Point<f32> {
        let origin = self
            .viewport
            .get()
            .map_or(point(px(0.0), px(0.0)), |b| b.origin);
        point(
            (position.x - origin.x).as_f32(),
            (position.y - origin.y).as_f32(),
        )
    }

    fn pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.view.pan = point(self.view.pan.x + dx, self.view.pan.y + dy);
        cx.notify();
    }

    fn refit(&mut self, cx: &mut Context<Self>) {
        let (w, h) = self.viewport_size();
        self.fit(w, h);
        cx.notify();
    }

    fn toggle_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fullscreen {
            return;
        }
        self.editing = !self.editing;
        self.fitted = None;
        if self.editing && self.panel.is_none() {
            let fence = self.fence.clone();
            let mut config = self.config.clone();
            // `mermaid-canvas.css:115-121`: 12 px code at 1.5.
            config.font_size = 12.0;
            config.line_height = 1.5;
            config.font_families = config.mono_families.clone();
            let panel = cx.new(|cx| Editor::new(fence, None, config, cx).source_only());
            let sub = cx.observe(&panel, |canvas: &mut Self, panel, cx| {
                canvas.panel_changed(&panel, cx);
            });
            self.panel = Some((panel, sub));
        }
        match (&self.panel, self.editing) {
            (Some((panel, _)), true) => {
                let handle = panel.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
            }
            _ => window.focus(&self.focus_handle, cx),
        }
        cx.notify();
    }

    /// The panel changed: its text goes to the document 150 ms after the
    /// last keystroke.
    fn panel_changed(&mut self, panel: &Entity<Editor>, cx: &mut Context<Self>) {
        let state = &panel.read(cx).state;
        let revision = state.revision();
        if state.doc() == self.fence || self.panel_revision == Some(revision) {
            return;
        }
        self.panel_revision = Some(revision);
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SOURCE_CHANGE_DEBOUNCE).await;
            this.update(cx, |canvas, cx| canvas.flush(cx)).ok();
        }));
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        self.debounce = None;
        let Some((panel, _)) = &self.panel else {
            return;
        };
        let next = panel.read(cx).state.doc().to_owned();
        if next == self.fence {
            return;
        }
        self.fence = next.clone();
        cx.emit(CanvasEvent::Rewrite(next));
    }

    /// The image of `svg` rasterized at `scale` (1, 2 or 4): a zoomed
    /// diagram stays sharp.
    fn image(&mut self, svg: &Arc<Svg>, scale: u32) -> Arc<Image> {
        if self.images.len() > 16 {
            self.images.clear();
        }
        let key = (Arc::as_ptr(svg) as *const u8 as usize, scale);
        self.images
            .entry(key)
            .or_insert_with(|| {
                let data = if scale == 1 {
                    svg.data.to_string()
                } else {
                    scaled(
                        &svg.data,
                        svg.width * scale as f32,
                        svg.height * scale as f32,
                    )
                };
                Arc::new(Image::from_bytes(ImageFormat::Svg, data.into_bytes()))
            })
            .clone()
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        content: AnyElement,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let button = crate::a11y::button(div().id(id), label)
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .text_color(theme::muted())
            .text_size(px(13.0))
            .cursor_pointer()
            .hover(|s| s.text_color(theme::text()))
            .focus_visible(|s| s.border_color(theme::text()))
            .tooltip(super::math::tooltip(label.into()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |canvas, _, window, cx| {
                cx.stop_propagation();
                on_click(canvas, window, cx);
            }))
            .child(content);
        #[cfg(test)]
        let button = button.debug_selector(move || id.into());
        button.into_any_element()
    }

    fn diagram(
        &mut self,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let svg = self.svg();
        let identity = svg.as_ref().map(|s| Arc::as_ptr(s) as *const u8 as usize);
        if let Some(identity) = identity
            && self.fitted != Some((width, height, identity))
            && width > 0.0
        {
            self.fit(width, height);
        }
        let stage: AnyElement = match (&self.rendered, svg) {
            (_, Some(svg)) => {
                let zoom = self.view.zoom;
                let scale = if zoom > 2.0 {
                    4
                } else if zoom > 1.0 {
                    2
                } else {
                    1
                };
                let picture = img(self.image(&svg, scale))
                    .absolute()
                    .left(px(self.view.pan.x))
                    .top(px(self.view.pan.y))
                    .w(px(svg.width * zoom))
                    .h(px(svg.height * zoom));
                #[cfg(test)]
                let picture = picture.debug_selector(|| "mermaid-diagram".into());
                picture.into_any_element()
            }
            (Some(Err(error)), None) => {
                let body = div()
                    .id("mermaid-error")
                    .absolute()
                    .inset_0()
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .overflow_y_scroll()
                    .text_size(px(13.0))
                    .role(gpui::Role::Status)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight(600.0))
                            .text_color(theme::danger())
                            .child("Can't draw this diagram"),
                    )
                    .child(
                        div()
                            .font_family(self.config.mono_families.clone())
                            .text_color(theme::text())
                            .children(error.lines().map(|l| SharedString::from(l.to_owned()))),
                    )
                    .child(
                        div()
                            .p(px(8.0))
                            .rounded(px(6.0))
                            .bg(theme::code_background())
                            .font_family(self.config.mono_families.clone())
                            .text_color(theme::muted())
                            .children(
                                self.source
                                    .lines()
                                    .map(|l| SharedString::from(l.to_owned())),
                            ),
                    );
                #[cfg(test)]
                let body = body.debug_selector(|| "mermaid-error".into());
                body.into_any_element()
            }
            _ => div().into_any_element(),
        };
        let bounds = self.viewport.clone();
        let _ = window;
        div()
            .id("mermaid-viewport")
            .relative()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .cursor(if self.drag.is_some() {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .child(
                canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(stage)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|canvas, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    canvas.drag = Some((event.position, canvas.view.pan));
                    window.focus(&canvas.focus_handle, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|canvas, event: &MouseMoveEvent, _, cx| {
                let Some((start, pan)) = canvas.drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    canvas.drag = None;
                    cx.notify();
                    return;
                }
                canvas.view.pan = point(
                    pan.x + (event.position.x - start.x).as_f32(),
                    pan.y + (event.position.y - start.y).as_f32(),
                );
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|canvas, _, _, cx| {
                    canvas.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll(cx.listener(|canvas, event: &ScrollEvent, _, cx| {
                // Without Ctrl or Cmd the note scrolls past the diagram.
                if !(event.modifiers.control || event.modifiers.platform) {
                    cx.propagate();
                    return;
                }
                cx.stop_propagation();
                let dy = event.delta.pixel_delta(px(16.0)).y.as_f32();
                let sensitivity = if event.modifiers.control && !event.modifiers.platform {
                    PINCH_ZOOM_SENSITIVITY
                } else {
                    WHEEL_ZOOM_SENSITIVITY
                };
                let at = canvas.local(event.position);
                canvas.zoom_at(at, (dy * sensitivity).exp(), cx);
            }))
            .on_pinch(cx.listener(|canvas, event: &PinchEvent, _, cx| {
                cx.stop_propagation();
                let at = canvas.local(event.position);
                canvas.zoom_at(at, 1.0 + event.delta, cx);
            }))
            .into_any_element()
    }
}

/// The fit of a `w` x `h` diagram in a `width` x `height` viewport
/// (`fitToViewport`): the zoom filling one axis with 16 px around it, and
/// the view at that zoom (capped at 4), centered.
fn fit_view(w: f32, h: f32, width: f32, height: f32) -> Option<(f32, View)> {
    if w <= 0.0 || h <= 0.0 || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let fit_zoom = ((width - FIT_MARGIN * 2.0) / w)
        .min((height - FIT_MARGIN * 2.0) / h)
        .max(MIN_FIT_ZOOM);
    let zoom = fit_zoom.clamp(ZOOM_MIN.min(fit_zoom), ZOOM_MAX);
    let view = View {
        zoom,
        pan: point((width - w * zoom) / 2.0, (height - h * zoom) / 2.0),
    };
    Some((fit_zoom, view))
}

/// `svg` with its root sized `width` x `height` (its `viewBox` keeps the
/// drawing).
fn scaled(svg: &str, width: f32, height: f32) -> String {
    let Some(head_end) = svg.find('>') else {
        return svg.to_owned();
    };
    let (head, rest) = svg.split_at(head_end);
    let set = |head: &str, name: &str, value: f32| -> String {
        let needle = format!(" {name}=\"");
        match head.find(&needle) {
            Some(i) => {
                let start = i + needle.len();
                let end = head[start..].find('"').map_or(head.len(), |e| start + e);
                format!("{}{value}{}", &head[..start], &head[end..])
            }
            None => head.to_owned(),
        }
    };
    let head = set(head, "width", width);
    let head = set(&head, "height", height);
    format!("{head}{rest}")
}

impl Render for MermaidCanvas {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.viewport.get();
        let (vw, vh) = viewport.map_or((0.0, 0.0), |b| {
            (b.size.width.as_f32(), b.size.height.as_f32())
        });
        let diagram = self.diagram(vw, vh, window, cx);
        let fullscreen = self.fullscreen;
        let mut top = div()
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .flex()
            .gap(px(6.0));
        if !fullscreen {
            let label = if self.editing { "Preview" } else { "Edit code" };
            top = top.child(self.button(
                "mermaid-edit",
                label,
                icon(Icon::Pencil, 14.0, theme::muted()).into_any_element(),
                cx,
                |canvas, window, cx| canvas.toggle_edit(window, cx),
            ));
        } else {
            top = top.child(self.button(
                "mermaid-close",
                "Close full screen",
                icon(Icon::Close, 14.0, theme::muted()).into_any_element(),
                cx,
                |_, _, cx| cx.emit(CanvasEvent::Close),
            ));
        }
        let mut zoom = div()
            .absolute()
            .bottom(px(8.0))
            .right(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0));
        if !fullscreen {
            zoom = zoom.child(self.button(
                "mermaid-expand",
                "Open full screen",
                icon(Icon::Maximize, 14.0, theme::muted()).into_any_element(),
                cx,
                |_, _, cx| cx.emit(CanvasEvent::Expand),
            ));
        }
        zoom = zoom
            .child(self.button(
                "mermaid-fit",
                "Fit the diagram",
                div().text_size(px(11.0)).child("Fit").into_any_element(),
                cx,
                |canvas, _, cx| canvas.refit(cx),
            ))
            .child(self.button(
                "mermaid-zoom-in",
                "Zoom in",
                icon(Icon::Add, 14.0, theme::muted()).into_any_element(),
                cx,
                |canvas, _, cx| canvas.zoom_center(BUTTON_ZOOM_FACTOR, cx),
            ))
            .child(self.button(
                "mermaid-zoom-out",
                "Zoom out",
                div().child("\u{2212}").into_any_element(),
                cx,
                |canvas, _, cx| canvas.zoom_center(1.0 / BUTTON_ZOOM_FACTOR, cx),
            ));

        // The diagram and its controls take the keys; the edit panel is
        // outside, so typing there stays text.
        let stage = div()
            .id("mermaid-stage")
            .relative()
            .flex_1()
            .h_full()
            .flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|c, _: &PanUp, _, cx| c.pan_by(0.0, KEY_PAN_STEP, cx)))
            .on_action(cx.listener(|c, _: &PanDown, _, cx| c.pan_by(0.0, -KEY_PAN_STEP, cx)))
            .on_action(cx.listener(|c, _: &PanLeft, _, cx| c.pan_by(KEY_PAN_STEP, 0.0, cx)))
            .on_action(cx.listener(|c, _: &PanRight, _, cx| c.pan_by(-KEY_PAN_STEP, 0.0, cx)))
            .on_action(cx.listener(|c, _: &ZoomIn, _, cx| c.zoom_center(KEY_ZOOM_FACTOR, cx)))
            .on_action(
                cx.listener(|c, _: &ZoomOut, _, cx| c.zoom_center(1.0 / KEY_ZOOM_FACTOR, cx)),
            )
            .on_action(cx.listener(|c, _: &Fit, _, cx| c.refit(cx)))
            .on_action(cx.listener(|c, _: &ToggleEdit, window, cx| {
                // Enter on a focused toolbar button clicks it instead.
                if c.focus_handle.is_focused(window) {
                    c.toggle_edit(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|c, _: &Close, _, cx| {
                if c.fullscreen {
                    cx.emit(CanvasEvent::Close);
                } else {
                    cx.propagate();
                }
            }))
            .child(diagram)
            .child(top)
            .child(zoom);

        let mut root = div()
            .id("mermaid-canvas")
            .size_full()
            .flex()
            .rounded(px(8.0))
            .border_1()
            .border_color(if self.focus_handle.is_focused(window) {
                theme::accent()
            } else {
                theme::border()
            })
            .bg(theme::background())
            .overflow_hidden()
            .cursor(CursorStyle::Arrow)
            .role(gpui::Role::Image)
            .aria_label(SharedString::from(accessible_name(&self.source)))
            // The text behind never sees a click in the canvas.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        if self.editing
            && let Some((panel, _)) = &self.panel
        {
            let panel = div()
                .id("mermaid-panel")
                .w(relative(PANEL_SHARE))
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(theme::border())
                .bg(theme::code_background())
                .child(panel.clone());
            #[cfg(test)]
            let panel = panel.debug_selector(|| "mermaid-panel".into());
            root = root.child(panel);
        }
        let root = root.child(stage);
        #[cfg(test)]
        let root = root.debug_selector(|| "mermaid-canvas".into());
        root
    }
}

// --- In the editor -------------------------------------------------------------

/// The ranges of the document's ` ```mermaid ` fences, in order, at any
/// depth of quotes and lists.
pub fn fences(
    tree: &cerata_markdown::MarkdownTree,
    text: &cerata_markdown::Text,
) -> Vec<std::ops::Range<usize>> {
    use cerata_markdown::NodeKind;
    fn walk(
        node: cerata_markdown::Node<'_>,
        text: &cerata_markdown::Text,
        out: &mut Vec<std::ops::Range<usize>>,
    ) {
        for child in node.children() {
            match child.kind() {
                NodeKind::FencedCode => {
                    let mermaid = child.child(NodeKind::CodeInfo).is_some_and(|i| {
                        text.slice(i.range())
                            .trim()
                            .to_ascii_lowercase()
                            .starts_with("mermaid")
                    });
                    if mermaid {
                        out.push(child.range());
                    }
                }
                NodeKind::Blockquote
                | NodeKind::BulletList
                | NodeKind::OrderedList
                | NodeKind::ListItem => walk(child, text, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(tree.root(), text, &mut out);
    out
}

impl Editor {
    /// The canvas drawn for the fence starting at `pos`, `width` wide.
    pub(super) fn mermaid_element(
        &mut self,
        source: &str,
        pos: usize,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fences = fences(self.state.tree(), self.state.text());
        let Some(index) = fences.iter().position(|r| r.start <= pos && pos <= r.end) else {
            return div().into_any_element();
        };
        let fence = self.state.text().slice(fences[index].clone()).to_owned();
        let config = self.config.clone();
        let canvas = match self.canvases.get(&index) {
            Some((canvas, _)) => canvas.clone(),
            None => {
                let canvas = cx.new(|cx| MermaidCanvas::new(false, config, cx));
                let sub = cx.subscribe(&canvas, move |editor: &mut Self, canvas, event, cx| {
                    editor.on_canvas(index, &canvas, event, cx);
                });
                self.canvases.insert(index, (canvas.clone(), sub));
                canvas
            }
        };
        let source = source.to_owned();
        canvas.update(cx, |c, cx| c.set_source(&source, &fence, cx));
        div()
            .w(px(width))
            .h(px(CANVAS_HEIGHT + CANVAS_MARGIN * 2.0))
            .py(px(CANVAS_MARGIN))
            .child(canvas)
            .into_any_element()
    }

    fn on_canvas(
        &mut self,
        index: usize,
        canvas: &Entity<MermaidCanvas>,
        event: &CanvasEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            CanvasEvent::Rewrite(next) => {
                let fences = fences(self.state.tree(), self.state.text());
                let Some(range) = fences.get(index).cloned() else {
                    return;
                };
                if self.state.text().slice(range.clone()) == next.as_str() {
                    return;
                }
                let change = cerata_markdown::Change::new(range, next.clone());
                // The caret stays where it was: the panel has the focus.
                self.update_state(cx, |state| {
                    let _ = state.apply(
                        cerata_markdown::Transaction::new()
                            .change(change)
                            .event(cerata_markdown::UserEvent::Type),
                    );
                });
            }
            CanvasEvent::Expand => {
                let config = self.config.clone();
                let (source, fence) = {
                    let c = canvas.read(cx);
                    (c.source.clone(), c.fence.clone())
                };
                let full = cx.new(|cx| {
                    let mut full = MermaidCanvas::new(true, config, cx);
                    full.set_source(&source, &fence, cx);
                    full
                });
                let sub = cx.subscribe(&full, |editor: &mut Self, _, event, cx| {
                    if *event == CanvasEvent::Close {
                        editor.close_fullscreen(cx);
                    }
                });
                self.fullscreen = Some(Fullscreen {
                    canvas: full,
                    _events: sub,
                    returns_to: canvas.read(cx).focus_handle.clone(),
                    focus: true,
                });
                cx.notify();
            }
            CanvasEvent::Close => {}
        }
    }

    /// Closes the full-screen diagram; the focus goes back to its canvas.
    pub(super) fn close_fullscreen(&mut self, cx: &mut Context<Self>) {
        if let Some(full) = self.fullscreen.take() {
            self.refocus_to = Some(full.returns_to);
            cx.notify();
        }
    }

    /// The full-screen overlay, over the whole editor.
    pub(super) fn render_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let full = self.fullscreen.as_mut()?;
        if std::mem::take(&mut full.focus) {
            let handle = full.canvas.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        }
        let canvas = full.canvas.clone();
        let name = accessible_name(&canvas.read(cx).source);
        let overlay = div()
            .id("mermaid-fullscreen")
            .aria_label(SharedString::from(name))
            .absolute()
            .inset_0()
            .p(px(32.0))
            .bg(theme::overlay())
            .occlude()
            .role(gpui::Role::Dialog)
            // A click on the backdrop closes; the canvas stops its clicks.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, _, _, cx| {
                    cx.stop_propagation();
                    editor.close_fullscreen(cx);
                }),
            )
            .child(canvas);
        #[cfg(test)]
        let overlay = overlay.debug_selector(|| "mermaid-fullscreen".into());
        Some(overlay.into_any_element())
    }
}

/// The diagram open full screen, and the canvas the focus returns to.
pub struct Fullscreen {
    pub canvas: Entity<MermaidCanvas>,
    _events: Subscription,
    returns_to: FocusHandle,
    /// Take the focus at the next frame.
    focus: bool,
}

#[cfg(test)]
mod tests {
    /// `mermaid.test.ts:314-330`: the first line names a diagram, unless it
    /// has an accessible title of its own.
    #[test]
    fn a_diagram_is_named_by_its_title_or_first_line() {
        assert_eq!(
            super::accessible_name(
                "pie
  \"A\" : 1"
            ),
            "Mermaid diagram: pie"
        );
        assert_eq!(
            super::accessible_name(
                "flowchart LR
  accTitle: Pipeline
  A --> B"
            ),
            "Pipeline"
        );
    }

    /// `mermaid.test.ts:243-275` and `:270`: the theme sets every color
    /// Pele reads; the canvas has a fixed whole-pixel height.
    #[test]
    fn the_theme_sets_every_color_pele_reads() {
        let theme = super::diagram_theme();
        for name in cerata_render::js::PELE_COLOR_VARS {
            assert!(theme.vars.iter().any(|(k, _)| k == name), "{name}");
        }
        assert!(super::CANVAS_HEIGHT > 0.0 && super::CANVAS_HEIGHT.fract() == 0.0);
    }

    use super::*;

    /// A wide diagram fills the width with 16 px each side, centered; a
    /// small one stops at 4x; one too large for 0.25 keeps its fit.
    #[test]
    fn a_fit_keeps_16_px_and_bounds_the_zoom() {
        let (fit, view) = fit_view(2000.0, 400.0, 600.0, 480.0).unwrap();
        assert!((view.zoom - fit).abs() < 1e-6);
        assert!((view.pan.x - 16.0).abs() < 1e-3);
        assert!((view.pan.y - (480.0 - 400.0 * fit) / 2.0).abs() < 1e-3);
        let (_, small) = fit_view(20.0, 10.0, 600.0, 480.0).unwrap();
        assert_eq!(small.zoom, ZOOM_MAX);
        let (huge_fit, huge) = fit_view(20000.0, 400.0, 600.0, 480.0).unwrap();
        assert!(huge_fit < ZOOM_MIN);
        assert_eq!(huge.zoom, huge_fit, "the whole diagram can be shown");
        assert!(fit_view(0.0, 10.0, 600.0, 480.0).is_none());
    }

    #[test]
    fn scaled_svg_keeps_its_view_box() {
        let out = scaled(
            "<svg viewBox=\"0 0 10 5\" width=\"10\" height=\"5\"><g/></svg>",
            20.0,
            10.0,
        );
        assert_eq!(
            out,
            "<svg viewBox=\"0 0 10 5\" width=\"20\" height=\"10\"><g/></svg>"
        );
    }
}
