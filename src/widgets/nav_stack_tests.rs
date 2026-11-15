//! The page stack at fixed frame times: pushes and pops in each transition,
//! the back gesture's progress followed, cancelled and committed, reduced
//! motion, and in an App: Back taken while there's a page to go back to, and
//! only the top page seen.

use std::time::Duration;

use super::nav_stack::*;
use crate::App;
use crate::anim::{Easing, Motion};
use crate::core::{AccessNode, Cx, Element, ElementKind, Handled};
use crate::geometry::Size;
use crate::platform::{BackEdge, BackGesture};
use crate::style::{MotionTokens, Theme};

const MS: Duration = Duration::from_millis(1);
const T0: Duration = Duration::from_secs(10);
const WIDTH: f64 = 400.0;

struct State {
    nav: NavStack<u32>,
    backs: u32,
}

/// The light theme with pages moving on a 100 ms linear tween, so values in
/// between are exact.
fn theme() -> Theme {
    Theme::light().motion(MotionTokens {
        page: Motion::tween(0.1, Easing::Linear),
        ..MotionTokens::standard()
    })
}

/// One page as built: whether it's off stage, its sideways shift, scale and
/// opacity.
#[derive(Debug, PartialEq)]
struct Page {
    offstage: bool,
    dx: f64,
    scale: f64,
    opacity: f32,
}

/// Build `nav` at `now`: the pages, and how many back handlers it declared.
fn build(nav: &NavStack<u32>, now: Duration) -> (Vec<Page>, usize) {
    let theme = theme();
    let mut cx = Cx::<State>::new(&theme);
    cx.set_frame_time(now);
    cx.set_window_size(Size::new(WIDTH, 600.0));
    let el = nav_stack(
        &mut cx,
        nav,
        |s| &mut s.nav,
        |cx, p| super::label(cx, format!("Page {p}")),
    );
    let backs = cx.take_back_handlers().len();
    (pages(&el), backs)
}

fn pages(el: &Element) -> Vec<Page> {
    let ElementKind::Layers { children, .. } = &el.kind else {
        panic!("a nav stack is layers of pages");
    };
    children
        .iter()
        .map(|c| {
            let t = c.transform.unwrap_or_default();
            Page {
                offstage: c.offstage,
                dx: (t.translate.dx * 1000.0).round() / 1000.0,
                scale: (t.scale.dx * 1000.0).round() / 1000.0,
                opacity: c.opacity,
            }
        })
        .collect()
}

fn at(dx: f64) -> Page {
    Page {
        offstage: false,
        dx,
        scale: 1.0,
        opacity: 1.0,
    }
}

fn hidden() -> Page {
    Page {
        offstage: true,
        ..at(0.0)
    }
}

/// A stack showing page 1 over the root 0, at rest, its motion learned from
/// the theme by a first build.
fn stack(transition: PageTransition) -> NavStack<u32> {
    crate::anim::set_reduced_motion(Some(false));
    let mut nav = NavStack::new(0).transition(transition);
    build(&nav, T0);
    nav.push_at(1, T0);
    build(&nav, T0 + 100 * MS);
    nav
}

#[test]
fn a_push_slides_the_page_in_over_the_one_below() {
    crate::anim::set_reduced_motion(Some(false));
    let mut nav = NavStack::new(0).transition(PageTransition::Slide);
    let (shown, backs) = build(&nav, T0);
    assert_eq!(shown, [at(0.0)]);
    assert_eq!(backs, 0, "nothing to go back to");
    nav.push_at(1, T0);
    assert_eq!(build(&nav, T0).0, [at(0.0), at(WIDTH)]);
    // Halfway: the new page half in, the old one shifted a sixth over.
    let (mid, backs) = build(&nav, T0 + 50 * MS);
    assert_eq!(mid, [at(-0.3 * WIDTH * 0.5), at(WIDTH / 2.0)]);
    assert_eq!(backs, 1, "the stack takes Back");
    // At rest the page under it is kept, off stage.
    assert_eq!(build(&nav, T0 + 101 * MS).0, [hidden(), at(0.0)]);
    assert_eq!(nav.depth(), 2);
}

#[test]
fn a_pop_slides_the_page_away_and_then_drops_it() {
    let mut nav = stack(PageTransition::Slide);
    let t1 = T0 + 200 * MS;
    assert!(nav.pop_at(t1));
    assert_eq!(nav.depth(), 1);
    // The popped page is still drawn, last, as it leaves.
    let (mid, backs) = build(&nav, t1 + 25 * MS);
    assert_eq!(mid, [at(-0.3 * WIDTH * 0.75), at(WIDTH * 0.25)]);
    assert_eq!(backs, 0, "nothing left to go back to");
    assert_eq!(build(&nav, t1 + 101 * MS).0, [at(0.0)]);
    // The root stays.
    assert!(!nav.pop_at(t1 + 200 * MS));
    assert_eq!(nav.depth(), 1);
}

#[test]
fn a_pop_mid_push_turns_round_from_where_it_is() {
    crate::anim::set_reduced_motion(Some(false));
    let mut nav = NavStack::new(0).transition(PageTransition::Slide);
    build(&nav, T0);
    nav.push_at(1, T0);
    nav.pop_at(T0 + 40 * MS);
    // It was 40% in; it doesn't jump.
    let (pages, _) = build(&nav, T0 + 40 * MS);
    assert_eq!(pages[1].dx, WIDTH * 0.6);
    assert!(build(&nav, T0 + 120 * MS).0[1].dx > WIDTH * 0.6);
    assert_eq!(build(&nav, T0 + 200 * MS).0, [at(0.0)]);
}

#[test]
fn the_back_gesture_drags_the_page_and_a_cancel_puts_it_back() {
    let mut nav = stack(PageTransition::Slide);
    let t1 = T0 + 200 * MS;
    nav.back_progress_at(
        BackGesture::Started {
            edge: BackEdge::Left,
        },
        PageTransition::Slide,
        t1,
    );
    nav.back_progress_at(
        BackGesture::Progressed { progress: 0.25 },
        PageTransition::Slide,
        t1 + 16 * MS,
    );
    // Both pages on stage, the top one a quarter of the way off.
    assert_eq!(
        build(&nav, t1 + 16 * MS).0,
        [at(-0.3 * WIDTH * 0.75), at(WIDTH * 0.25)]
    );
    nav.back_progress_at(BackGesture::Cancelled, PageTransition::Slide, t1 + 32 * MS);
    // It slides back over the page's motion, then rests as before.
    let mid = build(&nav, t1 + 82 * MS).0;
    assert!(mid[1].dx > 0.0 && mid[1].dx < WIDTH * 0.25, "{mid:?}");
    assert_eq!(build(&nav, t1 + 200 * MS).0, [hidden(), at(0.0)]);
    assert_eq!(nav.depth(), 2);
}

#[test]
fn a_committed_gesture_pops_from_where_the_finger_left_it() {
    let mut nav = stack(PageTransition::Slide);
    let t1 = T0 + 200 * MS;
    nav.back_progress_at(
        BackGesture::Started {
            edge: BackEdge::Left,
        },
        PageTransition::Slide,
        t1,
    );
    nav.back_progress_at(
        BackGesture::Progressed { progress: 0.6 },
        PageTransition::Slide,
        t1 + 16 * MS,
    );
    // The finger lifts: Back.
    assert!(nav.pop_at(t1 + 16 * MS));
    assert_eq!(nav.depth(), 1);
    let (pages, _) = build(&nav, t1 + 16 * MS);
    assert_eq!(pages[1].dx, WIDTH * 0.6, "no jump");
    let later = build(&nav, t1 + 40 * MS).0;
    assert!(later[1].dx > WIDTH * 0.6, "{later:?}");
    assert_eq!(build(&nav, t1 + 300 * MS).0, [at(0.0)]);
}

#[test]
fn materials_predictive_back_shrinks_the_page_into_a_card() {
    let mut nav = stack(PageTransition::SharedAxis);
    let t1 = T0 + 200 * MS;
    nav.back_progress_at(
        BackGesture::Started {
            edge: BackEdge::Left,
        },
        PageTransition::SharedAxis,
        t1,
    );
    nav.back_progress_at(
        BackGesture::Progressed { progress: 1.0 },
        PageTransition::SharedAxis,
        t1 + 16 * MS,
    );
    let pages = build(&nav, t1 + 16 * MS).0;
    // The page below shows in full; the top one is a 90% card shifted
    // toward the edge, still opaque.
    assert_eq!(pages[0], at(0.0));
    assert_eq!(pages[1].scale, 0.9);
    assert_eq!(pages[1].dx, WIDTH / 20.0 - 8.0);
    assert_eq!(pages[1].opacity, 1.0);
    // Committed, it fades away.
    nav.pop_at(t1 + 16 * MS);
    let gone = build(&nav, t1 + 66 * MS).0;
    assert!(gone[1].opacity < 1.0 && gone[1].scale == 0.9, "{gone:?}");
}

#[test]
fn shared_axis_and_fade_cross_fade() {
    crate::anim::set_reduced_motion(Some(false));
    let mut nav = NavStack::new(0).transition(PageTransition::SharedAxis);
    build(&nav, T0);
    nav.push_at(1, T0);
    let mid = build(&nav, T0 + 50 * MS).0;
    assert_eq!((mid[0].dx, mid[1].dx), (-15.0, 15.0));
    assert!(mid[0].opacity == 0.0 && mid[1].opacity > 0.0 && mid[1].opacity < 1.0);

    let mut nav = NavStack::new(0).transition(PageTransition::Fade);
    build(&nav, T0);
    nav.push_at(1, T0);
    let mid = build(&nav, T0 + 50 * MS).0;
    assert_eq!(mid[0], at(0.0));
    assert_eq!((mid[1].dx, mid[1].opacity), (8.0, 0.5));
}

#[test]
fn reduced_motion_switches_at_once() {
    crate::anim::set_reduced_motion(Some(true));
    let mut nav = NavStack::new(0).transition(PageTransition::Slide);
    build(&nav, T0);
    nav.push_at(1, T0);
    assert_eq!(build(&nav, T0).0, [hidden(), at(0.0)]);
    nav.pop_at(T0 + MS);
    assert_eq!(build(&nav, T0 + MS).0, [at(0.0)]);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn the_transition_follows_the_theme() {
    assert_eq!(PageTransition::of(&Theme::light()), PageTransition::Fade);
    assert_eq!(
        PageTransition::of(&Theme::material3_light()),
        PageTransition::SharedAxis
    );
}

#[test]
fn rtl_slides_from_the_left() {
    crate::anim::set_reduced_motion(Some(false));
    let theme = theme().layout_direction(crate::layout::LayoutDirection::Rtl);
    let mut nav = NavStack::new(0).transition(PageTransition::Slide);
    nav.push_at(1, T0);
    let mut cx = Cx::<State>::new(&theme);
    cx.set_frame_time(T0 + 50 * MS);
    cx.set_window_size(Size::new(WIDTH, 600.0));
    let el = nav_stack(
        &mut cx,
        &nav,
        |s| &mut s.nav,
        |cx, p| super::label(cx, format!("Page {p}")),
    );
    // The standard motion (the stack hadn't been shown): just its side.
    assert!(pages(&el)[1].dx < 0.0);
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    nav_stack(
        cx,
        &s.nav,
        |s| &mut s.nav,
        |cx, p| {
            let bar = app_bar(cx, format!("Title {p}"));
            let bar = if *p > 0 {
                bar.on_back(|s| {
                    s.nav.pop();
                })
            } else {
                bar
            };
            super::column(vec![bar.view(cx), super::label(cx, format!("Body {p}"))])
        },
    )
}

fn app() -> App<State> {
    crate::anim::set_reduced_motion(Some(true));
    let mut app = App::new(
        State {
            nav: NavStack::new(0),
            backs: 0,
        },
        view,
    )
    .on_back(|s| {
        s.backs += 1;
        Handled::No
    })
    .logical_size(Size::new(360.0, 640.0));
    app.render_once();
    app
}

fn names(node: &AccessNode, out: &mut Vec<String>) {
    if !node.name.is_empty() {
        out.push(node.name.clone());
    }
    for c in &node.children {
        names(c, out);
    }
}

fn find(node: &AccessNode, name: &str) -> Option<crate::geometry::Point> {
    if node.name == name {
        return Some(node.bounds.center());
    }
    node.children.iter().find_map(|c| find(c, name))
}

fn read(app: &App<State>) -> Vec<String> {
    let mut out = Vec::new();
    names(&app.accessibility_tree().unwrap(), &mut out);
    out
}

#[test]
fn the_stack_takes_back_while_it_can_pop() {
    let mut app = app();
    // The root: the app's own hook has Back.
    assert!(app.wants_back(), "the app has an on_back hook");
    assert!(!app.request_back());
    assert_eq!(app.state().backs, 1);
    app.handle().update(|s: &mut State| s.nav.push(1));
    app.run_pending();
    app.render_once();
    assert!(app.wants_back());
    // Back pops, before the app's hook.
    assert!(app.request_back());
    assert_eq!((app.state().nav.depth(), app.state().backs), (1, 1));
    app.render_once();
    assert!(!app.request_back());
    assert_eq!(app.state().backs, 2);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn the_gesture_reaches_the_stack() {
    let mut app = app();
    app.handle().update(|s: &mut State| s.nav.push(1));
    app.run_pending();
    app.render_once();
    assert!(app.back_progress(
        0,
        BackGesture::Started {
            edge: BackEdge::Left
        }
    ));
    assert!(app.back_progress(0, BackGesture::Progressed { progress: 0.5 }));
    app.render_once();
    // Mid-gesture both pages show: the page below is read again.
    let mid = read(&app);
    assert!(mid.iter().any(|n| n == "Body 0") && mid.iter().any(|n| n == "Body 1"));
    assert!(app.back_progress(0, BackGesture::Cancelled));
    app.render_once();
    assert_eq!(app.state().nav.depth(), 2);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn only_the_top_page_is_read_and_its_back_button_pops() {
    let mut app = app();
    app.handle().update(|s: &mut State| s.nav.push(1));
    app.run_pending();
    app.render_once();
    let shown = read(&app);
    assert!(shown.iter().any(|n| n == "Title 1"), "{shown:?}");
    assert!(!shown.iter().any(|n| n == "Body 0"), "{shown:?}");
    let back = find(&app.accessibility_tree().unwrap(), "Back").expect("a back button");
    assert!(app.click_at(back));
    assert_eq!(app.state().nav.depth(), 1);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn the_app_bar_has_the_platforms_proportions() {
    let theme = Theme::light();
    let mut cx = Cx::<State>::new(&theme);
    let room = Size::new(360.0, 800.0);
    let bar = app_bar(&cx, "Scan").look(super::NavLook::Ios).view(&mut cx);
    assert_eq!(crate::core::measure(&bar, room, None).height, 44.5);
    let bar = app_bar(&cx, "Scan")
        .top_inset(24.0)
        .look(super::NavLook::Material)
        .view(&mut cx);
    assert_eq!(crate::core::measure(&bar, room, None).height, 88.0);
}
