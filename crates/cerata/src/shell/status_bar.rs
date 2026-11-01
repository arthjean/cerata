//! The status bar under a note (`document-footer.tsx:19-56`): words,
//! characters and paragraphs, each shown by its `statusbar.show-*`
//! setting and toggled from the bar's context menu; hidden when all three
//! are off. The counts follow `document-stats.ts:26-47`.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Styled, div, px,
};

use super::WindowView;
use crate::editor::theme;
use crate::menu::{Item, Menu};

pub const HEIGHT: f32 = 44.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub words: usize,
    pub characters: usize,
    pub paragraphs: usize,
}

/// `(setting key, footer label, menu label)` in display order.
pub const METRICS: [(&str, &str, &str); 3] = [
    ("statusbar.show-words", "words", "Words"),
    ("statusbar.show-characters", "characters", "Characters"),
    ("statusbar.show-paragraphs", "paragraphs", "Paragraphs"),
];

/// `^[ \t]{0,3}(?:#{1,6}|[-*+]|\d+[.)]|>)[ \t]+` removed from a line.
fn strip_block_prefix(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < 3 && i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    let marker_start = i;
    match bytes.get(i) {
        Some(b'#') => {
            while i < bytes.len() && bytes[i] == b'#' && i - marker_start < 6 {
                i += 1;
            }
        }
        Some(b'-' | b'*' | b'+' | b'>') => i += 1,
        Some(b) if b.is_ascii_digit() => {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if !matches!(bytes.get(i), Some(b'.' | b')')) {
                return line;
            }
            i += 1;
        }
        _ => return line,
    }
    let marker_end = i;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    if i == marker_end {
        return line;
    }
    &line[i..]
}

/// `\[\[([^\]]+)\]\]` replaced by its content.
fn unwrap_wiki_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let inner = &rest[start + 2..];
        let end = inner.find(']');
        match end {
            Some(end) if end > 0 && inner[end..].starts_with("]]") => {
                out.push_str(&rest[..start]);
                out.push_str(&inner[..end]);
                rest = &inner[end + 2..];
            }
            _ => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `\[([^\]]+)\]\([^)]*\)` replaced by the link text.
fn unwrap_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let inner = &rest[start + 1..];
        let matched = inner.find(']').filter(|end| *end > 0).and_then(|end| {
            let after = &inner[end + 1..];
            let target = after.strip_prefix('(')?;
            let close = target.find(')')?;
            Some((end, end + 2 + close + 1))
        });
        match matched {
            Some((label_end, consumed)) => {
                out.push_str(&rest[..start]);
                out.push_str(&inner[..label_end]);
                rest = &inner[consumed..];
            }
            None => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn normalize(content: &str) -> String {
    let stripped: Vec<&str> = content.split('\n').map(strip_block_prefix).collect();
    let text = stripped.join("\n").replace('`', "");
    unwrap_links(&unwrap_wiki_links(&text)).trim().to_owned()
}

pub fn document_stats(content: &str) -> Stats {
    let normalized = normalize(content);
    if normalized.is_empty() {
        return Stats::default();
    }
    let mut stats = Stats {
        words: 0,
        characters: 0,
        paragraphs: 1,
    };
    let mut in_space = true;
    let mut newlines = 0;
    for c in normalized.chars() {
        if c.is_whitespace() {
            if !in_space {
                stats.characters += 1;
                newlines = 0;
            }
            in_space = true;
            if c == '\n' {
                newlines += 1;
            }
        } else {
            if in_space {
                stats.words += 1;
                if newlines >= 2 {
                    stats.paragraphs += 1;
                }
            }
            in_space = false;
            stats.characters += 1;
        }
    }
    stats
}

/// `1234567` as `1,234,567`.
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl WindowView {
    pub fn metric_visible(&self, key: &str) -> bool {
        self.core
            .settings()
            .get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(true)
    }

    /// The active note's counts, cached per editor and revision.
    pub fn active_stats(&mut self, cx: &gpui::App) -> Option<Stats> {
        let doc = self.active_doc()?;
        if doc.loading_since.is_some() {
            return None;
        }
        let editor = doc.editor.read(cx);
        let key = (doc.editor.entity_id(), editor.state.revision());
        if let Some((cached, stats)) = self.stats_cache
            && cached == key
        {
            return Some(stats);
        }
        let stats = document_stats(editor.state.doc());
        self.stats_cache = Some((key, stats));
        Some(stats)
    }

    pub fn open_status_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = METRICS
            .iter()
            .map(|(key, _, label)| {
                let visible = self.metric_visible(key);
                let key = (*key).to_owned();
                Item::check(*label, visible, move |this: &mut WindowView, _, cx| {
                    this.write_setting(
                        &key,
                        cerata_core::settings::ConfigValue::Bool(!visible),
                        cx,
                    );
                    this.setting_written(cx);
                })
            })
            .collect();
        self.tab_menu = Some(Menu::new(position, items));
        cx.notify();
    }
}

pub fn render(view: &mut WindowView, cx: &mut Context<WindowView>) -> Option<AnyElement> {
    let visible: Vec<(&str, &str)> = METRICS
        .iter()
        .filter(|(key, _, _)| view.metric_visible(key))
        .map(|(key, label, _)| (*key, *label))
        .collect();
    if visible.is_empty() {
        return None;
    }
    let stats = view.active_stats(cx)?;
    let mut bar = div()
        .id("status-bar")
        // In the flow under the editor: drawn over it, it covered the last
        // lines at large zoom.
        .flex_none()
        .w_full()
        .h(px(HEIGHT))
        .px(px(24.0))
        .flex()
        .items_center()
        .justify_end()
        .gap(px(20.0))
        .text_size(px(13.0))
        .text_color(theme::muted())
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_status_menu(event.position, cx);
            }),
        );
    for (key, label) in visible {
        let value = match key {
            "statusbar.show-words" => stats.words,
            "statusbar.show-characters" => stats.characters,
            _ => stats.paragraphs,
        };
        bar = bar.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(div().child(group_digits(value)))
                .child(div().text_color(theme::text().opacity(0.8)).child(label)),
        );
    }
    Some(bar.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from Writer `tests/document-stats.test.ts`.

    #[test]
    fn an_empty_document_counts_nothing() {
        assert_eq!(document_stats(""), Stats::default());
        assert_eq!(document_stats("   \n\n  "), Stats::default());
    }

    #[test]
    fn markdown_prefixes_are_stripped() {
        let doc = "# Title\n\n- one two\n- three\n\n> quoted `code` [link](x) [[wiki]]\n";
        assert_eq!(
            document_stats(doc),
            Stats {
                words: 8,
                characters: 41,
                paragraphs: 3
            }
        );
    }

    #[test]
    fn an_emoji_is_one_character() {
        assert_eq!(
            document_stats("hi 😀"),
            Stats {
                words: 2,
                characters: 4,
                paragraphs: 1
            }
        );
    }

    #[test]
    fn whitespace_runs_are_one_character() {
        assert_eq!(
            document_stats("a   b\n\n\nc"),
            Stats {
                words: 3,
                characters: 5,
                paragraphs: 2
            }
        );
    }

    #[test]
    fn a_blank_line_before_a_list_still_separates_paragraphs() {
        assert_eq!(document_stats("intro\n\n- item").paragraphs, 2);
        assert_eq!(document_stats("1. one\n2) two").words, 2);
        assert_eq!(document_stats("#tag").words, 1, "no space: not a heading");
    }

    #[test]
    fn digits_are_grouped_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1234), "1,234");
        assert_eq!(group_digits(1234567), "1,234,567");
    }
}
