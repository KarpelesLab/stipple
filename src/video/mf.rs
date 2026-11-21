//! H.264 and HEVC through Media Foundation (Windows): the system's decoder
//! as an `IMFTransform` (`CLSID_MSH264DecoderMFT`, and HEVC's where the HEVC
//! Video Extensions are installed: `CLSID_MSH265DecoderMFT`, else the first
//! synchronous decoder `MFTEnumEx` lists), driven by hand through its COM
//! vtable. `mfplat.dll` and `d3d11.dll` are loaded when the first video
//! opens; a Windows without Media Foundation (the N editions, Server without
//! the feature) says `Unsupported`.
//!
//! The demuxer's length-prefixed access units go in as Annex B (start
//! codes), the parameter sets of the `avcC`/`hvcC` record before the first
//! one and again after a seek, each with its presentation time; pictures
//! come out in presentation order with their times, as NV12.
//!
//! Where a hardware Direct3D 11 device with video support comes up and the
//! decoder is Direct3D 11 aware, it gets the device through an
//! `IMFDXGIDeviceManager` and decodes on the GPU (DXVA) into textures.
//! Each picture is then, while the renderer imports NV12 textures (the
//! Direct3D 11 renderer drew in the last half second), copied on the GPU
//! into a shared NV12 texture of its own and handed over as a DXGI shared
//! handle, sampled by the renderer through two plane views; otherwise it's
//! read back once (a staging texture) and converted. Without that device
//! (a WARP-only machine: CI's runners) the decoder runs in software into
//! system memory NV12, converted, or uploaded into such a shared texture
//! while the renderer imports them.
//!
//! COM: the decoder is made on a short-lived thread of its own in the
//! multithreaded apartment (the caller's thread may be the UI's
//! single-threaded one, or not initialized yet: it isn't touched) and used
//! from the player's thread, which joins the MTA too. The Microsoft decoders
//! are free-threaded (`ThreadingModel` Both).

use super::nal::{annex_b, parse_avcc, parse_hvcc};
use super::player::{Coded, Decoded, FrameDecoder};
use super::{VideoError, nv12};
use crate::geometry::PhysicalSize;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalFormat, ExternalImage};
use oxideav_core::CodecParameters;
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

type Hresult = i32;
type P = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Guid(u32, u16, u16, [u8; 8]);

/// The `FOURCC`-based media subtypes (`MFVideoFormat_*`).
const fn fourcc(code: &[u8; 4]) -> Guid {
    Guid(
        u32::from_le_bytes(*code),
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    )
}

const CLSID_MSH264_DECODER: Guid = Guid(
    0x62CE_7E72,
    0x4C71,
    0x4D20,
    [0xB1, 0x5D, 0x45, 0x28, 0x31, 0xA8, 0x7D, 0x9D],
);
const CLSID_MSH265_DECODER: Guid = Guid(
    0x420A_51A3,
    0xD605,
    0x430C,
    [0xB4, 0xFC, 0x45, 0x27, 0x4F, 0xA6, 0xC5, 0x62],
);
const IID_IMF_TRANSFORM: Guid = Guid(
    0xBF94_C121,
    0x5B05,
    0x4E6F,
    [0x80, 0x00, 0xBA, 0x59, 0x89, 0x61, 0x41, 0x4D],
);
const IID_IMF_2D_BUFFER: Guid = Guid(
    0x7DC9_D5F9,
    0x9ED9,
    0x44EC,
    [0x9B, 0xBF, 0x06, 0x00, 0xBB, 0x58, 0x9F, 0xBB],
);
const IID_IMF_DXGI_BUFFER: Guid = Guid(
    0xE717_4CFA,
    0x1C9E,
    0x48B1,
    [0x88, 0x66, 0x62, 0x62, 0x26, 0xBF, 0xC2, 0x58],
);
const IID_ID3D11_TEXTURE2D: Guid = Guid(
    0x6F15_AAF2,
    0xD208,
    0x4E89,
    [0x9A, 0xB4, 0x48, 0x95, 0x35, 0xD3, 0x4F, 0x9C],
);
const IID_IDXGI_RESOURCE: Guid = Guid(
    0x035F_3AB4,
    0x482E,
    0x4E50,
    [0xB4, 0x1F, 0x8A, 0x7F, 0x8B, 0xD8, 0x96, 0x0B],
);
const IID_ID3D10_MULTITHREAD: Guid = Guid(
    0x9B7E_4E00,
    0x342C,
    0x4106,
    [0xA1, 0x9F, 0x4F, 0x27, 0x04, 0xF6, 0x89, 0xF0],
);
const MF_MEDIA_TYPE_VIDEO: Guid = fourcc(b"vids");
const MF_VIDEO_FORMAT_H264: Guid = fourcc(b"H264");
const MF_VIDEO_FORMAT_HEVC: Guid = fourcc(b"HEVC");
const MF_VIDEO_FORMAT_NV12: Guid = fourcc(b"NV12");
const MFT_CATEGORY_VIDEO_DECODER: Guid = Guid(
    0xD6C0_2D4B,
    0x6833,
    0x45B4,
    [0x97, 0x1A, 0x05, 0xA4, 0xB0, 0x4B, 0xAB, 0x91],
);
const MF_MT_MAJOR_TYPE: Guid = Guid(
    0x48EB_A18E,
    0xF8C9,
    0x4687,
    [0xBF, 0x11, 0x0A, 0x74, 0xC9, 0xF9, 0x6A, 0x8F],
);
const MF_MT_SUBTYPE: Guid = Guid(
    0xF7E3_4C9A,
    0x42E8,
    0x4714,
    [0xB7, 0x4B, 0xCB, 0x29, 0xD7, 0x2C, 0x35, 0xE5],
);
const MF_MT_FRAME_SIZE: Guid = Guid(
    0x1652_C33D,
    0xD6B2,
    0x4012,
    [0xB8, 0x34, 0x72, 0x03, 0x08, 0x49, 0xA3, 0x7D],
);
const MF_MT_DEFAULT_STRIDE: Guid = Guid(
    0x644B_4E48,
    0x1E02,
    0x4516,
    [0xB0, 0xEB, 0xC0, 0x1C, 0xA9, 0xD4, 0x9A, 0xC6],
);
const MF_MT_MINIMUM_DISPLAY_APERTURE: Guid = Guid(
    0xD738_8766,
    0x18FE,
    0x48C6,
    [0xA1, 0x77, 0xEE, 0x89, 0x48, 0x67, 0xC8, 0xC4],
);
const MF_MT_YUV_MATRIX: Guid = Guid(
    0x3E23_D450,
    0x2C75,
    0x4D25,
    [0xA0, 0x0E, 0xB9, 0x16, 0x70, 0xD1, 0x23, 0x27],
);
const MF_MT_VIDEO_NOMINAL_RANGE: Guid = Guid(
    0xC21B_8EE5,
    0xB956,
    0x4071,
    [0x8D, 0xAF, 0x32, 0x5E, 0xDF, 0x5C, 0xAB, 0x11],
);
const MF_SA_D3D11_AWARE: Guid = Guid(
    0x206B_4FC8,
    0xFCF9,
    0x4C51,
    [0xAF, 0xE3, 0x97, 0x64, 0x36, 0x9E, 0x33, 0xA0],
);

const MF_VERSION: u32 = 0x0002_0070;
const MFSTARTUP_LITE: u32 = 1;
const COINIT_MULTITHREADED: u32 = 0;
const CLSCTX_INPROC_SERVER: u32 = 1;
const MFT_ENUM_FLAG_SYNCMFT: u32 = 0x1;
const MFT_ENUM_FLAG_LOCALMFT: u32 = 0x10;
const MFT_ENUM_FLAG_SORTANDFILTER: u32 = 0x40;
const MFT_MESSAGE_COMMAND_FLUSH: u32 = 0;
const MFT_MESSAGE_COMMAND_DRAIN: u32 = 1;
const MFT_MESSAGE_SET_D3D_MANAGER: u32 = 2;
const MFT_MESSAGE_NOTIFY_BEGIN_STREAMING: u32 = 0x1000_0000;
const MFT_MESSAGE_NOTIFY_END_OF_STREAM: u32 = 0x1000_0002;
const MFT_MESSAGE_NOTIFY_START_OF_STREAM: u32 = 0x1000_0003;
const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES: u32 = 0x100;
const MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES: u32 = 0x200;
const MF_E_TRANSFORM_NEED_MORE_INPUT: Hresult = 0xC00D_6D72_u32 as i32;
const MF_E_TRANSFORM_STREAM_CHANGE: Hresult = 0xC00D_6D61_u32 as i32;
const MF_E_NOTACCEPTING: Hresult = 0xC00D_36B5_u32 as i32;
const MF_E_NO_MORE_TYPES: Hresult = 0xC00D_36B9_u32 as i32;
/// `MFVideoTransferMatrix_BT709`.
const MATRIX_BT709: u32 = 1;
/// `MFNominalRange_0_255`.
const RANGE_FULL: u32 = 1;

const D3D_DRIVER_TYPE_HARDWARE: u32 = 1;
const D3D_DRIVER_TYPE_WARP: u32 = 5;
const D3D11_SDK_VERSION: u32 = 7;
const D3D11_CREATE_DEVICE_VIDEO_SUPPORT: u32 = 0x800;
const DXGI_FORMAT_NV12: u32 = 103;
const D3D11_USAGE_DEFAULT: u32 = 0;
const D3D11_USAGE_STAGING: u32 = 3;
const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
const D3D11_CPU_ACCESS_READ: u32 = 0x2_0000;
const D3D11_RESOURCE_MISC_SHARED: u32 = 0x2;
const D3D11_MAP_READ: u32 = 1;

// Vtable slots: IUnknown's three, then each interface's in declaration
// order (IMFMediaType and IMFSample after IMFAttributes' 30).
const QUERY_INTERFACE: usize = 0;
const RELEASE: usize = 2;
// IMFAttributes
const ATTR_GET_UINT32: usize = 7;
const ATTR_GET_UINT64: usize = 8;
const ATTR_GET_GUID: usize = 10;
const ATTR_GET_BLOB: usize = 15;
const ATTR_SET_UINT64: usize = 22;
const ATTR_SET_GUID: usize = 24;
// IMFActivate
const ACTIVATE_OBJECT: usize = 33;
// IMFSample
const SAMPLE_GET_TIME: usize = 35;
const SAMPLE_SET_TIME: usize = 36;
const SAMPLE_SET_DURATION: usize = 38;
const SAMPLE_GET_BUFFER: usize = 40;
const SAMPLE_CONTIGUOUS: usize = 41;
const SAMPLE_ADD_BUFFER: usize = 42;
// IMFMediaBuffer
const BUFFER_LOCK: usize = 3;
const BUFFER_UNLOCK: usize = 4;
const BUFFER_GET_LENGTH: usize = 5;
const BUFFER_SET_LENGTH: usize = 6;
// IMF2DBuffer
const BUFFER2D_LOCK: usize = 3;
const BUFFER2D_UNLOCK: usize = 4;
// IMFDXGIBuffer
const DXGI_BUFFER_GET_RESOURCE: usize = 3;
const DXGI_BUFFER_GET_SUBRESOURCE: usize = 4;
// IMFTransform
const MFT_GET_OUTPUT_STREAM_INFO: usize = 7;
const MFT_GET_ATTRIBUTES: usize = 8;
const MFT_GET_OUTPUT_AVAILABLE_TYPE: usize = 14;
const MFT_SET_INPUT_TYPE: usize = 15;
const MFT_SET_OUTPUT_TYPE: usize = 16;
const MFT_PROCESS_MESSAGE: usize = 23;
const MFT_PROCESS_INPUT: usize = 24;
const MFT_PROCESS_OUTPUT: usize = 25;
// IMFDXGIDeviceManager
const MANAGER_RESET_DEVICE: usize = 7;
// ID3D10Multithread
const MULTITHREAD_SET_PROTECTED: usize = 5;
// ID3D11Device, ID3D11DeviceContext, IDXGIResource
const DEVICE_CREATE_TEXTURE2D: usize = 5;
const CTX_MAP: usize = 14;
const CTX_UNMAP: usize = 15;
const CTX_COPY_SUBRESOURCE_REGION: usize = 46;
const CTX_FLUSH: usize = 111;
const DXGI_RESOURCE_GET_SHARED_HANDLE: usize = 8;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: P, coinit: u32) -> Hresult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: P,
        ctx: u32,
        iid: *const Guid,
        out: *mut P,
    ) -> Hresult;
    fn CoTaskMemFree(p: P);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> P;
    fn GetProcAddress(module: P, name: *const u8) -> *const c_void;
}

/// Method `slot` of `obj`'s vtable.
unsafe fn method(obj: P, slot: usize) -> *const c_void {
    unsafe { *(*(obj as *const *const *const c_void)).add(slot) }
}

/// Call method `slot` of `obj` with the arguments' types.
macro_rules! com {
    ($obj:expr, $slot:expr, ($($ty:ty),*) -> $ret:ty $(, $arg:expr)* $(,)?) => {{
        let obj: P = $obj;
        let f: unsafe extern "system" fn(P $(, $ty)*) -> $ret =
            std::mem::transmute(method(obj, $slot));
        f(obj $(, $arg)*)
    }};
}

/// An owned interface pointer, released when dropped.
struct Com(P);

impl Com {
    fn ptr(&self) -> P {
        self.0
    }

    /// `QueryInterface` for `iid`.
    unsafe fn cast(&self, iid: &Guid) -> Option<Com> {
        let mut out: P = std::ptr::null_mut();
        let hr = unsafe {
            com!(self.0, QUERY_INTERFACE, (*const Guid, *mut P) -> Hresult, iid, &mut out)
        };
        (hr >= 0 && !out.is_null()).then(|| Com(out))
    }

    /// Give up ownership (the pointer's reference goes with it).
    fn into_raw(self) -> P {
        let p = self.0;
        std::mem::forget(self);
        p
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { release(self.0) };
        }
    }
}

unsafe fn release(p: P) {
    unsafe {
        com!(p, RELEASE, () -> u32);
    }
}

fn check(hr: Hresult, what: &str) -> Result<(), VideoError> {
    if hr < 0 {
        Err(VideoError::Decode(format!(
            "Media Foundation: {what} failed ({:#010x})",
            hr as u32
        )))
    } else {
        Ok(())
    }
}

/// `MFT_REGISTER_TYPE_INFO`.
#[repr(C)]
struct TypeInfo {
    major: Guid,
    sub: Guid,
}

/// `MFT_OUTPUT_STREAM_INFO`.
#[repr(C)]
#[derive(Default)]
struct OutputStreamInfo {
    flags: u32,
    size: u32,
    alignment: u32,
}

/// `MFT_OUTPUT_DATA_BUFFER`.
#[repr(C)]
struct OutputDataBuffer {
    stream: u32,
    sample: P,
    status: u32,
    events: P,
}

/// `MFVideoArea`: an `MFOffset` (fraction, value) for x and y, then a
/// `SIZE`.
#[repr(C)]
#[derive(Default)]
struct VideoArea {
    x_fract: u16,
    x: i16,
    y_fract: u16,
    y: i16,
    cx: i32,
    cy: i32,
}

#[repr(C)]
struct Texture2dDesc {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind: u32,
    cpu: u32,
    misc: u32,
}

impl Texture2dDesc {
    fn nv12(w: u32, h: u32, usage: u32, bind: u32, cpu: u32, misc: u32) -> Self {
        Self {
            width: w,
            height: h,
            mip_levels: 1,
            array_size: 1,
            format: DXGI_FORMAT_NV12,
            sample_count: 1,
            sample_quality: 0,
            usage,
            bind,
            cpu,
            misc,
        }
    }
}

#[repr(C)]
struct SubresourceData {
    data: *const c_void,
    pitch: u32,
    slice_pitch: u32,
}

#[repr(C)]
struct D3dBox {
    left: u32,
    top: u32,
    front: u32,
    right: u32,
    bottom: u32,
    back: u32,
}

#[repr(C)]
struct Mapped {
    data: *mut c_void,
    row_pitch: u32,
    depth_pitch: u32,
}

type CreateFn = unsafe extern "system" fn(*mut P) -> Hresult;
type EnumFn = unsafe extern "system" fn(
    Guid,
    u32,
    *const TypeInfo,
    *const TypeInfo,
    *mut *mut P,
    *mut u32,
) -> Hresult;
type D3dCreateFn = unsafe extern "system" fn(
    P,
    u32,
    P,
    u32,
    *const u32,
    u32,
    u32,
    *mut P,
    *mut u32,
    *mut P,
) -> Hresult;

/// `mfplat.dll`'s functions, Media Foundation started.
struct Lib {
    create_media_type: CreateFn,
    create_sample: CreateFn,
    create_memory_buffer: unsafe extern "system" fn(u32, *mut P) -> Hresult,
    create_aligned_buffer: unsafe extern "system" fn(u32, u32, *mut P) -> Hresult,
    create_device_manager: Option<unsafe extern "system" fn(*mut u32, *mut P) -> Hresult>,
    enum_ex: Option<EnumFn>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn proc(module: P, name: &std::ffi::CStr) -> Option<P> {
    let p = unsafe { GetProcAddress(module, name.as_ptr().cast()) } as P;
    (!p.is_null()).then_some(p)
}

#[allow(clippy::missing_transmute_annotations)] // each symbol's type is its field's
fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let module = LoadLibraryW(wide("mfplat.dll").as_ptr());
        if module.is_null() {
            return None;
        }
        let startup: unsafe extern "system" fn(u32, u32) -> Hresult =
            std::mem::transmute(proc(module, c"MFStartup")?);
        if startup(MF_VERSION, MFSTARTUP_LITE) < 0 {
            return None;
        }
        Some(Lib {
            create_media_type: std::mem::transmute(proc(module, c"MFCreateMediaType")?),
            create_sample: std::mem::transmute(proc(module, c"MFCreateSample")?),
            create_memory_buffer: std::mem::transmute(proc(module, c"MFCreateMemoryBuffer")?),
            create_aligned_buffer: std::mem::transmute(proc(
                module,
                c"MFCreateAlignedMemoryBuffer",
            )?),
            create_device_manager: proc(module, c"MFCreateDXGIDeviceManager")
                .map(|p| std::mem::transmute(p)),
            enum_ex: proc(module, c"MFTEnumEx").map(|p| std::mem::transmute(p)),
        })
    })
    .as_ref()
}

fn d3d11_create() -> Option<D3dCreateFn> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    let f = (*F.get_or_init(|| unsafe {
        let module = LoadLibraryW(wide("d3d11.dll").as_ptr());
        if module.is_null() {
            return None;
        }
        proc(module, c"D3D11CreateDevice").map(|p| p as usize)
    }))?;
    Some(unsafe { std::mem::transmute::<usize, D3dCreateFn>(f) })
}

thread_local! {
    static IN_MTA: Cell<bool> = const { Cell::new(false) };
}

/// Join the multithreaded apartment, once per thread (only ever on the
/// decoder's own threads).
fn enter_mta() {
    IN_MTA.with(|j| {
        if !j.get() {
            unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED) };
            j.set(true);
        }
    });
}

/// A Direct3D 11 device and its immediate context.
struct D3d {
    device: Com,
    ctx: Com,
}

impl D3d {
    /// A device on the default adapter (`video`: one the decoder can
    /// decode on), else on WARP (for the shared textures only).
    unsafe fn new(video: bool) -> Option<D3d> {
        let create = d3d11_create()?;
        let drivers: &[(u32, u32)] = if video {
            &[(D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_VIDEO_SUPPORT)]
        } else {
            &[(D3D_DRIVER_TYPE_HARDWARE, 0), (D3D_DRIVER_TYPE_WARP, 0)]
        };
        for &(driver, flags) in drivers {
            let (mut device, mut ctx, mut level): (P, P, u32) =
                (std::ptr::null_mut(), std::ptr::null_mut(), 0);
            let hr = unsafe {
                create(
                    std::ptr::null_mut(),
                    driver,
                    std::ptr::null_mut(),
                    flags,
                    std::ptr::null(),
                    0,
                    D3D11_SDK_VERSION,
                    &mut device,
                    &mut level,
                    &mut ctx,
                )
            };
            if hr >= 0 && !device.is_null() && !ctx.is_null() {
                return Some(D3d {
                    device: Com(device),
                    ctx: Com(ctx),
                });
            }
        }
        None
    }

    unsafe fn texture(&self, desc: &Texture2dDesc, data: Option<&SubresourceData>) -> Option<Com> {
        let mut tex: P = std::ptr::null_mut();
        let data = data.map_or(std::ptr::null(), |d| d as *const SubresourceData);
        let hr = unsafe {
            com!(self.device.ptr(), DEVICE_CREATE_TEXTURE2D,
                (*const Texture2dDesc, *const SubresourceData, *mut P) -> Hresult,
                desc, data, &mut tex)
        };
        (hr >= 0 && !tex.is_null()).then(|| Com(tex))
    }

    unsafe fn flush(&self) {
        unsafe { com!(self.ctx.ptr(), CTX_FLUSH, () -> ()) };
    }

    /// `tex` (a shared texture) as an image of its legacy shared handle,
    /// the texture held until the image goes.
    unsafe fn shared_image(
        &self,
        tex: Com,
        size: PhysicalSize,
        color: YuvColorSpace,
    ) -> Option<Arc<ExternalImage>> {
        unsafe {
            let res = tex.cast(&IID_IDXGI_RESOURCE)?;
            let mut handle: P = std::ptr::null_mut();
            let hr =
                com!(res.ptr(), DXGI_RESOURCE_GET_SHARED_HANDLE, (*mut P) -> Hresult, &mut handle);
            if hr < 0 || handle.is_null() {
                return None;
            }
            self.flush();
            let image =
                ExternalImage::dxgi_shared_handle(handle, false, size, ExternalFormat::Nv12)
                    .ok()?;
            let held = tex.into_raw() as usize;
            Some(Arc::new(
                image
                    .color_space(color)
                    .on_release(move || release(held as P)),
            ))
        }
    }
}

/// Which codec.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    H264,
    Hevc,
}

pub(super) struct MfDecoder {
    mft: Com,
    sets: Vec<Vec<u8>>,
    len_size: usize,
    /// The parameter sets go before the next packet (the first, a seek's).
    need_sets: bool,
    /// The decoder gives its own output samples (DXVA), else they're ours.
    provides_samples: bool,
    sample_size: u32,
    sample_alignment: u32,
    /// The output's rows (padded to the decoder's alignment), its stride.
    rows: u32,
    stride: u32,
    /// The picture's visible part.
    visible: PhysicalSize,
    container: PhysicalSize,
    color: YuvColorSpace,
    /// The device decoding (DXVA), whose textures the frames are.
    d3d: Option<D3d>,
    _manager: Option<Com>,
    /// A device for shared textures of software-decoded frames, made when
    /// first needed (`Some(None)`: none could be).
    upload: Option<Option<D3d>>,
    last: Duration,
    frame_time: Duration,
    out: Vec<Decoded>,
}

// The transform and devices are free-threaded COM objects used by one
// thread at a time (the player's).
unsafe impl Send for MfDecoder {}

impl MfDecoder {
    pub(super) fn new(params: &CodecParameters, frame_time: Duration) -> Result<Self, VideoError> {
        // On a thread of its own, in the MTA, leaving the caller's
        // apartment alone.
        std::thread::scope(|s| {
            s.spawn(|| {
                enter_mta();
                unsafe { Self::create(params, frame_time) }
            })
            .join()
            .unwrap_or_else(|_| Err(VideoError::Decode("Media Foundation panicked".into())))
        })
    }

    unsafe fn create(params: &CodecParameters, frame_time: Duration) -> Result<Self, VideoError> {
        let lib = lib()
            .ok_or_else(|| VideoError::Unsupported("no Media Foundation (mfplat.dll)".into()))?;
        let codec = match params.codec_id.as_str() {
            "h264" => Codec::H264,
            _ => Codec::Hevc,
        };
        let (sets, len_size) = match codec {
            Codec::H264 => parse_avcc(&params.extradata),
            Codec::Hevc => parse_hvcc(&params.extradata),
        }
        .ok_or_else(|| {
            VideoError::Unsupported(
                "H.264/HEVC without its configuration record (an Annex B stream)".into(),
            )
        })?;
        let (clsid, subtype, name) = match codec {
            Codec::H264 => (CLSID_MSH264_DECODER, MF_VIDEO_FORMAT_H264, "H.264"),
            Codec::Hevc => (CLSID_MSH265_DECODER, MF_VIDEO_FORMAT_HEVC, "HEVC"),
        };
        let mft = unsafe { instantiate(lib, &clsid, &subtype) }.ok_or_else(|| {
            VideoError::Unsupported(format!("no Media Foundation {name} decoder here"))
        })?;
        let container = PhysicalSize::new(params.width.unwrap_or(0), params.height.unwrap_or(0));
        let mut me = Self {
            mft,
            sets,
            len_size: len_size as usize,
            need_sets: true,
            provides_samples: false,
            sample_size: 0,
            sample_alignment: 0,
            rows: container.height,
            stride: container.width,
            visible: container,
            container,
            color: nv12::default_color(container.height),
            d3d: None,
            _manager: None,
            upload: None,
            last: Duration::ZERO,
            frame_time,
            out: Vec::new(),
        };
        unsafe {
            if std::env::var_os("STIPPLE_VIDEO_NO_DXVA").is_none() {
                me.use_gpu(lib);
            }
            let ty = media_type(lib)?;
            check(
                set_guid(&ty, &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_VIDEO),
                "SetGUID",
            )?;
            check(set_guid(&ty, &MF_MT_SUBTYPE, &subtype), "SetGUID")?;
            if container.width > 0 && container.height > 0 {
                let size = (u64::from(container.width) << 32) | u64::from(container.height);
                com!(ty.ptr(), ATTR_SET_UINT64, (*const Guid, u64) -> Hresult, &MF_MT_FRAME_SIZE, size);
            }
            let hr =
                com!(me.mft.ptr(), MFT_SET_INPUT_TYPE, (u32, P, u32) -> Hresult, 0, ty.ptr(), 0);
            if hr < 0 {
                return Err(VideoError::Unsupported(format!(
                    "the Media Foundation decoder won't take this {name} stream ({:#010x})",
                    hr as u32
                )));
            }
            me.choose_output()?;
            me.message(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING)?;
            me.message(MFT_MESSAGE_NOTIFY_START_OF_STREAM)?;
        }
        Ok(me)
    }

    /// Give the decoder a hardware device to decode on (DXVA), where there
    /// is one with video support and the decoder takes it.
    unsafe fn use_gpu(&mut self, lib: &Lib) {
        unsafe {
            let Some(create_manager) = lib.create_device_manager else {
                return;
            };
            let mut attrs: P = std::ptr::null_mut();
            if com!(self.mft.ptr(), MFT_GET_ATTRIBUTES, (*mut P) -> Hresult, &mut attrs) < 0
                || attrs.is_null()
            {
                return;
            }
            let attrs = Com(attrs);
            let mut aware = 0u32;
            if com!(attrs.ptr(), ATTR_GET_UINT32, (*const Guid, *mut u32) -> Hresult,
                &MF_SA_D3D11_AWARE, &mut aware)
                < 0
                || aware == 0
            {
                return;
            }
            let Some(d3d) = D3d::new(true) else {
                return;
            };
            // Media Foundation uses the device from its own threads too.
            if let Some(mt) = d3d.device.cast(&IID_ID3D10_MULTITHREAD) {
                com!(mt.ptr(), MULTITHREAD_SET_PROTECTED, (i32) -> i32, 1);
            }
            let (mut token, mut manager): (u32, P) = (0, std::ptr::null_mut());
            if create_manager(&mut token, &mut manager) < 0 || manager.is_null() {
                return;
            }
            let manager = Com(manager);
            if com!(manager.ptr(), MANAGER_RESET_DEVICE, (P, u32) -> Hresult, d3d.device.ptr(), token)
                < 0
            {
                return;
            }
            let hr = com!(self.mft.ptr(), MFT_PROCESS_MESSAGE, (u32, usize) -> Hresult,
                MFT_MESSAGE_SET_D3D_MANAGER, manager.ptr() as usize);
            if hr >= 0 {
                self.d3d = Some(d3d);
                self._manager = Some(manager);
            }
        }
    }

    unsafe fn message(&self, message: u32) -> Result<(), VideoError> {
        let hr = unsafe {
            com!(self.mft.ptr(), MFT_PROCESS_MESSAGE, (u32, usize) -> Hresult, message, 0)
        };
        check(hr, "ProcessMessage")
    }

    /// Pick NV12 among the output types offered and read its layout.
    unsafe fn choose_output(&mut self) -> Result<(), VideoError> {
        unsafe {
            for i in 0.. {
                let mut ty: P = std::ptr::null_mut();
                let hr = com!(self.mft.ptr(), MFT_GET_OUTPUT_AVAILABLE_TYPE,
                    (u32, u32, *mut P) -> Hresult, 0, i, &mut ty);
                if hr == MF_E_NO_MORE_TYPES || hr < 0 || ty.is_null() {
                    break;
                }
                let ty = Com(ty);
                let mut sub = Guid(0, 0, 0, [0; 8]);
                com!(ty.ptr(), ATTR_GET_GUID, (*const Guid, *mut Guid) -> Hresult, &MF_MT_SUBTYPE, &mut sub);
                if sub != MF_VIDEO_FORMAT_NV12 {
                    continue;
                }
                let hr = com!(self.mft.ptr(), MFT_SET_OUTPUT_TYPE, (u32, P, u32) -> Hresult, 0, ty.ptr(), 0);
                check(hr, "SetOutputType")?;
                self.read_layout(&ty);
                let mut info = OutputStreamInfo::default();
                let hr = com!(self.mft.ptr(), MFT_GET_OUTPUT_STREAM_INFO,
                    (u32, *mut OutputStreamInfo) -> Hresult, 0, &mut info);
                check(hr, "GetOutputStreamInfo")?;
                self.provides_samples = info.flags
                    & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES)
                    != 0;
                self.sample_size = info.size;
                self.sample_alignment = info.alignment;
                return Ok(());
            }
        }
        Err(VideoError::Unsupported(
            "the Media Foundation decoder gives no NV12".into(),
        ))
    }

    /// The output type's size, stride, visible area and color space.
    unsafe fn read_layout(&mut self, ty: &Com) {
        unsafe {
            let mut size = 0u64;
            if com!(ty.ptr(), ATTR_GET_UINT64, (*const Guid, *mut u64) -> Hresult, &MF_MT_FRAME_SIZE, &mut size)
                >= 0
            {
                let (w, h) = ((size >> 32) as u32, size as u32);
                if w > 0 && h > 0 {
                    self.stride = w;
                    self.rows = h;
                    // The container's size, where it's inside the decoder's
                    // (padded to whole macroblocks).
                    self.visible = PhysicalSize::new(
                        if self.container.width > 0 {
                            self.container.width.min(w)
                        } else {
                            w
                        },
                        if self.container.height > 0 {
                            self.container.height.min(h)
                        } else {
                            h
                        },
                    );
                }
            }
            let mut stride = 0u32;
            if com!(ty.ptr(), ATTR_GET_UINT32, (*const Guid, *mut u32) -> Hresult, &MF_MT_DEFAULT_STRIDE, &mut stride)
                >= 0
                && (stride as i32) > 0
            {
                self.stride = stride;
            }
            let mut area = VideoArea::default();
            let mut got = 0u32;
            if com!(ty.ptr(), ATTR_GET_BLOB, (*const Guid, *mut VideoArea, u32, *mut u32) -> Hresult,
                &MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, size_of::<VideoArea>() as u32, &mut got)
                >= 0
                && area.cx > 0
                && area.cy > 0
                && area.x == 0
                && area.y == 0
            {
                self.visible = PhysicalSize::new(area.cx as u32, area.cy as u32);
            }
            let (mut matrix, mut range) = (0u32, 0u32);
            com!(ty.ptr(), ATTR_GET_UINT32, (*const Guid, *mut u32) -> Hresult, &MF_MT_YUV_MATRIX, &mut matrix);
            com!(ty.ptr(), ATTR_GET_UINT32, (*const Guid, *mut u32) -> Hresult, &MF_MT_VIDEO_NOMINAL_RANGE, &mut range);
            let bt709 = match matrix {
                0 => self.visible.height > 576,
                m => m == MATRIX_BT709,
            };
            self.color = match (bt709, range == RANGE_FULL) {
                (true, false) => YuvColorSpace::Bt709Limited,
                (true, true) => YuvColorSpace::Bt709Full,
                (false, false) => YuvColorSpace::Bt601Limited,
                (false, true) => YuvColorSpace::Bt601Full,
            };
        }
    }

    /// Take every picture the decoder has ready.
    unsafe fn drain(&mut self) -> Result<(), VideoError> {
        let lib = lib().ok_or_else(|| VideoError::Decode("Media Foundation went".into()))?;
        // A stream change is followed by a picture: a bounded number of
        // rounds catches a decoder that keeps changing.
        for _ in 0..64 {
            unsafe {
                let ours = if self.provides_samples {
                    None
                } else {
                    let sample = new_sample(lib)?;
                    let mut buffer: P = std::ptr::null_mut();
                    let align = self.sample_alignment.saturating_sub(1);
                    let size = self.sample_size.max(self.stride * self.rows * 3 / 2);
                    check(
                        (lib.create_aligned_buffer)(size, align, &mut buffer),
                        "MFCreateAlignedMemoryBuffer",
                    )?;
                    let buffer = Com(buffer);
                    check(
                        com!(sample.ptr(), SAMPLE_ADD_BUFFER, (P) -> Hresult, buffer.ptr()),
                        "AddBuffer",
                    )?;
                    Some(sample)
                };
                let mut db = OutputDataBuffer {
                    stream: 0,
                    sample: ours.as_ref().map_or(std::ptr::null_mut(), Com::ptr),
                    status: 0,
                    events: std::ptr::null_mut(),
                };
                let mut status = 0u32;
                let hr = com!(self.mft.ptr(), MFT_PROCESS_OUTPUT,
                    (u32, u32, *mut OutputDataBuffer, *mut u32) -> Hresult, 0, 1, &mut db, &mut status);
                if !db.events.is_null() {
                    release(db.events);
                }
                // The decoder's own sample is ours to release now.
                let given = (ours.is_none() && !db.sample.is_null()).then(|| Com(db.sample));
                match hr {
                    MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.choose_output()?;
                        continue;
                    }
                    hr if hr < 0 => return check(hr, "ProcessOutput"),
                    _ => {}
                }
                let Some(sample) = ours.or(given) else {
                    continue;
                };
                self.take(&sample);
            }
        }
        Ok(())
    }

    /// A decoded sample as a frame.
    unsafe fn take(&mut self, sample: &Com) {
        unsafe {
            let mut t = 0i64;
            let time = if com!(sample.ptr(), SAMPLE_GET_TIME, (*mut i64) -> Hresult, &mut t) >= 0
                && t >= 0
            {
                Duration::from_nanos(t as u64 * 100)
            } else {
                self.last + self.frame_time
            };
            self.last = time;
            let mut buffer: P = std::ptr::null_mut();
            if com!(sample.ptr(), SAMPLE_GET_BUFFER, (u32, *mut P) -> Hresult, 0, &mut buffer) < 0
                || buffer.is_null()
            {
                return;
            }
            let buffer = Com(buffer);
            let image = match buffer.cast(&IID_IMF_DXGI_BUFFER) {
                Some(dxgi) => self.texture_frame(&dxgi),
                None => self.memory_frame(sample),
            };
            if let Some(image) = image {
                self.out.push(Decoded { time, image });
            }
        }
    }

    /// The visible size, even (NV12's chroma is in 2×2 blocks).
    fn even_size(&self) -> PhysicalSize {
        PhysicalSize::new(self.visible.width & !1, self.visible.height & !1)
    }

    /// A picture in a texture of the decoder's (DXVA): copied on the GPU
    /// into a shared texture while the renderer imports NV12, else read
    /// back.
    unsafe fn texture_frame(&mut self, dxgi: &Com) -> Option<Arc<ExternalImage>> {
        unsafe {
            let d3d = self.d3d.as_ref()?;
            let mut tex: P = std::ptr::null_mut();
            if com!(dxgi.ptr(), DXGI_BUFFER_GET_RESOURCE, (*const Guid, *mut P) -> Hresult,
                &IID_ID3D11_TEXTURE2D, &mut tex)
                < 0
                || tex.is_null()
            {
                return None;
            }
            let tex = Com(tex);
            let mut sub = 0u32;
            com!(dxgi.ptr(), DXGI_BUFFER_GET_SUBRESOURCE, (*mut u32) -> Hresult, &mut sub);
            let size = self.even_size();
            let region = D3dBox {
                left: 0,
                top: 0,
                front: 0,
                right: size.width,
                bottom: size.height,
                back: 1,
            };
            let native = crate::platform::external::imported_natively(ExternalFormat::Nv12);
            let desc = if native {
                Texture2dDesc::nv12(
                    size.width,
                    size.height,
                    D3D11_USAGE_DEFAULT,
                    D3D11_BIND_SHADER_RESOURCE,
                    0,
                    D3D11_RESOURCE_MISC_SHARED,
                )
            } else {
                Texture2dDesc::nv12(
                    size.width,
                    size.height,
                    D3D11_USAGE_STAGING,
                    0,
                    D3D11_CPU_ACCESS_READ,
                    0,
                )
            };
            let copy = d3d.texture(&desc, None)?;
            com!(d3d.ctx.ptr(), CTX_COPY_SUBRESOURCE_REGION,
                (P, u32, u32, u32, u32, P, u32, *const D3dBox) -> (),
                copy.ptr(), 0, 0, 0, 0, tex.ptr(), sub, &region);
            if native {
                return d3d.shared_image(copy, size, self.color);
            }
            let mut mapped = Mapped {
                data: std::ptr::null_mut(),
                row_pitch: 0,
                depth_pitch: 0,
            };
            if com!(d3d.ctx.ptr(), CTX_MAP, (P, u32, u32, u32, *mut Mapped) -> Hresult,
                copy.ptr(), 0, D3D11_MAP_READ, 0, &mut mapped)
                < 0
                || mapped.data.is_null()
            {
                return None;
            }
            let pitch = mapped.row_pitch as usize;
            let rows = size.height as usize;
            let bytes = std::slice::from_raw_parts(mapped.data as *const u8, pitch * rows * 3 / 2);
            let image = nv12::image(
                size,
                (&bytes[..pitch * rows], pitch),
                (&bytes[pitch * rows..], pitch),
                self.color,
            );
            com!(d3d.ctx.ptr(), CTX_UNMAP, (P, u32) -> (), copy.ptr(), 0);
            image
        }
    }

    /// A picture in memory: uploaded into a shared texture while the
    /// renderer imports NV12, else converted.
    unsafe fn memory_frame(&mut self, sample: &Com) -> Option<Arc<ExternalImage>> {
        unsafe {
            let mut buffer: P = std::ptr::null_mut();
            if com!(sample.ptr(), SAMPLE_CONTIGUOUS, (*mut P) -> Hresult, &mut buffer) < 0
                || buffer.is_null()
            {
                return None;
            }
            let buffer = Com(buffer);
            let mut len = 0u32;
            com!(buffer.ptr(), BUFFER_GET_LENGTH, (*mut u32) -> Hresult, &mut len);
            // The 2D lock gives the real pitch; a plain lock, the type's.
            let two_d = buffer.cast(&IID_IMF_2D_BUFFER);
            let (mut data, mut pitch): (*mut u8, i32) = (std::ptr::null_mut(), 0);
            let locked = match &two_d {
                Some(b) => {
                    com!(b.ptr(), BUFFER2D_LOCK, (*mut *mut u8, *mut i32) -> Hresult, &mut data, &mut pitch)
                        >= 0
                }
                None => {
                    pitch = self.stride as i32;
                    let (mut max, mut cur) = (0u32, 0u32);
                    com!(buffer.ptr(), BUFFER_LOCK, (*mut *mut u8, *mut u32, *mut u32) -> Hresult,
                        &mut data, &mut max, &mut cur)
                        >= 0
                }
            };
            if !locked || data.is_null() || pitch <= 0 {
                return None;
            }
            let pitch = pitch as usize;
            // The chroma follows the padded rows; where the buffer is
            // shorter than the type says, it follows the rows it holds.
            let mut rows = self.rows as usize;
            if (len as usize) < pitch * rows * 3 / 2 && pitch > 0 {
                rows = len as usize / pitch * 2 / 3;
            }
            let bytes = std::slice::from_raw_parts(data, pitch * rows * 3 / 2);
            let size = self.even_size();
            let (y, uv) = (&bytes[..pitch * rows], &bytes[pitch * rows..]);
            let image = if crate::platform::external::imported_natively(ExternalFormat::Nv12) {
                self.upload(size, y, uv, pitch)
            } else {
                None
            }
            .or_else(|| nv12::image(size, (y, pitch), (uv, pitch), self.color));
            match &two_d {
                Some(b) => {
                    com!(b.ptr(), BUFFER2D_UNLOCK, () -> Hresult);
                }
                None => {
                    com!(buffer.ptr(), BUFFER_UNLOCK, () -> Hresult);
                }
            }
            image
        }
    }

    /// A software-decoded picture uploaded into a shared NV12 texture.
    unsafe fn upload(
        &mut self,
        size: PhysicalSize,
        y: &[u8],
        uv: &[u8],
        pitch: usize,
    ) -> Option<Arc<ExternalImage>> {
        let color = self.color;
        let d3d = self
            .upload
            .get_or_insert_with(|| unsafe { D3d::new(false) })
            .as_ref()?;
        let h = size.height as usize;
        // The texture's initial data: its planes back to back, at `pitch`.
        let mut packed = Vec::with_capacity(pitch * h * 3 / 2);
        packed.extend_from_slice(y.get(..pitch * h)?);
        packed.extend_from_slice(uv.get(..pitch * (h / 2))?);
        let desc = Texture2dDesc::nv12(
            size.width,
            size.height,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_SHADER_RESOURCE,
            0,
            D3D11_RESOURCE_MISC_SHARED,
        );
        let data = SubresourceData {
            data: packed.as_ptr().cast(),
            pitch: pitch as u32,
            slice_pitch: 0,
        };
        unsafe {
            let tex = d3d.texture(&desc, Some(&data))?;
            d3d.shared_image(tex, size, color)
        }
    }
}

/// The decoder for `subtype`: the Microsoft one by its CLSID, else the
/// first synchronous one Media Foundation lists.
unsafe fn instantiate(lib: &Lib, clsid: &Guid, subtype: &Guid) -> Option<Com> {
    unsafe {
        let mut p: P = std::ptr::null_mut();
        if CoCreateInstance(
            clsid,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IMF_TRANSFORM,
            &mut p,
        ) >= 0
            && !p.is_null()
        {
            return Some(Com(p));
        }
        let enum_ex = lib.enum_ex?;
        let input = TypeInfo {
            major: MF_MEDIA_TYPE_VIDEO,
            sub: *subtype,
        };
        let (mut list, mut count): (*mut P, u32) = (std::ptr::null_mut(), 0);
        let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER;
        if enum_ex(
            MFT_CATEGORY_VIDEO_DECODER,
            flags,
            &input,
            std::ptr::null(),
            &mut list,
            &mut count,
        ) < 0
            || list.is_null()
        {
            return None;
        }
        let activates: Vec<Com> = (0..count as usize).map(|i| Com(*list.add(i))).collect();
        CoTaskMemFree(list.cast());
        activates.iter().find_map(|a| {
            let mut p: P = std::ptr::null_mut();
            (com!(a.ptr(), ACTIVATE_OBJECT, (*const Guid, *mut P) -> Hresult, &IID_IMF_TRANSFORM, &mut p) >= 0
                && !p.is_null())
            .then(|| Com(p))
        })
    }
}

unsafe fn media_type(lib: &Lib) -> Result<Com, VideoError> {
    let mut p: P = std::ptr::null_mut();
    check(
        unsafe { (lib.create_media_type)(&mut p) },
        "MFCreateMediaType",
    )?;
    Ok(Com(p))
}

unsafe fn new_sample(lib: &Lib) -> Result<Com, VideoError> {
    let mut p: P = std::ptr::null_mut();
    check(unsafe { (lib.create_sample)(&mut p) }, "MFCreateSample")?;
    Ok(Com(p))
}

unsafe fn set_guid(obj: &Com, key: &Guid, value: &Guid) -> Hresult {
    unsafe { com!(obj.ptr(), ATTR_SET_GUID, (*const Guid, *const Guid) -> Hresult, key, value) }
}

/// A duration in Media Foundation's 100-nanosecond units.
fn hns(t: Duration) -> i64 {
    (t.as_nanos() / 100).min(i64::MAX as u128) as i64
}

impl FrameDecoder for MfDecoder {
    fn send(&mut self, packet: Coded<'_>) -> Result<(), VideoError> {
        enter_mta();
        let lib = lib().ok_or_else(|| VideoError::Decode("Media Foundation went".into()))?;
        let sets: &[Vec<u8>] = if self.need_sets { &self.sets } else { &[] };
        let data = annex_b(packet.data, self.len_size, sets);
        if data.is_empty() {
            return Ok(());
        }
        self.need_sets = false;
        unsafe {
            let mut buffer: P = std::ptr::null_mut();
            check(
                (lib.create_memory_buffer)(data.len() as u32, &mut buffer),
                "MFCreateMemoryBuffer",
            )?;
            let buffer = Com(buffer);
            let (mut ptr, mut max, mut cur): (*mut u8, u32, u32) = (std::ptr::null_mut(), 0, 0);
            check(
                com!(buffer.ptr(), BUFFER_LOCK, (*mut *mut u8, *mut u32, *mut u32) -> Hresult,
                &mut ptr, &mut max, &mut cur),
                "Lock",
            )?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            com!(buffer.ptr(), BUFFER_UNLOCK, () -> Hresult);
            com!(buffer.ptr(), BUFFER_SET_LENGTH, (u32) -> Hresult, data.len() as u32);
            let sample = new_sample(lib)?;
            check(
                com!(sample.ptr(), SAMPLE_ADD_BUFFER, (P) -> Hresult, buffer.ptr()),
                "AddBuffer",
            )?;
            if let Some(pts) = packet.pts {
                com!(sample.ptr(), SAMPLE_SET_TIME, (i64) -> Hresult, hns(pts));
            }
            com!(sample.ptr(), SAMPLE_SET_DURATION, (i64) -> Hresult, hns(self.frame_time));
            for _ in 0..64 {
                let hr = com!(self.mft.ptr(), MFT_PROCESS_INPUT, (u32, P, u32) -> Hresult, 0, sample.ptr(), 0);
                if hr == MF_E_NOTACCEPTING {
                    // Full: take its pictures, then it takes this one.
                    self.drain()?;
                    continue;
                }
                // A packet it can't decode is skipped, as players do.
                if hr >= 0 {
                    break;
                }
                return Ok(());
            }
            self.drain()
        }
    }

    fn receive(&mut self) -> Vec<Decoded> {
        std::mem::take(&mut self.out)
    }

    fn flush(&mut self) -> Result<(), VideoError> {
        enter_mta();
        unsafe {
            self.message(MFT_MESSAGE_NOTIFY_END_OF_STREAM)?;
            self.message(MFT_MESSAGE_COMMAND_DRAIN)?;
            self.drain()
        }
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        enter_mta();
        self.out.clear();
        self.need_sets = true;
        unsafe {
            self.message(MFT_MESSAGE_COMMAND_FLUSH)?;
            self.message(MFT_MESSAGE_NOTIFY_START_OF_STREAM)
        }
    }

    fn reorder_depth(&self) -> usize {
        // It gives its pictures in presentation order already.
        1
    }

    fn name(&self) -> &'static str {
        "Media Foundation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_abi_matches_the_headers() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(size_of::<VideoArea>(), 16);
        assert_eq!(size_of::<Texture2dDesc>(), 44);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<OutputDataBuffer>(), 32);
        assert_eq!(
            MF_VIDEO_FORMAT_NV12,
            Guid(
                0x3231_564E,
                0,
                0x10,
                [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]
            )
        );
        assert_eq!(MF_MEDIA_TYPE_VIDEO.0, 0x7364_6976);
        assert_eq!(hns(Duration::from_millis(100)), 1_000_000);
    }
}
