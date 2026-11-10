//! The [`Animation`] trait and its combinators: delays, repeats, sequences,
//! parallel groups, staggers, and decaying flings.

use std::sync::Arc;

use super::{Animatable, Tween, Vector};

/// A value as a pure function of time.
///
/// `t` is seconds since the animation started (its *local* time). Sampling
/// has no side effects, so an animation can be sampled at any frame time —
/// out of order, twice, or at fixed times in a test — and gives the same
/// answer. A view samples at `Cx::frame_time()` minus the start time and calls
/// `Cx::request_frame()` while [`is_finished`](Animation::is_finished) is
/// false (or uses `Cx::play`, which does both).
pub trait Animation {
    /// What the animation produces.
    type Value: Animatable;

    /// The value at local time `t` (seconds). Before `0` it's the start
    /// value; after the end, the end value.
    fn sample(&self, t: f64) -> Self::Value;

    /// The rate of change at `t`, in vector units per second. Used to hand
    /// motion over to a spring without a jolt. The default differentiates
    /// [`sample`](Animation::sample) numerically.
    fn velocity(&self, t: f64) -> <Self::Value as Animatable>::Vector {
        let h = 1e-4;
        let (a, b) = ((t - h).max(0.0), t + h);
        self.sample(b)
            .to_vector()
            .minus(self.sample(a).to_vector())
            .times(1.0 / (b - a))
    }

    /// How long the animation runs, in seconds, or `None` if it never ends
    /// (an infinite repeat, an undamped spring).
    fn duration(&self) -> Option<f64>;

    /// Whether the animation has come to rest at `t`. A view stops asking for
    /// frames once this is true.
    fn is_finished(&self, t: f64) -> bool {
        self.duration().is_some_and(|d| t >= d)
    }
}

/// A shared, type-erased animation (cheap to clone).
pub type BoxedAnimation<T> = Arc<dyn Animation<Value = T> + Send + Sync>;

impl<A: Animation + ?Sized> Animation for Arc<A> {
    type Value = A::Value;
    fn sample(&self, t: f64) -> A::Value {
        (**self).sample(t)
    }
    fn velocity(&self, t: f64) -> <A::Value as Animatable>::Vector {
        (**self).velocity(t)
    }
    fn duration(&self) -> Option<f64> {
        (**self).duration()
    }
    fn is_finished(&self, t: f64) -> bool {
        (**self).is_finished(t)
    }
}

impl<A: Animation + ?Sized> Animation for Box<A> {
    type Value = A::Value;
    fn sample(&self, t: f64) -> A::Value {
        (**self).sample(t)
    }
    fn velocity(&self, t: f64) -> <A::Value as Animatable>::Vector {
        (**self).velocity(t)
    }
    fn duration(&self) -> Option<f64> {
        (**self).duration()
    }
    fn is_finished(&self, t: f64) -> bool {
        (**self).is_finished(t)
    }
}

/// Combinators available on every [`Animation`].
pub trait AnimationExt: Animation + Sized {
    /// Start `seconds` later, holding the start value until then.
    fn delay(self, seconds: f64) -> Delay<Self> {
        Delay {
            inner: self,
            delay: seconds.max(0.0),
        }
    }

    /// Play `count` times in all (fractions allowed: `2.5` stops halfway
    /// through the third run).
    fn repeat(self, count: f64) -> Repeat<Self> {
        Repeat {
            inner: self,
            count: Some(count.max(0.0)),
            alternate: false,
        }
    }

    /// Play forever.
    fn repeat_forever(self) -> Repeat<Self> {
        Repeat {
            inner: self,
            count: None,
            alternate: false,
        }
    }

    /// Then play `next` (its local time starting where this one ends).
    fn then<B>(self, next: B) -> Sequence<Self::Value>
    where
        Self: Send + Sync + 'static,
        B: Animation<Value = Self::Value> + Send + Sync + 'static,
    {
        Sequence::new(self).then(next)
    }

    /// Erase the type, to store animations of different kinds together.
    fn boxed(self) -> BoxedAnimation<Self::Value>
    where
        Self: Send + Sync + 'static,
    {
        Arc::new(self)
    }
}

impl<A: Animation> AnimationExt for A {}

/// Holds a value: the end state of an animation, or a value at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Still<T>(pub T);

impl<T: Animatable> Animation for Still<T> {
    type Value = T;
    fn sample(&self, _t: f64) -> T {
        self.0
    }
    fn velocity(&self, _t: f64) -> T::Vector {
        T::Vector::ZERO
    }
    fn duration(&self) -> Option<f64> {
        Some(0.0)
    }
}

impl<T: Animatable> Animation for Tween<T> {
    type Value = T;
    fn sample(&self, t: f64) -> T {
        Tween::sample(self, t)
    }
    fn velocity(&self, t: f64) -> T::Vector {
        if self.duration <= 0.0 || !(0.0..self.duration).contains(&t) {
            return T::Vector::ZERO;
        }
        let d = self.to.to_vector().minus(self.from.to_vector());
        d.times(self.easing.slope(t / self.duration) / self.duration)
    }
    fn duration(&self) -> Option<f64> {
        Some(self.duration.max(0.0))
    }
}

/// An animation started late; see [`AnimationExt::delay`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delay<A> {
    pub inner: A,
    /// Seconds before `inner` starts.
    pub delay: f64,
}

impl<A: Animation> Delay<A> {
    /// The inner animation's time at `t`. Once finished it's exactly the
    /// inner end, whatever `t - delay` rounds to.
    fn local(&self, t: f64) -> f64 {
        match self.inner.duration() {
            Some(d) if t >= self.delay + d => d,
            _ => (t - self.delay).max(0.0),
        }
    }
}

impl<A: Animation> Animation for Delay<A> {
    type Value = A::Value;
    fn sample(&self, t: f64) -> A::Value {
        self.inner.sample(self.local(t))
    }
    fn velocity(&self, t: f64) -> <A::Value as Animatable>::Vector {
        if t < self.delay || self.is_finished(t) {
            return Vector::ZERO;
        }
        self.inner.velocity(self.local(t))
    }
    fn duration(&self) -> Option<f64> {
        self.inner.duration().map(|d| d + self.delay)
    }
    fn is_finished(&self, t: f64) -> bool {
        match self.duration() {
            // Compare with the sum, so `is_finished(duration())` holds exactly.
            Some(d) => t >= d,
            None => t >= self.delay && self.inner.is_finished(t - self.delay),
        }
    }
}

/// An animation played several times or forever; see
/// [`AnimationExt::repeat`] and [`AnimationExt::repeat_forever`].
///
/// The inner animation must have a finite, positive duration; otherwise it
/// plays once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Repeat<A> {
    pub inner: A,
    /// Number of runs, `None` for forever.
    pub count: Option<f64>,
    /// Whether every other run plays backward (CSS `alternate`).
    pub alternate: bool,
}

impl<A: Animation> Repeat<A> {
    /// Play every other run backward, so the motion goes back and forth
    /// instead of jumping back to the start.
    pub fn alternate(mut self) -> Self {
        self.alternate = true;
        self
    }

    /// The inner animation's local time at `t`, and whether that run plays
    /// backward. `None` when the inner animation can't repeat.
    fn local(&self, t: f64) -> Option<(f64, bool)> {
        let d = self.inner.duration().filter(|d| *d > 0.0)?;
        let t = t.max(0.0);
        let t = match self.count {
            Some(c) => t.min(c * d),
            None => t,
        };
        let mut i = (t / d).floor();
        let mut u = t - i * d;
        // At the very end of a whole number of runs, report the end of the
        // last run rather than the start of the next.
        if self.count.is_some_and(|c| t >= c * d) && u <= 1e-12 && i > 0.0 {
            i -= 1.0;
            u = d;
        }
        let backward = self.alternate && (i as u64) % 2 == 1;
        Some(if backward { (d - u, true) } else { (u, false) })
    }
}

impl<A: Animation> Animation for Repeat<A> {
    type Value = A::Value;
    fn sample(&self, t: f64) -> A::Value {
        match self.local(t) {
            Some((u, _)) => self.inner.sample(u),
            None => self.inner.sample(t),
        }
    }
    fn velocity(&self, t: f64) -> <A::Value as Animatable>::Vector {
        if self.is_finished(t) {
            return Vector::ZERO;
        }
        match self.local(t) {
            Some((u, true)) => self.inner.velocity(u).times(-1.0),
            Some((u, false)) => self.inner.velocity(u),
            None => self.inner.velocity(t),
        }
    }
    fn duration(&self) -> Option<f64> {
        match self.inner.duration().filter(|d| *d > 0.0) {
            Some(d) => self.count.map(|c| c * d),
            None => self.inner.duration(),
        }
    }
}

/// Animations of the same value played one after another; see
/// [`AnimationExt::then`].
///
/// Each step starts from its own start value: build steps that join up (or
/// use [`Keyframes`](super::Keyframes), which always does). A step that never
/// ends holds the sequence there.
#[derive(Clone)]
pub struct Sequence<T> {
    steps: Vec<BoxedAnimation<T>>,
}

impl<T> core::fmt::Debug for Sequence<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sequence")
            .field("steps", &self.steps.len())
            .finish()
    }
}

impl<T: Animatable> Sequence<T> {
    /// A sequence starting with `first`.
    pub fn new(first: impl Animation<Value = T> + Send + Sync + 'static) -> Self {
        Self {
            steps: vec![Arc::new(first)],
        }
    }

    /// Append a step.
    pub fn then(mut self, next: impl Animation<Value = T> + Send + Sync + 'static) -> Self {
        self.steps.push(Arc::new(next));
        self
    }

    /// The step playing at `t` and its local time.
    fn at(&self, t: f64) -> (&BoxedAnimation<T>, f64) {
        let last = self.steps.len() - 1;
        if let Some(total) = self.duration()
            && t >= total
        {
            // Past the end: exactly the last step's end.
            let step = &self.steps[last];
            return (step, step.duration().unwrap_or(0.0));
        }
        let mut start = 0.0;
        for (i, step) in self.steps.iter().enumerate() {
            match step.duration() {
                Some(d) if i < last && t >= start + d => start += d,
                _ => return (step, t - start),
            }
        }
        unreachable!("a sequence has at least one step")
    }
}

impl<T: Animatable> Animation for Sequence<T> {
    type Value = T;
    fn sample(&self, t: f64) -> T {
        let (step, u) = self.at(t);
        step.sample(u)
    }
    fn velocity(&self, t: f64) -> T::Vector {
        let (step, u) = self.at(t);
        step.velocity(u)
    }
    fn duration(&self) -> Option<f64> {
        self.steps.iter().map(|s| s.duration()).sum()
    }
    fn is_finished(&self, t: f64) -> bool {
        self.duration().is_some_and(|d| t >= d)
    }
}

/// Two animations played together, producing a pair: a parallel group. Nest
/// tuples or use a triple for more. It finishes when both have.
impl<A: Animation, B: Animation> Animation for (A, B) {
    type Value = (A::Value, B::Value);
    fn sample(&self, t: f64) -> Self::Value {
        (self.0.sample(t), self.1.sample(t))
    }
    fn velocity(&self, t: f64) -> <Self::Value as Animatable>::Vector {
        (self.0.velocity(t), self.1.velocity(t))
    }
    fn duration(&self) -> Option<f64> {
        Some(self.0.duration()?.max(self.1.duration()?))
    }
    fn is_finished(&self, t: f64) -> bool {
        self.0.is_finished(t) && self.1.is_finished(t)
    }
}

/// Three animations played together; see the pair.
impl<A: Animation, B: Animation, C: Animation> Animation for (A, B, C) {
    type Value = (A::Value, B::Value, C::Value);
    fn sample(&self, t: f64) -> Self::Value {
        (self.0.sample(t), self.1.sample(t), self.2.sample(t))
    }
    fn velocity(&self, t: f64) -> <Self::Value as Animatable>::Vector {
        (self.0.velocity(t), self.1.velocity(t), self.2.velocity(t))
    }
    fn duration(&self) -> Option<f64> {
        Some(
            self.0
                .duration()?
                .max(self.1.duration()?)
                .max(self.2.duration()?),
        )
    }
    fn is_finished(&self, t: f64) -> bool {
        self.0.is_finished(t) && self.1.is_finished(t) && self.2.is_finished(t)
    }
}

/// The delay of item `index` in a staggered group: `index * step` seconds.
/// Give each list item its entrance `.delay(stagger(i, 0.04))`.
pub fn stagger(index: usize, step: f64) -> f64 {
    index as f64 * step
}

/// Like [`stagger`], but the delays of `count` items never exceed `max`
/// seconds in all: long lists stagger faster rather than taking forever.
pub fn stagger_within(index: usize, count: usize, step: f64, max: f64) -> f64 {
    let step = if count > 1 {
        step.min(max / (count - 1) as f64)
    } else {
        step
    };
    stagger(index, step)
}

/// A fling that slows down by friction: starts at `from` moving at
/// `velocity` and decays exponentially (`v(t) = v₀·e^(−rate·t)`), coming to
/// rest at [`Decay::target`]. What a released scroll or thrown card does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decay<T: Animatable> {
    pub from: T,
    pub velocity: T::Vector,
    /// Decay rate per second. [`Decay::NORMAL`] is a scroll view's gentle
    /// glide, [`Decay::FAST`] stops quickly.
    pub rate: f64,
}

impl<T: Animatable> Decay<T> {
    /// A long glide (iOS's normal scroll deceleration, 0.998 per ms).
    pub const NORMAL: f64 = 2.002;
    /// A short glide (iOS's fast deceleration, 0.99 per ms).
    pub const FAST: f64 = 10.05;

    pub fn new(from: T, velocity: T::Vector, rate: f64) -> Self {
        Self {
            from,
            velocity,
            rate,
        }
    }

    /// Where the fling comes to rest: `from + velocity / rate`. Pick a snap
    /// point near it and spring there instead to fling into place.
    pub fn target(&self) -> T {
        if self.rate <= 0.0 {
            return self.from;
        }
        T::from_vector(
            self.from
                .to_vector()
                .plus(self.velocity.times(1.0 / self.rate)),
        )
    }
}

impl<T: Animatable> Animation for Decay<T> {
    type Value = T;
    fn sample(&self, t: f64) -> T {
        if self.rate <= 0.0 {
            return self.from;
        }
        if self.is_finished(t) {
            return self.target();
        }
        let k = (1.0 - (-self.rate * t.max(0.0)).exp()) / self.rate;
        T::from_vector(self.from.to_vector().plus(self.velocity.times(k)))
    }
    fn velocity(&self, t: f64) -> T::Vector {
        if self.rate <= 0.0 || self.is_finished(t) {
            return T::Vector::ZERO;
        }
        self.velocity.times((-self.rate * t.max(0.0)).exp())
    }
    fn duration(&self) -> Option<f64> {
        // Remaining distance |v(t)|/rate falls below REST_EPSILON.
        let remaining = self.velocity.norm() / self.rate;
        if self.rate <= 0.0 || remaining < super::REST_EPSILON {
            return Some(0.0);
        }
        Some((remaining / super::REST_EPSILON).ln() / self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Easing, Keyframes};
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn delay_holds_then_plays() {
        let a = Tween::new(0.0, 10.0, 1.0, Easing::Linear).delay(0.5);
        assert_eq!(a.sample(0.0), 0.0);
        assert_eq!(a.sample(0.5), 0.0);
        assert!(close(a.sample(1.0), 5.0));
        assert_eq!(a.velocity(0.25), 0.0);
        assert!(close(a.velocity(1.0), 10.0));
        assert_eq!(a.duration(), Some(1.5));
        assert!(!a.is_finished(1.49) && a.is_finished(1.5));
    }

    #[test]
    fn repeat_counts_and_alternates() {
        let tw = Tween::new(0.0, 1.0, 1.0, Easing::Linear);
        let r = tw.repeat(3.0);
        assert!(close(r.sample(0.25), 0.25));
        assert!(close(r.sample(1.25), 0.25), "restarts");
        assert_eq!(r.sample(3.0), 1.0, "ends at the end of the last run");
        assert_eq!(r.sample(10.0), 1.0);
        assert_eq!(r.duration(), Some(3.0));
        assert!(r.is_finished(3.0) && !r.is_finished(2.99));

        let a = tw.repeat(2.0).alternate();
        assert!(close(a.sample(0.75), 0.75));
        assert!(close(a.sample(1.25), 0.75), "second run goes back");
        assert!(close(a.velocity(1.25), -1.0));
        assert!(close(a.sample(2.0), 0.0), "even count ends where it began");
        assert_eq!(a.velocity(2.0), 0.0);

        let half = tw.repeat(1.5);
        assert!(
            close(half.sample(5.0), 0.5),
            "fractional count stops mid-run"
        );

        let f = tw.repeat_forever().alternate();
        assert_eq!(f.duration(), None);
        assert!(!f.is_finished(1e6));
        assert!(close(f.sample(1001.5), 0.5));
        assert!(close(f.sample(1000.25), 0.25));
        assert!(close(f.sample(1001.25), 0.75));
    }

    #[test]
    fn sequences_chain_steps() {
        let s = Tween::new(0.0, 1.0, 1.0, Easing::Linear)
            .then(Still(1.0).delay(0.5))
            .then(Tween::new(1.0, 0.0, 2.0, Easing::Linear));
        assert_eq!(s.duration(), Some(3.5));
        assert!(close(s.sample(0.5), 0.5));
        assert_eq!(s.sample(1.2), 1.0);
        assert!(close(s.sample(2.5), 0.5));
        assert!(close(s.velocity(2.5), -0.5));
        assert_eq!(s.sample(9.0), 0.0);
        assert!(!s.is_finished(3.4) && s.is_finished(3.5));
    }

    #[test]
    fn parallel_tuples_finish_together() {
        let p = (
            Tween::new(0.0, 1.0, 1.0, Easing::Linear),
            Keyframes::new(Point::ORIGIN).to(Point::new(4.0, 0.0), 2.0, Easing::Linear),
        );
        assert_eq!(p.duration(), Some(2.0));
        let (a, b) = p.sample(1.0);
        assert_eq!((a, b), (1.0, Point::new(2.0, 0.0)));
        assert!(!p.is_finished(1.5) && p.is_finished(2.0));
    }

    use crate::geometry::Point;

    #[test]
    fn stagger_spaces_items() {
        assert_eq!(stagger(0, 0.05), 0.0);
        assert!(close(stagger(3, 0.05), 0.15));
        // 101 items at 50 ms would take 5 s; capped at 1 s in all.
        assert!(close(stagger_within(100, 101, 0.05, 1.0), 1.0));
        assert!(close(stagger_within(2, 3, 0.05, 1.0), 0.1));
        // Staggered entrances: item i starts i * step later.
        let entrance = |i| Tween::new(0.0, 1.0, 0.2, Easing::Linear).delay(stagger(i, 0.1));
        assert_eq!(entrance(2).sample(0.19), 0.0);
        assert!(close(entrance(2).sample(0.3), 0.5));
        assert!(entrance(0).is_finished(0.2) && !entrance(1).is_finished(0.2));
    }

    #[test]
    fn decay_glides_to_its_target() {
        let d = Decay::new(0.0, 1000.0, Decay::<f64>::NORMAL);
        assert!(close(d.target(), 1000.0 / Decay::<f64>::NORMAL));
        assert_eq!(d.velocity(0.0), 1000.0);
        let end = d.duration().unwrap();
        assert!(end.is_finite() && end > 1.0);
        assert_eq!(d.sample(end), d.target());
        assert!((d.sample(end * 0.999) - d.target()).abs() < 1e-2);
        let h = 1e-6;
        let fd = (d.sample(0.3 + h) - d.sample(0.3 - h)) / (2.0 * h);
        assert!((fd - d.velocity(0.3)).abs() < 1e-3);
    }
}
