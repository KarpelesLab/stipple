//! Windows: `ShellExecuteW` opens URLs; notifications are balloons on the
//! notification-area icon (see `backend::windows`).

use super::Notification;
use crate::platform::PlatformError;
use std::ffi::c_void;

const SW_SHOWNORMAL: i32 = 1;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> *mut c_void;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn open_url(url: &str) -> Result<(), PlatformError> {
    // ShellExecute may hand the URL to a shell extension, which wants COM. A
    // thread already in another apartment just keeps it.
    unsafe {
        CoInitializeEx(
            std::ptr::null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        )
    };
    // A `file:` URL goes over as a path, which every handler understands.
    let target = match url.strip_prefix("file:///") {
        Some(path) => percent_decode(path).replace('/', "\\"),
        None => url.to_string(),
    };
    let op = wide("open");
    let file = wide(&target);
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    // Success is any value above 32; below are the SE_ERR_* codes.
    if r > 32 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!(
            "ShellExecuteW({target}) failed with {r}"
        )))
    }
}

pub fn notify(n: &Notification) -> Result<(), PlatformError> {
    crate::platform::backend::windows::notify(&n.title, &n.body)
}

/// Decode `%XX` escapes (UTF-8).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
