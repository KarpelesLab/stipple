//! App-level tests of the message box drawn in the window.

use crate::App;
use crate::core::{AccessNode, Cx, Element, KeyInput, Role};
use crate::geometry::{Insets, Point, Size};
use crate::platform::dialog::{MessageBox, MessageKind, MessageResponse};

#[derive(Default)]
struct State {
    answers: Vec<MessageResponse>,
    clicks: u32,
    name: super::EditBuffer,
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    super::column(vec![
        super::button_labeled(&theme, "Behind").on_tap(cx, |s: &mut State| s.clicks += 1),
        super::text_editor(cx, &theme, &s.name, |s: &mut State| &mut s.name),
    ])
    .padding(Insets::uniform(10.0))
}

fn app() -> App<State> {
    let mut app = App::new(State::default(), view).logical_size(Size::new(520.0, 360.0));
    app.render_once();
    app
}

fn ask(app: &App<State>, mb: MessageBox) {
    app.handle()
        .message(mb, |s: &mut State, r| s.answers.push(r));
}

fn find(tree: &AccessNode, f: impl Fn(&AccessNode) -> bool) -> Option<&AccessNode> {
    tree.descendants().into_iter().find(|n| f(n))
}

fn close_tab() -> MessageBox {
    MessageBox::new("Close this tab?")
        .detail("A process is still running in it.")
        .kind(MessageKind::Question)
        .buttons(["Close", "Cancel"])
        .cancel(1)
        .drawn(true)
}

#[test]
fn shows_a_modal_dialog_with_the_default_button_focused() {
    let mut app = app();
    ask(&app, close_tab());
    assert!(app.run_pending());
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let dialog = find(&tree, |n| n.role == Role::Dialog).expect("a dialog");
    assert_eq!(dialog.name, "Close this tab?");
    let close = find(dialog, |n| n.role == Role::Button && n.name == "Close").expect("Close");
    assert!(close.focused, "the default button takes focus");
    // The window behind is blocked.
    let behind = find(&tree, |n| n.name == "Behind").unwrap().bounds.center();
    app.click_at(behind);
    assert_eq!(app.state().clicks, 0);
    // Enter presses the default button.
    assert!(app.press_key(KeyInput::Enter));
    assert_eq!(
        app.state().answers,
        [MessageResponse {
            button: Some(0),
            input: None
        }]
    );
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    assert!(find(&tree, |n| n.role == Role::Dialog).is_none());
    app.click_at(behind);
    assert_eq!(app.state().clicks, 1);
}

#[test]
fn escape_cancels_and_buttons_answer_by_index() {
    let mut app = app();
    ask(&app, close_tab());
    ask(
        &app,
        MessageBox::new("Save changes?")
            .buttons(["Save", "Don't Save", "Cancel"])
            .default_button(0)
            .drawn(true),
    );
    app.run_pending();
    app.render_once();
    app.press_key(KeyInput::Escape);
    assert_eq!(app.state().answers[0].button, Some(1));
    // The second waited its turn.
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let dialog = find(&tree, |n| n.role == Role::Dialog).expect("the second box");
    assert_eq!(dialog.name, "Save changes?");
    // Without a cancel button, Escape does nothing.
    app.press_key(KeyInput::Escape);
    assert_eq!(app.state().answers.len(), 1);
    let dont = find(&tree, |n| n.name == "Don't Save")
        .unwrap()
        .bounds
        .center();
    assert!(app.click_at(dont));
    assert_eq!(app.state().answers[1].button, Some(1));
}

#[test]
fn a_secure_input_takes_text_and_focus_comes_back() {
    let mut app = app();
    // Focus the text field behind first.
    assert!(app.focus_next());
    let before = app.focused();
    assert!(before.is_some());
    ask(
        &app,
        MessageBox::new("Passphrase for id_ed25519")
            .buttons(["Unlock", "Cancel"])
            .cancel(1)
            .input("", true)
            .drawn(true),
    );
    app.run_pending();
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let field = find(&tree, |n| n.role == Role::PasswordField).expect("a password field");
    assert!(field.focused);
    assert!(app.type_text("s3cret"));
    app.render_once();
    // The typed text didn't reach the field behind.
    assert!(app.state().name.is_empty());
    app.press_key(KeyInput::Enter);
    assert_eq!(
        app.state().answers,
        [MessageResponse {
            button: Some(0),
            input: Some("s3cret".into())
        }]
    );
    app.render_once();
    assert_eq!(app.focused(), before);
    // A plain input starts with its value selected; typing replaces it.
    ask(
        &app,
        MessageBox::new("Rename tab")
            .buttons(["Rename", "Cancel"])
            .cancel(1)
            .input("Shell", false)
            .drawn(true),
    );
    app.run_pending();
    app.render_once();
    assert!(app.type_text("Logs"));
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let ok = find(&tree, |n| n.name == "Rename").unwrap().bounds.center();
    assert!(app.click_at(Point::new(ok.x, ok.y)));
    assert_eq!(app.state().answers[1].input.as_deref(), Some("Logs"));
}

#[test]
fn without_an_event_loop_boxes_are_drawn() {
    // Not marked `drawn`: in a test there's no loop, so no native alert.
    let mut app = app();
    ask(&app, MessageBox::new("Saved").title("Notes"));
    app.run_pending();
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let dialog = find(&tree, |n| n.role == Role::Dialog).expect("a dialog");
    assert_eq!(dialog.name, "Notes");
    app.press_key(KeyInput::Escape); // a lone OK cancels too
    assert_eq!(app.state().answers[0].button, Some(0));
}
