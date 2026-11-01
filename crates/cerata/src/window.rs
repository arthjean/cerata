//! Main window geometry, taken from Writer (`tauri.conf.json:14-31`).

use std::path::PathBuf;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, Focusable as _, IntoElement, ParentElement,
    Render, Size, Styled, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
};

use crate::editor::{Editor, EditorConfig};

/// Initial content size of a new window.
pub const DEFAULT_SIZE: Size<f32> = Size {
    width: 1200.0,
    height: 800.0,
};

/// Smallest size the user can shrink a window to.
pub const MIN_SIZE: Size<f32> = Size {
    width: 800.0,
    height: 500.0,
};

/// The note a window opens on: its text and, when it is a file, its path
/// and the workspace it belongs to.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub text: String,
    pub path: Option<PathBuf>,
    /// Local images load only from inside it; the note's folder when `None`.
    pub root: Option<PathBuf>,
}

/// The window's root view: the editor on the opened note, until the shell
/// (tabs, sidebar) lands in EP-004.
pub struct MainWindow {
    pub editor: Entity<Editor>,
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.editor.clone())
    }
}

pub fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(
        None,
        size(px(DEFAULT_SIZE.width), px(DEFAULT_SIZE.height)),
        cx,
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(MIN_SIZE.width), px(MIN_SIZE.height))),
        app_id: Some("cerata".into()),
        ..Default::default()
    }
}

pub fn open_main_window(
    cx: &mut App,
    document: Document,
    config: EditorConfig,
) -> anyhow::Result<WindowHandle<MainWindow>> {
    cx.open_window(window_options(cx), |window, cx| {
        let editor = cx.new(|cx| {
            let mut editor = Editor::new(document.text, document.path, config, cx);
            if let Some(root) = &document.root {
                editor.images.set_root(root);
            }
            editor
        });
        window.focus(&editor.focus_handle(cx), cx);
        cx.new(|_| MainWindow { editor })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn main_window_opens_at_writer_geometry(cx: &mut TestAppContext) {
        let options = cx.update(|cx| window_options(cx));
        assert_eq!(
            options.window_min_size,
            Some(size(px(800.0), px(500.0))),
            "minimum size"
        );
        let handle = cx
            .update(|cx| open_main_window(cx, Document::default(), EditorConfig::default()))
            .expect("window opens");
        let bounds = handle
            .update(cx, |_, window, _| window.bounds())
            .expect("window alive");
        assert_eq!(bounds.size, size(px(1200.0), px(800.0)));
    }
}
