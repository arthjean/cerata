//! What a tab shows besides an editor, and the window's notice.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};

use super::{Page, WindowView};
use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::palette::Intent;

pub fn render(page: Page, view: &WindowView, cx: &mut Context<WindowView>) -> AnyElement {
    match page {
        Page::Editor(editor) => editor.into_any_element(),
        Page::Loading { spinner } => centered(if spinner { "Loading…" } else { "" }),
        Page::Launcher => launcher(cx),
        Page::Welcome => welcome(view, cx),
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

/// A welcome screen button: primary (filled), or secondary (outlined).
/// An inactive one keeps its Tab stop, says why in its description, and
/// does nothing.
fn welcome_button(
    id: impl Into<gpui::ElementId>,
    test_id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    crate::a11y::button(selector(div().id(id), test_id), label.clone())
        .flex_shrink_0()
        .px(px(12.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|d| {
            if primary {
                // The shared ring is a text-colored border, invisible on a
                // text-colored fill: keyboard focus turns the fill into
                // that ring instead.
                d.bg(theme::text())
                    .text_color(theme::background())
                    .hover(|s| s.opacity(0.9))
                    .focus_visible(|s| {
                        s.bg(theme::background())
                            .text_color(theme::text())
                            .border_color(theme::text())
                    })
            } else {
                d.border_color(theme::border())
                    .text_color(theme::text())
                    .hover(|s| s.bg(theme::panel()))
            }
        })
        .child(label)
}

fn field_label(text: &'static str) -> gpui::Div {
    div()
        .mb(px(4.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::muted())
        .child(text)
}

fn field_note(text: impl Into<SharedString>, danger: bool) -> gpui::Div {
    let text = text.into();
    div()
        .mt(px(6.0))
        .text_size(px(12.0))
        .line_height(px(17.0))
        .text_color(if danger {
            theme::danger()
        } else {
            theme::muted()
        })
        .child(text)
}

/// The first screen without a library (US-078): "Create Library" (name,
/// location, Create), "Open Folder…" and "Open File…", then the
/// registered libraries.
fn welcome(view: &WindowView, cx: &mut Context<WindowView>) -> AnyElement {
    let name_problem = view.welcome_name_problem(cx);
    let can_create = view.can_create_library(cx);
    let location = view.welcome.location.clone();
    let synced_by = view.welcome.synced_by.clone();
    let problem = view.welcome.problem.clone();
    let libraries: Vec<cerata_core::library::Library> = view.listed_libraries().to_vec();
    let name_field = view.welcome.name.clone();

    let location_text = location
        .as_deref()
        .map(cerata_core::paths::display_path)
        .unwrap_or_else(|| "Choose a location".into());
    let create = welcome_button("welcome-create", "welcome-create", "Create", true)
        .when(!can_create, |d| {
            d.opacity(0.4).cursor_default().aria_description(
                name_problem
                    .clone()
                    .unwrap_or_else(|| "Choose a location".into()),
            )
        })
        .on_click(cx.listener(|this, _, window, cx| this.create_library(window, cx)));

    let mut form = div()
        .flex()
        .flex_col()
        .child(
            div()
                .mb(px(12.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Create Library"),
        )
        .child(field_label("Name"))
        .child(name_field)
        .children(name_problem.map(|problem| field_note(problem, true)))
        .child(div().h(px(14.0)))
        .child(field_label("Location"))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.0))
                        .child(location_text),
                )
                .child(
                    welcome_button("welcome-change", "welcome-change", "Change\u{2026}", false)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.pick_library_location(window, cx)
                        })),
                ),
        )
        .children(synced_by.map(|provider| {
            field_note(
                format!(
                    "This folder is synced by {provider}. Keep it always available offline, \
                     and don't sync it with a second service."
                ),
                false,
            )
        }));
    if let Some(problem) = &problem {
        let exists = matches!(problem, super::libraries::WelcomeProblem::Exists { .. });
        form = form.child(
            div()
                .mt(px(14.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("welcome-problem")
                        .role(gpui::Role::Status)
                        .aria_label(problem.message())
                        .flex_1()
                        .text_size(px(12.0))
                        .line_height(px(17.0))
                        .text_color(if exists {
                            theme::text()
                        } else {
                            theme::danger()
                        })
                        .child(problem.message()),
                )
                .when(exists, |d| {
                    d.child(
                        welcome_button("welcome-adopt", "welcome-adopt", "Open", false).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.adopt_existing_folder(window, cx)
                            }),
                        ),
                    )
                }),
        );
    }
    form = form.child(div().mt(px(16.0)).flex().child(create));

    let others = div()
        .flex()
        .gap(px(8.0))
        .child(
            welcome_button(
                "welcome-add-folder",
                "welcome-add-folder",
                "Open Folder\u{2026}",
                false,
            )
            .on_click(cx.listener(|this, _, window, cx| this.pick_workspace(window, cx))),
        )
        .child(
            welcome_button(
                "welcome-open-file",
                "welcome-open-file",
                "Open File\u{2026}",
                false,
            )
            .on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx))),
        );

    let list = (!libraries.is_empty()).then(|| {
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(field_label("Libraries"));
        for (ix, library) in libraries.into_iter().enumerate() {
            list = list.child(library_row(ix, library, cx));
        }
        list
    });

    // The column is centered in a box at least as tall as the window: when
    // it is taller, it starts at the top and scrolls, instead of being
    // centered with its heading above the scroll origin.
    let column = div()
        .w(px(360.0))
        .px(px(16.0))
        .py(px(48.0))
        .flex()
        .flex_col()
        .gap(px(28.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            selector(div().id("welcome-title"), "welcome-title")
                                .role(gpui::Role::Heading)
                                .text_size(px(20.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Welcome to Cerata"),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .line_height(px(20.0))
                                .text_color(theme::muted())
                                .child("Your notes live in a library: a folder of Markdown files on this computer."),
                        ),
                )
                .child(form)
                .child(others)
                .children(list);
    div()
        .id("welcome")
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .min_h_full()
                .flex()
                .justify_center()
                .items_center()
                .child(column),
        )
        .into_any_element()
}

/// A registered library: opens on click; one that is gone says
/// "Not found" and offers "Locate…" and "Remove from List".
fn library_row(
    ix: usize,
    library: cerata_core::library::Library,
    cx: &mut Context<WindowView>,
) -> AnyElement {
    let path = cerata_core::paths::display_path(&library.path);
    let text = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .text_size(px(13.0))
                .child(div().min_w_0().child(library.name.clone()))
                .when(library.is_default, |d| {
                    d.child(div().text_color(theme::muted()).child("Default"))
                }),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme::muted())
                .child(path.clone()),
        );
    if library.exists() {
        let id = library.id.clone();
        return crate::a11y::button(
            div().id(("welcome-library", ix)),
            format!("{}, {path}", library.name),
        )
        .px(px(8.0))
        .py(px(5.0))
        .mx(px(-8.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .hover(|s| s.bg(theme::panel()))
        .child(text)
        .on_click(cx.listener(move |this, _, window, cx| this.open_library(&id, window, cx)))
        .into_any_element();
    }
    let (locate, remove) = (library.id.clone(), library.id.clone());
    // The actions go under the text, which keeps the full width: beside it
    // they left a long name or path about 100 px.
    let actions = div()
        .flex()
        .gap(px(8.0))
        .child(
            welcome_button(
                ("welcome-locate", ix),
                "welcome-locate",
                "Locate\u{2026}",
                false,
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.locate_library(&locate, window, cx)),
            ),
        )
        .child(
            welcome_button(
                ("welcome-remove", ix),
                "welcome-remove",
                "Remove from List",
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.remove_library(&remove, cx))),
        );
    div()
        .py(px(5.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div().w_full().child(
                text.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::danger())
                        .child("Not found"),
                ),
            ),
        )
        .child(actions)
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
/// (its close button, or a click on it), with its action buttons if any.
pub fn notice(
    notice: Option<SharedString>,
    actions: Vec<super::libraries::NoticeAction>,
    cx: &mut Context<WindowView>,
) -> Option<AnyElement> {
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
                    .children(actions.into_iter().enumerate().map(|(ix, action)| {
                        crate::a11y::button(
                            selector(
                                div().id(("window-notice-action", ix)),
                                "window-notice-action",
                            ),
                            action.label(),
                        )
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .border_color(theme::border())
                        .font_weight(FontWeight::MEDIUM)
                        .hover(|s| s.bg(theme::panel()))
                        .child(action.label())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_notice_action(action.clone(), window, cx);
                            },
                        ))
                    }))
                    .child(
                        crate::a11y::button(div().id("window-notice-close"), "Dismiss")
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .group("window-notice-close")
                            .hover(|s| s.bg(theme::panel()))
                            .child(
                                icon(Icon::Close, 14.0, theme::muted())
                                    .group_hover("window-notice-close", |s| {
                                        s.text_color(theme::text())
                                    }),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    })),
            )
            .into_any_element(),
    )
}
