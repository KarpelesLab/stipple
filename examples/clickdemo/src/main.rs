//! The whole window is a button that counts clicks.
//!
//! Used by the CI X11 job to verify *real* native input end-to-end: the runner
//! launches this under Xvfb, screenshots ("Clicks: 0"), uses `xdotool` to click
//! the window, and screenshots again ("Clicks: 1") — proving the
//! native-event → hit-test → dispatch → re-present loop works on a real server.

use stipple::prelude::*;

struct Clicks {
    n: u32,
}

fn view(state: &Clicks, cx: &mut Cx<Clicks>) -> Element {
    let theme = *cx.theme();
    // The entire window is one big primary button showing the count.
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(
            format!("Clicks: {}", state.n),
            64.0,
            theme.palette.on_primary,
        )],
    )
    .fill(theme.palette.primary)
    .align(Align::Center, Align::Center)
    .on_tap(cx, |s| s.n += 1)
}

fn main() {
    let app = App::new(Clicks { n: 0 }, view)
        .title("Stipple Clicks")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 480.0));
    app.run();
}
