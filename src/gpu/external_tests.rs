//! External images on the GPU against the software renderer: real
//! `dma-buf`s (made by `udmabuf` from known bytes) drawn by the GPU
//! executor, imported where it can (GLES), and by the software renderer,
//! which maps them; the frames must agree. Without `/dev/udmabuf` or a
//! device they're skipped (failing instead with `STIPPLE_REQUIRE_GPU` and
//! `STIPPLE_REQUIRE_UDMABUF`).

use super::renderer::Api;
use super::*;
use crate::geometry::Size;
use crate::render::external::YuvColorSpace;
use crate::render::external::{DRM_FORMAT_MOD_LINEAR, DmaBufPlane, udmabuf};
use crate::render::{ExternalFormat, ExternalImage, ImageOptions, SoftwareRenderer};
use std::sync::Arc;

const BG: Color = Color::rgb(20, 30, 40);

fn buffer(bytes: &[u8]) -> Option<std::os::fd::OwnedFd> {
    match udmabuf(bytes) {
        Ok(fd) => Some(fd),
        Err(e) if std::env::var_os("STIPPLE_REQUIRE_UDMABUF").is_some() => {
            panic!("no udmabuf: {e}")
        }
        Err(e) => {
            eprintln!("skipping: no udmabuf ({e})");
            None
        }
    }
}

/// A 16×12 BGRA image: four quadrants, red, green, blue and translucent
/// white, rows padded to `stride` bytes.
fn bgra(stride: usize) -> Option<Arc<ExternalImage>> {
    bgra_as(stride, ExternalFormat::Bgra8)
}

/// [`bgra`]'s bytes as `format` (`Bgrx8`: the white opaque).
fn bgra_as(stride: usize, format: ExternalFormat) -> Option<Arc<ExternalImage>> {
    let (w, h) = (16usize, 12usize);
    let mut bytes = vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            let px = match (x < w / 2, y < h / 2) {
                (true, true) => [0, 0, 255, 255],  // B G R A: red
                (false, true) => [0, 255, 0, 255], // green
                (true, false) => [255, 0, 0, 255], // blue
                (false, false) => [255, 255, 255, 128],
            };
            bytes[y * stride + x * 4..][..4].copy_from_slice(&px);
        }
    }
    let fd = buffer(&bytes)?;
    let image = ExternalImage::dma_buf(
        PhysicalSize::new(w as u32, h as u32),
        format,
        DRM_FORMAT_MOD_LINEAR,
        vec![DmaBufPlane::new(fd, 0, stride as u32)],
    )
    .unwrap();
    Some(Arc::new(image))
}

/// A 16×8 NV12 frame: luma rising left to right and a little top to
/// bottom, chroma red on the left half and blue on the right (bluer row by
/// row); rows padded to 32 bytes, the chroma plane at a page-aligned
/// offset.
fn nv12() -> Option<Arc<ExternalImage>> {
    nv12_in(YuvColorSpace::Bt601Full)
}

/// [`nv12`] in the color space `color`.
fn nv12_in(color: YuvColorSpace) -> Option<Arc<ExternalImage>> {
    let (w, h, stride) = (16usize, 8usize, 32usize);
    let off = 4096;
    let mut bytes = vec![0u8; off + stride * h / 2];
    for y in 0..h {
        for x in 0..w {
            bytes[y * stride + x] = (60 + x * 8 + y * 6) as u8;
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let (cb, cr) = if x < w / 4 { (90, 220) } else { (220, 100) };
            bytes[off + y * stride + x * 2] = cb + (y * 8) as u8;
            bytes[off + y * stride + x * 2 + 1] = cr;
        }
    }
    let fd = buffer(&bytes)?;
    let fd2 = fd.try_clone().unwrap();
    let image = ExternalImage::dma_buf(
        PhysicalSize::new(w as u32, h as u32),
        ExternalFormat::Nv12,
        DRM_FORMAT_MOD_LINEAR,
        vec![
            DmaBufPlane::new(fd, 0, stride as u32),
            DmaBufPlane::new(fd2, off as u64, stride as u32),
        ],
    )
    .unwrap()
    .color_space(color);
    Some(Arc::new(image))
}

fn gpu(scene: &Scene) -> Option<(Pixmap, Api, usize, String)> {
    match with_default(|r| {
        let p = r.render(scene, BG, ScaleFactor::IDENTITY)?;
        Ok((p, r.api(), r.uploads(), r.device()))
    }) {
        Ok(r) => Some(r),
        Err(e) if std::env::var_os("STIPPLE_REQUIRE_GPU").is_some() => {
            panic!("GPU unavailable: {e}")
        }
        Err(e) => {
            eprintln!("skipping: GPU unavailable: {e}");
            None
        }
    }
}

/// The mean channel difference, and the worst one.
fn diff(a: &Pixmap, b: &Pixmap) -> (f64, u8) {
    assert_eq!(a.size(), b.size());
    let (mut sum, mut worst) = (0u64, 0u8);
    for (p, q) in a.as_bytes().iter().zip(b.as_bytes()) {
        let d = p.abs_diff(*q);
        sum += d as u64;
        worst = worst.max(d);
    }
    (sum as f64 / a.as_bytes().len() as f64, worst)
}

fn check(name: &str, scene: &Scene, imported: bool) {
    check_with(name, scene, |_, _| imported)
}

/// [`check`], whether the image must have been imported depending on the
/// API and the device.
fn check_with(name: &str, scene: &Scene, imported: impl FnOnce(Api, &str) -> bool) {
    let Some((gpu, api, uploads, device)) = gpu(scene) else {
        return;
    };
    let cpu = SoftwareRenderer::new()
        .background(BG)
        .render(scene.clone(), ScaleFactor::IDENTITY);
    if let Some(dir) = std::env::var_os("STIPPLE_GPU_DUMP") {
        let dir = std::path::Path::new(&dir);
        let _ = gpu.save_png(dir.join(format!("{name}-gpu.png")));
        let _ = cpu.save_png(dir.join(format!("{name}-cpu.png")));
    }
    let (mean, worst) = diff(&gpu, &cpu);
    eprintln!(
        "{name} on {}: mean {mean:.3}, worst {worst}, {uploads} uploads",
        api.name()
    );
    assert!(
        mean < 1.5,
        "{name}: the GPU frame differs: mean {mean}, worst {worst}"
    );
    if imported(api, &device) && matches!(api, Api::Gl | Api::Vulkan) {
        assert_eq!(uploads, 0, "{name}: imported, not uploaded");
    }
}

#[test]
fn a_bgra_dma_buf_is_imported_and_drawn_as_the_cpu_draws_it() {
    let Some(image) = bgra(96) else {
        return;
    };
    let mut scene = Scene::new(Size::new(64.0, 48.0));
    // At its size, then scaled up with hard pixels.
    scene.draw_external(Rect::from_xywh(2.0, 3.0, 16.0, 12.0), &image);
    let opts = ImageOptions::new().nearest();
    let src = Rect::from_xywh(0.0, 0.0, 16.0, 12.0);
    scene.draw_external_region(src, Rect::from_xywh(24.0, 8.0, 32.0, 24.0), &image, &opts);
    check("external-bgra", &scene, true);
    // Drawn again: still no upload.
    check("external-bgra-again", &scene, true);
}

/// The same bytes as BGRX: the translucent white's alpha byte ignored.
#[test]
fn a_bgrx_dma_buf_is_drawn_opaque() {
    let Some(image) = bgra_as(96, ExternalFormat::Bgrx8) else {
        return;
    };
    let mut scene = Scene::new(Size::new(24.0, 16.0));
    scene.draw_external(Rect::from_xywh(4.0, 2.0, 16.0, 12.0), &image);
    check("external-bgrx", &scene, true);
}

/// Rows 80 bytes apart, a pitch NVIDIA's Vulkan driver takes but doesn't
/// sample with (it reads them 64 apart): drawn right, imported or not.
#[test]
fn a_bgra_dma_buf_with_an_odd_pitch_is_drawn_as_the_cpu_draws_it() {
    let Some(image) = bgra(80) else {
        return;
    };
    let mut scene = Scene::new(Size::new(40.0, 24.0));
    let opts = ImageOptions::new().nearest();
    let src = Rect::from_xywh(0.0, 0.0, 16.0, 12.0);
    scene.draw_external_region(src, Rect::from_xywh(4.0, 0.0, 32.0, 24.0), &image, &opts);
    check("external-bgra-pitch80", &scene, false);
}

#[test]
fn an_nv12_dma_buf_is_converted_like_the_camera_frames() {
    let Some(image) = nv12() else {
        return;
    };
    let mut scene = Scene::new(Size::new(48.0, 48.0));
    scene.draw_external(Rect::from_xywh(4.0, 4.0, 16.0, 8.0), &image);
    // Turned upright (a quarter turn clockwise) and mirrored, as the
    // camera's preview draws a phone's frames.
    let opts = ImageOptions::new()
        .rotate(std::f64::consts::FRAC_PI_2)
        .flip_y(true)
        .nearest();
    let src = Rect::from_xywh(0.0, 0.0, 16.0, 8.0);
    scene.draw_external_region(src, Rect::from_xywh(16.0, 24.0, 32.0, 16.0), &image, &opts);
    // NVIDIA's GLES samples a linear NV12 buffer only through its own
    // conversion, which takes full range as limited: converted on the CPU.
    check_with("external-nv12", &scene, |api, device| {
        !(api == Api::Gl && device.contains("NVIDIA"))
    });
}

#[test]
fn an_nv12_dma_buf_in_bt709_limited_range_is_converted_as_the_cpu_converts_it() {
    let Some(image) = nv12_in(YuvColorSpace::Bt709Limited) else {
        return;
    };
    let mut scene = Scene::new(Size::new(40.0, 24.0));
    let opts = ImageOptions::new().nearest();
    let src = Rect::from_xywh(0.0, 0.0, 16.0, 8.0);
    scene.draw_external_region(src, Rect::from_xywh(4.0, 4.0, 32.0, 16.0), &image, &opts);
    check("external-nv12-bt709", &scene, true);
}

/// A buffer rendered and exported by the GPU itself (through GBM on the
/// first render node), tiled as the driver likes, imported back: the
/// modifier is echoed, and the CPU can't read it, so only the GPU draws it.
#[cfg(feature = "gl")]
#[test]
fn a_gpu_rendered_dma_buf_is_imported_with_its_modifier() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let Ok(node) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/dri/renderD128")
    else {
        eprintln!("skipping: no render node");
        return;
    };
    let export = match export_dmabuf_on_device(node.as_raw_fd(), 32, 16) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping: no dma-buf export ({e})");
            return;
        }
    };
    let Some(format) = ExternalFormat::from_drm_fourcc(export.fourcc) else {
        eprintln!("skipping: exported as {:#x}", export.fourcc);
        return;
    };
    let fd = unsafe { OwnedFd::from_raw_fd(export.fd) };
    let image = ExternalImage::dma_buf(
        PhysicalSize::new(32, 16),
        format,
        export.modifier,
        vec![DmaBufPlane::new(fd, export.offset as u64, export.stride)],
    )
    .unwrap();
    let mut scene = Scene::new(Size::new(40.0, 20.0));
    scene.draw_external(Rect::from_xywh(4.0, 2.0, 32.0, 16.0), &Arc::new(image));
    let Ok((frame, api, uploads)) = with_default(|r| {
        let p = r.render(&scene, BG, ScaleFactor::IDENTITY)?;
        Ok((p, r.api(), r.uploads()))
    }) else {
        eprintln!("skipping: no GPU");
        return;
    };
    eprintln!(
        "{} imported a modifier {:#x} buffer: {:?}, {uploads} uploads",
        api.name(),
        export.modifier,
        frame.pixel(20, 10)
    );
    if uploads == 0 {
        // The exporter's solid stipple blue.
        assert_eq!(frame.pixel(20, 10), Some([0x60, 0x9c, 0xff, 0xff]));
        assert_eq!(frame.pixel(1, 1), Some([20, 30, 40, 255]));
    }
}
