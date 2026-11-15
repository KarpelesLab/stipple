//! A tall list inside a fixed-height [`scroll`] viewport: the content overflows
//! the viewport and is clipped to it; the mouse wheel scrolls it.
//!
//! CI (X11/Xvfb) screenshots the initial frame (only the first items visible,
//! clipped), then `xdotool` wheel-scrolls and screenshots again to confirm the
//! content moved and is still clipped to the viewport.

use stipple::prelude::*;

const W: f64 = 420.0;
const H: f64 = 360.0;

// A distinct color per row so the screenshot diff before/after scroll is obvious.
fn row_color(i: usize) -> Color {
    const PALETTE: [(u8, u8, u8); 6] = [
        (0xef, 0x68, 0x68),
        (0xf5, 0x9e, 0x0b),
        (0x34, 0xd3, 0x99),
        (0x60, 0x9c, 0xff),
        (0xa7, 0x8b, 0xfa),
        (0xf4, 0x72, 0xb6),
    ];
    let (r, g, b) = PALETTE[i % PALETTE.len()];
    Color::rgb(r, g, b)
}

fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let theme = *cx.theme();
    // 24 rows — far taller than the 240px viewport, so it must scroll + clip.
    let items: Vec<Element> = (0..24)
        .map(|i| {
            row(vec![
                swatch(row_color(i), 22.0, 4.0),
                label(cx, format!("Item {i:02}")),
            ])
            .gap(theme.spacing.md)
            .padding(Insets::uniform(theme.spacing.sm))
            .align(
                stipple::prelude::Align::Start,
                stipple::prelude::Align::Center,
            )
        })
        .collect();
    let content = column(items).gap(theme.spacing.sm);
    let viewport = scroll(cx).height(240.0).child(content).width(340.0);

    let card = panel(cx)
        .children(vec![label(cx, "Scrollable list"), divider(cx), viewport])
        .width(360.0);
    column(vec![card]).grow(1.0).align(
        stipple::prelude::Align::Center,
        stipple::prelude::Align::Center,
    )
}

fn main() {
    let app = App::new((), view)
        .title("Stipple Scroll")
        .theme(Theme::dark())
        .logical_size(Size::new(W, H));
    app.run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
