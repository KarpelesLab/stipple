//! A **Material 3 (Material You)** theme rendered by Stipple's self-drawn widgets.
//!
//! Shows `Theme::material3_*`: the baseline light/dark schemes (the default
//! `#6750A4` seed) plus two **dynamic-color** schemes generated from arbitrary
//! seed colors — the "Material You" idea that one brand/wallpaper color recolors
//! the whole UI. Each scheme is built from tonal palettes (tones are CIELAB
//! L*) and maps onto Stipple's existing tokens, so the standard widgets theme
//! correctly with no changes.
//!
//! Writes one PNG per scheme (`theme0.png` … `theme3.png`) at a fixed size;
//! the CI Material 3 job montages them.

use stipple::prelude::*;

const W: f64 = 360.0;
const H: f64 = 320.0;

/// A representative Material 3 card: heading, supporting text, the button
/// variants (filled / outlined / text / error), and tonal status swatches.
fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let t = *cx.theme();
    let card = panel(cx)
        .children(vec![
            heading(cx, "Material 3"),
            label(cx, "Dynamic color from a seed"),
            divider(cx),
            row(vec![
                Button::new("Filled").variant(Variant::Primary).view(cx),
                Button::new("Outlined").variant(Variant::Secondary).view(cx),
            ])
            .gap(t.spacing.sm),
            row(vec![
                Button::new("Text").variant(Variant::Ghost).view(cx),
                Button::new("Error").variant(Variant::Danger).view(cx),
            ])
            .gap(t.spacing.sm),
            row(vec![
                swatch(t.palette.primary, 28.0, t.radius / 2.0),
                swatch(t.palette.success, 28.0, t.radius / 2.0),
                swatch(t.palette.warning, 28.0, t.radius / 2.0),
                swatch(t.palette.danger, 28.0, t.radius / 2.0),
            ])
            .gap(t.spacing.sm),
        ])
        .width(324.0);
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

fn render(theme: Theme) -> stipple::render::Pixmap {
    let mut app = App::new((), view)
        .theme(theme)
        .logical_size(Size::new(W, H));
    app.render_once()
}

/// The themes shown, by name.
fn schemes() -> [(&'static str, Theme); 4] {
    [
        ("m3-light", Theme::material3_light()),
        ("m3-dark", Theme::material3_dark()),
        // Material You: recolor the whole scheme from a single seed.
        (
            "seed-teal",
            Theme::material3_from_seed(Color::rgb(0x00, 0x69, 0x6b), false),
        ),
        (
            "seed-orange",
            Theme::material3_from_seed(Color::rgb(0xb3, 0x5a, 0x00), true),
        ),
    ]
}

fn main() {
    if cfg!(target_arch = "wasm32") {
        // In the browser: no files to write, so show them all at once.
        return web_app().run();
    }
    let schemes = schemes();
    for (i, (name, theme)) in schemes.iter().enumerate() {
        let pixmap = render(*theme);
        pixmap
            .save_png(format!("theme{i}.png"))
            .expect("write the PNG");
        println!("rendered scheme {i} ({name})");
    }
    println!("size {}x{}", W as u32, H as u32);
}

/// In the browser (the site's live examples): every scheme at once, each card
/// under its own theme, wrapping to the canvas's width.
fn web_app() -> App<()> {
    App::new((), |state: &(), cx: &mut Cx<()>| {
        let cards = schemes()
            .into_iter()
            .map(|(name, theme)| {
                cx.themed(theme, |cx| {
                    column(vec![view(state, cx), label(cx, name)])
                        .align(Align::Center, Align::Center)
                        .gap(theme.spacing.sm)
                        .padding(Insets::uniform(theme.spacing.md))
                        .width(W)
                        .height(H)
                        .fill(theme.palette.background)
                })
            })
            .collect();
        row(cards)
            .wrap()
            .grow(1.0)
            .align(Align::Center, Align::Center)
    })
    .theme(Theme::dark())
    .title("Material 3")
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
