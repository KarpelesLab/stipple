//! Win32 clipboard: `CF_UNICODETEXT`, the registered `HTML Format`
//! (`CF_HTML`), and images as `CF_DIBV5` plus the registered `PNG` format,
//! each in a moveable global memory block.

use core::ffi::c_void;

use super::format::{cf_html_decode, cf_html_encode, dib_decode, dib_encode, encode_png};
use super::{ClipboardContent, ClipboardFormats};
use crate::render::Pixmap;

type Handle = *mut c_void;

/// `CF_UNICODETEXT`: NUL-terminated UTF-16.
const CF_UNICODETEXT: u32 = 13;
/// `CF_DIB`: a `BITMAPINFO` then the pixels.
const CF_DIB: u32 = 8;
/// `CF_DIBV5`: a `BITMAPV5HEADER` then the pixels (with alpha).
const CF_DIBV5: u32 = 17;
/// `GMEM_MOVEABLE`: required for memory handed to `SetClipboardData`.
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(owner: Handle) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    fn SetClipboardData(format: u32, mem: Handle) -> Handle;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
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

/// A registered format's id (0 on failure).
fn registered(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
}

/// The bytes of `format` on the (open) clipboard.
unsafe fn get(format: u32) -> Option<Vec<u8>> {
    if format == 0 {
        return None;
    }
    unsafe {
        // The handle stays owned by the clipboard; we only lock it to copy out.
        let mem = GetClipboardData(format);
        if mem.is_null() {
            return None;
        }
        let ptr = GlobalLock(mem) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, GlobalSize(mem)).to_vec();
        GlobalUnlock(mem);
        Some(bytes)
    }
}

/// `bytes` in a fresh moveable block, for `SetClipboardData`.
fn global(bytes: &[u8]) -> Option<Handle> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if mem.is_null() {
            return None;
        }
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            GlobalFree(mem);
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        GlobalUnlock(mem);
        Some(mem)
    }
}

/// UTF-16 bytes up to the first NUL.
fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    super::from_crlf(&String::from_utf16_lossy(&units))
}

pub fn text() -> Option<String> {
    read(ClipboardFormats::TEXT).and_then(|c| c.text)
}

pub fn set_text(text: &str) -> bool {
    write(&ClipboardContent::text(text))
}

pub fn read(formats: ClipboardFormats) -> Option<ClipboardContent> {
    let (html_format, png_format) = (registered("HTML Format"), registered("PNG"));
    if !open(std::ptr::null_mut()) {
        return None;
    }
    let mut content = ClipboardContent::default();
    unsafe {
        if formats.text {
            content.text = get(CF_UNICODETEXT).map(|b| utf16_text(&b));
        }
        if formats.html {
            content.html = get(html_format).and_then(|b| cf_html_decode(&b));
        }
        if formats.image {
            content.image = get(png_format)
                .and_then(|b| Pixmap::decode(&b).ok())
                .or_else(|| get(CF_DIBV5).and_then(|b| dib_decode(&b)))
                .or_else(|| get(CF_DIB).and_then(|b| dib_decode(&b)));
        }
        CloseClipboard();
    }
    (!content.is_empty()).then_some(content)
}

pub fn write(content: &ClipboardContent) -> bool {
    // Every representation, encoded before the clipboard is opened.
    let mut blocks: Vec<(u32, Vec<u8>)> = Vec::new();
    if let Some(text) = &content.text {
        let wide = super::to_crlf(text)
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        blocks.push((CF_UNICODETEXT, wide));
    }
    if let Some(html) = &content.html {
        let mut data = cf_html_encode(html);
        data.push(0);
        blocks.push((registered("HTML Format"), data));
    }
    if let Some(image) = &content.image {
        blocks.push((CF_DIBV5, dib_encode(image)));
        if let Some(png) = encode_png(image) {
            blocks.push((registered("PNG"), png));
        }
    }
    blocks.retain(|(format, _)| *format != 0);
    if blocks.is_empty() {
        return false;
    }
    let mut handles = Vec::new();
    for (format, bytes) in &blocks {
        match global(bytes) {
            Some(mem) => handles.push((*format, mem)),
            None => {
                for (_, mem) in handles {
                    unsafe { GlobalFree(mem) };
                }
                return false;
            }
        }
    }
    unsafe {
        // Own the clipboard with our window when there is one, so the data has
        // an owner (a NULL owner works for immediate rendering, too).
        if !open(GetActiveWindow()) || EmptyClipboard() == 0 {
            CloseClipboard();
            for (_, mem) in handles {
                GlobalFree(mem);
            }
            return false;
        }
        let mut ok = true;
        for (format, mem) in handles {
            // On success the system owns `mem`; on failure it's still ours.
            if SetClipboardData(format, mem).is_null() {
                GlobalFree(mem);
                ok = false;
            }
        }
        CloseClipboard();
        ok
    }
}
