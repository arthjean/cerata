//! Block image widths from the view: the resize handles (US-067) and the
//! preset widths of the menu and the palette (US-068). The width lives in
//! the text as Obsidian writes it (`cerata_markdown::image_size`, ADR
//! 0008); during a gesture only the view's preview changes, and the release
//! writes it in one undo step.

use std::ops::Range;

use cerata_markdown::image_size::{self, ImageNode};
use gpui::{
    AnyElement, Context, CursorStyle, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, SharedString, Styled as _, Window, div, hsla, px,
};

use super::Editor;
use super::mouse::Drag;

/// The narrowest a handle makes an image.
pub const MIN_WIDTH: f32 = 64.0;
/// A width this close to the default one removes the size: the file stays
/// clean (the magnet).
pub const SNAP: f32 = 8.0;
/// Handle bar: its width, its distance from the image's edge, its tallest.
const BAR_WIDTH: f32 = 6.0;
const BAR_INSET: f32 = 8.0;
const BAR_MAX_HEIGHT: f32 = 48.0;
/// The handle's hit zone (WCAG 2.2 SC 2.5.8).
const HIT: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A resize gesture on a block image.
#[derive(Clone, Debug, PartialEq)]
pub struct Resize {
    /// The image's node and its source when the gesture started: a release
    /// writes only if the text there is still the same.
    pub range: Range<usize>,
    pub source: String,
    pub side: Side,
    pub start_x: Pixels,
    pub start_width: f32,
    /// What the view shows.
    pub width: f32,
    /// `min(natural, column)`: a release this close removes the size.
    pub default_width: f32,
    /// Escape: the width went back, the release writes nothing.
    pub canceled: bool,
}

/// The block image a handle resizes: its node, host key, line and the
/// size it is drawn at.
#[derive(Clone, Debug)]
pub struct Grip {
    pub range: Range<usize>,
    pub url: String,
    pub ix: usize,
    pub width: f32,
    pub height: f32,
}

/// A preset width (US-068), from the menu or the palette.
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

    /// The menu's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Quarter => "25%",
            Self::Half => "50%",
            Self::ThreeQuarters => "75%",
            Self::Full => "Full width",
            Self::Original => "Original size",
        }
    }

    /// The palette's label.
    pub fn command_label(self) -> &'static str {
        match self {
            Self::Quarter => "Image width: 25%",
            Self::Half => "Image width: 50%",
            Self::ThreeQuarters => "Image width: 75%",
            Self::Full => "Image width: Full width",
            Self::Original => "Image width: Original size",
        }
    }

    /// The width written for a column `column` wide, `None` for the
    /// original size.
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

/// The presets that apply to `image`: `Original size` only when it has a
/// size.
pub fn image_presets_of(image: &ImageNode) -> Vec<(Preset, bool)> {
    Preset::ALL
        .iter()
        .map(|p| (*p, *p != Preset::Original || image.size.is_some()))
        .collect()
}

/// The size written for a width the view shows: none within [`SNAP`] of
/// the default width.
pub fn written_width(width: f32, default_width: f32) -> Option<u32> {
    let width = width.round();
    ((width - default_width).abs() >= SNAP).then_some(width.max(1.0) as u32)
}

impl Editor {
    /// The preview of the gesture in flight, for the layout: the image's
    /// start and the width shown.
    pub(super) fn image_preview(&self) -> Option<(usize, f32)> {
        match &self.drag {
            Some(Drag::Resize(resize)) => Some((resize.range.start, resize.width)),
            _ => None,
        }
    }

    /// The block image the main selection's head is on, or in the source
    /// of, as an image node (the menu and the palette act on it).
    pub fn image_at_head(&self) -> Option<ImageNode> {
        let head = self.state.selection().main().head;
        image_size::image_at(self.state.tree(), self.state.text(), head).filter(|i| i.block)
    }

    /// `min(natural, column)` for the image of `url` (an embed by its
    /// key), the column when its size is unknown.
    fn default_width(&self, url: &str) -> f32 {
        let column = self.column_width();
        self.images
            .natural(url)
            .map_or(column, |(w, _)| w.min(column).floor().max(1.0))
    }

    /// The host key of `image`: its destination, or an embed's key.
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

    /// A press on a handle (or the benchmark's, US-066): the gesture
    /// starts from the width shown.
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
        // The next frame installs the window-wide drag listeners.
        cx.notify();
    }

    /// A move during the gesture: twice the horizontal move, since the
    /// image stays centered, between [`MIN_WIDTH`] and the column.
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

    /// The release ends the gesture in flight; `false` without one.
    pub fn release_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if !matches!(self.drag, Some(Drag::Resize(_))) {
            return false;
        }
        if let Some(Drag::Resize(resize)) = self.drag.take() {
            self.finish_resize(resize, cx);
        }
        true
    }

    /// The width shown is written, unless Escape canceled the gesture or
    /// the image's source changed meanwhile (a reload).
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

    /// Escape during a gesture: the width goes back. `false` without one.
    pub fn cancel_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(Drag::Resize(resize)) = &mut self.drag else {
            return false;
        };
        resize.canceled = true;
        resize.width = resize.start_width;
        cx.notify();
        true
    }

    /// The preset width of the image at the head (US-068), in one undo
    /// step. A width within [`SNAP`] of the default one removes the size.
    pub fn set_image_width_preset(&mut self, preset: Preset, cx: &mut Context<Self>) -> bool {
        match self.image_at_head() {
            Some(image) => self.set_image_width_preset_of(&image, preset, cx),
            None => false,
        }
    }

    /// [`Editor::set_image_width_preset`] for `image`, wherever the caret
    /// is (the toolbar's, US-070).
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

    /// The presets that apply to the image at the head.
    pub fn image_presets(&self) -> Vec<(Preset, bool)> {
        self.image_at_head()
            .map_or_else(Vec::new, |image| image_presets_of(&image))
    }

    /// The two handles of `grip`'s image, shown on hover of `group` (its
    /// frame), or always (`shown`: selected as an object, or resizing).
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
                        // Dark fill, light border: seen on a white image as
                        // on a black one (SC 1.4.11).
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
