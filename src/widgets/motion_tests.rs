//! The built-in widgets' default motion, checked at fixed frame times: the
//! tree each build would draw, through the same lowering pass the app runs.

use std::collections::HashMap;
use std::time::Duration;

use crate::anim::{Easing, Motion};
use crate::core::interact::Resolver;
use crate::core::presence::{Presence, PresenceSpec};
use crate::core::transition::Transitions;
use crate::core::{Cx, Element, ElementKey, Interaction, LayoutNode, OverlaySpec};
use crate::geometry::Rect;
use crate::style::{MotionTokens, Theme};

use super::*;

const MS: Duration = Duration::from_millis(1);

fn frame() -> Duration {
    crate::platform::frame::interval()
}

/// A tween of `ms` milliseconds, linear, so values in between are exact.
fn linear(ms: u64) -> Motion {
    Motion::tween(ms as f64 / 1000.0, Easing::Linear)
}

/// The light theme with every motion a 100 ms linear tween.
fn theme() -> Theme {
    let m = linear(100);
    Theme::light().with_motion(MotionTokens {
        state: m,
        press: m,
        toggle: m,
        indicator: m,
        popup: m,
        popup_exit: m,
        dialog: m,
        dialog_exit: m,
        toast_enter: m,
        toast_exit: m,
        disclosure: m,
        ..MotionTokens::standard()
    })
}

/// Builds a view the way an app does, at chosen frame times.
struct Rig {
    transitions: Transitions,
    states: HashMap<ElementKey, Interaction>,
    /// The enter and exit animations, and the last frame drawn (for
    /// [`draw`](Self::draw)).
    presence: Presence,
    specs: HashMap<ElementKey, PresenceSpec>,
    tree: Option<LayoutNode>,
}

impl Rig {
    fn new() -> Self {
        crate::anim::set_reduced_motion(Some(false));
        Self {
            transitions: Transitions::default(),
            states: HashMap::new(),
            presence: Presence::default(),
            specs: HashMap::new(),
            tree: None,
        }
    }

    /// [`build`](Self::build), then lay the view out with its overlays as
    /// the app does (each overlay at (10, 10), behind a scrim if modal) and run the enter and exit
    /// animations: the frame drawn at `now`, and whether anything leaving or
    /// entering still moves.
    fn draw<S>(
        &mut self,
        now: Duration,
        view: impl FnOnce(&mut Cx<S>) -> Element,
    ) -> (LayoutNode, bool) {
        let (el, overlays, _) = self.build(now, view);
        let mut specs = std::mem::take(&mut self.specs);
        let win = Rect::from_xywh(0.0, 0.0, 400.0, 300.0);
        let mut roots = vec![crate::core::layout(&el, win, None)];
        for o in &overlays {
            if o.modal {
                // The scrim, as the app lays it out (an empty box here).
                let mut scrim = LayoutNode::container(win, vec![]);
                crate::core::presence::scrim(&o.content, &mut scrim, &mut specs);
                roots.push(scrim);
            }
            let m = crate::core::measure(&o.content, win.size, None);
            let at = Rect::from_xywh(10.0, 10.0, m.width, m.height);
            roots.push(crate::core::layout(&o.content, at, None));
        }
        let mut tree = LayoutNode::container(win, roots);
        tree.key = Some(ElementKey::LAYERS);
        let old = self.tree.take();
        let offsets = HashMap::new();
        self.presence.update(
            &mut tree,
            &specs,
            old.as_ref(),
            &offsets,
            Some(now),
            win.size,
        );
        self.tree = Some(tree.clone());
        (tree, self.presence.animating())
    }

    /// Build `view` at `now` and lower it: what that frame draws (the tree
    /// and its overlays), and whether it asked for another frame.
    fn build<S>(
        &mut self,
        now: Duration,
        view: impl FnOnce(&mut Cx<S>) -> Element,
    ) -> (Element, Vec<OverlaySpec>, bool) {
        let theme = theme();
        let mut cx = Cx::<S>::new(&theme);
        cx.set_frame_time(now);
        let mut el = view(&mut cx);
        let mut overlays = cx.take_overlays();
        let requested = cx.frame_requested();
        self.transitions.begin(Some(now));
        let mut r = Resolver {
            states: &self.states,
            transitions: &mut self.transitions,
            watch: Default::default(),
            styled_actions: Vec::new(),
            styled_drops: Vec::new(),
            presence: HashMap::new(),
            anchors: self.presence.anchors(),
        };
        r.resolve(&mut el, ElementKey::root(0));
        let keys = ElementKey::layers(&overlays);
        for (o, key) in overlays.iter_mut().zip(keys) {
            r.resolve(&mut o.content, key);
        }
        self.specs = std::mem::take(&mut r.presence);
        self.transitions.end();
        let moving = requested || self.transitions.animating();
        (el, overlays, moving)
    }
}

fn kids(el: &mut Element) -> &mut Vec<Element> {
    el.children_mut().expect("a container")
}

/// Whether anything in `el` is drawn moved, scaled or faded (other than
/// fully hidden parts).
fn displaced(el: &mut Element) -> bool {
    let here = el.opacity > 0.0 && (el.transform.is_some() || el.opacity < 1.0);
    here || el
        .children_mut()
        .is_some_and(|c| c.iter_mut().any(displaced))
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_switch_slides_its_thumb_and_fades_its_track() {
    let mut rig = Rig::new();
    let switch_at = |on: bool| move |cx: &mut Cx<()>| switch(cx, &theme(), on, |_: &mut ()| {});
    let t0 = Duration::from_secs(10);
    let (mut el, _, moving) = rig.build(t0, switch_at(false));
    assert!(!moving, "at rest: no frames");
    assert_eq!(kids(&mut el)[0].layout.size.width, Some(0.0));
    // Turned on: the lead grows from 0 to 18 over 100 ms (counted from a
    // frame before the build that saw it), the track fades to the accent.
    let head = t0 + 500 * MS - frame();
    rig.build(head + frame(), switch_at(true)); // the build that sees it
    let (mut el, _, moving) = rig.build(head + 50 * MS, switch_at(true));
    assert!(moving);
    let w = kids(&mut el)[0].layout.size.width.unwrap();
    assert!(near(w, 9.0), "halfway across: {w}");
    let (from, to) = (theme().palette.border, theme().palette.primary);
    let fill = el.decoration.fill.unwrap();
    let between =
        |a: u8, b: u8, c: u8| (i32::from(c) - (i32::from(a) + i32::from(b)) / 2).abs() <= 1;
    assert!(
        between(from.r, to.r, fill.r) && between(from.b, to.b, fill.b),
        "{fill:?}"
    );
    let (mut el, _, moving) = rig.build(head + 101 * MS, switch_at(true));
    assert!(!moving, "there: no more frames");
    assert_eq!(kids(&mut el)[0].layout.size.width, Some(18.0));
    assert_eq!(el.decoration.fill, Some(to));
}

#[test]
fn a_check_mark_pops_in() {
    let mut rig = Rig::new();
    let at = |on: bool| move |cx: &mut Cx<()>| checkbox(cx, &theme(), on, |_: &mut ()| {});
    let t0 = Duration::from_secs(10);
    let (mut el, _, _) = rig.build(t0, at(false));
    assert_eq!(kids(&mut el)[0].opacity, 0.0, "hidden while unchecked");
    let head = t0 + 500 * MS - frame();
    rig.build(head + frame(), at(true));
    let (mut el, _, moving) = rig.build(head + 25 * MS, at(true));
    assert!(moving);
    let mark = &kids(&mut el)[0];
    assert!(near(f64::from(mark.opacity), 0.25), "{}", mark.opacity);
    let scale = mark.transform.expect("growing").scale.dx;
    assert!(near(scale, 0.4 + 0.6 * 0.25), "{scale}");
    let (mut el, _, moving) = rig.build(head + 101 * MS, at(true));
    assert!(!moving);
    let mark = &kids(&mut el)[0];
    assert_eq!((mark.opacity, mark.transform), (1.0, None));
}

#[test]
fn a_button_fades_its_hover_and_shrinks_while_pressed() {
    let mut rig = Rig::new();
    let t = theme();
    let base = t.palette.primary;
    let view = |cx: &mut Cx<()>| button_labeled(&theme(), "OK").on_tap(cx, |_: &mut ()| {});
    let t0 = Duration::from_secs(10);
    let (el, _, moving) = rig.build(t0, view);
    assert!(!moving);
    assert_eq!(el.decoration.fill, Some(base));
    // Hovered and pressed at once, halfway through.
    rig.states.insert(
        ElementKey::root(0),
        Interaction::HOVERED | Interaction::PRESSED,
    );
    let head = t0 + 500 * MS - frame();
    rig.build(head + frame(), view);
    let (el, _, moving) = rig.build(head + 50 * MS, view);
    assert!(moving);
    let scale = el.transform.expect("shrinking").scale.dx;
    assert!(near(scale, 1.0 - 0.03 * 0.5), "{scale}");
    let pressed = super::motion::lit(Some(base), t.palette.hover_overlay, 2);
    let fill = el.decoration.fill.unwrap();
    assert!(fill != base && fill != pressed, "between: {fill:?}");
    let (el, _, moving) = rig.build(head + 101 * MS, view);
    assert!(!moving);
    assert_eq!(el.decoration.fill, Some(pressed));
    assert!(near(el.transform.unwrap().scale.dx, 0.97));
    // Released and gone: back to rest, with nothing left over.
    rig.states.clear();
    rig.build(head + 1000 * MS, view);
    let (el, _, moving) = rig.build(head + 1100 * MS, view);
    assert!(!moving);
    assert_eq!(el.decoration.fill, Some(base));
    assert_eq!(el.transform, None);
}

/// What the stateful widgets' tests keep.
#[derive(Default)]
struct St {
    pick: Select,
    toasts: Toasts,
}

fn select_view(s: &St) -> impl FnOnce(&mut Cx<St>) -> Element + '_ {
    move |cx| {
        select(
            cx,
            &theme(),
            &["One", "Two", "Three"],
            &s.pick,
            |s: &mut St| &mut s.pick,
        )
    }
}

#[test]
fn a_select_list_grows_in_when_it_opens() {
    let mut rig = Rig::new();
    let mut st = St {
        pick: Select::new(Some(0)),
        ..St::default()
    };
    let t0 = Duration::from_secs(10);
    let (_, overlays, moving) = rig.build(t0, select_view(&st));
    assert!(overlays.is_empty() && !moving);
    st.pick.open();
    let head = t0 + 500 * MS - frame();
    rig.build(head + frame(), select_view(&st));
    let (_, overlays, moving) = rig.build(head + 50 * MS, select_view(&st));
    assert!(moving, "opening: frames");
    let list = &overlays[0].content;
    assert!(near(f64::from(list.opacity), 0.5), "{}", list.opacity);
    let tr = list.transform.expect("growing");
    let m = theme().motion;
    assert!(near(
        tr.scale.dx,
        m.popup_scale + (1.0 - m.popup_scale) * 0.5
    ));
    assert!(near(tr.translate.dy, -m.popup_offset * 0.5), "slides down");
    let (_, overlays, moving) = rig.build(head + 101 * MS, select_view(&st));
    assert!(!moving, "open: at rest");
    let list = &overlays[0].content;
    assert_eq!((list.opacity, list.transform), (1.0, None));
    // Closed and opened again: it grows in again.
    st.pick.close();
    rig.build(head + 200 * MS, select_view(&st));
    st.pick.open();
    let (_, overlays, _) = rig.build(head + 300 * MS, select_view(&st));
    assert!(overlays[0].content.opacity < 1.0);
}

#[test]
fn a_list_open_from_the_start_is_at_rest() {
    let mut rig = Rig::new();
    let mut st = St::default();
    st.pick.open();
    let (_, overlays, moving) = rig.build(Duration::from_secs(10), select_view(&st));
    assert!(!moving);
    assert_eq!(overlays[0].content.opacity, 1.0);
    assert_eq!(overlays[0].content.transform, None);
}

fn toasts_view(s: &St) -> impl FnOnce(&mut Cx<St>) -> Element + '_ {
    move |cx| {
        toasts(cx, &theme(), &s.toasts, |s: &mut St| &mut s.toasts);
        Element::stack(crate::layout::Axis::Vertical, vec![])
    }
}

#[test]
fn toasts_slide_in_and_fade_out() {
    let mut rig = Rig::new();
    let mut st = St::default();
    st.toasts.push("there from the start", ToastKind::Info);
    let t0 = Duration::from_secs(10);
    let (_, mut overlays, moving) = rig.build(t0, toasts_view(&st));
    assert!(!moving, "the first build's toasts are at rest");
    assert!(!displaced(&mut overlays[0].content));
    // A new one slides in from the right edge.
    let id = st.toasts.push("new", ToastKind::Success);
    let head = t0 + 500 * MS - frame();
    rig.build(head + frame(), toasts_view(&st));
    let (_, mut overlays, moving) = rig.build(head + 50 * MS, toasts_view(&st));
    assert!(moving);
    let cards = kids(&mut overlays[0].content);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].transform, None, "the old one stays put");
    let dx = cards[1].transform.expect("sliding").translate.dx;
    assert!(near(dx, theme().motion.toast_offset * 0.5), "{dx}");
    let (_, mut overlays, moving) = rig.build(head + 101 * MS, toasts_view(&st));
    assert!(!moving && !displaced(&mut overlays[0].content));
    // Dismissed: it stays a while, fading and sliding back out.
    st.toasts.dismiss(id);
    let head = head + 1000 * MS - frame();
    let (_, mut overlays, moving) = rig.build(head + frame(), toasts_view(&st));
    assert!(moving);
    let (_, mut later, _) = rig.build(head + 50 * MS, toasts_view(&st));
    let cards = kids(&mut later[0].content);
    assert_eq!(cards.len(), 2, "still drawn while it leaves");
    assert!(
        near(f64::from(cards[1].opacity), 0.5),
        "{}",
        cards[1].opacity
    );
    assert!(cards[1].semantics.is_none(), "no longer an alert");
    assert!(kids(&mut overlays[0].content)[1].opacity < 1.0);
    let (_, mut overlays, moving) = rig.build(head + 101 * MS, toasts_view(&st));
    assert!(!moving);
    assert_eq!(kids(&mut overlays[0].content).len(), 1, "gone");
    // The last one dismissed: the overlay stays until it has left.
    st.toasts.clear();
    let (_, overlays, _) = rig.build(head + 200 * MS, toasts_view(&st));
    assert_eq!(overlays.len(), 1);
    let (_, overlays, _) = rig.build(head + 400 * MS, toasts_view(&st));
    assert!(overlays.is_empty());
}

#[test]
fn disclosure_triangles_turn() {
    let m = linear(100);
    let mut turns = Vec::new();
    let t0 = Duration::from_secs(10);
    assert_eq!(
        super::tree::turn_of(&mut turns, 7, false, m, t0),
        (0.0, false),
        "a new row is at rest"
    );
    let head = t0 + 500 * MS - frame();
    let (_, moving) = super::tree::turn_of(&mut turns, 7, true, m, head + frame());
    assert!(moving);
    let (at, _) = super::tree::turn_of(&mut turns, 7, true, m, head + 40 * MS);
    assert!(near(at, 0.4), "{at}");
    assert_eq!(
        super::tree::turn_of(&mut turns, 7, true, m, head + 101 * MS),
        (1.0, false)
    );
}

#[test]
fn nothing_moves_at_rest() {
    let mut rig = Rig::new();
    let mut st = St {
        pick: Select::new(Some(1)),
        ..St::default()
    };
    st.pick.open();
    st.toasts.push("hello", ToastKind::Info);
    let t0 = Duration::from_secs(10);
    for i in 0..3 {
        let view = |cx: &mut Cx<St>| {
            let t = theme();
            toasts(cx, &t, &st.toasts, |s: &mut St| &mut s.toasts);
            column(vec![
                button_labeled(&t, "Primary").on_tap(cx, |_: &mut St| {}),
                button_variant(&t, "Ghost", Variant::Ghost).on_tap(cx, |_: &mut St| {}),
                switch(cx, &t, true, |_: &mut St| {}),
                switch(cx, &t, false, |_: &mut St| {}),
                checkbox(cx, &t, true, |_: &mut St| {}),
                checkbox(cx, &t, false, |_: &mut St| {}),
                radio(cx, &t, true, |_: &mut St| {}),
                radio(cx, &t, false, |_: &mut St| {}),
                tabs(cx, &t, &["A", "B"], 0, |_: &mut St, _| {}),
                select(cx, &t, &["x", "y"], &st.pick, |s: &mut St| &mut s.pick),
            ])
        };
        let (mut el, mut overlays, moving) = rig.build(t0 + i * 500 * MS, view);
        assert!(!moving, "build {i} asked for frames");
        assert!(!displaced(&mut el), "build {i}");
        for o in &mut overlays {
            assert!(!displaced(&mut o.content), "build {i}");
        }
    }
}

#[test]
fn the_drop_highlight_fades_with_the_level() {
    use crate::core::{DropId, DropState};
    let state = |over, accepted, level| DropState {
        over,
        accepted,
        position: None,
        level,
    };
    let states = [
        (DropId(0), state(true, true, 0.4)),
        (DropId(1), state(false, true, 0.7)),
        (DropId(2), state(true, false, 1.0)),
    ];
    let lit = crate::app_dnd::highlighted(&states, |id| id != DropId(1));
    assert_eq!(lit, vec![(DropId(0), 0.4)], "refused and self-styled: none");
    let lit = crate::app_dnd::highlighted(&states, |_| true);
    assert_eq!(lit.len(), 2, "one leaving still fades out");
}

/// Every element of `el` drawn moved, with its offset.
fn moved(el: &mut Element, out: &mut Vec<f64>) {
    if let Some(t) = el.transform {
        out.push(t.translate.dx);
    }
    if let Some(c) = el.children_mut() {
        for c in c {
            moved(c, out);
        }
    }
}

#[derive(Default)]
struct Bar {
    bar: TabBarState,
}

fn bar_view(s: &Bar, selected: usize) -> impl FnOnce(&mut Cx<Bar>) -> Element + '_ {
    move |cx| {
        let tabs = (0..3).map(|i| Tab::new(i, format!("Tab {i}"))).collect();
        let bar = TabBar::new("t", tabs).selected(selected).width(600.0);
        tab_bar(cx, &theme(), bar, &s.bar, |s: &mut Bar| &mut s.bar)
    }
}

#[test]
fn the_tab_indicator_slides_to_the_new_tab() {
    let mut rig = Rig::new();
    let s = Bar::default();
    let t0 = Duration::from_secs(10);
    let (mut el, _, moving) = rig.build(t0, bar_view(&s, 0));
    let mut at_rest = Vec::new();
    moved(&mut el, &mut at_rest);
    assert!(!moving && at_rest.is_empty(), "{at_rest:?}");
    // Tab 0 to tab 1: the strip leaves tab 0 rightwards and comes into tab 1
    // from the left, a tab's width (200 here, less the gaps) apart.
    let head = t0 + 500 * MS - frame();
    rig.build(head + frame(), bar_view(&s, 1));
    let (mut el, _, moving) = rig.build(head + 50 * MS, bar_view(&s, 1));
    assert!(moving);
    let mut dx = Vec::new();
    moved(&mut el, &mut dx);
    assert_eq!(dx.len(), 2, "{dx:?}");
    assert!(dx[0] > 0.0 && dx[1] < 0.0, "{dx:?}");
    assert!(near(dx[0], -dx[1]), "halfway: {dx:?}");
    // (The tabs' own fills take 120 ms.)
    let (mut el, _, moving) = rig.build(head + 200 * MS, bar_view(&s, 1));
    let mut after = Vec::new();
    moved(&mut el, &mut after);
    assert!(!moving && after.is_empty(), "{after:?}");
}

/// `open_menu` and `open_dialog` keep no state, so the core's enter
/// animations bring them in: shown after the first frame, they animate, then
/// rest. (Wall-clock frames, through an `App`.)
#[test]
fn stateless_popups_come_in_through_the_core() {
    use crate::App;
    use crate::geometry::{Point, Size};
    #[derive(Default)]
    struct Pop {
        menu: bool,
        dialog: bool,
    }
    let mut app = App::new(Pop::default(), |s: &Pop, cx: &mut Cx<Pop>| {
        let t = theme();
        if s.menu {
            let item = menu_item(cx, &t, "Cut", |_: &mut Pop| {});
            open_menu(cx, &t, Point::new(10.0, 10.0), vec![item], |s: &mut Pop| {
                s.menu = false
            });
        }
        if s.dialog {
            open_dialog(
                cx,
                &t,
                "Sure?",
                label(&t, "Really"),
                vec![],
                |s: &mut Pop| s.dialog = false,
            );
        }
        column(vec![
            button_labeled(&t, "Menu").on_tap(cx, |s: &mut Pop| s.menu = true),
            button_labeled(&t, "Dialog").on_tap(cx, |s: &mut Pop| s.dialog = true),
        ])
    })
    .logical_size(Size::new(400.0, 300.0));
    crate::anim::set_reduced_motion(Some(false));
    let _ = app.render_frame();
    assert!(!app.animating());
    assert!(app.click_at(Point::new(20.0, 15.0)), "opens the menu");
    let _ = app.render_frame();
    assert!(app.state().menu);
    // The menu is the window's top layer (over its press-outside catcher),
    // fading in.
    let layer = |app: &App<Pop>| {
        let tree = app.panes[0].tree.as_ref().unwrap();
        tree.children.last().unwrap().opacity
    };
    assert!(layer(&app) < 1.0, "the menu comes in: {}", layer(&app));
    std::thread::sleep(Duration::from_millis(400));
    let _ = app.render_frame();
    assert_eq!(layer(&app), 1.0);
    let _ = app.render_frame();
    assert!(!app.animating(), "then rests");
}

/// The topmost ghost (a copy drawn while it leaves) among the window's
/// layers, if any (over a leaving modal's scrim): where it's drawn, its
/// opacity and transform.
fn leaving(tree: &LayoutNode) -> Option<&LayoutNode> {
    tree.children.iter().rev().find(|c| c.ghost)
}

#[test]
fn a_select_list_fades_and_shrinks_out_when_it_closes() {
    let mut rig = Rig::new();
    let mut st = St {
        pick: Select::new(Some(0)),
        ..St::default()
    };
    st.pick.open();
    let t0 = Duration::from_secs(10);
    let (tree, moving) = rig.draw(t0, select_view(&st));
    assert!(!moving && leaving(&tree).is_none(), "open, at rest");
    let open = tree.children[1].clone();
    // Closed: the list is kept, where it was, as it leaves on the theme's
    // `popup_exit` (100 ms here).
    st.pick.close();
    let head = t0 + 500 * MS - frame();
    let (tree, moving) = rig.draw(head + frame(), select_view(&st));
    assert!(moving, "closing: frames");
    let ghost = leaving(&tree).expect("drawn while it leaves");
    assert_eq!(ghost.bounds, open.bounds);
    assert!(ghost.action.is_none() && ghost.semantics.is_none() && ghost.key.is_none());
    let (tree, _) = rig.draw(head + 50 * MS, select_view(&st));
    let ghost = leaving(&tree).expect("still leaving");
    let m = theme().motion;
    assert!(near(f64::from(ghost.opacity), 0.5), "{}", ghost.opacity);
    let tr = ghost.transform.expect("shrinking");
    assert!(near(
        tr.scale.dx,
        m.popup_scale + (1.0 - m.popup_scale) * 0.5
    ));
    assert!(near(tr.translate.dy, -m.popup_offset * 0.5), "back up");
    assert_eq!((tr.origin.x, tr.origin.y), (0.5, 0.0), "toward its button");
    let (tree, moving) = rig.draw(head + 101 * MS, select_view(&st));
    assert!(!moving, "gone: no more frames");
    assert_eq!(tree.children.len(), 1, "only the view");
}

#[test]
fn a_list_reopened_while_closing_turns_round() {
    let mut rig = Rig::new();
    let mut st = St::default();
    st.pick.open();
    let t0 = Duration::from_secs(10);
    rig.draw(t0, select_view(&st));
    st.pick.close();
    let head = t0 + 500 * MS - frame();
    rig.draw(head + frame(), select_view(&st));
    rig.draw(head + 50 * MS, select_view(&st));
    st.pick.open();
    let (tree, _) = rig.draw(head + 60 * MS, select_view(&st));
    assert!(leaving(&tree).is_none(), "back: no ghost");
    assert_eq!(tree.children.len(), 2);
}

#[test]
fn stateless_overlays_and_tooltips_leave_on_the_theme_motion() {
    use crate::geometry::Point;
    #[derive(Clone, Copy, PartialEq)]
    enum Up {
        Menu,
        Dialog,
        Tip,
    }
    let view = |up: Option<Up>| {
        move |cx: &mut Cx<()>| {
            let t = theme();
            match up {
                Some(Up::Menu) => {
                    let item = menu_item(cx, &t, "Cut", |_: &mut ()| {});
                    open_menu(cx, &t, Point::new(10.0, 10.0), vec![item], |_: &mut ()| {});
                }
                Some(Up::Dialog) => open_dialog(
                    cx,
                    &t,
                    "Sure?",
                    label(&t, "Really"),
                    vec![],
                    |_: &mut ()| {},
                ),
                Some(Up::Tip) => cx.overlay(OverlaySpec {
                    content: tooltip(&t, "Bold"),
                    anchor: crate::core::Anchor::Popup(Point::new(10.0, 10.0)),
                    modal: false,
                    dismiss: None,
                }),
                None => {}
            }
            column(vec![])
        }
    };
    let m = theme().motion;
    for (up, scale, dy) in [
        (Up::Menu, m.popup_scale, -m.popup_offset),
        (Up::Dialog, m.dialog_scale, 0.0),
        (Up::Tip, 1.0, 0.0),
    ] {
        let mut rig = Rig::new();
        let t0 = Duration::from_secs(10);
        let (_, moving) = rig.draw(t0, view(Some(up)));
        assert!(!moving, "there from the first frame: at rest");
        let head = t0 + 500 * MS - frame();
        rig.draw(head + frame(), view(None));
        let (tree, moving) = rig.draw(head + 50 * MS, view(None));
        assert!(moving);
        let ghost = leaving(&tree).expect("leaving");
        assert!(near(f64::from(ghost.opacity), 0.5), "{}", ghost.opacity);
        let tr = ghost
            .transform
            .unwrap_or(crate::geometry::Transform::IDENTITY);
        assert!(near(tr.scale.dx, scale + (1.0 - scale) * 0.5), "{tr:?}");
        assert!(near(tr.translate.dy, dy * 0.5), "{tr:?}");
        let (tree, moving) = rig.draw(head + 101 * MS, view(None));
        assert!(!moving && leaving(&tree).is_none(), "then gone");
    }
}

#[test]
fn a_dialog_and_its_scrim_come_in_together_on_the_theme_motion() {
    let view = |open: bool| {
        move |cx: &mut Cx<()>| {
            if open {
                let t = theme();
                open_dialog(
                    cx,
                    &t,
                    "Sure?",
                    label(&t, "Really"),
                    vec![],
                    |_: &mut ()| {},
                );
            }
            column(vec![])
        }
    };
    let mut rig = Rig::new();
    let t0 = Duration::from_secs(10);
    rig.draw(t0, view(false));
    let head = t0 + 500 * MS - frame();
    let (tree, moving) = rig.draw(head + frame(), view(true));
    assert!(moving, "opening: frames");
    assert_eq!(tree.children.len(), 3, "the view, the scrim, the dialog");
    let (tree, _) = rig.draw(head + 50 * MS, view(true));
    let (scrim, dialog) = (&tree.children[1], &tree.children[2]);
    assert!(near(f64::from(scrim.opacity), 0.5), "{}", scrim.opacity);
    assert!(near(f64::from(dialog.opacity), 0.5), "{}", dialog.opacity);
    assert_eq!(scrim.transform, None);
    let (tree, moving) = rig.draw(head + 101 * MS, view(true));
    assert!(!moving, "then at rest");
    assert_eq!(tree.children[1].opacity, 1.0);
    assert_eq!(tree.children[1].transform, None);
    // Reduced motion: the next one is there at once.
    let mut rig = Rig::new();
    rig.draw(t0, view(false));
    crate::anim::set_reduced_motion(Some(true));
    let (tree, moving) = rig.draw(t0 + 500 * MS, view(true));
    crate::anim::set_reduced_motion(Some(false));
    assert!(!moving);
    assert_eq!(tree.children[1].opacity, 1.0);
}

#[test]
fn reduced_motion_closes_overlays_at_once() {
    let mut rig = Rig::new();
    let mut st = St::default();
    st.pick.open();
    let t0 = Duration::from_secs(10);
    rig.draw(t0, select_view(&st));
    crate::anim::set_reduced_motion(Some(true));
    st.pick.close();
    let (tree, moving) = rig.draw(t0 + 500 * MS, select_view(&st));
    crate::anim::set_reduced_motion(Some(false));
    assert!(!moving && leaving(&tree).is_none(), "gone at once");
}
