//! Text flowing around elements (`Element::flow_text`) through the `App`:
//! where the children go, that no text is drawn over them, that the text
//! selects and copies across the pieces of a line split around a shape, and
//! that screen readers get it whole and in order.

use crate::App;
use crate::core::{BoxStyle, Cx, Element, KeyInput, clipboard_text, set_clipboard_text};
use crate::geometry::{Point, Rect, Size};
use crate::platform::{ButtonState, Event, PointerButton};
use crate::render::{Color, FlowItem, FlowShape, Font, Pixmap};
use crate::widgets::paragraph;

const TEXT: &str = "Text flows around the card on the right and the disc in the \
    middle, line by line, on both sides of the disc and only on the left of the \
    card, keeping a margin from each. The quick brown fox jumps over the lazy dog, \
    again and again, until the lines run past both shapes and take the whole width \
    of the window once more. Pack my box with five dozen liquor jugs. How vexingly \
    quick daft zebras jump; sphinx of black quartz, judge my vow. The five boxing \
    wizards jump quickly, and a wizard's job is to vex chumps quickly in fog. Text \
    keeps flowing for a few more lines so that it reaches well below the disc, where \
    it takes the whole width of the window again, as the last lines show.";

const GREEN: Color = Color::rgb(0, 200, 0);
const BLUE: Color = Color::rgb(0, 0, 230);
const CARD: Rect = Rect::from_xywh(380.0, 0.0, 120.0, 80.0);
const DISC: Rect = Rect::from_xywh(190.0, 90.0, 100.0, 100.0);

fn app(font: Font) -> App<()> {
    App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::flow_text(paragraph(cx, TEXT).selectable())
            .flow_child(
                FlowItem::float_right().margin(8.0),
                Element::boxed(BoxStyle::filled(GREEN).radius(10.0))
                    .width(CARD.width())
                    .height(CARD.height()),
            )
            .flow_child(
                FlowItem::at(DISC.origin)
                    .shape(FlowShape::Ellipse)
                    .margin(6.0),
                Element::boxed(BoxStyle::filled(BLUE).radius(50.0))
                    .width(DISC.width())
                    .height(DISC.height()),
            )
    })
    .font(font)
    .logical_size(Size::new(500.0, 400.0))
}

fn font() -> Option<Font> {
    let f = Font::system_default();
    if f.is_none() {
        eprintln!("skipping: no system font");
    }
    f
}

/// Whether `p` is text ink: darker than the light theme's background and
/// than the two shapes.
fn ink(p: [u8; 4]) -> bool {
    p[0] < 140 && p[1] < 140 && p[2] < 140
}

#[test]
fn children_float_and_take_no_text() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let pm: Pixmap = app.render_once();
    if let Ok(dir) = std::env::var("STIPPLE_FLOW_PNG") {
        let _ = pm.save_png(format!("{dir}/flow-app.png"));
    }
    // The card is drawn against the right edge, the disc where it was put.
    let at = |x: f64, y: f64| pm.pixel(x as u32, y as u32).unwrap();
    assert_eq!(at(CARD.center().x, CARD.center().y)[..3], [0, 200, 0]);
    assert_eq!(at(DISC.center().x, DISC.center().y)[..3], [0, 0, 230]);
    // No ink on either, nor within the margins kept around them.
    let mut inked = 0;
    for y in 0..400u32 {
        for x in 0..500u32 {
            let p = pm.pixel(x, y).unwrap();
            if !ink(p) {
                continue;
            }
            inked += 1;
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let in_card = fx > CARD.min_x() - 6.0 && fy < CARD.max_y() + 6.0;
            let (dx, dy) = (fx - DISC.center().x, fy - DISC.center().y);
            let in_disc = (dx * dx + dy * dy).sqrt() < 50.0 + 4.0;
            assert!(!in_card && !in_disc, "ink at ({x}, {y})");
        }
    }
    assert!(inked > 2000, "the text is drawn ({inked} pixels)");
    // Text runs on both sides of the disc: ink left and right of it, at
    // its middle.
    let row = DISC.center().y as u32;
    let band =
        |a: u32, b: u32| (row - 6..row + 6).any(|y| (a..b).any(|x| ink(pm.pixel(x, y).unwrap())));
    assert!(band(0, DISC.min_x() as u32), "no text left of the disc");
    assert!(
        band(DISC.max_x() as u32, CARD.min_x() as u32),
        "no text right of the disc"
    );
}

#[test]
fn flowed_text_selects_across_the_pieces_and_reads_whole() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    app.render_once();
    // Screen readers get the text as one, in order.
    let node = app.find_text(TEXT).expect("the text is one node");
    assert!(node.bounds.width() >= 499.0);
    // Drag from the left of the disc's middle line to its right.
    let y = DISC.center().y;
    let press = |state| Event::PointerButton {
        button: PointerButton::Left,
        state,
        position: Point::new(4.0, y),
    };
    app.pointer_event(press(ButtonState::Pressed));
    app.pointer_event(Event::PointerMoved {
        position: Point::new(DISC.max_x() + 60.0, y),
    });
    app.pointer_event(Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Released,
        position: Point::new(DISC.max_x() + 60.0, y),
    });
    app.render_once();
    set_clipboard_text("");
    app.press_key(KeyInput::Copy);
    let got = clipboard_text();
    // The left piece and the start of the right one: a run of the text,
    // more than one piece's worth.
    assert!(got.len() > 20 && TEXT.contains(&got), "{got:?}");
    let (s, e) = app.find_text(TEXT).unwrap().text_selection.unwrap();
    assert_eq!(&TEXT[s..e], got);
}
