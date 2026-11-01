//! Sections and folders opening and closing.
//!
//! A section's rows grow from their header and fade in over the filter's
//! 180 ms cubic ease-out, and shrink back the same way; a toggle mid-flight
//! retargets from what is shown. Reduced motion keeps the fade and drops the
//! growth. A folder is opened often, so its rows only fade in, over 150 ms,
//! once its listing is read; it closes at once.

use std::time::{Duration, Instant};

use super::filter::{MOTION, ease_out_cubic};

pub const SECTION_MOTION: Duration = MOTION;
pub const FOLDER_MOTION: Duration = Duration::from_millis(150);

/// How far content is shown, from 0 (gone) to 1 (open).
#[derive(Clone, Copy, Debug)]
pub struct Reveal {
    from: f32,
    to: f32,
    started: Option<Instant>,
    duration: Duration,
}

impl Reveal {
    /// Settled open or closed.
    pub fn new(open: bool, duration: Duration) -> Self {
        let at = if open { 1. } else { 0. };
        Self {
            from: at,
            to: at,
            started: None,
            duration,
        }
    }

    pub fn amount(&self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.to;
        };
        let progress = (now.saturating_duration_since(started).as_secs_f32()
            / self.duration.as_secs_f32())
        .min(1.);
        self.from + (self.to - self.from) * ease_out_cubic(progress)
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.started
            .is_some_and(|started| now.saturating_duration_since(started) < self.duration)
    }

    /// Heads open or closed from what is shown at `now`.
    pub fn retarget(&mut self, open: bool, now: Instant) {
        self.from = self.amount(now);
        self.to = if open { 1. } else { 0. };
        self.started = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_out_and_settles() {
        let now = Instant::now();
        let mut reveal = Reveal::new(false, SECTION_MOTION);
        reveal.retarget(true, now);
        assert_eq!(reveal.amount(now), 0.);
        // Halfway in time, the ease is 7/8 of the way.
        assert!((reveal.amount(now + SECTION_MOTION / 2) - 0.875).abs() < 0.001);
        assert!(reveal.animating(now + SECTION_MOTION / 2));
        assert_eq!(reveal.amount(now + SECTION_MOTION), 1.);
        assert!(!reveal.animating(now + SECTION_MOTION));
    }

    #[test]
    fn a_toggle_mid_flight_reverses_from_what_is_shown() {
        let now = Instant::now();
        let mut reveal = Reveal::new(true, SECTION_MOTION);
        reveal.retarget(false, now);
        let midway = now + Duration::from_millis(60);
        let shown = reveal.amount(midway);
        assert!(shown > 0. && shown < 1.);
        reveal.retarget(true, midway);
        assert_eq!(reveal.amount(midway), shown);
        assert!(reveal.amount(midway + SECTION_MOTION / 2) > shown);
        assert_eq!(reveal.amount(midway + SECTION_MOTION), 1.);
    }
}
