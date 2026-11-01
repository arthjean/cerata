//! The banner of a document that changed or vanished on disk while it had
//! unsaved changes, and the read-only side-by-side comparison. Writer had
//! neither: it reloaded over the unsaved text (`use-file-watcher.ts:31-50`).

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};

use super::WindowView;
use crate::documents::Conflict;
use crate::editor::theme;

pub const MODIFIED: &str = "This file changed on disk";
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

pub fn render(conflict: &Conflict, comparing: bool, cx: &mut Context<WindowView>) -> AnyElement {
    let bar = div()
        .id("conflict-banner")
        .flex()
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
                    .font_weight(FontWeight::MEDIUM)
                    .child(MODIFIED),
            )
            .child(
                button("conflict-keep", "Keep My Version")
                    .on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx))),
            )
            .child(
                button("conflict-reload", "Reload")
                    .on_click(cx.listener(|this, _, _, cx| this.reload_from_disk(cx))),
            )
            .child(
                button(
                    "conflict-compare",
                    if comparing {
                        "Hide Comparison"
                    } else {
                        "Compare"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_compare(cx))),
            )
            .into_any_element(),
        Conflict::Deleted => bar
            .child(
                div()
                    .flex_1()
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

fn column(id: &'static str, heading: &'static str, text: &str) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(16.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(theme::muted())
                .border_b_1()
                .border_color(theme::border())
                .child(heading),
        )
        .child(
            div()
                .id(id)
                .flex_1()
                .overflow_y_scroll()
                .px(px(16.0))
                .py(px(8.0))
                .font_family("monospace")
                .text_size(px(13.0))
                .children(text.lines().map(|line| {
                    div()
                        .min_h(px(18.0))
                        .whitespace_normal()
                        .child(line.to_owned())
                })),
        )
        .into_any_element()
}

/// The disk's text and the editor's, side by side and read-only.
pub fn compare(disk: &str, mine: &str) -> AnyElement {
    div()
        .id("conflict-compare-view")
        .absolute()
        .top(px(41.0))
        .bottom_0()
        .left_0()
        .right_0()
        .flex()
        .bg(theme::background())
        .child(column("compare-disk", "On disk", disk))
        .child(div().w(px(1.0)).h_full().bg(theme::border()))
        .child(column("compare-mine", "In the editor", mine))
        .into_any_element()
}
