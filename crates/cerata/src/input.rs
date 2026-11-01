//! coss ui's input control (https://coss.com/ui/docs/components/input),
//! re-implemented from its measures: a 32 px frame, radius 10, a 1 px
//! border in the input tint, an 11 px inset and 14 px text. The whole frame
//! takes the focus. Focus turns the border to coss ui's neutral ring
//! color with a 3 px ring at 24 % around it; an invalid value turns it red
//! (36 %, 64 % focused, with a red ring). The select trigger and the switch
//! share the tokens and the ring, in the accent.
//!
//! The ring is a hollow outline, not a spread shadow: GPUI paints a shadow
//! under the whole element, and the frame's translucent fill would show it
//! as a gray tint where CSS keeps it outside.

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
/// The text's inset: coss ui's 12 px less the border.
pub const INSET: f32 = 11.0;
const RING: f32 = 3.0;
/// Paneflow's input focus: the ring grows from the border to 3 px and the
/// focus border fades in, over 160 ms on a cubic ease-out.
const FOCUS_MOTION: Duration = Duration::from_millis(160);

/// How far the focus has come in, `elapsed` after the field took it.
pub fn focus_progress(elapsed: Duration, reduce_motion: bool) -> f32 {
    if reduce_motion {
        return 1.0;
    }
    let t = (elapsed.as_secs_f32() / FOCUS_MOTION.as_secs_f32()).min(1.0);
    1.0 - (1.0 - t).powi(3)
}

/// coss ui's `input` tint, the frame's border: the text at 10 % (8 % on
/// dark).
pub fn border() -> Hsla {
    theme::text().opacity(if theme::palette().dark { 0.08 } else { 0.10 })
}

/// coss ui's `ring` for the fields' focus, neutral-400 on light and
/// neutral-500 on dark: the text at 40 % (45 % on dark) lands on the same
/// gray over the page.
fn neutral_ring() -> Hsla {
    theme::text().opacity(if theme::palette().dark { 0.45 } else { 0.40 })
}

/// The select trigger's and the switch's focus color: the theme's accent
/// (orange by default).
pub fn ring_color() -> Hsla {
    theme::accent()
}

/// The page background on light themes; on dark, the input tint at 32 %
/// over the card.
pub fn background() -> Hsla {
    if theme::palette().dark {
        theme::text().opacity(0.08 * 0.32)
    } else {
        theme::background()
    }
}

/// A black shadow at `alpha`.
pub fn shadow(y: f32, blur: f32, spread: f32, alpha: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0.0, 0.0, 0.0, alpha),
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

/// Tailwind's `shadow-xs` at 5 %, under a frame at rest.
pub fn rest_shadow() -> BoxShadow {
    shadow(1.0, 2.0, 0.0, 0.05)
}

/// A hollow ring `width` thick (2 or 3 px) in `color`, `outset` beyond
/// its parent's padding box (the parent's border plus any gap), rounded
/// to follow a parent of `radius`.
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

/// The frame's border and ring for its state.
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

/// The frame around `field`; the caller sets the width and adds the field
/// (and anything beside it). A press anywhere in it puts the caret in the
/// field. Taking the focus animates in, on Paneflow's inputs.
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
            // Keeps a focusable ancestor (the settings panel) from taking
            // the focus back, as a focusable element's own press does.
            window.prevent_default();
        })
        .when_some(ring, |d, ring| {
            // Drawn over the border box and the ring's room around it.
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

/// [`frame`] holding `field`, `width` wide.
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
        // Halfway in time, a cubic ease-out is 7/8 of the way.
        assert!((focus_progress(FOCUS_MOTION / 2, false) - 0.875).abs() < 1e-4);
        assert_eq!(focus_progress(FOCUS_MOTION, false), 1.0);
        assert_eq!(focus_progress(Duration::ZERO, true), 1.0, "reduced motion");
    }
}
