//! Golden-pixel conformance tests.
//!
//! Renders deterministic, **font-free** views off-screen and asserts exact
//! pixel values at sampled coordinates. Because no font is involved, the output
//! is identical on every platform — this is the cross-platform "pixel-identical"
//! guarantee from `ROADMAP.md` §Phase 2, enforced in CI via the headless path.
//!
//! When native backends land, the same views rendered through their surfaces
//! must match these samples.

use stipple::prelude::*;

/// A nested layout with known geometry: an outer panel filled `surface`,
/// containing a centered 40×40 `primary` box.
fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let theme = *cx.theme();
    let inner = Element::boxed(BoxStyle::default())
        .fill(theme.palette.primary)
        .width(40.0)
        .height(40.0);
    Element::stack(Axis::Vertical, vec![inner])
        .fill(theme.palette.surface)
        .align(Align::Center, Align::Center)
}

#[test]
fn samples_are_pixel_stable() {
    let mut app = App::new((), view)
        .theme(Theme::light())
        .logical_size(Size::new(100.0, 100.0))
        .scale(ScaleFactor::IDENTITY);
    let frame = app.render_once();

    assert_eq!(frame.size(), stipple::geometry::PhysicalSize::new(100, 100));

    let theme = Theme::light();
    let bg = theme.palette.background;
    let surface = theme.palette.surface;
    let primary = theme.palette.primary;

    // Corner: window background (no root fill reaches the corner — the panel
    // fills the whole window here, so the corner is the surface color).
    assert_eq!(
        frame.pixel(1, 1),
        Some([surface.r, surface.g, surface.b, 255])
    );
    // Center: the 40×40 primary box (centered in 100×100).
    assert_eq!(
        frame.pixel(50, 50),
        Some([primary.r, primary.g, primary.b, 255])
    );
    // Just outside the centered box (box spans 30..70): at x=20 it's surface.
    assert_eq!(
        frame.pixel(20, 50),
        Some([surface.r, surface.g, surface.b, 255])
    );

    // `bg` only shows if the root doesn't fill; keep it referenced so the
    // intent (background vs surface distinction) is documented.
    let _ = bg;
}

#[test]
fn theme_swap_changes_samples() {
    let mk = |theme: Theme| {
        App::new((), view)
            .theme(theme)
            .logical_size(Size::new(40.0, 40.0))
            .render_once()
    };
    let light = mk(Theme::light());
    let dark = mk(Theme::dark());
    // The center primary box differs between the two themes.
    assert_ne!(light.pixel(20, 20), dark.pixel(20, 20));
}
