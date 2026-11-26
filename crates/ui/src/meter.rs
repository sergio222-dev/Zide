//! A debug overlay with the frame rate: start Zide with `ZIDE_FPS=1`.
//!
//! The window redraws only when something changes, so at rest there are no frames and the
//! numbers are those of the last redraw. Read them while scrolling or holding a movement key.
//! The rate is that of the burst of frames in progress (a 160 ms scroll glide is one burst), not
//! an average over a second that is mostly idle.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(1);
/// A longer wait than this between two frames ends a burst.
const BURST_GAP: Duration = Duration::from_millis(100);

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

    /// Frames per second of the burst in progress: the frames since the last wait longer than
    /// `BURST_GAP`, over the time they span. `None` for a single frame.
    fn burst_fps(&self) -> Option<f64> {
        let mut count = 1;
        let mut first = self.frames.back()?.0;
        for (at, _) in self.frames.iter().rev().skip(1) {
            if first.duration_since(*at) > BURST_GAP {
                break;
            }
            first = *at;
            count += 1;
        }
        let span = self.frames.back()?.0.duration_since(first).as_secs_f64();
        (count > 1 && span > 0.).then(|| (count - 1) as f64 / span)
    }

    /// `62 fps | 11 frames | worst gap 21.3 ms | build 0.8 ms (max 1.9)`: the rate of the
    /// burst in progress, the frames in the last second, the longest wait between two of them
    /// and the time spent building the view.
    pub fn label(&self) -> String {
        let worst_gap = self
            .frames
            .iter()
            .zip(self.frames.iter().skip(1))
            .map(|((a, _), (b, _))| b.duration_since(*a))
            .filter(|gap| *gap <= BURST_GAP)
            .max()
            .unwrap_or_default();
        let fps = self
            .burst_fps()
            .map_or("-".to_string(), |fps| format!("{fps:.0}"));
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
            "{fps} fps | {} frames | worst gap {} | build {} (max {})",
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
            "48 fps | 6 frames | worst gap 40.0 ms | build 0.8 ms (max 0.8)"
        );
    }

    #[test]
    fn frames_older_than_a_second_are_forgotten() {
        let mut meter = meter();
        let t0 = Instant::now();
        meter.frame(t0, Duration::ZERO);
        meter.frame(t0 + Duration::from_millis(500), Duration::ZERO);
        meter.frame(t0 + Duration::from_millis(1_600), Duration::ZERO);
        assert!(meter.label().contains("1 frames"));
    }

    #[test]
    fn the_rate_is_that_of_the_burst_not_of_the_whole_second() {
        let mut meter = meter();
        let t0 = Instant::now();
        // a 160 ms glide at 100 Hz (17 frames), after half a second without frames
        meter.frame(t0, Duration::ZERO);
        for i in 0..=16 {
            meter.frame(t0 + Duration::from_millis(500 + i * 10), Duration::ZERO);
        }
        let label = meter.label();
        assert!(label.starts_with("100 fps | 18 frames"), "{label}");
    }
}
