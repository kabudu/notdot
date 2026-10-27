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
            "thinking" | "connecting" | "transcribing" | "synthesizing" | "updating" => {
                Self::Thinking
            }
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
