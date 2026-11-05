use std::time::Duration;

use gpui::{
    App, BorderStyle, BoxShadow, CursorStyle, Div, Entity, Hsla, InteractiveElement as _,
    MouseButton, ParentElement as _, Styled as _, Window, canvas, div, hsla, point,
    prelude::FluentBuilder as _, px, quad, transparent_black,
};

use crate::editor::text_field::TextField;
use crate::editor::theme;

pub const HEIGHT: f32 = 32.0;
pub const RADIUS: f32 = 10.0;
pub const INSET: f32 = 11.0;
const RING: f32 = 3.0;
const FOCUS_MOTION: Duration = Duration::from_millis(160);

pub fn focus_progress(elapsed: Duration, reduce_motion: bool) -> f32 {
    if reduce_motion {
        return 1.0;
    }
    let t = (elapsed.as_secs_f32() / FOCUS_MOTION.as_secs_f32()).min(1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn border() -> Hsla {
    theme::text().opacity(if theme::palette().dark { 0.08 } else { 0.10 })
}

fn neutral_ring() -> Hsla {
    theme::text().opacity(if theme::palette().dark { 0.45 } else { 0.40 })
}

pub fn ring_color() -> Hsla {
    theme::accent()
}

pub fn background() -> Hsla {
    if theme::palette().dark {
        theme::text().opacity(0.08 * 0.32)
    } else {
        theme::background()
    }
}

pub fn shadow(y: f32, blur: f32, spread: f32, alpha: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0.0, 0.0, 0.0, alpha),
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

pub fn rest_shadow() -> BoxShadow {
    shadow(1.0, 2.0, 0.0, 0.05)
}

pub fn halo(outset: f32, width: f32, radius: f32, color: Hsla) -> Div {
    let ring = div()
        .absolute()
        .top(px(-outset))
        .left(px(-outset))
        .right(px(-outset))
        .bottom(px(-outset))
        .rounded(px(radius + outset))
        .border_color(color);
    if width < 2.5 {
        ring.border_2()
    } else {
        ring.border_3()
    }
}

fn look(focused: bool, invalid: bool) -> (Hsla, Option<Hsla>) {
    let danger = theme::danger();
    match (invalid, focused) {
        (false, false) => (border(), None),
        (false, true) => (neutral_ring(), Some(neutral_ring().opacity(0.24))),
        (true, false) => (danger.opacity(0.36), None),
        (true, true) => (
            danger.opacity(0.64),
            Some(danger.opacity(if theme::palette().dark { 0.24 } else { 0.16 })),
        ),
    }
}

pub fn frame(field: &Entity<TextField>, window: &Window, cx: &App) -> Div {
    let state = field.read(cx);
    let focused = state.focus_handle.is_focused(window);
    let (rest_border, _) = look(false, state.invalid);
    let (focus_border, ring) = look(focused, state.invalid);
    let target = field.clone();
    let painted = field.clone();
    div()
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .h(px(HEIGHT))
        .px(px(INSET))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(rest_border)
        .bg(background())
        .when(ring.is_none() && !state.invalid, |d| {
            d.shadow(vec![rest_shadow()])
        })
        .text_size(px(14.0))
        .text_color(theme::text())
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            target.update(cx, |field, cx| field.click_at(event.position, window, cx));
            window.prevent_default();
        })
        .when_some(ring, |d, ring| {
            let outset = 1.0 + RING;
            d.child(
                div()
                    .absolute()
                    .top(px(-outset))
                    .left(px(-outset))
                    .right(px(-outset))
                    .bottom(px(-outset))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                let Some(since) = painted.read(cx).focused_since() else {
                                    return;
                                };
                                let progress = focus_progress(since.elapsed(), cx.reduce_motion());
                                if progress < 1.0 {
                                    window.request_animation_frame();
                                }
                                let width = RING * progress;
                                if width > 0.0 {
                                    window.paint_quad(quad(
                                        bounds.inset(px(RING - width)),
                                        px(RADIUS + width),
                                        transparent_black(),
                                        px(width),
                                        ring.opacity(progress),
                                        BorderStyle::Solid,
                                    ));
                                }
                                window.paint_quad(quad(
                                    bounds.inset(px(RING)),
                                    px(RADIUS),
                                    transparent_black(),
                                    px(1.0),
                                    focus_border.opacity(progress),
                                    BorderStyle::Solid,
                                ));
                            },
                        )
                        .size_full(),
                    ),
            )
        })
}

pub fn field(field: &Entity<TextField>, width: f32, window: &Window, cx: &App) -> Div {
    frame(field, window, cx)
        .w(px(width))
        .child(div().flex_1().min_w_0().child(field.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_eases_in_over_160_ms() {
        assert_eq!(focus_progress(Duration::ZERO, false), 0.0);
        assert!((focus_progress(FOCUS_MOTION / 2, false) - 0.875).abs() < 1e-4);
        assert_eq!(focus_progress(FOCUS_MOTION, false), 1.0);
        assert_eq!(focus_progress(Duration::ZERO, true), 1.0, "reduced motion");
    }
}
