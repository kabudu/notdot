#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Idle,
    Listening,
    Thinking,
    Speaking,
    Error,
}
impl Mode {
    pub(crate) fn resolve(status: &str, playing: bool) -> Self {
        if playing {
            return Self::Speaking;
        }
        match status {
            "listening" => Self::Listening,
            "announcing" => Self::Speaking,
            "thinking" | "connecting" | "transcribing" | "synthesizing" | "updating"
            | "resetting" => Self::Thinking,
            "error" => Self::Error,
            _ => Self::Idle,
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Error => "error",
        }
    }
}
pub(crate) fn rms(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.;
    }
    (bytes
        .iter()
        .map(|b| ((f64::from(*b) - 128.) / 128.).powi(2))
        .sum::<f64>()
        / bytes.len() as f64)
        .sqrt()
        .min(1.)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_waiting_stages_are_visibly_thinking_and_playback_overrides_idle() {
        for stage in ["thinking", "connecting", "transcribing", "synthesizing"] {
            assert_eq!(Mode::resolve(stage, false), Mode::Thinking);
        }
        assert_eq!(Mode::resolve("transcribing", false).name(), "thinking");
        assert_eq!(Mode::resolve("idle", true), Mode::Speaking);
        assert_eq!(Mode::resolve("idle", false), Mode::Idle);
        assert_eq!(Mode::resolve("listening", false), Mode::Listening);
    }
    #[test]
    fn audio_meter_distinguishes_silence_from_known_waveform() {
        assert_eq!(rms(&[128; 256]), 0.);
        assert_eq!(rms(&[96, 160, 96, 160]), 0.25);
        assert!(rms(&[0, 255]) <= 1.);
    }
}

/// Follow speech energy with a soft attack and a longer release, in seconds.
pub(crate) fn smooth_speech(current: f64, target: f64, seconds: f64) -> f64 {
    let current = if current.is_finite() {
        current.clamp(0., 1.)
    } else {
        0.
    };
    let target = if target.is_finite() {
        target.clamp(0., 1.)
    } else {
        0.
    };
    let tau = if target > current { 0.10 } else { 0.32 };
    current + (target - current) * (1. - (-seconds.clamp(0., 0.1) / tau).exp())
}
/// A closed, low-frequency shell: broad waves rather than individual audio spikes.
pub(crate) fn speaking_shell(
    angle: f64,
    time: f64,
    energy: f64,
    layer: usize,
    reduced: bool,
) -> f64 {
    let base = 1.82 + layer as f64 * 0.11;
    if reduced {
        return base;
    }
    let phase = layer as f64 * 0.8;
    base + energy * 0.065
        + 0.016 * (time * 1.6 + phase).sin()
        + (0.3 + energy * 0.7)
            * (0.022 * (angle * 2. + time * 0.9 + phase).sin()
                + 0.012 * (angle * 3. - time * 0.55 + phase).sin())
}

#[cfg(test)]
mod speech_tests {
    use super::*;
    #[test]
    fn envelope_softens_attack_and_release_without_overshooting() {
        let attack = smooth_speech(0., 1., 1. / 30.);
        assert!(attack > 0.2 && attack < 0.4);
        let release = smooth_speech(1., 0., 1. / 30.);
        assert!(release > 0.8 && release < 1.);
        let mut value = 0.;
        for _ in 0..300 {
            value = smooth_speech(value, 1., 1. / 30.);
            assert!((0. ..=1.).contains(&value));
        }
        assert!((value - 1.).abs() < 1e-9);
    }
    #[test]
    fn shell_is_closed_bounded_and_static_with_reduced_motion() {
        use std::f64::consts::TAU;
        for layer in 0..2 {
            for step in 0..200 {
                let t = step as f64 * 0.17;
                for energy in [0., 0.5, 1.] {
                    assert!(
                        (speaking_shell(0., t, energy, layer, false)
                            - speaking_shell(TAU, t, energy, layer, false))
                        .abs()
                            < 1e-12
                    );
                    for i in 0..180 {
                        let a = i as f64 / 180. * TAU;
                        let r = speaking_shell(a, t, energy, layer, false);
                        assert!((1.7..2.1).contains(&r));
                        assert!(
                            (r - speaking_shell(a + 0.01, t, energy, layer, false)).abs() < 0.002
                        );
                    }
                }
                assert_eq!(
                    speaking_shell(1., t, 1., layer, true),
                    speaking_shell(0., 0., 0., layer, true)
                );
            }
        }
    }
}
