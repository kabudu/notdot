//! Orb colour maths shared by the desktop WebGL renderer and the mobile
//! adapters. Native code never re-implements any of this.

/// Saved themes are a hue in degrees. Matches `notdot_core::settings`.
pub const DEFAULT_HUE: u16 = 192;
/// Largest hue a saved setting may hold; 360 wraps to 0.
pub const MAX_HUE: u16 = 360;

/// Shader tint for a hue: a fully saturated colour lifted so the darkest
/// channel stays at 0.42. Hues wrap, so -120 and 240 are the same colour.
pub fn tint(hue: f64) -> [f32; 3] {
    let hue = if hue.is_finite() { hue } else { 0. };
    let h = hue.rem_euclid(360.) / 60.;
    let x = 1. - (h.rem_euclid(2.) - 1.).abs();
    let rgb = match h as u32 {
        0 => [1., x, 0.],
        1 => [x, 1., 0.],
        2 => [0., 1., x],
        3 => [0., x, 1.],
        4 => [x, 0., 1.],
        _ => [1., 0., x],
    };
    rgb.map(|v| (0.42 + v * 0.58) as f32)
}

/// Blend a tint towards the warning red by `error` (0 to 1).
pub fn error_blend(tint: [f32; 3], error: f64) -> [f32; 3] {
    let error = if error.is_finite() {
        error.clamp(0., 1.) as f32
    } else {
        0.
    };
    [
        tint[0] * (1. - error) + error,
        tint[1] * (1. - error) + 0.3 * error,
        tint[2] * (1. - error) + 0.3 * error,
    ]
}

/// Mobile "computer offline" presentation: keep the orb but drain most of
/// its colour, so state is not carried by hue alone. Desktop never sets it.
pub fn offline_blend(tint: [f32; 3]) -> [f32; 3] {
    let luma = 0.2126 * tint[0] + 0.7152 * tint[1] + 0.0722 * tint[2];
    tint.map(|channel| luma + (channel - luma) * 0.15)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn themes_wrap_and_stay_bounded() {
        assert_eq!(tint(0.), tint(360.));
        assert_eq!(tint(-120.), tint(240.));
        assert_eq!(tint(f64::NAN), tint(0.));
        for hue in 0..360 {
            assert!(tint(f64::from(hue)).iter().all(|v| (0.42..=1.).contains(v)));
        }
    }
    #[test]
    fn error_blend_reaches_warning_red_and_ignores_bad_input() {
        let calm = tint(f64::from(DEFAULT_HUE));
        assert_eq!(error_blend(calm, 0.), calm);
        assert_eq!(error_blend(calm, f64::NAN), calm);
        assert_eq!(error_blend(calm, 1.), [1., 0.3, 0.3]);
        assert_eq!(error_blend(calm, 7.), [1., 0.3, 0.3]);
    }
    #[test]
    fn offline_keeps_brightness_but_removes_most_colour() {
        let [r, g, b] = offline_blend(tint(f64::from(DEFAULT_HUE)));
        let spread = r.max(g).max(b) - r.min(g).min(b);
        assert!(spread < 0.1, "{spread}");
        assert!(r > 0.4 && g > 0.4 && b > 0.4);
    }
}
