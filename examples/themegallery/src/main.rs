//! Renders the same UI under several themes to show the theme engine and the
//! customization builder (`accent`, `high_contrast`, …).
//!
//! Writes one PNG per theme (`theme0.png` … `themeN.png`) at a fixed size;
//! the CI theme job montages them into one screenshot.

use stipple::prelude::*;

const W: f64 = 360.0;
const H: f64 = 300.0;

/// A representative card: heading, the four button variants, and status swatches.
fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let t = *cx.theme();
    let card = panel(cx)
        .children(vec![
            heading(cx, "Stipple"),
            label(cx, "Theme customization"),
            divider(cx),
            row(vec![
                Button::new("Primary").variant(Variant::Primary).view(cx),
                Button::new("Secondary")
                    .variant(Variant::Secondary)
                    .view(cx),
            ])
            .gap(t.spacing.sm),
            row(vec![
                Button::new("Ghost").variant(Variant::Ghost).view(cx),
                Button::new("Danger").variant(Variant::Danger).view(cx),
            ])
            .gap(t.spacing.sm),
            row(vec![
                swatch(t.palette.success, 28.0, t.radius / 2.0),
                swatch(t.palette.warning, 28.0, t.radius / 2.0),
                swatch(t.palette.danger, 28.0, t.radius / 2.0),
            ])
            .gap(t.spacing.sm),
        ])
        .width(320.0);
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
    let themes: [(&str, Theme); 4] = [
        ("light", Theme::light()),
        ("dark", Theme::dark()),
        (
            "violet",
            Theme::dark()
                .accent(Color::rgb(0x8b, 0x5c, 0xf6))
                .radius(14.0),
        ),
        ("high-contrast", Theme::light().high_contrast()),
    ];
    for (i, (name, theme)) in themes.iter().enumerate() {
        let pixmap = render(*theme);
        pixmap
            .save_png(format!("theme{i}.png"))
            .expect("write the PNG");
        println!("rendered theme {i} ({name})");
    }
    println!("size {}x{}", W as u32, H as u32);
}
