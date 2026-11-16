//! Text flowing around shapes: rich text around a floated flower whose
//! soft glow the text may run into (its alpha above a threshold keeps text
//! out), a ring placed in the middle that text runs on both sides of (and,
//! "through", into its hole), and a card with a button floated left.
//!
//! Drag the ring around: the text reflows live. The chips pick which sides
//! of the ring text runs on; the sliders set the margin text keeps from
//! every shape and the alpha threshold of the flower and the ring. The text
//! is selectable. `--check` renders the window off screen and checks the
//! shapes are drawn, text runs on both sides of the ring, and no text is
//! drawn over the flower.
//!
//! The images are drawn here, pixel by pixel: no assets.

use std::sync::Arc;

use stipple::prelude::*;

const SIDES: [(&str, WrapSide); 5] = [
    ("Both sides", WrapSide::Both),
    ("Largest", WrapSide::Largest),
    ("Left", WrapSide::Left),
    ("Right", WrapSide::Right),
    ("Clear", WrapSide::Clear),
];

/// The flower's petals, opaque.
const PETAL: [u8; 3] = [0xd9, 0x2b, 0xb0];
/// The ring.
const RING: [u8; 3] = [0x1f, 0x8a, 0x70];

struct Flow {
    margin: f64,
    side: usize,
    threshold: f32,
    through: bool,
    likes: u32,
    /// The ring's top-left, in the text's box.
    ring: Point,
    /// The ring's top-left when the drag moving it began, and where that
    /// press was.
    drag: Option<(Point, Point)>,
    flower: Arc<Pixmap>,
    ring_image: Arc<Pixmap>,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            margin: 8.0,
            side: 0,
            threshold: 0.5,
            through: false,
            likes: 3,
            ring: Point::new(300.0, 150.0),
            drag: None,
            flower: Arc::new(flower(240)),
            ring_image: Arc::new(ring(200)),
        }
    }
}

/// A five-petaled flower, opaque, in a glow fading from 60% opacity at its
/// edge to nothing: at a low threshold the glow keeps text away too.
fn flower(side: u32) -> Pixmap {
    let c = side as f64 / 2.0;
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
            let r = (dx * dx + dy * dy).sqrt();
            let a = dy.atan2(dx);
            let edge = c * 0.62 * (1.0 + 0.22 * (5.0 * a).sin().abs() - 0.1);
            let px = if r <= c * 0.16 {
                [0xff, 0xc8, 0x3d, 0xff]
            } else if r <= edge {
                let shade = (1.0 - 0.35 * r / edge) as f32;
                [
                    (PETAL[0] as f32 * shade) as u8,
                    (PETAL[1] as f32 * shade) as u8,
                    (PETAL[2] as f32 * shade + 30.0).min(255.0) as u8,
                    0xff,
                ]
            } else {
                let glow = (1.0 - (r - edge) / (c - edge)).clamp(0.0, 1.0);
                [0xf5, 0x9e, 0xd8, (glow * glow * 0.6 * 255.0) as u8]
            };
            data.extend_from_slice(&px);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(side, side), data)
}

/// A ring: opaque between 55% and 96% of its radius, see-through inside
/// and out.
fn ring(side: u32) -> Pixmap {
    let c = side as f64 / 2.0;
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
            let r = (dx * dx + dy * dy).sqrt() / c;
            // A pixel's worth of soft edge on both sides.
            let px = 1.0 / c;
            let cover = ((r - 0.55) / px).clamp(0.0, 1.0) * ((0.96 - r) / px).clamp(0.0, 1.0);
            data.extend_from_slice(&[RING[0], RING[1], RING[2], (cover * 255.0) as u8]);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(side, side), data)
}

fn article(s: &Flow) -> RichText {
    let accent = Color::rgb(PETAL[0], PETAL[1], PETAL[2]);
    let green = Color::rgb(RING[0], RING[1], RING[2]);
    RichText::new()
        .float(
            FloatImage::new(s.flower.clone())
                .right()
                .width(240.0)
                .alpha(s.threshold)
                .margin(s.margin)
                .alt("A flower. "),
        )
        .styled("Text that flows. ", SpanStyle::new().size(26.0).bold())
        .push("This paragraph is ")
        .bold("rich text")
        .push(" laid out around three objects. The ")
        .styled("flower", SpanStyle::new().color(accent).bold())
        .push(
            " on the right floats in the text itself: it is anchored at the start of \
             the paragraph and keeps text out of its opaque petals only, so lines run \
             into the gaps between them and over its soft glow. Raise the threshold and \
             the text comes closer; lower it and the glow keeps the text away too.\n",
        )
        .push("The ")
        .styled("ring", SpanStyle::new().color(green).bold())
        .push(
            " is placed in the middle of the column, and the text is split around it, \
             filling the room on both sides of it line by line. Drag it anywhere: every \
             line is fitted again around it, and only the lines it touches are measured \
             again. With ",
        )
        .italic("through")
        .push(
            " checked, the words go into its hole wherever the hole is wide enough \
             for them. A line with a ",
        )
        .styled("taller run", SpanStyle::new().size(22.0))
        .push(
            " in it is measured at its own height, so the bigger letters keep clear of the \
             shapes too.\n",
        )
        .push("The ")
        .bold("card")
        .push(
            " on the left is an ordinary element with a button inside, floated left: \
             text runs on its right, a margin away, with the shape of its rounded \
             corners. Everything here is one text, selected, copied and read aloud in \
             its own order, whatever the shapes do to its lines. Mixed scripts work as \
             they do in any paragraph: עברית נכתבת מימין לשמאל בתוך השורה, and the \
             line goes on in English after it.\n",
        )
        .push(
            "Narrow gaps are left empty rather than filled with a word or two, and a \
             word too wide for the room beside a shape moves down to where it fits. \
             The quick brown fox jumps over the lazy dog; pack my box with five dozen \
             liquor jugs; how vexingly quick daft zebras jump.",
        )
}

fn view(s: &Flow, cx: &mut Cx<Flow>) -> Element {
    let theme = *cx.theme();
    let wrap = SIDES[s.side].1;
    let mut ring_item = FlowItem::at(s.ring)
        .shape(FlowShape::alpha(s.ring_image.clone(), s.threshold))
        .margin(s.margin)
        .wrap(wrap);
    if s.through {
        ring_item = ring_item.through();
    }
    let card = column(vec![
        heading(cx, "A floated card"),
        label(cx, format!("{} people like this.", s.likes)),
        button(cx, "Like").on_tap(cx, |s: &mut Flow| s.likes += 1),
    ])
    .gap(8.0)
    .padding(Insets::uniform(12.0))
    .fill(theme.palette.surface)
    .border(theme.palette.border, 1.0)
    .radius(12.0)
    .width(190.0);
    let ring = Element::image(s.ring_image.clone())
        .width(200.0)
        .height(200.0)
        .cursor(CursorIcon::Grab)
        .a11y_label("A ring you can drag")
        .on_drag_motion(cx, |s: &mut Flow, m| {
            let (base, origin) = match s.drag {
                Some((base, origin)) if origin == m.origin => (base, origin),
                _ => (s.ring, m.origin),
            };
            s.drag = Some((base, origin));
            s.ring = Point::new(
                (base.x + m.position.x - origin.x).clamp(0.0, 2000.0),
                (base.y + m.position.y - origin.y).clamp(0.0, 2000.0),
            );
        });
    let body = Element::flow_text(paragraph(cx, article(s)).selectable())
        .flow_child(FlowItem::float_left().margin(s.margin.max(4.0)), card)
        .flow_child(ring_item, ring)
        .flow_options(FlowOptions::default().paragraph_spacing(10.0));
    let controls = column(vec![
        row(vec![
            label(cx, "Text runs on the ring's"),
            choice_chips(
                cx,
                SIDES.map(|(name, _)| name),
                s.side,
                |s: &mut Flow, i| s.side = i,
            ),
            checkbox(cx, "Through", s.through, |s: &mut Flow| {
                s.through = !s.through
            }),
        ])
        .gap(10.0)
        .align(Align::Start, Align::Center),
        row(vec![
            label(cx, format!("Margin {:>2.0} px", s.margin)),
            slider(cx, s.margin / 24.0, |s: &mut Flow, v| {
                s.margin = (v * 24.0).round()
            }),
            label(cx, format!("Alpha threshold {:.2}", s.threshold)),
            slider(cx, s.threshold as f64, |s: &mut Flow, v| {
                s.threshold = ((v * 20.0).round() / 20.0) as f32
            }),
        ])
        .gap(10.0)
        .align(Align::Start, Align::Center),
    ])
    .gap(8.0);
    panel(cx)
        .children(vec![controls, divider(cx), body])
        .padding(Insets::uniform(16.0))
}

fn app() -> App<Flow> {
    App::new(Flow::default(), view)
        .title("Stipple Text Flow")
        .theme(Theme::light())
        .logical_size(Size::new(960.0, 720.0))
}

/// The flower and the ring are drawn; text runs left and right of the ring
/// across its middle; no text is drawn over the flower's petals (a dark
/// pixel with petal on both sides of it on its row).
fn check(pm: &Pixmap) -> Result<(), String> {
    let (w, h) = (pm.size().width, pm.size().height);
    let px = |x: u32, y: u32| pm.pixel(x, y).unwrap_or([255; 4]);
    let near =
        |p: [u8; 4], c: [u8; 3], tol: i32| (0..3).all(|i| (p[i] as i32 - c[i] as i32).abs() <= tol);
    let petal = |p: [u8; 4]| p[0] > 120 && p[2] > 110 && p[1] < 70;
    let ink = |p: [u8; 4]| p[0] < 110 && p[1] < 110 && p[2] < 110;
    let (mut petals, mut rings) = (0, 0);
    let (mut ring_top, mut ring_bottom, mut ring_left, mut ring_right) = (h, 0, w, 0);
    let mut over = 0;
    for y in 0..h {
        let row: Vec<[u8; 4]> = (0..w).map(|x| px(x, y)).collect();
        let first_petal = row.iter().position(|&p| petal(p));
        let last_petal = row.iter().rposition(|&p| petal(p));
        for (x, &p) in row.iter().enumerate() {
            if petal(p) {
                petals += 1;
            }
            if near(p, RING, 12) {
                rings += 1;
                ring_top = ring_top.min(y);
                ring_bottom = ring_bottom.max(y);
                ring_left = ring_left.min(x as u32);
                ring_right = ring_right.max(x as u32);
            }
            if let (Some(a), Some(b)) = (first_petal, last_petal)
                && ink(p)
                && x > a
                && x < b
                && b - a < 260
            {
                over += 1;
            }
        }
    }
    println!("textflow: {petals} petal pixels, {rings} ring pixels, {over} ink pixels on petals");
    if petals < 2000 {
        return Err("the flower isn't drawn".into());
    }
    if rings < 2000 {
        return Err("the ring isn't drawn".into());
    }
    if over > 0 {
        return Err("text is drawn over the flower".into());
    }
    let mid = (ring_top + ring_bottom) / 2;
    let inked =
        |a: u32, b: u32| (mid.saturating_sub(10)..mid + 10).any(|y| (a..b).any(|x| ink(px(x, y))));
    if !inked(ring_left.saturating_sub(120), ring_left) {
        return Err("no text left of the ring".into());
    }
    if !inked(ring_right + 1, (ring_right + 120).min(w)) {
        return Err("no text right of the ring".into());
    }
    Ok(())
}

fn main() {
    let mut app = app();
    if std::env::args().any(|a| a == "--check") {
        match check(&app.render_once()) {
            Ok(()) => println!("textflow: ok"),
            Err(e) => {
                println!("textflow: FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    app.run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_around_the_shapes() {
        let Some(font) = Font::system_default() else {
            return;
        };
        let mut app = app().font(font);
        let pm = app.render_once();
        if let Ok(path) = std::env::var("STIPPLE_TEXTFLOW_PNG") {
            let _ = pm.save_png(path);
        }
        check(&pm).unwrap();
    }
}
