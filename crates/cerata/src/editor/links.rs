//! Following links from the editor (US-033): a click on a folded link, a
//! folded wiki link, a bare URL or a link drawn in a widget. Ported from
//! `link-navigation.ts:15-95` and `wiki-link-extension.ts:330-373`.
//!
//! `#anchor` scrolls this note; a note of the workspace opens in the app
//! (the shell handles [`EditorEvent::OpenNote`]); `http`, `https` and
//! `mailto` go to the OS; any other local file goes to the OS after a
//! confirmation naming it; other schemes are refused.

use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

use cerata_core::links::{self, LinkTarget as Resolved};
use cerata_markdown::{Decoration, MarkStyle, NodeKind, Widget};
use gpui::{Context, EventEmitter, Pixels, Point, PromptLevel, Window, point, px};

use super::blocks::LinkTarget;
use super::{Autoscroll, Editor};

/// What the editor asks of the window around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// Open a note of the workspace, then scroll to the heading whose slug
    /// is `anchor`.
    OpenNote {
        path: PathBuf,
        anchor: Option<String>,
    },
}

impl EventEmitter<EditorEvent> for Editor {}

fn to_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn from_slash(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', MAIN_SEPARATOR_STR))
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl Editor {
    /// The link drawn under a window point: the text of a folded link, a
    /// folded wiki link or a bare URL. Revealed syntax is not a link.
    pub(super) fn link_hit(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<LinkTarget> {
        let viewport = self.viewport?;
        let (ix, y) = self.line_at_point(position)?;
        let line = self.layout_line(ix, window, cx);
        let wrapped = line.wrapped.as_ref()?;
        let row_height = line.metrics.row_height;
        let local_y = (y - self.heights.prefix(ix)) as f32 - line.text_top;
        if local_y < 0.0 || local_y >= line.rows() as f32 * row_height {
            return None;
        }
        let scroll = line.code.as_ref().map_or(0.0, |c| c.scroll_x);
        let x = (position.x - viewport.column_left).as_f32() - line.text_left + scroll;
        let display = wrapped
            .index_for_position(point(px(x), px(local_y)), px(row_height))
            .ok()?;
        let segment = line
            .display
            .source_runs()
            .find(|s| s.display.start <= display && display < s.display.end)?;
        let range = line.start + segment.buffer.start..line.start + segment.buffer.end;
        let decorations = self.state.decorations_in(range.clone());
        if !segment.source {
            return decorations.into_iter().find_map(|d| match d {
                Decoration::Replace {
                    range: r,
                    widget: Widget::WikiLink { .. },
                    ..
                } if r.start <= range.start && range.end <= r.end => {
                    let text = self.state.text().slice(r.start + 2..r.end - 2);
                    Some(LinkTarget::Wiki(text.to_owned()))
                }
                _ => None,
            });
        }
        let pos = range.start + (display - segment.display.start);
        let folded_link = decorations.iter().any(|d| {
            matches!(d, Decoration::Mark { range: r, style: MarkStyle::RenderedLink } if r.start <= pos && pos < r.end)
        });
        if folded_link {
            return super::menu::link_at(&self.state, pos).map(LinkTarget::Url);
        }
        let tree = self.state.tree();
        let node = tree.resolve(pos, 1);
        std::iter::once(node)
            .chain(node.ancestors())
            .find(|n| n.kind() == NodeKind::Url)
            .filter(|url| {
                url.parent()
                    .is_none_or(|p| !matches!(p.kind(), NodeKind::Link | NodeKind::Image))
            })
            .map(|url| LinkTarget::Url(self.state.text().slice(url.range()).to_owned()))
    }

    /// Follows a link clicked in the text or in a widget.
    pub fn follow_link(&mut self, target: LinkTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Url(href) => self.follow_href(&href, window, cx),
            LinkTarget::Wiki(raw) => self.follow_wiki(&raw, cx),
        }
    }

    fn root(&self) -> Option<String> {
        self.workspace.as_ref().map(|w| to_slash(&w.root))
    }

    fn current(&self) -> String {
        self.path.as_deref().map(to_slash).unwrap_or_default()
    }

    /// A Markdown destination (`followLink`).
    pub(super) fn follow_href(&mut self, href: &str, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root();
        let current = self.current();
        let exists = |p: &str| from_slash(p).is_file();
        let Some(target) = links::resolve(href, &current, root.as_deref(), &exists) else {
            return;
        };
        match target {
            Resolved::SameDocAnchor(anchor) => self.go_to_anchor(&anchor, cx),
            Resolved::Internal { path, anchor } => {
                if links::normalize_path(&path) == links::normalize_path(&current) {
                    if let Some(anchor) = anchor {
                        self.go_to_anchor(&anchor, cx);
                    }
                    return;
                }
                cx.emit(EditorEvent::OpenNote {
                    path: from_slash(&path),
                    anchor,
                });
            }
            Resolved::ExternalUrl(url) => {
                let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
                if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                    cx.open_url(&url);
                } else {
                    self.show_notice(format!("{scheme}: links can't be opened"), cx);
                }
            }
            Resolved::ExternalPath(path) => {
                let local = from_slash(&path);
                if !local.exists() {
                    self.show_notice(format!("File not found: {}", file_name(&path)), cx);
                    return;
                }
                let detail = local.display().to_string();
                let (level, message) = match super::menu::openable(&local) {
                    Some(_) => (PromptLevel::Info, "Open this file in its default app?"),
                    None if local.is_file() => (
                        PromptLevel::Warning,
                        "This file may run a program. Open it anyway?",
                    ),
                    None => {
                        self.show_notice(format!("Can't open {}", file_name(&path)), cx);
                        return;
                    }
                };
                let answer = window.prompt(level, message, Some(&detail), &["Open", "Cancel"], cx);
                cx.spawn(async move |_, cx| {
                    if answer.await == Ok(0) {
                        cx.update(|cx| cx.open_with_system(&local));
                    }
                })
                .detach();
            }
        }
    }

    /// A wiki link: the note it names opens; none, and a notice says so
    /// (no file is created).
    fn follow_wiki(&mut self, raw: &str, cx: &mut Context<Self>) {
        let parsed = cerata_markdown::wiki::parse(raw);
        let anchor = parsed
            .fragment
            .as_deref()
            .map(cerata_markdown::headings::slugify)
            .filter(|a| !a.is_empty());
        let (Some(root), Some(workspace)) = (self.root(), self.workspace.clone()) else {
            self.show_notice(format!("Note not found: {}", parsed.display), cx);
            return;
        };
        let current = self.current();
        let exists = |p: &str| from_slash(p).is_file();
        let stems = |query: &str| -> Vec<String> {
            workspace
                .index
                .lock()
                .search(query, 50)
                .into_iter()
                .map(|r| to_slash(&r.path))
                .collect()
        };
        let found = links::resolve_wiki(raw, &root, Some(&current), &exists, &stems);
        match found {
            Some(path) if links::normalize_path(&path) == links::normalize_path(&current) => {
                if let Some(anchor) = anchor {
                    self.go_to_anchor(&anchor, cx);
                }
            }
            Some(path) => cx.emit(EditorEvent::OpenNote {
                path: from_slash(&path),
                anchor,
            }),
            None => {
                let name = if parsed.path.is_empty() {
                    parsed.target
                } else {
                    parsed.path
                };
                self.show_notice(format!("Note not found: {name}"), cx);
            }
        }
    }

    /// Scrolls the heading whose slug is `anchor` to the top of the safe
    /// zone; a note still loading does it once loaded.
    pub fn go_to_anchor(&mut self, anchor: &str, cx: &mut Context<Self>) {
        if self.state.is_loading() {
            self.pending_anchor = Some(anchor.to_owned());
            return;
        }
        match cerata_markdown::headings::find_by_slug(self.state.doc(), anchor) {
            Some(heading) => {
                self.autoscroll = Some(Autoscroll::Top(heading.pos));
                cx.notify();
            }
            None => self.show_notice(format!("Heading \"#{anchor}\" not found in this note"), cx),
        }
    }
}
