//! A translucent window: the desktop shows through its background, which the
//! slider fades from clear to opaque, and the text and controls drawn over it
//! stay solid (see `docs/windows-and-cursors.md#translucent-windows`).
//!
//! Flags:
//!
//! - `--blur` also blurs what shows through, where the system can (a
//!   vibrancy view on macOS, acrylic on Windows 11, KWin's blur or
//!   `ext-background-effect` on Wayland, KDE's blur on X11).
//! - `--opacity N` starts the background at `N` percent (default 70).

use stipple::prelude::*;

struct Glass {
    /// The background's opacity, 0 to 1.
    opacity: f64,
    blur: bool,
}

fn view(state: &Glass, cx: &mut Cx<Glass>) -> Element {
    let theme = *cx.theme();
    let percent = (state.opacity * 100.0).round();
    let card = panel(cx)
        .children(vec![
            label(cx, "A translucent window"),
            paragraph(
                cx,
                "The window's background is painted at the opacity below, and \
                 whatever is behind the window shows through it.",
            ),
            slider(cx, state.opacity, |s, v| s.opacity = v)
                .a11y_label(format!("Background opacity, {percent}%")),
            label(
                cx,
                format!(
                    "Background: {percent}%{}",
                    if state.blur { ", blurred" } else { "" }
                ),
            ),
        ])
        .width(340.0);
    column(vec![card])
        .grow(1.0)
        .padding(Insets::uniform(theme.spacing.lg))
        .align(Align::Center, Align::Center)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let blur = args.iter().any(|a| a == "--blur");
    let opacity = args
        .iter()
        .position(|a| a == "--opacity")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<f64>().ok())
        .map_or(0.7, |p| (p / 100.0).clamp(0.0, 1.0));
    App::new(Glass { opacity, blur }, view)
        .title("Glass")
        .logical_size(Size::new(480.0, 320.0))
        .transparent(true)
        .blur(blur)
        // The theme's background carries the slider's opacity; panels and
        // text keep theirs.
        .theme_with(|state: &Glass, system: &Theme| {
            let mut theme = *system;
            let alpha = (state.opacity * 255.0).round() as u8;
            theme.palette.background = theme.palette.background.with_alpha(alpha);
            theme
        })
        .run();
}
