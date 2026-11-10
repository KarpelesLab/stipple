//! Opens a real native window, but renders each frame's `Scene` **on the GPU**
//! (`stipple::gpu::render_scene_scaled`: boxes, paths, images, glyphs, clips and
//! layers, at the window's scale) and presents
//! the read-back `Pixmap` through the platform [`Surface`] — i.e. the GPU render
//! path wired into the live on-screen present loop via [`App::render_with`],
//! without stipple itself depending on any GPU crate.
//!
//! Built with `--features stipple::gpu/gl`, the CI visual job launches this under
//! Xvfb + Mesa (the EGL context is surfaceless, so it renders headlessly) and
//! screenshots the window to confirm GPU-rendered frames reach the screen. If
//! the GPU path is unavailable it transparently falls back to software, so the
//! window still draws.

use std::sync::Arc;
use stipple::prelude::*;
use stipple::render::{Pixmap, Scene, SoftwareRenderer};

struct Demo {
    /// A bitmap, drawn as a texture uploaded once.
    image: Arc<Pixmap>,
}

/// A 32x32 color ramp.
fn ramp() -> Arc<Pixmap> {
    let mut data = Vec::new();
    for y in 0..32u32 {
        for x in 0..32u32 {
            data.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, 0xc0, 0xff]);
        }
    }
    Arc::new(Pixmap::from_rgba8(
        stipple::geometry::PhysicalSize::new(32, 32),
        data,
    ))
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let card = panel(
        &theme,
        vec![
            label(&theme, "Stipple on the GPU"),
            divider(&theme),
            setting_row(&theme, Color::rgb(0xef, 0x68, 0x68), "Edit"),
            setting_row(&theme, Color::rgb(0x34, 0xd3, 0x99), "Edit"),
            setting_row(&theme, Color::rgb(0x60, 0x9c, 0xff), "Edit"),
            // An image, and a tilted, translucent layer: drawn on the GPU too.
            row(vec![
                image(&state.image).width(48.0).height(48.0),
                button_labeled(&theme, "Tilted")
                    .transform(Transform::IDENTITY.rotate_degrees(-6.0))
                    .opacity(0.85),
            ])
            .gap(16.0),
            button_labeled(&theme, "OK"),
        ],
    )
    .width(360.0);
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

fn main() {
    let mut app = App::new(Demo { image: ramp() }, view)
        .title("Stipple GPU")
        .theme(Theme::dark())
        .logical_size(Size::new(640.0, 480.0));

    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    // Render each frame on the GPU; fall back to the software rasterizer if the
    // GPU path is unavailable (e.g. built without the `gl` feature).
    app = app.render_with(move |scene: &Scene, bg, scale| {
        stipple::gpu::render_scene_scaled(scene, bg, scale).unwrap_or_else(|e| {
            eprintln!("GPU render unavailable ({e}); using software");
            SoftwareRenderer::new()
                .with_background(bg)
                .render(scene.clone(), scale)
        })
    });

    app.run();
}
