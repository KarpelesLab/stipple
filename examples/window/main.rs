//! Opens a real native window and runs the Stipple event loop.
//!
//! On Linux with `$DISPLAY` set this uses the native X11 backend
//! (`stipple::platform::backend::x11`); otherwise it falls back to a one-shot
//! headless present. Used by the visual-test CI job: the runner starts an Xvfb
//! display, launches this binary, and screenshots the root window to confirm
//! the X11 backend actually paints.
//!
//! The window has an icon and a minimum size. Flags:
//!
//! - `--controls` adds buttons that maximize the window and make it
//!   fullscreen, a text field, and a strip of cursor shapes to hover.
//! - `--maximized` / `--fullscreen` open the window that way.

use stipple::prelude::*;

#[derive(Default)]
struct Demo {
    controls: bool,
    maximized: bool,
    fullscreen: bool,
    name: TextInput,
}

/// A small box showing `cursor` while hovered.
fn cursor_swatch(cx: &Cx<Demo>, name: &str, cursor: CursorIcon) -> Element {
    let theme = cx.theme();
    column(vec![label(cx, name)])
        .padding(Insets::symmetric(6.0, 4.0))
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .cursor(cursor)
}

/// The window-control and cursor demo shown with `--controls`.
fn controls(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    let maximize = button(
        cx,
        if state.maximized {
            "Restore"
        } else {
            "Maximize"
        },
    )
    .on_tap(cx, move |s| {
        s.maximized = !s.maximized;
        handle.window(WindowRequest::Maximized(s.maximized));
    });
    let handle = cx.handle();
    let fullscreen = button(
        cx,
        if state.fullscreen {
            "Leave fullscreen"
        } else {
            "Fullscreen"
        },
    )
    .on_tap(cx, move |s| {
        s.fullscreen = !s.fullscreen;
        handle.window(WindowRequest::Fullscreen(s.fullscreen));
    });
    let field = text_field(cx, &state.name, |s| &mut s.name);
    let swatches = [
        ("cross", CursorIcon::Crosshair),
        ("no", CursorIcon::NotAllowed),
        ("grab", CursorIcon::Grab),
        ("move", CursorIcon::Move),
        ("ew", CursorIcon::EwResize),
        ("ns", CursorIcon::NsResize),
        ("nwse", CursorIcon::NwseResize),
        ("nesw", CursorIcon::NeswResize),
    ]
    .into_iter()
    .map(|(name, cursor)| cursor_swatch(cx, name, cursor))
    .collect();
    column(vec![
        row(vec![maximize, fullscreen]).gap(theme.spacing.sm),
        field,
        row(swatches).gap(theme.spacing.xs).wrap(),
    ])
    .gap(theme.spacing.sm)
}

/// A row with a leading color swatch and a trailing button.
fn setting_row(cx: &Cx<Demo>, accent: Color, action: &str) -> Element {
    let theme = cx.theme();
    row(vec![
        swatch(accent, 24.0, theme.radius / 2.0),
        spacer(),
        Button::new(action).variant(Variant::Secondary).view(cx),
    ])
    .gap(theme.spacing.md)
    .height(40.0)
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    // OK closes the app (and, as a tap target, shows the pointing hand).
    let handle = cx.handle();
    let ok = button(cx, "OK").on_tap(cx, move |_| handle.exit());
    let mut children = vec![
        label(cx, "Welcome to Stipple"),
        // A word-wrapping paragraph: long text flows to the panel width.
        paragraph(
            cx,
            "Stipple is a self-drawn cross-platform UI toolkit that wraps this \
             paragraph across multiple lines to fit the panel width.",
        ),
        divider(cx),
        setting_row(cx, Color::rgb(0xef, 0x68, 0x68), "Edit"),
        setting_row(cx, Color::rgb(0x34, 0xd3, 0x99), "Edit"),
        setting_row(cx, Color::rgb(0xf5, 0x9e, 0x0b), "Edit"),
        ok,
    ];
    if state.controls {
        children.push(divider(cx));
        children.push(controls(state, cx));
    }
    let card = panel(cx).children(children).width(360.0);
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

/// A 64×64 icon drawn in code: a rounded blue square with a white dot grid.
fn icon() -> Pixmap {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            // Distance outside a rounded square of corner radius 12.
            let (cx, cy) = (
                (x as f64 + 0.5 - 32.0).abs() - 20.0,
                (y as f64 + 0.5 - 32.0).abs() - 20.0,
            );
            let outside = cx.max(0.0).hypot(cy.max(0.0)) - 12.0;
            let alpha = (0.5 - outside).clamp(0.0, 1.0);
            let dot = (x % 16).abs_diff(8).max((y % 16).abs_diff(8)) <= 2;
            let [r, g, b] = if dot {
                [255, 255, 255]
            } else {
                [0x3b, 0x82, 0xf6]
            };
            px.extend_from_slice(&[r, g, b, (alpha * 255.0).round() as u8]);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(N, N), px)
}

fn main() {
    let flag = |name: &str| std::env::args().any(|a| a == name);
    let state = Demo {
        controls: flag("--controls"),
        maximized: flag("--maximized"),
        fullscreen: flag("--fullscreen"),
        ..Demo::default()
    };
    let (maximized, fullscreen) = (state.maximized, state.fullscreen);
    let app = App::new(state, view)
        .title("Stipple")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 480.0))
        .min_size(Size::new(400.0, 320.0))
        .maximized(maximized)
        .fullscreen(fullscreen)
        .icon(icon());
    app.run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
