//! Formulas (US-036), rendered by MathJax on the renderer's thread
//! (`cerata_render::js`) and drawn as SVG images: inline in the text, or as
//! a block for `$$...$$` alone on its lines. Ported from
//! `math-decorations.ts:12-78` and `math-widget.css`.
//!
//! Until a formula is rendered its place is estimated; a formula MathJax
//! rejects shows its source in red, the error in a tooltip.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cerata_render::js::{MathRequest, Rendered, Renderer, Svg};
use gpui::{
    AnyElement, AnyView, Context, Image, ImageFormat, Render, SharedString, Window, div, img,
    prelude::*, px,
};

use super::blocks::BlockStyle;
use super::{Editor, theme};

/// Where a formula's render stands.
#[derive(Clone, Debug)]
pub enum MathState {
    Ready(Arc<Svg>),
    Error(Arc<str>),
    Pending,
}

/// Formulas this editor asked for, and their images.
#[derive(Default)]
pub struct MathHost {
    pending: HashSet<MathRequest>,
    /// Asked for during the last layout, sent after it.
    queued: Vec<MathRequest>,
    images: HashMap<usize, Arc<Image>>,
}

/// `#rrggbb` of a theme color.
pub fn hex(color: gpui::Hsla) -> String {
    let rgba = color.to_rgb();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b)
    )
}

impl MathHost {
    fn request(tex: &str, display: bool, font_px: f32) -> MathRequest {
        MathRequest {
            tex: tex.to_owned(),
            display,
            font_px: font_px.round() as u32,
            color: hex(theme::text()),
        }
    }

    /// The render of `tex` if done, else queued for after this layout.
    pub fn state(&mut self, tex: &str, display: bool, font_px: f32) -> MathState {
        let request = Self::request(tex, display, font_px);
        match Renderer::global().cached_math(&request) {
            Some(Ok(svg)) => MathState::Ready(svg),
            Some(Err(error)) => MathState::Error(error),
            None => {
                if self.pending.insert(request.clone()) {
                    self.queued.push(request);
                }
                MathState::Pending
            }
        }
    }

    /// A GPUI image of `svg`, made once per render.
    pub fn image(&mut self, svg: &Arc<Svg>) -> Arc<Image> {
        if self.images.len() > 512 {
            self.images.clear();
        }
        let key = Arc::as_ptr(&svg.data) as *const u8 as usize;
        self.images
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    svg.data.as_bytes().to_vec(),
                ))
            })
            .clone()
    }
}

/// The estimated width of a formula not rendered yet.
pub fn estimated_width(tex: &str, font_px: f32) -> f32 {
    (tex.chars().count() as f32 * font_px * 0.45).clamp(font_px, font_px * 24.0)
}

/// A block formula's height in a column, padding included.
pub fn block_height(state: &MathState, tex: &str, font_px: f32, row_height: f32) -> f32 {
    let pad = font_px;
    match state {
        MathState::Ready(svg) => svg.height + pad,
        MathState::Error(_) => (tex.lines().count() + 2) as f32 * row_height * 0.9 + pad,
        MathState::Pending => (row_height * 2.0).max(font_px * 2.5),
    }
}

/// A tooltip with the error MathJax gave.
pub struct Tip(pub SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(420.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(theme::text())
            .text_color(theme::background())
            .text_size(px(12.0))
            .child(self.0.clone())
    }
}

pub fn tooltip(message: SharedString) -> impl Fn(&mut Window, &mut gpui::App) -> AnyView + 'static {
    move |_, cx| AnyView::from(cx.new(|_| Tip(message.clone())))
}

impl Editor {
    /// Sends the formulas the last layout asked for to the renderer; each
    /// answer draws the editor again.
    pub(super) fn start_math(&mut self, cx: &mut Context<Self>) {
        let batch = std::mem::take(&mut self.math.queued);
        if batch.is_empty() {
            return;
        }
        // One background task waits for the whole batch: the renderer's
        // thread does the work, the task only blocks on its answers.
        let waited = batch.clone();
        let task = cx.background_spawn(async move {
            for request in waited {
                let _: Rendered = Renderer::global().math_blocking(request);
            }
        });
        cx.spawn(async move |this, cx| {
            task.await;
            this.update(cx, |editor, cx| {
                for request in &batch {
                    editor.math.pending.remove(request);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The element of a block formula `width` wide.
    pub(super) fn math_block(
        &mut self,
        tex: &str,
        display: bool,
        width: f32,
        style: &BlockStyle,
        _window: &Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let em = style.font_size;
        let state = self.math.state(tex, display, em);
        let frame = div().w(px(width)).py(px(em * 0.5)).flex().justify_center();
        match state {
            MathState::Ready(svg) => {
                // Wider than the column: scaled down to fit.
                let scale = (width / svg.width.max(1.0)).min(1.0);
                let image = self.math.image(&svg);
                let picture = img(image)
                    .w(px(svg.width * scale))
                    .h(px(svg.height * scale));
                #[cfg(test)]
                let picture = picture.debug_selector(|| "math-block".into());
                frame.child(picture).into_any_element()
            }
            MathState::Error(error) => {
                let source = if display {
                    format!("$${tex}$$")
                } else {
                    format!("${tex}$")
                };
                let message = SharedString::from(error.to_string());
                let error = div()
                    .id(SharedString::from(format!(
                        "math-error-{}",
                        cerata_render::js::hash_of(&tex)
                    )))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(theme::code_background())
                    .text_color(theme::danger())
                    .font_family(style.fonts.mono.clone())
                    .children(source.lines().map(|l| SharedString::from(l.to_owned())))
                    .tooltip(tooltip(message));
                #[cfg(test)]
                let error = error.debug_selector(|| "math-error".into());
                frame.justify_start().child(error).into_any_element()
            }
            MathState::Pending => frame
                .h(px(block_height(&MathState::Pending, tex, em, em * 1.6)))
                .into_any_element(),
        }
    }
}
