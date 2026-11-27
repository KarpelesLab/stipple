//! H.264 and HEVC through `MediaCodec` (Android), the NDK's
//! `AMediaCodec` in `libmediandk.so` (loaded when the first video opens):
//! the device's decoder, in hardware where it has one, else the system's
//! software one (the emulator's).
//!
//! The demuxer's length-prefixed access units go in as Annex B (start
//! codes), the `avcC`/`hvcC` record's parameter sets as the format's
//! `csd-0`/`csd-1` (and in-band again after a seek), each with its
//! presentation time; pictures come out in presentation order.
//!
//! The decoder renders into an `AImageReader` of `YUV_420_888` images made
//! for the GPU to sample and the CPU to read (`AImageReader_newWithUsage`,
//! Android 8). While the renderer imports such buffers (GLES through an
//! external texture, Vulkan through a sampler YCbCr conversion, as the
//! camera's are), each picture's `AHardwareBuffer` is the frame, the
//! `AImage` kept out of the reader until the frame is dropped: nothing is
//! copied. Otherwise (and for a picture whose visible part is smaller than
//! its buffer) its planes are read and converted once. Where the reader
//! can't be made or the decoder won't render into it, the decoder writes
//! into its own buffers (`ByteBuffer` mode: planar or semi-planar 4:2:0, as
//! its output format says), converted the same way.
//!
//! A native executable (the tests, run over `adb`) has no binder threads
//! for the decoder's service to call back on: they're started
//! (`ABinderProcess_startThreadPool`, a no-op in an app, whose runtime
//! started them).

use super::VideoError;
use super::nal::{annex_b, parse_avcc, parse_hvcc};
use super::nv12::{self, Chroma, Layout};
use super::player::{Coded, Decoded, FrameDecoder};
use crate::geometry::PhysicalSize;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalFormat, ExternalImage};
use oxideav_core::CodecParameters;
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

type P = *mut c_void;

/// `AMediaCodecBufferInfo`.
#[repr(C)]
#[derive(Default)]
struct BufferInfo {
    offset: i32,
    size: i32,
    pts_us: i64,
    flags: u32,
}

/// `AImageCropRect`.
#[repr(C)]
#[derive(Default, PartialEq, Eq, Debug)]
struct CropRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

const BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
const BUFFER_FLAG_END_OF_STREAM: u32 = 4;
const INFO_TRY_AGAIN_LATER: isize = -1;
const INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
const INFO_OUTPUT_BUFFERS_CHANGED: isize = -3;
const AIMAGE_FORMAT_YUV_420_888: i32 = 0x23;
const USAGE_CPU_READ_OFTEN: u64 = 3;
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// Images the reader lends: the few the player holds ahead, the one shown,
/// the renderer's last, and room for the decoder to render the next.
const MAX_IMAGES: i32 = 8;
dlsym::library! {
    /// The NDK's MediaCodec functions used.
    struct Lib {
        unsafe extern "C" {
            #[link_name = "AMediaCodec_createDecoderByType"]
            fn create_by_type(a: *const c_char) -> P;
            #[link_name = "AMediaCodec_configure"]
            fn configure(a: P, b: P, c: P, d: P, e: u32) -> i32;
            #[link_name = "AMediaCodec_start"]
            fn start(a: P) -> i32;
            #[link_name = "AMediaCodec_stop"]
            fn stop(a: P) -> i32;
            #[link_name = "AMediaCodec_delete"]
            fn delete(a: P) -> i32;
            #[link_name = "AMediaCodec_flush"]
            fn flush(a: P) -> i32;
            #[link_name = "AMediaCodec_dequeueInputBuffer"]
            fn dequeue_input(a: P, b: i64) -> isize;
            #[link_name = "AMediaCodec_getInputBuffer"]
            fn input_buffer(a: P, b: usize, c: *mut usize) -> *mut u8;
            #[link_name = "AMediaCodec_queueInputBuffer"]
            fn queue_input(a: P, b: usize, c: isize, d: usize, e: u64, f: u32) -> i32;
            #[link_name = "AMediaCodec_dequeueOutputBuffer"]
            fn dequeue_output(a: P, b: *mut BufferInfo, c: i64) -> isize;
            #[link_name = "AMediaCodec_getOutputBuffer"]
            fn output_buffer(a: P, b: usize, c: *mut usize) -> *mut u8;
            #[link_name = "AMediaCodec_getOutputFormat"]
            fn output_format(a: P) -> P;
            #[link_name = "AMediaCodec_releaseOutputBuffer"]
            fn release_output(a: P, b: usize, c: bool) -> i32;
            #[link_name = "AMediaFormat_new"]
            fn format_new() -> P;
            #[link_name = "AMediaFormat_delete"]
            fn format_delete(a: P) -> i32;
            #[link_name = "AMediaFormat_setString"]
            fn set_string(a: P, b: *const c_char, c: *const c_char);
            #[link_name = "AMediaFormat_setInt32"]
            fn set_i32(a: P, b: *const c_char, c: i32);
            #[link_name = "AMediaFormat_setBuffer"]
            fn set_buffer(a: P, b: *const c_char, c: *const c_void, d: usize);
            #[link_name = "AMediaFormat_getInt32"]
            fn get_i32(a: P, b: *const c_char, c: *mut i32) -> bool;
        }
    }
}

dlsym::library! {
    /// The image reader the decoder renders into, where the system has
    /// `AImageReader_newWithUsage` (Android 8).
    struct ReaderLib {
        unsafe extern "C" {
            #[link_name = "AImageReader_newWithUsage"]
            fn new_with_usage(a: i32, b: i32, c: i32, d: u64, e: i32, f: *mut P) -> i32;
            #[link_name = "AImageReader_getWindow"]
            fn window(a: P, b: *mut P) -> i32;
            #[link_name = "AImageReader_acquireNextImage"]
            fn acquire_next(a: P, b: *mut P) -> i32;
            #[link_name = "AImageReader_delete"]
            fn delete(a: P);
            #[link_name = "AImage_delete"]
            fn image_delete(a: P);
            #[link_name = "AImage_getTimestamp"]
            fn timestamp(a: P, b: *mut i64) -> i32;
            #[link_name = "AImage_getWidth"]
            fn width(a: P, b: *mut i32) -> i32;
            #[link_name = "AImage_getHeight"]
            fn height(a: P, b: *mut i32) -> i32;
            #[link_name = "AImage_getCropRect"]
            fn crop(a: P, b: *mut CropRect) -> i32;
            #[link_name = "AImage_getPlaneData"]
            fn plane_data(a: P, b: c_int, c: *mut *mut u8, d: *mut c_int) -> i32;
            #[link_name = "AImage_getPlaneRowStride"]
            fn row_stride(a: P, b: c_int, c: *mut i32) -> i32;
            #[link_name = "AImage_getPlanePixelStride"]
            fn pixel_stride(a: P, b: c_int, c: *mut i32) -> i32;
            #[link_name = "AImage_getHardwareBuffer"]
            fn hardware_buffer(a: P, b: *mut P) -> i32;
        }
    }
}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: a system library, trusted like the rest of the platform.
        unsafe { <Lib as dlsym::Bindings>::open("libmediandk.so") }.ok()
    })
    .as_ref()
}

fn reader_lib() -> Option<&'static ReaderLib> {
    static LIB: OnceLock<Option<ReaderLib>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: as for `lib`.
        unsafe { <ReaderLib as dlsym::Bindings>::open("libmediandk.so") }.ok()
    })
    .as_ref()
}

/// Start this process's binder threads, once (see the module's docs).
fn binder_threads() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        // SAFETY: a system library; the symbol takes and returns nothing.
        unsafe {
            if let Ok(lib) = dlsym::Library::open("libbinder_ndk.so")
                && let Ok(start) =
                    lib.get::<unsafe extern "C" fn()>(c"ABinderProcess_startThreadPool")
            {
                start.call(());
                // The pool's threads run the library's code from now on.
                lib.leak();
            }
        }
    });
}

/// The reader the decoder renders into; deleted when the decoder and every
/// frame still holding one of its images are gone.
struct Reader {
    lib: &'static ReaderLib,
    reader: P,
}

// An `AImageReader` may be used and its images deleted from any thread.
unsafe impl Send for Reader {}
unsafe impl Sync for Reader {}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe { self.lib.delete(self.reader) };
    }
}

pub(super) struct CodecDecoder {
    lib: &'static Lib,
    codec: P,
    reader: Option<Arc<Reader>>,
    sets: Vec<Vec<u8>>,
    len_size: usize,
    /// The parameter sets go in-band before the next packet (a seek's).
    need_sets: bool,
    layout: Layout,
    color: YuvColorSpace,
    /// The times of pictures rendered into the reader, not acquired yet.
    rendered: VecDeque<Duration>,
    /// The decoder gave its last picture.
    ended: bool,
    out: Vec<Decoded>,
}

// The codec is used by one thread at a time (the player's).
unsafe impl Send for CodecDecoder {}

fn codec_err(what: &str, status: impl std::fmt::Display) -> VideoError {
    VideoError::Decode(format!("MediaCodec: {what} failed ({status})"))
}

impl CodecDecoder {
    pub(super) fn new(params: &CodecParameters) -> Result<Self, VideoError> {
        let lib =
            lib().ok_or_else(|| VideoError::Unsupported("no MediaCodec (libmediandk)".into()))?;
        let hevc = params.codec_id.as_str() != "h264";
        let (sets, len_size) = if hevc {
            parse_hvcc(&params.extradata)
        } else {
            parse_avcc(&params.extradata)
        }
        .ok_or_else(|| {
            VideoError::Unsupported(
                "H.264/HEVC without its configuration record (an Annex B stream)".into(),
            )
        })?;
        let (w, h) = (params.width.unwrap_or(0), params.height.unwrap_or(0));
        if w == 0 || h == 0 {
            return Err(VideoError::Decode("a video track without its size".into()));
        }
        binder_threads();
        let mime: &CStr = if hevc { c"video/hevc" } else { c"video/avc" };
        let mut me = Self {
            lib,
            codec: std::ptr::null_mut(),
            reader: None,
            sets,
            len_size: len_size as usize,
            need_sets: false,
            layout: Layout::new(w as usize, h as usize),
            color: nv12::default_color(h),
            rendered: VecDeque::new(),
            ended: false,
            out: Vec::new(),
        };
        let surface = std::env::var_os("STIPPLE_VIDEO_NO_SURFACE").is_none();
        unsafe {
            if surface
                && let Some(reader) = me.new_reader(w as i32, h as i32)
                && me.open(mime, hevc, w, h, Some(&reader)).is_ok()
            {
                me.reader = Some(reader);
                return Ok(me);
            }
            me.open(mime, hevc, w, h, None)?;
        }
        Ok(me)
    }

    unsafe fn new_reader(&self, w: i32, h: i32) -> Option<Arc<Reader>> {
        let rl = reader_lib()?;
        let mut reader: P = std::ptr::null_mut();
        let st = unsafe {
            rl.new_with_usage(
                w,
                h,
                AIMAGE_FORMAT_YUV_420_888,
                USAGE_CPU_READ_OFTEN | USAGE_GPU_SAMPLED_IMAGE,
                MAX_IMAGES,
                &mut reader,
            )
        };
        (st == 0 && !reader.is_null()).then(|| Arc::new(Reader { lib: rl, reader }))
    }

    /// Make, configure and start the codec, rendering into `reader` if
    /// given.
    unsafe fn open(
        &mut self,
        mime: &CStr,
        hevc: bool,
        w: u32,
        h: u32,
        reader: Option<&Reader>,
    ) -> Result<(), VideoError> {
        let lib = self.lib;
        unsafe {
            let codec = lib.create_by_type(mime.as_ptr());
            if codec.is_null() {
                return Err(VideoError::Unsupported(format!(
                    "no {} decoder on this device",
                    mime.to_string_lossy()
                )));
            }
            let format = lib.format_new();
            lib.set_string(format, c"mime".as_ptr(), mime.as_ptr());
            lib.set_i32(format, c"width".as_ptr(), w as i32);
            lib.set_i32(format, c"height".as_ptr(), h as i32);
            // H.264: the SPS, then the PPS; HEVC: all of them in one.
            let csd: Vec<Vec<u8>> = if hevc {
                vec![annex_b(&[], 4, &self.sets)]
            } else {
                self.sets
                    .iter()
                    .map(|s| annex_b(&[], 4, std::slice::from_ref(s)))
                    .collect()
            };
            for (key, data) in [c"csd-0", c"csd-1"].iter().zip(&csd) {
                lib.set_buffer(format, key.as_ptr(), data.as_ptr().cast(), data.len());
            }
            let mut window: P = std::ptr::null_mut();
            if let Some(r) = reader
                && r.lib.window(r.reader, &mut window) != 0
            {
                window = std::ptr::null_mut();
            }
            if reader.is_some() && window.is_null() {
                lib.format_delete(format);
                lib.delete(codec);
                return Err(VideoError::Decode("the image reader has no window".into()));
            }
            let st = lib.configure(codec, format, window, std::ptr::null_mut(), 0);
            lib.format_delete(format);
            if st != 0 {
                lib.delete(codec);
                return Err(VideoError::Unsupported(format!(
                    "MediaCodec won't decode this stream (configure: {st})"
                )));
            }
            let st = lib.start(codec);
            if st != 0 {
                lib.delete(codec);
                return Err(codec_err("start", st));
            }
            self.codec = codec;
        }
        Ok(())
    }

    /// Take the pictures the decoder has ready (waiting up to `timeout_us`
    /// for the first).
    unsafe fn drain(&mut self, timeout_us: i64) -> Result<(), VideoError> {
        let lib = self.lib;
        let mut timeout = timeout_us;
        loop {
            let mut info = BufferInfo::default();
            let idx = unsafe { lib.dequeue_output(self.codec, &mut info, timeout) };
            timeout = 0;
            match idx {
                INFO_TRY_AGAIN_LATER => return Ok(()),
                INFO_OUTPUT_FORMAT_CHANGED => {
                    unsafe {
                        let format = lib.output_format(self.codec);
                        if !format.is_null() {
                            self.layout.read(|k| {
                                let mut v = 0;
                                lib.get_i32(format, k.as_ptr(), &mut v).then_some(v)
                            });
                            lib.format_delete(format);
                        }
                    }
                    continue;
                }
                INFO_OUTPUT_BUFFERS_CHANGED => continue,
                i if i < 0 => return Err(codec_err("dequeueOutputBuffer", i)),
                _ => {}
            }
            let idx = idx as usize;
            let time = Duration::from_micros(info.pts_us.max(0) as u64);
            let picture = info.flags & BUFFER_FLAG_CODEC_CONFIG == 0
                && !(info.flags & BUFFER_FLAG_END_OF_STREAM != 0 && info.size <= 0);
            unsafe {
                if picture && self.reader.is_some() {
                    lib.release_output(self.codec, idx, true);
                    self.rendered.push_back(time);
                    // Taken before the next is rendered: the decoder's
                    // surface drops a queued picture for a newer one.
                    self.collect(Duration::from_millis(500), true);
                } else {
                    if picture {
                        self.read_buffer(idx, &info, time);
                    }
                    lib.release_output(self.codec, idx, false);
                }
            }
            if info.flags & BUFFER_FLAG_END_OF_STREAM != 0 {
                self.ended = true;
                return Ok(());
            }
        }
    }

    /// A picture in the decoder's own buffer, converted.
    unsafe fn read_buffer(&mut self, idx: usize, info: &BufferInfo, time: Duration) {
        let mut cap = 0usize;
        let base = unsafe { self.lib.output_buffer(self.codec, idx, &mut cap) };
        let (off, len) = (info.offset.max(0) as usize, info.size.max(0) as usize);
        if base.is_null() || off + len > cap {
            return;
        }
        let data = unsafe { std::slice::from_raw_parts(base.add(off), len) };
        let l = self.layout;
        let size = PhysicalSize::new((l.width & !1) as u32, (l.height & !1) as u32);
        if let Some((y, ys, uv, uvs)) = l.nv12(data)
            && let Some(image) = nv12::image(size, (y, ys), (&uv, uvs), self.color)
        {
            self.out.push(Decoded { time, image });
        }
    }

    /// Take the pictures that reached the reader, waiting up to `wait` for
    /// those still on their way.
    unsafe fn collect(&mut self, wait: Duration, keep: bool) {
        let Some(reader) = self.reader.clone() else {
            return;
        };
        let rl = reader.lib;
        let start = Instant::now();
        while !self.rendered.is_empty() {
            let mut image: P = std::ptr::null_mut();
            let st = unsafe { rl.acquire_next(reader.reader, &mut image) };
            if st != 0 || image.is_null() {
                if start.elapsed() >= wait {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            // Its time: the one the decoder gave it (the buffer's
            // timestamp, in nanoseconds), else the next rendered.
            let mut ns = 0i64;
            unsafe { rl.timestamp(image, &mut ns) };
            let at = self
                .rendered
                .iter()
                .position(|t| t.as_micros() as i64 == ns / 1000)
                .unwrap_or(0);
            let time = self.rendered.drain(..=at).next_back().unwrap_or_default();
            if !keep {
                unsafe { rl.image_delete(image) };
                continue;
            }
            if let Some(frame) = unsafe { self.image_frame(&reader, image) } {
                self.out.push(Decoded { time, image: frame });
            }
        }
    }

    /// A reader's image as a frame: its hardware buffer while the renderer
    /// imports those (and the whole buffer is the picture), else its planes
    /// converted. The image is deleted, or kept with the frame.
    unsafe fn image_frame(&self, reader: &Arc<Reader>, image: P) -> Option<Arc<ExternalImage>> {
        let rl = reader.lib;
        unsafe {
            let (mut w, mut h) = (0, 0);
            rl.width(image, &mut w);
            rl.height(image, &mut h);
            let mut crop = CropRect::default();
            if rl.crop(image, &mut crop) != 0 || crop.right <= crop.left || crop.bottom <= crop.top
            {
                crop = CropRect {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                };
            }
            let whole = crop
                == CropRect {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                };
            if whole && crate::platform::external::imported_natively(ExternalFormat::Yuv420) {
                let mut buffer: P = std::ptr::null_mut();
                if rl.hardware_buffer(image, &mut buffer) == 0
                    && !buffer.is_null()
                    && let Ok(external) = ExternalImage::hardware_buffer(buffer)
                {
                    let (keep, held) = (reader.clone(), image as usize);
                    return Some(Arc::new(external.color_space(self.color).on_release(
                        move || {
                            keep.lib.image_delete(held as P);
                            drop(keep);
                        },
                    )));
                }
            }
            let frame = self.convert(rl, image, &crop);
            rl.image_delete(image);
            frame
        }
    }

    /// An image's planes, the `crop` of them, converted.
    unsafe fn convert(
        &self,
        rl: &ReaderLib,
        image: P,
        crop: &CropRect,
    ) -> Option<Arc<ExternalImage>> {
        unsafe {
            let plane = |i: c_int| -> Option<(&[u8], usize, usize)> {
                let (mut data, mut len, mut rs, mut ps) = (std::ptr::null_mut(), 0, 0, 0);
                if rl.plane_data(image, i, &mut data, &mut len) != 0
                    || data.is_null()
                    || rl.row_stride(image, i, &mut rs) != 0
                    || rl.pixel_stride(image, i, &mut ps) != 0
                {
                    return None;
                }
                Some((
                    std::slice::from_raw_parts(data, len.max(0) as usize),
                    rs.max(0) as usize,
                    ps.max(1) as usize,
                ))
            };
            let ((y, ys, _), (u, us, up), (v, vs, vp)) = (plane(0)?, plane(1)?, plane(2)?);
            let (l, t) = (crop.left.max(0) as usize, crop.top.max(0) as usize);
            let w = (crop.right - crop.left).max(0) as usize & !1;
            let h = (crop.bottom - crop.top).max(0) as usize & !1;
            let u = Chroma {
                data: u.get((t / 2) * us + (l / 2) * up..)?,
                row_stride: us,
                pixel_stride: up,
            };
            let v = Chroma {
                data: v.get((t / 2) * vs + (l / 2) * vp..)?,
                row_stride: vs,
                pixel_stride: vp,
            };
            let uv = nv12::interleave(u, v, w, h)?;
            nv12::image(
                PhysicalSize::new(w as u32, h as u32),
                (y.get(t * ys + l..)?, ys),
                (&uv, w.div_ceil(2) * 2),
                self.color,
            )
        }
    }

    /// Queue `data` (or the end of the stream) into the decoder's next free
    /// input buffer, taking its pictures meanwhile.
    unsafe fn queue(&mut self, data: &[u8], pts: Duration, flags: u32) -> Result<(), VideoError> {
        let lib = self.lib;
        let start = Instant::now();
        loop {
            let idx = unsafe { lib.dequeue_input(self.codec, 10_000) };
            if idx >= 0 {
                unsafe {
                    let mut cap = 0usize;
                    let buf = lib.input_buffer(self.codec, idx as usize, &mut cap);
                    let len = if buf.is_null() || cap < data.len() {
                        0 // too big for its buffers: skipped
                    } else {
                        std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
                        data.len()
                    };
                    let us = pts.as_micros().min(i64::MAX as u128) as u64;
                    let st = lib.queue_input(self.codec, idx as usize, 0, len, us, flags);
                    if st != 0 {
                        return Err(codec_err("queueInputBuffer", st));
                    }
                }
                return Ok(());
            }
            if idx != INFO_TRY_AGAIN_LATER {
                return Err(codec_err("dequeueInputBuffer", idx));
            }
            // Full: its pictures make room.
            unsafe {
                self.drain(0)?;
                self.collect(Duration::ZERO, true);
            }
            if start.elapsed() > Duration::from_secs(5) {
                return Err(VideoError::Decode("MediaCodec stopped taking input".into()));
            }
        }
    }
}

impl FrameDecoder for CodecDecoder {
    fn send(&mut self, packet: Coded<'_>) -> Result<(), VideoError> {
        let sets: &[Vec<u8>] = if self.need_sets { &self.sets } else { &[] };
        let data = annex_b(packet.data, self.len_size, sets);
        if data.is_empty() {
            return Ok(());
        }
        self.need_sets = false;
        unsafe {
            self.queue(&data, packet.pts.unwrap_or_default(), 0)?;
            self.drain(0)?;
            self.collect(Duration::ZERO, true);
        }
        Ok(())
    }

    fn receive(&mut self) -> Vec<Decoded> {
        unsafe { self.collect(Duration::ZERO, true) };
        std::mem::take(&mut self.out)
    }

    fn flush(&mut self) -> Result<(), VideoError> {
        unsafe {
            self.queue(&[], Duration::ZERO, BUFFER_FLAG_END_OF_STREAM)?;
            let start = Instant::now();
            while !self.ended && start.elapsed() < Duration::from_secs(5) {
                self.drain(10_000)?;
            }
            // The last pictures, on their way to the reader.
            self.collect(Duration::from_secs(1), true);
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        unsafe {
            let st = self.lib.flush(self.codec);
            if st != 0 {
                return Err(codec_err("flush", st));
            }
            // Pictures rendered before the flush that are still coming.
            self.collect(Duration::from_millis(50), false);
        }
        self.rendered.clear();
        self.out.clear();
        self.ended = false;
        self.need_sets = true;
        Ok(())
    }

    fn reorder_depth(&self) -> usize {
        // Its pictures come in presentation order already.
        1
    }

    fn name(&self) -> &'static str {
        "MediaCodec"
    }
}

impl Drop for CodecDecoder {
    fn drop(&mut self) {
        if !self.codec.is_null() {
            unsafe {
                self.lib.stop(self.codec);
                self.lib.delete(self.codec);
            }
        }
        // The reader goes with the last frame holding one of its images.
    }
}
