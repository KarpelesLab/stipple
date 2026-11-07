//! Images: a PNG decoded with `Pixmap::decode` and an SVG parsed with
//! `Svg::parse`, shown with the `image` / `svg` widgets.
//!
//! The PNG (64×64) is drawn at 2× its natural size, so it scales up; the SVG
//! (also 64×64) is rasterized at whatever physical size it's shown at, so it
//! stays sharp. The bottom row puts the SVG in a wide box with each
//! [`ImageFit`] mode.
//!
//! The decoded assets live in the app state behind `Arc`s: images compare by
//! identity, so building the view from the same `Arc` each frame is a no-op
//! repaint.
//!
//! The `visual-image` CI job screenshots the window and asserts both image
//! areas (fixed positions at the top-left, independent of the font) have
//! content rather than the flat background.

use std::sync::Arc;
use stipple::prelude::*;

const SUN_PNG: &[u8] = include_bytes!("assets/sun.png");
const LOGO_SVG: &[u8] = include_bytes!("assets/logo.svg");

struct Assets {
    sun: Arc<Pixmap>,
    logo: Arc<Svg>,
}

fn load() -> Assets {
    Assets {
        sun: Arc::new(Pixmap::decode(SUN_PNG).expect("bundled PNG decodes")),
        logo: Arc::new(Svg::parse(LOGO_SVG).expect("bundled SVG parses")),
    }
}

/// A fitted box: the SVG in a wide frame, placed per `fit`.
fn fitted(theme: &Theme, logo: &Arc<Svg>, fit: ImageFit) -> Element {
    svg(logo)
        .fit(fit)
        .width(160.0)
        .height(64.0)
        .border(theme.palette.border, 1.0)
        .clip()
}

fn view(a: &Assets, cx: &mut Cx<Assets>) -> Element {
    let theme = *cx.theme();
    column(vec![
        // Images first, at fixed offsets, so the CI crops don't depend on text.
        row(vec![
            // 64×64 PNG shown at 128×128 (height follows the aspect ratio).
            image(&a.sun).width(128.0),
            // 64×64 SVG at 128×128: re-rasterized at that size, so it's crisp.
            svg(&a.logo).width(128.0).height(128.0),
            // The SVG at its natural size.
            svg(&a.logo),
        ])
        .gap(24.0),
        row(vec![
            fitted(&theme, &a.logo, ImageFit::Contain),
            fitted(&theme, &a.logo, ImageFit::Cover),
            fitted(&theme, &a.logo, ImageFit::Fill),
        ])
        .gap(24.0),
        label(&theme, "PNG at 2x · SVG at 2x · SVG at 1x"),
        label(&theme, "SVG fit: contain · cover · fill"),
    ])
    .gap(24.0)
    .padding(Insets::uniform(24.0))
    .grow(1.0)
}

fn demo_app() -> App<Assets> {
    App::new(load(), view)
        .title("Stipple — images")
        .theme(Theme::dark())
        .logical_size(Size::new(580.0, 340.0))
}

fn main() {
    let mut app = demo_app();
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distinct colors in a `w`×`h` region at `(x, y)` of `pm`.
    fn colors(pm: &Pixmap, x: u32, y: u32, w: u32, h: u32) -> usize {
        let mut seen = std::collections::HashSet::new();
        for yy in y..y + h {
            for xx in x..x + w {
                seen.insert(pm.pixel(xx, yy).unwrap());
            }
        }
        seen.len()
    }

    /// The regions the `visual-image` CI job crops (the PNG at 40,40 and the
    /// SVG at 192,40, each 96×96) must hold image content, not flat background.
    #[test]
    fn ci_crop_regions_show_the_images() {
        let pm = demo_app().render_once();
        assert!(colors(&pm, 40, 40, 96, 96) > 50, "PNG region is flat");
        assert!(colors(&pm, 192, 40, 96, 96) > 4, "SVG region is flat");
        // The gap between them is plain background.
        assert_eq!(colors(&pm, 154, 40, 20, 96), 1);
    }
}
