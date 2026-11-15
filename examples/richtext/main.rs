//! Styled and selectable text: runs in their own colors, weights and sizes
//! with icons in the line (`RichText`), outlined and shadowed text over
//! artwork, and labels the user can select and copy (`.selectable()`).
//!
//! Drag across the selectable text, double-click a word or triple-click a
//! line, then Ctrl/Cmd+C (or right-click → Copy; on a touch screen, long
//! press). `--check` renders the window off screen and checks that the
//! styled runs and the outline were drawn.

use std::sync::Arc;

use stipple::geometry::Vec2;
use stipple::prelude::*;

#[derive(Default)]
struct Shop {
    gold: u32,
}

const COIN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
<circle cx="8" cy="8" r="7.5" fill="#b8860b"/>
<circle cx="8" cy="8" r="5.6" fill="#f2c94c"/>
<rect x="7" y="4.5" width="2" height="7" rx="1" fill="#b8860b"/>
</svg>"##;

const POTION: Color = Color::rgb(0xa8, 0x55, 0xf7);
const WARN: Color = Color::rgb(0xef, 0x44, 0x44);
const GOLD: Color = Color::rgb(0xf2, 0xc9, 0x4c);

fn coin() -> Arc<Svg> {
    Arc::new(Svg::parse(COIN.as_bytes()).expect("the coin parses"))
}

/// A banner of "artwork" (bands of color) with a title and a name over it,
/// readable thanks to their outline and shadow.
fn banner() -> Element {
    let band = |c: Color| {
        Element::boxed(BoxStyle::default())
            .fill(c)
            .grow(1.0)
            .height(96.0)
    };
    let art = row(vec![
        band(Color::rgb(0x2d, 0x6a, 0x4f)),
        band(Color::rgb(0xf4, 0xa2, 0x61)),
        band(Color::rgb(0xe9, 0xc4, 0x6a)),
        band(Color::rgb(0x26, 0x46, 0x53)),
    ])
    .height(96.0);
    let title = Element::text("Victory!", 40.0, Color::WHITE)
        .bold()
        .text_outline(TextOutline::new(2.5, Color::rgb(0x1b, 0x1b, 0x2f)))
        .text_shadow(TextShadow::new(Vec2::new(2.0, 3.0), Color::rgba(0, 0, 0, 150)).blur(2.0));
    let name = Element::text("Aria the Bold — level 12", 16.0, GOLD)
        .text_outline(TextOutline::new(1.5, Color::BLACK));
    Element::layers(vec![
        art,
        column(vec![title, name]).align(Align::Center, Align::Center),
    ])
    .align(Align::Stretch, Align::Stretch)
    .height(96.0)
    .radius(8.0)
    .clip()
}

fn view(state: &Shop, cx: &mut Cx<Shop>) -> Element {
    let coin = coin();
    let sold = RichText::new()
        .push("Sold 3 × ")
        .styled("Healing Potion", SpanStyle::new().color(POTION).bold())
        .push(" for 120 ")
        .icon(InlineIcon::new(coin.clone()).alt(" gold"))
        .push(".");
    let price = 250;
    let afford = RichText::new()
        .push("Dragon Scale: ")
        .styled(
            price.to_string(),
            SpanStyle::new()
                .color(if state.gold >= price { GOLD } else { WARN })
                .bold(),
        )
        .icon(InlineIcon::new(coin.clone()).alt(" gold"))
        .styled(
            format!("  (you have {})", state.gold),
            SpanStyle::new().italic().size(12.0),
        );
    let dialogue = RichText::new()
        .push("“You ")
        .italic("really")
        .push(" think the ")
        .styled("Ember Crown", SpanStyle::new().color(WARN).bold())
        .push(" is just a story? Bring me ")
        .styled("three", SpanStyle::new().underline(UnderlineStyle::Single))
        .push(" dragon scales and I'll show you a map that says otherwise.”");
    let earn = button(cx, "Loot 100 gold").on_tap(cx, |s| s.gold += 100);
    panel(cx)
        .children(vec![
            banner(),
            label(cx, sold),
            row(vec![label(cx, afford), earn])
                .gap(12.0)
                .align(Align::Start, Align::Center),
            paragraph(cx, dialogue),
            divider(cx),
            label(
                cx,
                "Selectable (drag, double-click, right-click, Ctrl/Cmd+C):",
            ),
            paragraph(
                cx,
                RichText::new()
                    .push("Decoded EAN-13: ")
                    .styled("4006381333931", SpanStyle::new().bold()),
            )
            .selectable(),
            paragraph(
                cx,
                "Karpeles Lab Inc., 1-2-3 Shibuya, Tokyo 150-0002, Japan. \
                 Long lines wrap, and a triple-click selects the whole paragraph.",
            )
            .selectable(),
        ])
        .padding(Insets::uniform(20.0))
}

fn app() -> App<Shop> {
    App::new(Shop { gold: 180 }, view)
        .title("Stipple Rich Text")
        .theme(Theme::light())
        .logical_size(Size::new(640.0, 520.0))
}

/// The banner's title has a dark outline over the light bands, and the
/// potion's name is drawn in its purple.
fn check(pm: &Pixmap) -> Result<(), String> {
    let mut purple = 0;
    let mut dark = 0;
    for y in 0..pm.size().height {
        for x in 0..pm.size().width {
            let Some([r, g, b, _]) = pm.pixel(x, y) else {
                continue;
            };
            if r > 130 && b > 200 && g < 120 {
                purple += 1;
            }
            if y < 140 && r < 50 && g < 50 && b < 70 {
                dark += 1;
            }
        }
    }
    println!("richtext: {purple} purple pixels, {dark} outline pixels");
    if purple < 20 {
        return Err("the potion's name isn't purple".into());
    }
    if dark < 50 {
        return Err("the title has no outline".into());
    }
    Ok(())
}

fn main() {
    let mut app = app();
    if std::env::args().any(|a| a == "--check") {
        match check(&app.render_once()) {
            Ok(()) => println!("richtext: ok"),
            Err(e) => {
                println!("richtext: FAILED: {e}");
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
    fn renders_styled_runs_and_an_outline() {
        let Some(font) = Font::system_default() else {
            return;
        };
        check(&app().font(font).render_once()).unwrap();
    }
}
