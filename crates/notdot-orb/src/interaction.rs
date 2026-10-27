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
    if !modal && !command && !alt && (key == " " || (key == "Enter" && !debug)) {
        return Some(KeyboardAction::Voice);
    }
    None
}
#[cfg(test)]
mod keyboard_tests {
    use super::*;
    #[test]
    fn orb_is_keyboard_operable_and_modal_buttons_never_start_recording() {
        for key in [" ", "Enter"] {
            assert_eq!(
                keyboard_action(key, false, false, false, false, false),
                Some(KeyboardAction::Voice)
            );
            assert_eq!(keyboard_action(key, false, false, false, true, false), None);
            assert_eq!(keyboard_action(key, true, false, false, false, false), None);
            assert_eq!(keyboard_action(key, false, true, false, false, false), None);
            assert_eq!(keyboard_action(key, false, false, true, false, false), None);
        }
        assert_eq!(
            keyboard_action(",", false, true, false, false, false),
            Some(KeyboardAction::Settings)
        );
        assert_eq!(keyboard_action(",", false, true, false, true, false), None);
        assert_eq!(
            keyboard_action("Escape", false, false, false, true, false),
            Some(KeyboardAction::Cancel)
        );
        assert_eq!(
            keyboard_action("Enter", false, false, false, false, true),
            None
        );
    }
}
