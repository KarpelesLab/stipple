//! Owned windows on macOS: a window that belongs to another
//! ([`WindowAttributes::parent`]) is its child window
//! (`addChildWindow:ordered:`), opening centered on it and staying above
//! it, or with [`WindowAttributes::sheet`] a sheet on it
//! (`beginSheet:completionHandler:`), which slides out of the parent's title
//! bar and blocks it until it closes (`endSheet:`). Message boxes for a
//! window find it here too ([`ns_window`]).

use std::cell::RefCell;
use std::ffi::c_void;

use super::{CgPoint, Id, MacWindow, Sel, WINDOWS, msg_rect, msg_void_id, objc_msgSend, sel};
use crate::platform::WindowId;
use crate::platform::window::WindowAttributes;

/// `NSWindowAbove`.
const NS_WINDOW_ABOVE: i64 = 1;

thread_local! {
    /// The sheets up, with the window each is on.
    static SHEETS: RefCell<Vec<(Id, Id)>> = const { RefCell::new(Vec::new()) };
}

/// The `NSWindow` of our window `id`, while it is open.
pub(crate) fn ns_window(id: WindowId) -> Option<Id> {
    WINDOWS.with(|ws| {
        ws.borrow()
            .iter()
            .find(|w| w.id == id.0 && !w.closing.get())
            .map(|w| w.window)
    })
}

/// The parent `attrs` names, if it is open.
pub(super) fn parent_of(attrs: &WindowAttributes) -> Option<Id> {
    ns_window(attrs.parent?)
}

/// Center `window` on `parent`.
///
/// # Safety
/// Both are live `NSWindow`s, on the main thread.
pub(super) unsafe fn center_on(window: Id, parent: Id) {
    unsafe {
        let p = msg_rect(parent, sel("frame"));
        let c = msg_rect(window, sel("frame"));
        let origin = CgPoint {
            x: p.origin.x + (p.size.width - c.size.width) / 2.0,
            y: p.origin.y + (p.size.height - c.size.height) / 2.0,
        };
        let set: unsafe extern "C" fn(Id, Sel, CgPoint) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set(window, sel("setFrameOrigin:"), origin);
    }
}

/// Keep `w` (on screen) above `parent`, moving with it.
///
/// # Safety
/// `parent` is a live `NSWindow`, on the main thread.
pub(super) unsafe fn add_child(w: &MacWindow, parent: Id) {
    unsafe {
        let add: unsafe extern "C" fn(Id, Sel, Id, i64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        add(
            parent,
            sel("addChildWindow:ordered:"),
            w.window,
            NS_WINDOW_ABOVE,
        );
    }
}

/// Show `w` as a sheet on `parent`.
///
/// # Safety
/// `parent` is a live `NSWindow`, on the main thread.
pub(super) unsafe fn begin_sheet(w: &MacWindow, parent: Id) {
    unsafe {
        let begin: unsafe extern "C" fn(Id, Sel, Id, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        // No completion handler: the app hears the close itself.
        begin(
            parent,
            sel("beginSheet:completionHandler:"),
            w.window,
            std::ptr::null_mut(),
        );
    }
    SHEETS.with(|s| s.borrow_mut().push((w.window, parent)));
}

/// `w` is closing: end its sheet first if it is one, and the sheet on it if
/// it has one.
///
/// # Safety
/// On the main thread, before `w`'s window is closed.
pub(super) unsafe fn closing(w: &MacWindow) {
    let ended: Vec<(Id, Id)> = SHEETS.with(|s| {
        let mut s = s.borrow_mut();
        let (ended, kept) = std::mem::take(&mut *s)
            .into_iter()
            .partition(|(sheet, parent)| *sheet == w.window || *parent == w.window);
        *s = kept;
        ended
    });
    for (sheet, parent) in ended {
        unsafe { msg_void_id(parent, sel("endSheet:"), sheet) };
    }
}
