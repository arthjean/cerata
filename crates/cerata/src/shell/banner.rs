use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};

use super::WindowView;
use crate::documents::Conflict;
use crate::editor::theme;

pub const MODIFIED: &str = "This note changed outside Cerata.";
pub const DELETED: &str = "This file was deleted from disk";

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    crate::a11y::button(div().id(id), label)
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .border_color(theme::border())
        .bg(theme::background())
        .text_size(px(13.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme::panel()))
        .child(label)
}

pub fn render(conflict: &Conflict, cx: &mut Context<WindowView>) -> AnyElement {
    let bar = div()
        .id("conflict-banner")
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(8.0))
        .bg(theme::panel())
        .border_b_1()
        .border_color(theme::border())
        .text_size(px(13.0));
    match conflict {
        Conflict::Modified { .. } => bar
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(MODIFIED),
            )
            .child(
                button("conflict-review", "Review Changes")
                    .on_click(cx.listener(|this, _, window, cx| this.review_changes(window, cx))),
            )
            .child(
                button("conflict-keep", "Keep My Version")
                    .on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx))),
            )
            .child(
                button("conflict-reload", "Reload")
                    .on_click(cx.listener(|this, _, _, cx| this.reload_from_disk(cx))),
            )
            .into_any_element(),
        Conflict::Deleted => bar
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(DELETED),
            )
            .child(
                button("conflict-recreate", "Recreate")
                    .on_click(cx.listener(|this, _, _, cx| this.recreate(cx))),
            )
            .child(
                button("conflict-close", "Close")
                    .on_click(cx.listener(|this, _, window, cx| this.discard_deleted(window, cx))),
            )
            .into_any_element(),
    }
}
