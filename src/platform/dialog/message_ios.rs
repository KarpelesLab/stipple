//! iOS message boxes: a `UIAlertController` presented from the key window's
//! root view controller, each button an action whose handler (a block built
//! by hand) answers.

use super::{MessageBox, MessageDone, MessageResponse};
use crate::platform::native::objc::{Id, class, msg, nsstring, rust_string, with_pool};
use std::cell::RefCell;
use std::ffi::c_void;

/// `UIAlertControllerStyleAlert`.
const STYLE_ALERT: isize = 1;
// `UIAlertActionStyle`.
const ACTION_DEFAULT: isize = 0;
const ACTION_CANCEL: isize = 1;

unsafe extern "C" {
    static _NSConcreteGlobalBlock: c_void;
}

const BLOCK_IS_GLOBAL: i32 = 1 << 28;

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A block literal taking a `UIAlertAction *`, carrying the alert's id and
/// the button's index.
#[repr(C)]
struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut Block, Id),
    descriptor: *const BlockDescriptor,
    id: u64,
    index: usize,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<Block>(),
};

struct Pending {
    id: u64,
    controller: Id,
    done: MessageDone,
}

thread_local! {
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

unsafe extern "C" fn tapped(block: *mut Block, _action: Id) {
    let (id, index) = unsafe { ((*block).id, (*block).index) };
    let Some(p) = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let i = p.iter().position(|x| x.id == id)?;
        Some(p.remove(i))
    }) else {
        return;
    };
    let input = unsafe {
        let fields = msg!(p.controller, "textFields", () -> Id);
        let field = if fields.is_null() {
            std::ptr::null_mut()
        } else {
            msg!(fields, "firstObject", () -> Id)
        };
        (!field.is_null())
            .then(|| rust_string(msg!(field, "text", () -> Id)))
            .flatten()
    };
    unsafe { msg!(p.controller, "release", () -> ()) };
    (p.done)(MessageResponse {
        button: Some(index),
        input,
    });
}

pub(super) fn show(mb: &MessageBox, done: MessageDone) -> Result<(), MessageDone> {
    let root = with_pool(|| unsafe {
        let app = msg!(class("UIApplication"), "sharedApplication", () -> Id);
        let window = msg!(app, "keyWindow", () -> Id);
        if window.is_null() {
            std::ptr::null_mut()
        } else {
            msg!(window, "rootViewController", () -> Id)
        }
    });
    if root.is_null() {
        return Err(done);
    }
    let id = NEXT.with(|n| n.replace(n.get() + 1));
    let labels = mb.button_labels();
    let cancel = mb.cancel_index();
    let controller = with_pool(|| unsafe {
        let title = nsstring(&mb.text);
        let message: Id = if mb.detail.is_empty() {
            std::ptr::null_mut()
        } else {
            nsstring(&mb.detail)
        };
        let style = STYLE_ALERT;
        let ac = msg!(class("UIAlertController"), "alertControllerWithTitle:message:preferredStyle:",
            (title: Id, message: Id, style: isize) -> Id);
        let ac = msg!(ac, "retain", () -> Id);
        for (index, label) in labels.iter().enumerate() {
            let block = Box::leak(Box::new(Block {
                isa: &raw const _NSConcreteGlobalBlock,
                flags: BLOCK_IS_GLOBAL,
                reserved: 0,
                invoke: tapped,
                descriptor: &DESCRIPTOR,
                id,
                index,
            }));
            let block: *mut Block = block;
            let title = nsstring(label);
            let style = if cancel == Some(index) && labels.len() > 1 {
                ACTION_CANCEL
            } else {
                ACTION_DEFAULT
            };
            let action = msg!(class("UIAlertAction"), "actionWithTitle:style:handler:",
                (title: Id, style: isize, block: *mut Block) -> Id);
            msg!(ac, "addAction:", (action: Id) -> ());
            if index == mb.default_index() {
                msg!(ac, "setPreferredAction:", (action: Id) -> ());
            }
        }
        if let Some(input) = &mb.input {
            let none: Id = std::ptr::null_mut();
            msg!(ac, "addTextFieldWithConfigurationHandler:", (none: Id) -> ());
            let fields = msg!(ac, "textFields", () -> Id);
            let field = msg!(fields, "firstObject", () -> Id);
            let text = nsstring(&input.value);
            msg!(field, "setText:", (text: Id) -> ());
            let secure = input.secure;
            msg!(field, "setSecureTextEntry:", (secure: bool) -> ());
        }
        let animated = true;
        let none: Id = std::ptr::null_mut();
        msg!(root, "presentViewController:animated:completion:",
            (ac: Id, animated: bool, none: Id) -> ());
        ac
    });
    PENDING.with(|p| {
        p.borrow_mut().push(Pending {
            id,
            controller,
            done,
        })
    });
    Ok(())
}

pub(super) fn press(_index: usize) -> bool {
    // UIKit has no public way to trigger an alert action.
    false
}
