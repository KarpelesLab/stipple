//! Every control under [`Theme::native`]: on Windows, macOS and iOS the OS draws
//! them, and on Linux they stay self-drawn in the system's colors. CI screenshots
//! this on each platform.

use stipple::prelude::*;

struct Gallery {
    checked: bool,
    choice: usize,
    on: bool,
    level: f64,
    name: TextInput,
}

fn view(state: &Gallery, cx: &mut Cx<Gallery>) -> Element {
    let theme = *cx.theme();
    let radios: Vec<Element> = ["Small", "Large"]
        .iter()
        .enumerate()
        .map(|(i, text)| radio(cx, *text, state.choice == i, move |s| s.choice = i))
        .collect();
    let check = checkbox(cx, "Show hidden files", state.checked, |s| {
        s.checked = !s.checked;
    });
    let toggle = switch(cx, "Sync", state.on, |s| s.on = !s.on);
    let level = slider(cx, state.level, |s, v| s.level = v);
    let name = text_field(cx, &state.name, |s| &mut s.name);
    column(vec![
        heading(cx, "Native controls"),
        name,
        row(vec![
            button(cx, "Save"),
            Button::new("Cancel").variant(Variant::Secondary).view(cx),
        ])
        .gap(theme.spacing.sm),
        check,
        row(radios).gap(theme.spacing.lg),
        toggle,
        level,
        progress_bar(cx, state.level),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
    .align(Align::Start, Align::Start)
}

fn main() {
    let state = Gallery {
        checked: true,
        choice: 0,
        on: true,
        level: 0.6,
        name: TextInput::from_text("Stipple"),
    };
    let app = App::new(state, view)
        .title("Stipple")
        .theme(Theme::native())
        .logical_size(Size::new(360.0, 340.0));
    app.run();
}

// In the browser, `src/web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
