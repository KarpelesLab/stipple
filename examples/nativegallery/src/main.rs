//! Every control under [`Theme::native`]: on Windows, macOS and iOS the OS draws
//! them, and on Linux they stay self-drawn in the system's colors. CI screenshots
//! this on each platform.

use stipple::prelude::*;

struct Gallery {
    checked: bool,
    choice: usize,
    on: bool,
    level: f64,
    name: EditBuffer,
}

fn labeled(theme: &Theme, control: Element, text: &str) -> Element {
    row(vec![control, label(theme, text)])
        .gap(theme.spacing.sm)
        .align(Align::Start, Align::Center)
}

fn view(state: &Gallery, cx: &mut Cx<Gallery>) -> Element {
    let theme = *cx.theme();
    let radios: Vec<Element> = ["Small", "Large"]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let control = radio(cx, &theme, state.choice == i, move |s: &mut Gallery| {
                s.choice = i;
            });
            labeled(&theme, control, text)
        })
        .collect();
    let check = checkbox(cx, &theme, state.checked, |s: &mut Gallery| {
        s.checked = !s.checked;
    });
    let toggle = switch(cx, &theme, state.on, |s: &mut Gallery| s.on = !s.on);
    let level = slider(cx, &theme, state.level, |s: &mut Gallery, v| s.level = v);
    let name = text_editor(cx, &theme, &state.name, |s: &mut Gallery| &mut s.name);
    column(vec![
        heading(&theme, "Native controls"),
        name,
        row(vec![
            button_labeled(&theme, "Save"),
            button_variant(&theme, "Cancel", Variant::Secondary),
        ])
        .gap(theme.spacing.sm),
        labeled(&theme, check, "Show hidden files"),
        row(radios).gap(theme.spacing.lg),
        labeled(&theme, toggle, "Sync"),
        level,
        progress_bar(&theme, state.level),
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
        name: EditBuffer::from_text("Stipple"),
    };
    let mut app = App::new(state, view)
        .title("Stipple")
        .theme(Theme::native())
        .logical_size(Size::new(360.0, 340.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
