//! Receiving drops on macOS: `NSDraggingDestination` on the view.
//!
//! The view registers for file URLs and plain text. AppKit calls
//! `draggingEntered:` / `draggingUpdated:` / `draggingExited:` /
//! `performDragOperation:` from inside its event handling, where the app's
//! handler can't run: each queues its event (delivered right after, like menu
//! picks), and answers with the app's last verdict (from
//! `Window::set_drop_accepted`), so the cursor's badge trails the pointer by
//! one move; a drag entering is first answered "copy" when it carries files or
//! text. `performDragOperation:` reads the pasteboard: each item's
//! `public.file-url` (resolved through `NSURL`, so Finder's file-reference
//! URLs become paths), else `public.utf8-plain-text`.

use super::{
    CTX, CgPoint, Class, Id, Sel, class, class_addMethod, msg_id, msg_id_id, msg_id_u64, msg_u64,
    msg_void_id, nsstring, nsstring_to_string, objc_allocateClassPair, objc_getClass, objc_msgSend,
    objc_registerClassPair, sel,
};
use crate::geometry::Point;
use crate::platform::event::{DragOffer, DropData, Event};
use std::ffi::c_void;

const FILE_URL: &str = "public.file-url";
const PLAIN_TEXT: &str = "public.utf8-plain-text";
const DRAG_OPERATION_NONE: u64 = 0;
const DRAG_OPERATION_COPY: u64 = 1;

/// Add the `NSDraggingDestination` methods to the view class.
pub(super) unsafe fn add_methods(cls: Class) {
    unsafe {
        let op_types = c"Q@:@";
        class_addMethod(
            cls,
            sel("draggingEntered:"),
            dragging_entered as *const c_void,
            op_types.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("draggingUpdated:"),
            dragging_updated as *const c_void,
            op_types.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("draggingExited:"),
            dragging_exited as *const c_void,
            c"v@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("prepareForDragOperation:"),
            prepare_for_drag as *const c_void,
            c"c@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("performDragOperation:"),
            perform_drag as *const c_void,
            c"c@:@".as_ptr(),
        );
    }
}

/// Register `view` for the dragged types we read.
pub(super) unsafe fn register(view: Id) {
    unsafe {
        let types = msg_id(class("NSMutableArray"), sel("array"));
        for t in [FILE_URL, PLAIN_TEXT] {
            msg_void_id(types, sel("addObject:"), nsstring(t));
        }
        msg_void_id(view, sel("registerForDraggedTypes:"), types);
    }
}

fn queue(event: Event) {
    CTX.with(|c| c.borrow_mut().pending.push(event));
}

/// The drag's position in the (flipped) view, in points.
unsafe fn location(view: Id, info: Id) -> Point {
    unsafe {
        let get: unsafe extern "C" fn(Id, Sel) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let in_window = get(info, sel("draggingLocation"));
        let convert: unsafe extern "C" fn(Id, Sel, CgPoint, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = convert(
            view,
            sel("convertPoint:fromView:"),
            in_window,
            std::ptr::null_mut(),
        );
        Point::new(p.x, p.y)
    }
}

/// Whether the pasteboard has `ty`.
unsafe fn has_type(pasteboard: Id, ty: &str) -> bool {
    unsafe {
        let types = msg_id(pasteboard, sel("types"));
        if types.is_null() {
            return false;
        }
        let f: unsafe extern "C" fn(Id, Sel, Id) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(types, sel("containsObject:"), nsstring(ty)) != 0
    }
}

unsafe fn offer_of(info: Id) -> DragOffer {
    unsafe {
        let pb = msg_id(info, sel("draggingPasteboard"));
        DragOffer {
            files: has_type(pb, FILE_URL),
            text: has_type(pb, PLAIN_TEXT),
            internal: false,
        }
    }
}

/// The operation to answer: copy if the app takes the drag.
fn operation() -> u64 {
    if CTX.with(|c| c.borrow().drop_accepted) {
        DRAG_OPERATION_COPY
    } else {
        DRAG_OPERATION_NONE
    }
}

extern "C" fn dragging_entered(this: Id, _cmd: Sel, info: Id) -> u64 {
    let offer = unsafe { offer_of(info) };
    // Until the app answers, take anything we can read.
    CTX.with(|c| c.borrow_mut().drop_accepted = !offer.is_empty());
    let position = unsafe { location(this, info) };
    queue(Event::DragEnter { position, offer });
    operation()
}

extern "C" fn dragging_updated(this: Id, _cmd: Sel, info: Id) -> u64 {
    let position = unsafe { location(this, info) };
    // AppKit repeats this while the pointer rests; only moves are news.
    let moved = CTX.with(|c| {
        let mut c = c.borrow_mut();
        let moved = c.drag_last != Some(position);
        c.drag_last = Some(position);
        moved
    });
    if moved {
        queue(Event::DragOver { position });
    }
    operation()
}

extern "C" fn dragging_exited(_this: Id, _cmd: Sel, _info: Id) {
    CTX.with(|c| c.borrow_mut().drag_last = None);
    queue(Event::DragLeave);
}

extern "C" fn prepare_for_drag(_this: Id, _cmd: Sel, _info: Id) -> i8 {
    (operation() != DRAG_OPERATION_NONE) as i8
}

extern "C" fn perform_drag(this: Id, _cmd: Sel, info: Id) -> i8 {
    CTX.with(|c| c.borrow_mut().drag_last = None);
    let position = unsafe { location(this, info) };
    let pb = unsafe { msg_id(info, sel("draggingPasteboard")) };
    let data = unsafe { read_files(pb).or_else(|| read_text(pb)) };
    let ok = data.is_some() && operation() != DRAG_OPERATION_NONE;
    queue(match data {
        Some(data) => Event::Drop { position, data },
        None => Event::DragLeave,
    });
    ok as i8
}

/// The file paths on the pasteboard, one per item.
unsafe fn read_files(pb: Id) -> Option<DropData> {
    unsafe {
        let items = msg_id(pb, sel("pasteboardItems"));
        if items.is_null() {
            return None;
        }
        let mut files = Vec::new();
        for i in 0..msg_u64(items, sel("count")) {
            let item = msg_id_u64(items, sel("objectAtIndex:"), i);
            let s = msg_id_id(item, sel("stringForType:"), nsstring(FILE_URL));
            if s.is_null() {
                continue;
            }
            let url = msg_id_id(class("NSURL"), sel("URLWithString:"), s);
            // A file-reference URL (file:///.file/id=…) resolves to its path.
            let url = if url.is_null() {
                url
            } else {
                msg_id(url, sel("filePathURL"))
            };
            if url.is_null() {
                continue;
            }
            let path = nsstring_to_string(msg_id(url, sel("path")));
            if !path.is_empty() {
                files.push(std::path::PathBuf::from(path));
            }
        }
        (!files.is_empty()).then_some(DropData::Files(files))
    }
}

unsafe fn read_text(pb: Id) -> Option<DropData> {
    unsafe {
        let s = msg_id_id(pb, sel("stringForType:"), nsstring(PLAIN_TEXT));
        (!s.is_null()).then(|| DropData::Text(nsstring_to_string(s)))
    }
}

// ---- Self-test --------------------------------------------------------------
//
// CI can't drag in Finder, so with `STIPPLE_COCOA_DROP=<path>` the backend
// drives the view's own dragging methods once the loop runs, through real objc
// dispatch, with a stand-in `NSDraggingInfo` whose pasteboard (a private,
// uniquely named one) holds that file's URL. The drop reaches the app like a
// real one.

thread_local! {
    /// The stand-in's pasteboard and location (window points), read by its
    /// methods.
    static FAKE: std::cell::Cell<Option<(usize, CgPoint)>> = const { std::cell::Cell::new(None) };
}

extern "C" fn fake_pasteboard(_this: Id, _cmd: Sel) -> Id {
    FAKE.with(|f| f.get().map_or(std::ptr::null_mut(), |(pb, _)| pb as Id))
}

extern "C" fn fake_location(_this: Id, _cmd: Sel) -> CgPoint {
    FAKE.with(|f| f.get().map_or(CgPoint { x: 0.0, y: 0.0 }, |(_, p)| p))
}

extern "C" fn fake_mask(_this: Id, _cmd: Sel) -> u64 {
    u64::MAX
}

unsafe fn fake_class() -> Class {
    let name = c"StippleFakeDraggingInfo";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("draggingPasteboard"),
            fake_pasteboard as *const c_void,
            c"@@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("draggingLocation"),
            fake_location as *const c_void,
            c"{CGPoint=dd}@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("draggingSourceOperationMask"),
            fake_mask as *const c_void,
            c"Q@:".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

/// Drive a drop of the file at `path` onto the view at `at` (view points):
/// enter, two moves, perform. The events wait in the pending queue.
pub(super) unsafe fn self_drop(view: Id, path: &str, at: Point) -> String {
    unsafe {
        let pb = msg_id(class("NSPasteboard"), sel("pasteboardWithUniqueName"));
        if pb.is_null() {
            return "no pasteboard".into();
        }
        msg_id(pb, sel("clearContents"));
        let url = msg_id_id(class("NSURL"), sel("fileURLWithPath:"), nsstring(path));
        let abs = msg_id(url, sel("absoluteString"));
        let set: unsafe extern "C" fn(Id, Sel, Id, Id) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let wrote = set(pb, sel("setString:forType:"), abs, nsstring(FILE_URL)) != 0;
        let info = msg_id(msg_id(fake_class(), sel("alloc")), sel("init"));
        let to_window: unsafe extern "C" fn(Id, Sel, CgPoint, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let op: unsafe extern "C" fn(Id, Sel, Id) -> u64 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let flag: unsafe extern "C" fn(Id, Sel, Id) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let mut ops = Vec::new();
        for (i, dx) in [0.0, 4.0, 8.0].into_iter().enumerate() {
            let p = to_window(
                view,
                sel("convertPoint:toView:"),
                CgPoint {
                    x: at.x + dx,
                    y: at.y,
                },
                std::ptr::null_mut(),
            );
            FAKE.with(|f| f.set(Some((pb as usize, p))));
            let s = if i == 0 {
                "draggingEntered:"
            } else {
                "draggingUpdated:"
            };
            ops.push(op(view, sel(s), info));
        }
        let prepared = flag(view, sel("prepareForDragOperation:"), info) != 0;
        let performed = flag(view, sel("performDragOperation:"), info) != 0;
        FAKE.with(|f| f.set(None));
        format!("wrote={wrote} ops={ops:?} prepared={prepared} performed={performed}")
    }
}
