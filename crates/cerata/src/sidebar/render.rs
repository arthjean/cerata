//! Sidebar rows and sections (`file-tree-node.tsx:31-173`,
//! `sidebar-section.tsx:17-93`): 32 px rows, 10 px left padding at depth 0
//! then `depth × 12 + 6`, 20 px section headers with a chevron.

use cerata_core::fs::DirEntry;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Render, Role,
    StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder as _, px,
    uniform_list,
};

use super::tree::{Listing, label};
use super::{ClearSelection, ROW_HEIGHT, Section, Sidebar, SidebarEvent};
use crate::editor::theme;
use crate::icons::{Icon, icon};

fn indent(depth: usize) -> f32 {
    if depth == 0 {
        10.0
    } else {
        depth as f32 * 12.0 + 6.0
    }
}

pub struct RowState {
    pub depth: usize,
    pub expanded: bool,
    pub selected: bool,
    pub active: bool,
    pub error: Option<String>,
    pub drop_target: bool,
    pub dragged: bool,
    /// Tree rows can be dragged; Pinned and Recents rows cannot.
    pub draggable: bool,
}

/// One row: icon slot, label (or the rename field), and the folder's read
/// error if any.
pub fn row(
    id: impl Into<gpui::ElementId>,
    entry: &DirEntry,
    state: RowState,
    label_text: String,
    sidebar: &Sidebar,
    cx: &mut Context<Sidebar>,
) -> gpui::Stateful<gpui::Div> {
    let label_name = label_text.clone();
    let row_icon = if entry.is_dir {
        if state.expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        }
    } else {
        Icon::File
    };
    let renaming = sidebar
        .renaming
        .as_ref()
        .filter(|renaming| renaming.path == entry.path);
    let label_el: AnyElement = match renaming {
        Some(renaming) => div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(renaming.field.clone())
            .children(renaming.error.clone().map(|error| {
                div()
                    .text_size(px(11.0))
                    .text_color(theme::danger())
                    .child(error)
            }))
            .into_any_element(),
        None => div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when(!(state.active || state.selected), |d| d.opacity(0.75))
            .child(label_text)
            .into_any_element(),
    };
    let pressed = entry.path.clone();
    let clicked = entry.clone();
    let hovered = entry.clone();
    let menu_entry = entry.clone();
    let key_entry = entry.clone();
    let draggable = state.draggable;
    let bounds = sidebar.row_bounds.clone();
    let measured = entry.path.clone();
    let name = if entry.is_dir {
        format!("{label_name}, folder")
    } else {
        label_name.clone()
    };
    crate::a11y::control(div().id(id), Role::TreeItem, name)
        .aria_selected(state.selected || state.active)
        .when(entry.is_dir, |d| d.aria_expanded(state.expanded))
        .relative()
        .child(
            canvas(
                move |b, _, _| {
                    bounds.borrow_mut().insert(measured.clone(), b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if this.renaming.is_some() {
                return;
            }
            let keystroke = &event.keystroke;
            let menu_key =
                keystroke.key == "menu" || (keystroke.key == "f10" && keystroke.modifiers.shift);
            if menu_key {
                cx.stop_propagation();
                let at = this
                    .row_bounds
                    .borrow()
                    .get(&key_entry.path)
                    .map(|b| b.bottom_left() + gpui::point(px(16.0), px(0.0)))
                    .unwrap_or_default();
                this.open_row_menu(&key_entry, at, cx);
            } else if keystroke.key == "f2" && keystroke.modifiers.number_of_modifiers() == 0 {
                cx.stop_propagation();
                this.start_rename(&key_entry.path, window, cx);
            } else if keystroke.key == "delete" && keystroke.modifiers.number_of_modifiers() == 0 {
                cx.stop_propagation();
                let paths = if this.selection.len() >= 2 && this.selection.contains(&key_entry.path)
                {
                    this.selection.paths.clone()
                } else {
                    vec![key_entry.path.clone()]
                };
                cx.emit(SidebarEvent::Delete(paths));
            }
        }))
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(ROW_HEIGHT))
        .pl(px(indent(state.depth)))
        .pr(px(8.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .line_height(px(15.0))
        .cursor_pointer()
        .when(state.selected, |d| d.bg(theme::border()))
        .when(!state.selected && state.active, |d| d.bg(theme::panel()))
        .when(state.drop_target, |d| d.bg(theme::selection()))
        .when(!state.selected && !state.drop_target, |d| {
            d.hover(|s| s.bg(theme::panel()))
        })
        .when(state.dragged, |d| d.opacity(0.4))
        .child(
            div()
                .flex_none()
                .w(px(20.0))
                .flex()
                .justify_center()
                .child(icon(row_icon, 14.0, theme::muted())),
        )
        .child(label_el)
        .when_some(state.error, |d, error| {
            d.child(
                div()
                    .flex_none()
                    .max_w(px(120.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(11.0))
                    .text_color(theme::danger())
                    .child(format!("Error: {error}")),
            )
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.press_row(
                    &pressed,
                    event.modifiers.shift,
                    event.modifiers.secondary(cx),
                    cx,
                );
                if draggable && !event.modifiers.shift && !event.modifiers.secondary(cx) {
                    this.begin_drag(&pressed, event.position);
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_row_menu(&menu_entry, event.position, cx);
            }),
        )
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            if this.drag.is_some() {
                cx.stop_propagation();
                this.drag_moved(event.position, Some(hovered.clone()), cx);
            }
        }))
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            let modifiers = event.modifiers();
            if !modifiers.shift && !modifiers.secondary(cx) {
                this.click_row(&clicked, cx);
            }
        }))
}

fn header(
    section: Section,
    title: &'static str,
    collapsed: bool,
    cx: &mut Context<Sidebar>,
) -> AnyElement {
    crate::a11y::button(div().id(("sidebar-section", section as usize)), title)
        .aria_expanded(!collapsed)
        .flex()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .mt(px(8.0))
        .px(px(10.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|s| s.text_color(theme::text()))
        .group("sidebar-section")
        .child(
            icon(
                if collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                },
                12.0,
                theme::muted(),
            )
            .group_hover("sidebar-section", |s| s.text_color(theme::text())),
        )
        .child(title)
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
        .into_any_element()
}

fn show_more(section: Section, cx: &mut Context<Sidebar>) -> AnyElement {
    let name = match section {
        Section::Pinned => "Show more pinned files",
        _ => "Show more recent files",
    };
    crate::a11y::button(div().id(("sidebar-more", section as usize)), name)
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(ROW_HEIGHT))
        .pl(px(10.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|s| s.bg(theme::panel()))
        .child(div().w(px(20.0)).flex().justify_center().child(icon(
            Icon::More,
            14.0,
            theme::muted(),
        )))
        .child("Show More")
        .on_click(cx.listener(move |this, _, _, cx| this.show_more(section, cx)))
        .into_any_element()
}

fn flat_rows(
    section: Section,
    entries: &[DirEntry],
    visible: usize,
    sidebar: &Sidebar,
    cx: &mut Context<Sidebar>,
) -> Vec<AnyElement> {
    entries
        .iter()
        .take(visible)
        .enumerate()
        .map(|(ix, entry)| {
            let state = RowState {
                depth: 0,
                expanded: false,
                selected: false,
                active: sidebar.active_path.as_deref() == Some(entry.path.as_path()),
                error: None,
                drop_target: false,
                dragged: false,
                draggable: false,
            };
            let text = label(entry, sidebar.options.label);
            row(
                ("sidebar-flat", (section as usize) * 1_000_000 + ix),
                entry,
                state,
                text,
                sidebar,
                cx,
            )
            .into_any_element()
        })
        .collect()
}

fn empty(text: &'static str) -> AnyElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .text_size(px(13.0))
        .text_color(theme::muted())
        .child(text)
        .into_any_element()
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Refilled by the rows drawn this frame.
        self.row_bounds.borrow_mut().clear();
        if let Some(path) = self.pending_rename.clone()
            && self.rows.iter().any(|row| row.entry.path == path)
        {
            self.pending_rename = None;
            self.start_rename(&path, window, cx);
        }

        let mut root = div()
            .id("sidebar")
            .role(Role::Tree)
            .aria_label("Files")
            .key_context("Sidebar")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .px(px(6.0))
            .on_action(cx.listener(|this, _: &ClearSelection, window, cx| {
                if this.renaming.is_some() {
                    this.cancel_rename(window, cx);
                } else if this.menu.take().is_some() {
                    cx.notify();
                } else {
                    this.clear_selection(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.open_surface_menu(event.position, cx)
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.drag_moved(event.position, None, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.end_drag(cx) {
                        this.suppress_click = true;
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_drag(cx)),
            );

        if let Some(menu) = self.menu.as_mut() {
            root = root.child(crate::menu::render(menu, window, cx));
        }

        if self.root().is_none() {
            return root.child(empty("No folder open"));
        }

        if self.setting_bool(super::SHOW_SEARCH_KEY).unwrap_or(true) {
            root = root.child(
                crate::a11y::button(div().id("sidebar-search"), "Search")
                    .mt(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(ROW_HEIGHT))
                    .pl(px(10.0))
                    .rounded(px(6.0))
                    .text_size(px(13.0))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::panel()))
                    .child(div().w(px(20.0)).flex().justify_center().child(icon(
                        Icon::Search,
                        14.0,
                        theme::muted(),
                    )))
                    .child("Search")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::Search))),
            );
        }

        if !self.pinned_entries.is_empty() {
            let collapsed = self.collapsed.contains(&Section::Pinned);
            root = root.child(header(Section::Pinned, "Pinned", collapsed, cx));
            if !collapsed {
                let entries = self.pinned_entries.clone();
                root = root.children(flat_rows(
                    Section::Pinned,
                    &entries,
                    self.pinned_visible,
                    self,
                    cx,
                ));
                if self.pinned_has_more() {
                    root = root.child(show_more(Section::Pinned, cx));
                }
            }
        }
        if self.show_recents && !self.recents.is_empty() {
            let collapsed = self.collapsed.contains(&Section::Recents);
            root = root.child(header(Section::Recents, "Recents", collapsed, cx));
            if !collapsed {
                let entries = self.recents.clone();
                root = root.children(flat_rows(
                    Section::Recents,
                    &entries,
                    self.recents_visible,
                    self,
                    cx,
                ));
                if self.recents_more {
                    root = root.child(show_more(Section::Recents, cx));
                }
            }
        }

        let collapsed = self.collapsed.contains(&Section::Everything);
        root = root.child(header(Section::Everything, "Everything", collapsed, cx));
        if collapsed {
            return root;
        }
        match &self.root_listing {
            Listing::Loading => return root,
            Listing::Failed(error) => {
                return root.child(
                    div()
                        .px(px(16.0))
                        .py(px(8.0))
                        .text_size(px(13.0))
                        .text_color(theme::danger())
                        .child(format!("Cannot read the folder: {error}")),
                );
            }
            Listing::Loaded(entries) if entries.is_empty() => return root.child(empty("No files")),
            Listing::Loaded(_) => {}
        }

        let count = self.rows.len();
        let list = uniform_list(
            "sidebar-tree",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                let highlight = this.drag_highlight();
                let dragged: Vec<std::path::PathBuf> = this
                    .drag
                    .as_ref()
                    .filter(|drag| drag.active)
                    .map(|drag| drag.paths.clone())
                    .unwrap_or_default();
                range
                    .filter_map(|ix| {
                        let tree_row = this.rows.get(ix)?.clone();
                        let state = RowState {
                            depth: tree_row.depth,
                            expanded: tree_row.expanded,
                            selected: this.selection.contains(&tree_row.entry.path),
                            active: this.active_path.as_deref()
                                == Some(tree_row.entry.path.as_path()),
                            error: tree_row.error.clone(),
                            drop_target: highlight.as_ref().is_some_and(|r| r.contains(&ix)),
                            dragged: dragged.contains(&tree_row.entry.path),
                            draggable: true,
                        };
                        let text = label(&tree_row.entry, this.options.label);
                        Some(row(
                            ("sidebar-row", ix),
                            &tree_row.entry,
                            state,
                            text,
                            this,
                            cx,
                        ))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        root.child(list)
    }
}
