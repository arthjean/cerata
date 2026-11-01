//! Editor zoom (`lib/zoom.ts`): a fixed ladder from 50 to 300 %, stored in
//! `editor.zoom`; it multiplies the font size, which it never rewrites.

pub const ZOOM_KEY: &str = "editor.zoom";
pub const DEFAULT: f64 = 100.0;
pub const MIN: f64 = 50.0;
pub const MAX: f64 = 300.0;
pub const STEPS: [f64; 13] = [
    50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 125.0, 150.0, 175.0, 200.0, 250.0, 300.0,
];

/// A stored value as a zoom: anything not finite is the default, the rest
/// is clamped to the ladder's ends.
pub fn normalize(value: Option<f64>) -> f64 {
    match value {
        Some(v) if v.is_finite() => v.clamp(MIN, MAX),
        _ => DEFAULT,
    }
}

/// The first step above `current`, or the maximum.
pub fn zoom_in(current: Option<f64>) -> f64 {
    let current = normalize(current);
    STEPS
        .into_iter()
        .find(|step| *step > current)
        .unwrap_or(MAX)
}

/// The last step below `current`, or the minimum.
pub fn zoom_out(current: Option<f64>) -> f64 {
    let current = normalize(current);
    STEPS
        .into_iter()
        .rev()
        .find(|step| *step < current)
        .unwrap_or(MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from Writer `tests/zoom.test.ts:56-132`.

    #[test]
    fn the_ladder_spans_the_schema_bounds_and_holds_the_default() {
        assert_eq!(STEPS[0], MIN);
        assert_eq!(STEPS[STEPS.len() - 1], MAX);
        assert!(STEPS.contains(&DEFAULT));
        assert!(STEPS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn walks_the_ladder() {
        assert_eq!(zoom_in(Some(100.0)), 110.0);
        assert_eq!(zoom_in(Some(110.0)), 125.0);
        assert_eq!(zoom_out(Some(100.0)), 90.0);
        assert_eq!(zoom_out(Some(125.0)), 110.0);
    }

    #[test]
    fn clamps_at_the_ends() {
        assert_eq!(zoom_in(Some(1000.0)), 300.0);
        assert_eq!(zoom_out(Some(1.0)), 50.0);
    }

    #[test]
    fn snaps_off_ladder_values() {
        assert_eq!(zoom_in(Some(120.0)), 125.0);
        assert_eq!(zoom_out(Some(120.0)), 110.0);
        assert_eq!(zoom_in(Some(299.0)), 300.0);
    }

    #[test]
    fn unset_or_invalid_values_are_the_default() {
        assert_eq!(normalize(Some(f64::NAN)), 100.0);
        assert_eq!(normalize(None), 100.0);
        assert_eq!(zoom_in(None), 110.0);
        assert_eq!(zoom_out(None), 90.0);
    }
}
