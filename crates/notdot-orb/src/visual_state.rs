#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Idle,
    Listening,
    Followup,
    Thinking,
    Speaking,
    Error,
}
impl Mode {
    pub(crate) fn resolve(status: &str, playing: bool, waiting: bool) -> Self {
        if playing {
            return Self::Speaking;
        }
        if status == "idle" && waiting {
            return Self::Thinking;
        }
        match status {
            "listening" => Self::Listening,
            "followup" => Self::Followup,
            "announcing" => Self::Speaking,
            "thinking" | "connecting" | "starting_capture" | "transcribing" | "synthesizing"
            | "updating" | "resetting" => Self::Thinking,
            "error" => Self::Error,
            _ => Self::Idle,
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Followup => "followup",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Error => "error",
        }
    }
}
/// Whether a native wake capture should still show as listening. After the UI
/// ends a capture, status can report that same capture for one more poll.
pub(crate) fn native_capture_live(phase: &str, token: Option<&str>, ended: Option<&str>) -> bool {
    phase == "capturing" && ended.is_none_or(|ended| token != Some(ended))
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
        for stage in [
            "thinking",
            "connecting",
            "starting_capture",
            "transcribing",
            "synthesizing",
        ] {
            assert_eq!(Mode::resolve(stage, false, false), Mode::Thinking);
        }
        assert_eq!(
            Mode::resolve("transcribing", false, false).name(),
            "thinking"
        );
        assert_eq!(Mode::resolve("idle", true, false), Mode::Speaking);
        assert_eq!(Mode::resolve("idle", false, false), Mode::Idle);
        assert_eq!(Mode::resolve("idle", false, true), Mode::Thinking);
        assert_eq!(Mode::resolve("error", false, true), Mode::Error);
        assert_eq!(Mode::resolve("idle", true, true), Mode::Speaking);
        assert_eq!(Mode::resolve("listening", false, false), Mode::Listening);
        assert_eq!(Mode::resolve("followup", false, false), Mode::Followup);
        assert_eq!(Mode::resolve("followup", true, false), Mode::Speaking);
        assert_eq!(Mode::Followup.name(), "followup");
    }
    #[test]
    fn an_ended_capture_does_not_flash_listening_before_idle() {
        assert!(native_capture_live("capturing", Some("a"), None));
        assert!(native_capture_live("capturing", None, None));
        // The follow-up window that just timed out is still reported once.
        assert!(!native_capture_live("capturing", Some("a"), Some("a")));
        // A new capture after it shows immediately.
        assert!(native_capture_live("capturing", Some("b"), Some("a")));
        assert!(native_capture_live("capturing", None, Some("a")));
        assert!(!native_capture_live("ready", Some("b"), None));
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
}

/// Smooth transitions share one bounded clock, so returning from a hidden
/// window never jumps a surface ribbon or changes the orb's size abruptly.
#[derive(Default)]
pub(crate) struct OrbMotion {
    pub phase: f64,
    pub listening: f64,
    pub followup: f64,
    pub thinking: f64,
    pub speaking: f64,
    pub error: f64,
}
impl OrbMotion {
    pub fn advance(&mut self, mode: Mode, seconds: f64, reduced: bool) {
        let seconds = if seconds.is_finite() {
            seconds.clamp(0., 0.1)
        } else {
            0.
        };
        let ease = 1. - (-seconds / 0.24).exp();
        for (value, active) in [
            (&mut self.listening, mode == Mode::Listening),
            (&mut self.followup, mode == Mode::Followup),
            (&mut self.thinking, mode == Mode::Thinking),
            (&mut self.speaking, mode == Mode::Speaking),
            (&mut self.error, mode == Mode::Error),
        ] {
            *value += (f64::from(active) - *value) * ease;
        }
        if !reduced {
            self.phase += seconds * (0.18 + self.thinking * 0.5 + self.speaking * 0.24);
        }
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    #[test]
    fn transitions_ease_without_jumps_and_pause_motion_when_requested() {
        let mut motion = OrbMotion::default();
        motion.advance(Mode::Thinking, 1. / 30., false);
        assert!(motion.thinking > 0. && motion.thinking < 0.2);
        let phase = motion.phase;
        motion.advance(Mode::Speaking, 30., true);
        assert_eq!(motion.phase, phase);
        assert!(motion.speaking > 0. && motion.speaking < 0.4);
        motion.advance(Mode::Error, f64::NAN, false);
        assert_eq!(motion.phase, phase);
        for _ in 0..300 {
            motion.advance(Mode::Followup, 1. / 30., false);
        }
        assert!(motion.followup > 0.999);
        assert!(motion.speaking < 0.001);
    }
}

/// Keep a rendering deadline rather than sleeping a full interval after each
/// delivered frame. The latter halves cadence on 30-32 Hz remote displays.
#[derive(Default)]
pub(crate) struct FrameClock {
    deadline: Option<f64>,
    previous: Option<f64>,
    reduced: bool,
}
impl FrameClock {
    pub fn tick(&mut self, now: f64, reduced: bool) -> Option<f64> {
        if !now.is_finite() || now < 0. {
            return None;
        }
        let interval = if reduced { 100. } else { 1000. / 30. };
        let reset = self.previous.is_none_or(|previous| now < previous) || self.reduced != reduced;
        if !reset && self.deadline.is_some_and(|deadline| now + 0.25 < deadline) {
            return None;
        }
        let seconds = if reset {
            0.
        } else {
            (now - self.previous.unwrap_or(now)) / 1000.
        };
        let next = self.deadline.unwrap_or(now) + interval;
        self.deadline = Some(if reset || now - next > interval {
            now + interval
        } else {
            next
        });
        self.previous = Some(now);
        self.reduced = reduced;
        Some(seconds)
    }
}
#[cfg(test)]
mod frame_clock_tests {
    use super::*;
    #[test]
    fn near_target_displays_do_not_halve_rendering_cadence() {
        for source in [10, 24, 30, 32, 60, 120] {
            let mut clock = FrameClock::default();
            let rendered = (0..source * 2)
                .filter(|frame| {
                    clock
                        .tick(f64::from(*frame) * 1000. / f64::from(source), false)
                        .is_some()
                })
                .count();
            let expected = source.min(30) * 2;
            assert!(
                rendered.abs_diff(expected as usize) <= 1,
                "{source} Hz source delivered {rendered} frames in two seconds"
            );
        }
    }
    #[test]
    fn reduced_motion_and_long_hidden_gaps_do_not_create_catch_up_bursts() {
        let mut clock = FrameClock::default();
        let rendered = (0..120)
            .filter(|frame| clock.tick(f64::from(*frame) * 1000. / 60., true).is_some())
            .count();
        assert_eq!(rendered, 20);
        assert!(clock.tick(30_000., true).is_some());
        assert!(clock.tick(30_001., true).is_none());
        assert!(clock.tick(30_002., false).is_some());
        assert!(clock.tick(f64::NAN, false).is_none());
        assert!(clock.tick(f64::INFINITY, false).is_none());
    }
}
