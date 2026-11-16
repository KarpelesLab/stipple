//! Whether a window can be seen ([`Event::Occluded`]): minimized
//! (`IsIconic`), or cloaked by the compositor (`DWMWA_CLOAKED`: on another
//! virtual desktop, or hidden by the shell). Read again whenever the window
//! is resized, moved, shown or hidden, activated or deactivated, and painted.
//!
//! A window covered by others isn't reported here; a GPU window's swap chain
//! says so itself (`DXGI_STATUS_OCCLUDED` from `Present`, which stops that
//! window's frames until it's seen again: see `app_occlusion`).

use super::{Hwnd, IsIconic, push_event_to, read_win, with_win};
use crate::platform::Event;
use std::ffi::c_void;

/// `DWMWA_CLOAKED`: a `DWORD`, non-zero while the window is cloaked.
const DWMWA_CLOAKED: u32 = 14;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(hwnd: Hwnd, attr: u32, value: *mut c_void, size: u32) -> i32;
}

/// Whether `hwnd` is cloaked (false where the attribute can't be read).
fn cloaked(hwnd: Hwnd) -> bool {
    let mut value = 0u32;
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr >= 0 && value != 0
}

/// Whether a window minimized or cloaked as said is hidden.
fn hidden(iconic: bool, cloaked: bool) -> bool {
    iconic || cloaked
}

/// Read whether `hwnd` can be seen, and tell the app when that changed.
pub(super) fn refresh(hwnd: Hwnd) {
    if read_win(hwnd, |_| ()).is_none() {
        return;
    }
    let now = hidden(unsafe { IsIconic(hwnd) } != 0, cloaked(hwnd));
    let was = with_win(hwnd, |w| std::mem::replace(&mut w.occluded, now));
    if was != now {
        crate::platform::window::debug_window(format_args!("occluded {:#x} {now}", hwnd as u64));
        push_event_to(hwnd, Event::Occluded(now));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn minimized_or_cloaked_is_hidden() {
        assert!(!super::hidden(false, false));
        assert!(super::hidden(true, false));
        assert!(super::hidden(false, true));
    }
}
