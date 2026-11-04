//! Bounded follow-up scheduling. The capture worker owns the speech/silence timeout.
//! No timers survive cancellation, and only completed reply playback can arm a window.
pub(crate) const ECHO_GUARD_MS: f64 = 600.;
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Phase {
    #[default]
    Closed,
    Cooling {
        until: f64,
    },
    Starting,
    Listening,
}
#[derive(Default)]
pub(crate) struct Conversation {
    phase: Phase,
}
impl Conversation {
    pub(crate) fn after_reply(&mut self, now: f64, enabled: bool, playback_ended: bool) {
        self.phase = if enabled && playback_ended && now.is_finite() {
            Phase::Cooling {
                until: now + ECHO_GUARD_MS,
            }
        } else {
            Phase::Closed
        };
    }
    pub(crate) fn active(&self) -> bool {
        self.phase != Phase::Closed
    }
    pub(crate) fn listening(&self) -> bool {
        self.phase == Phase::Listening
    }
    pub(crate) fn starting(&self) -> bool {
        self.phase == Phase::Starting
    }
    pub(crate) fn begin_capture(&mut self) {
        self.phase = Phase::Starting;
    }
    pub(crate) fn opened(&mut self) {
        self.phase = Phase::Listening;
    }
    pub(crate) fn close(&mut self) {
        self.phase = Phase::Closed;
    }
    pub(crate) fn poll(&mut self, now: f64, allowed: bool) -> bool {
        if !allowed || !now.is_finite() {
            self.close();
            return false;
        }
        if matches!(self.phase, Phase::Cooling { until } if now >= until) {
            self.phase = Phase::Starting;
            true
        } else {
            false
        }
    }
}
/// Whole utterances only: quoted words, longer requests and embedded phrases remain messages.
pub(crate) fn is_goodbye(text: &str) -> bool {
    if text.len() > 128 {
        return false;
    }
    let text = text
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .replace('’', "'")
        .to_lowercase();
    matches!(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
        "that's all" | "thats all" | "that is all"
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_complete_playback_opt_in_and_echo_guard_then_opens_once() {
        let mut c = Conversation::default();
        for (enabled, ended) in [(false, true), (true, false), (false, false)] {
            c.after_reply(100., enabled, ended);
            assert!(!c.poll(10000., true));
        }
        c.after_reply(100., true, true);
        assert!(!c.poll(699., true));
        assert!(c.poll(700., true));
        assert!(c.starting());
        assert!(!c.poll(900., true));
        c.opened();
        assert!(c.listening());
        assert!(!c.poll(2000., true));
        c.close();
        assert!(!c.active());
        assert!(!c.poll(3000., true));
    }
    #[test]
    fn settings_mute_hide_errors_and_dialogs_cancel_without_delayed_reopening() {
        for now in [0., 599., 600., 1000.] {
            let mut c = Conversation::default();
            c.after_reply(0., true, true);
            assert!(!c.poll(now, false));
            assert!(!c.poll(10000., true));
        }
        let mut c = Conversation::default();
        c.after_reply(f64::NAN, true, true);
        assert!(!c.active());
        c.after_reply(0., true, true);
        assert!(!c.poll(f64::NAN, true));
        assert!(!c.active());
    }
    #[test]
    fn goodbye_is_exact_and_never_eats_an_ordinary_request() {
        for text in [
            "That's all",
            "THAT’S ALL.",
            "  that's   all! ",
            "Thats all",
            "that is all",
        ] {
            assert!(is_goodbye(text), "{text}");
        }
        for text in [
            "",
            "That's all I remember",
            "Remember the phrase that's all",
            "Is that all?",
            "Say that's all",
            "that's all, and one more thing",
        ] {
            assert!(!is_goodbye(text), "{text}");
        }
    }
}
