//! Opens a real native window and runs the Stipple event loop.
//!
//! On Linux with `$DISPLAY` set this uses the native X11 backend
//! (`stipple::platform::backend::x11`); otherwise it falls back to a one-shot
//! headless present. Used by the visual-test CI job: the runner starts an Xvfb
//! display, launches this binary, and screenshots the root window to confirm
//! the X11 backend actually paints.

use stipple::prelude::*;

struct Demo;

fn view(_state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let card = panel(
        &theme,
        vec![
            label(&theme, "Welcome to Stipple"),
            // A word-wrapping paragraph: long text flows to the panel width.
            paragraph(
                &theme,
                "Stipple is a self-drawn cross-platform UI toolkit that wraps this \
                 paragraph across multiple lines to fit the panel width.",
            ),
            divider(&theme),
            setting_row(&theme, Color::rgb(0xef, 0x68, 0x68), "Edit"),
            setting_row(&theme, Color::rgb(0x34, 0xd3, 0x99), "Edit"),
            setting_row(&theme, Color::rgb(0xf5, 0x9e, 0x0b), "Edit"),
            button_labeled(&theme, "OK"),
        ],
    )
    .width(360.0);
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

fn main() {
    let mut app = App::new(Demo, view)
        .title("Stipple")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 480.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
