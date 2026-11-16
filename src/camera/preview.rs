//! The viewfinder element: [`camera_preview`].

use super::{Camera, ExternalFrame, os};
use crate::core::{CanvasId, Cx, Element};
use crate::geometry::{PhysicalSize, Rect, ScaleFactor, Size};
use crate::render::{Color, ImageFit, ImageOptions, ImageSource, Pixmap, Scene, SoftwareRenderer};
use std::sync::atomic::Ordering;

/// The canvas ids previews take: this plus the camera's number, out of the
/// way of the small numbers apps choose.
const CANVAS_BASE: u32 = 0xCA4E_0000;

/// A live preview of `camera`: the latest frame, upright, scaled to cover
/// the element (centered, the overflow cropped), black until the first
/// frame. It grows to fill its parent by default; size it like any element
/// ([`width`](Element::width), [`height`](Element::height),
/// [`grow`](Element::grow)).
///
/// It repaints by itself as frames arrive, without the view being rebuilt.
/// Draw over it by stacking elements on it
/// ([`Element::layers`]); map what a decoder found to it, or a
/// tap on it to the frame, with
/// [`Camera::preview_mapping`](super::Camera::preview_mapping):
///
/// ```no_run
/// # use stipple::prelude::*;
/// # use stipple::camera::{Camera, camera_preview};
/// # struct State { camera: Camera }
/// fn view(state: &State, cx: &mut Cx<State>) -> Element {
///     let camera = state.camera.clone();
///     camera_preview(cx, &state.camera).on_pointer(cx, move |ev, _| {
///         if ev.kind == PointerEventKind::Down {
///             let _ = camera.focus_at_preview(ev.position, ev.size);
///         }
///     })
/// }
/// ```
///
/// Each frame shown costs a conversion on the capture thread, to at most
/// the element's size, and an upload through the renderer; see
/// [the module's notes](super).
pub fn camera_preview<S: 'static>(cx: &Cx<S>, camera: &Camera) -> Element {
    let id = CanvasId(CANVAS_BASE.wrapping_add(camera.inner.id));
    let sink = camera.inner.sink.clone();
    sink.display_rotation
        .store(os::display_rotation(), Ordering::Relaxed);
    {
        let handle = cx.handle();
        let mut p = sink.preview.lock().unwrap_or_else(|e| e.into_inner());
        p.wake = Some(Box::new(move || handle.invalidate_canvas(id)));
    }
    Element::canvas(id, move |c| {
        let size = c.size();
        let scale = c.scale();
        let bounds = Rect::from_xywh(0.0, 0.0, size.width, size.height);
        c.fill_rect(bounds, Color::BLACK);
        let (image, external) = {
            let mut p = sink.preview.lock().unwrap_or_else(|e| e.into_inner());
            p.wanted = PhysicalSize::new(
                (size.width * scale).round().max(0.0) as u32,
                (size.height * scale).round().max(0.0) as u32,
            );
            p.fresh = false;
            (p.image.clone(), p.external.clone())
        };
        if let Some(frame) = external {
            draw_frame(c, bounds, &frame);
        } else if let Some(image) = image {
            c.draw_image_source(bounds, &ImageSource::Bitmap(image), ImageFit::Cover);
        }
    })
    .grow(1.0)
}

/// Draw the camera's buffer `frame` turned upright and mirrored as it
/// should be, covering `bounds` (centered, the overflow cropped): the
/// buffer's own size, scaled, rotated about the center and flipped (before
/// the turn, so a turned frame flips top to bottom).
pub(crate) fn draw_frame(scene: &mut Scene, bounds: Rect, frame: &ExternalFrame) {
    let size = frame.image.size();
    let (w, h) = (size.width as f64, size.height as f64);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let turned = frame.rotation % 180 == 90;
    let (uw, uh) = if turned { (h, w) } else { (w, h) };
    let s = (bounds.width() / uw).max(bounds.height() / uh);
    let center = bounds.center();
    let dst = Rect::from_xywh(center.x - w * s / 2.0, center.y - h * s / 2.0, w * s, h * s);
    let opts = ImageOptions::new()
        .rotate((frame.rotation % 360) as f64 * std::f64::consts::PI / 180.0)
        .flip_x(frame.mirrored && !turned)
        .flip_y(frame.mirrored && turned);
    scene.push_clip(bounds);
    let src = Rect::from_xywh(0.0, 0.0, w, h);
    scene.draw_external_region(src, dst, &frame.image, &opts);
    scene.pop_clip();
}

/// `frame` read on the CPU and turned upright, at full size.
pub(crate) fn upright(frame: &ExternalFrame) -> Option<Pixmap> {
    let size = super::frame::upright(frame.image.size(), frame.rotation);
    let mut scene = Scene::new(Size::new(size.width as f64, size.height as f64));
    let bounds = Rect::from_xywh(0.0, 0.0, size.width as f64, size.height as f64);
    frame.image.pixmap()?;
    draw_frame(&mut scene, bounds, frame);
    Some(SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Frame, FrameFormat, Plane};
    use crate::render::ExternalImage;
    use std::sync::Arc;

    /// A frame drawn from its buffer turns out as the CPU converts it, for
    /// every rotation, mirrored or not.
    #[test]
    fn a_buffer_drawn_upright_matches_the_converted_frame() {
        // A 4×2 RGBA frame of distinct pixels.
        let (w, h) = (4usize, 2usize);
        let data: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 30) as u8, 255 - (i * 30) as u8, (i * 7) as u8, 255])
            .collect();
        let pixels = Arc::new(Pixmap::from_rgba8(PhysicalSize::new(4, 2), data.clone()));
        let planes = [Plane::new(&data, w * 4, 4)];
        for rotation in [0, 90, 180, 270] {
            for mirrored in [false, true] {
                let frame = Frame {
                    size: PhysicalSize::new(w as u32, h as u32),
                    format: FrameFormat::Rgba,
                    planes: &planes,
                    timestamp: std::time::Duration::ZERO,
                    rotation,
                    mirrored,
                };
                let ext = ExternalFrame {
                    image: Arc::new(ExternalImage::from_pixels(pixels.clone())),
                    rotation,
                    mirrored,
                };
                let ours = upright(&ext).unwrap();
                assert_eq!(
                    ours.as_bytes(),
                    frame.to_pixmap().as_bytes(),
                    "rotation {rotation}, mirrored {mirrored}"
                );
            }
        }
    }
}
