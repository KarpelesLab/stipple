//! An iPad's trackpad and mouse: scrolling and hovering.
//!
//! - **Scrolling**: a `UIPanGestureRecognizer` taking only indirect scroll
//!   input (`allowedScrollTypesMask` = continuous and discrete, iOS 13.4
//!   and later; `allowedTouchTypes` empty, so fingers on the screen never
//!   reach it and touches stay the app's). Two fingers on a trackpad, or a
//!   mouse's wheel, become [`Event::Scroll`] in points (see
//!   [`pan_scroll`](super::pan_scroll)), after an [`Event::PointerMoved`] to
//!   where the pointer is, so the container under it scrolls.
//! - **Hovering**: a `UIHoverGestureRecognizer` (iOS 13 and later), whose
//!   moves are [`Event::PointerMoved`] while no finger (or click) is down,
//!   and [`Event::PointerLeft`] when the pointer leaves the view or hides.
//!
//! Without a trackpad or mouse neither recognizer ever fires.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use super::pan_scroll::{self, PanScroll};
use super::{
    CTX, CgPoint, Class, Id, Sel, call_handler, class, class_addMethod, current_window, msg_id,
    objc_allocateClassPair, objc_getClass, objc_msgSend, objc_registerClassPair, sel,
};
use crate::geometry::Point;
use crate::platform::event::Event;

/// `UIScrollTypeMaskDiscrete | UIScrollTypeMaskContinuous`.
const SCROLL_TYPES_ALL: isize = 3;

thread_local! {
    static PAN: RefCell<PanScroll> = RefCell::default();
    /// The pointer hovers the view (the app was told where).
    static HOVERING: Cell<bool> = const { Cell::new(false) };
    /// The recognizers, for [`status`].
    static RECOGNIZERS: Cell<(Id, Id)> =
        const { Cell::new((std::ptr::null_mut(), std::ptr::null_mut())) };
}

/// Add the recognizers to `view`.
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn install(view: Id) {
    unsafe {
        let target = msg_id(msg_id(register_target(), sel("alloc")), sel("init"));
        let init: unsafe extern "C" fn(Id, Sel, Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let add: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let set_bool: unsafe extern "C" fn(Id, Sel, bool) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let responds: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let mut pan = std::ptr::null_mut();
        let candidate = init(
            msg_id(class("UIPanGestureRecognizer"), sel("alloc")),
            sel("initWithTarget:action:"),
            target,
            sel("stippleScroll:"),
        );
        if !candidate.is_null()
            && responds(
                candidate,
                sel("respondsToSelector:"),
                sel("setAllowedScrollTypesMask:"),
            ) != 0
        {
            let set_mask: unsafe extern "C" fn(Id, Sel, isize) =
                std::mem::transmute(objc_msgSend as *const c_void);
            set_mask(
                candidate,
                sel("setAllowedScrollTypesMask:"),
                SCROLL_TYPES_ALL,
            );
            // No touch types: fingers on the screen are the view's own.
            let none = msg_id(class("NSArray"), sel("array"));
            add(candidate, sel("setAllowedTouchTypes:"), none);
            set_bool(candidate, sel("setCancelsTouchesInView:"), false);
            set_bool(candidate, sel("setDelaysTouchesEnded:"), false);
            add(view, sel("addGestureRecognizer:"), candidate);
            pan = candidate;
        }
        let mut hover = std::ptr::null_mut();
        let hover_class = class("UIHoverGestureRecognizer");
        if !hover_class.is_null() {
            hover = init(
                msg_id(hover_class, sel("alloc")),
                sel("initWithTarget:action:"),
                target,
                sel("stippleHover:"),
            );
            if !hover.is_null() {
                add(view, sel("addGestureRecognizer:"), hover);
            }
        }
        RECOGNIZERS.with(|r| r.set((pan, hover)));
    }
}

fn register_target() -> Class {
    let name = c"StipplePointerTarget";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("stippleScroll:"),
            scroll_action as *const c_void,
            c"v@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("stippleHover:"),
            hover_action as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

/// A recognizer's state and a point it reports for the view.
unsafe fn read(recognizer: Id, point: &str) -> (isize, Point) {
    let view = super::app_view();
    unsafe {
        let state: unsafe extern "C" fn(Id, Sel) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        let at: unsafe extern "C" fn(Id, Sel, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = at(recognizer, sel(point), view);
        (state(recognizer, sel("state")), Point::new(p.x, p.y))
    }
}

extern "C" fn scroll_action(_this: Id, _cmd: Sel, recognizer: Id) {
    let (state, translation) = unsafe { read(recognizer, "translationInView:") };
    let (_, location) = unsafe { read(recognizer, "locationInView:") };
    scrolled(state, location, translation);
}

/// A scroll step: `translation` since the gesture began, the pointer at
/// `location` (both in points).
pub(super) fn scrolled(state: isize, location: Point, translation: Point) {
    let win = current_window();
    if state == pan_scroll::BEGAN {
        call_handler(Event::PointerMoved { position: location }, &win);
    }
    let delta = PAN.with(|p| p.borrow_mut().update(state, translation.x, translation.y));
    if let Some(delta) = delta {
        call_handler(Event::Scroll { delta }, &win);
    }
}

extern "C" fn hover_action(_this: Id, _cmd: Sel, recognizer: Id) {
    let (state, location) = unsafe { read(recognizer, "locationInView:") };
    hovered(state, location);
}

/// The pointer hovering at `location` (points), at recognizer `state`.
pub(super) fn hovered(state: isize, location: Point) {
    // A finger (or a click) down is the pointer already.
    if CTX.with(|c| c.borrow().primary.is_some()) {
        return;
    }
    let win = current_window();
    match state {
        pan_scroll::BEGAN | pan_scroll::CHANGED => {
            HOVERING.with(|h| h.set(true));
            call_handler(Event::PointerMoved { position: location }, &win);
        }
        pan_scroll::ENDED | pan_scroll::CANCELLED | pan_scroll::FAILED
            if HOVERING.with(|h| h.replace(false)) =>
        {
            call_handler(Event::PointerLeft, &win);
        }
        _ => {}
    }
}

/// For self-tests: the recognizers as UIKit has them, read back through
/// objc dispatch.
pub(super) fn status() -> String {
    let (pan, hover) = RECOGNIZERS.with(|r| r.get());
    unsafe {
        let int: unsafe extern "C" fn(Id, Sel) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        let (mask, touch_types) = if pan.is_null() {
            (-1, -1)
        } else {
            let types = msg_id(pan, sel("allowedTouchTypes"));
            (
                int(pan, sel("allowedScrollTypesMask")),
                if types.is_null() {
                    -1
                } else {
                    int(types, sel("count"))
                },
            )
        };
        let attached = |r: Id| {
            !r.is_null() && {
                let view = msg_id(r, sel("view"));
                view == super::app_view()
            }
        };
        format!(
            "scroll_pan={} scroll_types={mask} touch_types={touch_types} hover={}",
            attached(pan),
            attached(hover),
        )
    }
}
