//! The element showing a video: [`video_view`].

use super::Video;
use crate::core::{CanvasId, Cx, Element};
use crate::geometry::Rect;
use crate::render::{Color, ImageOptions, Scene};

/// The canvas ids video views take: this plus the video's number, out of
/// the way of the small numbers apps choose (and of the camera's).
const CANVAS_BASE: u32 = 0x71DE_0000;

/// The current frame of `video`, as large as fits the element's box with
/// its aspect kept (centered, black bars filling the rest), black until the
/// first frame. It grows to fill its parent by default; size it like any
/// element.
///
/// It repaints by itself as frames come, without the view being rebuilt:
/// on a GPU renderer a hardware decoder's buffer is drawn as it is (an
/// `IOSurface` on Metal), elsewhere its pixels are read once per frame.
pub fn video_view<S: 'static>(cx: &Cx<S>, video: &Video) -> Element {
    let id = CanvasId(CANVAS_BASE.wrapping_add(video.id()));
    let handle = cx.handle();
    video.set_wake(id.0, Box::new(move || handle.invalidate_canvas(id)));
    let shared = video.shared().clone();
    Element::canvas(id, move |c| {
        let size = c.size();
        let bounds = Rect::from_xywh(0.0, 0.0, size.width, size.height);
        c.fill_rect(bounds, Color::BLACK);
        let frame = shared.lock().frame.clone();
        if let Some(frame) = frame {
            draw_contained(c, bounds, &frame.image);
        }
    })
    .grow(1.0)
}

/// Draw `image` as large as fits `bounds`, centered, its aspect kept.
pub(crate) fn draw_contained(
    scene: &mut Scene,
    bounds: Rect,
    image: &std::sync::Arc<crate::render::ExternalImage>,
) {
    let size = image.size();
    let (w, h) = (size.width as f64, size.height as f64);
    if w <= 0.0 || h <= 0.0 || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return;
    }
    let s = (bounds.width() / w).min(bounds.height() / h);
    let center = bounds.center();
    let dst = Rect::from_xywh(center.x - w * s / 2.0, center.y - h * s / 2.0, w * s, h * s);
    scene.draw_external_region(
        Rect::from_xywh(0.0, 0.0, w, h),
        dst,
        image,
        &ImageOptions::new(),
    );
}
