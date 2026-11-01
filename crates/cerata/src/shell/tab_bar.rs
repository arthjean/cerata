//! The tab strip (`editor-tabs.tsx:104-225`): back and forward for the
//! active tab, the tabs, and a new-tab button. It sits in the title bar, so
//! each control occludes the drag area.
//!
//! The tabs take Paneflow's pane tabs at comfortable density (`pane.rs`,
//! `unified_tab_chip`) but keep Cerata's width (at most 180 px): 32 px
//! pills with a 12 px medium label, the active one raised by a border and a
//! shadow, a round close button on hover only, a red dot after a failed
//! save, and fades over the strip's hidden ends.

use gpui::{
    BoxShadow, Context, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, ScrollHandle, StatefulInteractiveElement, Styled, black, canvas, div, fill,
    linear_color_stop, linear_gradient, point, prelude::FluentBuilder as _, px, size,
};

use super::{WindowView, file_name};
use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::menu::{HasMenu, Item, Menu};
use crate::tabs::Location;

const TAB_MAX_WIDTH: f32 = 180.0;
const TAB_HEIGHT: f32 = 32.0;
const TAB_GAP: f32 = 3.0;
const TAB_CLOSE_SIZE: f32 = 16.0;
const TAB_CLOSE_GLYPH: f32 = 12.0;
const TAB_FADE_WIDTH: f32 = 28.0;
const NEW_TAB_SIZE: f32 = 22.0;
const NEW_TAB_RADIUS: f32 = 14.0;

fn label(location: &Location) -> String {
    match location {
        Location::File { path } => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name(path)),
        Location::Launcher => "New Tab".into(),
    }
}

/// The active chip's fill, border and shadow, and the hover fill of the
/// others. Light themes raise the active chip on the editor's surface;
/// dark ones fill it with the text at 16 %.
struct ChipColors {
    active: Hsla,
    active_border: Hsla,
    active_shadow: BoxShadow,
    hover: Hsla,
}

fn chip_colors() -> ChipColors {
    if theme::palette().dark {
        ChipColors {
            active: theme::row_active(),
            active_border: theme::text().opacity(0.12),
            active_shadow: BoxShadow::new(px(0.0), px(1.0), black().opacity(0.24))
                .blur_radius(px(2.0)),
            hover: theme::row_hover(),
        }
    } else {
        ChipColors {
            active: theme::background(),
            active_border: theme::accent().opacity(0.14),
            active_shadow: BoxShadow::new(px(0.0), px(1.0), black().opacity(0.10))
                .blur_radius(px(3.0)),
            hover: theme::text().opacity(0.05),
        }
    }
}

/// Gradients over the strip's ends while tabs hide past them.
fn strip_fades(scroll: ScrollHandle, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let hidden_right = scroll.max_offset().x + scroll.offset().x;
            let hidden_left = -scroll.offset().x;
            let width = px(TAB_FADE_WIDTH).min(bounds.size.width);
            if hidden_right > px(1.0) {
                window.paint_quad(fill(
                    gpui::Bounds {
                        origin: point(bounds.right() - width, bounds.top()),
                        size: size(width, bounds.size.height),
                    },
                    linear_gradient(
                        90.0,
                        linear_color_stop(color.opacity(0.0), 0.0),
                        linear_color_stop(color, 1.0),
                    ),
                ));
            }
            if hidden_left > px(1.0) {
                window.paint_quad(fill(
                    gpui::Bounds {
                        origin: bounds.origin,
                        size: size(width, bounds.size.height),
                    },
                    linear_gradient(
                        90.0,
                        linear_color_stop(color, 0.0),
                        linear_color_stop(color.opacity(0.0), 1.0),
                    ),
                ));
            }
        },
    )
    .absolute()
    .inset_0()
}

pub fn nav_button(
    id: &'static str,
    glyph: Icon,
    name: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    crate::a11y::button(div().id(id), name)
        .occlude()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.0))
        .h(px(28.0))
        .rounded(px(6.0))
        .when(enabled, |d| {
            d.cursor_pointer().hover(|s| s.bg(theme::panel()))
        })
        .when(!enabled, |d| d.opacity(0.3))
        .child(icon(glyph, 16.0, theme::text()))
}

pub fn render(view: &WindowView, cx: &mut Context<WindowView>) -> impl IntoElement {
    let active = view.tabs.active();
    let can_back = active.can_go_back();
    let can_forward = active.can_go_forward();
    let active_ix = view.tabs.active_index();
    let colors = chip_colors();

    let mut strip = div()
        .id("tab-strip")
        .flex()
        .items_center()
        .min_w_0()
        .h_full()
        .gap(px(TAB_GAP))
        .overflow_x_scroll()
        .track_scroll(&view.tab_scroll);
    for (ix, tab) in view.tabs.tabs().iter().enumerate() {
        let is_active = ix == active_ix;
        let failed = tab
            .location
            .path()
            .and_then(|path| view.docs.get(path))
            .and_then(|doc| doc.save_error.clone());
        let loading = tab
            .location
            .path()
            .and_then(|path| view.docs.get(path))
            .is_some_and(|doc| doc.loading_since.is_some());
        let group = format!("tab-{}", tab.id);
        let name = label(&tab.location);
        let mut item =
            crate::a11y::control(div().id(("tab", tab.id)), gpui::Role::Tab, name.clone())
                .aria_selected(is_active)
                .occlude()
                .on_click(cx.listener(move |this, _, window, cx| this.activate_tab(ix, window, cx)))
                .group(group.clone())
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.0))
                .max_w(px(TAB_MAX_WIDTH))
                .h(px(TAB_HEIGHT))
                .pl(px(12.0))
                .pr(px(8.0))
                .rounded_full()
                .border_1()
                .border_color(theme::transparent())
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| this.activate_tab(ix, window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.close_tab(ix, window, cx)),
                )
                .when(tab.location.path().is_some(), |item| {
                    item.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_tab_menu(ix, event.position, cx);
                        }),
                    )
                });
        if is_active {
            item = item
                .bg(colors.active)
                .border_color(colors.active_border)
                .shadow(vec![colors.active_shadow.clone()])
                .text_color(theme::text());
        } else {
            let hover = colors.hover;
            item = item.text_color(theme::muted()).hover(move |s| s.bg(hover));
        }
        if loading {
            item = item.opacity(0.6);
        }
        if let Some(error) = failed {
            item = item.aria_description(format!("Not saved: {error}")).child(
                div()
                    .flex_none()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(theme::danger()),
            );
        }
        item = item.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name.clone()),
        );
        // The close button shows on hover only; the keyboard reaches the
        // active tab's (or Ctrl+W closes it).
        let close = crate::a11y::button(div().id(("tab-close", tab.id)), format!("Close {name}"))
            .tab_stop(is_active)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(TAB_CLOSE_SIZE))
            .rounded_full()
            .hover(|s| s.bg(theme::text().opacity(0.12)))
            .opacity(0.0)
            .group_hover(group, |s| s.opacity(1.0))
            .child(icon(Icon::Close, TAB_CLOSE_GLYPH, theme::muted()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| this.close_tab(ix, window, cx)));
        strip = strip.child(item.child(close));
    }

    let new_tab_hover = colors.hover;
    div()
        .id("tab-bar")
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        // On Windows the title bar's top pixel sits under the window border,
        // so the chips center on the 38 px between it and the row's border.
        .when(cfg!(target_os = "windows"), |bar| bar.pt(px(1.0)))
        .items_center()
        .gap(px(4.0))
        .px(px(8.0))
        .child(
            nav_button("tab-back", Icon::ArrowLeft, "Back", can_back)
                .on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx))),
        )
        .child(
            nav_button("tab-forward", Icon::ArrowRight, "Forward", can_forward)
                .on_click(cx.listener(|this, _, window, cx| this.go_forward(window, cx))),
        )
        .child(
            div()
                .relative()
                .flex()
                .min_w_0()
                .h_full()
                .child(strip)
                .child(strip_fades(view.tab_scroll.clone(), theme::background())),
        )
        .child(
            crate::a11y::button(div().id("tab-new"), "New tab")
                .occlude()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(NEW_TAB_SIZE))
                .rounded(px(NEW_TAB_RADIUS))
                .hover(move |s| s.bg(new_tab_hover))
                .child(icon(Icon::Add, 14.0, theme::muted()))
                .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
        )
}

impl WindowView {
    /// A file tab's menu (`editor-context-menu.ts:162-181`).
    pub fn open_tab_menu(
        &mut self,
        ix: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .tabs
            .tabs()
            .get(ix)
            .and_then(|tab| tab.location.path())
            .map(|p| p.to_path_buf())
        else {
            return;
        };
        self.tab_menu = Some(Menu::new(position, tab_menu_items(ix, path)));
        cx.notify();
    }

    /// Expands the file's folders in the sidebar (showing it if hidden)
    /// and scrolls to its row.
    pub fn reveal_in_sidebar(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        if !self.sidebar_visible {
            self.toggle_sidebar(cx);
        }
        sidebar.update(cx, |sidebar, cx| sidebar.reveal_in_sidebar(path, cx));
    }
}

pub fn tab_menu_items(ix: usize, path: std::path::PathBuf) -> Vec<Item<WindowView>> {
    let reveal = path.clone();
    let copy = path;
    vec![
        Item::action("Close", move |this: &mut WindowView, window, cx| {
            this.close_tab(ix, window, cx)
        }),
        Item::action("Close others", move |this: &mut WindowView, window, cx| {
            this.close_other_tabs(ix, window, cx)
        }),
        Item::action("Close all", |this: &mut WindowView, window, cx| {
            this.close_all_tabs(window, cx)
        }),
        Item::Separator,
        Item::action("Reveal in sidebar", move |this: &mut WindowView, _, cx| {
            this.reveal_in_sidebar(&reveal, cx)
        }),
        Item::action("Copy path", move |_: &mut WindowView, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                copy.to_string_lossy().into_owned(),
            ))
        }),
    ]
}

impl HasMenu for WindowView {
    fn menu(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.tab_menu
    }
}
