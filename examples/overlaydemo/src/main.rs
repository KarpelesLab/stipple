//! Overlay layers: a dropdown menu and a modal dialog drawn above the main tree.
//!
//! Two fixed-position buttons open a dropdown (non-modal, dismiss on outside
//! press) and a modal dialog (dark scrim, dismiss on scrim press). The CI X11
//! interaction job clicks the buttons and screenshots: the dropdown appears over
//! the content, and the modal darkens the whole frame (the scrim).

use stipple::prelude::*;

const W: f64 = 480.0;
const H: f64 = 420.0;

#[derive(Default)]
struct State {
    menu_open: bool,
    dialog_open: bool,
    picked: String,
}

fn btn<S>(cx: &mut Cx<S>, text: &str, on_tap: impl FnMut(&mut S) + 'static) -> Element {
    button(cx, text)
        .on_tap(cx, on_tap)
        .width(120.0)
        .height(40.0)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let card = panel(cx, |cx| {
        vec![
            row(vec![
                btn(cx, "Menu", |s| s.menu_open = true),
                btn(cx, "Dialog", |s| s.dialog_open = true),
            ])
            .gap(theme.spacing.md),
            divider(cx),
            label(
                cx,
                if state.picked.is_empty() {
                    "Picked: (none)".to_string()
                } else {
                    format!("Picked: {}", state.picked)
                },
            ),
        ]
    })
    .width(420.0);

    // Dropdown anchored just below the Menu button (fixed point for the demo).
    if state.menu_open {
        let mut menu = Menu::new("Color");
        for color in ["Red", "Green", "Blue"] {
            menu = menu.item(MenuItem::new(color, move |s: &mut State| {
                s.picked = color.into()
            }));
        }
        open_menu(cx, Point::new(36.0, 84.0), menu, |s| s.menu_open = false);
    }

    // Modal confirmation dialog.
    if state.dialog_open {
        let body = label(cx, "Apply changes?");
        let actions = vec![
            button(cx, "Cancel")
                .on_tap(cx, |s| s.dialog_open = false)
                .width(96.0)
                .height(36.0),
            button(cx, "OK")
                .on_tap(cx, |s| s.dialog_open = false)
                .width(96.0)
                .height(36.0),
        ];
        open_dialog(cx, "Confirm", body, actions, |s| s.dialog_open = false);
    }

    column(vec![card])
        .align(Align::Start, Align::Start)
        .padding(Insets::uniform(20.0))
        .grow(1.0)
}

fn main() {
    let app = App::new(State::default(), view)
        .title("Stipple Overlays")
        .theme(Theme::dark())
        .logical_size(Size::new(W, H));
    app.run();
}
