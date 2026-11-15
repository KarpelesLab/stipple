//! The screen the app fills: its size and safe area as the device turns,
//! which ways it may turn, the status bar's look, and the idle timer.
//!
//! The window's root view controller is a `StippleViewController` built at
//! run time. UIKit lays its view out again whenever its bounds or safe area
//! change: a rotation (inside the rotation's animation), iPad split view or
//! Stage Manager, the status bar showing or hiding. `viewDidLayoutSubviews`
//! and `viewSafeAreaInsetsDidChange` then place the app's view (the safe
//! area, or the whole screen when the app draws edge to edge) and tell the
//! app: [`Event::Resized`] and a redraw when the size changed, else
//! [`Event::WindowChanged`] when only the safe area did.
//!
//! `supportedInterfaceOrientations` answers the app's
//! [`Orientations`](crate::platform::Orientations) (UIKit's own default
//! until it sets some), and `preferredStatusBarStyle` dark or light icons
//! from the color the app draws at the top-left corner.

use std::ffi::c_void;

use super::{
    CTX, CgRect, CgSize, Class, Id, IosWindow, ObjcSuper, Sel, call_handler, class,
    class_addMethod, msg_id, msg_insets, msg_rect, msg_void, msg_void_bool, msg_void_rect,
    objc_allocateClassPair, objc_getClass, objc_msgSend, objc_msgSendSuper, objc_registerClassPair,
    sel,
};
use crate::geometry::{Insets, PhysicalSize};
use crate::platform::Orientations;
use crate::platform::event::Event;

/// Call the `UIViewController` implementation of the `void` method `cmd`.
unsafe fn super_void(this: Id, cmd: Sel) {
    unsafe {
        let sup = ObjcSuper {
            receiver: this,
            super_class: class("UIViewController"),
        };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel) =
            std::mem::transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd);
    }
}

extern "C" fn view_did_layout_subviews(this: Id, cmd: Sel) {
    unsafe { super_void(this, cmd) };
    relayout();
}

extern "C" fn view_safe_area_insets_did_change(this: Id, cmd: Sel) {
    unsafe { super_void(this, cmd) };
    relayout();
}

/// `supportedInterfaceOrientations`: the app's, or UIKit's default.
extern "C" fn supported_orientations(this: Id, cmd: Sel) -> u64 {
    let mask = CTX.with(|c| c.borrow().orientations);
    if mask != 0 {
        return mask;
    }
    unsafe {
        let sup = ObjcSuper {
            receiver: this,
            super_class: class("UIViewController"),
        };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel) -> u64 =
            std::mem::transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd)
    }
}

/// `preferredStatusBarStyle`: dark icons over a light top edge
/// (`UIStatusBarStyleDarkContent`, 3), light ones over a dark one
/// (`LightContent`, 1); the default (0) until a frame was drawn.
extern "C" fn status_bar_style(_this: Id, _cmd: Sel) -> i64 {
    match CTX.with(|c| c.borrow().top_light) {
        Some(true) => 3,
        Some(false) => 1,
        None => 0,
    }
}

/// The root view controller's class.
pub(super) fn register_controller_class() -> Class {
    let name = c"StippleViewController";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("UIViewController"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("viewDidLayoutSubviews"),
            view_did_layout_subviews as *const c_void,
            c"v@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("viewSafeAreaInsetsDidChange"),
            view_safe_area_insets_did_change as *const c_void,
            c"v@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("supportedInterfaceOrientations"),
            supported_orientations as *const c_void,
            c"Q@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("preferredStatusBarStyle"),
            status_bar_style as *const c_void,
            c"q@:".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

/// Place the app's view in its root view (the safe area, or all of it edge
/// to edge) and tell the app what changed. The first call always reports
/// the size.
pub(super) fn relayout() {
    let (view, edge, scale) = CTX.with(|c| {
        let c = c.borrow();
        (c.view, c.edge_to_edge, c.scale)
    });
    if view.is_null() {
        return;
    }
    let (frame, safe) = unsafe {
        let root = msg_id(view, sel("superview"));
        if root.is_null() {
            return;
        }
        let bounds = msg_rect(root, sel("bounds"));
        let safe = msg_insets(root, sel("safeAreaInsets"));
        let frame = if edge {
            bounds
        } else {
            CgRect {
                origin: super::CgPoint {
                    x: bounds.origin.x + safe.left,
                    y: bounds.origin.y + safe.top,
                },
                size: CgSize {
                    width: (bounds.size.width - safe.left - safe.right).max(0.0),
                    height: (bounds.size.height - safe.top - safe.bottom).max(0.0),
                },
            }
        };
        let old = msg_rect(view, sel("frame"));
        if (old.origin.x, old.origin.y, old.size.width, old.size.height)
            != (
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            )
        {
            msg_void_rect(view, sel("setFrame:"), frame);
        }
        (frame, safe)
    };
    let size = PhysicalSize::new(
        ((frame.size.width * scale).round() as u32).max(1),
        ((frame.size.height * scale).round() as u32).max(1),
    );
    let insets = if edge {
        Insets::new(safe.top, safe.right, safe.bottom, safe.left)
    } else {
        Insets::ZERO
    };
    let (resized, moved_in) = CTX.with(|c| {
        let mut c = c.borrow_mut();
        let first = !std::mem::replace(&mut c.laid_out, true);
        let resized = first || std::mem::replace(&mut c.size, size) != size;
        c.size = size;
        let moved_in = std::mem::replace(&mut c.safe_area, insets) != insets;
        (resized, moved_in)
    });
    let win = IosWindow { size };
    if resized {
        call_handler(Event::Resized(size), &win);
        // Drawn now, so the new frame is in the rotation's animation.
        call_handler(Event::RedrawRequested, &win);
    } else if moved_in {
        call_handler(Event::WindowChanged, &win);
    }
}

/// The top edge of a new frame is light (`true`) or dark: the status bar's
/// icons go the other way.
pub(super) fn set_top_color(light: bool) {
    let (changed, window) = CTX.with(|c| {
        let mut c = c.borrow_mut();
        (c.top_light.replace(light) != Some(light), c.window)
    });
    if changed && !window.is_null() {
        unsafe {
            let vc = msg_id(window, sel("rootViewController"));
            if !vc.is_null() {
                msg_void(vc, sel("setNeedsStatusBarAppearanceUpdate"));
            }
        }
    }
}

/// `[obj respondsToSelector:s]`.
unsafe fn responds(obj: Id, s: Sel) -> bool {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, sel("respondsToSelector:"), s) != 0
    }
}

/// Let the device turn the app only the ways in `set`, turning it now if it
/// stands a way that isn't.
pub(super) fn set_orientations(set: Orientations) {
    let mask = set.ios_mask();
    let window = CTX.with(|c| {
        let mut c = c.borrow_mut();
        c.orientations = mask;
        c.window
    });
    if window.is_null() {
        return;
    }
    unsafe {
        let vc = msg_id(window, sel("rootViewController"));
        if vc.is_null() {
            return;
        }
        let update = sel("setNeedsUpdateOfSupportedInterfaceOrientations");
        if !responds(vc, update) {
            // Before iOS 16.
            msg_void(
                class("UIViewController"),
                sel("attemptRotationToDeviceOrientation"),
            );
            return;
        }
        msg_void(vc, update);
        // And turn to one of them if the app stands another way.
        let scene = msg_id(window, sel("windowScene"));
        let prefs_class = class("UIWindowSceneGeometryPreferencesIOS");
        if scene.is_null() || prefs_class.is_null() {
            return;
        }
        let init: unsafe extern "C" fn(Id, Sel, u64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let prefs = init(
            msg_id(prefs_class, sel("alloc")),
            sel("initWithInterfaceOrientations:"),
            mask,
        );
        if prefs.is_null() {
            return;
        }
        let request: unsafe extern "C" fn(Id, Sel, Id, *mut c_void) =
            std::mem::transmute(objc_msgSend as *const c_void);
        request(
            scene,
            sel("requestGeometryUpdateWithPreferences:errorHandler:"),
            prefs,
            std::ptr::null_mut(),
        );
        msg_void(prefs, sel("release"));
    }
}

/// Keep the screen awake (`true`) while the app is in the foreground: the
/// application's idle timer. iOS ignores it in the background.
pub(super) fn set_keep_awake(on: bool) {
    unsafe {
        let app = msg_id(class("UIApplication"), sel("sharedApplication"));
        if !app.is_null() {
            msg_void_bool(app, sel("setIdleTimerDisabled:"), on);
        }
    }
}

/// For the self-test: the app's view's frame in its root view, the root
/// view's safe area and whether the idle timer is off.
pub(super) fn status() -> String {
    let view = CTX.with(|c| c.borrow().view);
    if view.is_null() {
        return "no view".into();
    }
    unsafe {
        let f = msg_rect(view, sel("frame"));
        let root = msg_id(view, sel("superview"));
        let s = msg_insets(root, sel("safeAreaInsets"));
        let app = msg_id(class("UIApplication"), sel("sharedApplication"));
        let idle_off = super::msg_bool(app, sel("isIdleTimerDisabled"));
        format!(
            "frame {}x{} at ({},{}) safe area {} {} {} {} idle timer off {idle_off}",
            f.size.width, f.size.height, f.origin.x, f.origin.y, s.top, s.right, s.bottom, s.left
        )
    }
}
