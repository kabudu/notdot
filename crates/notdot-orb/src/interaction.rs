/// Tracks a gesture across native window dragging, where local coordinates can return to the origin.
#[derive(Default)]
pub(crate) struct Gesture {
    origin: (i32, i32),
    dragged: bool,
}
impl Gesture {
    pub(crate) fn press(&mut self, x: i32, y: i32) {
        self.origin = (x, y);
        self.dragged = false;
    }
    pub(crate) fn moved(&mut self, x: i32, y: i32, primary_down: bool) -> bool {
        if primary_down && !self.dragged && self.distance(x, y) > 6 {
            self.dragged = true;
            true
        } else {
            false
        }
    }
    pub(crate) fn activates(&self, x: i32, y: i32, button: i16, control: bool) -> bool {
        button == 0 && !control && !self.dragged && self.distance(x, y) <= 6
    }
    fn distance(&self, x: i32, y: i32) -> i64 {
        (i64::from(x) - i64::from(self.origin.0)).abs()
            + (i64::from(y) - i64::from(self.origin.1)).abs()
    }
}
#[cfg(test)]
mod tests {
    use super::Gesture;
    #[test]
    fn moving_window_never_activates_microphone_even_if_local_coordinates_return() {
        let mut gesture = Gesture::default();
        gesture.press(180, 180);
        assert!(gesture.moved(200, 180, true));
        assert!(!gesture.moved(220, 180, true));
        assert!(!gesture.activates(180, 180, 0, false));
        gesture.press(180, 180);
        assert!(gesture.activates(181, 180, 0, false));
        assert!(!gesture.activates(180, 180, 2, false));
        assert!(!gesture.activates(180, 180, 0, true));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyboardAction {
    Settings,
    Voice,
    Cancel,
}
pub(crate) fn keyboard_action(
    key: &str,
    repeat: bool,
    command: bool,
    alt: bool,
    modal: bool,
    debug: bool,
    wake: bool,
) -> Option<KeyboardAction> {
    if repeat {
        return None;
    }
    if key == "Escape" {
        return Some(KeyboardAction::Cancel);
    }
    if command && key == "," && !modal {
        return Some(KeyboardAction::Settings);
    }
    if !wake && !modal && !command && !alt && (key == " " || (key == "Enter" && !debug)) {
        return Some(KeyboardAction::Voice);
    }
    None
}
#[cfg(test)]
mod keyboard_tests {
    use super::*;
    #[test]
    fn wake_mode_never_starts_manual_capture_but_retains_settings_and_cancel() {
        for key in [" ", "Enter"] {
            assert_eq!(
                keyboard_action(key, false, false, false, false, false, true),
                None
            );
        }
        assert_eq!(
            keyboard_action("Escape", false, false, false, false, false, true),
            Some(KeyboardAction::Cancel)
        );
        assert_eq!(
            keyboard_action(",", false, true, false, false, false, true),
            Some(KeyboardAction::Settings)
        );
    }
    #[test]
    fn orb_is_keyboard_operable_and_modal_buttons_never_start_recording() {
        for key in [" ", "Enter"] {
            assert_eq!(
                keyboard_action(key, false, false, false, false, false, false),
                Some(KeyboardAction::Voice)
            );
            assert_eq!(
                keyboard_action(key, false, false, false, true, false, false),
                None
            );
            assert_eq!(
                keyboard_action(key, true, false, false, false, false, false),
                None
            );
            assert_eq!(
                keyboard_action(key, false, true, false, false, false, false),
                None
            );
            assert_eq!(
                keyboard_action(key, false, false, true, false, false, false),
                None
            );
        }
        assert_eq!(
            keyboard_action(",", false, true, false, false, false, false),
            Some(KeyboardAction::Settings)
        );
        assert_eq!(
            keyboard_action(",", false, true, false, true, false, false),
            None
        );
        assert_eq!(
            keyboard_action("Escape", false, false, false, true, false, false),
            Some(KeyboardAction::Cancel)
        );
        assert_eq!(
            keyboard_action("Enter", false, false, false, false, true, false),
            None
        );
    }
}

/// A bounded local end-of-speech detector; it never transcribes background audio.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CaptureDecision {
    Continue,
    Submit,
    Discard,
}
pub(crate) struct SpeechEnd {
    started: f64,
    last_sample: f64,
    last_voice: f64,
    voiced_ms: f64,
}
impl SpeechEnd {
    pub(crate) fn new(now: f64) -> Self {
        Self {
            started: now,
            last_sample: now,
            last_voice: now,
            voiced_ms: 0.,
        }
    }
    pub(crate) fn heard_speech(&self) -> bool {
        self.voiced_ms >= 180.
    }
    pub(crate) fn observe(&mut self, now: f64, rms: f64, metered: bool) -> CaptureDecision {
        let elapsed = (now - self.last_sample).clamp(0., 100.);
        self.last_sample = now;
        if metered && rms.is_finite() && rms >= 0.015 {
            self.voiced_ms += elapsed;
            self.last_voice = now;
        }
        if self.heard_speech() && now - self.last_voice >= 1400. {
            CaptureDecision::Submit
        } else if now - self.started >= 8000. && !self.heard_speech() {
            CaptureDecision::Discard
        } else {
            CaptureDecision::Continue
        }
    }
}
#[cfg(test)]
mod speech_end_tests {
    use super::*;
    #[test]
    fn speech_submits_after_a_pause_and_a_short_pause_does_not_cut_off_words() {
        let mut capture = SpeechEnd::new(0.);
        for tick in 1..=12 {
            assert_eq!(
                capture.observe(tick as f64 * 33., 0.06, true),
                CaptureDecision::Continue
            );
        }
        assert!(capture.heard_speech());
        assert_eq!(capture.observe(1000., 0., true), CaptureDecision::Continue);
        assert_eq!(
            capture.observe(1100., 0.05, true),
            CaptureDecision::Continue
        );
        assert_eq!(capture.observe(2400., 0., true), CaptureDecision::Continue);
        assert_eq!(capture.observe(2500., 0., true), CaptureDecision::Submit);
    }
    #[test]
    fn silence_clicks_and_missing_meter_are_discarded_without_a_paid_request() {
        for metered in [true, false] {
            let mut capture = SpeechEnd::new(0.);
            for tick in 1..240 {
                assert_eq!(
                    capture.observe(tick as f64 * 33., if tick == 3 { 0.1 } else { 0. }, metered),
                    CaptureDecision::Continue
                );
            }
            assert!(!capture.heard_speech());
            assert_eq!(
                capture.observe(8000., 0., metered),
                CaptureDecision::Discard
            );
        }
    }
}
