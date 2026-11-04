//! "Review Changes" (US-092) and "Review Conflict" (US-093): the blocks of
//! a three-way merge, the overlapping ones side by side with "Keep Mine",
//! "Keep Theirs" and "Keep Both". Blocks that merged cleanly show as a
//! one-line summary. "Apply" writes the result once every overlap has a
//! choice; for a sync service's conflict copy, "Keep Both" at the top keeps
//! the copy as a note of its own instead.

use std::path::PathBuf;

use cerata_core::merge::{Block, Choice, Merge};
use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window,
    div, prelude::FluentBuilder, px,
};

use crate::editor::theme;

/// What is reviewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The disk changed while the note had unsaved edits.
    Disk,
    /// A sync service's conflict copy of the note.
    Copy(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReviewEvent {
    Apply {
        path: PathBuf,
        source: Source,
        text: String,
    },
    /// Keep the conflict copy as a note of its own.
    KeepBothNotes {
        copy: PathBuf,
    },
    Close,
}

pub struct MergeReview {
    pub focus_handle: FocusHandle,
    pub path: PathBuf,
    pub source: Source,
    pub merge: Merge,
    /// One per conflict, in order.
    pub choices: Vec<Option<Choice>>,
}

impl EventEmitter<ReviewEvent> for MergeReview {}

impl Focusable for MergeReview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MergeReview {
    pub fn new(path: PathBuf, source: Source, merge: Merge, cx: &mut Context<Self>) -> Self {
        let choices = vec![None; merge.conflicts()];
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            path,
            source,
            merge,
            choices,
        }
    }

    pub fn choose(&mut self, conflict: usize, choice: Choice, cx: &mut Context<Self>) {
        if let Some(slot) = self.choices.get_mut(conflict) {
            *slot = Some(choice);
            cx.notify();
        }
    }

    pub fn ready(&self) -> bool {
        self.choices.iter().all(Option::is_some)
    }

    pub fn apply(&mut self, cx: &mut Context<Self>) {
        if !self.ready() {
            return;
        }
        let choices: Vec<Choice> = self.choices.iter().flatten().copied().collect();
        cx.emit(ReviewEvent::Apply {
            path: self.path.clone(),
            source: self.source.clone(),
            text: self.merge.resolve(&choices),
        });
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    enabled: bool,
    chosen: bool,
    cx: &mut Context<MergeReview>,
    run: impl Fn(&mut MergeReview, &mut Context<MergeReview>) + 'static,
) -> AnyElement {
    crate::a11y::button(div().id(id), label)
        .aria_selected(chosen)
        .min_h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .border_color(if chosen {
            theme::text()
        } else {
            theme::border()
        })
        .bg(if chosen {
            theme::row_active()
        } else {
            theme::background()
        })
        .text_size(px(13.0))
        .when(chosen, |b| b.font_weight(FontWeight::SEMIBOLD))
        .when(enabled, |b| {
            b.cursor_pointer()
                .hover(|s| s.bg(theme::row_hover()))
                .on_click(cx.listener(move |this, _, _, cx| run(this, cx)))
        })
        .when(!enabled, |b| b.tab_stop(false).text_color(theme::muted()))
        .child(if chosen {
            format!("\u{2713} {label}")
        } else {
            label.to_owned()
        })
        .into_any_element()
}

fn side(heading: &'static str, text: &str) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme::muted())
                .child(heading),
        )
        .child(
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(theme::code_background())
                .text_size(px(13.0))
                .children(if text.is_empty() {
                    vec![div().text_color(theme::muted()).child("(nothing)")]
                } else {
                    text.lines()
                        .map(|line| {
                            div()
                                .min_h(px(18.0))
                                .whitespace_normal()
                                .child(line.to_owned())
                        })
                        .collect()
                }),
        )
        .into_any_element()
}

impl Render for MergeReview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.choices.len();
        let left = self.choices.iter().filter(|c| c.is_none()).count();
        let summary = match (total, left) {
            (0, _) => "No change overlaps yours. Apply merges both.".to_owned(),
            (1, 1) => "One change overlaps yours. Choose what to keep.".to_owned(),
            (_, 0) => "Every overlap has a choice. Apply writes the note.".to_owned(),
            (n, l) => format!("{n} changes overlap yours; {l} still need a choice."),
        };
        let (theirs_label, title) = match &self.source {
            Source::Disk => ("Theirs (on disk)", "Review Changes".to_owned()),
            Source::Copy(copy) => (
                "Theirs (conflict copy)",
                format!(
                    "Review Conflict with {}",
                    copy.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
            ),
        };
        let copy = match &self.source {
            Source::Copy(copy) => Some(copy.clone()),
            Source::Disk => None,
        };
        let ready = self.ready();
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.0))
                            .child(title),
                    )
                    .child(
                        div()
                            .id("review-summary")
                            .role(gpui::Role::Status)
                            .aria_label(summary.clone())
                            .text_size(px(12.0))
                            .text_color(theme::muted())
                            .child(summary),
                    ),
            )
            .children(copy.map(|copy| {
                button(
                    "review-keep-both-notes",
                    "Keep Both",
                    true,
                    false,
                    cx,
                    move |_, cx| cx.emit(ReviewEvent::KeepBothNotes { copy: copy.clone() }),
                )
            }))
            .child(button(
                "review-apply",
                "Apply",
                ready,
                false,
                cx,
                |this, cx| this.apply(cx),
            ))
            .child(button("review-close", "Close", true, false, cx, |_, cx| {
                cx.emit(ReviewEvent::Close)
            }));

        let mut conflict = 0;
        let mut blocks: Vec<AnyElement> = Vec::new();
        for (ix, block) in self.merge.blocks.iter().enumerate() {
            match block {
                Block::Merged(text) => {
                    let lines = text.lines().count().max(1);
                    let label = if lines == 1 {
                        "1 line kept or merged".to_owned()
                    } else {
                        format!("{lines} lines kept or merged")
                    };
                    blocks.push(
                        div()
                            .px(px(4.0))
                            .text_size(px(12.0))
                            .text_color(theme::muted())
                            .child(label)
                            .into_any_element(),
                    );
                }
                Block::Conflict { ours, theirs, .. } => {
                    let n = conflict;
                    conflict += 1;
                    let chosen = self.choices.get(n).copied().flatten();
                    blocks.push(
                        div()
                            .id(("review-block", ix))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .p(px(12.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("Overlap {} of {total}", n + 1)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(12.0))
                                    .child(side("Mine (in the editor)", ours))
                                    .child(side(theirs_label, theirs)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(6.0))
                                    .child(button(
                                        ("review-mine", n),
                                        "Keep Mine",
                                        true,
                                        chosen == Some(Choice::Mine),
                                        cx,
                                        move |this, cx| this.choose(n, Choice::Mine, cx),
                                    ))
                                    .child(button(
                                        ("review-theirs", n),
                                        "Keep Theirs",
                                        true,
                                        chosen == Some(Choice::Theirs),
                                        cx,
                                        move |this, cx| this.choose(n, Choice::Theirs, cx),
                                    ))
                                    .child(button(
                                        ("review-both", n),
                                        "Keep Both",
                                        true,
                                        chosen == Some(Choice::Both),
                                        cx,
                                        move |this, cx| this.choose(n, Choice::Both, cx),
                                    )),
                            )
                            .into_any_element(),
                    );
                }
            }
        }

        div()
            .id("merge-review")
            .key_context("MergeReview")
            .track_focus(&self.focus_handle)
            .role(gpui::Role::Dialog)
            .aria_label("Review changes")
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(ReviewEvent::Close);
                    cx.stop_propagation();
                }
            }))
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(theme::background())
            .child(header)
            .child(
                div()
                    .id("review-blocks")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .children(blocks),
            )
    }
}
