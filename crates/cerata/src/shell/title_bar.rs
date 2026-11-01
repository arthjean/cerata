//! The title bar. On Windows the system one is hidden and Cerata draws its
//! own: the sidebar's top row carries the wordmark, the main column's top
//! row carries the tabs (or the compact header) and the caption buttons.
//! Empty space in either row drags the window; every control in them
//! occludes so its clicks are not taken for a drag. Other platforms keep
//! their system title bar and get the same rows below it.

use std::borrow::Cow;

use gpui::{
    AnyElement, App, FontFallbacks, InteractiveElement, IntoElement, ParentElement, Rgba,
    SharedString, Styled, TitlebarOptions, Window, WindowControlArea, div,
    prelude::FluentBuilder as _, px,
};

use crate::editor::theme;

/// Height of both top rows.
pub const HEIGHT: f32 = 40.0;

/// Cerata draws the title bar where the system one can be hidden and
/// replaced by drag and caption hit areas.
pub const DRAWN: bool = cfg!(target_os = "windows");

/// Family of the wordmark (SIL Open Font License, `assets/fonts/OFL.txt`).
const WORDMARK_FONT: &str = "UnifrakturMaguntia";

/// Registers the wordmark font.
pub fn init(cx: &mut App) {
    let font = include_bytes!("../../assets/fonts/UnifrakturMaguntia-Book.ttf");
    if let Err(error) = cx.text_system().add_fonts(vec![Cow::Borrowed(font)]) {
        log::warn!("wordmark font unavailable: {error}");
    }
}

pub fn options() -> Option<TitlebarOptions> {
    Some(TitlebarOptions {
        appears_transparent: DRAWN,
        ..Default::default()
    })
}

/// A top row: a window drag area where the title bar is drawn.
pub fn row(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(HEIGHT))
        .when(DRAWN, |d| d.window_control_area(WindowControlArea::Drag))
}

/// "Cerata" in blackletter, at the top of the sidebar.
pub fn wordmark() -> impl IntoElement {
    row("sidebar-title-bar")
        .px(px(16.0))
        .text_size(px(24.0))
        .font_family(WORDMARK_FONT)
        .text_color(theme::text())
        // The face's ascent runs high: lower it onto the tabs' baseline.
        .child(div().relative().top(px(3.0)).child("Cerata"))
}

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Minimize, maximize (or restore) and close, at the right of the main
/// row. Windows handles their clicks from the hit areas.
pub fn caption_buttons(window: &Window) -> Option<AnyElement> {
    if !DRAWN || window.is_fullscreen() {
        return None;
    }
    let size = if window.is_maximized() {
        Caption::Restore
    } else {
        Caption::Maximize
    };
    Some(
        div()
            .flex()
            .flex_none()
            .h_full()
            // Glyphs of Windows' own caption buttons (11, then 10).
            .font(gpui::Font {
                fallbacks: Some(FontFallbacks::from_fonts(vec!["Segoe MDL2 Assets".into()])),
                ..gpui::font("Segoe Fluent Icons")
            })
            .children([Caption::Minimize, size, Caption::Close].map(caption_button))
            .into_any_element(),
    )
}

fn caption_button(caption: Caption) -> impl IntoElement {
    let (id, glyph, area) = match caption {
        Caption::Minimize => ("caption-minimize", "\u{e921}", WindowControlArea::Min),
        Caption::Maximize => ("caption-maximize", "\u{e922}", WindowControlArea::Max),
        Caption::Restore => ("caption-restore", "\u{e923}", WindowControlArea::Max),
        Caption::Close => ("caption-close", "\u{e8bb}", WindowControlArea::Close),
    };
    // Windows 11's close hover.
    let close_red = Rgba {
        r: 196.0 / 255.0,
        g: 43.0 / 255.0,
        b: 28.0 / 255.0,
        a: 1.0,
    };
    div()
        .id(SharedString::from(id))
        .occlude()
        .window_control_area(area)
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.0))
        .h_full()
        .text_size(px(10.0))
        .text_color(theme::text())
        .map(|d| match caption {
            Caption::Close => d.hover(|s| s.bg(close_red).text_color(gpui::white())),
            _ => d.hover(|s| s.bg(theme::panel())),
        })
        .child(glyph)
}
