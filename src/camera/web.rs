//! The camera in a browser: `getUserMedia`, a hidden `<video>` playing the
//! stream, and each new video frame (`requestVideoFrameCallback`, else every
//! animation frame) drawn into a canvas whose RGBA pixels the page hands to
//! the module (`stipple_camera_frame`). Everything runs on the page's
//! thread, the frame callback too. The torch, zoom and focus point are
//! `applyConstraints` on the track, where the browser offers them
//! (`getCapabilities`, reported once the stream starts).
//!
//! The page script (`web/stipple.js`) implements the imports; see the table
//! in [`backend::web`](crate::platform::backend::web).

use super::{CameraError, CameraOptions, Caps, Frame, FrameFormat, Lens, Plane, Session, Sink};
use crate::geometry::{PhysicalSize, Point};
use crate::platform::backend::web as js;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The open cameras, by the id the page knows them by.
static OPEN: Mutex<Option<HashMap<u32, Arc<Sink>>>> = Mutex::new(None);
static NEXT: AtomicU32 = AtomicU32::new(1);

fn sink(id: u32) -> Option<Arc<Sink>> {
    OPEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(&id)
        .cloned()
}

pub(super) fn lenses() -> Vec<Lens> {
    // The video inputs `enumerateDevices` listed (before it answers, or
    // without the API: unknown, so one).
    match js::camera_count() {
        0 => Vec::new(),
        1 => vec![Lens::Front],
        n if n > 1 && n != u32::MAX => vec![Lens::Back, Lens::Front],
        _ => vec![Lens::Front],
    }
}

pub(super) fn display_rotation() -> u32 {
    // The browser turns the stream upright itself.
    0
}

struct WebSession {
    id: u32,
}

impl Session for WebSession {
    fn start(&mut self) -> Result<(), CameraError> {
        js::camera_pause(self.id, false);
        Ok(())
    }

    fn stop(&mut self) {
        js::camera_pause(self.id, true);
    }

    fn set_torch(&mut self, on: bool) -> Result<(), CameraError> {
        js::camera_torch(self.id, on);
        Ok(())
    }

    fn set_zoom(&mut self, factor: f32) {
        js::camera_zoom(self.id, f64::from(factor));
    }

    fn focus_at(&mut self, point: Point) {
        js::camera_focus(self.id, point.x, point.y);
    }
}

impl Drop for WebSession {
    fn drop(&mut self) {
        js::camera_close(self.id);
        if let Some(map) = OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            map.remove(&self.id);
        }
        if let Some(map) = SOURCES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            map.remove(&self.id);
        }
    }
}

pub(super) fn open(
    sink: Arc<Sink>,
    options: &CameraOptions,
) -> Result<(Box<dyn Session>, Caps), CameraError> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    OPEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(id, sink);
    let facing = u32::from(options.lens == Lens::Front);
    let size = options.resolution;
    js::camera_open(id, facing, size.width, size.height);
    let caps = Caps {
        lens: options.lens,
        ..Caps::default()
    };
    Ok((Box::new(WebSession { id }), caps))
}

/// Flags of `stipple_camera_caps`.
const TORCH: u32 = 1;
const FOCUS: u32 = 2;
const FACING_USER: u32 = 4;

/// The stream of camera `id` started: what its track can do.
pub(crate) fn caps(id: u32, flags: u32, zoom_min: f64, zoom_max: f64) {
    let Some(sink) = sink(id) else {
        return;
    };
    let old = sink.caps();
    let lo = if zoom_min.is_finite() && zoom_min > 0.0 {
        zoom_min
    } else {
        1.0
    };
    let hi = if zoom_max.is_finite() {
        zoom_max.max(lo)
    } else {
        lo
    };
    sink.set_caps(Caps {
        torch: flags & TORCH != 0,
        focus: flags & FOCUS != 0,
        zoom: (lo as f32, hi as f32),
        lens: if flags & FACING_USER != 0 {
            Lens::Front
        } else {
            old.lens
        },
    });
    FRONT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(id, flags & FACING_USER != 0);
}

/// Which cameras face the user (their preview is mirrored).
static FRONT: Mutex<Option<HashMap<u32, bool>>> = Mutex::new(None);

/// Camera `id` couldn't start: 1 not allowed, 2 no camera, else another
/// failure.
pub(crate) fn failed(id: u32, code: u32) {
    let Some(sink) = sink(id) else {
        return;
    };
    sink.set_status(super::CameraStatus::Failed(match code {
        1 => CameraError::NotPermitted,
        2 => CameraError::NoCamera,
        _ => CameraError::Os("the browser couldn't start the camera".into()),
    }));
}

/// A frame of camera `id` as its `<video>`, the page's external source
/// `source`: taken as that (the preview draws the video, the GPU renderer
/// straight from it) when nothing needs its pixels; `false` asks the page
/// for them ([`frame`]).
pub(crate) fn video(id: u32, source: u32, width: u32, height: u32, _ms: f64) -> bool {
    let Some(sink) = sink(id) else {
        return false;
    };
    SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(id, source);
    let size = PhysicalSize::new(width, height);
    sink.deliver_external(
        size,
        0,
        is_front(id),
        crate::render::ExternalFormat::Rgbx8,
        || crate::render::ExternalImage::web(source, size, false),
    )
}

/// Each camera's `<video>`, as the page's external source id.
static SOURCES: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);

/// Whether camera `id` faces the user (its preview mirrored).
fn is_front(id: u32) -> bool {
    FRONT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&id).copied())
        .unwrap_or(false)
}

/// A frame of camera `id`: `width`×`height` RGBA pixels.
pub(crate) fn frame(id: u32, pixels: &[u8], width: u32, height: u32, timestamp_ms: f64) {
    let Some(sink) = sink(id) else {
        return;
    };
    let row = width as usize * 4;
    if pixels.len() < row * height as usize {
        return;
    }
    let front = is_front(id);
    let planes = [Plane::new(pixels, row, 4)];
    let source = SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&id).copied());
    let size = PhysicalSize::new(width, height);
    // The app reads the pixels; the preview can still draw the video.
    let mut video = || source.map(|s| crate::render::ExternalImage::web(s, size, false));
    let external = source.map(|_| {
        (
            crate::render::ExternalFormat::Rgbx8,
            &mut video as &mut dyn FnMut() -> Option<crate::render::ExternalImage>,
        )
    });
    sink.deliver_with(
        &Frame {
            size,
            format: FrameFormat::Rgba,
            planes: &planes,
            timestamp: Duration::from_secs_f64((timestamp_ms / 1000.0).max(0.0)),
            rotation: 0,
            mirrored: front,
        },
        external,
    );
}
