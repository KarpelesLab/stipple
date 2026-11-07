//! Copy/cut/paste in a caret-aware text field. The field starts with "Clip";
//! CI focuses it, selects all (Ctrl+A), copies (Ctrl+C), moves to the end, and
//! pastes (Ctrl+V) — doubling the text to "ClipClip" — then screenshots before
//! and after to confirm the paste changed the field. The copied text is also
//! pushed to the OS clipboard (verified separately via xclip).
//!
//! `clipboarddemo --roundtrip` skips the window: it writes a string to the
//! system clipboard with `stipple::platform::clipboard`, reads it back, and
//! prints the result (exiting non-zero on a mismatch). CI runs it on Windows and
//! macOS, and cross-checks with the OS's own `Get-Clipboard` / `pbpaste`.

use stipple::prelude::*;
use stipple::widgets::EditBuffer;

#[derive(Default)]
struct State {
    buffer: EditBuffer,
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let field = text_editor(cx, &theme, &state.buffer, |s: &mut State| &mut s.buffer)
        .width(560.0)
        .height(64.0);
    panel(
        &theme,
        vec![label(&theme, "Clipboard (Tab to focus):"), field],
    )
    .padding(Insets::uniform(32.0))
    .align(Align::Start, Align::Stretch)
}

/// Set the system clipboard, read it back, and report.
fn roundtrip() {
    let sent = "stipple clipboard \u{2713} round-trip";
    let set = stipple::platform::clipboard::set_text(sent);
    let got = stipple::platform::clipboard::text();
    println!("clipboard set: {set}");
    println!("clipboard read: {got:?}");
    if got.as_deref() == Some(sent) {
        println!("clipboard roundtrip: ok");
    } else {
        eprintln!("clipboard roundtrip: mismatch (sent {sent:?})");
        std::process::exit(1);
    }
}

fn main() {
    if std::env::args().any(|a| a == "--roundtrip") {
        roundtrip();
        return;
    }
    let mut app = App::new(
        State {
            buffer: EditBuffer::from_text("Clip"),
        },
        view,
    )
    .title("Stipple Clipboard")
    .theme(Theme::dark())
    .logical_size(Size::new(640.0, 240.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
