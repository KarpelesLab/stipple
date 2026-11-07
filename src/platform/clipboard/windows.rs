//! Win32 clipboard: `CF_UNICODETEXT` in a moveable global memory block.

use core::ffi::c_void;

type Handle = *mut c_void;

/// `CF_UNICODETEXT`: NUL-terminated UTF-16.
const CF_UNICODETEXT: u32 = 13;
/// `GMEM_MOVEABLE`: required for memory handed to `SetClipboardData`.
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(owner: Handle) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    fn SetClipboardData(format: u32, mem: Handle) -> Handle;
    fn GetActiveWindow() -> Handle;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalFree(mem: Handle) -> Handle;
    fn GlobalLock(mem: Handle) -> *mut c_void;
    fn GlobalUnlock(mem: Handle) -> i32;
    fn GlobalSize(mem: Handle) -> usize;
    fn Sleep(ms: u32);
}

/// Open the clipboard, retrying briefly: another process may hold it open for
/// a moment (clipboard managers, RDP). Returns whether it is now open.
fn open(owner: Handle) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(owner) } != 0 {
            return true;
        }
        unsafe { Sleep(5) };
    }
    false
}

pub fn text() -> Option<String> {
    if !open(std::ptr::null_mut()) {
        return None;
    }
    let out = unsafe {
        // The handle stays owned by the clipboard; we only lock it to copy out.
        let mem = GetClipboardData(CF_UNICODETEXT);
        if mem.is_null() {
            None
        } else {
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                None
            } else {
                // Bound the NUL scan by the block's size, in case it's missing.
                let max = GlobalSize(mem) / 2;
                let units = std::slice::from_raw_parts(ptr, max);
                let len = units.iter().position(|&u| u == 0).unwrap_or(max);
                let s = String::from_utf16_lossy(&units[..len]);
                GlobalUnlock(mem);
                Some(super::from_crlf(&s))
            }
        }
    };
    unsafe { CloseClipboard() };
    out
}

pub fn set_text(text: &str) -> bool {
    let wide: Vec<u16> = super::to_crlf(text)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if mem.is_null() {
            return false;
        }
        let dst = GlobalLock(mem) as *mut u16;
        if dst.is_null() {
            GlobalFree(mem);
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        GlobalUnlock(mem);

        // Own the clipboard with our window when there is one, so the data has
        // an owner (a NULL owner works for immediate rendering, too).
        if !open(GetActiveWindow()) {
            GlobalFree(mem);
            return false;
        }
        let ok = EmptyClipboard() != 0 && !SetClipboardData(CF_UNICODETEXT, mem).is_null();
        // On success the system owns `mem`; on failure it's still ours.
        if !ok {
            GlobalFree(mem);
        }
        CloseClipboard();
        ok
    }
}
