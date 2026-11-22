//! The taskbar button's badge and progress: `ITaskbarList3::SetOverlayIcon`
//! with an icon Stipple draws (a red disc with the count, see
//! [`crate::platform::badge`]), and `SetProgressValue` / `SetProgressState`.
//!
//! The taskbar only takes these once the window's button exists, which it
//! says with the registered `TaskbarButtonCreated` message (also sent again
//! when Explorer restarts); what the app asked for before is applied then.

use super::{Handle, Hwnd, wide};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

type Hresult = i32;

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

/// `CLSID_TaskbarList`.
const CLSID_TASKBAR_LIST: Guid = Guid(
    0x56FD_F344,
    0xFD6D,
    0x11D0,
    [0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90],
);
/// `IID_ITaskbarList3`.
const IID_ITASKBAR_LIST3: Guid = Guid(
    0xEA1A_FB91,
    0x9E28,
    0x4B86,
    [0x90, 0xE9, 0x9E, 0x9F, 0x8A, 0x5E, 0xEF, 0xAF],
);

const CLSCTX_INPROC_SERVER: u32 = 0x1;
const COINIT_APARTMENTTHREADED: u32 = 0x2;

/// `TBPFLAG`: no bar, and a normal (green) one.
const TBPF_NOPROGRESS: u32 = 0;
const TBPF_NORMAL: u32 = 2;

/// The progress bar's scale: `SetProgressValue` takes whole numbers.
const PROGRESS_TOTAL: u64 = 10_000;

/// The `ITaskbarList3` vtable slots we call: `IUnknown` (0–2), then
/// `ITaskbarList` (`HrInit` 3 … `SetActiveAlt` 7), `ITaskbarList2`
/// (`MarkFullscreenWindow` 8), then `ITaskbarList3`'s own in order.
const SLOT_RELEASE: usize = 2;
const SLOT_HR_INIT: usize = 3;
const SLOT_SET_PROGRESS_VALUE: usize = 9;
const SLOT_SET_PROGRESS_STATE: usize = 10;
const SLOT_SET_OVERLAY_ICON: usize = 18;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn DestroyIcon(icon: Handle) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
}

/// `TaskbarButtonCreated`'s message number, once registered.
static BUTTON_CREATED: AtomicU32 = AtomicU32::new(0);

/// What each window's button should show, and the taskbar object.
#[derive(Default)]
struct State {
    /// `ITaskbarList3`, once made (null: not yet, or it failed).
    list: usize,
    /// Tried to make it already.
    tried: bool,
    /// Per window: its badge label and progress.
    wanted: Vec<(usize, Option<String>, Option<f64>)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// The font badges are drawn with, loaded on the first badge.
    static FONT: Option<crate::render::Font> = crate::render::Font::system_default();
}

/// Register `TaskbarButtonCreated` (once, before the first window shows).
pub(super) fn install() {
    if BUTTON_CREATED.load(Ordering::Acquire) == 0 {
        let name = wide("TaskbarButtonCreated");
        let id = unsafe { RegisterWindowMessageW(name.as_ptr()) };
        BUTTON_CREATED.store(id, Ordering::Release);
    }
}

fn debug(args: std::fmt::Arguments) {
    if std::env::var_os("STIPPLE_DEBUG_TASKBAR").is_some() {
        eprintln!("stipple taskbar: {args}");
    }
}

/// Call slot `n` of the `ITaskbarList3` at `p` as `F`.
unsafe fn slot<F: Copy>(p: *mut c_void, n: usize) -> F {
    unsafe {
        let vtbl = *(p as *const *const usize);
        std::mem::transmute_copy::<usize, F>(&*vtbl.add(n))
    }
}

/// The `ITaskbarList3`, made (and `HrInit`ed) on first use.
fn list() -> Option<*mut c_void> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if !s.tried {
            s.tried = true;
            unsafe {
                // Fine if COM is already up on this thread, either way.
                let _ = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
                let mut out = std::ptr::null_mut();
                let hr = CoCreateInstance(
                    &CLSID_TASKBAR_LIST,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_ITASKBAR_LIST3,
                    &mut out,
                );
                if hr < 0 || out.is_null() {
                    debug(format_args!(
                        "CoCreateInstance(TaskbarList) failed: {hr:#x}"
                    ));
                    return None;
                }
                let init: unsafe extern "system" fn(*mut c_void) -> Hresult =
                    slot(out, SLOT_HR_INIT);
                let hr = init(out);
                if hr < 0 {
                    debug(format_args!("HrInit failed: {hr:#x}"));
                    let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                        slot(out, SLOT_RELEASE);
                    release(out);
                    return None;
                }
                s.list = out as usize;
            }
        }
        (s.list != 0).then_some(s.list as *mut c_void)
    })
}

/// Remember `hwnd`'s badge or progress (`f` changes them), then show them.
fn want(hwnd: Hwnd, f: impl FnOnce(&mut Option<String>, &mut Option<f64>)) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let key = hwnd as usize;
        let at = match s.wanted.iter().position(|w| w.0 == key) {
            Some(i) => i,
            None => {
                s.wanted.push((key, None, None));
                s.wanted.len() - 1
            }
        };
        let w = &mut s.wanted[at];
        f(&mut w.1, &mut w.2);
    });
}

fn wanted(hwnd: Hwnd) -> (Option<String>, Option<f64>) {
    STATE.with(|s| {
        s.borrow()
            .wanted
            .iter()
            .find(|w| w.0 == hwnd as usize)
            .map(|w| (w.1.clone(), w.2))
            .unwrap_or_default()
    })
}

/// Show `label` as the overlay icon on `hwnd`'s taskbar button, or none.
pub(super) fn set_badge(hwnd: Hwnd, label: Option<&str>) {
    want(hwnd, |badge, _| *badge = label.map(str::to_string));
    let hr = apply_badge(hwnd, label);
    debug(format_args!("overlay {label:?}: {hr:#x}"));
}

/// Show `progress` on `hwnd`'s taskbar button, or none.
pub(super) fn set_progress(hwnd: Hwnd, progress: Option<f64>) {
    let progress = progress.filter(|p| !p.is_nan()).map(|p| p.clamp(0.0, 1.0));
    want(hwnd, |_, p| *p = progress);
    let hr = apply_progress(hwnd, progress);
    debug(format_args!("progress {progress:?}: {hr:#x}"));
}

/// `E_FAIL`, for no taskbar object.
const E_FAIL: Hresult = 0x8000_4005_u32 as i32;

fn apply_badge(hwnd: Hwnd, label: Option<&str>) -> Hresult {
    let Some(list) = list() else { return E_FAIL };
    let (icon, text) = match label {
        Some(label) => {
            // The overlay is a small icon: 16 pixels at 96 dpi.
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
            let px = (16 * dpi).div_ceil(96);
            let pixmap = FONT.with(|f| crate::platform::badge::render(label, px, f.as_ref()));
            (super::create_icon(&pixmap), wide(label))
        }
        None => (std::ptr::null_mut(), wide("")),
    };
    let hr = unsafe {
        let set: unsafe extern "system" fn(*mut c_void, Hwnd, Handle, *const u16) -> Hresult =
            slot(list, SLOT_SET_OVERLAY_ICON);
        set(list, hwnd, icon, text.as_ptr())
    };
    // The taskbar keeps a copy of the icon.
    if !icon.is_null() {
        unsafe { DestroyIcon(icon) };
    }
    hr
}

fn apply_progress(hwnd: Hwnd, progress: Option<f64>) -> Hresult {
    let Some(list) = list() else { return E_FAIL };
    unsafe {
        let state: unsafe extern "system" fn(*mut c_void, Hwnd, u32) -> Hresult =
            slot(list, SLOT_SET_PROGRESS_STATE);
        let Some(p) = progress else {
            return state(list, hwnd, TBPF_NOPROGRESS);
        };
        let hr = state(list, hwnd, TBPF_NORMAL);
        if hr < 0 {
            return hr;
        }
        let value: unsafe extern "system" fn(*mut c_void, Hwnd, u64, u64) -> Hresult =
            slot(list, SLOT_SET_PROGRESS_VALUE);
        value(list, hwnd, progress_value(p), PROGRESS_TOTAL)
    }
}

/// `p` (0 to 1) on the bar's scale.
fn progress_value(p: f64) -> u64 {
    (p.clamp(0.0, 1.0) * PROGRESS_TOTAL as f64).round() as u64
}

/// `msg` is `TaskbarButtonCreated` for `hwnd`: show what the app asked for
/// before the button was there. Returns whether it was.
pub(super) fn button_created(hwnd: Hwnd, msg: u32) -> bool {
    let id = BUTTON_CREATED.load(Ordering::Acquire);
    if id == 0 || msg != id {
        return false;
    }
    let (badge, progress) = wanted(hwnd);
    if badge.is_some() {
        let hr = apply_badge(hwnd, badge.as_deref());
        debug(format_args!("button created: overlay {badge:?}: {hr:#x}"));
    }
    if progress.is_some() {
        let hr = apply_progress(hwnd, progress);
        debug(format_args!(
            "button created: progress {progress:?}: {hr:#x}"
        ));
    }
    self_test(hwnd);
    true
}

/// `hwnd` is going away.
pub(super) fn forget(hwnd: Hwnd) {
    STATE.with(|s| s.borrow_mut().wanted.retain(|w| w.0 != hwnd as usize));
}

/// The CI self-test (`STIPPLE_WIN_TASKBAR`): once the button exists, set an
/// overlay and a progress and clear them, printing each call's `HRESULT`
/// (the taskbar can't be read back).
fn self_test(hwnd: Hwnd) {
    if std::env::var_os("STIPPLE_WIN_TASKBAR").is_none() {
        return;
    }
    let overlay = apply_badge(hwnd, Some("3"));
    let dot = apply_badge(hwnd, Some("New"));
    let progress = apply_progress(hwnd, Some(0.5));
    let (badge, wanted_progress) = wanted(hwnd);
    let cleared = (
        apply_badge(hwnd, badge.as_deref()),
        apply_progress(hwnd, wanted_progress),
    );
    println!(
        "Win32 taskbar: overlay={overlay:#x} dot={dot:#x} progress={progress:#x} restored={:#x}/{:#x}",
        cleared.0, cleared.1
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_maps_onto_the_bars_scale() {
        assert_eq!(progress_value(0.0), 0);
        assert_eq!(progress_value(0.5), 5_000);
        assert_eq!(progress_value(1.0), PROGRESS_TOTAL);
        assert_eq!(progress_value(2.0), PROGRESS_TOTAL);
    }
}
