//! UIKit: real, offscreen `UIControl`s rendered through their layers.

use super::objc::{CgRect, CgSize, Id, class, msg, with_pool};
use super::{NativeControl, NativeDeco, SystemLook, unpremultiply};
use crate::geometry::PhysicalSize;
use crate::render::{Color, Pixmap};
use std::ffi::c_void;

const PREMULTIPLIED_LAST: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        row: usize,
        space: *mut c_void,
        info: u32,
    ) -> *mut c_void;
    fn CGContextRelease(ctx: *mut c_void);
    fn CGContextTranslateCTM(ctx: *mut c_void, x: f64, y: f64);
    fn CGContextScaleCTM(ctx: *mut c_void, x: f64, y: f64);
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIGraphicsBeginImageContextWithOptions(size: CgSize, opaque: bool, scale: f64);
    fn UIGraphicsGetCurrentContext() -> *mut c_void;
    fn UIGraphicsEndImageContext();
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBitmapContextGetData(ctx: *mut c_void) -> *mut c_void;
    fn CGBitmapContextGetBytesPerRow(ctx: *mut c_void) -> usize;
}

pub const AVAILABLE: bool = true;
pub const PRIMARY_LABEL_ON_ACCENT: bool = true;

pub fn system_look() -> SystemLook {
    with_pool(|| unsafe {
        let screen: Id = msg!(class("UIScreen"), "mainScreen", () -> Id);
        let traits: Id = msg!(screen, "traitCollection", () -> Id);
        let style: i64 = msg!(traits, "userInterfaceStyle", () -> i64);
        SystemLook {
            dark: style == 2,
            accent: Some(Color::rgb(0, 122, 255)), // iOS's systemBlue tint
        }
    })
}

pub fn supports(control: NativeControl) -> bool {
    // UIKit has no checkbox or radio button.
    !matches!(
        control,
        NativeControl::Checkbox { .. } | NativeControl::Radio { .. }
    )
}

pub fn size_hint(control: NativeControl) -> (Option<f64>, Option<f64>) {
    match control {
        NativeControl::Button { .. } => (None, Some(44.0)),
        NativeControl::Switch { .. } => switch_size(),
        NativeControl::Slider { .. } => (Some(200.0), Some(31.0)),
        NativeControl::Progress { .. } => (Some(200.0), Some(4.0)),
        NativeControl::TextField => (None, Some(34.0)),
        NativeControl::Checkbox { .. } | NativeControl::Radio { .. } => (None, None),
    }
}

/// The switch's size varies between iOS versions, so ask UIKit for it.
fn switch_size() -> (Option<f64>, Option<f64>) {
    static SIZE: std::sync::OnceLock<CgSize> = std::sync::OnceLock::new();
    let size = SIZE.get_or_init(|| {
        with_pool(|| unsafe {
            let alloc: Id = msg!(class("UISwitch"), "alloc", () -> Id);
            let view: Id = msg!(alloc, "init", () -> Id);
            let size: CgSize = msg!(view, "intrinsicContentSize", () -> CgSize);
            msg!(view, "release", () -> ());
            size
        })
    });
    (Some(size.width), Some(size.height))
}

pub fn draws_focus(_control: NativeControl) -> bool {
    false
}

unsafe fn make_view(deco: NativeDeco, frame: CgRect) -> Id {
    unsafe {
        let init = |name: &str| -> Id {
            let alloc: Id = msg!(class(name), "alloc", () -> Id);
            msg!(alloc, "initWithFrame:", (frame: CgRect) -> Id)
        };
        let view = match deco.control {
            NativeControl::Button { primary } => {
                let kind = 1_i64; // UIButtonTypeSystem
                let b: Id = msg!(class("UIButton"), "buttonWithType:", (kind: i64) -> Id);
                let b: Id = msg!(b, "retain", () -> Id);
                let config: Id = msg!(
                    class("UIButtonConfiguration"),
                    if primary { "filledButtonConfiguration" } else { "grayButtonConfiguration" },
                    () -> Id
                );
                msg!(b, "setConfiguration:", (config: Id) -> ());
                msg!(b, "setFrame:", (frame: CgRect) -> ());
                b
            }
            NativeControl::Switch { on } => {
                let s = init("UISwitch");
                let animated = false;
                msg!(s, "setOn:animated:", (on: bool, animated: bool) -> ());
                s
            }
            NativeControl::Slider { value } => {
                let s = init("UISlider");
                let v = value as f32 / 1000.0;
                msg!(s, "setValue:", (v: f32) -> ());
                s
            }
            NativeControl::Progress { value } => {
                let style = 0_i64; // UIProgressViewStyleDefault
                let alloc: Id = msg!(class("UIProgressView"), "alloc", () -> Id);
                let p: Id = msg!(alloc, "initWithProgressViewStyle:", (style: i64) -> Id);
                msg!(p, "setFrame:", (frame: CgRect) -> ());
                let v = value as f32 / 1000.0;
                msg!(p, "setProgress:", (v: f32) -> ());
                p
            }
            NativeControl::TextField => {
                let t = init("UITextField");
                let rounded = 3_i64; // UITextBorderStyleRoundedRect
                msg!(t, "setBorderStyle:", (rounded: i64) -> ());
                t
            }
            NativeControl::Checkbox { .. } | NativeControl::Radio { .. } => {
                return std::ptr::null_mut();
            }
        };
        if deco.state.pressed && matches!(deco.control, NativeControl::Button { .. }) {
            let yes = true;
            msg!(view, "setHighlighted:", (yes: bool) -> ());
        }
        // UIUserInterfaceStyleDark = 2, Light = 1.
        let style = if deco.dark { 2_i64 } else { 1_i64 };
        msg!(view, "setOverrideUserInterfaceStyle:", (style: i64) -> ());
        msg!(view, "layoutIfNeeded", () -> ());
        view
    }
}

/// Snapshot `view` with `drawViewHierarchyInRect:afterScreenUpdates:`, returning
/// straight RGBA, or `None` when there's no window yet or the snapshot fails.
unsafe fn snapshot_in_window(
    view: Id,
    frame: CgRect,
    w: usize,
    h: usize,
    scale: f64,
) -> Option<Vec<u8>> {
    unsafe {
        let app: Id = msg!(class("UIApplication"), "sharedApplication", () -> Id);
        let window: Id = msg!(app, "keyWindow", () -> Id);
        if window.is_null() {
            return None;
        }
        let index = 0_i64;
        msg!(window, "insertSubview:atIndex:", (view: Id, index: i64) -> ());
        let logical = CgSize {
            width: frame.width,
            height: frame.height,
        };
        UIGraphicsBeginImageContextWithOptions(logical, false, scale);
        let updates = true;
        let ok: bool = msg!(
            view,
            "drawViewHierarchyInRect:afterScreenUpdates:",
            (frame: CgRect, updates: bool) -> bool
        );
        let ctx = UIGraphicsGetCurrentContext();
        let data = if ctx.is_null() {
            std::ptr::null_mut()
        } else {
            CGBitmapContextGetData(ctx)
        };
        // The context's row size can exceed w * 4 and its channels are BGRA.
        let row = if ctx.is_null() {
            0
        } else {
            CGBitmapContextGetBytesPerRow(ctx)
        };
        let out = (ok && !data.is_null() && row >= w * 4).then(|| {
            let src = std::slice::from_raw_parts(data as *const u8, row * h);
            let mut px = Vec::with_capacity(w * h * 4);
            for line in src.chunks_exact(row) {
                for p in line[..w * 4].as_chunks::<4>().0 {
                    px.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            unpremultiply(&mut px);
            px
        });
        UIGraphicsEndImageContext();
        msg!(view, "removeFromSuperview", () -> ());
        out
    }
}

pub fn render(deco: NativeDeco, size: PhysicalSize, scale: f64) -> Option<Pixmap> {
    with_pool(|| unsafe {
        let frame = CgRect {
            x: 0.0,
            y: 0.0,
            width: size.width as f64 / scale,
            height: size.height as f64 / scale,
        };
        let view = make_view(deco, frame);
        if view.is_null() {
            return None;
        }
        let (w, h) = (size.width as usize, size.height as usize);
        // Current UIKit controls have parts a layer render can't capture (a
        // switch's knob, a slider's thumb), so prefer a snapshot through the
        // window, tucked behind the app's content while it's taken.
        if let Some(px) = snapshot_in_window(view, frame, w, h, scale) {
            msg!(view, "release", () -> ());
            return Some(Pixmap::from_rgba8(size, px));
        }
        let mut px = vec![0u8; w * h * 4];
        let space = CGColorSpaceCreateDeviceRGB();
        let ctx = CGBitmapContextCreate(
            px.as_mut_ptr() as *mut c_void,
            w,
            h,
            8,
            w * 4,
            space,
            PREMULTIPLIED_LAST,
        );
        CGColorSpaceRelease(space);
        if ctx.is_null() {
            msg!(view, "release", () -> ());
            return None;
        }
        // CoreGraphics is y-up; UIKit layers draw y-down.
        CGContextTranslateCTM(ctx, 0.0, h as f64);
        CGContextScaleCTM(ctx, scale, -scale);
        let layer: Id = msg!(view, "layer", () -> Id);
        msg!(layer, "renderInContext:", (ctx: *mut c_void) -> ());
        CGContextRelease(ctx);
        msg!(view, "release", () -> ());
        unpremultiply(&mut px);
        Some(Pixmap::from_rgba8(size, px))
    })
}
