//! The camera on Android, through the NDK's Camera2 (`libcamera2ndk.so`)
//! and `AImageReader` (`libmediandk.so`), with no Java: both libraries are
//! loaded when the camera is first used (`dlopen`, Android 7 and later), so
//! an app that never uses it doesn't link them.
//!
//! The camera device streams a repeating preview request into an
//! `AImageReader` of `YUV_420_888` images at the configured size closest to
//! the one asked for; the reader's own thread hears each image, and its
//! three planes go to the app as they are (no copy) before the image goes
//! back. Continuous autofocus (`CONTINUOUS_PICTURE`) from the start; the
//! torch is `FLASH_MODE_TORCH`, the zoom a centered `SCALER_CROP_REGION`,
//! and a focus point the AF (and AE) regions there.
//!
//! While the window's renderer imports the reader's buffers (GLES through
//! an external texture), the preview gets each image's `AHardwareBuffer`
//! instead of a converted copy ([`hwbuf`]).
//!
//! [`stop`](Session::stop) closes the device (another app may take the
//! camera while this one is in the background) and
//! [`start`](Session::start) opens it again.

use super::{
    CameraError, CameraOptions, CameraStatus, Caps, Frame, FrameFormat, Lens, Plane, Session, Sink,
};
use crate::geometry::{PhysicalSize, Point};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[path = "android/hwbuf.rs"]
mod hwbuf;

/// Whether Stipple captures from cameras here.
pub(super) const SUPPORTED: bool = true;

// ---- Metadata tags (NdkCameraMetadataTags.h: section << 16 | index) ----------

const CONTROL: u32 = 1 << 16;
const CONTROL_AE_MODE: u32 = CONTROL + 3;
const CONTROL_AE_REGIONS: u32 = CONTROL + 4;
const CONTROL_AF_MODE: u32 = CONTROL + 7;
const CONTROL_AF_REGIONS: u32 = CONTROL + 8;
const CONTROL_AF_AVAILABLE_MODES: u32 = CONTROL + 23;
const CONTROL_MAX_REGIONS: u32 = CONTROL + 28;
const FLASH_MODE: u32 = (4 << 16) + 2;
const FLASH_INFO_AVAILABLE: u32 = 5 << 16;
const LENS_FACING: u32 = (8 << 16) + 5;
const SCALER: u32 = 13 << 16;
const SCALER_CROP_REGION: u32 = SCALER;
const SCALER_AVAILABLE_MAX_DIGITAL_ZOOM: u32 = SCALER + 4;
const SCALER_AVAILABLE_STREAM_CONFIGURATIONS: u32 = SCALER + 10;
const SENSOR_ORIENTATION: u32 = (14 << 16) + 14;
const SENSOR_INFO_ACTIVE_ARRAY_SIZE: u32 = 15 << 16;

const AE_MODE_ON: u8 = 1;
const AF_MODE_CONTINUOUS_PICTURE: u8 = 4;
const FLASH_MODE_OFF: u8 = 0;
const FLASH_MODE_TORCH: u8 = 2;
const FACING_FRONT: u8 = 0;
const FACING_BACK: u8 = 1;
const TEMPLATE_PREVIEW: c_int = 1;
const AIMAGE_FORMAT_YUV_420_888: i32 = 0x23;
const ACAMERA_OK: i32 = 0;

#[repr(C)]
struct ConstEntry {
    tag: u32,
    ty: u8,
    count: u32,
    data: *const c_void,
}

#[repr(C)]
struct IdList {
    count: c_int,
    ids: *const *const c_char,
}

#[repr(C)]
struct DeviceCallbacks {
    context: *mut c_void,
    on_disconnected: unsafe extern "C" fn(*mut c_void, *mut c_void),
    on_error: unsafe extern "C" fn(*mut c_void, *mut c_void, c_int),
}

#[repr(C)]
struct SessionCallbacks {
    context: *mut c_void,
    on_closed: unsafe extern "C" fn(*mut c_void, *mut c_void),
    on_ready: unsafe extern "C" fn(*mut c_void, *mut c_void),
    on_active: unsafe extern "C" fn(*mut c_void, *mut c_void),
}

#[repr(C)]
struct ImageListener {
    context: *mut c_void,
    on_image_available: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
}

type P = *mut c_void;

dlsym::library! {
    /// The camera NDK's functions used (`libcamera2ndk.so`, Android 7).
    struct Cam {
        unsafe extern "C" {
            #[link_name = "ACameraManager_create"]
            fn manager_create() -> P;
            #[link_name = "ACameraManager_delete"]
            fn manager_delete(a: P);
            #[link_name = "ACameraManager_getCameraIdList"]
            fn id_list(a: P, b: *mut *mut IdList) -> i32;
            #[link_name = "ACameraManager_deleteCameraIdList"]
            fn delete_id_list(a: *mut IdList);
            #[link_name = "ACameraManager_getCameraCharacteristics"]
            fn characteristics(a: P, b: *const c_char, c: *mut P) -> i32;
            #[link_name = "ACameraMetadata_getConstEntry"]
            fn const_entry(a: P, b: u32, c: *mut ConstEntry) -> i32;
            #[link_name = "ACameraMetadata_free"]
            fn metadata_free(a: P);
            #[link_name = "ACameraManager_openCamera"]
            fn open_camera(a: P, b: *const c_char, c: *mut DeviceCallbacks, d: *mut P) -> i32;
            #[link_name = "ACameraDevice_close"]
            fn device_close(a: P) -> i32;
            #[link_name = "ACameraDevice_createCaptureRequest"]
            fn create_request(a: P, b: c_int, c: *mut P) -> i32;
            #[link_name = "ACaptureRequest_free"]
            fn request_free(a: P);
            #[link_name = "ACaptureRequest_addTarget"]
            fn request_add_target(a: P, b: P) -> i32;
            #[link_name = "ACaptureRequest_setEntry_u8"]
            fn set_u8(a: P, b: u32, c: u32, d: *const u8) -> i32;
            #[link_name = "ACaptureRequest_setEntry_i32"]
            fn set_i32(a: P, b: u32, c: u32, d: *const i32) -> i32;
            #[link_name = "ACameraOutputTarget_create"]
            fn target_create(a: P, b: *mut P) -> i32;
            #[link_name = "ACameraOutputTarget_free"]
            fn target_free(a: P);
            #[link_name = "ACaptureSessionOutputContainer_create"]
            fn container_create(a: *mut P) -> i32;
            #[link_name = "ACaptureSessionOutputContainer_free"]
            fn container_free(a: P);
            #[link_name = "ACaptureSessionOutputContainer_add"]
            fn container_add(a: P, b: P) -> i32;
            #[link_name = "ACaptureSessionOutput_create"]
            fn output_create(a: P, b: *mut P) -> i32;
            #[link_name = "ACaptureSessionOutput_free"]
            fn output_free(a: P);
            #[link_name = "ACameraDevice_createCaptureSession"]
            fn create_session(a: P, b: P, c: *const SessionCallbacks, d: *mut P) -> i32;
            #[link_name = "ACameraCaptureSession_close"]
            fn session_close(a: P);
            #[link_name = "ACameraCaptureSession_setRepeatingRequest"]
            fn set_repeating(a: P, b: P, c: c_int, d: *mut P, e: *mut c_int) -> i32;
            #[link_name = "ACameraCaptureSession_stopRepeating"]
            fn stop_repeating(a: P) -> i32;
        }
    }
}

dlsym::library! {
    /// The media NDK's image reader functions used (`libmediandk.so`).
    struct Media {
        unsafe extern "C" {
            #[link_name = "AImageReader_new"]
            fn reader_new(a: i32, b: i32, c: i32, d: i32, e: *mut P) -> i32;
            #[link_name = "AImageReader_delete"]
            fn reader_delete(a: P);
            #[link_name = "AImageReader_getWindow"]
            fn reader_window(a: P, b: *mut P) -> i32;
            #[link_name = "AImageReader_setImageListener"]
            fn reader_listener(a: P, b: *mut ImageListener) -> i32;
            #[link_name = "AImageReader_acquireLatestImage"]
            fn acquire_latest(a: P, b: *mut P) -> i32;
            #[link_name = "AImage_delete"]
            fn image_delete(a: P);
            #[link_name = "AImage_getWidth"]
            fn image_width(a: P, b: *mut i32) -> i32;
            #[link_name = "AImage_getHeight"]
            fn image_height(a: P, b: *mut i32) -> i32;
            #[link_name = "AImage_getTimestamp"]
            fn image_timestamp(a: P, b: *mut i64) -> i32;
            #[link_name = "AImage_getPlaneRowStride"]
            fn plane_row_stride(a: P, b: c_int, c: *mut i32) -> i32;
            #[link_name = "AImage_getPlanePixelStride"]
            fn plane_pixel_stride(a: P, b: c_int, c: *mut i32) -> i32;
            #[link_name = "AImage_getPlaneData"]
            fn plane_data(a: P, b: c_int, c: *mut *mut u8, d: *mut c_int) -> i32;
        }
    }
}

/// The NDK functions used.
struct Lib {
    cam: Cam,
    media: Media,
}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: system libraries, trusted like the rest of the platform.
        unsafe {
            Some(Lib {
                cam: <Cam as dlsym::Bindings>::open("libcamera2ndk.so").ok()?,
                media: <Media as dlsym::Bindings>::open("libmediandk.so").ok()?,
            })
        }
    })
    .as_ref()
}

/// A camera's characteristics.
struct Info {
    id: CString,
    lens: Lens,
    sensor: u32,
    torch: bool,
    max_zoom: f32,
    /// `(left, top, width, height)`.
    active: [i32; 4],
    af_regions: bool,
    ae_regions: bool,
    continuous_af: bool,
    /// The `YUV_420_888` output sizes.
    sizes: Vec<(i32, i32)>,
}

/// Entry `tag` of `meta`, if there.
unsafe fn entry(lib: &Lib, meta: P, tag: u32) -> Option<ConstEntry> {
    let mut e = ConstEntry {
        tag: 0,
        ty: 0,
        count: 0,
        data: std::ptr::null(),
    };
    let ok = unsafe { lib.cam.const_entry(meta, tag, &mut e) } == ACAMERA_OK;
    (ok && !e.data.is_null() && e.count > 0).then_some(e)
}

unsafe fn i32s(lib: &Lib, meta: P, tag: u32) -> Vec<i32> {
    unsafe { entry(lib, meta, tag) }
        .map(|e| {
            unsafe { std::slice::from_raw_parts(e.data as *const i32, e.count as usize) }.to_vec()
        })
        .unwrap_or_default()
}

unsafe fn u8s(lib: &Lib, meta: P, tag: u32) -> Vec<u8> {
    unsafe { entry(lib, meta, tag) }
        .map(|e| {
            unsafe { std::slice::from_raw_parts(e.data as *const u8, e.count as usize) }.to_vec()
        })
        .unwrap_or_default()
}

/// Every camera's characteristics, through a manager made for the call.
fn cameras(lib: &Lib) -> Vec<Info> {
    let mut out = Vec::new();
    unsafe {
        let manager = lib.cam.manager_create();
        if manager.is_null() {
            return out;
        }
        let mut list: *mut IdList = std::ptr::null_mut();
        if lib.cam.id_list(manager, &mut list) == ACAMERA_OK && !list.is_null() {
            let ids = std::slice::from_raw_parts((*list).ids, (*list).count.max(0) as usize);
            for &id in ids {
                let mut meta: P = std::ptr::null_mut();
                if id.is_null() || lib.cam.characteristics(manager, id, &mut meta) != ACAMERA_OK {
                    continue;
                }
                let lens = match u8s(lib, meta, LENS_FACING).first() {
                    Some(&FACING_FRONT) => Lens::Front,
                    Some(&FACING_BACK) => Lens::Back,
                    _ => Lens::External,
                };
                let zoom = entry(lib, meta, SCALER_AVAILABLE_MAX_DIGITAL_ZOOM)
                    .map_or(1.0, |e| *(e.data as *const f32));
                let active = i32s(lib, meta, SENSOR_INFO_ACTIVE_ARRAY_SIZE);
                let regions = i32s(lib, meta, CONTROL_MAX_REGIONS);
                let configs = i32s(lib, meta, SCALER_AVAILABLE_STREAM_CONFIGURATIONS);
                let sizes = configs
                    .as_chunks::<4>()
                    .0
                    .iter()
                    // Output streams (input flag 0) of YUV_420_888.
                    .filter(|c| c[0] == AIMAGE_FORMAT_YUV_420_888 && c[3] == 0)
                    .map(|c| (c[1], c[2]))
                    .collect();
                out.push(Info {
                    id: CStr::from_ptr(id).to_owned(),
                    lens,
                    sensor: i32s(lib, meta, SENSOR_ORIENTATION)
                        .first()
                        .map_or(0, |&o| o.rem_euclid(360) as u32),
                    torch: u8s(lib, meta, FLASH_INFO_AVAILABLE).first() == Some(&1),
                    max_zoom: if zoom.is_finite() { zoom.max(1.0) } else { 1.0 },
                    active: if active.len() >= 4 {
                        [active[0], active[1], active[2], active[3]]
                    } else {
                        [0, 0, 0, 0]
                    },
                    // (AE, AWB, AF).
                    ae_regions: regions.first().is_some_and(|&n| n > 0),
                    af_regions: regions.get(2).is_some_and(|&n| n > 0),
                    continuous_af: u8s(lib, meta, CONTROL_AF_AVAILABLE_MODES)
                        .contains(&AF_MODE_CONTINUOUS_PICTURE),
                    sizes,
                });
                lib.cam.metadata_free(meta);
            }
            lib.cam.delete_id_list(list);
        }
        lib.cam.manager_delete(manager);
    }
    out
}

pub(super) fn lenses() -> Vec<Lens> {
    let Some(lib) = lib() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for c in cameras(lib) {
        if !out.contains(&c.lens) {
            out.push(c.lens);
        }
    }
    out
}

pub(super) fn display_rotation() -> u32 {
    crate::platform::backend::android::display_rotation()
}

/// The output size closest to `want` (compared landscape, as sensors
/// deliver).
fn choose_size(sizes: &[(i32, i32)], want: PhysicalSize) -> (i32, i32) {
    let (long, short) = (
        want.width.max(want.height) as i64,
        want.width.min(want.height) as i64,
    );
    sizes
        .iter()
        .copied()
        .min_by_key(|&(w, h)| {
            let (l, s) = (i64::from(w.max(h)), i64::from(w.min(h)));
            (l - long).abs() + (s - short).abs()
        })
        .unwrap_or((640, 480))
}

/// What the callbacks share: the sink (taken away when the session goes,
/// so a late callback finds nothing) and how frames are turned.
struct Ctx {
    sink: Mutex<Option<Arc<Sink>>>,
    /// The reader whose images may be taken (0: none), held by `on_image`
    /// for as long as it runs. `close` clears it before deleting the reader,
    /// which waits for a callback in flight: `AImageReader_delete` joins the
    /// reader's callback thread while holding the reader's own lock, so a
    /// callback still taking or freeing an image would deadlock with it.
    reader: Mutex<usize>,
    sensor: u32,
    front: bool,
}

impl Ctx {
    fn sink(&self) -> Option<Arc<Sink>> {
        self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

unsafe extern "C" fn on_disconnected(ctx: *mut c_void, _device: *mut c_void) {
    if let Some(sink) = unsafe { &*(ctx as *const Ctx) }.sink() {
        sink.set_status(CameraStatus::Failed(CameraError::Os(
            "the camera was disconnected".into(),
        )));
    }
}

unsafe extern "C" fn on_error(ctx: *mut c_void, _device: *mut c_void, error: c_int) {
    if let Some(sink) = unsafe { &*(ctx as *const Ctx) }.sink() {
        sink.set_status(CameraStatus::Failed(CameraError::Os(format!(
            "camera device error {error}"
        ))));
    }
}

unsafe extern "C" fn session_event(_ctx: *mut c_void, _session: *mut c_void) {}

/// An image is ready (on the reader's thread): hand its planes over.
unsafe extern "C" fn on_image(ctx: *mut c_void, reader: *mut c_void) {
    let Some(lib) = lib() else {
        return;
    };
    let ctx = unsafe { &*(ctx as *const Ctx) };
    // Held to the end: see `Ctx::reader`. A callback that was queued before
    // its reader went finds another reader here (or none) and leaves.
    let live = ctx.reader.lock().unwrap_or_else(|e| e.into_inner());
    if *live != reader as usize {
        return;
    }
    let mut image: P = std::ptr::null_mut();
    if unsafe { lib.media.acquire_latest(reader, &mut image) } != 0 || image.is_null() {
        return;
    }
    let kept = std::cell::Cell::new(false);
    let delivered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let Some(sink) = ctx.sink() else {
            return;
        };
        let (mut w, mut h, mut t) = (0i32, 0i32, 0i64);
        lib.media.image_width(image, &mut w);
        lib.media.image_height(image, &mut h);
        lib.media.image_timestamp(image, &mut t);
        let mut planes = [Plane::new(&[], 0, 1); 3];
        for (i, plane) in planes.iter_mut().enumerate() {
            let (mut data, mut len, mut row, mut pixel) = (std::ptr::null_mut(), 0, 0, 0);
            let i = i as c_int;
            if lib.media.plane_data(image, i, &mut data, &mut len) != 0
                || lib.media.plane_row_stride(image, i, &mut row) != 0
                || lib.media.plane_pixel_stride(image, i, &mut pixel) != 0
                || data.is_null()
            {
                return;
            }
            *plane = Plane::new(
                std::slice::from_raw_parts(data, len.max(0) as usize),
                row.max(0) as usize,
                pixel.max(1) as usize,
            );
        }
        let (rotation, mirrored) = sink.orient(ctx.sensor, ctx.front, false);
        // The image's own buffer, for a renderer that samples it: kept out
        // of the reader while the preview holds it.
        let mut external = || hwbuf::external(lib, reader, image, &kept);
        sink.deliver_with(
            &Frame {
                size: PhysicalSize::new(w.max(0) as u32, h.max(0) as u32),
                format: FrameFormat::Yuv420,
                planes: &planes,
                timestamp: Duration::from_nanos(t.max(0) as u64),
                rotation,
                mirrored,
            },
            Some((crate::render::ExternalFormat::Yuv420, &mut external)),
        );
    }));
    if !kept.get() {
        unsafe { lib.media.image_delete(image) };
    }
    if delivered.is_err() {
        eprintln!("stipple: the camera's frame callback panicked");
    }
}

/// The open device and its streaming session.
struct Open {
    manager: P,
    device: P,
    reader: P,
    output: P,
    container: P,
    target: P,
    request: P,
    session: P,
    /// Kept alive while the device can call it.
    _device_callbacks: Box<DeviceCallbacks>,
    _session_callbacks: Box<SessionCallbacks>,
}

/// An open Camera2 camera.
struct NdkSession {
    info: Info,
    size: (i32, i32),
    /// Leaked: a callback may still be running as the session goes; its sink
    /// is taken away instead.
    ctx: &'static Ctx,
    open: Option<Open>,
    torch: bool,
    /// The crop region, `(left, top, width, height)`.
    crop: [i32; 4],
    /// AF/AE region around the focus point, if one was set.
    region: Option<[i32; 5]>,
}

// The NDK objects are thread-safe; they're only touched under the camera's
// lock.
unsafe impl Send for NdkSession {}

fn os(what: &str, status: i32) -> CameraError {
    CameraError::Os(format!("{what} failed ({status})"))
}

impl NdkSession {
    /// Open the device and stream into a new reader.
    fn open_device(&mut self, lib: &Lib) -> Result<(), CameraError> {
        unsafe {
            let ctx = self.ctx as *const Ctx as *mut c_void;
            let mut o = Open {
                manager: lib.cam.manager_create(),
                device: std::ptr::null_mut(),
                reader: std::ptr::null_mut(),
                output: std::ptr::null_mut(),
                container: std::ptr::null_mut(),
                target: std::ptr::null_mut(),
                request: std::ptr::null_mut(),
                session: std::ptr::null_mut(),
                _device_callbacks: Box::new(DeviceCallbacks {
                    context: ctx,
                    on_disconnected,
                    on_error,
                }),
                _session_callbacks: Box::new(SessionCallbacks {
                    context: ctx,
                    on_closed: session_event,
                    on_ready: session_event,
                    on_active: session_event,
                }),
            };
            // `o` closes whatever was made if a step fails.
            if o.manager.is_null() {
                return Err(CameraError::NoCamera);
            }
            let callbacks: *mut DeviceCallbacks = &mut *o._device_callbacks;
            let r = lib
                .cam
                .open_camera(o.manager, self.info.id.as_ptr(), callbacks, &mut o.device);
            if r != ACAMERA_OK || o.device.is_null() {
                close(lib, self.ctx, o);
                return Err(if r == -10013 {
                    // ACAMERA_ERROR_PERMISSION_DENIED.
                    CameraError::NotPermitted
                } else {
                    os("opening the camera", r)
                });
            }
            let (w, h) = self.size;
            let r = hwbuf::new_reader(lib, w, h, AIMAGE_FORMAT_YUV_420_888, &mut o.reader);
            if r != 0 || o.reader.is_null() {
                close(lib, self.ctx, o);
                return Err(os("AImageReader_new", r));
            }
            let mut listener = ImageListener {
                context: ctx,
                on_image_available: Some(on_image),
            };
            *self.ctx.reader.lock().unwrap_or_else(|e| e.into_inner()) = o.reader as usize;
            lib.media.reader_listener(o.reader, &mut listener);
            let mut window: P = std::ptr::null_mut();
            lib.media.reader_window(o.reader, &mut window);
            let steps = [
                lib.cam.output_create(window, &mut o.output),
                lib.cam.container_create(&mut o.container),
            ];
            if steps.iter().any(|&r| r != ACAMERA_OK) {
                close(lib, self.ctx, o);
                return Err(os("making the session's output", steps[0].min(steps[1])));
            }
            lib.cam.container_add(o.container, o.output);
            lib.cam.target_create(window, &mut o.target);
            let r = lib
                .cam
                .create_request(o.device, TEMPLATE_PREVIEW, &mut o.request);
            if r != ACAMERA_OK || o.request.is_null() {
                close(lib, self.ctx, o);
                return Err(os("making the capture request", r));
            }
            lib.cam.request_add_target(o.request, o.target);
            let r = lib.cam.create_session(
                o.device,
                o.container,
                &*o._session_callbacks,
                &mut o.session,
            );
            if r != ACAMERA_OK || o.session.is_null() {
                close(lib, self.ctx, o);
                return Err(os("making the capture session", r));
            }
            self.open = Some(o);
            self.apply(lib);
            Ok(())
        }
    }

    /// Write the request's settings (focus, torch, crop, regions) and
    /// stream it.
    fn apply(&mut self, lib: &Lib) {
        let Some(o) = &self.open else {
            return;
        };
        unsafe {
            let set_u8 = |tag, v: u8| lib.cam.set_u8(o.request, tag, 1, &v);
            set_u8(CONTROL_AE_MODE, AE_MODE_ON);
            if self.info.continuous_af {
                set_u8(CONTROL_AF_MODE, AF_MODE_CONTINUOUS_PICTURE);
            }
            if self.info.torch {
                set_u8(
                    FLASH_MODE,
                    if self.torch {
                        FLASH_MODE_TORCH
                    } else {
                        FLASH_MODE_OFF
                    },
                );
            }
            if self.crop[2] > 0 && self.crop[3] > 0 {
                lib.cam
                    .set_i32(o.request, SCALER_CROP_REGION, 4, self.crop.as_ptr());
            }
            if let Some(region) = self.region {
                if self.info.af_regions {
                    lib.cam
                        .set_i32(o.request, CONTROL_AF_REGIONS, 5, region.as_ptr());
                }
                if self.info.ae_regions {
                    lib.cam
                        .set_i32(o.request, CONTROL_AE_REGIONS, 5, region.as_ptr());
                }
            }
            let mut request = o.request;
            let r = lib.cam.set_repeating(
                o.session,
                std::ptr::null_mut(),
                1,
                &mut request,
                std::ptr::null_mut(),
            );
            if r != ACAMERA_OK
                && let Some(sink) = self.ctx.sink()
            {
                sink.set_status(CameraStatus::Failed(os("streaming", r)));
            }
        }
    }
}

/// Close what `o` made, in order: the session, the request and outputs, the
/// device, then the reader.
fn close(lib: &Lib, ctx: &Ctx, o: Open) {
    unsafe {
        if !o.session.is_null() {
            lib.cam.stop_repeating(o.session);
            lib.cam.session_close(o.session);
        }
        if !o.request.is_null() {
            lib.cam.request_free(o.request);
        }
        if !o.target.is_null() {
            lib.cam.target_free(o.target);
        }
        if !o.container.is_null() {
            lib.cam.container_free(o.container);
        }
        if !o.output.is_null() {
            lib.cam.output_free(o.output);
        }
        if !o.device.is_null() {
            lib.cam.device_close(o.device);
        }
        if !o.reader.is_null() {
            let mut none = ImageListener {
                context: std::ptr::null_mut(),
                on_image_available: None,
            };
            lib.media.reader_listener(o.reader, &mut none);
            // Wait for a callback in flight, and turn away those queued.
            *ctx.reader.lock().unwrap_or_else(|e| e.into_inner()) = 0;
            hwbuf::delete_reader(lib, o.reader);
        }
        if !o.manager.is_null() {
            lib.cam.manager_delete(o.manager);
        }
    }
}

impl Session for NdkSession {
    fn start(&mut self) -> Result<(), CameraError> {
        let lib = lib().ok_or(CameraError::NoCamera)?;
        if self.open.is_none() {
            self.open_device(lib)?;
        } else {
            self.apply(lib);
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let (Some(lib), Some(o)) = (lib(), self.open.take()) {
            close(lib, self.ctx, o);
        }
    }

    fn set_torch(&mut self, on: bool) -> Result<(), CameraError> {
        self.torch = on;
        if let Some(lib) = lib() {
            self.apply(lib);
        }
        Ok(())
    }

    fn set_zoom(&mut self, factor: f32) {
        let [l, t, w, h] = self.info.active;
        let f = f64::from(factor.max(1.0));
        let (cw, ch) = ((f64::from(w) / f) as i32, (f64::from(h) / f) as i32);
        self.crop = [l + (w - cw) / 2, t + (h - ch) / 2, cw, ch];
        if let Some(lib) = lib() {
            self.apply(lib);
        }
    }

    fn focus_at(&mut self, point: Point) {
        // The frame shows the crop region, scaled.
        let [l, t, w, h] = if self.crop[2] > 0 {
            self.crop
        } else {
            self.info.active
        };
        let (x, y) = (
            l + (point.x * f64::from(w)) as i32,
            t + (point.y * f64::from(h)) as i32,
        );
        let (rw, rh) = ((w / 8).max(1), (h / 8).max(1));
        let [al, at, aw, ah] = self.info.active;
        let xmin = (x - rw / 2).clamp(al, al + aw - rw);
        let ymin = (y - rh / 2).clamp(at, at + ah - rh);
        // (xmin, ymin, xmax, ymax, weight).
        self.region = Some([xmin, ymin, xmin + rw, ymin + rh, 1000]);
        if let Some(lib) = lib() {
            self.apply(lib);
        }
    }
}

impl Drop for NdkSession {
    fn drop(&mut self) {
        self.stop();
        *self.ctx.sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

pub(super) fn open(
    sink: Arc<Sink>,
    options: &CameraOptions,
) -> Result<(Box<dyn Session>, Caps), CameraError> {
    let lib = lib().ok_or(CameraError::NoCamera)?;
    let mut all = cameras(lib);
    let index = all
        .iter()
        .position(|c| c.lens == options.lens)
        .or((!all.is_empty()).then_some(0))
        .ok_or(CameraError::NoCamera)?;
    let info = all.swap_remove(index);
    let size = choose_size(&info.sizes, options.resolution);
    let caps = Caps {
        torch: info.torch,
        focus: info.af_regions,
        zoom: (1.0, info.max_zoom),
        lens: info.lens,
    };
    let ctx: &'static Ctx = Box::leak(Box::new(Ctx {
        sink: Mutex::new(Some(sink)),
        reader: Mutex::new(0),
        sensor: info.sensor,
        front: info.lens == Lens::Front,
    }));
    let mut session = NdkSession {
        crop: info.active,
        info,
        size,
        ctx,
        open: None,
        torch: false,
        region: None,
    };
    // Open now, so a refusal shows here rather than at the first start.
    session.open_device(lib)?;
    Ok((Box::new(session), caps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_close_to_the_ask() {
        let sizes = [(1920, 1080), (1280, 720), (640, 480), (320, 240)];
        assert_eq!(
            choose_size(&sizes, PhysicalSize::new(1280, 720)),
            (1280, 720)
        );
        assert_eq!(
            choose_size(&sizes, PhysicalSize::new(720, 1280)),
            (1280, 720)
        );
        assert_eq!(
            choose_size(&sizes, PhysicalSize::new(4000, 3000)),
            (1920, 1080)
        );
        assert_eq!(choose_size(&[], PhysicalSize::new(1, 1)), (640, 480));
    }
}
