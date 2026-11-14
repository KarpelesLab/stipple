//! Keyboard focus for buttons and the other controls a tap activates, driven
//! through an [`App`]: Tab and Shift+Tab across a field and buttons, Enter and
//! Space pressing the focused one, the focus ring, screen readers told, and
//! clicks leaving buttons unfocused.

use crate::App;
use crate::core::{Cx, KeyInput, OverlaySpec, Role};
use crate::geometry::{Insets, Size};
use crate::platform::Modifiers;
use crate::widgets::{EditBuffer, button_labeled, checkbox, column, text_editor};

#[derive(Default)]
struct Form {
    name: EditBuffer,
    saved: u32,
    cancelled: u32,
    agree: bool,
    dialog: bool,
    dialog_ok: u32,
}

fn view(s: &Form, cx: &mut Cx<Form>) -> crate::core::Element {
    let theme = *cx.theme();
    let main = column(vec![
        text_editor(cx, &theme, &s.name, |s: &mut Form| &mut s.name).width(200.0),
        button_labeled(&theme, "Save").on_tap(cx, |s: &mut Form| s.saved += 1),
        button_labeled(&theme, "Cancel").on_tap(cx, |s: &mut Form| s.cancelled += 1),
        checkbox(cx, &theme, s.agree, |s: &mut Form| s.agree = !s.agree),
    ])
    .gap(8.0)
    .padding(Insets::uniform(10.0));
    if s.dialog {
        let ok = button_labeled(&theme, "OK").on_tap(cx, |s: &mut Form| s.dialog_ok += 1);
        cx.overlay(OverlaySpec {
            content: ok,
            anchor: crate::core::Anchor::Center,
            modal: true,
            dismiss: None,
        });
    }
    main
}

fn app_with(form: Form) -> App<Form> {
    let mut app = App::new(form, view).logical_size(Size::new(300.0, 300.0));
    app.render_once();
    app
}

fn app() -> App<Form> {
    app_with(Form::default())
}

/// The role and name of the focused node in the accessibility tree.
fn focused(app: &mut App<Form>) -> Option<(Role, String)> {
    app.render_once();
    let tree = app.accessibility_tree()?;
    tree.descendants()
        .into_iter()
        .find(|n| n.focused)
        .map(|n| (n.role, n.name.clone()))
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::default()
    }
}

#[test]
fn tab_cycles_through_the_field_and_the_buttons_and_shift_tab_goes_back() {
    let mut app = app();
    assert!(app.tab_key(0, Modifiers::default()));
    assert_eq!(focused(&mut app).map(|f| f.0), Some(Role::TextField));
    assert!(app.tab_key(0, Modifiers::default()));
    assert_eq!(focused(&mut app), Some((Role::Button, "Save".into())));
    assert!(app.tab_key(0, Modifiers::default()));
    assert_eq!(focused(&mut app), Some((Role::Button, "Cancel".into())));
    assert!(app.tab_key(0, Modifiers::default()));
    assert_eq!(focused(&mut app).map(|f| f.0), Some(Role::CheckBox));
    // Round to the field again.
    assert!(app.tab_key(0, Modifiers::default()));
    assert_eq!(focused(&mut app).map(|f| f.0), Some(Role::TextField));
    // And back: Shift+Tab wraps to the checkbox, then Cancel.
    assert!(app.tab_key(0, shift()));
    assert_eq!(focused(&mut app).map(|f| f.0), Some(Role::CheckBox));
    assert!(app.tab_key(0, shift()));
    assert_eq!(focused(&mut app), Some((Role::Button, "Cancel".into())));
    assert!(app.focus_prev());
    assert_eq!(focused(&mut app), Some((Role::Button, "Save".into())));
}

#[test]
fn enter_and_space_press_the_focused_button_and_nothing_else_reaches_it() {
    let mut app = app();
    app.focus_next();
    app.focus_next();
    assert_eq!(focused(&mut app), Some((Role::Button, "Save".into())));
    assert!(app.press_key(KeyInput::Enter));
    assert_eq!(app.state().saved, 1);
    assert!(app.type_text(" "));
    assert_eq!(app.state().saved, 2);
    // Other keys and text don't press it (or go anywhere).
    assert!(!app.type_text("x"));
    assert!(!app.press_key(KeyInput::Backspace));
    assert_eq!(app.state().saved, 2);
    assert_eq!(app.state().name.text(), "");
    // The checkbox toggles with Space.
    app.focus_next();
    app.focus_next();
    assert!(app.type_text(" "));
    assert!(app.state().agree);
    assert_eq!(app.state().cancelled, 0);
    // Focus is still there after the rebuild.
    assert_eq!(focused(&mut app).map(|f| f.0), Some(Role::CheckBox));
}

#[test]
fn the_focused_button_gets_the_ring_and_a_click_does_not_focus_a_button() {
    let mut app = app();
    let ring = app.theme.palette.focus_ring;
    // The ring's color, give or take its antialiased edge.
    let is_ring = |px: Option<[u8; 4]>| {
        px.is_some_and(|p| {
            [ring.r, ring.g, ring.b]
                .iter()
                .zip(p)
                .all(|(a, b)| a.abs_diff(b) < 16)
        })
    };
    app.focus_next();
    app.render_once();
    let tree = app.panes[0].tree.as_ref().unwrap();
    let save = tree.children[1].bounds;
    let out = crate::core::tab_focus::RING_OFFSET;
    let edge = ((save.min_x() - out) as u32, save.center().y as u32);
    let (before, _) = app.render_frame();
    assert!(
        !is_ring(before.pixel(edge.0, edge.1)),
        "no ring on Save yet"
    );
    app.focus_next();
    let (after, _) = app.render_frame();
    assert!(is_ring(after.pixel(edge.0, edge.1)), "the ring around Save");
    let node = crate::core::find_focus(app.panes[0].tree.as_ref().unwrap(), app.focused().unwrap())
        .expect("the button is in the tree");
    assert!(crate::core::tab_focus::is_tap_focus(node));
    // A click on Cancel presses it but leaves no focus (and no ring) on it,
    // as a click on a button moved focus away from a field before.
    let cancel = tree_child(&app, 2).center();
    assert!(app.click_at(cancel));
    assert_eq!(app.state().cancelled, 1);
    assert_eq!(app.focused(), None);
    let (clicked, _) = app.render_frame();
    assert!(!is_ring(clicked.pixel(edge.0, edge.1)), "the ring went");
    // A click on the field still focuses it.
    assert!(app.click_at(tree_child(&app, 0).center()));
    assert_eq!(focused(&mut app).map(|f| f.0), Some(Role::TextField));
}

fn tree_child(app: &App<Form>, i: usize) -> crate::geometry::Rect {
    app.panes[0].tree.as_ref().unwrap().children[i].bounds
}

#[test]
fn a_modal_overlay_keeps_tab_inside_it() {
    let mut app = app_with(Form {
        dialog: true,
        ..Form::default()
    });
    for _ in 0..3 {
        app.focus_next();
        assert_eq!(focused(&mut app), Some((Role::Button, "OK".into())));
    }
    assert!(app.press_key(KeyInput::Enter));
    assert_eq!(app.state().dialog_ok, 1);
    assert_eq!(app.state().saved, 0);
}
