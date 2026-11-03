//! What can be done to a block image besides its width (US-069): copy it,
//! open it, edit its source, delete its Markdown. The context menu, the
//! palette and the hover toolbar (US-070) offer the same actions; each acts
//! on an image named by its node, so a toolbar click leaves the caret
//! where it was.
//!
//! The toolbar sits in the image's top right corner, on the context
//! menu's surface, while the pointer is on the image or on it, or while
//! the image is selected as an object; Escape closes it.

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

    /// The menu's label, also the toolbar's tooltip.
    pub fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy image",
            Self::Open => "Open image",
            Self::EditSource => "Edit source",
            Self::Delete => "Delete image",
        }
    }

    /// The palette's label.
    pub fn command_label(self) -> &'static str {
        match self {
            Self::EditSource => "Edit image source",
            other => other.label(),
        }
    }
}

/// The Image section of the context menu: the image's node, its preset
/// widths and its actions, each with whether it applies.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageMenu {
    pub range: Range<usize>,
    pub presets: Vec<(Preset, bool)>,
    pub actions: Vec<(ImageAction, bool)>,
}

impl Editor {
    /// The block image whose node is exactly `range`, if the text still
    /// holds it there.
    pub(super) fn block_image(&self, range: &Range<usize>) -> Option<ImageNode> {
        image_size::image_at(self.state.tree(), self.state.text(), range.start)
            .filter(|image| image.block && image.range == *range)
    }

    /// What `image`'s actions reach: its file, or the bytes of a remote or
    /// inline image; `None` until it is drawn (pending, unavailable).
    fn image_reach(&self, image: &ImageNode) -> Option<Reach> {
        self.image_key(image)
            .and_then(|key| self.images.reach(&key))
    }

    /// `image`'s actions: Copy needs its file or bytes, Open its file or
    /// a remote URL (never a `data:` URI).
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

    /// The actions of the block image at the head, empty without one.
    pub fn image_actions(&self) -> Vec<(ImageAction, bool)> {
        self.image_at_head()
            .map_or_else(Vec::new, |image| self.image_actions_of(&image))
    }

    /// The Image section of the menu, for the block image at the head.
    pub(super) fn image_menu(&self) -> Option<ImageMenu> {
        let image = self.image_at_head()?;
        Some(ImageMenu {
            range: image.range.clone(),
            presets: image_presets_of(&image),
            actions: self.image_actions_of(&image),
        })
    }

    /// Runs `action` on the block image at the head (the palette's).
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

    /// Runs `action` on the block image whose node is `range`; `false`
    /// when it is gone or the action does not apply.
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
                // As a double click does (US-065): the caret at the end of
                // the alt text, before a size.
                let caret = image.alt_end(self.state.text());
                window.focus(&self.focus_handle, cx);
                self.select(Selection::cursor(caret), cx);
                true
            }
            // Its Markdown only: the file is never deleted.
            ImageAction::Delete => self.update_state(cx, |state| state.delete_image(image.range)),
        }
    }

    /// A local image is read off the main thread, then put on the
    /// clipboard in its own format (SVG included); a remote or inline one
    /// is already in memory.
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

    /// A local image opens in its default app after a confirmation
    /// (FR-14), a remote one in the browser; never a `data:` URI.
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

/// The notice of an image whose file went: the PRD's text, then the way
/// out.
fn not_found(name: &str) -> String {
    format!("Image not found: {name}. Restore the file or edit the image's source.")
}

/// The notice of an image file that no longer reads as an image.
fn unreadable(name: &str) -> String {
    format!(
        "Can't copy {name}: the file can't be read as an image. Check it or edit the image's source."
    )
}

/// The toolbar's distance from the image's edges, a button's target (WCAG
/// 2.2 SC 2.5.8), the bar's padding and the gap between its buttons.
const INSET: f32 = 8.0;
const BUTTON: f32 = 28.0;
const PAD: f32 = 3.0;
const GAP: f32 = 2.0;

/// What the toolbar follows: the lines of the block image and of the
/// toolbar under the pointer, the toolbar drawn by the last frame, and the
/// one Escape closed: until the pointer leaves when it was closed under
/// the pointer (`held`), else until the pointer enters or leaves an image.
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

    /// Forgets the hover of an image or a toolbar the last frame did not
    /// draw: it went (a scroll, an edit) without the pointer leaving it.
    /// `framed` lists the lines of the images drawn; `true` when something
    /// was forgotten.
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

/// The block image a toolbar is drawn on: its node, its line, its frame's
/// width and window origin, and the leftmost frame-relative x the toolbar
/// may reach (the editor's left edge).
pub(super) struct Spot {
    pub range: Range<usize>,
    pub ix: usize,
    pub width: f32,
    pub origin: Point<Pixels>,
    pub min_left: f32,
}

/// One toolbar button: the action it runs, or the Image width submenu.
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

    /// The button's name in its id: `image-toolbar-{name}-{line}`.
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
    /// The toolbar of line `ix` shows: the pointer is on its image or on
    /// it, or the image is selected as an object, and Escape did not close
    /// it.
    pub(super) fn toolbar_shown(&self, ix: usize, object: bool) -> bool {
        (object || self.toolbar.hovered() == Some(ix)) && self.toolbar.closed != Some(ix)
    }

    /// The pointer entered or left the image of line `ix` (`bar`: its
    /// toolbar); the toolbar Escape closed shows again once the pointer
    /// leaves, or comes when it was away. Closed under the pointer, the bar
    /// goes and the image under it takes the hover: that is no leave.
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

    /// Escape on a shown toolbar closes it; `false` without one.
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

    /// The toolbar of the image at `spot`: Image width, Copy image, Open
    /// image, Edit source and Delete image, those that apply. An image not
    /// measured (pending, unreadable) keeps Edit source and Delete image.
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
        // On the image's right edge, growing to the left, inside the
        // editor.
        let left = (spot.width - INSET - bar_width).max(spot.min_left);
        let ix = spot.ix;
        self.toolbar.drawn = Some(ix);
        // A tooltip would cover the width submenu open under its button.
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
            // Concentric with the buttons: their 4 px, the padding, the
            // border.
            .rounded(px(4.0 + PAD + 1.0))
            .bg(theme::overlay())
            .border_1()
            .border_color(theme::border())
            .shadow_md()
            // Escape is a key: the hover must not end with it.
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
            // The width submenu opens under its button.
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
            // The press stays here: the caret does not move, no text is
            // selected, the editor keeps the focus.
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

    /// The Image width submenu of the image whose node is `range`, open
    /// at `at`, under its toolbar button.
    fn open_width_menu(&mut self, range: &Range<usize>, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(image) = self.block_image(range) else {
            return;
        };
        let menu = ImageMenu {
            range: image.range.clone(),
            presets: image_presets_of(&image),
            actions: Vec::new(),
        };
        self.menu = Some(ContextMenu {
            position: at,
            items: width_items(&menu),
            open_submenu: None,
            in_submenu: false,
        });
        cx.notify();
    }
}
