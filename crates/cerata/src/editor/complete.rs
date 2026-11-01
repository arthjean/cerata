//! Wiki link completion (US-033): typing `[[` and one non-blank character
//! lists up to 20 notes of the workspace (name, folder); accepting writes
//! the note's name and `]]`, absorbing a `]]` already after the caret.
//! Ported from `wiki-link-extension.ts:263-307` and `lib/wiki-links.ts:193`.

use cerata_core::index::SearchResult;
use cerata_markdown::{Change, NodeKind, Selection, Transaction, UserEvent};
use gpui::{
    AnyElement, AppContext, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, px,
};

use super::{Editor, theme};

/// Results Writer asks the index for.
pub const LIMIT: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The note's file name without its extension.
    pub label: String,
    /// Its folder, relative to the root; empty at the root.
    pub folder: String,
    /// What accepting it writes before `]]`.
    pub insert: String,
}

#[derive(Clone, Debug)]
pub struct Completion {
    /// Start of the query, right after `[[`.
    pub from: usize,
    pub query: String,
    pub items: Vec<Item>,
    pub selected: usize,
    /// Keeps the selected row in view as the arrows move it.
    pub scroll: ScrollHandle,
}

use cerata_core::links::file_stem as stem;

fn strip_markdown_extension(path: &str) -> &str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".md") {
        &path[..path.len() - 3]
    } else if lower.ends_with(".markdown") {
        &path[..path.len() - 9]
    } else {
        path
    }
}

/// The items for search results (`canonicalWikiTarget`): a note's name,
/// or its relative path when another result shares the name.
pub fn items(results: &[SearchResult]) -> Vec<Item> {
    results
        .iter()
        .map(|r| {
            let label = stem(&r.filename).to_owned();
            let shared = results.iter().any(|o| {
                o.path != r.path && stem(&o.filename).to_lowercase() == label.to_lowercase()
            });
            let insert = if shared {
                strip_markdown_extension(&r.relative_path).to_owned()
            } else {
                label.clone()
            };
            let folder = r.relative_path[..r.relative_path.len() - r.filename.len()]
                .trim_end_matches('/')
                .to_owned();
            Item {
                label,
                folder,
                insert,
            }
        })
        .collect()
}

impl Editor {
    /// The `[[query` before a lone caret, outside code: its start and text.
    fn completion_query(&self) -> Option<(usize, String)> {
        let main = self.state.selection().main();
        if !main.is_empty() || self.state.selection().ranges().len() > 1 {
            return None;
        }
        let text = self.state.text();
        let line = text.line_at(main.head);
        let before = text.slice(line.from..main.head);
        let open = before.rfind("[[")?;
        let query = &before[open + 2..];
        if query.contains([']', '#', '^', '|']) || query.trim().is_empty() {
            return None;
        }
        let tree = self.state.tree();
        let node = tree.resolve(line.from + open, 1);
        let in_code = std::iter::once(node)
            .chain(node.ancestors())
            .any(|n| n.kind().is_code() || n.kind() == NodeKind::InlineCode);
        (!in_code).then(|| (line.from + open + 2, query.to_owned()))
    }

    /// After a change: a typed `[[x` opens the list, a change of the query
    /// searches again, anything else closes it.
    pub(super) fn refresh_completion(&mut self, typed: bool, cx: &mut Context<Self>) {
        let Some((from, query)) = self.completion_query() else {
            self.completion = None;
            return;
        };
        let open = self.completion.as_ref().is_some_and(|c| c.from == from);
        if !open && !typed {
            self.completion = None;
            return;
        }
        if self.completion.as_ref().is_some_and(|c| c.query == query) {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let search = query.clone();
        let task =
            cx.background_spawn(async move { workspace.index.lock().search(&search, LIMIT) });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            this.update(cx, |editor, cx| {
                // Only the answer to the query still under the caret counts.
                if editor.completion_query() != Some((from, query.clone())) {
                    return;
                }
                editor.completion = (!results.is_empty()).then(|| Completion {
                    from,
                    query,
                    items: items(&results),
                    selected: 0,
                    scroll: ScrollHandle::new(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Up or Down in the open list; `false` when it is closed.
    pub(super) fn completion_move(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(c) = &mut self.completion else {
            return false;
        };
        let len = c.items.len() as isize;
        c.selected = (c.selected as isize + delta).rem_euclid(len.max(1)) as usize;
        c.scroll.scroll_to_item(c.selected);
        cx.notify();
        true
    }

    /// Escape closes the list; `false` when it was closed.
    pub(super) fn completion_close(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.completion.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    /// Writes the selected note's name and `]]` over the query, absorbing a
    /// `]]` right after the caret; `false` when the list is closed.
    pub(super) fn completion_accept(
        &mut self,
        index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(c) = self.completion.take() else {
            return false;
        };
        let Some(item) = c.items.get(index.unwrap_or(c.selected)) else {
            return false;
        };
        let to = self.cursor();
        let text = self.state.text();
        let end = if text.as_str().get(to..to + 2) == Some("]]") {
            to + 2
        } else {
            to
        };
        let insert = format!("{}]]", item.insert);
        let caret = c.from + insert.len();
        self.update_state(cx, |state| {
            let _ = state.apply(
                Transaction::new()
                    .change(Change::new(c.from..end, insert))
                    .select(Selection::cursor(caret))
                    .event(UserEvent::Command)
                    .scroll(),
            );
        });
        true
    }

    /// The list under the caret.
    pub(super) fn render_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let c = self.completion.clone()?;
        let caret = self.caret_bounds(c.from, window, cx)?;
        let mut list = div()
            .id("wiki-completion")
            .role(Role::ListBox)
            .aria_label(SharedString::from("Notes"))
            .track_scroll(&c.scroll)
            .flex()
            .flex_col()
            .min_w(px(220.0))
            .max_w(px(420.0))
            .max_h(px(280.0))
            .overflow_y_scroll()
            .p(px(4.0))
            .rounded(px(10.0))
            .bg(theme::overlay())
            .border_1()
            .border_color(theme::border())
            .shadow_md()
            .text_size(px(13.0));
        for (i, item) in c.items.iter().enumerate() {
            let mut row = div()
                .id(("wiki-completion-item", i))
                .role(Role::ListBoxOption)
                .aria_label(if item.folder.is_empty() {
                    SharedString::from(item.label.clone())
                } else {
                    SharedString::from(format!("{}, {}", item.label, item.folder))
                })
                .aria_selected(i == c.selected)
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme::row_hover()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |editor, _, _, cx| {
                        cx.stop_propagation();
                        editor.completion_accept(Some(i), cx);
                    }),
                )
                .child(div().text_color(theme::text()).child(item.label.clone()));
            if i == c.selected {
                row = row.bg(theme::row_active());
            }
            if !item.folder.is_empty() {
                row = row.child(div().text_color(theme::muted()).child(item.folder.clone()));
            }
            list = list.child(row);
        }
        Some(
            deferred(
                anchored()
                    .position(point(caret.left(), caret.bottom() + px(4.0)))
                    .child(list),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn result(relative: &str) -> SearchResult {
        SearchResult {
            path: PathBuf::from(format!("/vault/{relative}")),
            filename: relative.rsplit('/').next().unwrap().to_owned(),
            relative_path: relative.to_owned(),
            score: 0,
            match_indices: Vec::new(),
        }
    }

    #[test]
    fn items_use_the_name_or_the_path_when_names_collide() {
        let items = items(&[
            result("Roadmap.md"),
            result("a/Plan.md"),
            result("b/plan.markdown"),
        ]);
        assert_eq!(
            items[0],
            Item {
                label: "Roadmap".into(),
                folder: String::new(),
                insert: "Roadmap".into()
            }
        );
        assert_eq!(items[1].folder, "a");
        assert_eq!(items[1].insert, "a/Plan");
        assert_eq!(items[2].insert, "b/plan");
    }

    /// `wiki-links.test.ts:318-394`: a unique name, kept as written; a
    /// shared one (case aside) is written as its relative path.
    #[test]
    fn canonical_targets_follow_writer() {
        let all = [result("Roadmap.md"), result("Notes.md")];
        assert_eq!(items(&all)[0].insert, "Roadmap");
        let all = [result("planning/Roadmap.md"), result("teams/Roadmap.md")];
        assert_eq!(items(&all)[0].insert, "planning/Roadmap");
        let all = [result("docs/Guide.md"), result("archive/Guide.md")];
        assert_eq!(items(&all)[0].insert, "docs/Guide");
        let all = [result("Art direction ideas.md")];
        assert_eq!(items(&all)[0].insert, "Art direction ideas");
        assert_eq!(items(&all)[0].label, "Art direction ideas");
        let all = [result("a/Notes.md"), result("b/notes.md")];
        assert_eq!(items(&all)[0].insert, "a/Notes");
    }
}
