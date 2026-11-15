//! Two side-by-side buttons that lighten on hover.
//!
//! Used by the CI X11 job to verify pointer hover: the runner moves the cursor
//! onto the right button with `xdotool` and screenshots — the hovered button is
//! visibly lighter than the other.
//!
//! Fixed positions (no centering) so the cursor target is predictable: with the
//! column's 24px padding and a 20px gap, the left button spans x∈[24,184] and
//! the right x∈[204,364], both at y∈[24,114].

use stipple::prelude::*;

fn hover_button(cx: &mut Cx<()>, label: &str) -> Element {
    button(cx, label)
        .width(160.0)
        .height(90.0)
        .on_tap(cx, |_| {})
}

fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let row = row(vec![hover_button(cx, "Left"), hover_button(cx, "Right")]).gap(20.0);
    column(vec![row])
        .padding(Insets::uniform(24.0))
        .align(Align::Start, Align::Start)
}

fn main() {
    let app = App::new((), view)
        .title("Stipple Hover")
        .theme(Theme::dark())
        .logical_size(Size::new(420.0, 160.0));
    app.run();
}
