//! Embedded-content viewport: reserve a rectangle in the Stipple UI and composite
//! externally-rendered pixels into it.
//!
//! This is the toolkit-side seam of the **Stipple-as-compositor** model
//! (ROADMAP.md, browser viewport): a sandboxed content process renders a page
//! into a GPU texture and hands it to the UI process (a `dma-buf` on Linux),
//! which imports it as a texture and composites it into a viewport element so
//! the chrome draws around it. Here a CPU-generated checkerboard stands in for
//! that content, proving the compositing path end to end on the software
//! backend — no GPU required, so it runs under Xvfb in CI.
//!
//! The `visual-viewport` CI job screenshots the window and asserts the viewport
//! region shows the content (cyan/magenta), not the dark placeholder — i.e. the
//! registered content was blitted into the reserved rect.

use stipple::prelude::*;

/// The embedded content's pixel size (physical, == logical at 1× scale).
const VW: u32 = 320;
const VH: u32 = 240;

/// The viewport's stable id, correlating the reserved rect with its content.
const PAGE: ViewportId = ViewportId(1);

/// Stand-in "rendered page": a 32px magenta/cyan checkerboard. A real content
/// process would hand over a GPU texture instead; the compositing seam is the
/// same.
fn content() -> Pixmap {
    let mut pm = Pixmap::new(PhysicalSize::new(VW, VH));
    let stride = pm.stride();
    let bytes = pm.as_bytes_mut();
    for y in 0..VH as usize {
        for x in 0..VW as usize {
            let cell = ((x / 32) + (y / 32)) % 2 == 0;
            let px = if cell {
                [0xff, 0x00, 0xff, 0xff] // magenta
            } else {
                [0x00, 0xff, 0xff, 0xff] // cyan
            };
            let o = y * stride + x * 4;
            bytes[o..o + 4].copy_from_slice(&px);
        }
    }
    pm
}

fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let theme = *cx.theme();
    let card = panel(
        &theme,
        vec![
            heading(&theme, "Embedded content"),
            // The viewport: the app blits the registered content over this rect.
            viewport(&theme, PAGE, VW as f64, VH as f64),
        ],
    );
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

/// Build the demo's app, off-screen-renderable for tests/screenshots.
fn demo_app() -> App<()> {
    App::new((), view)
        .title("Stipple — viewport")
        .theme(Theme::dark())
        .logical_size(Size::new(480.0, 360.0))
        .viewport_content(PAGE, content())
        // Input landing in the viewport forwards here with viewport-local
        // coordinates — what a real build hands to the content process. We log
        // it (the CI job clicks the viewport and greps for the forwarded press).
        .on_viewport_input(|id, ev| println!("viewport input: {id:?} {ev:?}"))
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

    /// The composited checkerboard must land in an on-screen region that fully
    /// contains the rectangle the `visual-viewport` CI job crops (200×150 at
    /// (140,105)), so the screenshot assertion can't silently drift off the
    /// viewport. Needs a system font (the heading's height shifts the centered
    /// layout); skipped when none is available.
    #[test]
    fn composited_content_contains_the_ci_crop_region() {
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let pm = demo_app().font(font).render_once();
        let (w, h) = (pm.size().width, pm.size().height);

        // Bounding box of the content pixels (cyan/magenta both have B=255 with
        // one of R/G zero — a signature no theme/placeholder color shares).
        let is_content = |p: [u8; 4]| p[2] == 255 && (p[0] == 0 || p[1] == 0);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..h {
            for x in 0..w {
                if pm.pixel(x, y).is_some_and(is_content) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        assert!(x0 != u32::MAX, "no composited content found on screen");
        // The CI crop rectangle, which must sit entirely within the content box.
        let (cx0, cy0, cx1, cy1) = (140, 105, 140 + 200, 105 + 150);
        assert!(
            x0 <= cx0 && y0 <= cy0 && x1 >= cx1 && y1 >= cy1,
            "content bbox ({x0},{y0})-({x1},{y1}) must contain CI crop \
             ({cx0},{cy0})-({cx1},{cy1})"
        );
    }
}
