//! macOS: `NSWorkspace` opens URLs; notifications go through `osascript`.
//!
//! The notification frameworks need an app bundle: `UNUserNotificationCenter`
//! raises an exception in a bare executable (no bundle identifier), and the
//! deprecated `NSUserNotificationCenter` hands it `nil`. AppleScript's
//! `display notification` works from any process (macOS shows it as coming
//! from Script Editor), so that's what [`notify`] runs.

use super::Notification;
use crate::platform::PlatformError;
use crate::platform::native::objc::{Id, class, msg, nsstring, with_pool};

pub fn open_url(url: &str) -> Result<(), PlatformError> {
    with_pool(|| {
        let s = nsstring(url);
        let nsurl = unsafe { msg!(class("NSURL"), "URLWithString:", (s: Id) -> Id) };
        if nsurl.is_null() {
            return Err(PlatformError::Os(format!("NSURL refused {url:?}")));
        }
        let ws = unsafe { msg!(class("NSWorkspace"), "sharedWorkspace", () -> Id) };
        let ok = unsafe { msg!(ws, "openURL:", (nsurl: Id) -> bool) };
        if ok {
            Ok(())
        } else {
            Err(PlatformError::Os(format!(
                "NSWorkspace couldn't open {url}"
            )))
        }
    })
}

pub fn notify(n: &Notification) -> Result<(), PlatformError> {
    let script = format!(
        "display notification {} with title {}",
        applescript_string(&n.body),
        applescript_string(&n.title)
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| PlatformError::Os(format!("osascript: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("osascript exited with {status}")))
    }
}

/// `s` as an AppleScript string literal.
fn applescript_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- Sharing and haptics ---------------------------------------------------------

use super::{Haptic, Share};
use crate::platform::native::objc::{CgRect, Sel, objc_msgSend, on_main_thread, sel};
use std::ffi::c_void;
use std::sync::Mutex;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn objc_getProtocol(name: *const i8) -> *mut c_void;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
    fn class_addProtocol(cls: Id, protocol: *mut c_void) -> bool;
}

/// The pickers on screen (retained), with the share request each answers.
static PICKERS: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

/// Show an `NSSharingServicePicker` with `share`'s items from the key
/// window's view, for share request `id`. `false` if there's no window (or
/// this isn't the main thread).
pub(crate) fn share(id: u32, share: &Share) -> bool {
    if !on_main_thread() {
        return false;
    }
    with_pool(|| unsafe {
        let app = msg!(class("NSApplication"), "sharedApplication", () -> Id);
        if app.is_null() {
            return false;
        }
        let mut window = msg!(app, "keyWindow", () -> Id);
        if window.is_null() {
            window = msg!(app, "mainWindow", () -> Id);
        }
        if window.is_null() {
            return false;
        }
        let view = msg!(window, "contentView", () -> Id);
        if view.is_null() {
            return false;
        }
        let items = msg!(class("NSMutableArray"), "array", () -> Id);
        if !share.text.is_empty() {
            let s = nsstring(&share.text);
            msg!(items, "addObject:", (s: Id) -> ());
        }
        if !share.url.is_empty() {
            let s = nsstring(&share.url);
            let url = msg!(class("NSURL"), "URLWithString:", (s: Id) -> Id);
            let item = if url.is_null() { s } else { url };
            msg!(items, "addObject:", (item: Id) -> ());
        }
        for path in super::sharing::write_files(id, &share.files) {
            let s = nsstring(&path.to_string_lossy());
            let url = msg!(class("NSURL"), "fileURLWithPath:", (s: Id) -> Id);
            msg!(items, "addObject:", (url: Id) -> ());
        }
        let picker = msg!(class("NSSharingServicePicker"), "alloc", () -> Id);
        let picker = msg!(picker, "initWithItems:", (items: Id) -> Id);
        if picker.is_null() {
            return false;
        }
        let delegate = delegate();
        msg!(picker, "setDelegate:", (delegate: Id) -> ());
        PICKERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((picker as usize, id));
        // NSRect comes back in registers on arm64 (no `_stret`), as the
        // backend's own `msg_rect` relies on.
        let bounds = {
            let f: unsafe extern "C" fn(Id, Sel) -> CgRect =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(view, sel("bounds"))
        };
        // Our view is flipped: the anchor's top-left coordinates are its own.
        let rect = match share.anchor {
            Some(r) => CgRect {
                x: r.origin.x,
                y: r.origin.y,
                width: r.size.width.max(1.0),
                height: r.size.height.max(1.0),
            },
            None => CgRect {
                x: bounds.x + bounds.width / 2.0,
                y: bounds.y + bounds.height / 2.0,
                width: 1.0,
                height: 1.0,
            },
        };
        // NSRectEdgeMaxY: below the anchor in a flipped view.
        let edge: usize = 3;
        let show: unsafe extern "C" fn(Id, Sel, CgRect, Id, usize) =
            std::mem::transmute(objc_msgSend as *const c_void);
        show(
            picker,
            sel("showRelativeToRect:ofView:preferredEdge:"),
            rect,
            view,
            edge,
        );
        true
    })
}

/// `sharingServicePicker:didChooseSharingService:`: `service` is nil when the
/// picker was dismissed.
extern "C" fn did_choose(_this: Id, _cmd: Sel, picker: Id, service: Id) {
    let id = {
        let mut pickers = PICKERS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(i) = pickers.iter().position(|&(p, _)| p == picker as usize) else {
            return;
        };
        pickers.remove(i).1
    };
    unsafe { msg!(picker, "release", () -> ()) };
    let outcome = if service.is_null() {
        super::ShareOutcome::Cancelled
    } else {
        super::ShareOutcome::Shared
    };
    super::sharing::answer(id, outcome.code());
}

/// The pickers' delegate, made once.
fn delegate() -> Id {
    static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DELEGATE.get_or_init(|| unsafe {
        let name = c"StippleSharingPickerDelegate";
        let mut cls = objc_getClass(name.as_ptr());
        if cls.is_null() {
            cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            class_addMethod(
                cls,
                sel("sharingServicePicker:didChooseSharingService:"),
                did_choose as *const c_void,
                c"v@:@@".as_ptr(),
            );
            let protocol = objc_getProtocol(c"NSSharingServicePickerDelegate".as_ptr());
            if !protocol.is_null() {
                class_addProtocol(cls, protocol);
            }
            objc_registerClassPair(cls);
        }
        msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) as usize
    }) as Id
}

/// `NSHapticFeedbackManager`'s default performer: felt on a Force Touch
/// trackpad while the user touches it.
pub(crate) fn haptic(kind: Haptic) {
    if !on_main_thread() {
        return;
    }
    // NSHapticFeedbackPattern: generic 0, alignment 1, levelChange 2.
    let pattern: isize = match kind {
        Haptic::Selection | Haptic::Light => 1,
        Haptic::Medium | Haptic::Heavy => 2,
        _ => 0,
    };
    with_pool(|| unsafe {
        let manager = class("NSHapticFeedbackManager");
        if manager.is_null() {
            return;
        }
        let performer = msg!(manager, "defaultPerformer", () -> Id);
        if performer.is_null() {
            return;
        }
        // NSHapticFeedbackPerformanceTimeNow.
        let now: usize = 1;
        msg!(performer, "performFeedbackPattern:performanceTime:", (pattern: isize, now: usize) -> ());
    })
}
