//! The camera: a live preview in the layout, every frame handed to the app on
//! a capture thread (for a barcode decoder), the torch, zoom, tap to focus
//! and switching lenses.
//!
//! ```no_run
//! use stipple::camera::{Camera, CameraOptions, Lens, camera_preview};
//! use stipple::prelude::*;
//!
//! struct State {
//!     camera: Option<Camera>,
//! }
//!
//! fn view(state: &State, cx: &mut Cx<State>) -> Element {
//!     match &state.camera {
//!         Some(camera) => camera_preview(cx, camera),
//!         None => label(cx, "No camera"),
//!     }
//! }
//!
//! // After the permission is granted (`platform::permission`):
//! let camera = Camera::open(CameraOptions::new().lens(Lens::Back)).unwrap();
//! camera.on_frame(|frame| {
//!     // On the capture thread: the luminance plane, borrowed, not copied.
//!     if let Some(y) = frame.luma() {
//!         let _ = (y.data, y.row_stride, frame.size);
//!     }
//! });
//! # let _ = State { camera: Some(camera) };
//! ```
//!
//! - [`Camera::open`] starts capturing from a lens, at about the resolution
//!   asked for. It needs the [camera permission](crate::platform::permission)
//!   first. [`stop`](Camera::stop) and [`start`](Camera::start) pause and
//!   resume it; it also pauses by itself while the app is in the background,
//!   and closes when the last clone is dropped.
//! - [`Camera::on_frame`] hears every frame, on the capture thread, as a
//!   [`Frame`]: YUV 4:2:0 planes on phones and macOS, RGBA on the web, with
//!   their strides, a timestamp and the rotation that turns them upright.
//! - [`camera_preview`] is the viewfinder: an element showing the latest
//!   frame, filling its box (cropped to cover it).
//!   [`Camera::preview_mapping`] maps frame coordinates to it and back, to
//!   draw over what a decoder found or to focus where the user tapped.
//! - [`set_torch`](Camera::set_torch), [`set_zoom`](Camera::set_zoom) (within
//!   [`zoom_range`](Camera::zoom_range)), [`focus_at`](Camera::focus_at) and
//!   [`set_lens`](Camera::set_lens) control it while it runs.
//!
//! | | Capture | Frames |
//! |---|---|---|
//! | iOS | `AVCaptureSession` with an `AVCaptureVideoDataOutput` | NV12 (`CVPixelBuffer`, locked, not copied) |
//! | macOS | the same | NV12 |
//! | Android | Camera2 through the NDK (`ACameraManager`, `ACaptureSession`) into an `AImageReader` | `YUV_420_888` (`AImage` planes, not copied) |
//! | Web | `getUserMedia`, a `<video>` drawn into a canvas each frame | RGBA, on the page's thread |
//! | Windows, Linux | not yet: [`Camera::open`] says [`Unsupported`](CameraError::Unsupported) | |
//!
//! **The preview's cost.** The first version draws the preview through the
//! renderer: each frame is converted on the capture thread to an upright RGBA
//! image no bigger than the element (and at most 960 pixels on its long
//! side), then the element repaints alone, without rebuilding the view
//! ([`AppHandle::invalidate_canvas`](crate::core::AppHandle::invalidate_canvas)).
//! Frames that arrive before the last one was painted aren't converted,
//! so the preview runs at whatever rate the renderer keeps up with, while
//! [`on_frame`](Camera::on_frame) still sees every frame. A zero-copy path (the
//! camera's buffer as a GPU texture) is on the roadmap. See `docs/camera.md`.

mod frame;
mod preview;

pub use frame::{Frame, FrameFormat, Plane, PreviewMapping};
pub use preview::camera_preview;

use crate::geometry::{PhysicalSize, Point, Size};
use crate::render::Pixmap;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(unsafe_code)]
#[path = "avf.rs"]
mod os;

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[path = "android.rs"]
mod os;

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
pub(crate) mod os;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_arch = "wasm32"
)))]
#[path = "none.rs"]
mod os;

/// Which camera.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lens {
    /// The main camera on the back of a phone or tablet.
    #[default]
    Back,
    /// The camera facing the user (a laptop's built-in one). Its preview is
    /// mirrored.
    Front,
    /// A camera plugged in (a USB webcam on a Mac or an Android device).
    External,
}

/// What [`Camera::open`] opens.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraOptions {
    /// The lens; [`Lens::Back`] by default. Where there's none of that kind
    /// (a Mac has no back camera), the one facing the user, else the first.
    pub lens: Lens,
    /// The frame size to aim for, in pixels as the sensor delivers them
    /// (landscape on a phone: 1920×1080 is about 1080p): the closest the
    /// camera offers is chosen. 1280×720 by default.
    pub resolution: PhysicalSize,
}

impl Default for CameraOptions {
    fn default() -> Self {
        Self {
            lens: Lens::Back,
            resolution: PhysicalSize::new(1280, 720),
        }
    }
}

impl CameraOptions {
    /// The back lens at 1280×720.
    pub fn new() -> Self {
        Self::default()
    }

    /// Choose the lens.
    pub fn lens(mut self, lens: Lens) -> Self {
        self.lens = lens;
        self
    }

    /// Choose the frame size to aim for.
    pub fn resolution(mut self, width: u32, height: u32) -> Self {
        self.resolution = PhysicalSize::new(width, height);
        self
    }
}

/// Why the camera couldn't be opened or controlled.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CameraError {
    /// Stipple has no camera support on this platform (Windows, Linux).
    Unsupported,
    /// There's no camera (the iOS simulator), or none for the lens asked for.
    NoCamera,
    /// The user didn't grant the camera
    /// [permission](crate::platform::permission::Permission::Camera).
    NotPermitted,
    /// The camera can't do that (no torch, no focus point).
    NotAvailable,
    /// The system refused, or the camera failed (another app has it, it was
    /// unplugged).
    Os(String),
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("no camera support on this platform"),
            Self::NoCamera => f.write_str("no camera"),
            Self::NotPermitted => f.write_str("the camera permission isn't granted"),
            Self::NotAvailable => f.write_str("the camera can't do that"),
            Self::Os(e) => write!(f, "camera error: {e}"),
        }
    }
}

impl std::error::Error for CameraError {}

/// Where a [`Camera`] stands.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CameraStatus {
    /// Opened, frames not flowing yet (the web's prompt may be up).
    Starting,
    /// Frames are coming.
    Running,
    /// Paused while the app is in the background.
    Paused,
    /// Stopped by [`Camera::stop`].
    Stopped,
    /// It stopped working.
    Failed(CameraError),
}

/// What a camera can do, as its backend found out when it opened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caps {
    pub(crate) torch: bool,
    pub(crate) focus: bool,
    pub(crate) zoom: (f32, f32),
    /// The lens it actually opened.
    pub(crate) lens: Lens,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            torch: false,
            focus: false,
            zoom: (1.0, 1.0),
            lens: Lens::Back,
        }
    }
}

/// A platform's open camera.
pub(crate) trait Session: Send {
    fn start(&mut self) -> Result<(), CameraError>;
    fn stop(&mut self);
    fn set_torch(&mut self, on: bool) -> Result<(), CameraError>;
    fn set_zoom(&mut self, factor: f32);
    /// Focus (and expose) at `point`, in `0..1` of the frame as delivered.
    fn focus_at(&mut self, point: Point);
}

type FrameFn = Box<dyn FnMut(&Frame<'_>) + Send>;
type WakeFn = Box<dyn Fn() + Send + Sync>;

/// The preview's latest image and who repaints it.
#[derive(Default)]
struct Preview {
    image: Option<Arc<Pixmap>>,
    /// Converted but not painted yet: the next frame isn't converted.
    fresh: bool,
    /// The element's physical size, as last painted.
    wanted: PhysicalSize,
    /// Repaints the preview element (set by [`camera_preview`]).
    wake: Option<WakeFn>,
}

/// What the capture thread shares with the camera's handle and preview:
/// what a backend delivers frames to.
pub(crate) struct Sink {
    on_frame: Mutex<Option<FrameFn>>,
    preview: Mutex<Preview>,
    /// Degrees the screen's content is turned counterclockwise from the
    /// device's natural orientation (Android's `Display.getRotation`).
    display_rotation: AtomicU32,
    frames: AtomicU64,
    /// The last frame's size, rotation and mirroring.
    last: Mutex<Option<(PhysicalSize, u32, bool)>>,
    status: Mutex<CameraStatus>,
    /// What the camera can do (the web finds out once the stream starts).
    caps: Mutex<Caps>,
}

impl Sink {
    fn new() -> Self {
        Self {
            on_frame: Mutex::new(None),
            preview: Mutex::new(Preview::default()),
            display_rotation: AtomicU32::new(0),
            frames: AtomicU64::new(0),
            last: Mutex::new(None),
            status: Mutex::new(CameraStatus::Starting),
            caps: Mutex::new(Caps::default()),
        }
    }

    pub(crate) fn caps(&self) -> Caps {
        *self.caps.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the camera turned out to be able to do.
    pub(crate) fn set_caps(&self, caps: Caps) {
        *self.caps.lock().unwrap_or_else(|e| e.into_inner()) = caps;
    }

    /// The rotation (clockwise degrees to upright) and mirroring of a frame
    /// from a sensor mounted `sensor` degrees from the device's natural
    /// orientation (`ACAMERA_SENSOR_ORIENTATION`), facing the user or not;
    /// `raw_mirrored` when its buffers come mirrored already (the iOS front
    /// camera's), so the preview doesn't mirror them again.
    #[cfg_attr(
        not(any(target_os = "ios", target_os = "macos", target_os = "android")),
        allow(dead_code)
    )]
    pub(crate) fn orient(&self, sensor: u32, front: bool, raw_mirrored: bool) -> (u32, bool) {
        let display = self.display_rotation.load(Ordering::Relaxed) % 360;
        let rotation = if front {
            (sensor + display) % 360
        } else {
            (sensor + 360 - display) % 360
        };
        (rotation, front != raw_mirrored)
    }

    pub(crate) fn set_status(&self, status: CameraStatus) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
        self.wake_preview();
    }

    fn wake_preview(&self) {
        if let Some(wake) = &self.preview.lock().unwrap_or_else(|e| e.into_inner()).wake {
            wake();
        }
    }

    /// A frame arrived (on the capture thread): count it, hand it to the
    /// app, and convert it for the preview unless the last one is still to
    /// be painted.
    #[cfg_attr(
        not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "android",
            target_arch = "wasm32"
        )),
        allow(dead_code)
    )]
    pub(crate) fn deliver(&self, frame: &Frame<'_>) {
        let n = self.frames.fetch_add(1, Ordering::Relaxed);
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((frame.size, frame.rotation, frame.mirrored));
        if n == 0 {
            let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
            if *status == CameraStatus::Starting {
                *status = CameraStatus::Running;
            }
        }
        if let Some(f) = self
            .on_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            f(frame);
        }
        let wanted = {
            let p = self.preview.lock().unwrap_or_else(|e| e.into_inner());
            if p.fresh || p.wake.is_none() || p.wanted.width == 0 || p.wanted.height == 0 {
                return;
            }
            p.wanted
        };
        let image = Arc::new(frame.convert(frame::preview_size(frame.upright_size(), wanted)));
        let mut p = self.preview.lock().unwrap_or_else(|e| e.into_inner());
        p.image = Some(image);
        p.fresh = true;
        if let Some(wake) = &p.wake {
            wake();
        }
    }
}

/// The live cameras, for pausing them in the background.
static CAMERAS: Mutex<Vec<Weak<Inner>>> = Mutex::new(Vec::new());

/// The next camera's number (its preview's canvas id).
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

struct State {
    options: CameraOptions,
    session: Option<Box<dyn Session>>,
    /// The app wants it running (it's paused in the background).
    running: bool,
    background: bool,
    torch: bool,
    zoom: f32,
}

struct Inner {
    id: u32,
    sink: Arc<Sink>,
    state: Mutex<State>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if let Some(mut s) = state.session.take() {
            s.stop();
        }
    }
}

/// An open camera. Clones share it; it closes when the last one is dropped.
/// Its methods can be called from any thread.
#[derive(Clone)]
pub struct Camera {
    inner: Arc<Inner>,
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Camera")
            .field("id", &self.inner.id)
            .field("status", &self.status())
            .finish()
    }
}

impl PartialEq for Camera {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Camera {
    /// The kinds of camera this device has (empty where there's none, or no
    /// support: the iOS simulator, Windows, Linux).
    pub fn lenses() -> Vec<Lens> {
        os::lenses()
    }

    /// Open the camera `options` asks for and start capturing.
    ///
    /// Needs the camera [permission](crate::platform::permission) granted
    /// beforehand ([`CameraError::NotPermitted`] otherwise), except on the
    /// web, where the browser asks as the camera opens. On the web frames
    /// start once the user answered; [`status`](Self::status) says
    /// [`Starting`](CameraStatus::Starting) until then.
    pub fn open(options: CameraOptions) -> Result<Camera, CameraError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use crate::platform::permission::{self, Permission, PermissionStatus};
            if !os::SUPPORTED {
                return Err(CameraError::Unsupported);
            }
            if os::lenses().is_empty() {
                return Err(CameraError::NoCamera);
            }
            if permission::status(Permission::Camera) != PermissionStatus::Granted {
                return Err(CameraError::NotPermitted);
            }
        }
        let sink = Arc::new(Sink::new());
        let (mut session, caps) = os::open(sink.clone(), &options)?;
        sink.set_caps(caps);
        session.start()?;
        let inner = Arc::new(Inner {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            sink,
            state: Mutex::new(State {
                options: CameraOptions {
                    lens: caps.lens,
                    ..options
                },
                session: Some(session),
                running: true,
                background: false,
                torch: false,
                zoom: 1.0,
            }),
        });
        let mut cameras = CAMERAS.lock().unwrap_or_else(|e| e.into_inner());
        cameras.retain(|c| c.strong_count() > 0);
        cameras.push(Arc::downgrade(&inner));
        Ok(Camera { inner })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hear every frame, on the capture thread (the page's thread on the
    /// web), replacing the last callback. The frame borrows the camera's
    /// buffer, which the camera waits for: return quickly (a decoder that
    /// takes longer than a frame drops the frames in between), and copy
    /// what must outlive the call. Don't call the camera's methods from it.
    pub fn on_frame(&self, f: impl FnMut(&Frame<'_>) + Send + 'static) {
        *self
            .inner
            .sink
            .on_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// Where it stands.
    pub fn status(&self) -> CameraStatus {
        self.inner
            .sink
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Frames captured so far.
    pub fn frame_count(&self) -> u64 {
        self.inner.sink.frames.load(Ordering::Relaxed)
    }

    /// The size of the frames (before rotation), once one arrived.
    pub fn frame_size(&self) -> Option<PhysicalSize> {
        self.inner
            .sink
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|(size, _, _)| size)
    }

    /// The latest frame as the preview shows it (upright, at most the
    /// preview's size), once one was converted for a [`camera_preview`].
    pub fn preview_image(&self) -> Option<Arc<Pixmap>> {
        self.inner
            .sink
            .preview
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .image
            .clone()
    }

    /// Resume capturing after [`stop`](Self::stop).
    pub fn start(&self) -> Result<(), CameraError> {
        let mut state = self.state();
        state.running = true;
        if state.background {
            return Ok(());
        }
        let torch = state.torch;
        let session = state.session.as_mut().ok_or(CameraError::NoCamera)?;
        session.start()?;
        if torch {
            let _ = session.set_torch(true);
        }
        self.inner.sink.set_status(CameraStatus::Running);
        Ok(())
    }

    /// Stop capturing (the camera's light goes off), keeping it open.
    pub fn stop(&self) {
        let mut state = self.state();
        state.running = false;
        if let Some(s) = state.session.as_mut() {
            s.stop();
        }
        self.inner.sink.set_status(CameraStatus::Stopped);
    }

    /// Whether the app wants it running (it may still be paused in the
    /// background).
    pub fn is_running(&self) -> bool {
        self.state().running
    }

    /// The lens it's capturing from.
    pub fn lens(&self) -> Lens {
        self.inner.sink.caps().lens
    }

    /// Switch to another lens: the camera closes and opens that one, at the
    /// same resolution; the zoom goes back to 1 and the torch off.
    pub fn set_lens(&self, lens: Lens) -> Result<(), CameraError> {
        let mut state = self.state();
        if self.inner.sink.caps().lens == lens {
            return Ok(());
        }
        let options = state.options.lens(lens);
        if let Some(mut old) = state.session.take() {
            old.stop();
        }
        let opened = os::open(self.inner.sink.clone(), &options);
        let (mut session, caps) = match opened {
            Ok(s) => s,
            Err(e) => {
                // Back to the lens it had.
                let previous = state.options;
                if let Ok((mut s, caps)) = os::open(self.inner.sink.clone(), &previous) {
                    if state.running && !state.background {
                        let _ = s.start();
                    }
                    state.session = Some(s);
                    self.inner.sink.set_caps(caps);
                }
                return Err(e);
            }
        };
        *self
            .inner
            .sink
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let started = if state.running && !state.background {
            session.start()
        } else {
            Ok(())
        };
        state.options = options.lens(caps.lens);
        state.session = Some(session);
        self.inner.sink.set_caps(caps);
        state.torch = false;
        state.zoom = 1.0;
        started
    }

    /// Whether the lens has a torch (the flash, kept on).
    pub fn has_torch(&self) -> bool {
        self.inner.sink.caps().torch
    }

    /// Whether the torch is on.
    pub fn torch(&self) -> bool {
        self.state().torch
    }

    /// Turn the torch on or off. [`CameraError::NotAvailable`] if the lens
    /// has none.
    pub fn set_torch(&self, on: bool) -> Result<(), CameraError> {
        let mut state = self.state();
        if !self.inner.sink.caps().torch {
            return Err(CameraError::NotAvailable);
        }
        if let Some(s) = state.session.as_mut() {
            s.set_torch(on)?;
        }
        state.torch = on;
        Ok(())
    }

    /// The zoom factors the lens offers, from 1 (no zoom) up.
    pub fn zoom_range(&self) -> (f32, f32) {
        self.inner.sink.caps().zoom
    }

    /// The zoom factor.
    pub fn zoom(&self) -> f32 {
        self.state().zoom
    }

    /// Zoom by `factor` (clamped to [`zoom_range`](Self::zoom_range)).
    pub fn set_zoom(&self, factor: f32) {
        let mut state = self.state();
        let (lo, hi) = self.inner.sink.caps().zoom;
        let factor = if factor.is_finite() { factor } else { 1.0 }.clamp(lo, hi.max(lo));
        state.zoom = factor;
        if let Some(s) = state.session.as_mut() {
            s.set_zoom(factor);
        }
    }

    /// Whether the lens can focus on a point.
    pub fn can_focus(&self) -> bool {
        self.inner.sink.caps().focus
    }

    /// Focus (and set the exposure) at `point`, in `0..1` of the frame as
    /// delivered (before rotation: `(0, 0)` is the frame's top-left), then
    /// keep focusing continuously around it. [`CameraError::NotAvailable`]
    /// if the lens can't.
    pub fn focus_at(&self, point: Point) -> Result<(), CameraError> {
        let mut state = self.state();
        if !self.inner.sink.caps().focus {
            return Err(CameraError::NotAvailable);
        }
        let p = Point::new(point.x.clamp(0.0, 1.0), point.y.clamp(0.0, 1.0));
        if let Some(s) = state.session.as_mut() {
            s.focus_at(p);
        }
        Ok(())
    }

    /// [`focus_at`](Self::focus_at) the frame point shown at `point` of a
    /// preview of `preview` size: what a tap on the preview passes
    /// ([`PointerEvent::position`](crate::core::PointerEvent::position) and
    /// [`size`](crate::core::PointerEvent::size)).
    pub fn focus_at_preview(&self, point: Point, preview: Size) -> Result<(), CameraError> {
        let m = self
            .preview_mapping(preview)
            .ok_or(CameraError::NotAvailable)?;
        let p = m.to_frame(point);
        self.focus_at(Point::new(
            p.x / f64::from(m.frame.width.max(1)),
            p.y / f64::from(m.frame.height.max(1)),
        ))
    }

    /// How the frames map onto a [`camera_preview`] of `preview` size (its
    /// laid-out size), once a frame arrived.
    pub fn preview_mapping(&self, preview: Size) -> Option<PreviewMapping> {
        let last = *self
            .inner
            .sink
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        last.map(|(size, rotation, mirrored)| {
            PreviewMapping::new(size, rotation, mirrored, preview)
        })
    }
}

/// The app went to the background or came back: pause the cameras running,
/// or resume them.
pub(crate) fn set_app_background(background: bool) {
    let cameras: Vec<Arc<Inner>> = CAMERAS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for inner in cameras {
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.background == background {
            continue;
        }
        state.background = background;
        if !state.running {
            continue;
        }
        let torch = state.torch;
        let Some(session) = state.session.as_mut() else {
            continue;
        };
        if background {
            session.stop();
            inner.sink.set_status(CameraStatus::Paused);
        } else {
            match session.start() {
                Ok(()) => {
                    if torch {
                        let _ = session.set_torch(true);
                    }
                    inner.sink.set_status(CameraStatus::Running);
                }
                Err(e) => inner.sink.set_status(CameraStatus::Failed(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_follows_the_sensor_and_the_screen() {
        let sink = Sink::new();
        // A phone upright: the back sensor's 90° turns frames a quarter
        // clockwise; the front one's 270° the other way, mirrored.
        assert_eq!(sink.orient(90, false, false), (90, false));
        assert_eq!(sink.orient(270, true, false), (270, true));
        // iOS's front camera: mirrored already.
        assert_eq!(sink.orient(90, true, true), (90, false));
        // Turned a quarter counterclockwise (Surface.ROTATION_90).
        sink.display_rotation.store(90, Ordering::Relaxed);
        assert_eq!(sink.orient(90, false, false), (0, false));
        assert_eq!(sink.orient(270, true, false), (0, true));
        assert_eq!(sink.orient(90, true, true), (180, false));
    }

    #[test]
    fn frames_reach_the_app_and_the_preview() {
        let sink = Arc::new(Sink::new());
        let seen = Arc::new(AtomicU64::new(0));
        let s = seen.clone();
        *sink.on_frame.lock().unwrap() = Some(Box::new(move |f: &Frame<'_>| {
            assert_eq!(f.luma().unwrap().at(1, 0), Some(200));
            s.fetch_add(1, Ordering::Relaxed);
        }));
        let woken = Arc::new(AtomicU64::new(0));
        let w = woken.clone();
        {
            let mut p = sink.preview.lock().unwrap();
            p.wanted = PhysicalSize::new(2, 2);
            p.wake = Some(Box::new(move || {
                w.fetch_add(1, Ordering::Relaxed);
            }));
        }
        let y = [0u8, 200, 0, 200];
        let uv = [128u8, 128];
        let planes = [
            Plane::new(&y, 2, 1),
            Plane::new(&uv, 2, 2),
            Plane::new(&uv[1..], 2, 2),
        ];
        let frame = Frame {
            size: PhysicalSize::new(2, 2),
            format: FrameFormat::Yuv420,
            planes: &planes,
            timestamp: std::time::Duration::ZERO,
            rotation: 0,
            mirrored: false,
        };
        sink.deliver(&frame);
        sink.deliver(&frame);
        assert_eq!(seen.load(Ordering::Relaxed), 2);
        assert_eq!(sink.frames.load(Ordering::Relaxed), 2);
        // The second frame came before the first was painted: one image.
        assert_eq!(woken.load(Ordering::Relaxed), 1);
        assert_eq!(*sink.status.lock().unwrap(), CameraStatus::Running);
        let image = sink.preview.lock().unwrap().image.clone().unwrap();
        assert_eq!(image.pixel(1, 0), Some([200, 200, 200, 255]));
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn desktops_have_no_camera_yet() {
        assert!(Camera::lenses().is_empty());
        assert_eq!(
            Camera::open(CameraOptions::new()).unwrap_err(),
            CameraError::Unsupported
        );
    }
}
