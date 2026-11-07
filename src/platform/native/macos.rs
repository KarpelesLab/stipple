//! AppKit: real, offscreen `NSControl`s captured with
//! `cacheDisplayInRect:toBitmapImageRep:`, so controls take AppKit's current look
//! on whichever macOS version runs the app.

use super::objc::{CgRect, CgSize, Id, class, msg, nsstring, with_pool};
use super::{NativeControl, NativeDeco, SystemLook, unpremultiply};
use crate::geometry::PhysicalSize;
use crate::render::{Color, Pixmap};
use std::ffi::c_void;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

pub const AVAILABLE: bool = true;
pub const PRIMARY_LABEL_ON_ACCENT: bool = true;

const BEZEL_ROUNDED: u64 = 1;
const BUTTON_SWITCH: u64 = 3;
const BUTTON_RADIO: u64 = 4;

pub fn system_look() -> SystemLook {
    with_pool(|| unsafe {
        let defaults: Id = msg!(class("NSUserDefaults"), "standardUserDefaults", () -> Id);
        let key = nsstring("AppleInterfaceStyle");
        let style: Id = msg!(defaults, "stringForKey:", (key: Id) -> Id);
        let dark = !style.is_null() && {
            let target = nsstring("Dark");
            msg!(style, "isEqualToString:", (target: Id) -> bool)
        };
        let accent: Id = msg!(class("NSColor"), "controlAccentColor", () -> Id);
        let srgb: Id = msg!(class("NSColorSpace"), "sRGBColorSpace", () -> Id);
        let rgb: Id = msg!(accent, "colorUsingColorSpace:", (srgb: Id) -> Id);
        let accent = (!rgb.is_null()).then(|| {
            let c = |v: f64| (v * 255.0).round() as u8;
            let r: f64 = msg!(rgb, "redComponent", () -> f64);
            let g: f64 = msg!(rgb, "greenComponent", () -> f64);
            let b: f64 = msg!(rgb, "blueComponent", () -> f64);
            Color::rgb(c(r), c(g), c(b))
        });
        SystemLook { dark, accent }
    })
}

pub fn supports(_control: NativeControl) -> bool {
    true
}

pub fn size_hint(control: NativeControl) -> (Option<f64>, Option<f64>) {
    match control {
        NativeControl::Button { .. } => (None, Some(28.0)),
        NativeControl::Checkbox { .. } | NativeControl::Radio { .. } => (Some(18.0), Some(18.0)),
        NativeControl::Switch { .. } => switch_size(),
        NativeControl::Slider { .. } => (Some(160.0), Some(24.0)),
        NativeControl::Progress { .. } => (Some(200.0), Some(12.0)),
        NativeControl::TextField => (None, Some(24.0)),
    }
}

/// The switch's size varies between macOS versions, so ask AppKit for it.
fn switch_size() -> (Option<f64>, Option<f64>) {
    static SIZE: std::sync::OnceLock<CgSize> = std::sync::OnceLock::new();
    let size = SIZE.get_or_init(|| {
        with_pool(|| unsafe {
            let alloc: Id = msg!(class("NSSwitch"), "alloc", () -> Id);
            let view: Id = msg!(alloc, "init", () -> Id);
            let size: CgSize = msg!(view, "intrinsicContentSize", () -> CgSize);
            msg!(view, "release", () -> ());
            size
        })
    });
    (Some(size.width), Some(size.height))
}

pub fn draws_focus(_control: NativeControl) -> bool {
    false // AppKit only draws focus rings for a window's first responder
}

/// Create the view for `deco` with its state applied. Caller releases it.
unsafe fn make_view(deco: NativeDeco, frame: CgRect) -> Id {
    unsafe {
        let init = |name: &str| -> Id {
            let alloc: Id = msg!(class(name), "alloc", () -> Id);
            msg!(alloc, "initWithFrame:", (frame: CgRect) -> Id)
        };
        let button = |kind: u64, on: bool| -> Id {
            let b = init("NSButton");
            let empty = nsstring("");
            msg!(b, "setTitle:", (empty: Id) -> ());
            msg!(b, "setButtonType:", (kind: u64) -> ());
            let state = on as i64;
            msg!(b, "setState:", (state: i64) -> ());
            b
        };
        let view = match deco.control {
            NativeControl::Button { primary } => {
                let b = init("NSButton");
                let empty = nsstring("");
                msg!(b, "setTitle:", (empty: Id) -> ());
                msg!(b, "setBezelStyle:", (BEZEL_ROUNDED: u64) -> ());
                if primary {
                    // A return key equivalent is what makes a button the accent one.
                    let ret = nsstring("\r");
                    msg!(b, "setKeyEquivalent:", (ret: Id) -> ());
                }
                b
            }
            NativeControl::Checkbox { checked } => button(BUTTON_SWITCH, checked),
            NativeControl::Radio { selected } => button(BUTTON_RADIO, selected),
            NativeControl::Switch { on } => {
                let s = init("NSSwitch");
                let state = on as i64;
                msg!(s, "setState:", (state: i64) -> ());
                s
            }
            NativeControl::Slider { value } => {
                let s = init("NSSlider");
                let (min, max, v) = (0.0_f64, 1000.0_f64, value as f64);
                msg!(s, "setMinValue:", (min: f64) -> ());
                msg!(s, "setMaxValue:", (max: f64) -> ());
                msg!(s, "setDoubleValue:", (v: f64) -> ());
                s
            }
            NativeControl::Progress { value } => {
                let p = init("NSProgressIndicator");
                let no = false;
                msg!(p, "setIndeterminate:", (no: bool) -> ());
                let (min, max, v) = (0.0_f64, 1000.0_f64, value as f64);
                msg!(p, "setMinValue:", (min: f64) -> ());
                msg!(p, "setMaxValue:", (max: f64) -> ());
                msg!(p, "setDoubleValue:", (v: f64) -> ());
                p
            }
            NativeControl::TextField => {
                let t = init("NSTextField");
                let yes = true;
                msg!(t, "setBezeled:", (yes: bool) -> ());
                msg!(t, "setBezelStyle:", (BEZEL_ROUNDED: u64) -> ());
                t
            }
        };
        if deco.state.pressed {
            let yes = true;
            msg!(view, "setHighlighted:", (yes: bool) -> ());
        }
        let name = nsstring(if deco.dark {
            "NSAppearanceNameDarkAqua"
        } else {
            "NSAppearanceNameAqua"
        });
        let appearance: Id = msg!(class("NSAppearance"), "appearanceNamed:", (name: Id) -> Id);
        msg!(view, "setAppearance:", (appearance: Id) -> ());
        view
    }
}

pub fn render(deco: NativeDeco, size: PhysicalSize, scale: f64) -> Option<Pixmap> {
    with_pool(|| unsafe {
        let (w, h) = (size.width as i64, size.height as i64);
        let logical = CgSize {
            width: size.width as f64 / scale,
            height: size.height as f64 / scale,
        };
        let frame = CgRect {
            x: 0.0,
            y: 0.0,
            width: logical.width,
            height: logical.height,
        };
        let view = make_view(deco, frame);
        if view.is_null() {
            return None;
        }
        // A bitmap at our exact pixel size; setSize: maps the view's points onto it.
        let alloc: Id = msg!(class("NSBitmapImageRep"), "alloc", () -> Id);
        let planes: *mut *mut u8 = std::ptr::null_mut();
        let (bits, samples, row, pixel) = (8_i64, 4_i64, w * 4, 32_i64);
        let (alpha, planar) = (true, false);
        let space = nsstring("NSDeviceRGBColorSpace");
        let rep: Id = msg!(
            alloc,
            "initWithBitmapDataPlanes:pixelsWide:pixelsHigh:bitsPerSample:samplesPerPixel:hasAlpha:isPlanar:colorSpaceName:bytesPerRow:bitsPerPixel:",
            (planes: *mut *mut u8, w: i64, h: i64, bits: i64, samples: i64, alpha: bool, planar: bool,
             space: Id, row: i64, pixel: i64) -> Id
        );
        if rep.is_null() {
            msg!(view, "release", () -> ());
            return None;
        }
        msg!(rep, "setSize:", (logical: CgSize) -> ());
        msg!(view, "cacheDisplayInRect:toBitmapImageRep:", (frame: CgRect, rep: Id) -> ());

        let data: *const c_void = msg!(rep, "bitmapData", () -> *const c_void);
        let out = (!data.is_null()).then(|| {
            let len = (w * h * 4) as usize;
            let mut px = std::slice::from_raw_parts(data as *const u8, len).to_vec();
            unpremultiply(&mut px);
            Pixmap::from_rgba8(size, px)
        });
        msg!(rep, "release", () -> ());
        msg!(view, "release", () -> ());
        out
    })
}
