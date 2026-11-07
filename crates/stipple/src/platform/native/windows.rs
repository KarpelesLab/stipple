//! Windows: the visual styles engine (`uxtheme`) draws each control's parts into
//! a memory bitmap.
//!
//! `DrawThemeBackground` doesn't reliably report alpha, so each control is drawn
//! twice, over white and over black, and alpha is recovered from the difference.
//!
//! Dark mode uses the `DarkMode_*` theme classes the OS's own apps use. Microsoft
//! doesn't document them, so a class that's missing falls back to the light one.

use super::{NativeControl, NativeDeco, NativeState, SystemLook};
use crate::geometry::PhysicalSize;
use crate::render::{Color, Pixmap};
use std::ffi::c_void;

type Handle = *mut c_void;

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

#[link(name = "uxtheme")]
unsafe extern "system" {
    fn OpenThemeData(hwnd: Handle, classes: *const u16) -> Handle;
    fn CloseThemeData(theme: Handle) -> i32;
    fn DrawThemeBackground(
        theme: Handle,
        hdc: Handle,
        part: i32,
        state: i32,
        rect: *const Rect,
        clip: *const Rect,
    ) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn DeleteDC(hdc: Handle) -> i32;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn GdiFlush() -> i32;
    fn CreateDIBSection(
        hdc: Handle,
        info: *const BitmapInfoHeader,
        usage: u32,
        bits: *mut *mut c_void,
        section: Handle,
        offset: u32,
    ) -> Handle;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetColorizationColor(color: *mut u32, opaque: *mut i32) -> i32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

pub const AVAILABLE: bool = true;
/// Windows' default button keeps its regular text color.
pub const PRIMARY_LABEL_ON_ACCENT: bool = false;

const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
const RRF_RT_REG_DWORD: u32 = 0x10;

// vsstyle.h parts and states.
const BP_PUSHBUTTON: i32 = 1;
const BP_RADIOBUTTON: i32 = 2;
const BP_CHECKBOX: i32 = 3;
const PP_BAR: i32 = 1;
const PP_FILL: i32 = 5;
const TKP_TRACK: i32 = 1;
const TKP_THUMB: i32 = 3;
const EP_EDITBORDER_NOSCROLL: i32 = 6;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn system_look() -> SystemLook {
    let mut light: u32 = 1;
    let mut size = 4_u32;
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("AppsUseLightTheme");
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut light as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    let (mut argb, mut opaque) = (0_u32, 0_i32);
    let accent = (unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) } == 0)
        .then(|| Color::rgb((argb >> 16) as u8, (argb >> 8) as u8, argb as u8));
    SystemLook {
        dark: status == 0 && light == 0,
        accent,
    }
}

pub fn supports(control: NativeControl) -> bool {
    // Win32 has no toggle switch control.
    !matches!(control, NativeControl::Switch { .. })
}

pub fn size_hint(control: NativeControl) -> (Option<f64>, Option<f64>) {
    match control {
        NativeControl::Button { .. } => (None, Some(26.0)),
        NativeControl::Checkbox { .. } | NativeControl::Radio { .. } => (Some(13.0), Some(13.0)),
        NativeControl::Slider { .. } => (Some(160.0), Some(22.0)),
        NativeControl::Progress { .. } => (Some(200.0), Some(15.0)),
        NativeControl::TextField => (None, Some(24.0)),
        NativeControl::Switch { .. } => (None, None),
    }
}

pub fn draws_focus(control: NativeControl) -> bool {
    matches!(control, NativeControl::TextField)
}

/// 1 = normal, 2 = hot, 3 = pressed: the order most vsstyle state lists share.
fn interaction(state: NativeState) -> i32 {
    if state.pressed {
        3
    } else if state.hovered {
        2
    } else {
        1
    }
}

/// A set of theme parts drawn in order onto one control.
struct Part {
    class: &'static str,
    dark_class: Option<&'static str>,
    part: i32,
    state: i32,
    rect: Rect,
}

fn parts(deco: NativeDeco, w: i32, h: i32, scale: f64) -> Vec<Part> {
    let i = interaction(deco.state);
    let full = || Rect {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    let button = |part, state| Part {
        class: "BUTTON",
        dark_class: Some("DarkMode_Explorer::BUTTON"),
        part,
        state,
        rect: full(),
    };
    match deco.control {
        NativeControl::Button { primary } => {
            // PBS_DEFAULTED = 5 marks the dialog's default button.
            let state = if primary && i == 1 { 5 } else { i };
            vec![button(BP_PUSHBUTTON, state)]
        }
        // Checked states follow the unchecked ones: normal 5, hot 6, pressed 7.
        NativeControl::Checkbox { checked } => {
            vec![button(BP_CHECKBOX, if checked { i + 4 } else { i })]
        }
        NativeControl::Radio { selected } => {
            vec![button(BP_RADIOBUTTON, if selected { i + 4 } else { i })]
        }
        NativeControl::Slider { value } => {
            let thumb_w = (11.0 * scale).round() as i32;
            let track_h = (4.0 * scale).round() as i32;
            let x = ((w - thumb_w) as f64 * value as f64 / 1000.0).round() as i32;
            let mid = h / 2;
            let trackbar = |part, state, rect| Part {
                class: "TRACKBAR",
                dark_class: None,
                part,
                state,
                rect,
            };
            vec![
                trackbar(
                    TKP_TRACK,
                    1,
                    Rect {
                        left: 0,
                        top: mid - track_h / 2,
                        right: w,
                        bottom: mid - track_h / 2 + track_h,
                    },
                ),
                trackbar(
                    TKP_THUMB,
                    i,
                    Rect {
                        left: x,
                        top: 0,
                        right: x + thumb_w,
                        bottom: h,
                    },
                ),
            ]
        }
        NativeControl::Progress { value } => {
            let progress = |part, rect| Part {
                class: "PROGRESS",
                dark_class: None,
                part,
                state: 1,
                rect,
            };
            vec![
                progress(PP_BAR, full()),
                progress(
                    PP_FILL,
                    Rect {
                        left: 0,
                        top: 0,
                        right: (w as f64 * value as f64 / 1000.0).round() as i32,
                        bottom: h,
                    },
                ),
            ]
        }
        NativeControl::TextField => {
            // EPSN_NORMAL = 1, EPSN_HOT = 2, EPSN_FOCUSED = 3.
            let state = if deco.state.focused {
                3
            } else if deco.state.hovered {
                2
            } else {
                1
            };
            vec![Part {
                class: "EDIT",
                dark_class: Some("DarkMode_CFD::EDIT"),
                part: EP_EDITBORDER_NOSCROLL,
                state,
                rect: full(),
            }]
        }
        NativeControl::Switch { .. } => Vec::new(),
    }
}

pub fn render(deco: NativeDeco, size: PhysicalSize, scale: f64) -> Option<Pixmap> {
    let (w, h) = (size.width as i32, size.height as i32);
    let parts = parts(deco, w, h, scale);
    if parts.is_empty() {
        return None;
    }
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let info = BitmapInfoHeader {
            size: std::mem::size_of::<BitmapInfoHeader>() as u32,
            width: w,
            height: -h, // negative = top-down
            planes: 1,
            bit_count: 32,
            compression: 0,
            size_image: 0,
            x_ppm: 0,
            y_ppm: 0,
            clr_used: 0,
            clr_important: 0,
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, 0, &mut bits, std::ptr::null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, bitmap);
        let len = (w * h * 4) as usize;
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, len);

        let mut draw = |background: u8| -> Vec<u8> {
            pixels.fill(background);
            for p in &parts {
                let mut theme = std::ptr::null_mut();
                if deco.dark
                    && let Some(dark) = p.dark_class
                {
                    theme = OpenThemeData(std::ptr::null_mut(), wide(dark).as_ptr());
                }
                if theme.is_null() {
                    theme = OpenThemeData(std::ptr::null_mut(), wide(p.class).as_ptr());
                }
                if !theme.is_null() {
                    DrawThemeBackground(theme, dc, p.part, p.state, &p.rect, std::ptr::null());
                    CloseThemeData(theme);
                }
            }
            GdiFlush();
            pixels.to_vec()
        };
        let on_black = draw(0);
        let on_white = draw(255);

        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);

        let mut out = vec![0u8; len];
        let black = on_black.as_chunks::<4>().0;
        let white = on_white.as_chunks::<4>().0;
        for ((d, b), wh) in out.as_chunks_mut::<4>().0.iter_mut().zip(black).zip(white) {
            *d = straight_rgba(*b, *wh);
        }
        Some(Pixmap::from_rgba8(size, out))
    }
}

/// Recover straight RGBA from one BGRX pixel drawn over black and over white.
fn straight_rgba(on_black: [u8; 4], on_white: [u8; 4]) -> [u8; 4] {
    // white - black = 255 - alpha, per channel; green is the least noisy.
    let alpha = 255 - (on_white[1] as i32 - on_black[1] as i32).clamp(0, 255);
    if alpha == 0 {
        return [0, 0, 0, 0];
    }
    let c = |v: u8| ((v as i32 * 255 + alpha / 2) / alpha).min(255) as u8;
    [c(on_black[2]), c(on_black[1]), c(on_black[0]), alpha as u8]
}

#[cfg(test)]
mod tests {
    use super::straight_rgba;

    #[test]
    fn recovers_alpha_from_black_and_white() {
        assert_eq!(
            straight_rgba([0, 0, 0, 0], [255, 255, 255, 0]),
            [0, 0, 0, 0]
        );
        assert_eq!(
            straight_rgba([30, 20, 10, 0], [30, 20, 10, 0]),
            [10, 20, 30, 255]
        );
    }
}
