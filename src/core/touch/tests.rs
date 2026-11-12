use super::*;

const LIST: ScrollId = ScrollId(7);

fn touch(id: u64, phase: TouchPhase, x: f64, y: f64) -> Touch {
    Touch {
        id,
        phase,
        position: Point::new(x, y),
    }
}

fn over_list(_: Point) -> TouchTargets {
    TouchTargets {
        scroll: ScrollTargets {
            vertical: Some(LIST),
            horizontal: None,
        },
        ..TouchTargets::default()
    }
}

fn nothing(_: Point) -> TouchTargets {
    TouchTargets::default()
}

/// Drag finger 1 from y0 to y1 in `steps` moves, `dt` seconds apart,
/// starting at time 0; returns the total scrolled and the end time.
fn drag(s: &mut TouchGestures, y0: f64, y1: f64, steps: usize, dt: f64) -> (f64, f64) {
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
            TouchAction::Begin { target, delta } | TouchAction::Scroll { target, delta } => {
                assert_eq!(target, LIST);
                scrolled += delta;
            }
            TouchAction::None => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    (scrolled, steps as f64 * dt)
}

#[test]
fn a_tap_stays_a_press() {
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
    s.touch(&touch(1, TouchPhase::Started, 50.0, 300.0), 0.0, over_list);
    // Up 20 px: the press becomes a scroll of 12 (20 less the slop).
    let a = s.touch(&touch(1, TouchPhase::Moved, 52.0, 280.0), 0.016, over_list);
    assert_eq!(
        a,
        TouchAction::Begin {
            target: LIST,
            delta: 20.0 - SLOP
        }
    );
    assert!(s.dragging() && s.holds_moves());
    // From then on the content follows the finger exactly.
    let a = s.touch(&touch(1, TouchPhase::Moved, 52.0, 250.0), 0.032, over_list);
    assert_eq!(
        a,
        TouchAction::Scroll {
            target: LIST,
            delta: 30.0
        }
    );
    // Back down scrolls back.
    let a = s.touch(&touch(1, TouchPhase::Moved, 52.0, 260.0), 0.048, over_list);
    assert_eq!(
        a,
        TouchAction::Scroll {
            target: LIST,
            delta: -10.0
        }
    );
}

#[test]
fn a_slow_release_doesnt_fling() {
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
    let (_, end) = drag(&mut s, 500.0, 200.0, 6, 1.0 / 60.0);
    let a = s.touch(&touch(1, TouchPhase::Ended, 50.0, 200.0), end, over_list);
    assert_eq!(a, TouchAction::End);
    assert!(!s.flinging(), "no fling");
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_release_away_from_the_last_move_scrolls_there_then_flings() {
    let mut s = TouchGestures::default();
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
            delta: 20.0
        }
    );
    assert!(s.flinging());
}

#[test]
fn a_release_with_its_moves_folded_in_still_scrolls() {
    let mut s = TouchGestures::default();
    s.touch(&touch(1, TouchPhase::Started, 50.0, 400.0), 0.0, over_list);
    let a = s.touch(&touch(1, TouchPhase::Ended, 52.0, 250.0), 0.3, over_list);
    assert_eq!(
        a,
        TouchAction::Begin {
            target: LIST,
            delta: 150.0
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
    let mut s = TouchGestures::default();
    let (scrolled, end) = drag(&mut s, 200.0, 500.0, 6, 1.0 / 60.0);
    assert!(scrolled < 0.0);
    s.touch(&touch(1, TouchPhase::Ended, 50.0, 500.0), end, over_list);
    let (_, dy) = s.tick(end + 0.016).expect("flinging");
    assert!(dy < 0.0);
}

#[test]
fn a_touch_stops_a_fling_without_clicking() {
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
    s.touch(&touch(1, TouchPhase::Started, 50.0, 100.0), 0.0, over_list);
    s.yield_to_widget();
    assert!(!s.holds_moves());
    let a = s.touch(&touch(1, TouchPhase::Moved, 50.0, 300.0), 0.02, over_list);
    assert_eq!(a, TouchAction::None);
}

#[test]
fn other_fingers_are_ignored() {
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
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
    let mut s = TouchGestures::default();
    let (_, _) = drag(&mut s, 500.0, 200.0, 6, 0.0);
    s.touch(&touch(1, TouchPhase::Ended, 50.0, 200.0), 0.0, over_list);
    assert!(!s.flinging());
}

const STRIP: ScrollId = ScrollId(9);
const PHOTO: PointerId = PointerId(3);

/// A sideways strip inside the vertical list.
fn over_strip_in_list(_: Point) -> TouchTargets {
    TouchTargets {
        scroll: ScrollTargets {
            vertical: Some(LIST),
            horizontal: Some(STRIP),
        },
        ..TouchTargets::default()
    }
}

/// A row with a context menu, in the list.
fn over_row_with_menu(p: Point) -> TouchTargets {
    TouchTargets {
        long_press: true,
        ..over_list(p)
    }
}

/// A photo that takes pinches, in the list.
fn over_photo(p: Point) -> TouchTargets {
    TouchTargets {
        pinch: Some(PHOTO),
        ..over_list(p)
    }
}

#[test]
fn a_sideways_drag_scrolls_the_sideways_strip_and_flings_it() {
    let mut s = TouchGestures::default();
    let t = |phase, x, y| touch(1, phase, x, y);
    s.touch(
        &t(TouchPhase::Started, 300.0, 100.0),
        0.0,
        over_strip_in_list,
    );
    // Left 20 px (and a little down): the strip scrolls towards its end.
    let a = s.touch(
        &t(TouchPhase::Moved, 280.0, 103.0),
        0.016,
        over_strip_in_list,
    );
    assert_eq!(
        a,
        TouchAction::Begin {
            target: STRIP,
            delta: 20.0 - SLOP
        }
    );
    assert!(s.dragging() && s.holds_moves());
    // Vertical movement no longer counts; only x does.
    let a = s.touch(
        &t(TouchPhase::Moved, 250.0, 140.0),
        0.032,
        over_strip_in_list,
    );
    assert_eq!(
        a,
        TouchAction::Scroll {
            target: STRIP,
            delta: 30.0
        }
    );
    for (i, x) in [200.0, 150.0, 100.0].into_iter().enumerate() {
        let now = 0.048 + i as f64 * 0.016;
        s.touch(&t(TouchPhase::Moved, x, 140.0), now, over_strip_in_list);
    }
    let a = s.touch(
        &t(TouchPhase::Ended, 100.0, 140.0),
        0.096,
        over_strip_in_list,
    );
    assert_eq!(a, TouchAction::End);
    let (target, delta) = s.tick(0.112).expect("a sideways fling");
    assert_eq!(target, STRIP);
    assert!(delta > 0.0, "it goes on the way the finger went");
}

#[test]
fn a_vertical_drag_over_the_strip_scrolls_the_list() {
    let mut s = TouchGestures::default();
    let t = |phase, x, y| touch(1, phase, x, y);
    s.touch(
        &t(TouchPhase::Started, 300.0, 300.0),
        0.0,
        over_strip_in_list,
    );
    let a = s.touch(
        &t(TouchPhase::Moved, 305.0, 270.0),
        0.016,
        over_strip_in_list,
    );
    assert_eq!(
        a,
        TouchAction::Begin {
            target: LIST,
            delta: 30.0 - SLOP
        }
    );
}

#[test]
fn a_sideways_drag_with_no_sideways_container_goes_to_the_widget() {
    let mut s = TouchGestures::default();
    s.touch(&touch(1, TouchPhase::Started, 50.0, 100.0), 0.0, over_list);
    let a = s.touch(&touch(1, TouchPhase::Moved, 90.0, 102.0), 0.02, over_list);
    assert_eq!(a, TouchAction::None);
    assert!(!s.holds_moves());
}

#[test]
fn a_sideways_release_with_its_moves_folded_in_scrolls_the_strip() {
    let mut s = TouchGestures::default();
    let t = |phase, x, y| touch(1, phase, x, y);
    s.touch(
        &t(TouchPhase::Started, 300.0, 100.0),
        0.0,
        over_strip_in_list,
    );
    let a = s.touch(&t(TouchPhase::Ended, 100.0, 110.0), 0.3, over_strip_in_list);
    assert_eq!(
        a,
        TouchAction::Begin {
            target: STRIP,
            delta: 200.0
        }
    );
}

#[test]
fn a_finger_held_still_becomes_a_long_press() {
    let mut s = TouchGestures::default();
    let t = |id, phase, x, y| touch(id, phase, x, y);
    s.touch(
        &t(1, TouchPhase::Started, 50.0, 100.0),
        1.0,
        over_row_with_menu,
    );
    assert_eq!(s.long_press_deadline(), Some(1.0 + LONG_PRESS));
    // A jitter within the slop doesn't cancel it, and it is reported where
    // the finger is now.
    s.touch(
        &t(1, TouchPhase::Moved, 53.0, 104.0),
        1.2,
        over_row_with_menu,
    );
    assert_eq!(s.long_press_due(1.3), None, "not yet");
    assert_eq!(
        s.long_press_due(1.0 + LONG_PRESS),
        Some(Point::new(53.0, 104.0))
    );
    // Once only, and the finger is spent: its moves don't scroll, and its
    // release does nothing.
    assert_eq!(s.long_press_deadline(), None);
    assert_eq!(s.long_press_due(2.0), None);
    assert!(s.holds_moves());
    let a = s.touch(
        &t(1, TouchPhase::Moved, 53.0, 300.0),
        1.7,
        over_row_with_menu,
    );
    assert_eq!(a, TouchAction::None);
    let a = s.touch(
        &t(1, TouchPhase::Ended, 53.0, 300.0),
        1.8,
        over_row_with_menu,
    );
    assert_eq!(a, TouchAction::None);
    assert!(!s.holds_moves() && !s.flinging());
    // The next finger starts afresh.
    s.touch(
        &t(2, TouchPhase::Started, 50.0, 100.0),
        3.0,
        over_row_with_menu,
    );
    assert_eq!(s.long_press_deadline(), Some(3.0 + LONG_PRESS));
}

#[test]
fn moving_lifting_or_a_widget_cancels_a_long_press() {
    let t = |id, phase, x, y| touch(id, phase, x, y);
    // A scroll first.
    let mut s = TouchGestures::default();
    s.touch(
        &t(1, TouchPhase::Started, 50.0, 300.0),
        0.0,
        over_row_with_menu,
    );
    let a = s.touch(
        &t(1, TouchPhase::Moved, 50.0, 280.0),
        0.1,
        over_row_with_menu,
    );
    assert!(matches!(a, TouchAction::Begin { .. }));
    assert_eq!(s.long_press_due(1.0), None);
    // A tap: lifted before the delay.
    let mut s = TouchGestures::default();
    s.touch(
        &t(1, TouchPhase::Started, 50.0, 300.0),
        0.0,
        over_row_with_menu,
    );
    s.touch(
        &t(1, TouchPhase::Ended, 50.0, 300.0),
        0.1,
        over_row_with_menu,
    );
    assert_eq!(s.long_press_due(1.0), None);
    // A slider latched the finger.
    let mut s = TouchGestures::default();
    s.touch(
        &t(1, TouchPhase::Started, 50.0, 300.0),
        0.0,
        over_row_with_menu,
    );
    s.yield_to_widget();
    assert_eq!(s.long_press_due(1.0), None);
    // Nothing with a context menu under the finger.
    let mut s = TouchGestures::default();
    s.touch(&t(1, TouchPhase::Started, 50.0, 300.0), 0.0, over_list);
    assert_eq!(s.long_press_deadline(), None);
    assert_eq!(s.long_press_due(1.0), None);
    // A finger that caught a fling.
    let mut s = TouchGestures::default();
    let (_, end) = drag(&mut s, 500.0, 200.0, 6, 1.0 / 60.0);
    s.touch(&t(1, TouchPhase::Ended, 50.0, 200.0), end, over_list);
    assert!(s.flinging());
    let now = end + 0.1;
    s.touch(
        &t(2, TouchPhase::Started, 50.0, 300.0),
        now,
        over_row_with_menu,
    );
    assert_eq!(s.long_press_due(end + 2.0), None);
}

/// Put two fingers down on the photo, `gap` apart around (100, 200).
fn two_fingers(s: &mut TouchGestures, gap: f64) -> TouchAction {
    let left = touch(1, TouchPhase::Started, 100.0 - gap / 2.0, 200.0);
    s.touch(&left, 0.0, over_photo);
    let right = touch(2, TouchPhase::Started, 100.0 + gap / 2.0, 200.0);
    s.touch(&right, 0.05, over_photo)
}

fn pinch(a: TouchAction) -> Pinch {
    match a {
        TouchAction::Pinch(p) => p,
        other => panic!("not a pinch: {other:?}"),
    }
}

#[test]
fn two_fingers_spreading_apart_pinch_out() {
    let mut s = TouchGestures::default();
    let p = pinch(two_fingers(&mut s, 100.0));
    assert_eq!(p.target, PHOTO);
    assert_eq!(p.phase, PinchPhase::Began);
    assert_eq!((p.scale, p.rotation), (1.0, 0.0));
    assert_eq!(p.centroid, Point::new(100.0, 200.0));
    assert_eq!(p.translation, Vec2::ZERO);
    assert!(s.pinching() && s.holds_moves());
    assert_eq!(s.long_press_deadline(), None);
    // Spread to 200 apart: twice the size.
    s.touch(&touch(1, TouchPhase::Moved, 0.0, 200.0), 0.1, over_photo);
    let p = pinch(s.touch(&touch(2, TouchPhase::Moved, 200.0, 200.0), 0.1, over_photo));
    assert_eq!(p.phase, PinchPhase::Changed);
    assert!((p.scale - 2.0).abs() < 1e-9, "scale {}", p.scale);
    assert_eq!(p.centroid, Point::new(100.0, 200.0));
    // Lifting one finger ends it with the last values; the other is ignored
    // until it lifts.
    let p = pinch(s.touch(&touch(2, TouchPhase::Ended, 200.0, 200.0), 0.2, over_photo));
    assert_eq!(p.phase, PinchPhase::Ended);
    assert!((p.scale - 2.0).abs() < 1e-9);
    assert!(!s.pinching() && s.holds_moves());
    let a = s.touch(&touch(1, TouchPhase::Moved, 0.0, 400.0), 0.3, over_photo);
    assert_eq!(a, TouchAction::None, "the finger left over doesn't scroll");
    s.touch(&touch(1, TouchPhase::Ended, 0.0, 400.0), 0.4, over_photo);
    assert!(!s.holds_moves() && !s.flinging());
}

#[test]
fn two_fingers_moving_together_pan_and_turning_rotates() {
    let mut s = TouchGestures::default();
    two_fingers(&mut s, 100.0);
    // Both move 30 right and 40 down: a two-finger pan, same size.
    s.touch(&touch(1, TouchPhase::Moved, 80.0, 240.0), 0.1, over_photo);
    let p = pinch(s.touch(&touch(2, TouchPhase::Moved, 180.0, 240.0), 0.1, over_photo));
    assert_eq!(p.translation, Vec2::new(30.0, 40.0));
    assert!((p.scale - 1.0).abs() < 1e-9);
    assert!(p.rotation.abs() < 1e-9);
    // Finger 2 swings round below finger 1: a quarter turn clockwise on
    // screen (y grows downwards), at the same distance.
    let p = pinch(s.touch(&touch(2, TouchPhase::Moved, 80.0, 340.0), 0.2, over_photo));
    let quarter = std::f64::consts::FRAC_PI_2;
    assert!((p.rotation - quarter).abs() < 1e-9, "{}", p.rotation);
    assert!((p.scale - 1.0).abs() < 1e-9);
    // A cancel ends it as cancelled.
    let a = s.touch(
        &touch(1, TouchPhase::Cancelled, 80.0, 240.0),
        0.3,
        over_photo,
    );
    assert_eq!(pinch(a).phase, PinchPhase::Cancelled);
    s.touch(
        &touch(2, TouchPhase::Cancelled, 80.0, 340.0),
        0.3,
        over_photo,
    );
    assert!(!s.holds_moves());
}

#[test]
fn a_second_finger_turns_a_scroll_into_a_pinch() {
    let mut s = TouchGestures::default();
    s.touch(
        &touch(1, TouchPhase::Started, 100.0, 300.0),
        0.0,
        over_photo,
    );
    let a = s.touch(&touch(1, TouchPhase::Moved, 100.0, 250.0), 0.05, over_photo);
    assert!(matches!(a, TouchAction::Begin { .. }));
    let p = pinch(s.touch(
        &touch(2, TouchPhase::Started, 200.0, 250.0),
        0.1,
        over_photo,
    ));
    // Measured from where the first finger is now.
    assert_eq!(p.centroid, Point::new(150.0, 250.0));
    assert!(s.pinching());
    // Lifting doesn't fling the list.
    s.touch(&touch(1, TouchPhase::Ended, 100.0, 250.0), 0.2, over_photo);
    s.touch(&touch(2, TouchPhase::Ended, 200.0, 250.0), 0.2, over_photo);
    assert!(!s.flinging());
}

#[test]
fn without_a_pinch_target_a_second_finger_is_ignored() {
    let mut s = TouchGestures::default();
    s.touch(&touch(1, TouchPhase::Started, 100.0, 300.0), 0.0, over_list);
    let a = s.touch(
        &touch(2, TouchPhase::Started, 200.0, 300.0),
        0.05,
        over_list,
    );
    assert_eq!(a, TouchAction::None);
    let a = s.touch(&touch(2, TouchPhase::Moved, 200.0, 100.0), 0.06, over_list);
    assert_eq!(a, TouchAction::None);
    // The first still scrolls.
    let a = s.touch(&touch(1, TouchPhase::Moved, 100.0, 250.0), 0.1, over_list);
    assert!(matches!(a, TouchAction::Begin { .. }));
}

#[test]
fn a_third_finger_doesnt_disturb_a_pinch() {
    let mut s = TouchGestures::default();
    two_fingers(&mut s, 100.0);
    let a = s.touch(
        &touch(3, TouchPhase::Started, 300.0, 300.0),
        0.1,
        over_photo,
    );
    assert_eq!(a, TouchAction::None);
    let a = s.touch(&touch(3, TouchPhase::Moved, 0.0, 0.0), 0.15, over_photo);
    assert_eq!(a, TouchAction::None);
    let a = s.touch(&touch(3, TouchPhase::Ended, 0.0, 0.0), 0.2, over_photo);
    assert_eq!(a, TouchAction::None);
    assert!(s.pinching());
}

#[test]
fn a_finger_on_a_widget_doesnt_pinch() {
    // A slider has the first finger: a second one is ignored.
    let mut s = TouchGestures::default();
    s.touch(
        &touch(1, TouchPhase::Started, 100.0, 300.0),
        0.0,
        over_photo,
    );
    s.yield_to_widget();
    let a = s.touch(
        &touch(2, TouchPhase::Started, 200.0, 300.0),
        0.05,
        over_photo,
    );
    assert_eq!(a, TouchAction::None);
    assert!(!s.pinching());
}
