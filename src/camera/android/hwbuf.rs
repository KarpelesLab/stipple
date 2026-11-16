//! The camera's frames handed to the preview as their own buffers: the
//! reader is made with `AImageReader_newWithUsage` (Android 8) for the GPU
//! to sample its images as well as the CPU to read them, and while the
//! renderer imports `ExternalFormat::Yuv420` (GLES, through an external
//! texture), each image's `AHardwareBuffer` (`AImage_getHardwareBuffer`)
//! becomes the preview's [`ExternalImage`]: the `AImage` is kept out of the
//! reader until the image is dropped, and nothing is converted on the CPU.

use super::{Lib, P};
use crate::render::ExternalImage;
use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::{c_char, c_int, c_void};
use std::sync::{Mutex, OnceLock};

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// `AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN`.
const USAGE_CPU_READ_OFTEN: u64 = 3;
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE`.
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// Images the reader holds: the one being delivered, the preview's latest,
/// the last frame's (held by the renderer until the next) and room for the
/// reader to acquire the newest past them.
pub(super) const MAX_IMAGES: i32 = 6;

/// The functions for it (Android 8), where the system has them.
struct Fns {
    new_with_usage: unsafe extern "C" fn(i32, i32, i32, u64, i32, *mut P) -> i32,
    hardware_buffer: unsafe extern "C" fn(P, *mut P) -> i32,
}

#[allow(clippy::missing_transmute_annotations)] // each symbol's type is its field's
fn fns() -> Option<&'static Fns> {
    static FNS: OnceLock<Option<Fns>> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let media = dlopen(c"libmediandk.so".as_ptr(), 2);
        if media.is_null() {
            return None;
        }
        let new = dlsym(media, c"AImageReader_newWithUsage".as_ptr());
        let get = dlsym(media, c"AImage_getHardwareBuffer".as_ptr());
        (!new.is_null() && !get.is_null()).then(|| Fns {
            new_with_usage: std::mem::transmute(new),
            hardware_buffer: std::mem::transmute(get),
        })
    })
    .as_ref()
}

/// The readers alive, by address: an image released after its reader was
/// deleted (which took its images back) mustn't be deleted again.
static LIVE: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// A reader of `w`×`h` `format` images whose buffers the GPU can sample
/// too, when the system can make one; else a plain one.
pub(super) unsafe fn new_reader(lib: &Lib, w: i32, h: i32, format: i32, out: *mut P) -> i32 {
    unsafe {
        let r = match fns() {
            Some(f) => (f.new_with_usage)(
                w,
                h,
                format,
                USAGE_CPU_READ_OFTEN | USAGE_GPU_SAMPLED_IMAGE,
                MAX_IMAGES,
                out,
            ),
            None => (lib.reader_new)(w, h, format, MAX_IMAGES, out),
        };
        if r == 0 && !(*out).is_null() {
            LIVE.lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashSet::new)
                .insert(*out as usize);
        }
        r
    }
}

/// Delete `reader` (and with it every image it lent out).
pub(super) unsafe fn delete_reader(lib: &Lib, reader: P) {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = live.as_mut() {
        set.remove(&(reader as usize));
    }
    // Under the lock: no image is being deleted meanwhile.
    unsafe { (lib.reader_delete)(reader) };
}

/// `image` (of `reader`) as an [`ExternalImage`] of its hardware buffer,
/// the `AImage` kept (`kept` set) until the image is dropped; `None` if it
/// has no buffer the renderer can take.
pub(super) unsafe fn external(
    lib: &'static Lib,
    reader: P,
    image: P,
    kept: &Cell<bool>,
) -> Option<ExternalImage> {
    let f = fns()?;
    unsafe {
        let mut buffer: P = std::ptr::null_mut();
        if (f.hardware_buffer)(image, &mut buffer) != 0 || buffer.is_null() {
            return None;
        }
        let external = ExternalImage::hardware_buffer(buffer).ok()?;
        kept.set(true);
        let (reader, image) = (reader as usize, image as usize);
        Some(external.on_release(move || {
            let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
            if live.as_ref().is_some_and(|s| s.contains(&reader)) {
                (lib.image_delete)(image as P);
            }
        }))
    }
}
