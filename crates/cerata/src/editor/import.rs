use std::path::{Path, PathBuf};
use std::time::Duration;

use cerata_core::fs::WorkspaceFs;
use cerata_core::images::{ImportSource, Refusal, plan_import, refusal_notice, run_import};
use gpui::{AppContext as _, Context, Task};

use super::Editor;

pub const LOADING_NOTICE_DELAY: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageTarget {
    Caret,
    At(usize),
}

pub fn named_like_image(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "svg",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

impl Editor {
    fn import_root(&self) -> Option<PathBuf> {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.root.clone())
            .or_else(|| self.path.as_ref()?.parent().map(Path::to_path_buf))
    }

    pub fn pick_images(&mut self, cx: &mut Context<Self>) {
        if self.path.is_none() {
            self.show_notice(Refusal::NotSaved.notice(), cx);
            return;
        }
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Insert".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = picked.await;
            this.update(cx, |editor, cx| match picked {
                Ok(Ok(Some(paths))) => {
                    let sources = paths.into_iter().map(ImportSource::File).collect();
                    editor.add_images(sources, ImageTarget::Caret, cx);
                }
                Ok(Err(error)) => {
                    editor.show_notice(format!("Can't open the file picker: {error}"), cx);
                }
                Ok(Ok(None)) | Err(_) => {}
            })
            .ok();
        })
        .detach();
    }

    pub fn add_images(
        &mut self,
        sources: Vec<ImportSource>,
        target: ImageTarget,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() {
            return;
        }
        let (Some(note), Some(root)) = (self.path.clone(), self.import_root()) else {
            self.show_notice(Refusal::NotSaved.notice(), cx);
            return;
        };
        let count = sources.len();
        let work = cx.background_spawn(async move {
            let fs = WorkspaceFs::new(&root);
            plan_import(Some(&note), &root, sources)
                .into_iter()
                .map(|planned| {
                    let planned = planned?;
                    match &fs {
                        Ok(fs) => run_import(fs, planned),
                        Err(error) => Err(Refusal::Io(error.to_string())),
                    }
                })
                .collect::<Vec<_>>()
        });
        self.insert_when_done(target, count, work, cx);
    }

    pub(super) fn insert_when_done(
        &mut self,
        target: ImageTarget,
        count: usize,
        work: Task<Vec<Result<String, Refusal>>>,
        cx: &mut Context<Self>,
    ) {
        let (pos, boundary) = match target {
            ImageTarget::Caret => (self.state.selection().main().head, false),
            ImageTarget::At(pos) => (pos, true),
        };
        let anchor = self.state.track(pos);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOADING_NOTICE_DELAY).await;
            this.update(cx, |editor, cx| {
                if editor.importing.contains(&anchor) {
                    editor.show_notice(loading_notice(count), cx);
                }
            })
            .ok();
        })
        .detach();
        self.importing.push(anchor);
        cx.spawn(async move |this, cx| {
            let results = work.await;
            this.update(cx, |editor, cx| {
                editor.finish_import(anchor, boundary, count, results, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_import(
        &mut self,
        anchor: u64,
        boundary: bool,
        count: usize,
        results: Vec<Result<String, Refusal>>,
        cx: &mut Context<Self>,
    ) {
        self.importing.retain(|id| *id != anchor);
        let Some(pos) = self.state.untrack(anchor) else {
            return;
        };
        let (mut images, mut refusals) = (Vec::new(), Vec::new());
        for result in results {
            match result {
                Ok(markdown) => images.push(markdown),
                Err(refusal) => refusals.push(refusal),
            }
        }
        if !images.is_empty() {
            self.update_state(cx, |state| {
                if boundary {
                    state.insert_image_blocks(pos, &images)
                } else {
                    state.insert_images(pos, &images)
                }
            });
        } else if self
            .notice
            .as_ref()
            .is_some_and(|notice| **notice == *loading_notice(count))
        {
            self.notice = None;
            cx.notify();
        }
        if let Some(notice) = refusal_notice(&refusals) {
            self.show_notice(notice, cx);
        }
    }
}

fn loading_notice(count: usize) -> String {
    match count {
        1 => "Adding 1 image\u{2026}".into(),
        n => format!("Adding {n} images\u{2026}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropBar {
    pub pos: usize,
    pub y: f64,
}

pub fn split_drop(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    if !paths.iter().any(|path| named_like_image(path)) {
        return (Vec::new(), paths.to_vec());
    }
    paths.iter().cloned().partition(|path| !opens_on_drop(path))
}

fn opens_on_drop(path: &Path) -> bool {
    match cerata_core::open_target::resolve(path) {
        Some(cerata_core::open_target::OpenTarget::Workspace(_)) => true,
        Some(cerata_core::open_target::OpenTarget::File(file)) => {
            cerata_core::markdown::is_markdown_path(&file)
        }
        None => false,
    }
}

impl Editor {
    pub(super) fn drop_boundary(&self, y: f64) -> Option<DropBar> {
        let text = self.state.text();
        let line_top = |pos: usize| self.heights.prefix(text.line_at(pos).number);
        let nearest = |before: Option<DropBar>, after: DropBar| match before {
            Some(before) if (y - before.y) <= (after.y - y) => before,
            _ => after,
        };
        let mut before = None;
        for block in self.state.tree().root().children() {
            if block.kind() == cerata_markdown::NodeKind::Frontmatter {
                continue;
            }
            let bar = DropBar {
                pos: block.from(),
                y: line_top(block.from()),
            };
            if bar.y > y {
                return Some(nearest(before, bar));
            }
            before = Some(bar);
        }
        Some(nearest(
            before,
            DropBar {
                pos: text.len(),
                y: self.heights.total(),
            },
        ))
    }

    pub(super) fn on_file_drag_move(
        &mut self,
        event: &gpui::DragMoveEvent<gpui::ExternalPaths>,
        cx: &mut Context<Self>,
    ) {
        let carries_image = event
            .drag(cx)
            .paths()
            .iter()
            .any(|path| named_like_image(path));
        let bar = if carries_image && event.bounds.contains(&event.event.position) {
            self.line_at_point(event.event.position)
                .and_then(|(_, y)| self.drop_boundary(y))
        } else {
            None
        };
        if bar != self.drop_bar {
            self.drop_bar = bar;
            cx.notify();
        }
    }

    pub(super) fn clear_drop_bar(&mut self, cx: &mut Context<Self>) {
        if self.drop_bar.take().is_some() {
            cx.notify();
        }
    }

    pub fn on_file_drop(
        &mut self,
        paths: &gpui::ExternalPaths,
        position: Option<gpui::Point<gpui::Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let bar = self.drop_bar.take().or_else(|| {
            let (_, y) = self.line_at_point(position?)?;
            self.drop_boundary(y)
        });
        let (images, others) = split_drop(paths.paths());
        if !images.is_empty() {
            let target = bar.map_or(ImageTarget::Caret, |bar| ImageTarget::At(bar.pos));
            let sources = images.into_iter().map(ImportSource::File).collect();
            self.add_images(sources, target, cx);
        }
        if !others.is_empty() {
            cx.emit(super::EditorEvent::OpenDropped(others));
        }
        cx.notify();
    }
}
