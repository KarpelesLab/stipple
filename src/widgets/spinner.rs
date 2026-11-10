//! The busy spinner: an arc turning around a faint ring, driven by the frame
//! clock.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::{BoxStyle, Cx, Element};
use crate::render::{Color, Svg};
use crate::style::Theme;

/// The spinner's diameter, in logical pixels.
const SIZE: f64 = 22.0;
/// Ring stroke width.
const STROKE: f64 = 3.0;
/// One turn takes this long.
const PERIOD_SECS: f64 = 1.0;
/// Distinct positions per turn: one per frame at 60 Hz.
const STEPS: u32 = 60;
/// How much of the ring the arc covers, in degrees.
const SWEEP: f64 = 100.0;

/// A busy indicator: an arc in the theme's primary color, turning once a second
/// around a faint ring. It asks for a frame each time it's built, so it turns
/// at the display's refresh rate for as long as it's on screen, and the app
/// goes back to idle once it's gone. Under
/// [reduced motion](crate::anim::reduced_motion) it turns at half the speed.
pub fn spinner<S>(cx: &mut Cx<S>, theme: &Theme) -> Element {
    cx.request_frame();
    // It shows that something is happening, so it keeps turning under
    // reduced motion, at half the speed (as Bootstrap's spinners do).
    let period = if crate::anim::reduced_motion() {
        PERIOD_SECS * 2.0
    } else {
        PERIOD_SECS
    };
    let turns = cx.frame_time().as_secs_f64() / period;
    let step = ((turns.fract() * STEPS as f64) as u32).min(STEPS - 1);
    match frame(theme.palette.primary, step) {
        Some(doc) => Element::image(&doc).width(SIZE).height(SIZE),
        // Not expected: the document is ours. Fall back to the plain ring.
        None => Element::boxed(BoxStyle {
            fill: None,
            radius: SIZE / 2.0,
            border: Some((theme.palette.primary, STROKE)),
        })
        .width(SIZE)
        .height(SIZE),
    }
}

/// The spinner at `step` of [`STEPS`], drawn in `color`. Each document is built
/// once and kept, so a frame that lands on an already-seen step reuses it (and
/// its raster), and an unchanged step compares equal for damage.
fn frame(color: Color, step: u32) -> Option<Arc<Svg>> {
    thread_local! {
        static FRAMES: RefCell<HashMap<(Color, u32), Arc<Svg>>> = RefCell::new(HashMap::new());
    }
    FRAMES.with(|frames| {
        if let Some(doc) = frames.borrow().get(&(color, step)) {
            return Some(doc.clone());
        }
        let doc = Arc::new(Svg::parse(document(color, step).as_bytes()).ok()?);
        let mut frames = frames.borrow_mut();
        // Bounded by STEPS per color; drop everything if themes keep changing.
        if frames.len() > STEPS as usize * 8 {
            frames.clear();
        }
        frames.insert((color, step), doc.clone());
        Some(doc)
    })
}

/// The SVG source for one step: the track ring, and the arc starting at the
/// step's angle (clockwise from 12 o'clock).
fn document(color: Color, step: u32) -> String {
    let c = SIZE / 2.0;
    let r = c - STROKE / 2.0;
    let start = step as f64 * 360.0 / STEPS as f64;
    let point = |deg: f64| {
        let a = (deg - 90.0).to_radians();
        (c + r * a.cos(), c + r * a.sin())
    };
    let (x0, y0) = point(start);
    let (x1, y1) = point(start + SWEEP);
    let rgb = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
    let alpha = color.a as f64 / 255.0;
    let track = alpha * 0.25;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{SIZE}\" height=\"{SIZE}\" \
         viewBox=\"0 0 {SIZE} {SIZE}\">\
         <circle cx=\"{c}\" cy=\"{c}\" r=\"{r}\" fill=\"none\" stroke=\"{rgb}\" \
         stroke-opacity=\"{track:.3}\" stroke-width=\"{STROKE}\"/>\
         <path d=\"M {x0:.3} {y0:.3} A {r} {r} 0 0 1 {x1:.3} {y1:.3}\" fill=\"none\" \
         stroke=\"{rgb}\" stroke-opacity=\"{alpha:.3}\" stroke-width=\"{STROKE}\" \
         stroke-linecap=\"round\"/></svg>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;
    use crate::render::ImageFit;

    /// The angle (degrees clockwise from 12 o'clock) of the brightest opaque
    /// pixels' centroid: where the arc is.
    fn arc_angle(step: u32) -> f64 {
        let color = Color::rgb(0x20, 0x60, 0xe0);
        let doc = frame(color, step).expect("the spinner document parses");
        let px = doc.rasterize(PhysicalSize::new(44, 44), ImageFit::Contain);
        let bytes = px.as_bytes();
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for y in 0..44 {
            for x in 0..44 {
                if bytes[(y * 44 + x) * 4 + 3] > 200 {
                    sx += x as f64 - 22.0;
                    sy += y as f64 - 22.0;
                    n += 1.0;
                }
            }
        }
        assert!(n > 20.0, "the arc is drawn opaque ({n} px)");
        (sx.atan2(-sy).to_degrees() + 360.0) % 360.0
    }

    #[test]
    fn the_arc_turns_with_the_step() {
        // The arc spans SWEEP degrees from its start, so its centroid sits at
        // start + SWEEP / 2.
        for step in [0, 15, 30, 45] {
            let want = (step as f64 * 360.0 / STEPS as f64 + SWEEP / 2.0) % 360.0;
            let got = arc_angle(step);
            let diff = ((got - want + 540.0) % 360.0 - 180.0).abs();
            assert!(
                diff < 15.0,
                "step {step}: arc at {got:.0}°, want {want:.0}°"
            );
        }
    }

    #[test]
    fn spinner_asks_for_frames_and_reuses_documents() {
        let theme = Theme::light();
        let mut cx = Cx::<()>::new(&theme);
        assert!(!cx.frame_requested());
        let a = spinner(&mut cx, &theme);
        assert!(cx.frame_requested(), "a spinner keeps frames coming");
        let b = spinner(&mut cx, &theme);
        assert_eq!(a, b, "the same frame time draws the same document");
    }
}
