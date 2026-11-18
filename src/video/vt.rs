//! H.264 and HEVC through VideoToolbox (macOS, iOS), the media engine's
//! hardware decoder: a `VTDecompressionSession` asked for NV12 (`420v`)
//! `CVPixelBuffer`s backed by an `IOSurface` and Metal-compatible, each
//! decoded buffer handed over as an [`ExternalImage`] of its surface
//! (retained until the image goes, so the session's pool doesn't reuse it
//! while it's drawn). Metal binds the surface's planes as textures and
//! converts them in its shader: no pixel is copied from the decoder to the
//! screen. The software renderer locks the surface and converts it.
//!
//! VideoToolbox, CoreMedia and CoreVideo are loaded when the first video
//! opens (`dlopen`), as the camera loads AVFoundation. (`oxideav-videotoolbox`
//! decodes through the same API but copies every frame into its own planes,
//! so it isn't used.)

use super::VideoError;
use super::nal::{parse_avcc, parse_hvcc};
use super::player::{Coded, Decoded, FrameDecoder};
use crate::platform::external::apple::{CFRelease, CFRetain};
use crate::platform::native::objc::{load_framework, symbol};
use crate::render::ExternalImage;
use crate::render::external::YuvColorSpace;
use oxideav_core::CodecParameters;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

type Ref = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const TIME_VALID: u32 = 1;
const INVALID_TIME: CmTime = CmTime {
    value: 0,
    timescale: 0,
    flags: 0,
    epoch: 0,
};

#[repr(C)]
struct SampleTiming {
    duration: CmTime,
    pts: CmTime,
    dts: CmTime,
}

type OutputFn = unsafe extern "C" fn(Ref, Ref, i32, u32, Ref, CmTime, CmTime);

#[repr(C)]
struct CallbackRecord {
    callback: OutputFn,
    refcon: Ref,
}

type CfRef = *const c_void;

/// Opaque stand-in for the `CFDictionary{Key,Value}CallBacks` structs:
/// only the real globals' addresses are taken.
#[repr(C)]
struct CFCallbacks {
    _opaque: [u8; 0],
}

// As the other backends declare them (one signature per symbol).
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryCreate(
        allocator: CfRef,
        keys: *const CfRef,
        values: *const CfRef,
        num_values: isize,
        key_callbacks: *const CFCallbacks,
        value_callbacks: *const CFCallbacks,
    ) -> CfRef;
    fn CFNumberCreate(allocator: CfRef, the_type: isize, value: *const c_void) -> CfRef;
    fn CFEqual(a: CfRef, b: CfRef) -> u8;
    static kCFTypeDictionaryKeyCallBacks: CFCallbacks;
    static kCFTypeDictionaryValueCallBacks: CFCallbacks;
    static kCFBooleanTrue: CfRef;
}

/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`.
const FORMAT_420V: i32 = i32::from_be_bytes(*b"420v");
const CF_NUMBER_SINT32: isize = 3;
/// `kVTDecodeFrame_EnableAsynchronousDecompression` is left off: each
/// frame is decoded before `VTDecompressionSessionDecodeFrame` returns.
const DECODE_FLAGS: u32 = 0;

/// The functions and constants used, resolved on first use.
struct Lib {
    create_h264:
        unsafe extern "C" fn(Ref, usize, *const *const u8, *const usize, i32, *mut Ref) -> i32,
    create_hevc:
        unsafe extern "C" fn(Ref, usize, *const *const u8, *const usize, i32, Ref, *mut Ref) -> i32,
    session_create:
        unsafe extern "C" fn(Ref, Ref, Ref, Ref, *const CallbackRecord, *mut Ref) -> i32,
    decode: unsafe extern "C" fn(Ref, Ref, u32, Ref, *mut u32) -> i32,
    wait: unsafe extern "C" fn(Ref) -> i32,
    finish_delayed: unsafe extern "C" fn(Ref) -> i32,
    invalidate: unsafe extern "C" fn(Ref),
    block_create:
        unsafe extern "C" fn(Ref, Ref, usize, Ref, Ref, usize, usize, u32, *mut Ref) -> i32,
    block_replace: unsafe extern "C" fn(*const c_void, Ref, usize, usize) -> i32,
    sample_create: unsafe extern "C" fn(
        Ref,
        Ref,
        Ref,
        isize,
        isize,
        *const SampleTiming,
        isize,
        *const usize,
        *mut Ref,
    ) -> i32,
    io_surface: unsafe extern "C" fn(Ref) -> Ref,
    attachment: unsafe extern "C" fn(Ref, Ref, *mut u32) -> Ref,
    format_key: usize,
    io_surface_key: usize,
    metal_key: usize,
    matrix_key: usize,
    matrix_709: usize,
}

// Each symbol's type is its field's.
#[allow(clippy::missing_transmute_annotations)]
fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let vt = load_framework("VideoToolbox");
        let cm = load_framework("CoreMedia");
        let cv = load_framework("CoreVideo");
        if vt.is_null() || cm.is_null() || cv.is_null() {
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
        macro_rules! k {
            ($h:expr, $name:literal) => {{
                let p = symbol($h, $name) as *const Ref;
                if p.is_null() {
                    return None;
                }
                *p as usize
            }};
        }
        Some(Lib {
            create_h264: f!(cm, c"CMVideoFormatDescriptionCreateFromH264ParameterSets"),
            create_hevc: f!(cm, c"CMVideoFormatDescriptionCreateFromHEVCParameterSets"),
            session_create: f!(vt, c"VTDecompressionSessionCreate"),
            decode: f!(vt, c"VTDecompressionSessionDecodeFrame"),
            wait: f!(vt, c"VTDecompressionSessionWaitForAsynchronousFrames"),
            finish_delayed: f!(vt, c"VTDecompressionSessionFinishDelayedFrames"),
            invalidate: f!(vt, c"VTDecompressionSessionInvalidate"),
            block_create: f!(cm, c"CMBlockBufferCreateWithMemoryBlock"),
            block_replace: f!(cm, c"CMBlockBufferReplaceDataBytes"),
            sample_create: f!(cm, c"CMSampleBufferCreateReady"),
            io_surface: f!(cv, c"CVPixelBufferGetIOSurface"),
            attachment: f!(cv, c"CVBufferGetAttachment"),
            format_key: k!(cv, c"kCVPixelBufferPixelFormatTypeKey"),
            io_surface_key: k!(cv, c"kCVPixelBufferIOSurfacePropertiesKey"),
            metal_key: k!(cv, c"kCVPixelBufferMetalCompatibilityKey"),
            matrix_key: k!(cv, c"kCVImageBufferYCbCrMatrixKey"),
            matrix_709: k!(cv, c"kCVImageBufferYCbCrMatrix_ITU_R_709_2"),
        })
    })
    .as_ref()
}

/// Which codec, and its parameter sets from the container's
/// configuration record.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    H264,
    Hevc,
}

/// What the output callback fills.
#[derive(Default)]
struct Output {
    frames: Vec<Decoded>,
}

pub(super) struct VtDecoder {
    session: Ref,
    format: Ref,
    /// Boxed so the callback's pointer to it stays put.
    output: Box<Mutex<Output>>,
}

// The session and format description are CF objects any thread may use;
// the decoder is used by one thread at a time.
unsafe impl Send for VtDecoder {}

impl VtDecoder {
    pub(super) fn new(params: &CodecParameters) -> Result<Self, VideoError> {
        let lib = lib().ok_or_else(|| VideoError::Unsupported("no VideoToolbox".into()))?;
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
        if sets.len() < 2 {
            return Err(VideoError::Decode("no parameter sets".into()));
        }
        let ptrs: Vec<*const u8> = sets.iter().map(|s| s.as_ptr()).collect();
        let sizes: Vec<usize> = sets.iter().map(|s| s.len()).collect();
        let null = std::ptr::null_mut();
        let mut format: Ref = null;
        let st = unsafe {
            match codec {
                Codec::H264 => (lib.create_h264)(
                    null,
                    ptrs.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    len_size,
                    &mut format,
                ),
                Codec::Hevc => (lib.create_hevc)(
                    null,
                    ptrs.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    len_size,
                    null,
                    &mut format,
                ),
            }
        };
        if st != 0 || format.is_null() {
            return Err(VideoError::Decode(format!(
                "CMVideoFormatDescriptionCreate: OSStatus {st}"
            )));
        }
        let output: Box<Mutex<Output>> = Box::default();
        let session = unsafe {
            let attrs = destination_attributes(lib);
            let record = CallbackRecord {
                callback: on_output,
                refcon: &*output as *const Mutex<Output> as Ref,
            };
            let mut session: Ref = null;
            let st = (lib.session_create)(null, format, null, attrs, &record, &mut session);
            CFRelease(attrs);
            if st != 0 || session.is_null() {
                CFRelease(format);
                // -12906 kVTCouldNotFindVideoDecoderErr, -12911
                // kVTVideoDecoderMalfunctionErr, -12210 ...: no decoder
                // for this stream here.
                return Err(VideoError::Unsupported(format!(
                    "VideoToolbox can't decode this stream (OSStatus {st})"
                )));
            }
            session
        };
        Ok(Self {
            session,
            format,
            output,
        })
    }

    /// Wait for what VideoToolbox still holds.
    fn finish(&mut self) {
        if let Some(lib) = lib() {
            unsafe {
                (lib.finish_delayed)(self.session);
                (lib.wait)(self.session);
            }
        }
    }
}

/// The session's output: NV12 in the video range, in an `IOSurface`
/// Metal can bind.
unsafe fn destination_attributes(lib: &Lib) -> Ref {
    unsafe {
        let null = std::ptr::null();
        let format = CFNumberCreate(null, CF_NUMBER_SINT32, &FORMAT_420V as *const i32 as _);
        let empty = CFDictionaryCreate(
            null,
            std::ptr::null(),
            std::ptr::null(),
            0,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let keys = [
            lib.format_key as CfRef,
            lib.io_surface_key as CfRef,
            lib.metal_key as CfRef,
        ];
        let values = [format, empty, kCFBooleanTrue];
        let dict = CFDictionaryCreate(
            null,
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        CFRelease(format);
        CFRelease(empty);
        dict as Ref
    }
}

/// The decompression output callback: keep the buffer as an image of its
/// surface.
unsafe extern "C" fn on_output(
    refcon: Ref,
    _frame: Ref,
    status: i32,
    _flags: u32,
    buffer: Ref,
    pts: CmTime,
    _duration: CmTime,
) {
    let output = unsafe { &*(refcon as *const Mutex<Output>) };
    let mut out = output.lock().unwrap_or_else(|e| e.into_inner());
    if status != 0 {
        // A frame that didn't decode (bad data): skipped, as players do.
        return;
    }
    let Some(lib) = lib() else {
        return;
    };
    if buffer.is_null() {
        // A frame dropped by the decoder.
        return;
    }
    let time = if pts.flags & TIME_VALID != 0 && pts.timescale > 0 && pts.value >= 0 {
        Duration::from_nanos(
            (i128::from(pts.value) * 1_000_000_000 / i128::from(pts.timescale)) as u64,
        )
    } else {
        Duration::ZERO
    };
    let image = unsafe {
        let surface = (lib.io_surface)(buffer);
        let Ok(image) = ExternalImage::io_surface(surface) else {
            return;
        };
        let matrix = (lib.attachment)(buffer, lib.matrix_key as Ref, std::ptr::null_mut());
        let bt709 = !matrix.is_null() && CFEqual(matrix, lib.matrix_709 as CfRef) != 0;
        CFRetain(buffer);
        let held = buffer as usize;
        let image = image.on_release(move || CFRelease(held as *const c_void));
        if bt709 {
            image.color_space(YuvColorSpace::Bt709Limited)
        } else {
            image
        }
    };
    out.frames.push(Decoded {
        time,
        image: Arc::new(image),
    });
}

impl FrameDecoder for VtDecoder {
    fn send(&mut self, packet: Coded<'_>) -> Result<(), VideoError> {
        let lib = lib().ok_or_else(|| VideoError::Unsupported("no VideoToolbox".into()))?;
        if packet.data.is_empty() {
            return Ok(());
        }
        let null = std::ptr::null_mut();
        unsafe {
            let mut block: Ref = null;
            let len = packet.data.len();
            // kCMBlockBufferAssureMemoryNowFlag: allocate it now, then copy
            // the packet in (the session may outlive the packet's bytes).
            let st = (lib.block_create)(null, null, len, null, null, 0, len, 1, &mut block);
            if st != 0 || block.is_null() {
                return Err(VideoError::Decode(format!("CMBlockBufferCreate: {st}")));
            }
            (lib.block_replace)(packet.data.as_ptr() as *const c_void, block, 0, len);
            let timing = SampleTiming {
                duration: INVALID_TIME,
                pts: match packet.pts {
                    Some(t) => CmTime {
                        value: t.as_micros().min(i64::MAX as u128) as i64,
                        timescale: 1_000_000,
                        flags: TIME_VALID,
                        epoch: 0,
                    },
                    None => INVALID_TIME,
                },
                dts: INVALID_TIME,
            };
            let mut sample: Ref = null;
            let st = (lib.sample_create)(
                null,
                block,
                self.format,
                1,
                1,
                &timing,
                1,
                &len,
                &mut sample,
            );
            CFRelease(block);
            if st != 0 || sample.is_null() {
                return Err(VideoError::Decode(format!("CMSampleBufferCreate: {st}")));
            }
            let mut info = 0u32;
            let st = (lib.decode)(self.session, sample, DECODE_FLAGS, null, &mut info);
            CFRelease(sample);
            // A bad frame (-12909 kVTVideoDecoderBadDataErr) is skipped, as
            // players do; the session goes on.
            if st == -12903 {
                // kVTInvalidSessionErr: the session is gone (the GPU reset).
                return Err(VideoError::Decode("the VideoToolbox session ended".into()));
            }
        }
        Ok(())
    }

    fn receive(&mut self) -> Vec<Decoded> {
        let mut out = self.output.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut out.frames)
    }

    fn flush(&mut self) -> Result<(), VideoError> {
        self.finish();
        Ok(())
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        self.finish();
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frames
            .clear();
        Ok(())
    }

    fn reorder_depth(&self) -> usize {
        // Decoded in decode order: B-frames come before the reference
        // shown before them.
        4
    }

    fn name(&self) -> &'static str {
        "VideoToolbox"
    }
}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        if let Some(lib) = lib() {
            unsafe {
                (lib.wait)(self.session);
                (lib.invalidate)(self.session);
                CFRelease(self.session);
                CFRelease(self.format);
            }
        }
    }
}
