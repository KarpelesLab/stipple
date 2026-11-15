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
    let card = panel(
        &t,
        vec![
            heading(&t, "Material 3"),
            label(&t, "Dynamic color from a seed"),
            divider(&t),
            row(vec![
                button_variant(&t, "Filled", Variant::Primary),
                button_variant(&t, "Outlined", Variant::Secondary),
            ])
            .gap(t.spacing.sm),
            row(vec![
                button_variant(&t, "Text", Variant::Ghost),
                button_variant(&t, "Error", Variant::Danger),
            ])
            .gap(t.spacing.sm),
            row(vec![
                swatch(t.palette.primary, 28.0, t.radius / 2.0),
                swatch(t.palette.success, 28.0, t.radius / 2.0),
                swatch(t.palette.warning, 28.0, t.radius / 2.0),
                swatch(t.palette.danger, 28.0, t.radius / 2.0),
            ])
            .gap(t.spacing.sm),
        ],
    )
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

fn main() {
    let schemes: [(&str, Theme); 4] = [
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
    ];
    for (i, (name, theme)) in schemes.iter().enumerate() {
        let pixmap = render(*theme);
        pixmap
            .save_png(format!("theme{i}.png"))
            .expect("write the PNG");
        println!("rendered scheme {i} ({name})");
    }
    println!("size {}x{}", W as u32, H as u32);
}
