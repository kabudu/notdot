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
