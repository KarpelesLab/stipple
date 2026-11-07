//! Grid tracks, wrapping rows, and min/max size constraints in one window.
//!
//! - **Grid**: a form with an `auto` label column and a `1fr` field column,
//!   under a heading that spans both; and a strip of `60px | 1fr | 2fr | auto`
//!   tracks showing how each sizes.
//! - **Wrap**: tag chips in a `.wrap()` row that flow onto new lines.
//! - **Constraints**: three growing bars sharing a row, one capped by
//!   `.max_width()` and one held open by `.min_width()`; and a `.max_width()`
//!   paragraph that wraps short of its panel.
//!
//! CI (X11/Xvfb) screenshots it.

use stipple::prelude::*;

const W: f64 = 720.0;
const H: f64 = 520.0;

const TEAL: Color = Color::rgb(0x2d, 0xd4, 0xbf);
const AMBER: Color = Color::rgb(0xf5, 0x9e, 0x0b);
const VIOLET: Color = Color::rgb(0xa7, 0x8b, 0xfa);
const ROSE: Color = Color::rgb(0xf4, 0x72, 0xb6);

/// A colored block with a caption, sized by its container.
fn tile(theme: &Theme, color: Color, caption: &str) -> Element {
    row(vec![Element::text(
        caption,
        theme.typography.caption,
        Color::rgb(0x10, 0x14, 0x1c),
    )])
    .fill(color)
    .radius(4.0)
    .padding(Insets::symmetric(6.0, 8.0))
}

/// A placeholder input box.
fn field(theme: &Theme) -> Element {
    Element::boxed(BoxStyle::default())
        .fill(theme.palette.background)
        .border(theme.palette.border, 1.0)
        .radius(4.0)
        .height(26.0)
}

fn chip(theme: &Theme, text: &str) -> Element {
    row(vec![label(theme, text)])
        .fill(theme.palette.background)
        .border(theme.palette.border, 1.0)
        .radius(12.0)
        .padding(Insets::symmetric(10.0, 3.0))
}

fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let theme = *cx.theme();
    let sp = theme.spacing;

    // A form: labels size the first column, fields take the rest.
    let form = grid(
        vec![Track::Auto, Track::fr(1.0)],
        vec![
            heading(&theme, "Grid").span(2, 1),
            label(&theme, "Name"),
            field(&theme),
            label(&theme, "Email address"),
            field(&theme),
            label(&theme, "Notes"),
            paragraph(
                &theme,
                "Wrapped text in a 1fr column wraps at the column's width.",
            ),
        ],
    )
    .gap(sp.sm)
    .align(Align::Stretch, Align::Center);

    // Track kinds side by side: fixed, two fractions, and content-sized.
    let tracks = grid(
        vec![Track::px(60.0), Track::fr(1.0), Track::fr(2.0), Track::Auto],
        vec![
            tile(&theme, TEAL, "60px"),
            tile(&theme, AMBER, "1fr"),
            tile(&theme, VIOLET, "2fr"),
            tile(&theme, ROSE, "auto"),
            tile(&theme, AMBER, "span 3").span(3, 1),
            tile(&theme, ROSE, "auto"),
        ],
    )
    .gap(sp.xs);

    let grid_panel = panel(&theme, vec![form, divider(&theme), tracks]);

    let tags = [
        "layout",
        "grid",
        "wrap",
        "min-width",
        "max-width",
        "flex",
        "tracks",
        "fr",
        "auto",
        "gap",
        "alignment",
        "stipple",
    ];
    let wrapped = row(tags.iter().map(|t| chip(&theme, t)).collect())
        .wrap()
        .gap(sp.sm)
        .align(Align::Start, Align::Center);
    let wrap_panel = panel(&theme, vec![heading(&theme, "Wrap"), wrapped]);

    // Both grow; the first stops at its max and the second takes the rest.
    let grow_bars = row(vec![
        tile(&theme, TEAL, "grow, max 90").grow(1.0).max_width(90.0),
        tile(&theme, AMBER, "grow").grow(1.0),
    ])
    .gap(sp.sm);
    // Two 240px bars overflow the panel; both shrink, the first only to 160.
    let shrink_bars = row(vec![
        tile(&theme, VIOLET, "shrink, min 160")
            .width(240.0)
            .shrink(1.0)
            .min_width(160.0),
        tile(&theme, ROSE, "shrink").width(240.0).shrink(1.0),
    ])
    .gap(sp.sm);
    let capped = paragraph(
        &theme,
        "This paragraph has a max width of 260px, so it wraps before reaching the \
         panel's edge.",
    )
    .max_width(260.0);
    let constraints_panel = panel(
        &theme,
        vec![
            heading(&theme, "Constraints"),
            grow_bars,
            shrink_bars,
            capped,
        ],
    );

    let page = grid(
        vec![Track::fr(1.0), Track::fr(1.0)],
        vec![grid_panel.span(1, 2), wrap_panel, constraints_panel],
    )
    .gap(sp.lg)
    .align(Align::Stretch, Align::Start);

    column(vec![page])
        .padding(Insets::uniform(sp.lg))
        .align(Align::Start, Align::Stretch)
        .grow(1.0)
}

fn main() {
    let mut app = App::new((), view)
        .title("Stipple Layout")
        .theme(Theme::dark())
        .logical_size(Size::new(W, H));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
