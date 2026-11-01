//! The tab strip (`editor-tabs.tsx:104-225`): back and forward for the
//! active tab, the tabs (at most 180 px, close button on hover and on the
//! active tab, a red dot after a failed save), and a new-tab button.

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};

use super::{WindowView, file_name};
use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::menu::{HasMenu, Item, Menu};
use crate::tabs::Location;

pub const HEIGHT: f32 = 40.0;
const TAB_MAX_WIDTH: f32 = 180.0;

fn label(location: &Location) -> String {
    match location {
        Location::File { path } => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name(path)),
        Location::Launcher => "New Tab".into(),
        Location::Settings => "Settings".into(),
    }
}

pub fn nav_button(
    id: &'static str,
    glyph: Icon,
    name: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    crate::a11y::button(div().id(id), name)
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

    let mut strip = div()
        .id("tab-strip")
        .flex()
        .flex_1()
        .min_w_0()
        .gap(px(2.0))
        .overflow_x_scroll();
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
                .on_click(cx.listener(move |this, _, window, cx| this.activate_tab(ix, window, cx)))
                .group(group.clone())
                .flex()
                .items_center()
                .gap(px(6.0))
                .max_w(px(TAB_MAX_WIDTH))
                .h(px(30.0))
                .pl(px(14.0))
                .pr(px(6.0))
                .rounded(px(8.0))
                .text_size(px(13.0))
                .cursor_pointer()
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
            item = item.bg(theme::panel()).font_weight(FontWeight::MEDIUM);
        } else {
            item = item
                .text_color(theme::muted())
                .hover(|s| s.bg(theme::panel()));
        }
        if loading {
            item = item.opacity(0.6);
        }
        if let Some(error) = failed {
            item = item
                .aria_description(format!("Not saved: {error}"))
                .child(div().size(px(6.0)).rounded_full().bg(theme::danger()));
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
        // Inactive tabs show their close button on hover only; the keyboard
        // closes the active one (or Ctrl+W).
        let close = crate::a11y::button(div().id(("tab-close", tab.id)), format!("Close {name}"))
            .tab_stop(is_active)
            .flex()
            .items_center()
            .justify_center()
            .size(px(20.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(theme::border()))
            .child(icon(Icon::Close, 14.0, theme::text()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| this.close_tab(ix, window, cx)));
        item = item.child(if is_active {
            close
        } else {
            close.opacity(0.0).group_hover(group, |s| s.opacity(1.0))
        });
        strip = strip.child(item);
    }

    div()
        .id("tab-bar")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.0))
        .h(px(HEIGHT))
        .px(px(8.0))
        .border_b_1()
        .border_color(theme::border())
        .child(
            nav_button("tab-back", Icon::ArrowLeft, "Back", can_back)
                .on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx))),
        )
        .child(
            nav_button("tab-forward", Icon::ArrowRight, "Forward", can_forward)
                .on_click(cx.listener(|this, _, window, cx| this.go_forward(window, cx))),
        )
        .child(strip)
        .child(
            crate::a11y::button(div().id("tab-new"), "New tab")
                .flex()
                .items_center()
                .justify_center()
                .w(px(36.0))
                .h(px(28.0))
                .rounded(px(6.0))
                .hover(|s| s.bg(theme::panel()))
                .child(icon(Icon::Add, 16.0, theme::text()))
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
