//! A debug overlay with the frame rate: start Zide with `ZIDE_FPS=1`.
//!
//! The window redraws only when something changes, so at rest there are no frames and the
//! numbers are those of the last redraw. Read them while scrolling or holding a movement key.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(1);

pub struct FrameMeter {
    /// When each frame of the last second began, and how long building it took.
    frames: VecDeque<(Instant, Duration)>,
}

impl FrameMeter {
    /// `Some` when `ZIDE_FPS` is set to something other than `0`.
    pub fn from_env() -> Option<Self> {
        Self::enabled(std::env::var("ZIDE_FPS").ok().as_deref()).then(|| Self {
            frames: VecDeque::new(),
        })
    }

    fn enabled(value: Option<&str>) -> bool {
        value.is_some_and(|v| !v.is_empty() && v != "0")
    }

    /// Records a frame that began at `start` and took `build` to build.
    pub fn frame(&mut self, start: Instant, build: Duration) {
        self.frames.push_back((start, build));
        while self
            .frames
            .front()
            .is_some_and(|(at, _)| start.duration_since(*at) > WINDOW)
        {
            self.frames.pop_front();
        }
    }

    /// `58 fps | worst gap 21.3 ms | build 0.8 ms (max 1.9)`: frames in the last second, the
    /// longest wait between two of them, and the time spent building the view.
    pub fn label(&self) -> String {
        let worst_gap = self
            .frames
            .iter()
            .zip(self.frames.iter().skip(1))
            .map(|((a, _), (b, _))| b.duration_since(*a))
            .max()
            .unwrap_or_default();
        let last = self
            .frames
            .back()
            .map(|(_, build)| *build)
            .unwrap_or_default();
        let slowest = self
            .frames
            .iter()
            .map(|(_, b)| *b)
            .max()
            .unwrap_or_default();
        format!(
            "{} fps | worst gap {} | build {} (max {})",
            self.frames.len(),
            ms(worst_gap),
            ms(last),
            ms(slowest).trim_end_matches(" ms"),
        )
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter() -> FrameMeter {
        FrameMeter {
            frames: VecDeque::new(),
        }
    }

    #[test]
    fn only_a_set_variable_other_than_zero_turns_it_on() {
        assert!(!FrameMeter::enabled(None));
        assert!(!FrameMeter::enabled(Some("")));
        assert!(!FrameMeter::enabled(Some("0")));
        assert!(FrameMeter::enabled(Some("1")));
    }

    #[test]
    fn counts_the_frames_of_the_last_second_and_the_longest_gap() {
        let mut meter = meter();
        let t0 = Instant::now();
        let build = Duration::from_micros(800);
        // one frame at 0 ms, then every 16 ms, with a 40 ms stall in the middle
        let mut at = 0;
        for step in [0, 16, 16, 40, 16, 16] {
            at += step;
            meter.frame(t0 + Duration::from_millis(at), build);
        }
        assert_eq!(
            meter.label(),
            "6 fps | worst gap 40.0 ms | build 0.8 ms (max 0.8)"
        );
    }

    #[test]
    fn frames_older_than_a_second_are_forgotten() {
        let mut meter = meter();
        let t0 = Instant::now();
        meter.frame(t0, Duration::ZERO);
        meter.frame(t0 + Duration::from_millis(500), Duration::ZERO);
        meter.frame(t0 + Duration::from_millis(1_600), Duration::ZERO);
        assert!(meter.label().starts_with("1 fps"));
    }
}
