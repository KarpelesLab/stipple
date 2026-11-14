//! Interaction states and implicit transitions, driven through an [`App`].

use std::cell::Cell;
use std::time::Duration;

use crate::App;
use crate::anim::Easing;
use crate::core::{Cx, Element, ElementKey, Interaction, LayoutNode, Transition};
use crate::geometry::{Point, Size};
use crate::layout::Axis;
use crate::platform::DragOffer;
use crate::render::Color;

const RED: Color = Color::rgb(255, 0, 0);
const GRAY: Color = Color::rgb(128, 128, 128);

fn tile() -> Element {
    Element::stack(Axis::Vertical, vec![])
        .width(100.0)
        .height(50.0)
        .fill(GRAY)
}

fn tree<S: 'static>(app: &App<S>) -> &LayoutNode {
    app.panes[0].tree.as_ref().expect("a frame was built")
}

/// The fill of the `i`th child of the root.
fn fill<S: 'static>(app: &App<S>, i: usize) -> Option<Color> {
    tree(app).children[i].decoration.fill
}

fn two_tiles(first: Element) -> App<()> {
    App::new((), move |_: &(), _: &mut Cx<()>| {
        Element::stack(Axis::Vertical, vec![first.clone(), tile()])
    })
    .logical_size(Size::new(200.0, 100.0))
}

#[test]
fn a_hover_override_restyles_and_only_its_states_rebuild() {
    let mut app = two_tiles(tile().hovered(|e| e.fill(RED)));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(GRAY));
    // Over the plain tile: nothing uses that, so nothing to rebuild.
    assert!(!app.hover_at(Point::new(10.0, 70.0)));
    // Over the styled one: rebuild, restyled.
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(RED));
    // Moving within it changes nothing.
    assert!(!app.hover_at(Point::new(20.0, 20.0)));
    assert!(app.hover_at(Point::new(10.0, 70.0)));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(GRAY));
    assert!(!app.animating(), "no transition: nothing to animate");
}

#[test]
fn a_transition_animates_the_change_then_rests() {
    let mut app = two_tiles(
        tile()
            .hovered(|e| e.fill(RED))
            .transition(Transition::tween(Duration::from_secs(2), Easing::Linear)),
    );
    let _ = app.render_frame();
    let _ = app.render_frame();
    assert!(!app.animating(), "at rest: no frames");
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert!(app.animating(), "moving: frames");
    // (Frames come from the wall clock here; fixed frame times are tested in
    // `core::transition`.)
    std::thread::sleep(Duration::from_millis(50));
    let _ = app.render_frame();
    let mid = fill(&app, 0).expect("a fill");
    assert!(mid != GRAY && mid != RED, "between: {mid:?}");
}

#[test]
fn a_tappable_element_styling_its_own_hover_gets_no_highlight() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::stack(
            Axis::Vertical,
            vec![
                tile().hovered(|e| e.fill(RED)).on_tap(cx, |_| {}),
                tile().on_tap(cx, |_| {}),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert!(app.panes[0].highlight_alphas.is_empty(), "its own look");
    assert!(app.hover_at(Point::new(10.0, 70.0)));
    let _ = app.render_frame();
    assert!(!app.panes[0].highlight_alphas.is_empty(), "the theme's");
}

#[test]
fn the_view_reads_keyed_states_through_cx() {
    thread_local!(static SEEN: Cell<Interaction> = const { Cell::new(Interaction::NONE) });
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        SEEN.with(|s| s.set(cx.interaction("card")));
        Element::stack(
            Axis::Vertical,
            vec![
                tile().key("card").on_key(cx, |_, _| {}),
                tile().on_key(cx, |_, _| {}),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    assert!(app.hover_at(Point::new(10.0, 10.0)), "the view reads it");
    let _ = app.render_frame();
    assert!(SEEN.with(Cell::get).hovered());
    // Tab focuses it from the keyboard: focus-visible.
    assert!(app.focus_next());
    let _ = app.render_frame();
    let s = SEEN.with(Cell::get);
    assert!(s.focused() && s.focus_visible(), "{s:?}");
    // A click on the other moves focus away.
    assert!(app.click_at(Point::new(10.0, 70.0)));
    let _ = app.render_frame();
    assert!(!SEEN.with(Cell::get).focused());
    // Pressed while held over it.
    app.pointer_press(Point::new(10.0, 10.0));
    let _ = app.render_frame();
    assert!(SEEN.with(Cell::get).pressed());
    app.pointer_release(Point::new(10.0, 10.0));
    let _ = app.render_frame();
    let s = SEEN.with(Cell::get);
    assert!(!s.pressed() && s.focused() && !s.focus_visible(), "{s:?}");
}

#[test]
fn a_drop_target_styles_its_drag_over_look() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::stack(
            Axis::Vertical,
            vec![
                tile()
                    .drag_over(|e| e.fill(RED))
                    .drag_refused(|e| e.fill(Color::rgb(0, 0, 0)))
                    .on_drop_of(cx, DragOffer::FILES, |_, _, _| {}),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    assert!(app.drag_enter(Point::new(10.0, 10.0), DragOffer::FILES));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(RED));
    assert_eq!(
        app.panes[0].dnd.highlight,
        Vec::new(),
        "its own look, not the app's"
    );
    app.drag_leave();
    assert!(!app.drag_enter(Point::new(10.0, 10.0), DragOffer::TEXT));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(Color::rgb(0, 0, 0)), "refused");
    app.drag_leave();
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(GRAY));
}

#[test]
fn a_dragged_source_knows_it() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::stack(
            Axis::Vertical,
            vec![
                tile()
                    .dragged(|e| e.radius(9.0))
                    .drag_source(cx, |_| Some(crate::platform::DropData::Text("x".into()))),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    app.pointer_press(Point::new(10.0, 10.0));
    app.pointer_drag_to(Point::new(10.0, 30.0));
    let _ = app.render_frame();
    assert_eq!(tree(&app).children[0].decoration.radius, 9.0);
    app.pointer_release(Point::new(10.0, 30.0));
    let _ = app.render_frame();
    assert_eq!(tree(&app).children[0].decoration.radius, 0.0);
}

#[test]
fn keys_are_on_the_laid_out_tree() {
    let mut app = two_tiles(tile().key("k").transition(Transition::default()));
    let _ = app.render_frame();
    assert_eq!(tree(&app).children[0].key, Some(ElementKey::new("k")));
    assert_eq!(
        tree(&app).children[1].key,
        None,
        "plain elements carry none"
    );
}

#[test]
fn a_leaving_element_stays_drawn_but_clicks_go_through_it() {
    use crate::core::Effect;
    #[derive(Default)]
    struct S {
        items: Vec<u32>,
        tapped: Vec<u32>,
    }
    let slow = Transition::tween(Duration::from_secs(5), Easing::Linear);
    let mut app = App::new(
        S {
            items: vec![1, 2, 3],
            tapped: Vec::new(),
        },
        move |s: &S, cx: &mut Cx<S>| {
            let rows = s
                .items
                .iter()
                .map(|&id| {
                    tile()
                        .key(id)
                        .exit(Effect::fade().transition(slow))
                        .animate_position(slow)
                        .on_tap(cx, move |s: &mut S| {
                            s.tapped.push(id);
                            s.items.retain(|&i| i != id);
                        })
                })
                .collect();
            Element::stack(Axis::Vertical, rows)
        },
    )
    .logical_size(Size::new(200.0, 200.0));
    let _ = app.render_frame();
    assert!(!app.animating());
    // Tapping the first removes it.
    assert!(app.click_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert!(app.animating(), "leaving: frames");
    let root = tree(&app);
    assert_eq!(root.children.len(), 3, "the ghost, 2 and 3");
    assert!(root.children[0].ghost);
    // The ghost is drawn where 1 was, but it can't be tapped.
    let _ = app.click_at(Point::new(10.0, 10.0));
    assert_eq!(app.state().tapped, vec![1], "nothing there to tap");
    // 2 is laid out there now, but drawn where it was, sliding up (slowly).
    assert!(app.click_at(Point::new(10.0, 52.0)));
    assert_eq!(app.state().tapped, vec![1, 2]);
    let _ = app.render_frame();
    let a11y = crate::core::a11y::accessibility_tree(tree(&app), None);
    let buttons = a11y.descendants().len() - 1;
    assert_eq!(buttons, 1, "only 3 is left to read: {a11y:?}");
}

/// A theme whose popups and dialogs open at once and take 5 s to close, so a
/// test sees them leaving.
fn slow_exits() -> crate::style::Theme {
    let slow = crate::anim::Motion::tween(5.0, Easing::Linear);
    crate::style::Theme::light().with_motion(crate::style::MotionTokens {
        popup: crate::anim::Motion::Instant,
        dialog: crate::anim::Motion::Instant,
        popup_exit: slow,
        dialog_exit: slow,
        ..crate::style::MotionTokens::standard()
    })
}

/// How many ghosts (copies drawn while something leaves) `node` holds.
fn ghosts(node: &LayoutNode) -> usize {
    if node.ghost {
        1
    } else {
        node.children.iter().map(ghosts).sum()
    }
}

#[test]
fn a_closing_popup_is_drawn_but_clicks_focus_and_readers_go_past_it() {
    use crate::widgets::{Select, select};
    #[derive(Default)]
    struct S {
        size: Select,
        under: u32,
    }
    let theme = slow_exits();
    let mut app = App::new(S::default(), move |s: &S, cx: &mut Cx<S>| {
        let pick = select(cx, &theme, &["Small", "Large"], &s.size, |s: &mut S| {
            &mut s.size
        });
        // A big target under where the list drops.
        let under = Element::stack(Axis::Vertical, vec![])
            .width(300.0)
            .height(200.0)
            .on_tap(cx, |s: &mut S| s.under += 1);
        Element::stack(Axis::Vertical, vec![pick, under])
    })
    .theme(slow_exits())
    .logical_size(Size::new(300.0, 300.0));
    let _ = app.render_frame();
    // Open the list, and find "Large" in it.
    assert!(app.click_at(Point::new(20.0, 10.0)));
    let _ = app.render_frame();
    assert!(app.state().size.is_open());
    let a11y = crate::core::a11y::accessibility_tree(tree(&app), None);
    let large = a11y
        .descendants()
        .into_iter()
        .find(|n| n.name == "Large")
        .expect("the open list is read")
        .bounds;
    let at = Point::new(large.min_x() + 10.0, large.min_y() + large.height() / 2.0);
    // Choosing it closes the list: it stays drawn while it fades out...
    assert!(app.click_at(at));
    let _ = app.render_frame();
    assert!(!app.state().size.is_open());
    assert_eq!(app.state().size.selected(), Some(1));
    assert!(app.animating(), "leaving: frames");
    assert_eq!(ghosts(tree(&app)), 1, "the list, leaving");
    // ...but a click where it is goes to what's under it,
    assert!(app.click_at(at));
    assert_eq!(app.state().under, 1, "the click went through");
    assert!(!app.state().size.is_open(), "not opened or chosen again");
    // and the keyboard and screen readers don't see it.
    let _ = app.render_frame();
    let mut focusables = Vec::new();
    crate::core::collect_focusables(tree(&app), &mut focusables);
    assert_eq!(
        focusables.len(),
        2,
        "only the select's button and the target under it"
    );
    let a11y = crate::core::a11y::accessibility_tree(tree(&app), None);
    assert!(
        !a11y.descendants().iter().any(|n| n.name == "Small"),
        "{a11y:?}"
    );
}

#[test]
fn a_closing_dialog_fades_with_its_scrim_which_no_longer_blocks() {
    #[derive(Default)]
    struct S {
        open: bool,
        under: u32,
    }
    let theme = slow_exits();
    let state = S {
        open: true,
        under: 0,
    };
    let mut app = App::new(state, move |s: &S, cx: &mut Cx<S>| {
        if s.open {
            crate::widgets::open_dialog(
                cx,
                &theme,
                "Title",
                Element::stack(Axis::Vertical, vec![]),
                vec![],
                |s: &mut S| s.open = false,
            );
        }
        Element::stack(Axis::Vertical, vec![])
            .width(300.0)
            .height(300.0)
            .on_tap(cx, |s: &mut S| s.under += 1)
    })
    .theme(slow_exits())
    .logical_size(Size::new(400.0, 400.0));
    let _ = app.render_frame();
    // The scrim takes the click (and dismisses the dialog).
    assert!(app.click_at(Point::new(5.0, 5.0)));
    assert_eq!(app.state().under, 0);
    assert!(!app.state().open);
    let _ = app.render_frame();
    let root = tree(&app);
    assert_eq!(ghosts(root), 2, "the dialog and its scrim, leaving");
    let scrim = root.children.iter().find(|c| c.ghost).expect("a scrim");
    assert!(scrim.decoration.fill.is_some());
    // Leaving, the scrim lets clicks through.
    assert!(app.click_at(Point::new(5.0, 5.0)));
    assert_eq!(app.state().under, 1);
}

/// The segmented `tabs` view, `selected` chosen, in a dark window.
fn tab_strip(selected: usize) -> App<usize> {
    App::new(selected, |s: &usize, cx: &mut Cx<usize>| {
        let t = crate::style::Theme::dark();
        let labels = ["Profile", "Settings", "About"];
        let strip = crate::widgets::tabs(cx, &t, &labels, *s, |s: &mut usize, i| *s = i);
        Element::stack(Axis::Vertical, vec![strip]).padding(crate::geometry::Insets::uniform(15.0))
    })
    .theme(crate::style::Theme::dark())
    .logical_size(Size::new(360.0, 100.0))
}

#[test]
fn the_tab_indicator_slides_and_presenting_only_the_damage_leaves_nothing_stale() {
    use crate::core::Damage;
    let mut app = tab_strip(0);
    let (first, _) = app.render_frame();
    let (w, h) = (first.size().width as usize, first.size().height as usize);
    let stride = first.stride();
    // A window that is only ever sent the damaged parts of each frame.
    let mut screen = first.as_bytes().to_vec();
    // The labels (the top layer) take the taps.
    let settings = tree(&app).children[0].children[2].children[1].bounds;
    assert!(app.click_at(Point::new(settings.min_x() + 5.0, settings.min_y() + 5.0)));
    assert_eq!(*app.state(), 1);
    let full = crate::geometry::Rect::from_xywh(0.0, 0.0, w as f64, h as f64);
    let mut slid = false;
    for _ in 0..300 {
        let (p, d) = app.render_frame();
        // The indicator (the middle layer's selected cell) is drawn moved.
        slid |= tree(&app).children[0].children[1].children[1]
            .transform
            .is_some();
        let rects = match &d {
            Damage::Full => vec![full],
            Damage::None => vec![],
            r => r.to_physical(crate::geometry::ScaleFactor::new(1.0), full),
        };
        for r in rects {
            for y in r.min_y() as usize..r.max_y() as usize {
                let (s, e) = (
                    y * stride + r.min_x() as usize * 4,
                    y * stride + r.max_x() as usize * 4,
                );
                screen[s..e].copy_from_slice(&p.as_bytes()[s..e]);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
        if !app.animating() {
            break;
        }
    }
    assert!(slid, "the indicator slid over");
    assert!(!app.animating(), "then rests");
    // At rest it's what the strip draws when it starts on that tab.
    let rest = tab_strip(1).render_once();
    assert!(screen == rest.as_bytes(), "stale pixels left on screen");
}
