//! What a tab shows besides an editor, and the window's notice.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};

use super::{Page, WindowView};
use crate::editor::theme;
use crate::palette::Intent;

pub fn render(page: Page, cx: &mut Context<WindowView>) -> AnyElement {
    match page {
        Page::Editor(editor) => editor.into_any_element(),
        Page::Loading { spinner } => centered(if spinner { "Loading…" } else { "" }),
        Page::Launcher => launcher(cx),
        Page::Settings(panel) => panel.into_any_element(),
        Page::Welcome => welcome(cx),
    }
}

/// The OS's name for a shortcut: ⌘N on macOS, Ctrl+N elsewhere.
pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The first screen without a workspace (`welcome/index.tsx:15-49`).
fn welcome(cx: &mut Context<WindowView>) -> AnyElement {
    let button = |id: &'static str, label: &'static str, primary: bool| {
        crate::a11y::button(selector(div().id(id), id), label)
            .px(px(16.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .text_size(px(13.0))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .map(|d| {
                if primary {
                    d.bg(theme::text())
                        .text_color(theme::background())
                        .hover(|s| s.opacity(0.9))
                } else {
                    d.border_color(theme::border())
                        .text_color(theme::muted())
                        .hover(|s| s.bg(theme::panel()))
                }
            })
            .child(label)
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(300.0))
                .px(px(24.0))
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .mb(px(24.0))
                        .text_center()
                        .text_size(px(13.0))
                        .line_height(px(21.0))
                        .text_color(theme::muted())
                        .child("Add a folder with your specs, docs, notes, or any markdown files."),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(button("welcome-add-folder", "Add Folder", true).on_click(
                            cx.listener(|this, _, window, cx| this.pick_workspace(window, cx)),
                        ))
                        .child(button("welcome-open-file", "Open File", false).on_click(
                            cx.listener(|this, _, window, cx| this.pick_file(window, cx)),
                        )),
                ),
        )
        .into_any_element()
}

/// A new tab (`new-tab-page.tsx:9-35`).
fn launcher(cx: &mut Context<WindowView>) -> AnyElement {
    let entry = |id: &'static str, label: &'static str, key: &str| {
        crate::a11y::button(selector(div().id(id), id), label)
            .px(px(6.0))
            .rounded(px(4.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(13.0))
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|s| s.text_color(theme::text()))
            .child(label)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::muted())
                    .child(shortcut(key)),
            )
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .child(
                    entry("launcher-create", "Create new note", "N").on_click(cx.listener(
                        |this, _, window, cx| this.open_palette(Intent::CreateFile, window, cx),
                    )),
                )
                .child(
                    entry("launcher-search", "Search", "O").on_click(cx.listener(
                        |this, _, window, cx| this.open_palette(Intent::Search, window, cx),
                    )),
                ),
        )
        .into_any_element()
}

/// Tests find an element by its id.
fn selector(element: gpui::Stateful<gpui::Div>, id: &'static str) -> gpui::Stateful<gpui::Div> {
    #[cfg(test)]
    let element = element.debug_selector(move || id.to_owned());
    #[cfg(not(test))]
    let _ = id;
    element
}

fn centered(text: &'static str) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .text_color(theme::muted())
        .child(text)
        .into_any_element()
}

/// The window's notice, announced as a status and kept until dismissed
/// (its close button, or a click on it).
pub fn notice(notice: Option<SharedString>, cx: &mut Context<WindowView>) -> Option<AnyElement> {
    let notice = notice?;
    Some(
        div()
            .absolute()
            .bottom(px(24.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("window-notice")
                    .role(gpui::Role::Status)
                    .aria_label(notice.clone())
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pl(px(12.0))
                    .pr(px(4.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(theme::overlay())
                    .border_1()
                    .border_color(theme::border())
                    .text_size(px(13.0))
                    .cursor_pointer()
                    .child(notice)
                    .child(
                        crate::a11y::button(div().id("window-notice-close"), "Dismiss")
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .text_color(theme::muted())
                            .hover(|s| s.bg(theme::panel()).text_color(theme::text()))
                            .child("×"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    })),
            )
            .into_any_element(),
    )
}
