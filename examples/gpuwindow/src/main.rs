//! Opens a real native window, but renders each frame's `Scene` **on the GPU**
//! (`stipple::gpu::render_scene`: rounded-rect SDF + a glyph atlas) and presents
//! the read-back `Pixmap` through the platform [`Surface`] — i.e. the GPU render
//! path wired into the live on-screen present loop via [`App::render_with`],
//! without stipple itself depending on any GPU crate.
//!
//! Built with `--features stipple::gpu/gl`, the CI visual job launches this under
//! Xvfb + Mesa (the EGL context is surfaceless, so it renders headlessly) and
//! screenshots the window to confirm GPU-rendered frames reach the screen. If
//! the GPU path is unavailable it transparently falls back to software, so the
//! window still draws.

use stipple::prelude::*;
use stipple::render::{Scene, SoftwareRenderer};

struct Demo;

fn view(_state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let card = panel(
        &theme,
        vec![
            label(&theme, "Stipple on the GPU"),
            divider(&theme),
            setting_row(&theme, Color::rgb(0xef, 0x68, 0x68)),
            setting_row(&theme, Color::rgb(0x34, 0xd3, 0x99)),
            setting_row(&theme, Color::rgb(0x60, 0x9c, 0xff)),
            button_labeled(&theme, "OK"),
        ],
    )
    .width(360.0);
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

fn main() {
    let mut app = App::new(Demo, view)
        .title("Stipple GPU")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 480.0));

    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    // Render each frame on the GPU; fall back to the software rasterizer if the
    // GPU path is unavailable (e.g. built without the `gl` feature). `Font` isn't
    // `Clone`, so the GPU renderer loads its own copy of the system font.
    if let Some(gpu_font) = Font::system_default() {
        app = app.render_with(move |scene: &Scene, bg, scale| {
            stipple::gpu::render_scene(scene, bg, &gpu_font).unwrap_or_else(|e| {
                eprintln!("GPU render unavailable ({e}); using software");
                SoftwareRenderer::new()
                    .with_background(bg)
                    .render(scene.clone(), scale)
            })
        });
    }

    app.run();
}
