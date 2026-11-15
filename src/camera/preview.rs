//! The viewfinder element: [`camera_preview`].

use super::{Camera, os};
use crate::core::{CanvasId, Cx, Element};
use crate::geometry::{PhysicalSize, Rect};
use crate::render::{Color, ImageFit, ImageSource};
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
        let image = {
            let mut p = sink.preview.lock().unwrap_or_else(|e| e.into_inner());
            p.wanted = PhysicalSize::new(
                (size.width * scale).round().max(0.0) as u32,
                (size.height * scale).round().max(0.0) as u32,
            );
            p.fresh = false;
            p.image.clone()
        };
        if let Some(image) = image {
            c.draw_image_source(bounds, &ImageSource::Bitmap(image), ImageFit::Cover);
        }
    })
    .grow(1.0)
}
