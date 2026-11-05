use std::borrow::Cow;
use std::cell::Cell;

use gpui::{
    AnyElement, App, Decorations, FontFallbacks, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Rgba, SharedString, StatefulInteractiveElement as _, Styled, TitlebarOptions,
    Window, WindowControlArea, div, prelude::FluentBuilder as _, px,
};

use crate::editor::theme;
use crate::icons::{Icon, icon};

pub const HEIGHT: f32 = 40.0;

const WINDOWS: bool = cfg!(target_os = "windows");

pub fn drawn(window: &Window) -> bool {
    WINDOWS || client_decorated(window)
}

fn client_decorated(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

const WORDMARK_FONT: &str = "UnifrakturMaguntia";

pub fn init(cx: &mut App) {
    let font = include_bytes!("../../assets/fonts/UnifrakturMaguntia-Book.ttf");
    if let Err(error) = cx.text_system().add_fonts(vec![Cow::Borrowed(font)]) {
        log::warn!("wordmark font unavailable: {error}");
    }
}

pub fn options() -> Option<TitlebarOptions> {
    Some(TitlebarOptions {
        appears_transparent: WINDOWS,
        ..Default::default()
    })
}

pub fn row(id: &'static str) -> gpui::Stateful<gpui::Div> {
    let row = div().id(id).flex().flex_none().items_center().h(px(HEIGHT));
    if WINDOWS {
        row.window_control_area(WindowControlArea::Drag)
    } else {
        client_drag(row)
    }
}

thread_local! {
    static PRESSED: Cell<bool> = const { Cell::new(false) };
}

fn client_drag(row: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    row.on_mouse_down(MouseButton::Left, |_, window, _| {
        PRESSED.set(client_decorated(window));
    })
    .on_mouse_up(MouseButton::Left, |_, _, _| PRESSED.set(false))
    .on_mouse_up_out(MouseButton::Left, |_, _, _| PRESSED.set(false))
    .on_mouse_move(|event, window, _| {
        if event.pressed_button == Some(MouseButton::Left) && PRESSED.replace(false) {
            window.start_window_move();
        }
    })
    .on_click(|event, window, _| {
        if event.click_count() == 2 && client_decorated(window) {
            window.zoom_window();
        }
    })
    .on_mouse_down(MouseButton::Right, |event, window, _| {
        if client_decorated(window) && window.window_controls().window_menu {
            window.show_window_menu(event.position);
        }
    })
}

pub fn wordmark() -> impl IntoElement {
    row("sidebar-title-bar")
        .px(px(10.0))
        .text_size(px(24.0))
        .font_family(WORDMARK_FONT)
        .text_color(theme::text())
        .child(div().relative().top(px(3.0)).child("Cerata"))
}

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

pub fn caption_buttons(window: &Window) -> Option<AnyElement> {
    if !drawn(window) || window.is_fullscreen() {
        return None;
    }
    let size = if window.is_maximized() {
        Caption::Restore
    } else {
        Caption::Maximize
    };
    if !WINDOWS {
        let controls = window.window_controls();
        let captions = [
            controls.minimize.then_some(Caption::Minimize),
            controls.maximize.then_some(size),
            Some(Caption::Close),
        ];
        return Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.0))
                .px(px(10.0))
                .children(captions.into_iter().flatten().map(client_caption_button))
                .into_any_element(),
        );
    }
    Some(
        div()
            .flex()
            .flex_none()
            .h_full()
            .font(gpui::Font {
                fallbacks: Some(FontFallbacks::from_fonts(vec!["Segoe MDL2 Assets".into()])),
                ..gpui::font("Segoe Fluent Icons")
            })
            .children([Caption::Minimize, size, Caption::Close].map(caption_button))
            .into_any_element(),
    )
}

fn client_caption_button(caption: Caption) -> impl IntoElement {
    let (id, glyph, label) = match caption {
        Caption::Minimize => ("caption-minimize", Icon::Minimize, "Minimize"),
        Caption::Maximize => ("caption-maximize", Icon::Maximize, "Maximize"),
        Caption::Restore => ("caption-restore", Icon::Restore, "Restore"),
        Caption::Close => ("caption-close", Icon::Close, "Close"),
    };
    crate::a11y::button(div().id(SharedString::from(id)), label)
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.0))
        .rounded_full()
        .bg(theme::text().opacity(0.08))
        .hover(|s| s.bg(theme::text().opacity(0.16)))
        .active(|s| s.bg(theme::text().opacity(0.24)))
        .child(icon(glyph, 14.0, theme::text()))
        .on_click(move |_, window, cx| match caption {
            Caption::Minimize => window.minimize_window(),
            Caption::Maximize | Caption::Restore => window.zoom_window(),
            Caption::Close => window.dispatch_action(Box::new(crate::app_menu::CloseWindow), cx),
        })
}

fn caption_button(caption: Caption) -> impl IntoElement {
    let (id, glyph, area) = match caption {
        Caption::Minimize => ("caption-minimize", "\u{e921}", WindowControlArea::Min),
        Caption::Maximize => ("caption-maximize", "\u{e922}", WindowControlArea::Max),
        Caption::Restore => ("caption-restore", "\u{e923}", WindowControlArea::Max),
        Caption::Close => ("caption-close", "\u{e8bb}", WindowControlArea::Close),
    };
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
