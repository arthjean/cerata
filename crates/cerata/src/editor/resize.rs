use std::ops::Range;

use cerata_markdown::image_size::{self, ImageNode};
use gpui::{
    AnyElement, Context, CursorStyle, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, SharedString, Styled as _, Window, div, hsla, px,
};

use super::Editor;
use super::mouse::Drag;

pub const MIN_WIDTH: f32 = 64.0;
pub const SNAP: f32 = 8.0;
const BAR_WIDTH: f32 = 6.0;
const BAR_INSET: f32 = 8.0;
const BAR_MAX_HEIGHT: f32 = 48.0;
const HIT: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resize {
    pub range: Range<usize>,
    pub source: String,
    pub side: Side,
    pub start_x: Pixels,
    pub start_width: f32,
    pub width: f32,
    pub default_width: f32,
    pub canceled: bool,
}

#[derive(Clone, Debug)]
pub struct Grip {
    pub range: Range<usize>,
    pub url: String,
    pub ix: usize,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    Quarter,
    Half,
    ThreeQuarters,
    Full,
    Original,
}

impl Preset {
    pub const ALL: [Self; 5] = [
        Self::Quarter,
        Self::Half,
        Self::ThreeQuarters,
        Self::Full,
        Self::Original,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Quarter => "25%",
            Self::Half => "50%",
            Self::ThreeQuarters => "75%",
            Self::Full => "Full width",
            Self::Original => "Original size",
        }
    }

    pub fn command_label(self) -> &'static str {
        match self {
            Self::Quarter => "Image width: 25%",
            Self::Half => "Image width: 50%",
            Self::ThreeQuarters => "Image width: 75%",
            Self::Full => "Image width: Full width",
            Self::Original => "Image width: Original size",
        }
    }

    fn width(self, column: f32) -> Option<f32> {
        let share = match self {
            Self::Quarter => 0.25,
            Self::Half => 0.5,
            Self::ThreeQuarters => 0.75,
            Self::Full => 1.0,
            Self::Original => return None,
        };
        Some((column * share).round())
    }
}

pub fn image_presets_of(image: &ImageNode) -> Vec<(Preset, bool)> {
    Preset::ALL
        .iter()
        .map(|p| (*p, *p != Preset::Original || image.size.is_some()))
        .collect()
}

pub fn written_width(width: f32, default_width: f32) -> Option<u32> {
    let width = width.round();
    ((width - default_width).abs() >= SNAP).then_some(width.max(1.0) as u32)
}

impl Editor {
    pub(super) fn image_preview(&self) -> Option<(usize, f32)> {
        match &self.drag {
            Some(Drag::Resize(resize)) => Some((resize.range.start, resize.width)),
            _ => None,
        }
    }

    pub fn image_at_head(&self) -> Option<ImageNode> {
        let head = self.state.selection().main().head;
        image_size::image_at(self.state.tree(), self.state.text(), head).filter(|i| i.block)
    }

    fn default_width(&self, url: &str) -> f32 {
        let column = self.column_width();
        self.images
            .natural(url)
            .map_or(column, |(w, _)| w.min(column).floor().max(1.0))
    }

    pub(super) fn image_key(&self, image: &ImageNode) -> Option<String> {
        let text = self.state.text();
        if image.embed {
            let embed = cerata_markdown::wiki::image_embed_target(text.slice(image.label.clone()))?;
            return Some(super::images::embed_key(&embed.path));
        }
        let node = self
            .state
            .tree()
            .iter_range(image.range.clone())
            .find(|n| n.kind() == cerata_markdown::NodeKind::Image && n.range() == image.range)?;
        let url = node
            .children()
            .find(|c| c.kind() == cerata_markdown::NodeKind::Url)?;
        Some(cerata_markdown::decorate::normalize_markdown_destination(
            text.slice(url.range()),
        ))
    }

    pub fn start_resize(
        &mut self,
        grip: &Grip,
        side: Side,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.menu = None;
        let source = self.state.text().slice(grip.range.clone()).to_owned();
        self.drag = Some(Drag::Resize(Resize {
            range: grip.range.clone(),
            source,
            side,
            start_x: event.position.x,
            start_width: grip.width,
            width: grip.width,
            default_width: self.default_width(&grip.url),
            canceled: false,
        }));
        cx.notify();
    }

    pub fn resize_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let column = self.column_width();
        let Some(Drag::Resize(resize)) = &mut self.drag else {
            return;
        };
        if resize.canceled {
            return;
        }
        let dx = (x - resize.start_x).as_f32();
        let grow = match resize.side {
            Side::Right => dx,
            Side::Left => -dx,
        };
        let width = (resize.start_width + 2.0 * grow)
            .clamp(MIN_WIDTH.min(column), column)
            .round();
        if width != resize.width {
            resize.width = width;
            cx.notify();
        }
    }

    pub fn release_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if !matches!(self.drag, Some(Drag::Resize(_))) {
            return false;
        }
        if let Some(Drag::Resize(resize)) = self.drag.take() {
            self.finish_resize(resize, cx);
        }
        true
    }

    fn finish_resize(&mut self, resize: Resize, cx: &mut Context<Self>) {
        cx.notify();
        if resize.canceled || resize.width == resize.start_width {
            return;
        }
        let text = self.state.text();
        if resize.range.end > text.len() || text.slice(resize.range.clone()) != resize.source {
            return;
        }
        let width = written_width(resize.width, resize.default_width);
        self.update_state(cx, |state| state.set_image_width(resize.range, width));
    }

    pub fn cancel_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(Drag::Resize(resize)) = &mut self.drag else {
            return false;
        };
        resize.canceled = true;
        resize.width = resize.start_width;
        cx.notify();
        true
    }

    pub fn set_image_width_preset(&mut self, preset: Preset, cx: &mut Context<Self>) -> bool {
        match self.image_at_head() {
            Some(image) => self.set_image_width_preset_of(&image, preset, cx),
            None => false,
        }
    }

    pub fn set_image_width_preset_of(
        &mut self,
        image: &ImageNode,
        preset: Preset,
        cx: &mut Context<Self>,
    ) -> bool {
        let column = self.column_width().floor();
        let default = self
            .image_key(image)
            .map_or(column, |url| self.default_width(&url));
        let width = preset.width(column).and_then(|w| written_width(w, default));
        let range = image.range.clone();
        self.update_state(cx, |state| state.set_image_width(range, width))
    }

    pub fn image_presets(&self) -> Vec<(Preset, bool)> {
        self.image_at_head()
            .map_or_else(Vec::new, |image| image_presets_of(&image))
    }

    pub(super) fn image_handles(
        &self,
        grip: &Grip,
        group: &SharedString,
        shown: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (ix, width, height) = (grip.ix, grip.width, grip.height);
        let bar = BAR_MAX_HEIGHT.min(height / 3.0).max(1.0);
        let hit_height = bar.max(HIT).min(height.max(HIT));
        let top = ((height - hit_height) / 2.0).max(0.0);
        [Side::Left, Side::Right]
            .into_iter()
            .map(|side| {
                let (name, left, bar_left) = match side {
                    Side::Left => ("left", 0.0, BAR_INSET),
                    Side::Right => ("right", width - HIT, HIT - BAR_INSET - BAR_WIDTH),
                };
                let grip = grip.clone();
                let handle = div()
                    .id(SharedString::from(format!("image-handle-{name}-{ix}")))
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(HIT))
                    .h(px(hit_height))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .child(
                        div()
                            .absolute()
                            .left(px(bar_left))
                            .top(px((hit_height - bar) / 2.0))
                            .w(px(BAR_WIDTH))
                            .h(px(bar))
                            .rounded(px(BAR_WIDTH / 2.0))
                            .bg(hsla(0.0, 0.0, 0.1, 0.85))
                            .border_1()
                            .border_color(hsla(0.0, 0.0, 1.0, 0.9)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |editor, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            editor.start_resize(&grip, side, event, window, cx);
                        }),
                    );
                #[cfg(test)]
                let handle = {
                    let selector = format!("image-handle-{name}-{ix}");
                    handle.debug_selector(move || selector)
                };
                let handle = if shown {
                    handle
                } else {
                    handle
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                };
                handle.into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_width_near_the_default_one_writes_no_size() {
        assert_eq!(written_width(300.0, 400.0), Some(300));
        assert_eq!(written_width(392.0, 400.0), Some(392));
        assert_eq!(written_width(392.6, 400.0), None, "393 is 7 px away");
        assert_eq!(written_width(395.0, 400.0), None);
        assert_eq!(written_width(407.4, 400.0), None);
        assert_eq!(written_width(408.0, 400.0), Some(408));
    }

    #[test]
    fn presets_share_the_column() {
        assert_eq!(Preset::Quarter.width(734.0), Some(184.0));
        assert_eq!(Preset::Half.width(734.0), Some(367.0));
        assert_eq!(Preset::ThreeQuarters.width(734.0), Some(551.0));
        assert_eq!(Preset::Full.width(734.0), Some(734.0));
        assert_eq!(Preset::Original.width(734.0), None);
    }
}
