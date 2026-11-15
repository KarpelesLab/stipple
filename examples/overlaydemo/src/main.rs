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

fn btn<S>(
    theme: &Theme,
    cx: &mut Cx<S>,
    text: &str,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    button_labeled(theme, text)
        .on_tap(cx, on_tap)
        .width(120.0)
        .height(40.0)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();

    let menu_btn = btn(&theme, cx, "Menu", |s: &mut State| s.menu_open = true);
    let dialog_btn = btn(&theme, cx, "Dialog", |s: &mut State| s.dialog_open = true);
    let picked = label(
        &theme,
        if state.picked.is_empty() {
            "Picked: (none)".to_string()
        } else {
            format!("Picked: {}", state.picked)
        },
    );

    let card = panel(
        &theme,
        vec![
            row(vec![menu_btn, dialog_btn]).gap(theme.spacing.md),
            divider(&theme),
            picked,
        ],
    )
    .width(420.0);

    // Dropdown anchored just below the Menu button (fixed point for the demo).
    if state.menu_open {
        let items = vec![
            menu_item(cx, &theme, "Red", |s: &mut State| {
                s.picked = "Red".into();
                s.menu_open = false;
            }),
            menu_item(cx, &theme, "Green", |s: &mut State| {
                s.picked = "Green".into();
                s.menu_open = false;
            }),
            menu_item(cx, &theme, "Blue", |s: &mut State| {
                s.picked = "Blue".into();
                s.menu_open = false;
            }),
        ];
        open_menu(
            cx,
            &theme,
            Point::new(36.0, 84.0),
            items,
            |s: &mut State| s.menu_open = false,
        );
    }

    // Modal confirmation dialog.
    if state.dialog_open {
        let body = label(&theme, "Apply changes?");
        let actions = vec![
            button_labeled(&theme, "Cancel")
                .on_tap(cx, |s: &mut State| s.dialog_open = false)
                .width(96.0)
                .height(36.0),
            button_labeled(&theme, "OK")
                .on_tap(cx, |s: &mut State| s.dialog_open = false)
                .width(96.0)
                .height(36.0),
        ];
        open_dialog(cx, &theme, "Confirm", body, actions, |s: &mut State| {
            s.dialog_open = false
        });
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
