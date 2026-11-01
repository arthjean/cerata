//! The compact window's header (`compact-file-layout.tsx:26-561`): back
//! and forward, and the file's title, which opens a picker of the global
//! recent files (30, the active one excluded, each removable). A press
//! outside it or Escape closes it.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext as _, Context, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};

use super::{WindowView, file_name, tab_bar};
use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::routing::OpenRequest;

/// Tallest the picker grows before it scrolls (`compact-file-layout.tsx:31`).
pub const PICKER_MAX_HEIGHT: f32 = 420.0;

impl WindowView {
    /// The header's label: the file's title, else "Choose file".
    pub fn compact_title(&self) -> String {
        match self.tabs.active_path() {
            Some(path) => cerata_core::fs::titles::extract_title(path)
                .unwrap_or_else(|| crate::sidebar::tree::stem(&file_name(path)).to_owned()),
            None => "Choose file".into(),
        }
    }

    /// The picker takes the focus while it is open, as a dialog does; it
    /// goes back to the note on close.
    pub fn toggle_recents_picker(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.recents_picker.is_some() {
            self.close_recents_picker(window, cx);
            return;
        }
        self.recents_picker = Some(self.global_recents(self.tabs.active_path(), cx));
        window.focus(&self.picker_focus, cx);
        cx.notify();
    }

    pub fn close_recents_picker(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.recents_picker.take().is_some() {
            self.active_editor = None;
            self.sync_active(window, cx);
            cx.notify();
        }
    }

    /// "Remove from recents": the entry leaves the global list.
    pub fn remove_recent(&mut self, path: &Path, cx: &mut Context<Self>) {
        let core = crate::app::core(cx);
        if let Err(error) = core.recents.remove_file(&path.to_string_lossy()) {
            self.show_notice(format!("Failed to remove from recents: {error}"), cx);
            return;
        }
        if let Some(entries) = &mut self.recents_picker {
            entries.retain(|entry| entry.path != path);
        }
        cx.notify();
    }

    pub fn choose_recent(
        &mut self,
        path: PathBuf,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.recents_picker = None;
        self.open_elsewhere(OpenRequest::File(path), window, cx);
    }
}

pub fn render(view: &WindowView, cx: &mut Context<WindowView>) -> AnyElement {
    let active = view.tabs.active();
    let open = view.recents_picker.is_some();
    let back = tab_bar::nav_button(
        "compact-back",
        Icon::ArrowLeft,
        "Back",
        active.can_go_back(),
    )
    .on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx)));
    let forward = tab_bar::nav_button(
        "compact-forward",
        Icon::ArrowRight,
        "Forward",
        active.can_go_forward(),
    )
    .on_click(cx.listener(|this, _, window, cx| this.go_forward(window, cx)));
    let title = view.compact_title();
    let trigger = crate::a11y::button(div().id("compact-title"), format!("{title}, recent files"))
        .aria_expanded(open)
        .occlude()
        .max_w(px(240.0))
        .h(px(28.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(8.0))
        .text_size(px(13.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme::panel()))
        .when(open, |d| d.bg(theme::panel()))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(title),
        )
        .child(icon(
            if open {
                Icon::ChevronUp
            } else {
                Icon::ChevronDown
            },
            12.0,
            theme::muted(),
        ))
        .on_click(cx.listener(|this, _, window, cx| this.toggle_recents_picker(window, cx)));

    let header = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .px(px(8.0))
        .flex()
        .items_center()
        .child(div().flex().gap(px(2.0)).child(back).child(forward))
        .child(div().flex_1().flex().justify_center().child(trigger))
        .child(div().w(px(58.0)));
    header.into_any_element()
}

/// The open picker, over the whole window: a press outside the card
/// (the title included) closes it.
pub fn overlay(view: &WindowView, cx: &mut Context<WindowView>) -> Option<AnyElement> {
    let entries = view.recents_picker.as_ref()?;
    Some(picker(entries, &view.picker_focus, cx))
}

fn picker(
    entries: &[crate::palette::RecentFile],
    focus: &gpui::FocusHandle,
    cx: &mut Context<WindowView>,
) -> AnyElement {
    let mut list = div()
        .id("compact-recents")
        .max_h(px(PICKER_MAX_HEIGHT))
        .overflow_y_scroll()
        .px(px(8.0))
        .py(px(12.0))
        .flex()
        .flex_col()
        .gap(px(2.0));
    if entries.is_empty() {
        list = list.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(13.0))
                .text_color(theme::muted())
                .child("No other recent files."),
        );
    }
    for (ix, entry) in entries.iter().enumerate() {
        let open = entry.path.clone();
        let remove = entry.path.clone();
        let folder = entry.path.parent().map(short_folder).unwrap_or_default();
        list =
            list.child(
                crate::a11y::button(
                    div().id(SharedString::from(format!("compact-recent-{ix}"))),
                    format!("{}, {}", entry.label, entry.path.display()),
                )
                .group("recent")
                .px(px(8.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme::panel()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(entry.label.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::muted())
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(folder),
                        ),
                )
                .child(
                    crate::a11y::button(
                        div().id(SharedString::from(format!("compact-recent-remove-{ix}"))),
                        format!("Remove {} from recents", entry.label),
                    )
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .group("compact-recent-remove")
                    .hover(|s| s.bg(theme::border()))
                    .tooltip(|_, cx| gpui::AnyView::from(cx.new(|_| Tip("Remove from recents"))))
                    .child(
                        icon(Icon::Close, 14.0, theme::muted())
                            .group_hover("compact-recent-remove", |s| s.text_color(theme::text())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_recent(&remove, cx);
                    })),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.choose_recent(open.clone(), window, cx)
                })),
            );
    }
    div()
        .id("compact-picker")
        .absolute()
        .inset_0()
        .occlude()
        .pt(px(super::title_bar::HEIGHT - 4.0))
        .flex()
        .justify_center()
        .items_start()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_recents_picker(window, cx)),
        )
        .child(
            div()
                .id("compact-picker-card")
                .track_focus(focus)
                .key_context("RecentsPicker")
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_recents_picker(window, cx);
                    }
                }))
                .occlude()
                .w(px(360.0))
                .rounded(px(10.0))
                .bg(theme::overlay())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(list),
        )
        .into_any_element()
}

/// A recent file's folder as the picker shows it: the home folder as `~`
/// and, past 44 characters, only its last components after `…`, so files
/// with the same title stay apart (their common prefix is what gets cut).
pub fn short_folder(folder: &Path) -> String {
    let text = folder.to_string_lossy();
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let text = match home.map(PathBuf::from) {
        Some(home) if folder.starts_with(&home) && !home.as_os_str().is_empty() => {
            let rest = folder.strip_prefix(&home).unwrap_or(folder);
            let sep = std::path::MAIN_SEPARATOR;
            if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~{sep}{}", rest.display())
            }
        }
        _ => text.into_owned(),
    };
    const MAX: usize = 44;
    if text.chars().count() <= MAX {
        return text;
    }
    let tail: Vec<&str> = text.rsplit(['/', '\\']).collect();
    let sep = std::path::MAIN_SEPARATOR;
    let mut kept = String::new();
    for part in tail {
        let candidate = if kept.is_empty() {
            part.to_owned()
        } else {
            format!("{part}{sep}{kept}")
        };
        if candidate.chars().count() + 2 > MAX && !kept.is_empty() {
            break;
        }
        kept = candidate;
    }
    format!("…{sep}{kept}")
}

struct Tip(&'static str);

impl gpui::Render for Tip {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(theme::text())
            .text_color(theme::background())
            .text_size(px(11.0))
            .child(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_folder_keeps_its_last_components() {
        let sep = std::path::MAIN_SEPARATOR;
        let long: PathBuf = [
            "base",
            "a-rather-long-folder",
            "another-long-folder",
            "notes",
            "fx22c",
        ]
        .iter()
        .collect();
        let shown = short_folder(&long);
        assert!(shown.starts_with('…'), "{shown}");
        assert!(shown.ends_with(&format!("notes{sep}fx22c")), "{shown}");
        assert!(shown.chars().count() <= 44, "{shown}");
        let short = PathBuf::from("notes");
        assert_eq!(short_folder(&short), "notes");
    }
}
