//! Dragging out of the app on macOS: an `NSDraggingSession` begun on the
//! view, which is its own `NSDraggingSource`.
//!
//! When the app hands a drag to the window ([`Window::start_drag`]), the
//! event loop begins the session with the mouse-dragged event it is
//! handling (`beginDraggingSessionWithItems:event:source:`): one
//! `NSDraggingItem` per file, written as its `NSURL` and shown with the
//! file's Finder icon, or one for text, written as the `NSString`. AppKit
//! then runs the drag (pointer, cursor, Escape, the slide back on a
//! cancel) and asks the source which operations it allows: copy.
//!
//! [`Window::start_drag`]: crate::platform::Window::start_drag

use std::ffi::c_void;

use super::super::{
    CgPoint, CgRect, CgSize, Class, Id, Sel, class, class_addMethod, msg_id, msg_id_id, msg_void,
    msg_void_id, nsstring, objc_msgSend, sel,
};
use super::DRAG_OPERATION_COPY;
use crate::platform::DropData;

/// The size of the image under the pointer, in points.
const ICON: f64 = 32.0;

/// Add the `NSDraggingSource` methods to the view class.
pub(in super::super) unsafe fn add_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("draggingSession:sourceOperationMaskForDraggingContext:"),
            operation_mask as *const c_void,
            c"Q@:@q".as_ptr(),
        );
    }
}

/// Inside or outside the app, the drag offers a copy.
extern "C" fn operation_mask(_this: Id, _cmd: Sel, _session: Id, _context: i64) -> u64 {
    DRAG_OPERATION_COPY
}

/// Begin dragging `data` out of `view`, from the mouse-dragged `event`.
/// Returns whether AppKit started a session.
pub(in super::super) unsafe fn begin(view: Id, event: Id, data: &DropData) -> bool {
    unsafe {
        let point: unsafe extern "C" fn(Id, Sel) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let convert: unsafe extern "C" fn(Id, Sel, CgPoint, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let set_frame: unsafe extern "C" fn(Id, Sel, CgRect, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let begin: unsafe extern "C" fn(Id, Sel, Id, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);

        let in_window = point(event, sel("locationInWindow"));
        let at = convert(
            view,
            sel("convertPoint:fromView:"),
            in_window,
            std::ptr::null_mut(),
        );
        let workspace = msg_id(class("NSWorkspace"), sel("sharedWorkspace"));
        // (writer, image) per item.
        let mut writers: Vec<(Id, Id)> = Vec::new();
        match data {
            DropData::Files(paths) => {
                for path in paths {
                    let path = nsstring(&path.to_string_lossy());
                    let url = msg_id_id(class("NSURL"), sel("fileURLWithPath:"), path);
                    let icon = msg_id_id(workspace, sel("iconForFile:"), path);
                    if !url.is_null() {
                        writers.push((url, icon));
                    }
                }
            }
            DropData::Text(text) => {
                let icon = msg_id_id(workspace, sel("iconForFileType:"), nsstring("txt"));
                writers.push((nsstring(text), icon));
            }
            DropData::Internal(_) => {}
        }
        if writers.is_empty() {
            return false;
        }
        let items = msg_id(class("NSMutableArray"), sel("array"));
        for (i, (writer, icon)) in writers.into_iter().enumerate() {
            let item = msg_id_id(
                msg_id(class("NSDraggingItem"), sel("alloc")),
                sel("initWithPasteboardWriter:"),
                writer,
            );
            if item.is_null() {
                continue;
            }
            // Centred on the pointer, each further file a little offset.
            let step = 4.0 * i.min(4) as f64;
            let frame = CgRect {
                origin: CgPoint {
                    x: at.x - ICON / 2.0 + step,
                    y: at.y - ICON / 2.0 + step,
                },
                size: CgSize {
                    width: ICON,
                    height: ICON,
                },
            };
            set_frame(item, sel("setDraggingFrame:contents:"), frame, icon);
            msg_void_id(items, sel("addObject:"), item);
            msg_void(item, sel("release"));
        }
        let session = begin(
            view,
            sel("beginDraggingSessionWithItems:event:source:"),
            items,
            event,
            view,
        );
        !session.is_null()
    }
}
