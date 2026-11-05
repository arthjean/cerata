use std::ops::Range;

use cerata_markdown::Selection;
use cerata_markdown::image_size::{self, ImageNode};
use gpui::{
    AnyElement, AppContext as _, ClipboardItem, Context, HoverListenerMode, Image,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Point,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};

use super::images::{Reach, accessible_name, sniff_format};
use super::menu::{ContextMenu, width_items};
use super::resize::{Preset, image_presets_of};
use super::{Editor, theme};
use crate::icons::{Icon, icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageAction {
    Copy,
    Open,
    EditSource,
    Delete,
}

impl ImageAction {
    pub const ALL: [Self; 4] = [Self::Copy, Self::Open, Self::EditSource, Self::Delete];

    pub fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy image",
            Self::Open => "Open image",
            Self::EditSource => "Edit source",
            Self::Delete => "Delete image",
        }
    }

    pub fn command_label(self) -> &'static str {
        match self {
            Self::EditSource => "Edit image source",
            other => other.label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageMenu {
    pub range: Range<usize>,
    pub presets: Vec<(Preset, bool)>,
    pub actions: Vec<(ImageAction, bool)>,
}

impl Editor {
    pub(super) fn block_image(&self, range: &Range<usize>) -> Option<ImageNode> {
        image_size::image_at(self.state.tree(), self.state.text(), range.start)
            .filter(|image| image.block && image.range == *range)
    }

    fn image_reach(&self, image: &ImageNode) -> Option<Reach> {
        self.image_key(image)
            .and_then(|key| self.images.reach(&key))
    }

    pub fn image_actions_of(&self, image: &ImageNode) -> Vec<(ImageAction, bool)> {
        let reach = self.image_reach(image);
        ImageAction::ALL
            .iter()
            .map(|action| {
                let enabled = match action {
                    ImageAction::Copy => reach.is_some(),
                    ImageAction::Open => {
                        matches!(reach, Some(Reach::File(_) | Reach::Remote { .. }))
                    }
                    ImageAction::EditSource | ImageAction::Delete => true,
                };
                (*action, enabled)
            })
            .collect()
    }

    pub fn image_actions(&self) -> Vec<(ImageAction, bool)> {
        self.image_at_head()
            .map_or_else(Vec::new, |image| self.image_actions_of(&image))
    }

    pub(super) fn image_menu(&self) -> Option<ImageMenu> {
        let image = self.image_at_head()?;
        Some(ImageMenu {
            range: image.range.clone(),
            presets: image_presets_of(&image),
            actions: self.image_actions_of(&image),
        })
    }

    pub fn run_image_action_at_head(
        &mut self,
        action: ImageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.image_at_head() {
            Some(image) => self.run_image_action(action, &image.range, window, cx),
            None => false,
        }
    }

    pub fn run_image_action(
        &mut self,
        action: ImageAction,
        range: &Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(image) = self.block_image(range) else {
            return false;
        };
        match action {
            ImageAction::Copy => self.copy_image(&image, cx),
            ImageAction::Open => self.open_image(&image, window, cx),
            ImageAction::EditSource => {
                let caret = image.alt_end(self.state.text());
                window.focus(&self.focus_handle, cx);
                self.select(Selection::cursor(caret), cx);
                true
            }
            ImageAction::Delete => self.update_state(cx, |state| state.delete_image(image.range)),
        }
    }

    fn copy_image(&mut self, image: &ImageNode, cx: &mut Context<Self>) -> bool {
        match self.image_reach(image) {
            Some(Reach::File(path)) => {
                let name = accessible_name(&path.to_string_lossy());
                let read = cx.background_spawn(async move { std::fs::read(&path) });
                cx.spawn(async move |this, cx| {
                    let bytes = read.await;
                    this.update(cx, |editor, cx| match bytes {
                        Ok(bytes) => match sniff_format(&bytes) {
                            Some(format) => cx.write_to_clipboard(ClipboardItem::new_image(
                                &Image::from_bytes(format, bytes),
                            )),
                            None => editor.show_notice(unreadable(&name), cx),
                        },
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            editor.show_notice(not_found(&name), cx)
                        }
                        Err(_) => editor.show_notice(unreadable(&name), cx),
                    })
                    .ok();
                })
                .detach();
                true
            }
            Some(Reach::Remote { image, .. } | Reach::Inline(image)) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&image));
                true
            }
            None => false,
        }
    }

    fn open_image(
        &mut self,
        image: &ImageNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.image_reach(image) {
            Some(Reach::File(path)) => {
                if path.is_file() {
                    self.confirm_open(path, window, cx);
                } else {
                    let name = accessible_name(&path.to_string_lossy());
                    self.show_notice(not_found(&name), cx);
                }
                true
            }
            Some(Reach::Remote { url, .. }) => {
                cx.open_url(&url);
                true
            }
            Some(Reach::Inline(_)) | None => false,
        }
    }
}

fn not_found(name: &str) -> String {
    format!("Image not found: {name}. Restore the file or edit the image's source.")
}

fn unreadable(name: &str) -> String {
    format!(
        "Can't copy {name}: the file can't be read as an image. Check it or edit the image's source."
    )
}

const INSET: f32 = 8.0;
const BUTTON: f32 = 28.0;
const PAD: f32 = 3.0;
const GAP: f32 = 2.0;

#[derive(Debug, Default)]
pub struct Toolbar {
    image: Option<usize>,
    bar: Option<usize>,
    pub(super) drawn: Option<usize>,
    closed: Option<usize>,
    held: bool,
}

impl Toolbar {
    fn hovered(&self) -> Option<usize> {
        self.image.or(self.bar)
    }

    pub(super) fn prune(&mut self, framed: &[usize]) -> bool {
        let before = (self.image, self.bar);
        if self.image.is_some_and(|ix| !framed.contains(&ix)) {
            self.image = None;
        }
        if self.bar.is_some() && self.bar != self.drawn {
            self.bar = None;
        }
        (self.image, self.bar) != before
    }
}

pub(super) struct Spot {
    pub range: Range<usize>,
    pub ix: usize,
    pub width: f32,
    pub origin: Point<Pixels>,
    pub min_left: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Width,
    Action(ImageAction),
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Self::Width => "Image width",
            Self::Action(action) => action.label(),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Width => Icon::Width,
            Self::Action(ImageAction::Copy) => Icon::Copy,
            Self::Action(ImageAction::Open) => Icon::OpenExternal,
            Self::Action(ImageAction::EditSource) => Icon::Source,
            Self::Action(ImageAction::Delete) => Icon::Delete,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Width => "width",
            Self::Action(ImageAction::Copy) => "copy",
            Self::Action(ImageAction::Open) => "open",
            Self::Action(ImageAction::EditSource) => "edit",
            Self::Action(ImageAction::Delete) => "delete",
        }
    }
}

impl Editor {
    pub(super) fn toolbar_shown(&self, ix: usize, object: bool) -> bool {
        (object || self.toolbar.hovered() == Some(ix)) && self.toolbar.closed != Some(ix)
    }

    pub(super) fn hover_image(
        &mut self,
        ix: usize,
        hovered: bool,
        bar: bool,
        cx: &mut Context<Self>,
    ) {
        let before = self.toolbar.hovered();
        let slot = if bar {
            &mut self.toolbar.bar
        } else {
            &mut self.toolbar.image
        };
        if hovered {
            *slot = Some(ix);
        } else if *slot == Some(ix) {
            *slot = None;
        }
        let after = self.toolbar.hovered();
        if after != before {
            if after.is_none() || !self.toolbar.held {
                self.toolbar.closed = None;
            }
            cx.notify();
        }
    }

    pub(super) fn close_toolbar(&mut self, cx: &mut Context<Self>) -> bool {
        match self.toolbar.drawn.take() {
            Some(ix) => {
                self.toolbar.closed = Some(ix);
                self.toolbar.held = self.toolbar.hovered() == Some(ix);
                cx.notify();
                true
            }
            None => false,
        }
    }

    pub(super) fn image_toolbar(
        &mut self,
        spot: Spot,
        ready: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let image = self.block_image(&spot.range)?;
        let actions = self.image_actions_of(&image);
        let buttons: Vec<Button> = ready
            .then_some(Button::Width)
            .into_iter()
            .chain(
                actions
                    .iter()
                    .filter(|(action, enabled)| {
                        *enabled
                            && (ready
                                || matches!(action, ImageAction::EditSource | ImageAction::Delete))
                    })
                    .map(|(action, _)| Button::Action(*action)),
            )
            .collect();
        let count = buttons.len() as f32;
        let bar_width = count * BUTTON + (count - 1.0).max(0.0) * GAP + 2.0 * (PAD + 1.0);
        let left = (spot.width - INSET - bar_width).max(spot.min_left);
        let ix = spot.ix;
        self.toolbar.drawn = Some(ix);
        let tips = self.menu.is_none();
        let first = point(
            spot.origin.x + px(left + PAD + 1.0),
            spot.origin.y + px(INSET + BUTTON + 2.0 * (PAD + 1.0) + 4.0),
        );
        let mut bar = div()
            .id(SharedString::from(format!("image-toolbar-{ix}")))
            .absolute()
            .left(px(left))
            .top(px(INSET))
            .occlude()
            .flex()
            .gap(px(GAP))
            .p(px(PAD))
            .rounded(px(4.0 + PAD + 1.0))
            .bg(theme::overlay())
            .border_1()
            .border_color(theme::border())
            .shadow_md()
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(move |editor, hovered: &bool, _, cx| {
                editor.hover_image(ix, *hovered, true, cx);
            }));
        #[cfg(test)]
        {
            bar = bar.debug_selector(move || format!("image-toolbar-{ix}"));
        }
        for (i, button) in buttons.into_iter().enumerate() {
            let label = button.label();
            let range = spot.range.clone();
            let name = button.name();
            let under = point(first.x + px(i as f32 * (BUTTON + GAP)), first.y);
            let mut el = crate::a11y::button(
                div().id(SharedString::from(format!("image-toolbar-{name}-{ix}"))),
                label,
            )
            .size(px(BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme::row_hover()))
            .child(icon(button.icon(), 16.0, theme::text()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |editor, _, window, cx| {
                match button {
                    Button::Width => editor.open_width_menu(&range, under, cx),
                    Button::Action(action) => {
                        editor.run_image_action(action, &range, window, cx);
                    }
                }
                window.focus(&editor.focus_handle, cx);
                cx.notify();
            }));
            if tips {
                el = el.tooltip(super::math::tooltip(SharedString::new_static(label)));
            }
            #[cfg(test)]
            let el = el.debug_selector(move || format!("image-toolbar-{name}-{ix}"));
            bar = bar.child(el);
        }
        Some(bar.into_any_element())
    }

    fn open_width_menu(&mut self, range: &Range<usize>, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(image) = self.block_image(range) else {
            return;
        };
        let menu = ImageMenu {
            range: image.range.clone(),
            presets: image_presets_of(&image),
            actions: Vec::new(),
        };
        self.menu = Some(ContextMenu::new(at, width_items(&menu)));
        cx.notify();
    }
}
