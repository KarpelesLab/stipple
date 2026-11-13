//! Putting a window's frames on screen.
//!
//! A present keeps the frame in the window's [`Framebuffer`] and marks only
//! what changed for redisplay (`setNeedsDisplayInRect:`), so AppKit's
//! `drawRect:` repaints just that. The buffer is kept in Core Graphics' native
//! layout (32-bit little-endian, alpha skipped first: `B, G, R, X`) and tagged
//! sRGB, and the window's backing store is sRGB too, so drawing it is a plain
//! copy instead of a per-pixel format and color conversion; the window server
//! matches the colors to the display when it composites.
//!
//! `drawRect:` timings go to the frame clock's
//! [`stats`](crate::platform::frame::stats).

use std::ffi::c_void;

use super::{CgPoint, CgRect, CgSize, Id, class, msg_id, msg_void_bool, msg_void_id, sel};
use crate::geometry::Rect;
use crate::render::Pixmap;
use crate::render::surface::{copy_as_bgrx, damage_rects};

/// `kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little`.
const CG_BITMAP_BGRX: u32 = 6 | (2 << 12);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: *const c_void;
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: *const c_void,
    ) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
    fn CGContextDrawImage(ctx: *mut c_void, rect: CgRect, image: *mut c_void);
    fn CGContextSaveGState(ctx: *mut c_void);
    fn CGContextRestoreGState(ctx: *mut c_void);
    fn CGContextTranslateCTM(ctx: *mut c_void, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut c_void, sx: f64, sy: f64);
}

/// A window's last frame: `B, G, R, X` bytes, top row first, in physical
/// pixels.
#[derive(Default)]
pub(super) struct Framebuffer {
    px: Vec<u8>,
    w: usize,
    h: usize,
}

impl Framebuffer {
    /// Take a presented frame, of which `damage` (physical pixels; empty:
    /// all) changed. Returns the pixel rects to redisplay, and whether they
    /// are the whole frame.
    pub(super) fn update(&mut self, pixmap: &Pixmap, damage: &[Rect]) -> (Vec<Rect>, bool) {
        let size = pixmap.size();
        let (w, h) = (size.width as usize, size.height as usize);
        let whole = damage.is_empty() || (w, h) != (self.w, self.h);
        let rects = damage_rects(size, if whole { &[] } else { damage });
        copy_as_bgrx(pixmap, &mut self.px, &rects);
        (self.w, self.h) = (w, h);
        let rects = rects
            .into_iter()
            .map(|(x, y, w, h)| Rect::from_xywh(x as f64, y as f64, w as f64, h as f64))
            .collect();
        (rects, whole)
    }

    /// Draw the frame into the view's current graphics context, whose
    /// coordinates are points (`scale` pixels each), top-left origin. AppKit
    /// has clipped it to what needs redisplay.
    ///
    /// # Safety
    /// Main thread, inside the view's `drawRect:`.
    pub(super) unsafe fn draw(&self, scale: f64) {
        if self.px.is_empty() {
            return;
        }
        let began = std::time::Instant::now();
        unsafe {
            // [[NSGraphicsContext currentContext] CGContext]
            let current = msg_id(class("NSGraphicsContext"), sel("currentContext"));
            if current.is_null() {
                return;
            }
            let cg = msg_id(current, sel("CGContext"));
            if cg.is_null() {
                return;
            }
            let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                self.px.as_ptr() as *const c_void,
                self.px.len(),
                std::ptr::null(),
            );
            let image = CGImageCreate(
                self.w,
                self.h,
                8,
                32,
                self.w * 4,
                space,
                CG_BITMAP_BGRX,
                provider,
                std::ptr::null(),
                false,
                0,
            );
            let (width, height) = (self.w as f64 / scale, self.h as f64 / scale);
            let rect = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize { width, height },
            };
            // The view is flipped (top-left origin), but CGContextDrawImage
            // puts an image's first row at the bottom: flip around the draw.
            CGContextSaveGState(cg);
            CGContextTranslateCTM(cg, 0.0, height);
            CGContextScaleCTM(cg, 1.0, -1.0);
            CGContextDrawImage(cg, rect, image);
            CGContextRestoreGState(cg);
            CGImageRelease(image);
            CGDataProviderRelease(provider);
            CGColorSpaceRelease(space);
        }
        crate::platform::frame::note_display(began.elapsed());
    }
}

/// Mark `rects` (physical pixels) of `view` for redisplay, at `scale` pixels
/// per point; `whole`: all of it.
///
/// # Safety
/// Main thread.
pub(super) unsafe fn invalidate(view: Id, rects: &[Rect], whole: bool, scale: f64) {
    if whole {
        unsafe { msg_void_bool(view, sel("setNeedsDisplay:"), true) };
        return;
    }
    let set: unsafe extern "C" fn(Id, *mut c_void, CgRect) =
        unsafe { std::mem::transmute(super::objc_msgSend as *const c_void) };
    for r in rects {
        // Whole points around the pixels.
        let (x0, y0) = ((r.min_x() / scale).floor(), (r.min_y() / scale).floor());
        let (x1, y1) = ((r.max_x() / scale).ceil(), (r.max_y() / scale).ceil());
        let rect = CgRect {
            origin: CgPoint { x: x0, y: y0 },
            size: CgSize {
                width: x1 - x0,
                height: y1 - y0,
            },
        };
        unsafe { set(view, sel("setNeedsDisplayInRect:"), rect) };
    }
}

/// Give `window` an sRGB backing store, the color space frames are drawn in.
///
/// # Safety
/// Main thread.
pub(super) unsafe fn use_srgb(window: Id) {
    unsafe {
        let srgb = msg_id(class("NSColorSpace"), sel("sRGBColorSpace"));
        if !srgb.is_null() {
            msg_void_id(window, sel("setColorSpace:"), srgb);
        }
    }
}
