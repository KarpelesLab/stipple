//! Input methods on Windows through IMM32.
//!
//! The IME composes in its own context; we draw the composition ourselves,
//! inline, so the default composition window is turned off
//! (`WM_IME_SETCONTEXT` without `ISC_SHOWUICOMPOSITIONWINDOW`, and
//! `WM_IME_STARTCOMPOSITION` not passed on). `WM_IME_COMPOSITION` then
//! carries the composition string (`GCS_COMPSTR`, with its caret in
//! `GCS_CURSORPOS`) and the final text (`GCS_RESULTSTR`), which become
//! [`Ime`] events. The candidate list is placed under the caret rectangle the
//! App reports, with `ImmSetCandidateWindow` and `ImmSetCompositionWindow`.
//! The IME is switched off while no text field has focus by associating no
//! input context with the window.
//!
//! Dead keys need none of this: `TranslateMessage` turns them into the
//! composed character's `WM_CHAR`.

use super::{CTX, Handle, Hwnd, Lparam, Point, Rect, current_scale, push_event};
use crate::platform::ImeState;
use crate::platform::event::{Event, Ime};
use crate::platform::input;
use std::ffi::c_void;

pub(super) const WM_IME_STARTCOMPOSITION: u32 = 0x010D;
pub(super) const WM_IME_ENDCOMPOSITION: u32 = 0x010E;
pub(super) const WM_IME_COMPOSITION: u32 = 0x010F;
pub(super) const WM_IME_SETCONTEXT: u32 = 0x0281;
/// `WM_IME_SETCONTEXT`'s flag for the IME's own composition window.
pub(super) const ISC_SHOWUICOMPOSITIONWINDOW: Lparam = 0x8000_0000u32 as i32 as Lparam;

const GCS_COMPSTR: u32 = 0x0008;
const GCS_CURSORPOS: u32 = 0x0080;
const GCS_RESULTSTR: u32 = 0x0800;
const CFS_POINT: u32 = 0x0002;
const CFS_EXCLUDE: u32 = 0x0080;
/// `ImmAssociateContextEx`: give the window back its default input context.
const IACE_DEFAULT: u32 = 0x0010;

#[repr(C)]
struct CompositionForm {
    style: u32,
    current_pos: Point,
    area: Rect,
}

#[repr(C)]
struct CandidateForm {
    index: u32,
    style: u32,
    current_pos: Point,
    area: Rect,
}

#[link(name = "imm32")]
unsafe extern "system" {
    fn ImmGetContext(hwnd: Hwnd) -> Handle;
    fn ImmReleaseContext(hwnd: Hwnd, himc: Handle) -> i32;
    fn ImmGetCompositionStringW(himc: Handle, index: u32, buf: *mut c_void, len: u32) -> i32;
    fn ImmSetCompositionWindow(himc: Handle, form: *const CompositionForm) -> i32;
    fn ImmSetCandidateWindow(himc: Handle, form: *const CandidateForm) -> i32;
    fn ImmAssociateContextEx(hwnd: Hwnd, himc: Handle, flags: u32) -> i32;
}

/// The caret rectangle in client-area physical pixels.
pub(super) fn caret_rect(state: &ImeState) -> Rect {
    let s = current_scale();
    let r = state.cursor_area;
    Rect {
        left: (r.min_x() * s).round() as i32,
        top: (r.min_y() * s).round() as i32,
        right: (r.max_x() * s).round() as i32,
        bottom: (r.max_y() * s).round() as i32,
    }
}

/// Enable the IME for a focused text field and place its windows, or disable
/// it.
pub(super) fn set_state(hwnd: Hwnd, state: Option<&ImeState>) {
    let was = CTX.with(|c| {
        let mut c = c.borrow_mut();
        let was = c.ime_area.is_some();
        c.ime_area = state.map(caret_rect);
        was
    });
    unsafe {
        match state {
            Some(_) if !was => {
                ImmAssociateContextEx(hwnd, std::ptr::null_mut(), IACE_DEFAULT);
            }
            None if was => {
                // No context: keys go straight to WM_CHAR.
                ImmAssociateContextEx(hwnd, std::ptr::null_mut(), 0);
            }
            _ => {}
        }
    }
    if state.is_some() {
        place_windows(hwnd);
    }
}

/// Put the composition and candidate windows by the caret.
pub(super) fn place_windows(hwnd: Hwnd) {
    let Some(area) = CTX.with(|c| c.borrow().ime_area.as_ref().map(copy_rect)) else {
        return;
    };
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_null() {
            return;
        }
        let composition = CompositionForm {
            style: CFS_POINT,
            current_pos: Point {
                x: area.left,
                y: area.top,
            },
            area: copy_rect(&area),
        };
        ImmSetCompositionWindow(himc, &composition);
        // The candidate list goes below the caret, never over it.
        let candidate = CandidateForm {
            index: 0,
            style: CFS_EXCLUDE,
            current_pos: Point {
                x: area.left,
                y: area.bottom,
            },
            area: copy_rect(&area),
        };
        ImmSetCandidateWindow(himc, &candidate);
        ImmReleaseContext(hwnd, himc);
    }
}

fn copy_rect(r: &Rect) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// One of the context's composition strings.
unsafe fn composition_string(himc: Handle, index: u32) -> String {
    let bytes = unsafe { ImmGetCompositionStringW(himc, index, std::ptr::null_mut(), 0) };
    if bytes <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; bytes as usize / 2];
    let got = unsafe {
        ImmGetCompositionStringW(himc, index, buf.as_mut_ptr() as *mut c_void, bytes as u32)
    };
    if got <= 0 {
        return String::new();
    }
    buf.truncate(got as usize / 2);
    String::from_utf16_lossy(&buf)
}

/// Handle `WM_IME_COMPOSITION`: queue the commit and the new preedit.
pub(super) fn composition(hwnd: Hwnd, lp: Lparam) {
    let flags = lp as u32;
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_null() {
            return;
        }
        if flags & GCS_RESULTSTR != 0 {
            let text = composition_string(himc, GCS_RESULTSTR);
            if !text.is_empty() {
                push_event(Event::Ime(Ime::Commit(text)));
            }
        }
        if flags & GCS_COMPSTR != 0 {
            let text = composition_string(himc, GCS_COMPSTR);
            let cursor = if flags & GCS_CURSORPOS != 0 {
                let units = ImmGetCompositionStringW(himc, GCS_CURSORPOS, std::ptr::null_mut(), 0);
                let at = input::utf16_to_byte(&text, units.max(0) as usize);
                Some((at, at))
            } else {
                Some((text.len(), text.len()))
            };
            push_event(Event::Ime(Ime::Preedit { text, cursor }));
        } else if flags & GCS_RESULTSTR == 0 {
            // Neither: the composition was cancelled.
            push_event(Event::Ime(Ime::Preedit {
                text: String::new(),
                cursor: None,
            }));
        }
        ImmReleaseContext(hwnd, himc);
    }
}

/// Handle `WM_IME_ENDCOMPOSITION`: whatever was composing is gone.
pub(super) fn end_composition() {
    push_event(Event::Ime(Ime::Preedit {
        text: String::new(),
        cursor: None,
    }));
}
