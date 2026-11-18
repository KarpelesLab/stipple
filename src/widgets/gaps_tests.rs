//! Small gaps closed in an app: a nav stack's change handler, an untitled
//! sheet's name and Escape headless, a button's role whatever is added to
//! it, icon-only and rounded swipe actions, equal tabs, a refresh label
//! following the language, a press on a button over a pointer element, and
//! `App::state_mut`.

use super::*;
use crate::App;
use crate::core::{
    AccessNode, Cx, Element, KeyInput, LayoutNode, NodeContent, PointerEventKind, Role,
};
use crate::geometry::{Point, Size};
use crate::platform::{ButtonState, Event, PointerButton};
use crate::render::Font;

fn tree<S: 'static>(app: &mut App<S>) -> AccessNode {
    app.render_once();
    app.accessibility_tree().unwrap()
}

// --- nav_stack's on_change -------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Page {
    Scanner,
    Result,
}

struct Scan {
    nav: NavStack<Page>,
    camera_on: bool,
    changes: Vec<usize>,
}

fn scan_view(s: &Scan, cx: &mut Cx<Scan>) -> Element {
    NavStackView::new(&s.nav, |s: &mut Scan| &mut s.nav)
        .on_change(|s| {
            s.changes.push(s.nav.depth());
            s.camera_on = s.nav.depth() == 1;
        })
        .view(cx, |cx, page| match page {
            Page::Scanner => button(cx, "Scan").on_tap(cx, |s| s.nav.push(Page::Result)),
            Page::Result => column(vec![button(cx, "Done").on_tap(cx, |s| {
                s.nav.pop();
            })]),
        })
}

fn scan() -> App<Scan> {
    crate::anim::set_reduced_motion(Some(true));
    App::new(
        Scan {
            nav: NavStack::new(Page::Scanner),
            camera_on: true,
            changes: Vec::new(),
        },
        scan_view,
    )
    .no_font()
    .logical_size(Size::new(360.0, 640.0))
}

#[test]
fn a_nav_stack_hears_pushes_and_pops_by_the_app_and_by_back() {
    let mut app = scan();
    assert!(app.click(Role::Button, "Scan"));
    assert!(!app.state().camera_on, "the page over the scanner stops it");
    assert_eq!(app.state().changes, [2]);
    // Back (Android's, a gamepad's B) pops it: the stack's own handler.
    app.render_once();
    assert!(app.request_back());
    assert!(app.state().camera_on);
    assert_eq!(app.state().changes, [2, 1]);
    // And the app's own pop.
    app.click(Role::Button, "Scan");
    app.click(Role::Button, "Done");
    assert_eq!(app.state().changes, [2, 1, 2, 1]);
    // Handlers that don't move the pages don't run it.
    app.click(Role::Button, "Scan");
    app.render_once();
    app.click(Role::Button, "Scan");
    assert_eq!(app.state().changes.len(), 5);
    crate::anim::set_reduced_motion(None);
}

// --- the sheet's name, Escape headless, state_mut ------------------------------

#[derive(Default)]
struct Sheets {
    sheet: BottomSheet,
    name: Option<&'static str>,
    closed: u32,
}

fn sheets() -> App<Sheets> {
    crate::anim::set_reduced_motion(Some(true));
    App::new(Sheets::default(), |s: &Sheets, cx: &mut Cx<Sheets>| {
        let mut sheet = bottom_sheet(cx, &s.sheet, |s| &mut s.sheet)
            .child(label(cx, "Inside"))
            .on_dismiss(|s| s.closed += 1);
        if let Some(name) = s.name {
            sheet = sheet.a11y_label(name);
        }
        sheet.open(cx);
        label(cx, "Under")
    })
    .no_font()
    .logical_size(Size::new(360.0, 640.0))
}

fn dialog(app: &mut App<Sheets>) -> Option<AccessNode> {
    tree(app)
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::Dialog)
        .cloned()
}

#[test]
fn an_untitled_sheet_takes_its_name_from_the_app_or_has_none() {
    let mut app = sheets();
    app.state_mut().sheet.open();
    assert_eq!(dialog(&mut app).unwrap().name, "", "no English \"Sheet\"");
    app.state_mut().name = Some("Résultat");
    assert_eq!(dialog(&mut app).unwrap().name, "Résultat");
    // Escape closes it headless, as from the keyboard.
    assert!(app.press_key(KeyInput::Escape));
    assert!(!app.state().sheet.is_open());
    assert_eq!(app.state().closed, 1);
    assert!(dialog(&mut app).is_none());
    crate::anim::set_reduced_motion(None);
}

// --- Button's role --------------------------------------------------------------

#[test]
fn a_button_is_a_button_disabled_or_expandable() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        column(vec![
            Button::new("Save").view(cx).disabled(true),
            Button::new("More")
                .variant(Variant::Ghost)
                .view(cx)
                .expanded(false),
            button(cx, "Go").on_tap(cx, |_| {}),
        ])
    })
    .no_font()
    .logical_size(Size::new(300.0, 300.0));
    for name in ["Save", "More", "Go"] {
        assert!(app.find(Role::Button, name).is_some(), "{name}");
    }
    let more = app.find(Role::Button, "More").unwrap();
    assert_eq!(more.expanded, Some(false));
}

// --- swipe_row: icon-only actions, rounded corners -------------------------------

#[derive(Default)]
struct Mail {
    swipe: SwipeState,
    log: Vec<&'static str>,
}

const TRASH: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M5 6h14l-1 15H6z" fill="currentColor"/></svg>"#;

fn mail() -> App<Mail> {
    crate::anim::set_reduced_motion(Some(true));
    App::new(Mail::default(), |s: &Mail, cx: &mut Cx<Mail>| {
        let icon = std::sync::Arc::new(crate::render::Svg::parse(TRASH.as_bytes()).unwrap());
        let actions = SwipeActions::new()
            .trailing(
                SwipeAction::new("Delete", |s: &mut Mail| s.log.push("delete"))
                    .icon(icon)
                    .icon_only(),
            )
            .trailing(SwipeAction::new("Flag", |s: &mut Mail| s.log.push("flag")));
        column(vec![
            swipe_row(cx, &s.swipe, |s| &mut s.swipe, 1, actions)
                .radius(12.0)
                .child(label(cx, "Message").height(60.0)),
        ])
    })
    .no_font()
    .logical_size(Size::new(360.0, 200.0))
}

fn mouse(app: &mut App<Mail>, pressed: Option<bool>, x: f64) {
    let position = Point::new(x, 30.0);
    app.pointer_event(Event::PointerMoved { position });
    if let Some(pressed) = pressed {
        app.pointer_event(Event::PointerButton {
            button: PointerButton::Left,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
            position,
        });
    }
    app.render_once();
}

/// The texts drawn in `node`'s subtree.
fn texts(node: &LayoutNode, out: &mut Vec<String>) {
    if let NodeContent::Text { text, .. } = &node.content {
        out.push(text.clone());
    }
    for c in &node.children {
        texts(c, out);
    }
}

fn find_named<'a>(node: &'a LayoutNode, name: &str) -> Option<&'a LayoutNode> {
    if node
        .semantics
        .as_ref()
        .and_then(|s| s.label.as_deref())
        .is_some_and(|l| l == name)
    {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_named(c, name))
}

#[test]
fn a_swipe_action_shows_its_icon_alone_and_follows_the_rows_corners() {
    let mut app = mail();
    mouse(&mut app, Some(true), 300.0);
    mouse(&mut app, None, 280.0);
    mouse(&mut app, None, 100.0);
    mouse(&mut app, Some(false), 100.0);
    assert!(app.state().swipe.is_open(1));
    let root = app.panes[0].tree.as_ref().unwrap();
    let delete = find_named(root, "Delete").expect("the delete button");
    let flag = find_named(root, "Flag").expect("the flag button");
    let (mut on_delete, mut on_flag) = (Vec::new(), Vec::new());
    texts(delete, &mut on_delete);
    texts(flag, &mut on_flag);
    assert!(on_delete.is_empty(), "icon only: {on_delete:?}");
    assert_eq!(on_flag, ["Flag"]);
    assert_eq!(delete.decoration.radius, 12.0);
    // Still named for screen readers, and in the context menu.
    assert!(app.find(Role::Button, "Delete").is_some());
    crate::anim::set_reduced_motion(None);
}

// --- equal tabs ---------------------------------------------------------------

#[test]
fn equal_tabs_share_the_width_whatever_their_labels() {
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font");
        return;
    };
    let strip = |equal: bool| {
        let mut app = App::new(0usize, move |s: &usize, cx: &mut Cx<usize>| {
            column(vec![
                Tabs::new(&["2D", "Stacked view"], *s, |s: &mut usize, i| *s = i)
                    .equal(equal)
                    .view(cx),
            ])
        })
        .font(font.clone())
        .logical_size(Size::new(360.0, 100.0));
        let widths: Vec<f64> = ["2D", "Stacked view"]
            .iter()
            .map(|n| app.find(Role::Button, n).expect(n).bounds.width())
            .collect();
        widths
    };
    let natural = strip(false);
    assert!(natural[0] < natural[1], "{natural:?}");
    let equal = strip(true);
    assert!((equal[0] - equal[1]).abs() < 0.5, "{equal:?}");
    assert!(equal[0] > 150.0, "they fill the width: {equal:?}");
}

// --- the refresh label ------------------------------------------------------------

#[test]
fn the_refresh_label_follows_a_language_change() {
    struct Feed {
        refresh: RefreshState,
    }
    let mut app = App::new(
        Feed {
            refresh: RefreshState::new(),
        },
        |s: &Feed, cx: &mut Cx<Feed>| {
            pull_to_refresh(cx, &s.refresh, |s| &mut s.refresh, |_| {})
                .child(scroll(cx).children(vec![label(cx, "Item")]))
        },
    )
    .no_font()
    .logical_size(Size::new(300.0, 300.0));
    app.state_mut().refresh.begin();
    assert!(app.find(Role::Text, "Refresh…").is_some());
    app.state_mut().refresh.set_label("Actualiser");
    assert!(app.find(Role::Text, "Actualiser…").is_some());
}

// --- a press on a button over a pointer element -------------------------------------

#[derive(Default)]
struct Camera {
    log: Vec<&'static str>,
}

fn camera(button_inside: bool) -> App<Camera> {
    App::new(Camera::default(), move |_: &Camera, cx: &mut Cx<Camera>| {
        let torch = button(cx, "Torch")
            .width(80.0)
            .height(40.0)
            .on_tap(cx, |s| s.log.push("torch"));
        let view = |cx: &mut Cx<Camera>, children: Vec<Element>| {
            Element::layers(children)
                .width(300.0)
                .height(300.0)
                .on_pointer(cx, |ev, s: &mut Camera| {
                    if ev.kind == PointerEventKind::Down {
                        s.log.push("focus");
                    }
                })
        };
        if button_inside {
            view(cx, vec![torch])
        } else {
            let camera = view(cx, Vec::new());
            Element::layers(vec![camera, torch])
        }
    })
    .no_font()
    .logical_size(Size::new(300.0, 300.0))
}

fn press(app: &mut App<Camera>, at: Point) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.pointer_event(Event::PointerButton {
            button: PointerButton::Left,
            state,
            position: at,
        });
    }
}

#[test]
fn a_button_over_a_pointer_element_keeps_its_press() {
    let mut app = camera(false);
    let torch = app.find(Role::Button, "Torch").unwrap().bounds.center();
    press(&mut app, torch);
    assert_eq!(
        app.state().log,
        ["torch"],
        "the camera underneath heard nothing"
    );
    // Beside the button, the camera gets it.
    press(&mut app, Point::new(290.0, 290.0));
    assert_eq!(app.state().log, ["torch", "focus"]);
    // A button inside the pointer element leaves the press its too.
    let mut app = camera(true);
    let torch = app.find(Role::Button, "Torch").unwrap().bounds.center();
    press(&mut app, torch);
    assert_eq!(app.state().log, ["focus", "torch"]);
}
