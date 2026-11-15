//! A fixed-timestep accumulator: the time each display frame brings is
//! spent in whole steps of a fixed `dt`, so a simulation runs the same at
//! 60 Hz, 144 Hz or a stuttering 40.

use std::time::Duration;

/// Spends frame time in fixed steps.
///
/// Each frame, [`advance`](Self::advance) adds the frame's `dt` and returns
/// how many fixed steps are due; the remainder is kept for the next frame,
/// and [`alpha`](Self::alpha) says how far the simulation is between its
/// last two states, for drawing in between.
///
/// ```
/// use stipple::game::FixedStep;
/// use std::time::Duration;
///
/// let mut step = FixedStep::hz(60);
/// let (mut prev, mut pos) = (0.0, 0.0);
/// // A 25 ms frame: one step of 16.7 ms, 8.3 ms left over.
/// for _ in 0..step.advance(Duration::from_millis(25)) {
///     prev = pos;
///     pos += 120.0 * step.dt_secs(); // 120 px/s
/// }
/// let drawn = prev + (pos - prev) * step.alpha();
/// assert!((step.alpha() - 0.5).abs() < 0.01);
/// assert!(drawn > prev && drawn < pos);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedStep {
    step: Duration,
    max_steps: u32,
    acc: Duration,
    dropped: Duration,
}

impl FixedStep {
    /// Steps of `step` (clamped to at least 1 µs), catching up at most 8
    /// steps per frame.
    pub fn new(step: Duration) -> FixedStep {
        FixedStep {
            step: step.max(Duration::from_micros(1)),
            max_steps: 8,
            acc: Duration::ZERO,
            dropped: Duration::ZERO,
        }
    }

    /// `hz` steps per second (`hz` of 0 counts as 1).
    pub fn hz(hz: u32) -> FixedStep {
        FixedStep::new(Duration::from_secs(1) / hz.max(1))
    }

    /// At most `n` steps per frame (at least 1). Time beyond that is dropped
    /// rather than run later, so a slow frame doesn't start a spiral of ever
    /// longer frames: the game slows down instead.
    pub fn max_steps(mut self, n: u32) -> FixedStep {
        self.max_steps = n.max(1);
        self
    }

    /// The fixed step.
    pub fn dt(&self) -> Duration {
        self.step
    }

    /// The fixed step, in seconds.
    pub fn dt_secs(&self) -> f64 {
        self.step.as_secs_f64()
    }

    /// Add a frame's `dt` and return how many steps to run now.
    pub fn advance(&mut self, dt: Duration) -> u32 {
        self.acc += dt;
        let due = self.acc.as_nanos() / self.step.as_nanos();
        let steps = u32::try_from(due).unwrap_or(u32::MAX).min(self.max_steps);
        self.acc -= self.step * steps;
        // Past the catch-up limit: keep less than a step.
        if self.acc >= self.step {
            let keep = Duration::from_nanos((self.acc.as_nanos() % self.step.as_nanos()) as u64);
            self.dropped += self.acc - keep;
            self.acc = keep;
        }
        steps
    }

    /// How far the time left over is into the next step, from 0 to 1: where
    /// to draw between the state before the last step (0) and after it (1).
    pub fn alpha(&self) -> f64 {
        self.acc.as_secs_f64() / self.step.as_secs_f64()
    }

    /// The time left over, less than one step.
    pub fn remainder(&self) -> Duration {
        self.acc
    }

    /// The time dropped so far because a frame needed more than
    /// [`max_steps`](Self::max_steps).
    pub fn dropped(&self) -> Duration {
        self.dropped
    }

    /// Forget the time left over (after a pause or a level change).
    pub fn reset(&mut self) {
        self.acc = Duration::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn frames_are_spent_in_whole_steps() {
        let mut s = FixedStep::new(10 * MS);
        assert_eq!(s.advance(4 * MS), 0);
        assert!((s.alpha() - 0.4).abs() < 1e-9);
        assert_eq!(s.advance(7 * MS), 1, "11 ms: one step");
        assert_eq!(s.remainder(), MS);
        assert_eq!(s.advance(29 * MS), 3, "30 ms: three steps");
        assert_eq!(s.remainder(), Duration::ZERO);
        assert_eq!(s.alpha(), 0.0);
    }

    #[test]
    fn the_steps_add_up_whatever_the_refresh_rate() {
        for hz in [30u32, 60, 75, 144, 240] {
            let mut s = FixedStep::hz(120);
            let frame = Duration::from_secs(1) / hz;
            let steps: u32 = (0..hz).map(|_| s.advance(frame)).sum();
            assert!((119..=120).contains(&steps), "{hz} Hz gave {steps} steps");
        }
    }

    #[test]
    fn catching_up_is_capped_and_the_rest_dropped() {
        let mut s = FixedStep::new(10 * MS).max_steps(3);
        assert_eq!(s.advance(105 * MS), 3);
        assert_eq!(s.remainder(), 5 * MS, "less than a step is kept");
        assert_eq!(s.dropped(), 70 * MS);
        assert_eq!(s.advance(5 * MS), 1);
        s.advance(3 * MS);
        s.reset();
        assert_eq!(s.alpha(), 0.0);
    }
}
