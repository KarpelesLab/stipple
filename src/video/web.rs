//! Video in a browser: the file plays in a hidden, muted `<video>` the page
//! holds (a Blob URL of its bytes), which the browser decodes, in hardware
//! where it can. Each new frame is the `<video>` itself as an external
//! image ([`ExternalImage::web`]): WebGPU imports it
//! (`importExternalTexture`), WebGL2 hands it to `texImage2D`, the software
//! renderer draws it into a 2D canvas and reads it back. Everything runs on
//! the page's thread.
//!
//! The page script (`web/stipple.js`) implements the imports; see the table
//! in [`backend::web`](crate::platform::backend::web).

use super::{Shared, VideoError, VideoFrame, VideoOptions, VideoStatus};
use crate::geometry::PhysicalSize;
use crate::platform::backend::web as js;
use crate::render::ExternalImage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The open videos, by the id the page knows them by.
static OPEN: Mutex<Option<HashMap<u32, Arc<Shared>>>> = Mutex::new(None);
static NEXT: AtomicU32 = AtomicU32::new(1);

fn shared(id: u32) -> Option<Arc<Shared>> {
    OPEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(&id)
        .cloned()
}

pub(super) fn open(bytes: &[u8], shared: Arc<Shared>, options: &VideoOptions) -> u32 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    shared.lock().decoder = "browser";
    OPEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(id, shared);
    js::video_open(id, bytes, options.autoplay, options.looping);
    id
}

pub(super) fn close(id: u32) {
    if let Some(map) = OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.remove(&id);
    }
    js::video_close(id);
}

pub(super) fn play(id: u32, on: bool) {
    js::video_play(id, on);
}

pub(super) fn seek(id: u32, at: Duration) {
    js::video_seek(id, at.as_secs_f64());
}

pub(super) fn set_looping(id: u32, on: bool) {
    js::video_loop(id, on);
}

/// Video `id`'s header: its size and duration (negative: unknown).
pub(crate) fn meta(id: u32, width: u32, height: u32, seconds: f64) {
    let Some(shared) = shared(id) else {
        return;
    };
    let mut st = shared.lock();
    st.size = PhysicalSize::new(width, height);
    if seconds.is_finite() && seconds >= 0.0 {
        st.duration = Some(Duration::from_secs_f64(seconds));
    }
}

/// Video `id` shows a frame: its `<video>`, the page's source `source`.
pub(crate) fn frame(id: u32, source: u32, width: u32, height: u32, seconds: f64, paused: bool) {
    let Some(shared) = shared(id) else {
        return;
    };
    let size = PhysicalSize::new(width, height);
    {
        let mut st = shared.lock();
        st.size = size;
        st.seek = None;
        st.playing = !paused;
    }
    shared.show(VideoFrame {
        timestamp: Duration::from_secs_f64(seconds.max(0.0)),
        image: Arc::new(ExternalImage::web(source, size, false)),
    });
    shared.set_status(if paused {
        VideoStatus::Paused
    } else {
        VideoStatus::Playing
    });
}

/// Video `id` reached its end.
pub(crate) fn ended(id: u32) {
    if let Some(shared) = shared(id) {
        shared.lock().playing = false;
        shared.set_status(VideoStatus::Ended);
    }
}

/// Video `id` couldn't be played: `code` is `MediaError.code`.
pub(crate) fn failed(id: u32, code: u32) {
    if let Some(shared) = shared(id) {
        shared.set_status(VideoStatus::Failed(match code {
            3 => VideoError::Decode("the browser couldn't decode the video".into()),
            4 => VideoError::Unsupported("a format this browser doesn't play".into()),
            _ => VideoError::Io("the browser couldn't read the video".into()),
        }));
    }
}
