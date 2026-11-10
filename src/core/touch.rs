//! Touch scrolling: a finger dragged inside a scroll container scrolls it, and
//! keeps going (a fling) when it lifts while moving.
//!
//! [`TouchScroll`] is the gesture's state machine, kept free of the app so it
//! can be fed synthetic touches in tests. The app hands it every
//! [`Touch`](crate::platform::Touch), asks it whether the first finger's
//! pointer events should still reach widgets, and ticks it once per frame
//! while it flings.
//!
//! A press doesn't become a scroll until the finger has moved more than
//! [`SLOP`] logical pixels, mostly vertically, so a tap still clicks. Once it
//! scrolls, the press is cancelled (the app clears the pressed button and any
//! text selection the press started) and the finger's pointer events stop
//! reaching widgets until it lifts. A finger that lands on a fling stops it,
//! and that touch doesn't click either. Under
//! [reduced motion](crate::anim::reduced_motion) there's no fling: the content
//! stops where the finger lifts.

use crate::geometry::Point;
use crate::platform::{Touch, TouchPhase};

use super::ScrollId;

/// How far (logical pixels) a finger moves before a press becomes a scroll.
pub const SLOP: f64 = 8.0;
/// Fling speeds below this (logical px/s) aren't worth animating.
const MIN_FLING: f64 = 50.0;
/// The fastest fling (logical px/s).
const MAX_FLING: f64 = 8000.0;
/// A fling stops once it slows below this (logical px/s).
const STOP_FLING: f64 = 20.0;
/// How quickly a fling slows: its speed falls by `1/e` every `DECAY` seconds,
/// roughly the deceleration of iOS's and Android's scroll views.
const DECAY: f64 = 0.325;
/// The span of finger movement (seconds) the release velocity is measured over.
const VELOCITY_WINDOW: f64 = 0.1;

/// What the app should do after a touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TouchAction {
    /// Nothing changes for the app.
    None,
    /// The press became a scroll: cancel the pressed widget and any selection
    /// it started, then scroll `target` by `dy`.
    Begin { target: ScrollId, dy: f64 },
    /// Scroll `target` by `dy` logical pixels (positive reveals content further
    /// down). A release that moved since the last move sends this instead of
    /// [`End`](Self::End), and may start a fling too.
    Scroll { target: ScrollId, dy: f64 },
    /// The scrolling finger lifted. If it was moving, a fling follows: call
    /// [`TouchScroll::tick`] every frame while [`TouchScroll::flinging`].
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    /// The first finger is down and hasn't moved past the slop. `target` is the
    /// scroll container under it, if any; `caught` when it landed on a fling.
    Pending {
        id: u64,
        start: Point,
        target: Option<ScrollId>,
        caught: bool,
    },
    /// The first finger is down and its gesture belongs to a widget (a slider,
    /// a sideways swipe): its pointer events pass through untouched.
    Yielded {
        id: u64,
    },
    /// The first finger is scrolling `target`.
    Dragging {
        id: u64,
        target: ScrollId,
        last_y: f64,
    },
    /// The finger lifted while moving; `target` keeps scrolling at `velocity`
    /// (logical px/s, in offset terms), slowing down.
    Flinging {
        target: ScrollId,
        velocity: f64,
        last: f64,
    },
}

/// The touch-scrolling state machine. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct TouchScroll {
    state: State,
    /// Recent finger positions while dragging: (time, y).
    samples: Vec<(f64, f64)>,
}

impl Default for TouchScroll {
    fn default() -> Self {
        Self {
            state: State::Idle,
            samples: Vec::new(),
        }
    }
}

impl TouchScroll {
    /// Feed one touch at time `now` (seconds, any fixed origin). `target_at`
    /// finds the scroll container under a point, asked once, when the first
    /// finger goes down. Other fingers are ignored.
    pub fn touch(
        &mut self,
        touch: &Touch,
        now: f64,
        target_at: impl FnOnce(Point) -> Option<ScrollId>,
    ) -> TouchAction {
        let p = touch.position;
        match (self.state, touch.phase) {
            (State::Idle | State::Flinging { .. }, TouchPhase::Started) => {
                let caught = matches!(self.state, State::Flinging { .. });
                self.state = State::Pending {
                    id: touch.id,
                    start: p,
                    target: target_at(p),
                    caught,
                };
                TouchAction::None
            }
            (
                State::Pending {
                    id,
                    start,
                    target,
                    caught,
                },
                TouchPhase::Moved,
            ) if id == touch.id => {
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                if dx.abs() <= SLOP && dy.abs() <= SLOP {
                    return TouchAction::None;
                }
                match target {
                    // Mostly vertical: scroll, from where the finger crossed
                    // the slop so the content doesn't jump.
                    Some(target) if dy.abs() >= dx.abs() => {
                        let crossed = start.y + SLOP.copysign(dy);
                        self.state = State::Dragging {
                            id,
                            target,
                            last_y: p.y,
                        };
                        self.samples.clear();
                        self.samples.push((now, p.y));
                        TouchAction::Begin {
                            target,
                            dy: crossed - p.y,
                        }
                    }
                    // A sideways drag, or nothing to scroll: the widget under
                    // the finger has it (a caught fling's touch stays inert).
                    _ if !caught => {
                        self.state = State::Yielded { id };
                        TouchAction::None
                    }
                    _ => TouchAction::None,
                }
            }
            (State::Dragging { id, target, last_y }, TouchPhase::Moved) if id == touch.id => {
                self.state = State::Dragging {
                    id,
                    target,
                    last_y: p.y,
                };
                self.sample(now, p.y);
                let dy = last_y - p.y;
                if dy == 0.0 {
                    TouchAction::None
                } else {
                    TouchAction::Scroll { target, dy }
                }
            }
            (State::Dragging { id, target, last_y }, TouchPhase::Ended) if id == touch.id => {
                self.sample(now, p.y);
                let velocity = self.release_velocity(now);
                // Under reduced motion the content stops with the finger.
                self.state = if velocity.abs() >= MIN_FLING && !crate::anim::reduced_motion() {
                    State::Flinging {
                        target,
                        velocity: velocity.clamp(-MAX_FLING, MAX_FLING),
                        last: now,
                    }
                } else {
                    State::Idle
                };
                self.samples.clear();
                // A last move folded into the release still scrolls.
                let dy = last_y - p.y;
                if dy != 0.0 {
                    return TouchAction::Scroll { target, dy };
                }
                TouchAction::End
            }
            // A release far from where the finger went down with no move in
            // between: a busy app can have its moves folded into the release.
            // It scrolls the whole way, without a fling.
            (
                State::Pending {
                    id,
                    start,
                    target: Some(target),
                    ..
                },
                TouchPhase::Ended,
            ) if id == touch.id
                && (p.y - start.y).abs() > SLOP
                && (p.y - start.y).abs() >= (p.x - start.x).abs() =>
            {
                self.state = State::Idle;
                TouchAction::Begin {
                    target,
                    dy: start.y - p.y,
                }
            }
            (
                State::Pending { id, .. } | State::Yielded { id } | State::Dragging { id, .. },
                TouchPhase::Ended | TouchPhase::Cancelled,
            ) if id == touch.id => {
                let was_dragging = matches!(self.state, State::Dragging { .. });
                self.state = State::Idle;
                self.samples.clear();
                if was_dragging {
                    TouchAction::End
                } else {
                    TouchAction::None
                }
            }
            _ => TouchAction::None,
        }
    }

    /// Advance a fling to `now`: how far its container scrolls this frame, or
    /// `None` when there's no fling (it stops on its own once slow enough).
    pub fn tick(&mut self, now: f64) -> Option<(ScrollId, f64)> {
        let State::Flinging {
            target,
            velocity,
            last,
        } = self.state
        else {
            return None;
        };
        let dt = (now - last).max(0.0);
        let decay = (-dt / DECAY).exp();
        // The distance covered while the speed decays exponentially over dt.
        let dy = velocity * DECAY * (1.0 - decay);
        let velocity = velocity * decay;
        self.state = if velocity.abs() < STOP_FLING {
            State::Idle
        } else {
            State::Flinging {
                target,
                velocity,
                last: now,
            }
        };
        Some((target, dy))
    }

    /// Stop a fling (it reached the end of its content).
    pub fn stop_fling(&mut self) {
        if matches!(self.state, State::Flinging { .. }) {
            self.state = State::Idle;
        }
    }

    /// Hand the first finger's gesture to the widget it pressed (a slider or
    /// other draggable latched it), so it never scrolls.
    pub fn yield_to_widget(&mut self) {
        if let State::Pending { id, caught, .. } = self.state
            && !caught
        {
            self.state = State::Yielded { id };
        }
    }

    /// Whether a fling is running (the app keeps asking for frames).
    pub fn flinging(&self) -> bool {
        matches!(self.state, State::Flinging { .. })
    }

    /// Whether the first finger is scrolling.
    #[cfg(test)]
    pub fn dragging(&self) -> bool {
        matches!(self.state, State::Dragging { .. })
    }

    /// Whether the first finger's press should be ignored: it landed on a
    /// fling to stop it.
    pub fn swallows_press(&self) -> bool {
        matches!(self.state, State::Pending { caught: true, .. })
    }

    /// Whether the first finger's pointer moves should be kept from widgets:
    /// it is scrolling, or may start to (it's inside a scroll container and
    /// hasn't left the slop), or it caught a fling.
    pub fn holds_moves(&self) -> bool {
        match self.state {
            State::Pending { target, caught, .. } => target.is_some() || caught,
            State::Dragging { .. } => true,
            _ => false,
        }
    }

    fn sample(&mut self, now: f64, y: f64) {
        self.samples.push((now, y));
        // Keep a little more than the window, for a release after a pause.
        let keep = now - VELOCITY_WINDOW * 2.0;
        if let Some(first) = self.samples.iter().position(|&(t, _)| t >= keep) {
            self.samples
                .drain(..first.min(self.samples.len().saturating_sub(2)));
        }
    }

    /// The finger's speed at release, in offset terms (a finger moving up
    /// scrolls towards the end: positive). Zero if it paused before lifting.
    fn release_velocity(&self, now: f64) -> f64 {
        let recent: Vec<_> = self
            .samples
            .iter()
            .filter(|&&(t, _)| t >= now - VELOCITY_WINDOW)
            .copied()
            .collect();
        let (Some(&(t0, y0)), Some(&(t1, y1))) = (recent.first(), recent.last()) else {
            return 0.0;
        };
        // Too short a span to measure: a single sample, or one burst of
        // events with no time between them.
        if t1 - t0 < 0.005 {
            return 0.0;
        }
        (y0 - y1) / (t1 - t0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: ScrollId = ScrollId(7);

    fn touch(id: u64, phase: TouchPhase, x: f64, y: f64) -> Touch {
        Touch {
            id,
            phase,
            position: Point::new(x, y),
        }
    }

    fn over_list(_: Point) -> Option<ScrollId> {
        Some(LIST)
    }

    fn nothing(_: Point) -> Option<ScrollId> {
        None
    }

    /// Drag finger 1 from y0 to y1 in `steps` moves, `dt` seconds apart,
    /// starting at time 0; returns the total scrolled and the end time.
    fn drag(s: &mut TouchScroll, y0: f64, y1: f64, steps: usize, dt: f64) -> (f64, f64) {
        assert_eq!(
            s.touch(&touch(1, TouchPhase::Started, 50.0, y0), 0.0, over_list),
            TouchAction::None
        );
        let mut scrolled = 0.0;
        for i in 1..=steps {
            let y = y0 + (y1 - y0) * i as f64 / steps as f64;
            match s.touch(
                &touch(1, TouchPhase::Moved, 50.0, y),
                i as f64 * dt,
                over_list,
            ) {
                TouchAction::Begin { target, dy } | TouchAction::Scroll { target, dy } => {
                    assert_eq!(target, LIST);
                    scrolled += dy;
                }
                TouchAction::None => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        (scrolled, steps as f64 * dt)
    }

    #[test]
    fn a_tap_stays_a_press() {
        let mut s = TouchScroll::default();
        s.touch(&touch(1, TouchPhase::Started, 50.0, 100.0), 0.0, over_list);
        assert!(!s.swallows_press());
        assert!(s.holds_moves(), "moves wait until the slop decides");
        // A jitter within the slop doesn't scroll.
        let a = s.touch(&touch(1, TouchPhase::Moved, 53.0, 104.0), 0.05, over_list);
        assert_eq!(a, TouchAction::None);
        assert!(!s.dragging());
        let a = s.touch(&touch(1, TouchPhase::Ended, 53.0, 104.0), 0.1, over_list);
        assert_eq!(a, TouchAction::None);
        assert!(!s.flinging() && !s.holds_moves());
    }

    #[test]
    fn a_drag_past_the_slop_scrolls_without_jumping() {
        let mut s = TouchScroll::default();
        s.touch(&touch(1, TouchPhase::Started, 50.0, 300.0), 0.0, over_list);
        // Up 20 px: the press becomes a scroll of 12 (20 less the slop).
        let a = s.touch(&touch(1, TouchPhase::Moved, 52.0, 280.0), 0.016, over_list);
        assert_eq!(
            a,
            TouchAction::Begin {
                target: LIST,
                dy: 20.0 - SLOP
            }
        );
        assert!(s.dragging() && s.holds_moves());
        // From then on the content follows the finger exactly.
        let a = s.touch(&touch(1, TouchPhase::Moved, 52.0, 250.0), 0.032, over_list);
        assert_eq!(
            a,
            TouchAction::Scroll {
                target: LIST,
                dy: 30.0
            }
        );
        // Back down scrolls back.
        let a = s.touch(&touch(1, TouchPhase::Moved, 52.0, 260.0), 0.048, over_list);
        assert_eq!(
            a,
            TouchAction::Scroll {
                target: LIST,
                dy: -10.0
            }
        );
    }

    #[test]
    fn a_slow_release_doesnt_fling() {
        let mut s = TouchScroll::default();
        let (scrolled, end) = drag(&mut s, 400.0, 200.0, 10, 0.02);
        assert_eq!(scrolled, 200.0 - SLOP);
        // The finger rests, then lifts where it is.
        s.touch(
            &touch(1, TouchPhase::Moved, 50.0, 200.0),
            end + 0.3,
            over_list,
        );
        let a = s.touch(
            &touch(1, TouchPhase::Ended, 50.0, 200.0),
            end + 0.4,
            over_list,
        );
        assert_eq!(a, TouchAction::End);
        assert!(!s.flinging());
        assert_eq!(s.tick(end + 0.5), None);
    }

    #[test]
    fn a_quick_release_flings_and_slows_to_a_stop() {
        let mut s = TouchScroll::default();
        // 300 px up in 100 ms: about 3000 px/s.
        let (_, end) = drag(&mut s, 500.0, 200.0, 6, 1.0 / 60.0);
        let a = s.touch(&touch(1, TouchPhase::Ended, 50.0, 200.0), end, over_list);
        assert_eq!(a, TouchAction::End);
        assert!(s.flinging());
        assert!(!s.holds_moves(), "the finger is gone");
        let mut t = end;
        let mut total = 0.0;
        let mut last = f64::INFINITY;
        let mut frames = 0;
        while let Some((target, dy)) = s.tick(t + 1.0 / 60.0) {
            t += 1.0 / 60.0;
            assert_eq!(target, LIST);
            assert!(dy > 0.0, "keeps going the way the finger went");
            assert!(dy <= last, "and slows down");
            last = dy;
            total += dy;
            frames += 1;
            assert!(frames < 600, "a fling ends");
        }
        // v * DECAY is how far an exponential decay travels in total.
        assert!(total > 500.0 && total < 3000.0 * DECAY, "flung {total}");
        assert!(!s.flinging());
    }

    #[test]
    fn under_reduced_motion_a_quick_release_stops_with_the_finger() {
        crate::anim::set_reduced_motion(Some(true));
        let mut s = TouchScroll::default();
        let (_, end) = drag(&mut s, 500.0, 200.0, 6, 1.0 / 60.0);
        let a = s.touch(&touch(1, TouchPhase::Ended, 50.0, 200.0), end, over_list);
        assert_eq!(a, TouchAction::End);
        assert!(!s.flinging(), "no fling");
        crate::anim::set_reduced_motion(None);
    }

    #[test]
    fn a_release_away_from_the_last_move_scrolls_there_then_flings() {
        let mut s = TouchScroll::default();
        let (_, end) = drag(&mut s, 500.0, 220.0, 6, 1.0 / 60.0);
        let a = s.touch(
            &touch(1, TouchPhase::Ended, 50.0, 200.0),
            end + 0.008,
            over_list,
        );
        assert_eq!(
            a,
            TouchAction::Scroll {
                target: LIST,
                dy: 20.0
            }
        );
        assert!(s.flinging());
    }

    #[test]
    fn a_release_with_its_moves_folded_in_still_scrolls() {
        let mut s = TouchScroll::default();
        s.touch(&touch(1, TouchPhase::Started, 50.0, 400.0), 0.0, over_list);
        let a = s.touch(&touch(1, TouchPhase::Ended, 52.0, 250.0), 0.3, over_list);
        assert_eq!(
            a,
            TouchAction::Begin {
                target: LIST,
                dy: 150.0
            }
        );
        assert!(!s.flinging() && !s.holds_moves());
        // Not outside a scroll container, nor sideways.
        s.touch(&touch(1, TouchPhase::Started, 50.0, 400.0), 1.0, nothing);
        let a = s.touch(&touch(1, TouchPhase::Ended, 50.0, 250.0), 1.3, nothing);
        assert_eq!(a, TouchAction::None);
        s.touch(&touch(1, TouchPhase::Started, 50.0, 400.0), 2.0, over_list);
        let a = s.touch(&touch(1, TouchPhase::Ended, 250.0, 350.0), 2.3, over_list);
        assert_eq!(a, TouchAction::None);
    }

    #[test]
    fn a_downward_fling_scrolls_back() {
        let mut s = TouchScroll::default();
        let (scrolled, end) = drag(&mut s, 200.0, 500.0, 6, 1.0 / 60.0);
        assert!(scrolled < 0.0);
        s.touch(&touch(1, TouchPhase::Ended, 50.0, 500.0), end, over_list);
        let (_, dy) = s.tick(end + 0.016).expect("flinging");
        assert!(dy < 0.0);
    }

    #[test]
    fn a_touch_stops_a_fling_without_clicking() {
        let mut s = TouchScroll::default();
        let (_, end) = drag(&mut s, 500.0, 200.0, 6, 1.0 / 60.0);
        s.touch(&touch(1, TouchPhase::Ended, 50.0, 200.0), end, over_list);
        assert!(s.flinging());
        s.touch(
            &touch(2, TouchPhase::Started, 60.0, 300.0),
            end + 0.1,
            over_list,
        );
        assert!(!s.flinging());
        assert!(s.swallows_press() && s.holds_moves());
        assert_eq!(s.tick(end + 0.2), None);
        // Lifting it ends the touch with nothing to click.
        let a = s.touch(
            &touch(2, TouchPhase::Ended, 60.0, 300.0),
            end + 0.2,
            over_list,
        );
        assert_eq!(a, TouchAction::None);
        assert!(!s.swallows_press());
        // ...but the touch could have scrolled again instead.
        s.touch(
            &touch(3, TouchPhase::Started, 60.0, 300.0),
            end + 0.3,
            over_list,
        );
        s.touch(
            &touch(3, TouchPhase::Ended, 60.0, 300.0),
            end + 0.4,
            over_list,
        );
        let mut s2 = s.clone();
        let (scrolled, _) = drag(&mut s2, 300.0, 250.0, 2, 0.02);
        assert_eq!(scrolled, 50.0 - SLOP);
    }

    #[test]
    fn sideways_or_outside_a_scroll_container_the_widget_keeps_the_finger() {
        let mut s = TouchScroll::default();
        s.touch(&touch(1, TouchPhase::Started, 50.0, 100.0), 0.0, over_list);
        let a = s.touch(&touch(1, TouchPhase::Moved, 90.0, 110.0), 0.02, over_list);
        assert_eq!(a, TouchAction::None);
        assert!(!s.dragging() && !s.holds_moves());
        // Even moving vertically afterwards.
        let a = s.touch(&touch(1, TouchPhase::Moved, 90.0, 300.0), 0.04, over_list);
        assert_eq!(a, TouchAction::None);
        s.touch(&touch(1, TouchPhase::Ended, 90.0, 300.0), 0.06, over_list);

        // No scroll container under the finger: moves pass straight through.
        s.touch(&touch(1, TouchPhase::Started, 50.0, 100.0), 1.0, nothing);
        assert!(!s.holds_moves());
        let a = s.touch(&touch(1, TouchPhase::Moved, 50.0, 300.0), 1.02, nothing);
        assert_eq!(a, TouchAction::None);
    }

    #[test]
    fn a_dragging_widget_keeps_the_finger() {
        let mut s = TouchScroll::default();
        s.touch(&touch(1, TouchPhase::Started, 50.0, 100.0), 0.0, over_list);
        s.yield_to_widget();
        assert!(!s.holds_moves());
        let a = s.touch(&touch(1, TouchPhase::Moved, 50.0, 300.0), 0.02, over_list);
        assert_eq!(a, TouchAction::None);
    }

    #[test]
    fn other_fingers_are_ignored() {
        let mut s = TouchScroll::default();
        s.touch(&touch(1, TouchPhase::Started, 50.0, 300.0), 0.0, over_list);
        s.touch(
            &touch(2, TouchPhase::Started, 150.0, 300.0),
            0.01,
            over_list,
        );
        let a = s.touch(&touch(2, TouchPhase::Moved, 150.0, 100.0), 0.02, over_list);
        assert_eq!(a, TouchAction::None);
        let a = s.touch(&touch(2, TouchPhase::Ended, 150.0, 100.0), 0.03, over_list);
        assert_eq!(a, TouchAction::None);
        // The first finger still scrolls.
        let a = s.touch(&touch(1, TouchPhase::Moved, 50.0, 250.0), 0.04, over_list);
        assert!(matches!(a, TouchAction::Begin { .. }));
    }

    #[test]
    fn a_cancelled_drag_ends_without_a_fling() {
        let mut s = TouchScroll::default();
        let (_, end) = drag(&mut s, 500.0, 200.0, 6, 1.0 / 60.0);
        let a = s.touch(
            &touch(1, TouchPhase::Cancelled, 50.0, 200.0),
            end,
            over_list,
        );
        assert_eq!(a, TouchAction::End);
        assert!(!s.flinging() && !s.dragging());
    }

    #[test]
    fn a_burst_of_events_with_no_time_between_them_doesnt_fling() {
        let mut s = TouchScroll::default();
        let (_, _) = drag(&mut s, 500.0, 200.0, 6, 0.0);
        s.touch(&touch(1, TouchPhase::Ended, 50.0, 200.0), 0.0, over_list);
        assert!(!s.flinging());
    }
}
