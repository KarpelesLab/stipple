//! macOS message boxes: `NSAlert`, as a sheet on the key window with a
//! completion handler (a block built by hand), so the event loop keeps
//! running; with no window to hang it on, `runModal`.

use super::{MessageBox, MessageDone, MessageKind, MessageResponse};
use crate::platform::native::objc::{CgRect, Id, class, msg, nsstring, rust_string, with_pool};
use std::cell::RefCell;
use std::ffi::c_void;

/// `NSAlertFirstButtonReturn`: the first button's response; the n-th is
/// this plus n.
const FIRST_BUTTON: isize = 1000;
// `NSAlertStyle`.
const STYLE_WARNING: usize = 0;
const STYLE_INFORMATIONAL: usize = 1;
const STYLE_CRITICAL: usize = 2;

unsafe extern "C" {
    /// The isa of a block that lives forever (libSystem's libclosure).
    static _NSConcreteGlobalBlock: c_void;
}

/// `BLOCK_IS_GLOBAL`: copies and releases of the block are no-ops.
const BLOCK_IS_GLOBAL: i32 = 1 << 28;

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A block literal taking an `NSModalResponse`, carrying the alert's id.
#[repr(C)]
struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut Block, isize),
    descriptor: *const BlockDescriptor,
    id: u64,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<Block>(),
};

/// An alert waiting for its answer.
struct Pending {
    id: u64,
    alert: Id,
    field: Id,
    buttons: usize,
    done: MessageDone,
}

thread_local! {
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// The completion handler: answer the alert `block` names.
unsafe extern "C" fn completed(block: *mut Block, response: isize) {
    let id = unsafe { (*block).id };
    finish(id, response);
}

fn finish(id: u64, response: isize) {
    let Some(p) = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let i = p.iter().position(|x| x.id == id)?;
        Some(p.remove(i))
    }) else {
        return;
    };
    let i = response - FIRST_BUTTON;
    let button = (0..p.buttons as isize).contains(&i).then_some(i as usize);
    let input = (!p.field.is_null())
        .then(|| unsafe { rust_string(msg!(p.field, "stringValue", () -> Id)) })
        .flatten();
    super::debug(format_args!(
        "message box: NSAlert answered {response} -> {button:?}"
    ));
    unsafe { msg!(p.alert, "release", () -> ()) };
    (p.done)(MessageResponse { button, input });
}

pub(super) fn show(mb: &MessageBox, done: MessageDone) -> Result<(), MessageDone> {
    let id = NEXT.with(|n| n.replace(n.get() + 1));
    let labels = mb.button_labels();
    let (alert, field, window) = with_pool(|| unsafe {
        let app = msg!(class("NSApplication"), "sharedApplication", () -> Id);
        let alert = msg!(class("NSAlert"), "alloc", () -> Id);
        let alert = msg!(alert, "init", () -> Id);
        let text = nsstring(&mb.text);
        msg!(alert, "setMessageText:", (text: Id) -> ());
        if !mb.detail.is_empty() {
            let detail = nsstring(&mb.detail);
            msg!(alert, "setInformativeText:", (detail: Id) -> ());
        }
        let style = match mb.kind {
            MessageKind::Info | MessageKind::Question => STYLE_INFORMATIONAL,
            MessageKind::Warning => STYLE_WARNING,
            MessageKind::Error => STYLE_CRITICAL,
        };
        msg!(alert, "setAlertStyle:", (style: usize) -> ());
        let mut buttons = Vec::new();
        for label in &labels {
            let title = nsstring(label);
            buttons.push(msg!(alert, "addButtonWithTitle:", (title: Id) -> Id));
        }
        // NSAlert makes the first button the default (Return); move that,
        // and give the cancel button Escape.
        let empty = nsstring("");
        let default = mb.default_index();
        if default != 0 {
            msg!(buttons[0], "setKeyEquivalent:", (empty: Id) -> ());
            let ret = nsstring("\r");
            msg!(buttons[default], "setKeyEquivalent:", (ret: Id) -> ());
        }
        if let Some(c) = mb.cancel_index().filter(|&c| c != default) {
            let esc = nsstring("\u{1b}");
            msg!(buttons[c], "setKeyEquivalent:", (esc: Id) -> ());
        }
        let field = match &mb.input {
            Some(input) => {
                let cls = if input.secure {
                    "NSSecureTextField"
                } else {
                    "NSTextField"
                };
                let f = msg!(class(cls), "alloc", () -> Id);
                let frame = CgRect {
                    x: 0.0,
                    y: 0.0,
                    width: 260.0,
                    height: 24.0,
                };
                let f = msg!(f, "initWithFrame:", (frame: CgRect) -> Id);
                let value = nsstring(&input.value);
                msg!(f, "setStringValue:", (value: Id) -> ());
                msg!(alert, "setAccessoryView:", (f: Id) -> ());
                msg!(f, "release", () -> ());
                msg!(alert, "layout", () -> ());
                let w = msg!(alert, "window", () -> Id);
                msg!(w, "setInitialFirstResponder:", (f: Id) -> ());
                f
            }
            None => std::ptr::null_mut(),
        };
        let mut window = msg!(app, "keyWindow", () -> Id);
        if window.is_null() {
            window = msg!(app, "mainWindow", () -> Id);
        }
        if window.is_null() {
            // An app that isn't active has no key or main window; any
            // visible one of its own will do.
            let windows = msg!(app, "windows", () -> Id);
            let count = msg!(windows, "count", () -> usize);
            for i in 0..count {
                let w = msg!(windows, "objectAtIndex:", (i: usize) -> Id);
                if msg!(w, "isVisible", () -> bool) {
                    window = w;
                    break;
                }
            }
        }
        (alert, field, window)
    });
    PENDING.with(|p| {
        p.borrow_mut().push(Pending {
            id,
            alert,
            field,
            buttons: labels.len(),
            done,
        })
    });
    if window.is_null() {
        // No window for a sheet: a modal alert (this blocks until answered).
        super::debug(format_args!("message box: NSAlert runModal"));
        let response = with_pool(|| unsafe { msg!(alert, "runModal", () -> isize) });
        finish(id, response);
        return Ok(());
    }
    // A block that is never copied or freed: AppKit may hold on to it, and
    // it's a few words per alert.
    let block = Box::leak(Box::new(Block {
        isa: &raw const _NSConcreteGlobalBlock,
        flags: BLOCK_IS_GLOBAL,
        reserved: 0,
        invoke: completed,
        descriptor: &DESCRIPTOR,
        id,
    }));
    let block: *mut Block = block;
    super::debug(format_args!("message box: NSAlert sheet"));
    with_pool(|| unsafe {
        msg!(alert, "beginSheetModalForWindow:completionHandler:",
            (window: Id, block: *mut Block) -> ());
    });
    Ok(())
}

pub(super) fn press(index: usize) -> bool {
    let Some(alert) = PENDING.with(|p| p.borrow().last().map(|p| p.alert)) else {
        return false;
    };
    with_pool(|| unsafe {
        let buttons = msg!(alert, "buttons", () -> Id);
        let count = msg!(buttons, "count", () -> usize);
        if index >= count {
            return false;
        }
        let button = msg!(buttons, "objectAtIndex:", (index: usize) -> Id);
        let none: Id = std::ptr::null_mut();
        msg!(button, "performClick:", (none: Id) -> ());
        true
    })
}
