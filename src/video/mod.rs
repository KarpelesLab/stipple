//! Video playback: a file's frames decoded (in hardware where the platform
//! can) and drawn by the renderer, on the GPU without a copy where the
//! decoder's buffers can be imported.
//!
//! ```no_run
//! use stipple::prelude::*;
//! use stipple::video::{Video, VideoOptions, video_view};
//!
//! struct State {
//!     video: Video,
//! }
//!
//! fn view(state: &State, cx: &mut Cx<State>) -> Element {
//!     let video = state.video.clone();
//!     column(vec![
//!         video_view(cx, &state.video),
//!         button(cx, "Play / pause").on_tap(cx, move |_| {
//!             if video.is_playing() { video.pause() } else { video.play() }
//!         }),
//!     ])
//! }
//!
//! let video = Video::open_file("clip.mp4", VideoOptions::new().autoplay(true)).unwrap();
//! # let _ = State { video };
//! ```
//!
//! - [`Video::open`] (bytes) and [`Video::open_file`] open a video and show
//!   its first frame; [`play`](Video::play), [`pause`](Video::pause),
//!   [`seek`](Video::seek) and [`set_looping`](Video::set_looping) control it
//!   from any thread.
//! - [`video_view`] is the element that shows it: the current frame,
//!   letterboxed in the element's box, repainted by itself as frames come
//!   (without the view being rebuilt).
//! - [`Video::frame`] is the current frame as a [`VideoFrame`]: an
//!   [`ExternalImage`] to draw with
//!   [`Scene::draw_external`](crate::render::Scene::draw_external), or to
//!   show in a viewport. [`Video::on_frame`] hears each frame as it's shown.
//!
//! | | Containers | Decoded by | Frames |
//! |---|---|---|---|
//! | macOS, iOS | MP4, Matroska | H.264 and HEVC: VideoToolbox (hardware); MPEG-2: software | the decoder's `CVPixelBuffer` as an `IOSurface` (NV12), drawn by Metal without a copy |
//! | Linux | MP4, Matroska | H.264: VA-API (hardware); MPEG-2: software | read back from the decoder's surface once, drawn as an image |
//! | Windows | MP4, Matroska | H.264 and HEVC: Media Foundation (DXVA on a GPU, else software); MPEG-2: software | a shared NV12 texture while Direct3D 11 draws, else an image |
//! | Android | MP4, Matroska | H.264 and HEVC: `MediaCodec`; MPEG-2: software | the decoder's `AHardwareBuffer` while the GPU renderer imports it, else an image |
//! | Web | what the browser plays | the browser, in a `<video>` | the `<video>` itself, imported by WebGPU or WebGL2 |
//!
//! Decoding natively needs the `video` feature (oxideav's MP4 and Matroska
//! demuxers, its MPEG-1/2 decoder and its VA-API bridge; VideoToolbox,
//! Media Foundation and `MediaCodec` are loaded at run time); without it,
//! [`Video::open`] says [`VideoError::Unsupported`] except on the web. There
//! is no sound yet: the audio track is ignored (the web's `<video>` plays
//! muted). See `docs/video.md`.

mod view;

#[cfg(all(feature = "video", target_os = "android"))]
#[allow(unsafe_code)]
mod mediacodec;
#[cfg(all(feature = "video", target_os = "windows"))]
#[allow(unsafe_code)]
mod mf;
#[cfg(all(feature = "video", not(target_arch = "wasm32")))]
// For the hardware decoders, which static fullrust builds have none of.
#[cfg_attr(target_os = "fullrust", allow(dead_code))]
mod nal;
#[cfg(all(feature = "video", not(target_arch = "wasm32")))]
mod nv12;
#[cfg(all(feature = "video", not(target_arch = "wasm32")))]
mod player;
#[cfg(all(feature = "video", not(target_arch = "wasm32")))]
mod soft;
#[cfg(all(feature = "video", target_os = "linux"))]
#[allow(unsafe_code)]
mod vaapi;
#[cfg(all(feature = "video", any(target_os = "macos", target_os = "ios")))]
#[allow(unsafe_code)]
mod vt;
#[cfg(target_arch = "wasm32")]
pub(crate) mod web;

#[cfg(all(test, feature = "video", not(target_arch = "wasm32")))]
mod tests;

pub use view::video_view;

use crate::geometry::PhysicalSize;
use crate::render::ExternalImage;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How [`Video::open`] opens a video.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoOptions {
    /// Start playing at once (`false` by default: the first frame shows,
    /// paused).
    pub autoplay: bool,
    /// Start again from the beginning at the end (`false` by default).
    pub looping: bool,
    /// Decode with the platform's decoder where it has one (`true` by
    /// default): VideoToolbox, VA-API, Media Foundation (in hardware where
    /// there's a GPU, else in software) or `MediaCodec`; `false` keeps to
    /// Stipple's own software decoders, which only know MPEG-1/2.
    /// The web always plays through the browser.
    pub hardware: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            autoplay: false,
            looping: false,
            hardware: true,
        }
    }
}

impl VideoOptions {
    /// Paused at the first frame, not looping, hardware decoding.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start playing at once.
    pub fn autoplay(mut self, autoplay: bool) -> Self {
        self.autoplay = autoplay;
        self
    }

    /// Start again at the end.
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    /// Decode with the platform's decoder where it has one.
    pub fn hardware(mut self, hardware: bool) -> Self {
        self.hardware = hardware;
        self
    }
}

/// Why a video couldn't be opened or played.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoError {
    /// A container or codec this build or platform can't decode (natively
    /// everything needs the `video` feature; H.264 needs the platform's
    /// decoder: VideoToolbox, VA-API, Media Foundation or `MediaCodec`).
    Unsupported(String),
    /// The file is damaged or not what it claims to be, or the decoder
    /// failed.
    Decode(String),
    /// The file couldn't be read.
    Io(String),
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(m) => write!(f, "unsupported video: {m}"),
            Self::Decode(m) => write!(f, "video decode error: {m}"),
            Self::Io(m) => write!(f, "video read error: {m}"),
        }
    }
}

impl std::error::Error for VideoError {}

/// Where a [`Video`] stands.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoStatus {
    /// Opened, its first frame not decoded yet (on the web, until the
    /// browser has read the file's header).
    Loading,
    /// Showing a frame, not advancing.
    Paused,
    /// Playing.
    Playing,
    /// At the end (and not looping).
    Ended,
    /// It stopped: the decoder failed.
    Failed(VideoError),
}

/// A frame of a [`Video`]: the image to draw and its time in the video.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// Its presentation time, from the start of the video.
    pub timestamp: Duration,
    /// The decoder's buffer (an `IOSurface` from VideoToolbox, a shared NV12
    /// texture from Media Foundation, an `AHardwareBuffer` from `MediaCodec`,
    /// the page's `<video>`) or the decoded pixels: draw it with
    /// [`Scene::draw_external`](crate::render::Scene::draw_external).
    pub image: Arc<ExternalImage>,
}

type WakeFn = Box<dyn Fn() + Send + Sync>;
type FrameFn = Box<dyn FnMut(&VideoFrame) + Send>;

/// What the player (a thread natively, the page's events on the web) and
/// the handles share.
pub(crate) struct State {
    pub(crate) playing: bool,
    pub(crate) looping: bool,
    /// A seek asked for and not done yet.
    pub(crate) seek: Option<Duration>,
    /// The last handle went: the player stops.
    pub(crate) quit: bool,
    /// Changes each time the handles change something (the player waits
    /// for that).
    pub(crate) generation: u64,
    pub(crate) frame: Option<VideoFrame>,
    pub(crate) duration: Option<Duration>,
    pub(crate) size: PhysicalSize,
    pub(crate) status: VideoStatus,
    pub(crate) decoder: &'static str,
    /// Repaints each [`video_view`] showing it, by canvas id.
    wakes: Vec<(u32, WakeFn)>,
    on_frame: Option<FrameFn>,
}

pub(crate) struct Shared {
    pub(crate) state: Mutex<State>,
    pub(crate) changed: Condvar,
}

impl Shared {
    fn new(options: &VideoOptions) -> Self {
        Self {
            state: Mutex::new(State {
                playing: options.autoplay,
                looping: options.looping,
                seek: None,
                quit: false,
                generation: 0,
                frame: None,
                duration: None,
                size: PhysicalSize::new(0, 0),
                status: VideoStatus::Loading,
                decoder: "",
                wakes: Vec::new(),
                on_frame: None,
            }),
            changed: Condvar::new(),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Change the state from a handle and tell the player.
    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut st = self.lock();
        f(&mut st);
        st.generation += 1;
        drop(st);
        self.changed.notify_all();
    }

    #[cfg_attr(not(any(feature = "video", target_arch = "wasm32")), allow(dead_code))]
    /// Show `frame`: keep it, hand it to the app's callback, repaint the
    /// views.
    pub(crate) fn show(&self, frame: VideoFrame) {
        let mut callback = {
            let mut st = self.lock();
            st.frame = Some(frame.clone());
            st.on_frame.take()
        };
        if let Some(f) = callback.as_mut() {
            f(&frame);
        }
        let st = &mut *self.lock();
        if st.on_frame.is_none() {
            st.on_frame = callback;
        }
        for (_, wake) in &st.wakes {
            wake();
        }
    }

    #[cfg_attr(not(any(feature = "video", target_arch = "wasm32")), allow(dead_code))]
    /// Set the status (from the player) and repaint the views.
    pub(crate) fn set_status(&self, status: VideoStatus) {
        let st = &mut *self.lock();
        if let VideoStatus::Failed(e) = &status
            && st.status != status
        {
            eprintln!("stipple: video: {e}");
        }
        st.status = status;
        for (_, wake) in &st.wakes {
            wake();
        }
    }
}

/// What the player hands back to [`Video::open`].
#[allow(dead_code)] // nothing to hold where there's no player
pub(crate) enum Backend {
    /// The player thread (native), stopped when the handle goes.
    Thread,
    /// The page's `<video>`, by the id the page knows it by.
    #[cfg(target_arch = "wasm32")]
    Web(u32),
}

struct Inner {
    id: u32,
    shared: Arc<Shared>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    backend: Backend,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shared.update(|st| {
            st.quit = true;
            st.wakes.clear();
            st.on_frame = None;
        });
        #[cfg(target_arch = "wasm32")]
        if let Backend::Web(id) = self.backend {
            web::close(id);
        }
    }
}

/// The next video's number (its views' canvas ids).
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// An open video. Clones share it; it stops when the last one is dropped.
/// Its methods can be called from any thread.
#[derive(Clone)]
pub struct Video {
    inner: Arc<Inner>,
}

impl fmt::Debug for Video {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let st = self.inner.shared.lock();
        f.debug_struct("Video")
            .field("id", &self.inner.id)
            .field("status", &st.status)
            .field("decoder", &st.decoder)
            .finish()
    }
}

impl PartialEq for Video {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Video {
    /// Open the video file in `bytes` (MP4 or Matroska natively; whatever
    /// the browser plays on the web) and show its first frame.
    ///
    /// Natively the container and codec are checked here: a file this
    /// build can't decode is [`VideoError::Unsupported`] at once. The
    /// frames are decoded on a thread of the video's own.
    pub fn open(bytes: impl Into<Arc<[u8]>>, options: VideoOptions) -> Result<Video, VideoError> {
        let bytes: Arc<[u8]> = bytes.into();
        let shared = Arc::new(Shared::new(&options));
        let backend = Self::start(Source::Bytes(bytes), &shared, &options)?;
        Ok(Video {
            inner: Arc::new(Inner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                shared,
                backend,
            }),
        })
    }

    /// Open the video file at `path`, read as it plays rather than all at
    /// once. Not on the web (read the file's bytes some other way and
    /// [`open`](Self::open) them).
    pub fn open_file(
        path: impl AsRef<std::path::Path>,
        options: VideoOptions,
    ) -> Result<Video, VideoError> {
        let path = path.as_ref().to_path_buf();
        let shared = Arc::new(Shared::new(&options));
        let backend = Self::start(Source::File(path), &shared, &options)?;
        Ok(Video {
            inner: Arc::new(Inner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                shared,
                backend,
            }),
        })
    }

    #[allow(unused_variables)]
    fn start(
        source: Source,
        shared: &Arc<Shared>,
        options: &VideoOptions,
    ) -> Result<Backend, VideoError> {
        #[cfg(target_arch = "wasm32")]
        {
            match source {
                Source::Bytes(bytes) => {
                    Ok(Backend::Web(web::open(&bytes, shared.clone(), options)))
                }
                Source::File(_) => Err(VideoError::Unsupported(
                    "the web can't open a file by path".into(),
                )),
            }
        }
        #[cfg(all(feature = "video", not(target_arch = "wasm32")))]
        {
            player::start(source, shared.clone(), options).map(|()| Backend::Thread)
        }
        #[cfg(not(any(feature = "video", target_arch = "wasm32")))]
        {
            Err(VideoError::Unsupported(
                "playing video needs stipple's `video` feature".into(),
            ))
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.inner.id
    }

    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.inner.shared
    }

    /// Play from where it is (from the start again after the end).
    pub fn play(&self) {
        self.inner.shared.update(|st| {
            if st.status == VideoStatus::Ended {
                st.seek = Some(Duration::ZERO);
            }
            st.playing = true;
        });
        #[cfg(target_arch = "wasm32")]
        if let Backend::Web(id) = self.inner.backend {
            web::play(id, true);
        }
    }

    /// Stop advancing, keeping the current frame.
    pub fn pause(&self) {
        self.inner.shared.update(|st| st.playing = false);
        #[cfg(target_arch = "wasm32")]
        if let Backend::Web(id) = self.inner.backend {
            web::play(id, false);
        }
    }

    /// Whether it's playing (asked to play and not at the end).
    pub fn is_playing(&self) -> bool {
        let st = self.inner.shared.lock();
        st.playing && !matches!(st.status, VideoStatus::Ended | VideoStatus::Failed(_))
    }

    /// Show the frame at `at` (the nearest at or after it), playing on
    /// from there if it was playing.
    pub fn seek(&self, at: Duration) {
        self.inner.shared.update(|st| st.seek = Some(at));
        #[cfg(target_arch = "wasm32")]
        if let Backend::Web(id) = self.inner.backend {
            web::seek(id, at);
        }
    }

    /// Start again from the beginning at the end, or not.
    pub fn set_looping(&self, looping: bool) {
        self.inner.shared.update(|st| st.looping = looping);
        #[cfg(target_arch = "wasm32")]
        if let Backend::Web(id) = self.inner.backend {
            web::set_looping(id, looping);
        }
    }

    /// Whether it starts again at the end.
    pub fn is_looping(&self) -> bool {
        self.inner.shared.lock().looping
    }

    /// The time of the frame showing (zero before the first).
    pub fn position(&self) -> Duration {
        self.inner
            .shared
            .lock()
            .frame
            .as_ref()
            .map_or(Duration::ZERO, |f| f.timestamp)
    }

    /// How long it is, where the file says.
    pub fn duration(&self) -> Option<Duration> {
        self.inner.shared.lock().duration
    }

    /// The frames' size in pixels (zero until known).
    pub fn size(&self) -> PhysicalSize {
        self.inner.shared.lock().size
    }

    /// Where it stands.
    pub fn status(&self) -> VideoStatus {
        self.inner.shared.lock().status.clone()
    }

    /// What decodes it: `"VideoToolbox"`, `"VA-API"`, `"Media Foundation"`,
    /// `"MediaCodec"`, `"software"` (oxideav's MPEG-1/2 decoder) or
    /// `"browser"`.
    pub fn decoder(&self) -> &'static str {
        self.inner.shared.lock().decoder
    }

    /// The frame showing, once there's one.
    pub fn frame(&self) -> Option<VideoFrame> {
        self.inner.shared.lock().frame.clone()
    }

    /// Hear each frame as it's shown, on the player's thread (the page's
    /// thread on the web), replacing the last callback. Return quickly, and
    /// don't call the video's methods from it.
    pub fn on_frame(&self, f: impl FnMut(&VideoFrame) + Send + 'static) {
        self.inner.shared.lock().on_frame = Some(Box::new(f));
    }

    /// Repaint with `wake` (a view's canvas `id`) as frames come.
    pub(crate) fn set_wake(&self, id: u32, wake: WakeFn) {
        let mut st = self.inner.shared.lock();
        st.wakes.retain(|(i, _)| *i != id);
        st.wakes.push((id, wake));
    }
}

/// Where a video's bytes come from.
#[cfg_attr(any(not(feature = "video"), target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum Source {
    Bytes(Arc<[u8]>),
    File(std::path::PathBuf),
}
