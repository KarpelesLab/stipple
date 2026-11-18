//! Programmatic focus and dialogs' keys: an autofocus field taking focus
//! when its overlay opens and giving it back when it closes, focus by key
//! from the view and from a handler, blur; and a dialog's Escape, Enter and
//! width.

use crate::App;
use crate::core::{Anchor, Cx, Element, KeyInput, OverlaySpec, Role};
use crate::geometry::Size;
use crate::platform::{KeyCode, Modifiers, WindowId};
use crate::widgets::{TextInput, button, dialog, text_area, text_field};

#[derive(Default)]
struct State {
    open: bool,
    query: TextInput,
    other: TextInput,
    log: Vec<String>,
    /// Ask the view to focus this key on its next build.
    focus: Option<&'static str>,
    multiline: bool,
    submit: bool,
    keep_escape: bool,
    width: Option<f64>,
}

/// Change the state as a handler would: every window rebuilds.
fn set(app: &mut App<State>, f: impl FnOnce(&mut State)) {
    f(&mut app.state);
    for pane in &mut app.panes {
        pane.dirty = true;
    }
}

fn focused_name(app: &mut App<State>) -> Option<(Role, String)> {
    let tree = app.accessibility_tree()?;
    tree.descendants().into_iter().find(|n| n.focused).map(|n| {
        (
            n.role,
            n.placeholder.clone().unwrap_or_else(|| n.name.clone()),
        )
    })
}

fn overlay_view(s: &State, cx: &mut Cx<State>) -> Element {
    if s.open {
        let field = text_field(cx, &s.query, |s| &mut s.query)
            .placeholder("Find")
            .autofocus();
        cx.overlay(OverlaySpec::new(field, Anchor::Center));
    }
    if let Some(key) = s.focus {
        cx.focus(key);
    }
    let h = cx.handle();
    let win = cx.window_id();
    crate::widgets::column(vec![
        text_field(cx, &s.other, |s| &mut s.other)
            .placeholder("Terminal")
            .key("terminal"),
        button(cx, "Open").on_tap(cx, |s| s.open = true),
        button(cx, "Refocus").on_tap(cx, move |_| h.focus(win, "terminal")),
        button(cx, "Blur").on_tap(cx, {
            let h = cx.handle();
            move |_| h.blur(win)
        }),
    ])
}

fn overlay_app() -> App<State> {
    crate::anim::set_reduced_motion(Some(true));
    let mut app = App::new(State::default(), overlay_view)
        .no_font()
        .logical_size(Size::new(400.0, 300.0));
    app.render_once();
    app
}

#[test]
fn autofocus_takes_focus_when_shown_and_gives_it_back() {
    let mut app = overlay_app();
    assert!(app.focus_next(), "focus the terminal field");
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Terminal".into()))
    );
    set(&mut app, |s| s.open = true);
    app.render_once();
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Find".into())),
        "the field that showed took focus"
    );
    assert!(app.type_text("abc"));
    assert_eq!(app.state().query.text(), "abc");
    // It doesn't take it again while it stays.
    assert!(app.focus_next());
    app.render_once();
    // Closing it with focus in it gives focus back.
    app.focus_prev();
    set(&mut app, |s| s.open = false);
    app.render_once();
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Terminal".into())),
        "focus went back to the terminal"
    );
}

#[test]
fn focus_by_key_from_the_view_and_a_handler_and_blur() {
    let mut app = overlay_app();
    assert_eq!(focused_name(&mut app), None);
    set(&mut app, |s| s.focus = Some("terminal"));
    app.render_once();
    set(&mut app, |s| s.focus = None);
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Terminal".into()))
    );
    // A handler blurs, then focuses by key.
    assert!(app.click(Role::Button, "Blur"));
    app.render_once();
    assert_eq!(focused_name(&mut app), None);
    assert!(app.click(Role::Button, "Refocus"));
    app.render_once();
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Terminal".into()))
    );
    // An unknown key changes nothing.
    app.handle().focus(WindowId::PRIMARY, "nothing here");
    app.render_once();
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Terminal".into()))
    );
}

fn dialog_view(s: &State, cx: &mut Cx<State>) -> Element {
    if s.open {
        let field = if s.multiline {
            text_area(cx, &s.query, |s| &mut s.query).placeholder("Name")
        } else {
            let f = text_field(cx, &s.query, |s| &mut s.query).placeholder("Name");
            if s.submit {
                f.on_submit(cx, |s| s.log.push("submit".into()))
            } else {
                f
            }
        };
        let mut d = dialog(cx, "Rename", |s| {
            s.log.push("dismiss".into());
            s.open = false;
        })
        .child(field.autofocus())
        .actions(vec![
            button(cx, "Cancel").on_tap(cx, |s| s.log.push("cancel".into())),
        ])
        .default_button(cx, "Rename", |s| {
            s.log.push("rename".into());
            s.open = false;
        })
        .cancel_on_escape(!s.keep_escape);
        if let Some(w) = s.width {
            d = d.width(w);
        }
        d.open(cx);
    }
    button(cx, "Open").on_tap(cx, |s| s.open = true)
}

fn dialog_app(state: State) -> App<State> {
    crate::anim::set_reduced_motion(Some(true));
    let mut app = App::new(
        State {
            open: true,
            ..state
        },
        dialog_view,
    )
    .no_font()
    .logical_size(Size::new(500.0, 400.0));
    app.render_once();
    app
}

#[test]
fn enter_runs_the_default_action_from_the_field() {
    let mut app = dialog_app(State::default());
    assert_eq!(
        focused_name(&mut app),
        Some((Role::TextField, "Name".into()))
    );
    assert!(app.type_text("x"));
    assert!(app.press_key(KeyInput::Enter));
    assert_eq!(app.state().log, ["rename"]);
    assert!(!app.state().open);
}

#[test]
fn enter_on_a_button_presses_it_not_the_default() {
    let mut app = dialog_app(State::default());
    assert!(app.focus_next(), "focus Cancel");
    assert_eq!(
        focused_name(&mut app),
        Some((Role::Button, "Cancel".into()))
    );
    app.press_key(KeyInput::Enter);
    assert_eq!(app.state().log, ["cancel"]);
}

#[test]
fn enter_in_a_multiline_field_or_a_submitting_one_stays_there() {
    let mut app = dialog_app(State {
        multiline: true,
        ..State::default()
    });
    app.press_key(KeyInput::Enter);
    assert!(app.state().log.is_empty());
    assert_eq!(app.state().query.text(), "\n");

    let mut app = dialog_app(State {
        submit: true,
        ..State::default()
    });
    app.press_key(KeyInput::Enter);
    assert_eq!(app.state().log, ["submit"]);
}

#[test]
fn escape_dismisses_unless_turned_off() {
    let mut app = dialog_app(State::default());
    assert!(app.press_key_code(KeyCode::Escape, Modifiers::default()));
    assert_eq!(app.state().log, ["dismiss"]);

    let mut app = dialog_app(State {
        keep_escape: true,
        ..State::default()
    });
    app.press_key_code(KeyCode::Escape, Modifiers::default());
    assert!(app.state().log.is_empty());
    assert!(app.state().open);
}

#[test]
fn the_width_is_set_and_clamped_to_the_window() {
    let width = |w: Option<f64>| {
        let mut app = dialog_app(State {
            width: w,
            ..State::default()
        });
        app.find(Role::Dialog, "Rename")
            .expect("the dialog")
            .bounds
            .width()
    };
    assert_eq!(width(None), 340.0);
    assert_eq!(width(Some(420.0)), 420.0);
    // 500 wide, less a margin each side.
    let clamped = width(Some(900.0));
    assert!(clamped < 500.0 && clamped > 400.0, "{clamped}");
}
