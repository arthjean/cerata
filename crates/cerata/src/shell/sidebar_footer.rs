//! The sidebar's footer, on Paneflow's measures and behavior: the Search
//! field (a 36 px capsule, glyph 17, text 15) and the Settings button
//! (36 px square, radius 12, glyph 18), 6 apart, on the rows' 8 px margin.
//!
//! At rest the capsule shows its glyph only; hovering it shows the field,
//! focus or a query fills it. Typing filters the sidebar; Escape, or a
//! press elsewhere, hands the focus back to the page (Escape also clears).

use gpui::{
    AnyElement, Context, Entity, Focusable as _, InteractiveElement, IntoElement, MouseButton,
    ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _,
    px,
};

use super::WindowView;
use crate::editor::text_field::{FieldEvent, TextField};
use crate::editor::theme;
use crate::icons::{Icon, icon};

const HEIGHT: f32 = 36.0;

impl WindowView {
    pub(super) fn on_filter_event(
        &mut self,
        field: &Entity<TextField>,
        event: &FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FieldEvent::Changed => {
                let query = field.read(cx).text.clone();
                if let Some(sidebar) = &self.sidebar {
                    sidebar.update(cx, |sidebar, cx| sidebar.set_filter(&query, cx));
                }
                cx.notify();
            }
            FieldEvent::Escape => {
                self.clear_sidebar_filter(cx);
                self.focus_page(window, cx);
            }
            FieldEvent::Submit { .. } => {}
        }
    }

    fn clear_sidebar_filter(&mut self, cx: &mut Context<Self>) {
        self.sidebar_filter
            .update(cx, |field, cx| field.set_text("", cx));
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(cx, |sidebar, cx| sidebar.set_filter("", cx));
        }
        cx.notify();
    }

    /// The focus goes back to the active note, else to the window.
    pub(super) fn focus_page(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_doc() {
            Some(doc) => window.focus(&doc.editor.focus_handle(cx), cx),
            None => window.focus(&self.focus_handle, cx),
        }
    }
}

pub fn render(view: &WindowView, window: &Window, cx: &mut Context<WindowView>) -> AnyElement {
    let show_search = view
        .core
        .settings()
        .get(crate::sidebar::SHOW_SEARCH_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let settings = crate::a11y::button(div().id("sidebar-settings"), "Settings")
        .flex_none()
        .size(px(HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(12.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme::row_hover()))
        .child(icon(Icon::Settings, 18.0, theme::muted()))
        .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)));

    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(9.5))
        .map(|d| {
            if show_search {
                d.child(search(view, window, cx))
            } else {
                d.child(div().flex_1())
            }
        })
        .child(settings)
        .into_any_element()
}

fn search(view: &WindowView, window: &Window, cx: &mut Context<WindowView>) -> AnyElement {
    let field = view.sidebar_filter.read(cx);
    let focus = field.focus_handle.clone();
    let focused = focus.is_focused(window);
    let has_query = !field.text.is_empty();
    let filled = focused || has_query;
    let shown = view.sidebar_filter_hovered || filled;

    let clear = crate::a11y::button(div().id("sidebar-search-clear"), "Clear search")
        .flex_none()
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme::muted().opacity(0.2))
        .cursor_pointer()
        .child(icon(Icon::Close, 12.0, theme::text()))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, window, cx| {
            this.clear_sidebar_filter(cx);
            let focus = this.sidebar_filter.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
        }));

    div()
        .id("sidebar-search")
        .flex_1()
        .min_w_0()
        .h(px(HEIGHT))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded_full()
        .cursor_text()
        .when(filled, |d| d.bg(theme::row_active()))
        .hover(move |s| {
            s.bg(if filled {
                theme::row_active()
            } else {
                theme::row_hover()
            })
        })
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            this.sidebar_filter_hovered = *hovered;
            cx.notify();
        }))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
            if this.sidebar_filter.read(cx).focus_handle.is_focused(window) {
                this.focus_page(window, cx);
            }
        }))
        // Focused, the glyph takes the text color, as Paneflow's lights up.
        .child(icon(
            Icon::Search,
            17.0,
            if focused {
                theme::text()
            } else {
                theme::muted()
            },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(!shown, |d| d.opacity(0.0))
                .child(view.sidebar_filter.clone()),
        )
        .when(has_query, |d| d.child(clear))
        .into_any_element()
}
