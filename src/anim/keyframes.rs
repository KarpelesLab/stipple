//! Keyframes, and ready-made effects built from them.

use super::{Animatable, Animation, AnimationExt, Delay, Easing, Repeat, Tween, Vector};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment<T> {
    to: T,
    duration: f64,
    easing: Easing,
}

/// A value moving through a list of keyframes, each segment with its own
/// duration and easing.
///
/// ```
/// use stipple::anim::{Animation, Easing, Keyframes};
/// // Grow, hold, then shrink back.
/// let pop = Keyframes::new(1.0)
///     .to(1.2, 0.1, Easing::EaseOut)
///     .hold(0.2)
///     .to(1.0, 0.3, Easing::EaseInOut);
/// assert!((pop.duration().unwrap() - 0.6).abs() < 1e-9);
/// assert_eq!(pop.sample(0.2), 1.2);
/// ```
///
/// At a boundary the next segment starts (with the same value, so the motion
/// is continuous). Combine with [`AnimationExt::repeat`] and
/// [`Repeat::alternate`] for loops.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframes<T> {
    start: T,
    segments: Vec<Segment<T>>,
}

impl<T: Animatable> Keyframes<T> {
    /// Keyframes starting at `start`.
    pub fn new(start: T) -> Self {
        Self {
            start,
            segments: Vec::new(),
        }
    }

    /// Move to `value` over `duration` seconds along `easing`.
    pub fn to(mut self, value: T, duration: f64, easing: Easing) -> Self {
        self.segments.push(Segment {
            to: value,
            duration: duration.max(0.0),
            easing,
        });
        self
    }

    /// Stay at the current value for `duration` seconds.
    pub fn hold(self, duration: f64) -> Self {
        let at = self.end();
        self.to(at, duration, Easing::Linear)
    }

    /// CSS-style keyframes: a total `duration` and `(offset, value, easing)`
    /// stops with offsets in `[0, 1]`, in order. Each easing applies to the
    /// segment leading *to* its stop. A stop at offset 0 replaces `start`.
    pub fn percent(
        start: T,
        duration: f64,
        stops: impl IntoIterator<Item = (f64, T, Easing)>,
    ) -> Self {
        let mut k = Self::new(start);
        let mut at = 0.0;
        for (offset, value, easing) in stops {
            let offset = offset.clamp(at, 1.0);
            if offset == 0.0 {
                k.start = value;
                continue;
            }
            k = k.to(value, (offset - at) * duration, easing);
            at = offset;
        }
        k
    }

    /// The value at the end.
    pub fn end(&self) -> T {
        self.segments.last().map_or(self.start, |s| s.to)
    }

    /// The segment at `t`: its start value, the segment, and its local time.
    fn at(&self, t: f64) -> Option<(T, &Segment<T>, f64)> {
        let mut from = self.start;
        let mut start = 0.0;
        for seg in &self.segments {
            if t < start + seg.duration {
                return Some((from, seg, t - start));
            }
            start += seg.duration;
            from = seg.to;
        }
        None
    }
}

impl<T: Animatable> Animation for Keyframes<T> {
    type Value = T;
    fn sample(&self, t: f64) -> T {
        if t <= 0.0 {
            return self.start;
        }
        match self.at(t) {
            Some((from, seg, u)) => from.interpolate(seg.to, seg.easing.apply(u / seg.duration)),
            None => self.end(),
        }
    }
    fn velocity(&self, t: f64) -> T::Vector {
        match self.at(t.max(0.0)) {
            Some((from, seg, u)) => seg
                .to
                .to_vector()
                .minus(from.to_vector())
                .times(seg.easing.slope(u / seg.duration) / seg.duration),
            None => T::Vector::ZERO,
        }
    }
    fn duration(&self) -> Option<f64> {
        Some(self.segments.iter().map(|s| s.duration).sum())
    }
}

/// A horizontal shake, for invalid input: an offset in logical pixels that
/// swings `±amplitude` and dies away over `duration` seconds, starting and
/// ending at 0. Typical: `shake(10.0, 0.4)`. Add it to the element's x
/// position (or a translate transform).
pub fn shake(amplitude: f64, duration: f64) -> Keyframes<f64> {
    let swings = [1.0, -1.0, 0.7, -0.7, 0.35, -0.35, 0.0];
    let step = duration / swings.len() as f64;
    swings.iter().fold(Keyframes::new(0.0), |k, s| {
        k.to(s * amplitude, step, Easing::EaseInOut)
    })
}

/// A value pulsing between `low` and `high` forever, one full cycle (there
/// and back) every `period` seconds, eased at both ends. For a drop target
/// that breathes, or an attention dot. Starts at `low`.
pub fn pulse<T: Animatable>(low: T, high: T, period: f64) -> Repeat<Tween<T>> {
    Tween::new(low, high, period / 2.0, Easing::EaseInOut)
        .repeat_forever()
        .alternate()
}

/// One item's entrance in a staggered list: progress from `0` to `1` over
/// `duration` seconds along [`Easing::EMPHASIZED_DECELERATE`], starting
/// `index * step` seconds in. Map the progress to opacity, an offset, or a
/// scale.
pub fn entrance(index: usize, step: f64, duration: f64) -> Delay<Tween<f64>> {
    Tween::new(0.0, 1.0, duration, Easing::EMPHASIZED_DECELERATE).delay(super::stagger(index, step))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Color;

    #[test]
    fn segments_meet_at_boundaries() {
        let k = Keyframes::new(0.0)
            .to(10.0, 1.0, Easing::EaseIn)
            .to(-10.0, 2.0, Easing::Linear)
            .hold(1.0);
        assert_eq!(k.duration(), Some(4.0));
        assert_eq!(k.sample(-1.0), 0.0);
        assert_eq!(k.sample(0.5), 2.5, "per-segment easing");
        assert_eq!(k.sample(1.0), 10.0, "boundary is the keyframe value");
        assert!((k.sample(1.0 - 1e-9) - 10.0).abs() < 1e-6, "continuous");
        assert_eq!(k.sample(2.0), 0.0);
        assert_eq!(k.velocity(2.0), -10.0);
        assert_eq!(k.velocity(0.5), 10.0);
        assert_eq!(k.sample(3.5), -10.0);
        assert_eq!(k.velocity(3.5), 0.0);
        assert_eq!(k.sample(100.0), -10.0);
        assert!(!k.is_finished(3.99) && k.is_finished(4.0));
    }

    #[test]
    fn percent_stops() {
        let k = Keyframes::percent(
            Color::BLACK,
            2.0,
            [
                (0.0, Color::WHITE, Easing::Linear),
                (0.5, Color::BLACK, Easing::Linear),
                (1.0, Color::WHITE, Easing::Linear),
            ],
        );
        assert_eq!(k.sample(0.0), Color::WHITE);
        assert_eq!(k.sample(1.0), Color::BLACK);
        assert_eq!(k.sample(2.0), Color::WHITE);
        assert_eq!(k.duration(), Some(2.0));
    }

    #[test]
    fn zero_length_segments_jump() {
        let k = Keyframes::new(0.0)
            .to(5.0, 0.0, Easing::Linear)
            .to(6.0, 1.0, Easing::Linear);
        assert_eq!(k.sample(0.0), 0.0);
        assert!((k.sample(0.5) - 5.5).abs() < 1e-12);
    }

    #[test]
    fn shake_swings_and_settles() {
        let s = shake(10.0, 0.7);
        assert_eq!(s.sample(0.0), 0.0);
        assert!((s.sample(0.1) - 10.0).abs() < 1e-9);
        assert!((s.sample(0.2) + 10.0).abs() < 1e-9);
        assert_eq!(s.sample(0.7), 0.0);
        assert!(s.is_finished(0.7));
        let peak = (0..70)
            .map(|i| s.sample(i as f64 / 100.0).abs())
            .fold(0.0, f64::max);
        assert!(peak <= 10.0 + 1e-9);
    }

    #[test]
    fn pulse_breathes_forever() {
        let p = pulse(0.0, 1.0, 2.0);
        assert_eq!(p.sample(0.0), 0.0);
        assert_eq!(p.sample(1.0), 1.0);
        assert!((p.sample(2.0)).abs() < 1e-12);
        assert!((p.sample(0.5) - p.sample(1.5)).abs() < 1e-12, "symmetric");
        assert!(p.velocity(0.5) > 0.0 && p.velocity(1.5) < 0.0);
        assert!(!p.is_finished(1e9));
    }

    #[test]
    fn entrance_is_staggered() {
        let e2 = entrance(2, 0.05, 0.3);
        assert_eq!(e2.sample(0.1), 0.0);
        assert!(e2.sample(0.2) > 0.0);
        assert_eq!(e2.sample(0.4), 1.0);
        assert!(e2.is_finished(0.4) && !e2.is_finished(0.39));
    }
}
