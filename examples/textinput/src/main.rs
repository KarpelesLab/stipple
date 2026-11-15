//! An interactive text field driven by the real event loop (`App::run`).
//!
//! Used by the CI X11 interaction job to verify keyboard text input: the runner
//! launches this under Xvfb, focuses the field with `xdotool key Tab`, types
//! with `xdotool type`, and screenshots — confirming the
//! keysym → text → [`TextInput`] → re-present path on a real X server. The
//! buffer is caret-aware, so arrow keys move the caret and insert mid-string.

use stipple::prelude::*;

#[derive(Default)]
struct Form {
    buffer: TextInput,
}

/// Print the text and composition when they change, with
/// `STIPPLE_TEXTINPUT_ECHO` set, so CI can check what dead keys and compose
/// sequences typed.
fn echo(buffer: &TextInput) {
    use std::sync::Mutex;
    static LAST: Mutex<Option<(String, Option<String>)>> = Mutex::new(None);
    if std::env::var_os("STIPPLE_TEXTINPUT_ECHO").is_none() {
        return;
    }
    let now = (
        buffer.text().to_string(),
        buffer.buffer().preedit().map(str::to_string),
    );
    let mut last = LAST.lock().unwrap();
    if last.as_ref() != Some(&now) {
        println!("textinput: text={:?} preedit={:?}", now.0, now.1);
        *last = Some(now);
    }
}

fn view(state: &Form, cx: &mut Cx<Form>) -> Element {
    echo(&state.buffer);
    let field = text_field(cx, &state.buffer, |s| &mut s.buffer).height(64.0);
    panel(cx)
        .children(vec![label(cx, "Type here (Tab to focus):"), field])
        .padding(Insets::uniform(32.0))
}

fn main() {
    let app = App::new(Form::default(), view)
        .title("Stipple Text Input")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 240.0));
    app.run();
}

// In the browser, `src/web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
