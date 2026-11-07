//! Renders a Stipple frame on the CPU, routes it through the GPU
//! ([`stipple::gpu::present_offscreen`] — EGL + GLES2 upload → draw → read back),
//! and writes the read-back RGBA to `gpu-out.raw`.
//!
//! The CI GPU job builds this with `--features stipple::gpu/gl`, runs it against
//! Mesa's software GL, and converts the raw output to a PNG screenshot. Without
//! the `gl` feature, `present_offscreen` returns an error and the demo exits
//! non-zero.

use stipple::prelude::*;

// Fixed size so CI knows the raw image dimensions (scale 1 ⇒ 420×300 px).
const W: f64 = 420.0;
const H: f64 = 300.0;

fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    let theme = *cx.theme();
    let card = panel(
        &theme,
        vec![
            label(&theme, "GPU present"),
            divider(&theme),
            setting_row(&theme, Color::rgb(0xef, 0x68, 0x68), "Edit"),
            setting_row(&theme, Color::rgb(0x34, 0xd3, 0x99), "Edit"),
            setting_row(&theme, Color::rgb(0xf5, 0x9e, 0x0b), "Edit"),
        ],
    )
    .width(360.0);
    column(vec![card])
        .grow(1.0)
        .align(Align::Center, Align::Center)
}

fn main() {
    let mut app = App::new((), view)
        .theme(Theme::dark())
        .logical_size(Size::new(W, H));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    let cpu = app.render_once();

    match stipple::gpu::present_offscreen(&cpu) {
        Ok(out) => {
            std::fs::write("gpu-out.raw", out.as_bytes()).expect("write raw");
            println!(
                "GPU round-trip ok: {}x{}",
                out.size().width,
                out.size().height
            );
        }
        Err(e) => {
            eprintln!("GPU present failed: {e}");
            std::process::exit(1);
        }
    }

    // GPU-native drawing: three solid rectangles tessellated and filled by the
    // GPU (no CPU pixmap), on a dark background.
    let size = stipple::geometry::PhysicalSize::new(W as u32, H as u32);
    // (rect, color, corner_radius, border_width): a sharp fill, a rounded fill,
    // a pill fill, and a rounded *outline* (border).
    let rects = [
        (
            Rect::from_xywh(40.0, 40.0, 120.0, 80.0),
            Color::rgb(0xef, 0x68, 0x68),
            0.0,
            0.0,
        ),
        (
            Rect::from_xywh(180.0, 60.0, 120.0, 80.0),
            Color::rgb(0x34, 0xd3, 0x99),
            24.0,
            0.0,
        ),
        (
            Rect::from_xywh(40.0, 180.0, 120.0, 80.0),
            Color::rgb(0x60, 0x9c, 0xff),
            40.0,
            0.0,
        ),
        (
            Rect::from_xywh(190.0, 180.0, 120.0, 80.0),
            Color::rgb(0xf5, 0x9e, 0x0b),
            18.0,
            6.0,
        ),
    ];
    // GPU text: rasterize a label to a white-on-black coverage mask on the CPU,
    // then let the GPU composite it (alpha-blended, recolored) over the boxes.
    let mask = Font::system_default().map(|font| {
        let label = "STIPPLE - GPU";
        let m = font.measure(label, 30.0);
        let (tw, th) = (m.width.ceil() + 12.0, m.height.ceil() + 6.0);
        let mut scene = stipple::render::Scene::new(Size::new(tw, th));
        scene.fill_text(&font, label, Point::new(6.0, 2.0), 30.0, Color::WHITE);
        let pm = stipple::render::SoftwareRenderer::new()
            .with_background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY);
        (pm, Rect::from_xywh(40.0, 130.0, tw, th))
    });
    let texts: Vec<(&stipple::render::Pixmap, Rect, Color)> = mask
        .iter()
        .map(|(pm, dst)| (pm, *dst, Color::rgb(0xec, 0xee, 0xf2)))
        .collect();

    match stipple::gpu::render_offscreen(size, Color::rgb(0x14, 0x15, 0x18), &rects, &texts) {
        Ok(out) => {
            std::fs::write("gpu-rects.raw", out.as_bytes()).expect("write raw");
            println!(
                "GPU-native scene ok: {}x{}",
                out.size().width,
                out.size().height
            );
        }
        Err(e) => {
            eprintln!("GPU rects failed: {e}");
            std::process::exit(1);
        }
    }

    // The capstone: render the *actual* widget-tree Scene (the same `view`)
    // GPU-natively via render_scene — boxes + text straight from the Scene's
    // recorded draw commands, no hand-built rectangles.
    if let Some(font) = Font::system_default() {
        let theme = Theme::dark();
        let element = view(&(), &mut Cx::new(&theme));
        let scene = stipple::core::render_view(&element, Size::new(W, H), &theme, Some(&font));
        match stipple::gpu::render_scene(&scene, theme.palette.background, &font) {
            Ok(out) => {
                std::fs::write("gpu-scene.raw", out.as_bytes()).expect("write raw");
                println!(
                    "GPU render_scene ok: {}x{}",
                    out.size().width,
                    out.size().height
                );
            }
            Err(e) => {
                eprintln!("GPU render_scene failed: {e}");
                std::process::exit(1);
            }
        }
    }

    // Vulkan FFI foundation: enumerate physical devices and create a logical
    // device + graphics queue (non-fatal — only built with the `vk` feature).
    // Under Mesa lavapipe this lists "llvmpipe".
    match stipple::gpu::vulkan_devices() {
        Ok(devs) => println!("Vulkan devices: {devs:?}"),
        Err(e) => println!("Vulkan unavailable: {e}"),
    }
    match stipple::gpu::vulkan_init_device() {
        Ok(summary) => println!("Vulkan init: {summary}"),
        Err(e) => println!("Vulkan init unavailable: {e}"),
    }
    match stipple::gpu::vulkan_init_image(W as u32, H as u32) {
        Ok(summary) => println!("Vulkan image: {summary}"),
        Err(e) => println!("Vulkan image unavailable: {e}"),
    }
    match stipple::gpu::vulkan_init_framebuffer(W as u32, H as u32) {
        Ok(summary) => println!("Vulkan framebuffer: {summary}"),
        Err(e) => println!("Vulkan framebuffer unavailable: {e}"),
    }
    match stipple::gpu::vulkan_clear(W as u32, H as u32) {
        Ok(summary) => println!("Vulkan clear: {summary}"),
        Err(e) => println!("Vulkan clear unavailable: {e}"),
    }
    // The Vulkan capstone: a GPU-rendered frame read back to the CPU and written
    // out, so CI can convert it to a real screenshot.
    match stipple::gpu::vulkan_render_clear(W as u32, H as u32) {
        Ok(pixels) => {
            std::fs::write("gpu-vk.raw", &pixels).expect("write raw");
            println!("Vulkan readback: {} bytes ({W}x{H})", pixels.len());
        }
        Err(e) => println!("Vulkan readback unavailable: {e}"),
    }
    // The full Vulkan pipeline: a triangle drawn by real SPIR-V shaders, read
    // back so CI can confirm the GPU actually rasterized green geometry.
    match stipple::gpu::vulkan_render_triangle(W as u32, H as u32) {
        Ok(pixels) => {
            std::fs::write("gpu-vk-tri.raw", &pixels).expect("write raw");
            println!("Vulkan triangle: {} bytes ({W}x{H})", pixels.len());
        }
        Err(e) => println!("Vulkan triangle unavailable: {e}"),
    }
}
