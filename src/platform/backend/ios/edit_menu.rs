//! The system's edit menu over a text field's selection, on iOS 16 and
//! later: a `UIEditMenuInteraction` added to the view, presented at the
//! selection the App asks for (`Window::show_edit_menu`).
//!
//! The menu's items are UIKit's standard edit actions: without a
//! `menuForConfiguration:` in the delegate, the interaction offers the
//! suggested ones the responder chain accepts, asked through the view's
//! `canPerformAction:withSender:`. The view (first responder while a field
//! has focus) says yes to `cut:`, `copy:`, `paste:` and `selectAll:` as the
//! App's menu offers them while it shows, and its implementations of those
//! hand the choice to the App as [`Event::EditAction`]. The delegate's
//! `editMenuInteraction:targetRectForConfiguration:` gives the selection's
//! rectangle, so the menu sits beside it rather than over it.
//!
//! Before iOS 16 (no `UIEditMenuInteraction` class) the App draws its own
//! menu.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use super::{
    CTX, CgPoint, CgRect, CgSize, Class, Id, ObjcSuper, Sel, call_handler, class, class_addMethod,
    current_window, later, msg_id, msg_void, msg_void_id, objc_allocateClassPair, objc_getClass,
    objc_msgSend, objc_msgSendSuper, objc_registerClassPair, sel,
};
use crate::platform::event::Event;
use crate::platform::{EditAction, EditMenu};

thread_local! {
    /// The interaction (retained) and its delegate, once made.
    static INTERACTION: Cell<(Id, Id)> = const { Cell::new((std::ptr::null_mut(), std::ptr::null_mut())) };
    /// The menu the App asked for, while it shows.
    static MENU: RefCell<Option<EditMenu>> = const { RefCell::new(None) };
}

/// The standard edit action each selector stands for.
fn action_of(selector: Sel) -> Option<EditAction> {
    [
        ("cut:", EditAction::Cut),
        ("copy:", EditAction::Copy),
        ("paste:", EditAction::Paste),
        ("selectAll:", EditAction::SelectAll),
    ]
    .into_iter()
    .find(|(name, _)| sel(name) == selector)
    .map(|(_, a)| a)
}

/// Whether the system has the edit menu (iOS 16 and later).
pub(super) fn available() -> bool {
    unsafe { !objc_getClass(c"UIEditMenuInteraction".as_ptr()).is_null() }
}

/// Add the standard edit actions to the view class (before it's
/// registered).
///
/// # Safety
/// `cls` must be a class pair still being built.
pub(super) unsafe fn add_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("canPerformAction:withSender:"),
            can_perform_action as *const c_void,
            c"B@::@".as_ptr(),
        );
        for name in ["cut:", "copy:", "paste:", "selectAll:"] {
            class_addMethod(cls, sel(name), perform as *const c_void, c"v@:@".as_ptr());
        }
    }
}

/// `canPerformAction:withSender:`: the edit actions the App's menu offers
/// while it shows; anything else as `UIView` answers.
extern "C" fn can_perform_action(this: Id, cmd: Sel, action: Sel, sender: Id) -> bool {
    if let Some(a) = action_of(action) {
        return MENU.with(|m| m.borrow().is_some_and(|m| m.offers(a)));
    }
    unsafe {
        let sup = ObjcSuper {
            receiver: this,
            super_class: class("UIView"),
        };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel, Sel, Id) -> bool =
            std::mem::transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd, action, sender)
    }
}

/// `cut:`, `copy:`, `paste:` and `selectAll:`: the user picked the item;
/// the App applies it, after UIKit is done with the menu.
extern "C" fn perform(_this: Id, cmd: Sel, _sender: Id) {
    let Some(action) = action_of(cmd) else {
        return;
    };
    crate::platform::window::debug_window(format_args!("edit menu: {action:?}"));
    MENU.with(|m| *m.borrow_mut() = None);
    later(move || call_handler(Event::EditAction(action), &current_window()));
}

/// `editMenuInteraction:targetRectForConfiguration:`: the selection the
/// menu is for, in the view's coordinates.
extern "C" fn target_rect(_this: Id, _cmd: Sel, _interaction: Id, _config: Id) -> CgRect {
    let r = MENU
        .with(|m| m.borrow().map(|m| m.rect))
        .unwrap_or_default();
    CgRect {
        origin: CgPoint {
            x: r.min_x(),
            y: r.min_y(),
        },
        size: CgSize {
            width: r.width(),
            height: r.height(),
        },
    }
}

/// The delegate class, made once.
fn delegate_class() -> Class {
    let name = c"StippleEditMenuDelegate";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("editMenuInteraction:targetRectForConfiguration:"),
            target_rect as *const c_void,
            c"{CGRect={CGPoint=dd}{CGSize=dd}}@:@@".as_ptr(),
        );
        let protocol = super::objc_getProtocol(c"UIEditMenuInteractionDelegate".as_ptr());
        if !protocol.is_null() {
            super::class_addProtocol(cls, protocol);
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// The view's interaction, added the first time.
unsafe fn interaction(view: Id) -> Id {
    let (have, _) = INTERACTION.with(Cell::get);
    if !have.is_null() {
        return have;
    }
    unsafe {
        let delegate = msg_id(msg_id(delegate_class(), sel("alloc")), sel("init"));
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let made = init(
            msg_id(class("UIEditMenuInteraction"), sel("alloc")),
            sel("initWithDelegate:"),
            delegate,
        );
        if made.is_null() {
            return made;
        }
        msg_void_id(view, sel("addInteraction:"), made);
        INTERACTION.with(|i| i.set((made, delegate)));
        made
    }
}

/// Present `menu` at its rectangle (replacing one shown), or dismiss the
/// menu (`None`).
pub(super) fn show(menu: Option<&EditMenu>) {
    let view = CTX.with(|c| c.borrow().view);
    if view.is_null() || !available() {
        return;
    }
    MENU.with(|m| *m.borrow_mut() = menu.copied());
    crate::platform::window::debug_window(format_args!("edit menu: show {menu:?}"));
    unsafe {
        let interaction = interaction(view);
        if interaction.is_null() {
            return;
        }
        let Some(menu) = menu else {
            msg_void(interaction, sel("dismissMenu"));
            return;
        };
        let at = CgPoint {
            x: menu.rect.center().x,
            y: menu.rect.min_y(),
        };
        let make: unsafe extern "C" fn(Id, Sel, Id, CgPoint) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let config = make(
            class("UIEditMenuConfiguration"),
            sel("configurationWithIdentifier:sourcePoint:"),
            std::ptr::null_mut(),
            at,
        );
        if !config.is_null() {
            msg_void_id(
                interaction,
                sel("presentEditMenuWithConfiguration:"),
                config,
            );
        }
    }
}

/// The edit menu's state as the system reads it, for self-tests: whether
/// the interaction exists, the menu asked for, and the view's answers to
/// `canPerformAction:withSender:` for the four actions.
pub(super) fn status() -> String {
    let view = CTX.with(|c| c.borrow().view);
    if view.is_null() {
        return "no view".into();
    }
    let (interaction, _) = INTERACTION.with(Cell::get);
    let mut out = format!(
        "available={} interaction={} shown={}",
        available(),
        !interaction.is_null(),
        MENU.with(|m| m.borrow().is_some())
    );
    for (name, short) in [
        ("cut:", "cut"),
        ("copy:", "copy"),
        ("paste:", "paste"),
        ("selectAll:", "select_all"),
    ] {
        let can = unsafe {
            let f: unsafe extern "C" fn(Id, Sel, Sel, Id) -> bool =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(
                view,
                sel("canPerformAction:withSender:"),
                sel(name),
                std::ptr::null_mut(),
            )
        };
        out.push_str(&format!(" {short}={can}"));
    }
    out
}

/// Send the view `action`'s message (`copy:`…), as picking the item in the
/// system's menu does.
pub(super) fn perform_action(action: EditAction) {
    let view = CTX.with(|c| c.borrow().view);
    if view.is_null() {
        return;
    }
    let name = match action {
        EditAction::Cut => "cut:",
        EditAction::Copy => "copy:",
        EditAction::Paste => "paste:",
        EditAction::SelectAll => "selectAll:",
    };
    unsafe { msg_void_id(view, sel(name), std::ptr::null_mut()) };
}
