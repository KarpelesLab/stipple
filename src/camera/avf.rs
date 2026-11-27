//! The camera on iOS and macOS, through AVFoundation over the raw
//! Objective-C runtime: an `AVCaptureSession` with an `AVCaptureDeviceInput`
//! and an `AVCaptureVideoDataOutput` asking for NV12 (`420f`, full range),
//! whose sample-buffer delegate (a class built at run time) gets each
//! `CMSampleBuffer` on a serial dispatch queue of its own. The frame's
//! `CVPixelBuffer` is locked read-only and its two planes handed to the app
//! as they are, without a copy.
//!
//! AVFoundation, CoreMedia and CoreVideo are loaded when the camera is first
//! used (`dlopen`), so apps that never use it don't link them.
//!
//! On an iPhone the back lens prefers the virtual triple or dual-wide camera
//! (which switches to the ultra-wide or macro lens by itself when focusing
//! close), with zoom 1 at the wide lens: the zoom range then starts below 1,
//! at the ultra-wide. The sensors are mounted a quarter turn from the
//! portrait screen, and the front camera's buffers come mirrored already
//! (Vision's `.leftMirrored`).

use super::{CameraError, CameraOptions, Caps, Frame, FrameFormat, Lens, Plane, Session, Sink};
use crate::geometry::{PhysicalSize, Point};
use crate::platform::native::objc::{
    Id, Sel, class, load_framework, msg, nsstring, rust_string, sel, symbol, with_pool,
};
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[repr(C)]
#[derive(Clone, Copy)]
struct CmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

/// The C functions and constants used, resolved when first needed.
struct Lib {
    av: crate::platform::native::objc::Framework,
    image_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    timestamp: unsafe extern "C" fn(*mut c_void) -> CmTime,
    lock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    unlock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    width: unsafe extern "C" fn(*mut c_void) -> usize,
    height: unsafe extern "C" fn(*mut c_void) -> usize,
    plane_count: unsafe extern "C" fn(*mut c_void) -> usize,
    plane_base: unsafe extern "C" fn(*mut c_void, usize) -> *mut u8,
    plane_stride: unsafe extern "C" fn(*mut c_void, usize) -> usize,
    plane_height: unsafe extern "C" fn(*mut c_void, usize) -> usize,
    /// `CVPixelBufferGetIOSurface`: the buffer's `IOSurface`, handed to a
    /// GPU renderer that imports it.
    io_surface: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    /// `kCVPixelBufferPixelFormatTypeKey`.
    format_key: usize,
    /// `kCVPixelBufferWidthKey` and `kCVPixelBufferHeightKey` (0 if missing).
    width_key: usize,
    height_key: usize,
}

// Each symbol's type is its field's.
#[allow(clippy::missing_transmute_annotations)]
fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let av = load_framework("AVFoundation");
        let cm = load_framework("CoreMedia");
        let cv = load_framework("CoreVideo");
        if av.is_null() || cm.is_null() || cv.is_null() {
            return None;
        }
        macro_rules! f {
            ($h:expr, $name:literal) => {{
                let p = symbol($h, $name);
                if p.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, _>(p)
            }};
        }
        let key = symbol(cv, c"kCVPixelBufferPixelFormatTypeKey") as *const Id;
        if key.is_null() {
            return None;
        }
        Some(Lib {
            av,
            image_buffer: f!(cm, c"CMSampleBufferGetImageBuffer"),
            timestamp: f!(cm, c"CMSampleBufferGetPresentationTimeStamp"),
            lock: f!(cv, c"CVPixelBufferLockBaseAddress"),
            unlock: f!(cv, c"CVPixelBufferUnlockBaseAddress"),
            width: f!(cv, c"CVPixelBufferGetWidth"),
            height: f!(cv, c"CVPixelBufferGetHeight"),
            plane_count: f!(cv, c"CVPixelBufferGetPlaneCount"),
            plane_base: f!(cv, c"CVPixelBufferGetBaseAddressOfPlane"),
            plane_stride: f!(cv, c"CVPixelBufferGetBytesPerRowOfPlane"),
            plane_height: f!(cv, c"CVPixelBufferGetHeightOfPlane"),
            io_surface: f!(cv, c"CVPixelBufferGetIOSurface"),
            format_key: *key as usize,
            width_key: {
                let k = symbol(cv, c"kCVPixelBufferWidthKey") as *const Id;
                if k.is_null() { 0 } else { *k as usize }
            },
            height_key: {
                let k = symbol(cv, c"kCVPixelBufferHeightKey") as *const Id;
                if k.is_null() { 0 } else { *k as usize }
            },
        })
    })
    .as_ref()
}

/// The AVFoundation string constant `name` (null where this OS lacks it).
fn constant(lib: &Lib, name: &CStr) -> Id {
    let p = symbol(lib.av, name) as *const Id;
    if p.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { *p }
    }
}

fn video(lib: &Lib) -> Id {
    let v = constant(lib, c"AVMediaTypeVideo");
    if v.is_null() { nsstring("vide") } else { v }
}

fn equal(a: Id, b: Id) -> bool {
    !a.is_null() && !b.is_null() && unsafe { msg!(a, "isEqual:", (b: Id) -> bool) }
}

fn responds(obj: Id, name: &str) -> bool {
    let s = sel(name);
    unsafe { msg!(obj, "respondsToSelector:", (s: Sel) -> bool) }
}

/// What kind of camera `device` is.
fn classify(lib: &Lib, device: Id) -> Lens {
    if cfg!(target_os = "ios") {
        match unsafe { msg!(device, "position", () -> isize) } {
            1 => Lens::Back,
            2 => Lens::Front,
            _ => Lens::External,
        }
    } else {
        // A Mac's built-in camera faces the user; the rest are plugged in
        // (or an iPhone as a Continuity Camera).
        let ty = unsafe { msg!(device, "deviceType", () -> Id) };
        if equal(
            ty,
            constant(lib, c"AVCaptureDeviceTypeBuiltInWideAngleCamera"),
        ) {
            Lens::Front
        } else {
            Lens::External
        }
    }
}

/// The video devices there are, with their kind.
fn devices(lib: &Lib) -> Vec<(Id, Lens)> {
    let cls = crate::platform::permission::apple::capture_device_class();
    if cls.is_null() {
        return Vec::new();
    }
    let media = video(lib);
    let list = unsafe { msg!(cls, "devicesWithMediaType:", (media: Id) -> Id) };
    if list.is_null() {
        return Vec::new();
    }
    let n = unsafe { msg!(list, "count", () -> usize) };
    (0..n)
        .map(|i| unsafe { msg!(list, "objectAtIndex:", (i: usize) -> Id) })
        .filter(|d| !d.is_null())
        .map(|d| (d, classify(lib, d)))
        .collect()
}

/// Whether Stipple captures from cameras here.
pub(super) const SUPPORTED: bool = true;

pub(super) fn lenses() -> Vec<Lens> {
    let Some(lib) = lib() else {
        return Vec::new();
    };
    with_pool(|| {
        let mut out = Vec::new();
        for (_, lens) in devices(lib) {
            if !out.contains(&lens) {
                out.push(lens);
            }
        }
        out
    })
}

/// The device to open for `lens`, the lens it is, and the zoom factor that
/// counts as 1 (a virtual device's switch to its wide lens).
fn choose(lib: &Lib, lens: Lens) -> Option<(Id, Lens, f64)> {
    if cfg!(target_os = "ios") && lens == Lens::Back {
        let cls = crate::platform::permission::apple::capture_device_class();
        let media = video(lib);
        for (name, virtual_) in [
            (c"AVCaptureDeviceTypeBuiltInTripleCamera", true),
            (c"AVCaptureDeviceTypeBuiltInDualWideCamera", true),
            (c"AVCaptureDeviceTypeBuiltInWideAngleCamera", false),
        ] {
            let ty = constant(lib, name);
            if ty.is_null() {
                continue;
            }
            let back: isize = 1;
            let device = unsafe {
                msg!(cls, "defaultDeviceWithDeviceType:mediaType:position:",
                    (ty: Id, media: Id, back: isize) -> Id)
            };
            if device.is_null() {
                continue;
            }
            let mut base = 1.0;
            if virtual_ && responds(device, "virtualDeviceSwitchOverVideoZoomFactors") {
                let factors =
                    unsafe { msg!(device, "virtualDeviceSwitchOverVideoZoomFactors", () -> Id) };
                if !factors.is_null() && unsafe { msg!(factors, "count", () -> usize) } > 0 {
                    let zero = 0usize;
                    let first = unsafe { msg!(factors, "objectAtIndex:", (zero: usize) -> Id) };
                    base = unsafe { msg!(first, "doubleValue", () -> f64) }.max(1.0);
                }
            }
            return Some((device, Lens::Back, base));
        }
    }
    let all = devices(lib);
    all.iter()
        .find(|(_, l)| *l == lens)
        .or_else(|| all.iter().find(|(_, l)| *l == Lens::Front))
        .or_else(|| all.first())
        .map(|&(d, l)| (d, l, 1.0))
}

/// The session preset closest to `want` (at least as big, else the
/// biggest), that the session accepts, and its frame size (none for the
/// fallback).
fn preset(lib: &Lib, session: Id, want: PhysicalSize) -> (Id, Option<(u32, u32)>) {
    let (long, short) = (want.width.max(want.height), want.width.min(want.height));
    let presets: [(u32, u32, &CStr); 4] = [
        (640, 480, c"AVCaptureSessionPreset640x480"),
        (1280, 720, c"AVCaptureSessionPreset1280x720"),
        (1920, 1080, c"AVCaptureSessionPreset1920x1080"),
        (3840, 2160, c"AVCaptureSessionPreset3840x2160"),
    ];
    let start = presets
        .iter()
        .position(|&(w, h, _)| w >= long && h >= short)
        .unwrap_or(presets.len() - 1);
    // From the best fit down, then up.
    let order = (0..=start).rev().chain(start + 1..presets.len());
    for i in order {
        let p = constant(lib, presets[i].2);
        if !p.is_null() && unsafe { msg!(session, "canSetSessionPreset:", (p: Id) -> bool) } {
            return (p, Some((presets[i].0, presets[i].1)));
        }
    }
    (constant(lib, c"AVCaptureSessionPresetHigh"), None)
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getProtocol(name: *const c_char) -> *mut c_void;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn class_addProtocol(cls: Id, protocol: *mut c_void) -> bool;
    fn class_createInstance(cls: Id, extra: usize) -> Id;
    fn object_getIndexedIvars(obj: Id) -> *mut c_void;
}

// libdispatch, in libSystem.
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_sync_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_release(object: *mut c_void);
}

/// What the delegate needs per frame (behind the delegate's extra bytes).
struct Ctx {
    sink: Arc<Sink>,
    sensor: u32,
    front: bool,
    raw_mirrored: bool,
}

/// The delegate class, made once.
fn delegate_class() -> Id {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| unsafe {
        let cls = objc_allocateClassPair(class("NSObject"), c"StippleCameraDelegate".as_ptr(), 0);
        if cls.is_null() {
            return 0;
        }
        class_addMethod(
            cls,
            sel("captureOutput:didOutputSampleBuffer:fromConnection:"),
            did_output as *const c_void,
            c"v@:@^v@".as_ptr(),
        );
        let protocol = objc_getProtocol(c"AVCaptureVideoDataOutputSampleBufferDelegate".as_ptr());
        if !protocol.is_null() {
            class_addProtocol(cls, protocol);
        }
        objc_registerClassPair(cls);
        cls as usize
    }) as Id
}

/// `captureOutput:didOutputSampleBuffer:fromConnection:`, on the capture
/// queue: hand the frame's planes over while the buffer is locked.
extern "C" fn did_output(this: Id, _cmd: Sel, _output: Id, buffer: *mut c_void, _conn: Id) {
    let Some(lib) = lib() else {
        return;
    };
    let ctx = unsafe { &*(*(object_getIndexedIvars(this) as *const *const Ctx)) };
    let delivered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let pixels = (lib.image_buffer)(buffer);
        if pixels.is_null() || (lib.plane_count)(pixels) < 2 {
            return;
        }
        const READ_ONLY: u64 = 1;
        if (lib.lock)(pixels, READ_ONLY) != 0 {
            return;
        }
        let (w, h) = ((lib.width)(pixels), (lib.height)(pixels));
        let plane = |i: usize| -> (&[u8], usize) {
            let base = (lib.plane_base)(pixels, i);
            let stride = (lib.plane_stride)(pixels, i);
            let rows = (lib.plane_height)(pixels, i);
            if base.is_null() {
                (&[][..], stride)
            } else {
                (std::slice::from_raw_parts(base, stride * rows), stride)
            }
        };
        let (y, y_stride) = plane(0);
        let (uv, uv_stride) = plane(1);
        if !y.is_empty() && uv.len() > 1 {
            let planes = [
                Plane::new(y, y_stride, 1),
                Plane::new(uv, uv_stride, 2),
                Plane::new(&uv[1..], uv_stride, 2),
            ];
            let t = (lib.timestamp)(buffer);
            let timestamp = if t.timescale > 0 && t.value >= 0 {
                Duration::from_nanos(
                    (i128::from(t.value) * 1_000_000_000 / i128::from(t.timescale)) as u64,
                )
            } else {
                Duration::ZERO
            };
            let (rotation, mirrored) = ctx.sink.orient(ctx.sensor, ctx.front, ctx.raw_mirrored);
            // The buffer itself, for a renderer that imports its IOSurface:
            // kept out of the capture pool while the preview holds it.
            let mut external = || {
                let surface = (lib.io_surface)(pixels);
                let image = crate::render::ExternalImage::io_surface(surface).ok()?;
                crate::platform::external::apple::CFRetain(pixels);
                let held = pixels as usize;
                Some(image.on_release(move || {
                    crate::platform::external::apple::CFRelease(held as *mut c_void)
                }))
            };
            ctx.sink.deliver_with(
                &Frame {
                    size: PhysicalSize::new(w as u32, h as u32),
                    format: FrameFormat::Yuv420,
                    planes: &planes,
                    timestamp,
                    rotation,
                    mirrored,
                },
                Some((crate::render::ExternalFormat::Nv12, &mut external)),
            );
        }
        (lib.unlock)(pixels, READ_ONLY);
    }));
    if delivered.is_err() {
        eprintln!("stipple: the camera's frame callback panicked");
    }
}

/// An open AVFoundation camera.
struct AvSession {
    session: Id,
    device: Id,
    output: Id,
    delegate: Id,
    queue: *mut c_void,
    ctx: *mut Ctx,
    /// The device zoom factor the app's 1 is.
    base_zoom: f64,
}

// The objects are only messaged under the camera's lock; AVFoundation's
// session and device are thread-safe to configure.
unsafe impl Send for AvSession {}

fn error_text(err: Id, what: &str) -> CameraError {
    let text = (!err.is_null())
        .then(|| rust_string(unsafe { msg!(err, "localizedDescription", () -> Id) }))
        .flatten();
    CameraError::Os(text.unwrap_or_else(|| what.to_string()))
}

impl AvSession {
    /// Run `f` with the device locked for configuration.
    fn configure(&self, f: impl FnOnce(Id)) -> Result<(), CameraError> {
        with_pool(|| unsafe {
            let mut err: Id = std::ptr::null_mut();
            let errp: *mut Id = &mut err;
            if !msg!(self.device, "lockForConfiguration:", (errp: *mut Id) -> bool) {
                return Err(error_text(err, "can't configure the camera"));
            }
            f(self.device);
            msg!(self.device, "unlockForConfiguration", () -> ());
            Ok(())
        })
    }
}

impl Session for AvSession {
    fn start(&mut self) -> Result<(), CameraError> {
        with_pool(|| unsafe { msg!(self.session, "startRunning", () -> ()) });
        Ok(())
    }

    fn stop(&mut self) {
        with_pool(|| unsafe { msg!(self.session, "stopRunning", () -> ()) });
    }

    fn set_torch(&mut self, on: bool) -> Result<(), CameraError> {
        let mode: isize = isize::from(on);
        self.configure(|d| unsafe {
            if msg!(d, "isTorchModeSupported:", (mode: isize) -> bool) {
                msg!(d, "setTorchMode:", (mode: isize) -> ());
            }
        })
    }

    fn set_zoom(&mut self, factor: f32) {
        if !responds(self.device, "setVideoZoomFactor:") {
            return;
        }
        let base = self.base_zoom;
        let _ = self.configure(|d| unsafe {
            let (lo, hi) = zoom_bounds(d);
            let z = (f64::from(factor) * base).clamp(lo, hi);
            msg!(d, "setVideoZoomFactor:", (z: f64) -> ());
        });
    }

    fn focus_at(&mut self, point: Point) {
        let p = CgPoint {
            x: point.x,
            y: point.y,
        };
        let _ = self.configure(|d| unsafe {
            // AVCaptureFocusModeContinuousAutoFocus,
            // AVCaptureExposureModeContinuousAutoExposure.
            let continuous: isize = 2;
            if msg!(d, "isFocusPointOfInterestSupported", () -> bool) {
                msg!(d, "setFocusPointOfInterest:", (p: CgPoint) -> ());
                if msg!(d, "isFocusModeSupported:", (continuous: isize) -> bool) {
                    msg!(d, "setFocusMode:", (continuous: isize) -> ());
                }
            }
            if msg!(d, "isExposurePointOfInterestSupported", () -> bool) {
                msg!(d, "setExposurePointOfInterest:", (p: CgPoint) -> ());
                if msg!(d, "isExposureModeSupported:", (continuous: isize) -> bool) {
                    msg!(d, "setExposureMode:", (continuous: isize) -> ());
                }
            }
        });
    }
}

/// The device's zoom factors, where it zooms.
unsafe fn zoom_bounds(device: Id) -> (f64, f64) {
    unsafe {
        let lo = if responds(device, "minAvailableVideoZoomFactor") {
            msg!(device, "minAvailableVideoZoomFactor", () -> f64)
        } else {
            1.0
        };
        let hi = if responds(device, "maxAvailableVideoZoomFactor") {
            msg!(device, "maxAvailableVideoZoomFactor", () -> f64)
        } else {
            let format = msg!(device, "activeFormat", () -> Id);
            if !format.is_null() && responds(format, "videoMaxZoomFactor") {
                msg!(format, "videoMaxZoomFactor", () -> f64)
            } else {
                1.0
            }
        };
        (lo.max(0.5), hi.max(lo))
    }
}

extern "C" fn nothing(_: *mut c_void) {}

impl Drop for AvSession {
    fn drop(&mut self) {
        with_pool(|| unsafe {
            msg!(self.session, "stopRunning", () -> ());
            let none: Id = std::ptr::null_mut();
            msg!(self.output, "setSampleBufferDelegate:queue:", (none: Id, none: Id) -> ());
            // A frame being delivered finishes before the context goes.
            dispatch_sync_f(self.queue, std::ptr::null_mut(), nothing);
            drop(Box::from_raw(self.ctx));
            msg!(self.delegate, "release", () -> ());
            msg!(self.output, "release", () -> ());
            msg!(self.session, "release", () -> ());
            msg!(self.device, "release", () -> ());
            dispatch_release(self.queue);
        });
    }
}

pub(super) fn open(
    sink: Arc<Sink>,
    options: &CameraOptions,
) -> Result<(Box<dyn Session>, Caps), CameraError> {
    let lib = lib().ok_or(CameraError::NoCamera)?;
    let cls = delegate_class();
    if cls.is_null() {
        return Err(CameraError::Os("can't make the capture delegate".into()));
    }
    with_pool(|| unsafe {
        let (device, lens, base_zoom) = choose(lib, options.lens).ok_or(CameraError::NoCamera)?;
        msg!(device, "retain", () -> Id);
        let session = msg!(msg!(class("AVCaptureSession"), "alloc", () -> Id), "init", () -> Id);
        let fail = |e: CameraError| {
            msg!(session, "release", () -> ());
            msg!(device, "release", () -> ());
            Err(e)
        };
        msg!(session, "beginConfiguration", () -> ());
        let mut err: Id = std::ptr::null_mut();
        let errp: *mut Id = &mut err;
        let input = msg!(class("AVCaptureDeviceInput"), "deviceInputWithDevice:error:",
            (device: Id, errp: *mut Id) -> Id);
        if input.is_null() {
            return fail(error_text(err, "can't open the camera"));
        }
        if !msg!(session, "canAddInput:", (input: Id) -> bool) {
            return fail(CameraError::Os("the session refused the camera".into()));
        }
        msg!(session, "addInput:", (input: Id) -> ());
        // With the input in: which presets it takes depends on the device.
        let (p, size) = preset(lib, session, options.resolution);
        if !p.is_null() {
            msg!(session, "setSessionPreset:", (p: Id) -> ());
        }
        let output =
            msg!(msg!(class("AVCaptureVideoDataOutput"), "alloc", () -> Id), "init", () -> Id);
        // kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, '420f'.
        let format = 0x3432_3066u32;
        let nv12 = msg!(class("NSNumber"), "numberWithUnsignedInt:", (format: u32) -> Id);
        let key = lib.format_key as Id;
        let settings = msg!(class("NSMutableDictionary"), "dictionary", () -> Id);
        msg!(settings, "setObject:forKey:", (nv12: Id, key: Id) -> ());
        // A Mac's camera keeps its native size whatever the preset: have the
        // output scale to the preset's.
        if cfg!(target_os = "macos")
            && let Some((w, h)) = size
            && lib.width_key != 0
            && lib.height_key != 0
        {
            let (w, h) = (w as isize, h as isize);
            let w = msg!(class("NSNumber"), "numberWithInteger:", (w: isize) -> Id);
            let h = msg!(class("NSNumber"), "numberWithInteger:", (h: isize) -> Id);
            let (wk, hk) = (lib.width_key as Id, lib.height_key as Id);
            msg!(settings, "setObject:forKey:", (w: Id, wk: Id) -> ());
            msg!(settings, "setObject:forKey:", (h: Id, hk: Id) -> ());
        }
        msg!(output, "setVideoSettings:", (settings: Id) -> ());
        let yes = true;
        msg!(output, "setAlwaysDiscardsLateVideoFrames:", (yes: bool) -> ());
        let queue = dispatch_queue_create(c"org.stipple.camera".as_ptr(), std::ptr::null());
        let front = lens == Lens::Front;
        let ctx = Box::into_raw(Box::new(Ctx {
            sink,
            sensor: if cfg!(target_os = "ios") { 90 } else { 0 },
            front,
            raw_mirrored: cfg!(target_os = "ios") && front,
        }));
        let delegate = class_createInstance(cls, std::mem::size_of::<*const Ctx>());
        *(object_getIndexedIvars(delegate) as *mut *const Ctx) = ctx;
        msg!(output, "setSampleBufferDelegate:queue:", (delegate: Id, queue: *mut c_void) -> ());
        if !msg!(session, "canAddOutput:", (output: Id) -> bool) {
            msg!(session, "commitConfiguration", () -> ());
            drop(AvSession {
                session,
                device,
                output,
                delegate,
                queue,
                ctx,
                base_zoom,
            });
            return Err(CameraError::Os(
                "the session refused the video output".into(),
            ));
        }
        msg!(session, "addOutput:", (output: Id) -> ());
        msg!(session, "commitConfiguration", () -> ());
        let s = AvSession {
            session,
            device,
            output,
            delegate,
            queue,
            ctx,
            base_zoom,
        };
        let zoom = if responds(device, "setVideoZoomFactor:") {
            let (lo, hi) = zoom_bounds(device);
            // Up to ten times the app's 1.
            (
                (lo / base_zoom) as f32,
                ((hi.min(base_zoom * 10.0)) / base_zoom) as f32,
            )
        } else {
            (1.0, 1.0)
        };
        let caps = Caps {
            torch: msg!(device, "hasTorch", () -> bool),
            focus: msg!(device, "isFocusPointOfInterestSupported", () -> bool),
            zoom,
            lens,
        };
        // Focus continuously from the start, and start at the app's 1.
        let _ = s.configure(|d| {
            let continuous: isize = 2;
            if msg!(d, "isFocusModeSupported:", (continuous: isize) -> bool) {
                msg!(d, "setFocusMode:", (continuous: isize) -> ());
            }
            if base_zoom != 1.0 && responds(d, "setVideoZoomFactor:") {
                msg!(d, "setVideoZoomFactor:", (base_zoom: f64) -> ());
            }
        });
        Ok((Box::new(s) as Box<dyn Session>, caps))
    })
}

/// Degrees the screen's content is turned counterclockwise from portrait:
/// the interface orientation, on iOS (from the main thread).
pub(super) fn display_rotation() -> u32 {
    #[cfg(target_os = "ios")]
    {
        if !crate::platform::native::objc::on_main_thread() {
            return 0;
        }
        let app = unsafe { msg!(class("UIApplication"), "sharedApplication", () -> Id) };
        if app.is_null() {
            return 0;
        }
        // UIInterfaceOrientation: the home button's side.
        return match unsafe { msg!(app, "statusBarOrientation", () -> isize) } {
            2 => 180,
            3 => 90,
            4 => 270,
            _ => 0,
        };
    }
    #[allow(unreachable_code)]
    0
}
