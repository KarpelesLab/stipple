//! Implicit transitions: an element's animatable properties ease (or spring)
//! to new values between builds instead of jumping.
//!
//! An element declares a [`Transition`] with
//! [`Element::transition`](crate::core::Element::transition). Each build, the
//! app reads the element's animatable properties ([`AnimProp`]) after its
//! per-state overrides have been applied; when one differs from the last
//! build's target, it heads there from wherever it is now, and the element is
//! drawn with the in-flight value. [`Transitions`] keeps an
//! [`Animated`] per property by element identity ([`ElementKey`]) and reports
//! whether anything still moves, so the app asks for frames only until
//! everything is at rest.
//!
//! The motion itself is [`crate::anim`]'s: a [`Transition`] wraps a
//! [`Motion`], each property is an [`Animatable`] value (colors blend in
//! premultiplied sRGB, a transform part by part like [`Transform::lerp`]), and
//! a change mid-flight keeps the current value and velocity. Adding a
//! property is adding an [`AnimProp`] variant and a `Tracks` field: how to
//! read it from an [`Element`], write it back, and what it fades from and to
//! when absent.

use std::collections::HashMap;
use std::time::Duration;

use crate::anim::{Animatable, Animated, Easing, Motion, SpringParams};
use crate::core::element::{Element, ElementKind};
use crate::core::interact::ElementKey;
use crate::geometry::{Insets, Transform};
use crate::render::Color;

/// How an element's properties move to new values: a [`Motion`] (a tween with
/// an easing curve, a spring, or a jump).
///
/// ```
/// use stipple::prelude::*;
/// let theme = Theme::light();
/// let card = label(&theme, "Hover me")
///     .fill(theme.palette.surface)
///     .hovered(|e| e.fill(theme.palette.primary))
///     .transition(Transition::ease(Duration::from_millis(150)));
/// # let _ = card;
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition(pub Motion);

impl Transition {
    /// Move to a new value over `duration`, along `easing`. A change mid-way
    /// starts from the current value, carrying its velocity in.
    pub const fn tween(duration: Duration, easing: Easing) -> Self {
        Self(Motion::Tween {
            duration: duration.as_secs_f64(),
            easing,
        })
    }

    /// [`tween`](Self::tween) with [`Easing::EaseOut`], the usual choice for
    /// state changes.
    pub const fn ease(duration: Duration) -> Self {
        Self::tween(duration, Easing::EaseOut)
    }

    /// Move on a spring ([`SpringParams::SNAPPY`], [`SpringParams::BOUNCY`],
    /// `SpringParams::new(stiffness, damping)`…). A change mid-flight keeps
    /// the current value and velocity.
    pub const fn spring(params: SpringParams) -> Self {
        Self(Motion::Spring(params))
    }

    /// Change at once: no transition (to switch one property off when the
    /// rest animate, with [`Element::transition_for`](crate::core::Element::transition_for)).
    pub const fn instant() -> Self {
        Self(Motion::Instant)
    }
}

impl Default for Transition {
    /// A 150 ms ease-out.
    fn default() -> Self {
        Self::ease(Duration::from_millis(150))
    }
}

impl From<Motion> for Transition {
    fn from(motion: Motion) -> Self {
        Self(motion)
    }
}

impl From<SpringParams> for Transition {
    fn from(params: SpringParams) -> Self {
        Self::spring(params)
    }
}

/// An animatable property of an element: what
/// [`Element::transition_for`](crate::core::Element::transition_for) names.
/// [`Element::transition`](crate::core::Element::transition) animates them all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnimProp {
    /// The background fill. Appearing or going away, it fades from or to
    /// transparent.
    Fill,
    /// The border's color and width. Appearing or going away, it grows from
    /// or shrinks to nothing.
    Border,
    /// The corner radius.
    Radius,
    /// The padding (which moves the content and, for an element sized by its
    /// content, its size).
    Padding,
    /// A fixed [`width`](crate::core::Element::width). Animates only between
    /// two fixed widths; to or from "sized by content" it switches.
    Width,
    /// A fixed [`height`](crate::core::Element::height), as for
    /// [`Width`](Self::Width).
    Height,
    /// A text element's color.
    TextColor,
    /// The group [`opacity`](crate::core::Element::opacity) of the element
    /// and its subtree.
    Opacity,
    /// The element's [`transform`](crate::core::Element::transform), part by
    /// part (as [`Transform::lerp`]); no transform is the identity.
    Transform,
}

impl AnimProp {
    /// Every property.
    pub const ALL: [AnimProp; 9] = [
        AnimProp::Fill,
        AnimProp::Border,
        AnimProp::Radius,
        AnimProp::Padding,
        AnimProp::Width,
        AnimProp::Height,
        AnimProp::TextColor,
        AnimProp::Opacity,
        AnimProp::Transform,
    ];
}

/// One property in motion: `None` while the element doesn't have it (and
/// nothing is fading out).
type Track<T> = Option<Animated<T>>;

/// Head `track` for `target` (`None`: the element doesn't have the property;
/// it fades to `absent` if there is one, else switches) with `motion`, the
/// change counting from `head`. Returns the value to draw at `now`.
fn drive<T: Animatable>(
    track: &mut Option<Track<T>>,
    target: Option<T>,
    absent: Option<T>,
    motion: Motion,
    now: Option<Duration>,
    head: Duration,
) -> Option<T> {
    let (Some(now), Some(slot)) = (now, track.as_mut()) else {
        // First sight, or no clock: as it is.
        *track = Some(target.map(Animated::new));
        return target;
    };
    match (target, slot.as_mut()) {
        (Some(v), Some(anim)) => {
            anim.animate_to(v, motion, head);
            Some(anim.value(now))
        }
        (Some(v), None) => {
            // Appearing: from its faded-out look, or at once.
            let mut anim = Animated::new(absent.unwrap_or(v));
            anim.animate_to(v, motion, head);
            let value = anim.value(now);
            *slot = Some(anim);
            Some(value)
        }
        (None, Some(anim)) => match absent {
            Some(a) => {
                anim.animate_to(a, motion, head);
                if anim.is_animating(now) {
                    Some(anim.value(now))
                } else {
                    *slot = None;
                    None
                }
            }
            None => {
                *slot = None;
                None
            }
        },
        (None, None) => None,
    }
}

fn moving<T: Animatable>(track: &Option<Track<T>>, now: Duration) -> bool {
    matches!(track, Some(Some(a)) if a.is_animating(now))
}

/// The tracks of one element, by property (`None`: not animated).
#[derive(Clone, Debug, Default)]
struct Tracks {
    fill: Option<Track<Color>>,
    border: Option<Track<(Color, f64)>>,
    radius: Option<Track<f64>>,
    padding: Option<Track<Insets>>,
    width: Option<Track<f64>>,
    height: Option<Track<f64>>,
    text_color: Option<Track<Color>>,
    opacity: Option<Track<f64>>,
    transform: Option<Track<Transform>>,
    /// The frame time of the last build that animated the element.
    last: Option<Duration>,
    /// Built this frame (entries that weren't are dropped).
    seen: bool,
}

impl Tracks {
    /// Animate `el`'s properties with the motion `motion_of` gives each
    /// (`None`: not animated), from `head`, as of `now`.
    fn animate(
        &mut self,
        el: &mut Element,
        motion_of: impl Fn(AnimProp) -> Option<Motion>,
        now: Option<Duration>,
        head: Duration,
    ) {
        // Drive one track: the value to draw, or `None` (and the track
        // dropped) when the property has no motion.
        macro_rules! go {
            ($prop:expr, $field:ident, $target:expr, $absent:expr) => {
                match motion_of($prop) {
                    Some(m) => Some(drive(&mut self.$field, $target, $absent, m, now, head)),
                    None => {
                        self.$field = None;
                        None
                    }
                }
            };
        }
        let fill = el.decoration.fill;
        if let Some(v) = go!(AnimProp::Fill, fill, fill, Some(Color::TRANSPARENT)) {
            el.decoration.fill = v;
        }
        let border = el.decoration.border;
        let gone = Some((Color::TRANSPARENT, 0.0));
        if let Some(v) = go!(AnimProp::Border, border, border, gone) {
            el.decoration.border = v.map(|(c, w)| (c, w.max(0.0)));
        }
        let radius = Some(el.decoration.radius);
        if let Some(Some(v)) = go!(AnimProp::Radius, radius, radius, None) {
            el.decoration.radius = v.max(0.0);
        }
        let padding = Some(el.layout.padding);
        if let Some(Some(v)) = go!(AnimProp::Padding, padding, padding, None) {
            el.layout.padding = v;
        }
        let width = el.layout.size.width;
        if let Some(v) = go!(AnimProp::Width, width, width, None) {
            el.layout.size.width = v.map(|w| w.max(0.0));
        }
        let height = el.layout.size.height;
        if let Some(v) = go!(AnimProp::Height, height, height, None) {
            el.layout.size.height = v.map(|h| h.max(0.0));
        }
        let text = match &el.kind {
            ElementKind::Text { color, .. } => Some(*color),
            _ => None,
        };
        if let (Some(Some(v)), ElementKind::Text { color, .. }) = (
            go!(AnimProp::TextColor, text_color, text, None),
            &mut el.kind,
        ) {
            *color = v;
        }
        let opacity = Some(el.opacity as f64);
        if let Some(Some(v)) = go!(AnimProp::Opacity, opacity, opacity, None) {
            el.opacity = v.clamp(0.0, 1.0) as f32;
        }
        let transform = Some(el.transform.unwrap_or(Transform::IDENTITY));
        if let Some(Some(t)) = go!(AnimProp::Transform, transform, transform, None) {
            el.transform = (!t.is_identity()).then_some(t);
        }
    }

    fn moving(&self, now: Duration) -> bool {
        moving(&self.fill, now)
            || moving(&self.border, now)
            || moving(&self.radius, now)
            || moving(&self.padding, now)
            || moving(&self.width, now)
            || moving(&self.height, now)
            || moving(&self.text_color, now)
            || moving(&self.opacity, now)
            || moving(&self.transform, now)
    }
}

/// The in-flight property values of one window's transitioning elements, by
/// element identity.
#[derive(Clone, Debug, Default)]
pub(crate) struct Transitions {
    entries: HashMap<ElementKey, Tracks>,
    /// The frame time of the build (`None`: no clock).
    now: Option<Duration>,
}

impl Transitions {
    /// Start a build at `now`: elements not animated before
    /// [`end`](Self::end) are forgotten.
    pub(crate) fn begin(&mut self, now: Option<Duration>) {
        self.now = now;
        for e in self.entries.values_mut() {
            e.seen = false;
        }
    }

    /// Finish a build, forgetting the elements it didn't have.
    pub(crate) fn end(&mut self) {
        self.entries.retain(|_, e| e.seen);
    }

    /// Animate element `id`'s properties as `specs` say (a transition for
    /// every property, `None`, or for one; later ones win). `el` holds the
    /// targets on entry and the values to draw on return. An element seen
    /// for the first time starts at its targets.
    pub(crate) fn animate(
        &mut self,
        id: ElementKey,
        el: &mut Element,
        specs: &[(Option<AnimProp>, Transition)],
    ) {
        let now = self.now;
        let entry = self.entries.entry(id).or_default();
        entry.seen = true;
        // A change counts from one frame before this one (but not before the
        // last build), so it already shows on the frame that sees it.
        let head = now.map_or(Duration::ZERO, |now| {
            let frame = now.saturating_sub(crate::platform::frame::interval());
            entry.last.map_or(frame, |last| frame.max(last.min(now)))
        });
        let motion_of = |prop: AnimProp| {
            specs
                .iter()
                .rev()
                .find(|(p, _)| p.is_none_or(|p| p == prop))
                .map(|(_, t)| t.0)
        };
        entry.animate(el, motion_of, now, head);
        entry.last = now;
    }

    /// Whether anything is still moving (as of the last build).
    pub(crate) fn animating(&self) -> bool {
        self.now
            .is_some_and(|now| self.entries.values().any(|e| e.moving(now)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Axis;

    const MS: Duration = Duration::from_millis(1);

    fn frame() -> Duration {
        crate::platform::frame::interval()
    }

    fn boxed(fill: Color, pad: f64) -> Element {
        Element::stack(Axis::Vertical, vec![])
            .fill(fill)
            .padding(Insets::uniform(pad))
    }

    const LINEAR: [(Option<AnimProp>, Transition); 1] = [(
        None,
        Transition::tween(Duration::from_millis(100), Easing::Linear),
    )];

    /// Animate `el` as element 1 at `now` with `specs`, returning what to draw.
    fn run(
        tr: &mut Transitions,
        specs: &[(Option<AnimProp>, Transition)],
        mut el: Element,
        now: Duration,
    ) -> Element {
        tr.begin(Some(now));
        tr.animate(ElementKey(1), &mut el, specs);
        tr.end();
        el
    }

    fn step(tr: &mut Transitions, el: Element, now: Duration) -> Element {
        run(tr, &LINEAR, el, now)
    }

    #[test]
    fn a_change_tweens_and_settles() {
        let mut tr = Transitions::default();
        let t0 = Duration::from_secs(1);
        let first = step(&mut tr, boxed(Color::rgb(0, 0, 0), 10.0), t0);
        assert_eq!(first.layout.padding.top, 10.0, "first sight: no motion");
        assert!(!tr.animating(), "at rest: no frames");

        // The padding goes to 20: on the change's own frame it is one frame
        // in, then halfway at 50 ms (linear), then there.
        let t1 = t0 + 500 * MS;
        let a = step(&mut tr, boxed(Color::rgb(0, 0, 0), 20.0), t1);
        let one = frame().as_secs_f64() / 0.1;
        assert!((a.layout.padding.top - (10.0 + 10.0 * one)).abs() < 1e-9);
        assert!(tr.animating());
        let mid_t = t1 + 50 * MS - frame();
        let mid = step(&mut tr, boxed(Color::rgb(0, 0, 0), 20.0), mid_t);
        assert!((mid.layout.padding.top - 15.0).abs() < 1e-9, "{mid:?}");
        let end_t = t1 + 100 * MS - frame();
        let end = step(&mut tr, boxed(Color::rgb(0, 0, 0), 20.0), end_t);
        assert_eq!(end.layout.padding.top, 20.0);
        assert!(!tr.animating(), "settled: no more frames");
    }

    #[test]
    fn colors_interpolate() {
        let mut tr = Transitions::default();
        let t0 = Duration::from_secs(1);
        step(&mut tr, boxed(Color::rgb(0, 0, 0), 0.0), t0);
        let t1 = t0 + frame();
        step(&mut tr, boxed(Color::rgb(200, 100, 0), 0.0), t1);
        let mid = step(
            &mut tr,
            boxed(Color::rgb(200, 100, 0), 0.0),
            t1 + 50 * MS - frame(),
        );
        assert_eq!(mid.decoration.fill, Some(Color::rgb(100, 50, 0)));
    }

    #[test]
    fn retargeting_mid_flight_continues_from_the_current_value() {
        let mut tr = Transitions::default();
        let t0 = Duration::from_secs(1);
        step(&mut tr, boxed(Color::rgb(0, 0, 0), 0.0), t0);
        let t1 = t0 + frame();
        step(&mut tr, boxed(Color::rgb(0, 0, 0), 100.0), t1);
        let t2 = t1 + 50 * MS - frame();
        let mid = step(&mut tr, boxed(Color::rgb(0, 0, 0), 100.0), t2);
        assert!((mid.layout.padding.top - 50.0).abs() < 1e-9);
        // Back to 0 from 50, a frame later: it is still near 50 (no jump back
        // to 100 or to 0), and it turns round smoothly.
        let t3 = t2 + frame();
        let back = step(&mut tr, boxed(Color::rgb(0, 0, 0), 0.0), t3);
        let p = back.layout.padding.top;
        assert!((p - 50.0).abs() < 20.0, "{p}");
        assert!(tr.animating());
        let done = step(&mut tr, boxed(Color::rgb(0, 0, 0), 0.0), t3 + 100 * MS);
        assert_eq!(done.layout.padding.top, 0.0);
        assert!(!tr.animating());
    }

    #[test]
    fn a_spring_keeps_its_velocity_when_retargeted() {
        let spring = [(None, Transition::spring(SpringParams::new(200.0, 0.5)))];
        let mut tr = Transitions::default();
        let pad = |tr: &mut Transitions, p: f64, now: Duration| {
            run(tr, &spring, boxed(Color::rgb(0, 0, 0), p), now)
                .layout
                .padding
                .top
        };
        let t0 = Duration::from_secs(1);
        pad(&mut tr, 0.0, t0);
        let mut t = t0 + frame();
        let mut last = pad(&mut tr, 10.0, t);
        assert!(last > 0.0 && last < 10.0);
        // Underdamped: it overshoots 10.
        let mut peak = last;
        for _ in 0..30 {
            t += frame();
            last = pad(&mut tr, 10.0, t);
            peak = peak.max(last);
        }
        assert!(peak > 10.0, "overshoots: {peak}");
        // Retarget while moving: the next value is close to the last one (no
        // jump), and it settles at the new target.
        let before = last;
        t += frame();
        let after = pad(&mut tr, 0.0, t);
        assert!((after - before).abs() < 2.0, "{before} -> {after}");
        for _ in 0..600 {
            t += frame();
            last = pad(&mut tr, 0.0, t);
            if !tr.animating() {
                break;
            }
        }
        assert!(!tr.animating(), "the spring comes to rest");
        assert!(last.abs() < 0.01, "{last}");
    }

    #[test]
    fn a_fill_fades_in_and_out() {
        let mut tr = Transitions::default();
        let t0 = Duration::from_secs(1);
        let bare = Element::stack(Axis::Vertical, vec![]);
        step(&mut tr, bare.clone(), t0);
        let t1 = t0 + frame();
        let red = bare.clone().fill(Color::rgb(255, 0, 0));
        let a = step(&mut tr, red.clone(), t1);
        let fill = a.decoration.fill.expect("fading in");
        assert_eq!((fill.r, fill.g, fill.b), (255, 0, 0), "keeps its hue");
        assert!(fill.a > 0 && fill.a < 255, "{fill:?}");
        step(&mut tr, red, t1 + 200 * MS);
        let t2 = t1 + 300 * MS;
        let out = step(&mut tr, bare.clone(), t2).decoration.fill;
        assert!(out.is_some_and(|c| c.r == 255 && c.a < 255), "{out:?}");
        let gone = step(&mut tr, bare, t2 + 200 * MS);
        assert_eq!(gone.decoration.fill, None, "faded out, then gone");
        assert!(!tr.animating());
    }

    #[test]
    fn opacity_and_transforms_animate() {
        let mut tr = Transitions::default();
        let t0 = Duration::from_secs(1);
        let plain = Element::stack(Axis::Vertical, vec![]);
        step(&mut tr, plain.clone(), t0);
        let t1 = t0 + frame();
        let to = Transform::IDENTITY.scale(2.0).rotate(1.0);
        let moved = plain.clone().opacity(0.0).transform(to);
        step(&mut tr, moved.clone(), t1);
        let mid = step(&mut tr, moved.clone(), t1 + 50 * MS - frame());
        assert!((mid.opacity - 0.5).abs() < 1e-6, "{}", mid.opacity);
        let want = Transform::lerp(Transform::IDENTITY, to, 0.5);
        let got = mid.transform.expect("mid-way");
        assert!((got.scale.dx - want.scale.dx).abs() < 1e-9);
        assert!((got.rotate - want.rotate).abs() < 1e-9);
        // Back to none: it ends at the identity, drawn as no transform.
        step(&mut tr, moved, t1 + 200 * MS);
        let t2 = t1 + 300 * MS;
        step(&mut tr, plain.clone(), t2);
        let back = step(&mut tr, plain, t2 + 200 * MS);
        assert_eq!(back.transform, None);
        assert_eq!(back.opacity, 1.0);
        assert!(!tr.animating());
    }

    #[test]
    fn one_property_can_switch_while_the_rest_animate() {
        let specs = [
            (None, LINEAR[0].1),
            (Some(AnimProp::Fill), Transition::instant()),
        ];
        let mut tr = Transitions::default();
        let t0 = Duration::from_secs(1);
        run(&mut tr, &specs, boxed(Color::rgb(0, 0, 0), 0.0), t0);
        let t1 = t0 + 50 * MS;
        let el = run(&mut tr, &specs, boxed(Color::rgb(9, 9, 9), 10.0), t1);
        assert_eq!(el.decoration.fill, Some(Color::rgb(9, 9, 9)));
        assert!(el.layout.padding.top < 10.0);
    }

    #[test]
    fn without_a_clock_changes_switch() {
        let mut tr = Transitions::default();
        let mut el = boxed(Color::rgb(0, 0, 0), 0.0);
        tr.begin(None);
        tr.animate(ElementKey(1), &mut el, &LINEAR);
        let mut el = boxed(Color::rgb(0, 0, 0), 30.0);
        tr.animate(ElementKey(1), &mut el, &LINEAR);
        assert_eq!(el.layout.padding.top, 30.0);
        assert!(!tr.animating());
    }

    #[test]
    fn elements_that_go_away_are_forgotten() {
        let mut tr = Transitions::default();
        step(
            &mut tr,
            boxed(Color::rgb(0, 0, 0), 0.0),
            Duration::from_secs(1),
        );
        tr.begin(None);
        tr.end();
        assert!(tr.entries.is_empty());
    }
}
