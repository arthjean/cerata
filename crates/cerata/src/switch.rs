//! A switch on coss ui's (https://coss.com/ui/docs/components/switch),
//! re-implemented from its measures and motion over Base UI: a 30 by 18
//! pill with a 1 px inset, a 16 px thumb that slides 12 px in 150 ms, the
//! track fading from the input tint to the accent (coss ui's primary) in
//! 200 ms, and the thumb stretching 10 % toward the center while pressed.
//! Reduced motion jumps to the end state.

use std::time::{Duration, Instant};

use gpui::{
    BoxShadow, Div, FocusHandle, InteractiveElement as _, ParentElement as _, Role, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Toggled, Window, div, hsla, point,
    prelude::FluentBuilder as _, px,
};

use crate::editor::theme;
use crate::sidebar::reveal::Reveal;

const THUMB: f32 = 16.0;
/// The thumb's travel: the 28 px inside less the thumb's 16, less coss
/// ui's 4 px overlap (`translate-x: thumb - 4px`).
const TRAVEL: f32 = THUMB - 4.0;
const STRETCH: f32 = THUMB * 0.1;
const SLIDE: Duration = Duration::from_millis(150);
const FADE: Duration = Duration::from_millis(200);
const PRESS: Duration = Duration::from_millis(100);

/// A switch's motion: the thumb's travel, the track's tint and the press
/// stretch, each from 0 to 1.
#[derive(Clone, Copy, Debug)]
pub struct SwitchMotion {
    on: bool,
    pressed: bool,
    thumb: Reveal,
    track: Reveal,
    press: Reveal,
}

impl SwitchMotion {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            pressed: false,
            thumb: Reveal::new(on, SLIDE),
            track: Reveal::new(on, FADE),
            press: Reveal::new(false, PRESS),
        }
    }

    /// Follows the value and the pointer: a change animates, or lands at
    /// once under reduced motion.
    pub fn sync(&mut self, on: bool, pressed: bool, now: Instant, reduce: bool) {
        if on != self.on {
            self.on = on;
            if reduce {
                self.thumb = Reveal::new(on, SLIDE);
                self.track = Reveal::new(on, FADE);
            } else {
                self.thumb.retarget(on, now);
                self.track.retarget(on, now);
            }
        }
        if pressed != self.pressed {
            self.pressed = pressed;
            if reduce {
                self.press = Reveal::new(pressed, PRESS);
            } else {
                self.press.retarget(pressed, now);
            }
        }
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.thumb.animating(now) || self.track.animating(now) || self.press.animating(now)
    }

    /// The thumb's left edge and width inside the track at `now`. The
    /// stretch grows from the left edge when off and from the right when
    /// on, toward the center either way.
    pub fn thumb(&self, now: Instant) -> (f32, f32) {
        let travel = self.thumb.amount(now);
        let stretch = STRETCH * self.press.amount(now);
        (TRAVEL * travel - stretch * travel, THUMB + stretch)
    }
}

/// The switch for a value `on`, drawn as `motion` stands at `now`; the
/// caller toggles it on click.
pub fn switch(
    el: Stateful<Div>,
    label: impl Into<SharedString>,
    on: bool,
    motion: &SwitchMotion,
    now: Instant,
    focus: &FocusHandle,
    window: &Window,
) -> Stateful<Div> {
    let dark = theme::palette().dark;
    let primary = theme::accent();
    let input = theme::text().opacity(if dark { 0.08 } else { 0.10 });
    let ring = focus.is_focused(window) && window.last_input_was_keyboard();
    let (left, width) = motion.thumb(now);
    crate::a11y::control(el, Role::Switch, label)
        .track_focus(focus)
        // The ring below replaces the shell's focus border.
        .focus_visible(|s| s)
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .relative()
        .flex_none()
        // 30 by 18; the thumb sits 1 px in, coss ui's `p-px`.
        .border_0()
        .w(px(2.0 * THUMB - 2.0))
        .h(px(THUMB + 2.0))
        .rounded_full()
        .bg(input)
        // coss ui's `ring-2` 1 px off the pill.
        .when(ring, |d| {
            d.child(crate::input::halo(
                3.0,
                2.0,
                THUMB / 2.0 + 1.0,
                crate::input::ring_color(),
            ))
        })
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(primary.opacity(primary.a * motion.track.amount(now))),
        )
        .child(
            div()
                .absolute()
                .top(px(1.0))
                .left(px(1.0 + left))
                .w(px(width))
                .h(px(THUMB))
                .rounded_full()
                .bg(gpui::white())
                .shadow(vec![
                    BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.05),
                        offset: point(px(0.0), px(1.0)),
                        blur_radius: px(3.0),
                        spread_radius: px(0.0),
                        inset: false,
                    },
                    BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.05),
                        offset: point(px(0.0), px(1.0)),
                        blur_radius: px(2.0),
                        spread_radius: px(-1.0),
                        inset: false,
                    },
                ]),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_slides_twelve_pixels_and_stretches_toward_the_center() {
        let now = Instant::now();
        let mut motion = SwitchMotion::new(false);
        assert_eq!(motion.thumb(now), (0.0, 16.0));
        motion.sync(false, true, now, false);
        let (left, width) = motion.thumb(now + PRESS);
        assert_eq!(left, 0.0, "off, it grows from the left edge");
        assert!((width - 17.6).abs() < 1e-4);
        motion.sync(true, false, now + PRESS, false);
        assert!(motion.animating(now + PRESS + SLIDE / 2));
        assert_eq!(motion.thumb(now + PRESS * 2 + FADE), (12.0, 16.0));
        motion.sync(true, true, now + Duration::from_secs(1), true);
        let (left, width) = motion.thumb(now + Duration::from_secs(1));
        assert!(
            (left - 10.4).abs() < 1e-4,
            "on, it grows from the right edge"
        );
        assert!((width - 17.6).abs() < 1e-4);
    }

    #[test]
    fn reduced_motion_lands_at_once() {
        let now = Instant::now();
        let mut motion = SwitchMotion::new(false);
        motion.sync(true, false, now, true);
        assert!(!motion.animating(now));
        assert_eq!(motion.thumb(now), (12.0, 16.0));
    }
}
