//! iOS's interactive back swipe, recognized from the view's touches: a
//! finger that starts at the leading edge (the left, or the right under a
//! right-to-left layout) and moves inward is the back gesture, as in a
//! `UINavigationController`, while the app would go back
//! ([`set_back_enabled`](crate::platform::set_back_enabled)).
//!
//! Recognized by hand rather than with a `UIScreenEdgePanGestureRecognizer`:
//! the decision then happens on the same path as every touch, including the
//! synthetic ones self-tests send, and stays free of UIKit, so it is
//! unit-tested on every host. It works like UIKit's recognizer with
//! `cancelsTouchesInView`: the finger reaches the app as usual until it is
//! recognized, then is cancelled there and becomes the gesture.
//!
//! - A first finger down within [`EDGE`] points of the leading edge, while
//!   back is enabled, is a candidate. Its `Started` reaches the app (a tap at
//!   the edge still taps); its moves wait while it's undecided.
//! - Moving [`SLOP`] points inward, mostly sideways, recognizes it: the app's
//!   touch is cancelled, and [`BackGesture::Started`] and then
//!   [`BackGesture::Progressed`] (the inward distance over the view's width)
//!   follow the finger.
//! - Moving mostly up or down first, or a second finger, gives the finger
//!   back to the app (its latest move delivered).
//! - Lifting a recognized finger past half the width, or flicking it inward,
//!   goes back ([`Out::Back`]); otherwise it is [`BackGesture::Cancelled`].
//!
//! While back isn't enabled nothing is recognized and the edge's touches
//! (scrolling a list near the edge, say) are the app's alone.

use crate::geometry::Point;
use crate::platform::event::TouchPhase;
use crate::platform::{BackEdge, BackGesture};

/// How far from the edge (points) a finger may start.
pub(crate) const EDGE: f64 = 24.0;
/// How far inward (points) the finger moves before it is the gesture.
pub(crate) const SLOP: f64 = 10.0;
/// An inward speed (points per second) at the lift that goes back however
/// far the finger went.
const FLICK: f64 = 500.0;

/// What to do with a touch the recognizer saw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Out {
    /// Deliver the touch to the app as usual.
    Touch,
    /// Deliver it to the app as cancelled: the gesture took the finger.
    CancelTouch,
    /// The back gesture moved on.
    Gesture(BackGesture),
    /// The gesture completed: go back.
    Back,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    /// A finger down at the edge, not decided yet.
    Candidate { id: u64, start: Point },
    /// The finger is the back gesture.
    Tracking {
        id: u64,
        start: Point,
        edge: BackEdge,
        /// The last inward distance and when it was seen.
        last: (f64, f64),
        /// Inward points per second, over the last move.
        velocity: f64,
    },
}

/// The recognizer: one per view.
#[derive(Debug, Default)]
pub(crate) struct EdgeSwipe {
    state: State,
    /// Fingers down that the app has (other than the candidate).
    others: usize,
}

/// What the view looks like now: its width in points, whether back is
/// enabled and whether the layout is right-to-left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Context {
    pub(crate) width: f64,
    pub(crate) enabled: bool,
    pub(crate) rtl: bool,
}

impl Context {
    /// The inward distance of `to` from `from`.
    fn inward(&self, from: Point, to: Point) -> f64 {
        if self.rtl {
            from.x - to.x
        } else {
            to.x - from.x
        }
    }
}

impl EdgeSwipe {
    /// Finger `id` at `at` (the view's points) changed with `phase`, at `time`
    /// seconds. Returns what to deliver, in order.
    pub(crate) fn touch(
        &mut self,
        phase: TouchPhase,
        id: u64,
        at: Point,
        time: f64,
        cx: Context,
    ) -> Vec<Out> {
        match (self.state, phase) {
            (State::Idle, TouchPhase::Started) => {
                let at_edge = if cx.rtl {
                    at.x >= cx.width - EDGE
                } else {
                    at.x <= EDGE
                };
                if cx.enabled && self.others == 0 && at_edge && cx.width > 0.0 {
                    self.state = State::Candidate { id, start: at };
                } else {
                    self.others += 1;
                }
                vec![Out::Touch]
            }
            (State::Candidate { .. }, TouchPhase::Started) => {
                // A second finger: not a back swipe; the first is the app's.
                self.state = State::Idle;
                self.others += 2;
                vec![Out::Touch]
            }
            (State::Candidate { id: c, start }, TouchPhase::Moved) if c == id => {
                let inward = cx.inward(start, at);
                let across = (at.y - start.y).abs();
                if inward >= SLOP && inward >= 2.0 * across {
                    let edge = if cx.rtl {
                        BackEdge::Right
                    } else {
                        BackEdge::Left
                    };
                    self.state = State::Tracking {
                        id,
                        start,
                        edge,
                        last: (inward, time),
                        velocity: 0.0,
                    };
                    vec![
                        Out::CancelTouch,
                        Out::Gesture(BackGesture::Started { edge }),
                        Out::Gesture(BackGesture::Progressed {
                            progress: progress(inward, cx.width),
                        }),
                    ]
                } else if across >= SLOP || inward <= -SLOP {
                    // Up, down or outward: the app's.
                    self.state = State::Idle;
                    self.others += 1;
                    vec![Out::Touch]
                } else {
                    Vec::new() // undecided: the move waits
                }
            }
            (State::Candidate { id: c, .. }, TouchPhase::Ended | TouchPhase::Cancelled)
                if c == id =>
            {
                // A tap at the edge.
                self.state = State::Idle;
                vec![Out::Touch]
            }
            (
                State::Tracking {
                    id: c,
                    start,
                    edge,
                    last,
                    velocity,
                },
                TouchPhase::Moved,
            ) if c == id => {
                let inward = cx.inward(start, at);
                let dt = time - last.1;
                let velocity = if dt > 0.0 {
                    (inward - last.0) / dt
                } else {
                    velocity
                };
                self.state = State::Tracking {
                    id,
                    start,
                    edge,
                    last: (inward, time),
                    velocity,
                };
                vec![Out::Gesture(BackGesture::Progressed {
                    progress: progress(inward, cx.width),
                })]
            }
            (
                State::Tracking {
                    id: c,
                    start,
                    last,
                    velocity,
                    ..
                },
                TouchPhase::Ended,
            ) if c == id => {
                self.state = State::Idle;
                let inward = cx.inward(start, at);
                let dt = time - last.1;
                // A lift long after the last move is no flick.
                let velocity = if dt > 0.1 { 0.0 } else { velocity };
                let far = progress(inward, cx.width) >= 0.5;
                if velocity >= FLICK || (far && velocity > -FLICK) {
                    vec![Out::Back]
                } else {
                    vec![Out::Gesture(BackGesture::Cancelled)]
                }
            }
            (State::Tracking { id: c, .. }, TouchPhase::Cancelled) if c == id => {
                self.state = State::Idle;
                vec![Out::Gesture(BackGesture::Cancelled)]
            }
            (State::Tracking { .. }, TouchPhase::Started) => {
                // Another finger during the gesture: the app's, as usual.
                self.others += 1;
                vec![Out::Touch]
            }
            (_, TouchPhase::Ended | TouchPhase::Cancelled) => {
                self.others = self.others.saturating_sub(1);
                vec![Out::Touch]
            }
            _ => vec![Out::Touch],
        }
    }
}

/// The gesture's progress: `inward` points over the view's `width`, from 0
/// to 1.
fn progress(inward: f64, width: f64) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    (inward / width).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const LTR: Context = Context {
        width: 400.0,
        enabled: true,
        rtl: false,
    };

    /// Drag finger 1 from `from` to `to` in `steps` moves 16 ms apart, then
    /// lift it; every output in order.
    fn drag(r: &mut EdgeSwipe, cx: Context, from: Point, to: Point, steps: u32) -> Vec<Out> {
        let mut out = r.touch(TouchPhase::Started, 1, from, 0.0, cx);
        let mut t = 0.0;
        for i in 1..=steps {
            let f = f64::from(i) / f64::from(steps);
            let at = Point::new(from.x + (to.x - from.x) * f, from.y + (to.y - from.y) * f);
            t += 0.016;
            out.extend(r.touch(TouchPhase::Moved, 1, at, t, cx));
        }
        out.extend(r.touch(TouchPhase::Ended, 1, to, t + 0.008, cx));
        out
    }

    fn gestures(out: &[Out]) -> Vec<Out> {
        out.iter()
            .copied()
            .filter(|o| !matches!(o, Out::Gesture(BackGesture::Progressed { .. })))
            .collect()
    }

    #[test]
    fn a_swipe_from_the_left_edge_goes_back() {
        let mut r = EdgeSwipe::default();
        let out = drag(
            &mut r,
            LTR,
            Point::new(4.0, 300.0),
            Point::new(250.0, 305.0),
            10,
        );
        assert_eq!(
            gestures(&out),
            [
                Out::Touch, // the press reached the app
                Out::CancelTouch,
                Out::Gesture(BackGesture::Started {
                    edge: BackEdge::Left
                }),
                Out::Back,
            ]
        );
        let progress: Vec<f32> = out
            .iter()
            .filter_map(|o| match o {
                Out::Gesture(BackGesture::Progressed { progress }) => Some(*progress),
                _ => None,
            })
            .collect();
        assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
        assert!(
            progress
                .last()
                .is_some_and(|p| (p - 246.0 / 400.0).abs() < 0.01)
        );
        assert_eq!(r.state, State::Idle);
    }

    #[test]
    fn a_short_slow_swipe_is_cancelled() {
        let mut r = EdgeSwipe::default();
        // 60 points over two seconds: neither far nor fast.
        let mut out = r.touch(TouchPhase::Started, 1, Point::new(2.0, 300.0), 0.0, LTR);
        for i in 1..=6 {
            let at = Point::new(2.0 + 10.0 * f64::from(i), 300.0);
            out.extend(r.touch(TouchPhase::Moved, 1, at, 0.3 * f64::from(i), LTR));
        }
        out.extend(r.touch(TouchPhase::Ended, 1, Point::new(62.0, 300.0), 2.0, LTR));
        assert_eq!(
            gestures(&out).last(),
            Some(&Out::Gesture(BackGesture::Cancelled))
        );
        // Out past half way, then taken back quickly towards the edge.
        let mut out = r.touch(TouchPhase::Started, 1, Point::new(2.0, 300.0), 0.0, LTR);
        let xs = [20.0, 120.0, 260.0, 290.0, 270.0, 205.0];
        for (i, x) in xs.into_iter().enumerate() {
            let t = 0.1 * (i + 1) as f64;
            out.extend(r.touch(TouchPhase::Moved, 1, Point::new(x, 300.0), t, LTR));
        }
        out.extend(r.touch(TouchPhase::Ended, 1, Point::new(205.0, 300.0), 0.62, LTR));
        assert_eq!(
            gestures(&out).last(),
            Some(&Out::Gesture(BackGesture::Cancelled))
        );
    }

    #[test]
    fn a_quick_flick_goes_back_however_short() {
        let mut r = EdgeSwipe::default();
        let out = drag(
            &mut r,
            LTR,
            Point::new(2.0, 300.0),
            Point::new(80.0, 300.0),
            3,
        );
        assert_eq!(gestures(&out).last(), Some(&Out::Back));
    }

    #[test]
    fn a_vertical_drag_at_the_edge_stays_the_apps() {
        let mut r = EdgeSwipe::default();
        let out = drag(
            &mut r,
            LTR,
            Point::new(4.0, 300.0),
            Point::new(8.0, 100.0),
            10,
        );
        assert!(out.iter().all(|o| *o == Out::Touch), "{out:?}");
        // The moves before it decided waited; the rest went through.
        assert!(out.len() >= 8, "{out:?}");
    }

    #[test]
    fn nothing_is_recognized_while_back_is_off_or_away_from_the_edge() {
        let mut r = EdgeSwipe::default();
        let off = Context {
            enabled: false,
            ..LTR
        };
        let out = drag(
            &mut r,
            off,
            Point::new(4.0, 300.0),
            Point::new(300.0, 300.0),
            10,
        );
        assert_eq!(out, vec![Out::Touch; 12]);
        let out = drag(
            &mut r,
            LTR,
            Point::new(60.0, 300.0),
            Point::new(300.0, 300.0),
            10,
        );
        assert_eq!(out, vec![Out::Touch; 12]);
    }

    #[test]
    fn right_to_left_swipes_from_the_right_edge() {
        let mut r = EdgeSwipe::default();
        let rtl = Context { rtl: true, ..LTR };
        let out = drag(
            &mut r,
            rtl,
            Point::new(4.0, 300.0),
            Point::new(300.0, 300.0),
            10,
        );
        assert_eq!(
            out,
            vec![Out::Touch; 12],
            "the left edge isn't the leading one"
        );
        let out = drag(
            &mut r,
            rtl,
            Point::new(396.0, 300.0),
            Point::new(120.0, 300.0),
            10,
        );
        assert_eq!(
            gestures(&out)[1..],
            [
                Out::CancelTouch,
                Out::Gesture(BackGesture::Started {
                    edge: BackEdge::Right
                }),
                Out::Back,
            ]
        );
    }

    #[test]
    fn a_tap_at_the_edge_and_a_second_finger_reach_the_app() {
        let mut r = EdgeSwipe::default();
        let at = Point::new(3.0, 300.0);
        assert_eq!(r.touch(TouchPhase::Started, 1, at, 0.0, LTR), [Out::Touch]);
        assert_eq!(r.touch(TouchPhase::Ended, 1, at, 0.1, LTR), [Out::Touch]);
        // A pinch whose first finger lands at the edge.
        r.touch(TouchPhase::Started, 1, at, 0.0, LTR);
        assert_eq!(
            r.touch(TouchPhase::Started, 2, Point::new(200.0, 300.0), 0.01, LTR),
            [Out::Touch]
        );
        let moved = r.touch(TouchPhase::Moved, 1, Point::new(100.0, 300.0), 0.05, LTR);
        assert_eq!(moved, [Out::Touch]);
        r.touch(TouchPhase::Ended, 1, Point::new(100.0, 300.0), 0.1, LTR);
        r.touch(TouchPhase::Ended, 2, Point::new(200.0, 300.0), 0.1, LTR);
        assert_eq!(r.others, 0);
        // Then a swipe works again.
        let out = drag(&mut r, LTR, at, Point::new(300.0, 300.0), 10);
        assert_eq!(gestures(&out).last(), Some(&Out::Back));
    }

    #[test]
    fn a_cancelled_finger_cancels_the_gesture() {
        let mut r = EdgeSwipe::default();
        r.touch(TouchPhase::Started, 1, Point::new(3.0, 300.0), 0.0, LTR);
        r.touch(TouchPhase::Moved, 1, Point::new(40.0, 300.0), 0.02, LTR);
        assert_eq!(
            r.touch(TouchPhase::Cancelled, 1, Point::new(40.0, 300.0), 0.03, LTR),
            [Out::Gesture(BackGesture::Cancelled)]
        );
    }
}
