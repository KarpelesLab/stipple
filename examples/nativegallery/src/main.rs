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

fn labeled<S>(cx: &Cx<S>, control: Element, text: &str) -> Element {
    let theme = cx.theme();
    row(vec![control, label(cx, text)]).gap(theme.spacing.sm)
}

fn view(state: &Gallery, cx: &mut Cx<Gallery>) -> Element {
    let theme = *cx.theme();
    let radios: Vec<Element> = ["Small", "Large"]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let control = radio(cx, state.choice == i, move |s| {
                s.choice = i;
            });
            labeled(cx, control, text)
        })
        .collect();
    let check = checkbox(cx, state.checked, |s| {
        s.checked = !s.checked;
    });
    let toggle = switch(cx, state.on, |s| s.on = !s.on);
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
        labeled(cx, check, "Show hidden files"),
        row(radios).gap(theme.spacing.lg),
        labeled(cx, toggle, "Sync"),
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
