//! Main window geometry, taken from Writer (`tauri.conf.json:14-31`).

use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, Render, Size, Styled, Window, WindowBounds,
    WindowHandle, WindowOptions, div, px, rgb, size,
};

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

/// Placeholder root view until the shell lands (EP-004).
pub struct MainWindow;

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xfafafa))
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

pub fn open_main_window(cx: &mut App) -> anyhow::Result<WindowHandle<MainWindow>> {
    cx.open_window(window_options(cx), |_, cx| cx.new(|_| MainWindow))
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
        let handle = cx.update(open_main_window).expect("window opens");
        let bounds = handle
            .update(cx, |_, window, _| window.bounds())
            .expect("window alive");
        assert_eq!(bounds.size, size(px(1200.0), px(800.0)));
    }
}
