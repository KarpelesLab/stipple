//! A multi-line text area driven by the real event loop (`App::run`).
//!
//! Used by the CI X11 interaction job to verify multi-line editing: type across
//! lines (Enter inserts a newline), navigate with arrows, and select across
//! lines with Shift+Down — the caret and selection render on the right line.

use stipple::prelude::*;

#[derive(Default)]
struct Form {
    buffer: TextInput,
}

fn view(state: &Form, cx: &mut Cx<Form>) -> Element {
    let area = text_area(cx, &state.buffer, |s| &mut s.buffer)
        .height(200.0)
        .placeholder("Write a few lines…");
    panel(cx, |cx| {
        vec![label(cx, "Multi-line editor (Tab to focus):"), area]
    })
    .padding(Insets::uniform(32.0))
}

fn main() {
    let app = App::new(Form::default(), view)
        .title("Stipple Text Area")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 320.0));
    app.run();
}
