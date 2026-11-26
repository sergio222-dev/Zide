use std::time::{Duration, Instant};

/// Detects a double press of Shift alone (ADR-010): press, release, press again
/// within `interval`, with no other key or modifier in between.
///
/// Fires on the second press, like IntelliJ's `ModifierKeyDoubleClickHandler`
/// (to be checked against its test in the spike).
#[derive(Debug, Clone)]
pub struct DoubleShiftDetector {
    interval: Duration,
    state: State,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    FirstDown,
    FirstUp(Instant),
}

impl DoubleShiftDetector {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(300);

    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            state: State::Idle,
        }
    }

    /// Feeds a modifier-state change. `shift` is the new Shift state and
    /// `others` tells whether Ctrl, Alt or Super are down. Returns `true` on a double Shift.
    pub fn modifiers_changed(&mut self, shift: bool, others: bool, now: Instant) -> bool {
        if others {
            self.state = State::Idle;
            return false;
        }
        match (self.state, shift) {
            (State::Idle, true) => self.state = State::FirstDown,
            (State::FirstDown, false) => self.state = State::FirstUp(now),
            (State::FirstUp(released), true) => {
                if now.duration_since(released) <= self.interval {
                    self.state = State::Idle;
                    return true;
                }
                self.state = State::FirstDown;
            }
            _ => {}
        }
        false
    }

    /// Any non-modifier key cancels a detection in progress.
    pub fn key_pressed(&mut self) {
        self.state = State::Idle;
    }
}

impl Default for DoubleShiftDetector {
    fn default() -> Self {
        Self::new(Self::DEFAULT_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn fires_on_second_press() {
        let (mut d, t0) = (DoubleShiftDetector::default(), Instant::now());
        assert!(!d.modifiers_changed(true, false, at(t0, 0)));
        assert!(!d.modifiers_changed(false, false, at(t0, 50)));
        assert!(d.modifiers_changed(true, false, at(t0, 150)));
    }

    #[test]
    fn slow_second_press_does_not_fire_but_starts_over() {
        let (mut d, t0) = (DoubleShiftDetector::default(), Instant::now());
        d.modifiers_changed(true, false, at(t0, 0));
        d.modifiers_changed(false, false, at(t0, 50));
        assert!(!d.modifiers_changed(true, false, at(t0, 500)));
        d.modifiers_changed(false, false, at(t0, 550));
        assert!(d.modifiers_changed(true, false, at(t0, 600)));
    }

    #[test]
    fn other_key_in_between_cancels() {
        let (mut d, t0) = (DoubleShiftDetector::default(), Instant::now());
        d.modifiers_changed(true, false, at(t0, 0));
        d.key_pressed(); // Shift+letter
        d.modifiers_changed(false, false, at(t0, 50));
        assert!(!d.modifiers_changed(true, false, at(t0, 100)));
    }

    #[test]
    fn other_modifier_cancels() {
        let (mut d, t0) = (DoubleShiftDetector::default(), Instant::now());
        d.modifiers_changed(true, false, at(t0, 0));
        d.modifiers_changed(true, true, at(t0, 10)); // Ctrl joins
        d.modifiers_changed(false, false, at(t0, 50));
        assert!(!d.modifiers_changed(true, false, at(t0, 100)));
    }

    #[test]
    fn does_not_fire_twice_in_a_row() {
        let (mut d, t0) = (DoubleShiftDetector::default(), Instant::now());
        d.modifiers_changed(true, false, at(t0, 0));
        d.modifiers_changed(false, false, at(t0, 50));
        assert!(d.modifiers_changed(true, false, at(t0, 100)));
        d.modifiers_changed(false, false, at(t0, 150));
        assert!(!d.modifiers_changed(true, false, at(t0, 200)));
    }
}
