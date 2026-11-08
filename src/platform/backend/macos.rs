//! A native macOS backend over the raw Objective-C runtime + Cocoa/CoreGraphics.
//!
//! No `objc`/`cocoa`/`core-graphics` crate — direct `objc_msgSend` FFI and
//! framework links, so the only "dependency" is the OS (workspace policy in
//! `ROADMAP.md` §1). This module opts into `unsafe` for the crate.
//!
//! An `NSWindow` hosts a custom `NSView` whose `drawRect:` blits the software
//! [`Pixmap`] as a `CGImage`. A manual `nextEventMatchingMask:` loop translates
//! `NSEvent`s into Stipple pointer/keyboard events (y-flipped to top-left origin)
//! and polls the view bounds for live resize.
//!
//! **Verification:** build-checked on the `macos-latest` CI runner via the
//! build matrix; runtime-screenshotted by the Visual workflow's macOS job
//! (`screencapture`).
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::{CString, c_void};

use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, KeyCode, PointerButton, WindowId};
use crate::platform::input;
use crate::platform::menu::{NativeMenuBar, NativeMenuItem, mnemonic};
use crate::platform::window::{CursorIcon, Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

/// Pacing animation frames by the display (`CVDisplayLink`).
mod display_link;
mod text_input;

// ---- Objective-C runtime + framework FFI ------------------------------------

type Id = *mut c_void;
type Sel = *mut c_void;
type Class = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const i8) -> Class;
    fn sel_registerName(name: *const i8) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(superclass: Class, name: *const i8, extra: usize) -> Class;
    fn objc_registerClassPair(cls: Class);
    fn class_addMethod(cls: Class, name: Sel, imp: *const c_void, types: *const i8) -> bool;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *mut c_void;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithCFData(data: *mut c_void) -> *mut c_void;
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: *const c_void,
    ) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
    fn CGContextDrawImage(ctx: *mut c_void, rect: CgRect, image: *mut c_void);
    fn CGContextSaveGState(ctx: *mut c_void);
    fn CGContextRestoreGState(ctx: *mut c_void);
    fn CGContextTranslateCTM(ctx: *mut c_void, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut c_void, sx: f64, sy: f64);
}

const NS_WINDOW_STYLE_TITLED: u64 = 1;
const NS_WINDOW_STYLE_CLOSABLE: u64 = 2;
const NS_WINDOW_STYLE_RESIZABLE: u64 = 8;
const NS_BACKING_BUFFERED: u64 = 2;
/// `NSWindowStyleMaskFullScreen`: set while the window is fullscreen.
const NS_WINDOW_STYLE_FULLSCREEN: u64 = 1 << 14;
/// `NSWindowCollectionBehaviorFullScreenPrimary`: the window can go fullscreen.
const NS_COLLECTION_FULLSCREEN_PRIMARY: u64 = 1 << 7;
// kCGImageAlphaLast | kCGBitmapByteOrder32Big — interprets the buffer bytes as
// R,G,B,A in memory with *straight* (non-premultiplied) alpha, matching the
// stipple::render Pixmap convention (see `crate::render::surface`).
const CG_BITMAP_RGBA8: u32 = 3 | (4 << 12);
const ACTIVATION_REGULAR: i64 = 0;

fn class(name: &str) -> Class {
    let c = CString::new(name).unwrap();
    unsafe { objc_getClass(c.as_ptr()) }
}

fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}

// Typed `objc_msgSend` shims. `objc_msgSend` is variadic in C; we transmute it
// to the exact signature of each call site (the documented way to use it).
unsafe fn msg_id(obj: Id, s: Sel) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_void(obj: Id, s: Sel) {
    let f: unsafe extern "C" fn(Id, Sel) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_f64(obj: Id, s: Sel) -> f64 {
    let f: unsafe extern "C" fn(Id, Sel) -> f64 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_id_id(obj: Id, s: Sel, a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}

unsafe fn msg_void_id(obj: Id, s: Sel, a: Id) {
    let f: unsafe extern "C" fn(Id, Sel, Id) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_void_bool(obj: Id, s: Sel, a: bool) {
    let f: unsafe extern "C" fn(Id, Sel, bool) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_void_i64(obj: Id, s: Sel, a: i64) {
    let f: unsafe extern "C" fn(Id, Sel, i64) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_u64(obj: Id, s: Sel) -> u64 {
    let f: unsafe extern "C" fn(Id, Sel) -> u64 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_id_u64(obj: Id, s: Sel, a: u64) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, u64) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_i64(obj: Id, s: Sel) -> i64 {
    let f: unsafe extern "C" fn(Id, Sel) -> i64 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_u16(obj: Id, s: Sel) -> u16 {
    let f: unsafe extern "C" fn(Id, Sel) -> u16 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
/// A `BOOL` getter. `BOOL` is `bool` on arm64 and `signed char` on x86_64;
/// both come back in the low byte of the return register, read here as `i8`.
unsafe fn msg_bool(obj: Id, s: Sel) -> bool {
    let f: unsafe extern "C" fn(Id, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) != 0 }
}
// NSPoint / NSRect are 2 / 4 doubles. On arm64 (macos-latest) these are
// returned through plain `objc_msgSend` (no `_stret`), so the transmute is ABI-
// correct there.
unsafe fn msg_point(obj: Id, s: Sel) -> CgPoint {
    let f: unsafe extern "C" fn(Id, Sel) -> CgPoint =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_rect(obj: Id, s: Sel) -> CgRect {
    let f: unsafe extern "C" fn(Id, Sel) -> CgRect =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}

unsafe fn msg_void_size(obj: Id, s: Sel, a: CgSize) {
    let f: unsafe extern "C" fn(Id, Sel, CgSize) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_void_u64(obj: Id, s: Sel, a: u64) {
    let f: unsafe extern "C" fn(Id, Sel, u64) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
/// `-respondsToSelector:` (a `BOOL`, read as in [`msg_bool`]).
unsafe fn responds_to(obj: Id, s: Sel) -> bool {
    let f: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, sel("respondsToSelector:"), s) != 0 }
}

/// The `NSCursor` for `icon`. AppKit has no public diagonal-resize or move
/// cursors; the private ones (also used by WebKit and winit) are taken when
/// present, else the nearest public cursor.
fn ns_cursor(icon: CursorIcon) -> Id {
    let (name, fallback) = match icon {
        CursorIcon::Default => ("arrowCursor", "arrowCursor"),
        CursorIcon::Text => ("IBeamCursor", "IBeamCursor"),
        CursorIcon::Pointer => ("pointingHandCursor", "pointingHandCursor"),
        CursorIcon::Crosshair => ("crosshairCursor", "crosshairCursor"),
        CursorIcon::NotAllowed => ("operationNotAllowedCursor", "operationNotAllowedCursor"),
        CursorIcon::Grab => ("openHandCursor", "openHandCursor"),
        CursorIcon::Grabbing => ("closedHandCursor", "closedHandCursor"),
        CursorIcon::Move => ("_moveCursor", "openHandCursor"),
        CursorIcon::EwResize => ("resizeLeftRightCursor", "resizeLeftRightCursor"),
        CursorIcon::NsResize => ("resizeUpDownCursor", "resizeUpDownCursor"),
        CursorIcon::NwseResize => ("_windowResizeNorthWestSouthEastCursor", "crosshairCursor"),
        CursorIcon::NeswResize => ("_windowResizeNorthEastSouthWestCursor", "crosshairCursor"),
    };
    let cls = class("NSCursor");
    let s = sel(name);
    let s = if unsafe { responds_to(cls, s) } {
        s
    } else {
        sel(fallback)
    };
    unsafe { msg_id(cls, s) }
}

/// An `NSImage` of a straight-alpha RGBA image (+1 retained), via a `CGImage`
/// over a copy of the pixels.
unsafe fn ns_image(icon: &Pixmap) -> Id {
    let size = icon.size();
    let (w, h) = (size.width as usize, size.height as usize);
    let bytes = icon.as_bytes();
    unsafe {
        let data = CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
        let provider = CGDataProviderCreateWithCFData(data);
        CFRelease(data);
        let space = CGColorSpaceCreateDeviceRGB();
        let image = CGImageCreate(
            w,
            h,
            8,
            32,
            w * 4,
            space,
            CG_BITMAP_RGBA8,
            provider,
            std::ptr::null(),
            true,
            0,
        );
        CGColorSpaceRelease(space);
        CGDataProviderRelease(provider);
        if image.is_null() {
            return std::ptr::null_mut();
        }
        let init: unsafe extern "C" fn(Id, Sel, *mut c_void, CgSize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let ns = init(
            msg_id(class("NSImage"), sel("alloc")),
            sel("initWithCGImage:size:"),
            image,
            CgSize {
                width: w as f64,
                height: h as f64,
            },
        );
        CGImageRelease(image);
        ns
    }
}

/// `[NSString stringWithUTF8String:s]` — an autoreleased NSString.
fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap();
    let f: unsafe extern "C" fn(Id, Sel, *const i8) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(class("NSString"), sel("stringWithUTF8String:"), c.as_ptr()) }
}

// NSEventType values we care about.
const NS_LEFT_MOUSE_DOWN: u64 = 1;
const NS_LEFT_MOUSE_UP: u64 = 2;
const NS_RIGHT_MOUSE_DOWN: u64 = 3;
const NS_RIGHT_MOUSE_UP: u64 = 4;
const NS_MOUSE_MOVED: u64 = 5;
const NS_LEFT_MOUSE_DRAGGED: u64 = 6;
const NS_KEY_DOWN: u64 = 10;
const NS_KEY_UP: u64 = 11;
const NS_SCROLL_WHEEL: u64 = 22;

/// The string an `NSString`-returning getter gives, or `None` if it's nil.
unsafe fn event_string(ev: Id, getter: &str) -> Option<String> {
    let chars = unsafe { msg_id(ev, sel(getter)) };
    if chars.is_null() {
        return None;
    }
    let utf8 = unsafe { msg_id(chars, sel("UTF8String")) } as *const i8;
    if utf8.is_null() {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(utf8) };
    Some(s.to_string_lossy().into_owned())
}

/// Translate a key-down or key-up `NSEvent`. Named keys (by virtual key code)
/// become [`Event::Key`]; with Command or Control held, a printable key becomes
/// a [`KeyCode::Char`] shortcut (from `charactersIgnoringModifiers`, which
/// keeps Shift); otherwise a key-down's `characters` are the typed text.
unsafe fn key_event(ev: Id, state: ButtonState) -> Option<Event> {
    let modifiers = input::mac_modifiers(unsafe { msg_u64(ev, sel("modifierFlags")) });
    let kc = unsafe { msg_u16(ev, sel("keyCode")) };
    if let Some(code) = input::mac_keycode(kc) {
        return Some(Event::Key {
            code,
            state,
            modifiers,
        });
    }
    if modifiers.meta || modifiers.ctrl {
        let s = unsafe { event_string(ev, "charactersIgnoringModifiers") }?;
        let mut chars = s.chars();
        let c = chars.next().filter(|c| !c.is_control())?;
        if chars.next().is_some() || input::mac_is_function_key(&s) {
            return None;
        }
        return Some(Event::Key {
            code: KeyCode::Char(c),
            state,
            modifiers,
        });
    }
    if state != ButtonState::Pressed {
        return None;
    }
    // Otherwise deliver the typed characters as text. Unmapped function keys
    // (F13, Page Up on old layouts, …) report private-use code points.
    let s = unsafe { event_string(ev, "characters") }?;
    if s.chars().all(|c| c.is_control()) || input::mac_is_function_key(&s) {
        None
    } else {
        Some(Event::Text(s))
    }
}

/// Post an application-defined event, ending the main loop's wait. Safe from any
/// thread.
fn post_wake_event() {
    const APPLICATION_DEFINED: u64 = 15;
    unsafe {
        let make: unsafe extern "C" fn(
            Id,
            Sel,
            u64,
            CgPoint,
            u64,
            f64,
            i64,
            Id,
            i16,
            i64,
            i64,
        ) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
        let event = make(
            class("NSEvent"),
            sel(
                "otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
            ),
            APPLICATION_DEFINED,
            CgPoint { x: 0.0, y: 0.0 },
            0,
            0.0,
            0,
            std::ptr::null_mut(),
            0,
            0,
            0,
        );
        if event.is_null() {
            return;
        }
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let post: unsafe extern "C" fn(Id, Sel, Id, bool) =
            std::mem::transmute(objc_msgSend as *const c_void);
        post(app, sel("postEvent:atStart:"), event, false);
    }
}

/// Translate an `NSEvent` into a Stipple event. `view_h` is the content height,
/// used to flip the y axis (Cocoa's window origin is bottom-left).
unsafe fn translate_event(etype: u64, ev: Id, view_h: f64) -> Option<Event> {
    let pointer = |button: PointerButton, state: ButtonState| {
        let p = unsafe { msg_point(ev, sel("locationInWindow")) };
        crate::platform::set_pointer_modifiers(input::mac_modifiers(unsafe {
            msg_u64(ev, sel("modifierFlags"))
        }));
        Event::PointerButton {
            button,
            state,
            position: Point::new(p.x, view_h - p.y),
        }
    };
    match etype {
        NS_LEFT_MOUSE_DOWN => Some(pointer(PointerButton::Left, ButtonState::Pressed)),
        NS_LEFT_MOUSE_UP => Some(pointer(PointerButton::Left, ButtonState::Released)),
        NS_RIGHT_MOUSE_DOWN => Some(pointer(PointerButton::Right, ButtonState::Pressed)),
        NS_RIGHT_MOUSE_UP => Some(pointer(PointerButton::Right, ButtonState::Released)),
        NS_MOUSE_MOVED | NS_LEFT_MOUSE_DRAGGED => {
            let p = unsafe { msg_point(ev, sel("locationInWindow")) };
            Some(Event::PointerMoved {
                position: Point::new(p.x, view_h - p.y),
            })
        }
        NS_KEY_DOWN => unsafe { key_event(ev, ButtonState::Pressed) },
        NS_KEY_UP => unsafe { key_event(ev, ButtonState::Released) },
        NS_SCROLL_WHEEL => {
            // CGFloat deltas: points for trackpads (precise), lines for wheels.
            let dx = unsafe { msg_f64(ev, sel("scrollingDeltaX")) };
            let dy = unsafe { msg_f64(ev, sel("scrollingDeltaY")) };
            let precise = unsafe { msg_bool(ev, sel("hasPreciseScrollingDeltas")) };
            let delta = input::mac_scroll(dx, dy, precise);
            // Momentum and phase-end events can carry no movement.
            (delta.dx != 0.0 || delta.dy != 0.0).then_some(Event::Scroll { delta })
        }
        _ => None,
    }
}

// ---- Shared framebuffer -----------------------------------------------------

#[derive(Default)]
struct MacCtx {
    fb: Vec<u8>, // straight RGBA8, top row first
    w: usize,
    h: usize,
    view: Id,
    window: Id,
    /// Physical pixels per point: the window's `backingScaleFactor`.
    scale: f64,
    /// Accessible label exposed to NSAccessibility (the window title).
    a11y_label: String,
    /// The current accessibility tree (pushed each frame by the App). The view's
    /// `accessibilityChildren` builds native elements from this on demand.
    a11y_tree: Option<crate::platform::access::A11yNode>,
    /// Set by the window delegate's `windowWillClose:` so the event loop can
    /// break instead of spinning on a closed window.
    should_close: bool,
    /// The cursor the app asked for, set again on each mouse move over the
    /// view (AppKit resets it at the window's edges and title bar).
    cursor: CursorIcon,
    /// The focused text field, for the input method (see `text_input`).
    ime: Option<crate::platform::ImeState>,
    /// The input method's marked (composition) text.
    marked: String,
    /// Events the `NSTextInputClient` callbacks produced during a key-down.
    ime_events: Vec<Event>,
    /// Whether the key-down being interpreted was a command, not text.
    ime_command: bool,
    /// The target of the menu bar's items (a `StippleMenuTarget`), created
    /// with the first menu bar.
    menu_target: Id,
    /// Each menu bar command's (enabled, checked) state by tag, for
    /// `validateMenuItem:`.
    menu_states: std::collections::HashMap<i64, (bool, Option<bool>)>,
    /// Events raised inside AppKit callbacks (a menu pick), delivered to the
    /// handler once `sendEvent:` returns.
    pending: Vec<Event>,
}

// Cocoa is single-threaded (main thread); the backend runs there.
thread_local! {
    static CTX: RefCell<MacCtx> = RefCell::new(MacCtx::default());
}

/// `drawRect:` implementation for our NSView subclass: blit the framebuffer.
extern "C" fn draw_rect(_this: Id, _cmd: Sel, _dirty: CgRect) {
    CTX.with(|c| {
        let c = c.borrow();
        if c.fb.is_empty() {
            return;
        }
        unsafe {
            // Current CGContext: [[NSGraphicsContext currentContext] CGContext]
            let gctx_cls = class("NSGraphicsContext");
            let current = msg_id(gctx_cls, sel("currentContext"));
            if current.is_null() {
                return;
            }
            let cg = msg_id(current, sel("CGContext"));
            if cg.is_null() {
                return;
            }

            let space = CGColorSpaceCreateDeviceRGB();
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                c.fb.as_ptr() as *const c_void,
                c.fb.len(),
                std::ptr::null(),
            );
            let image = CGImageCreate(
                c.w,
                c.h,
                8,
                32,
                c.w * 4,
                space,
                CG_BITMAP_RGBA8,
                provider,
                std::ptr::null(),
                false,
                0,
            );
            // The context is in points; the framebuffer is in physical pixels.
            let (width, height) = (c.w as f64 / c.scale, c.h as f64 / c.scale);
            let rect = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize { width, height },
            };
            // The view is `isFlipped` (top-left origin), but CGContextDrawImage
            // interprets image data as Y=0-at-bottom, so without compensation
            // the framebuffer lands upside down. Flip the CTM around the draw —
            // the same fix the x11anywhere macOS backend uses.
            CGContextSaveGState(cg);
            CGContextTranslateCTM(cg, 0.0, height);
            CGContextScaleCTM(cg, 1.0, -1.0);
            CGContextDrawImage(cg, rect, image);
            CGContextRestoreGState(cg);
            CGImageRelease(image);
            CGDataProviderRelease(provider);
            CGColorSpaceRelease(space);
        }
    });
}

/// `isFlipped` => YES, so the view's coordinate origin is top-left and our
/// top-row-first framebuffer draws upright.
extern "C" fn is_flipped(_this: Id, _cmd: Sel) -> bool {
    true
}

/// `windowWillClose:` — the view doubles as the window's delegate. Flag the
/// close so `run`'s event loop exits instead of spinning forever on a window
/// the user already dismissed.
extern "C" fn window_will_close(_this: Id, _cmd: Sel, _notification: Id) {
    CTX.with(|c| c.borrow_mut().should_close = true);
}

/// `stippleAppearanceChanged:` — the view observes the system's appearance and
/// accent-color notifications; the next wake reports the change to the App.
extern "C" fn appearance_changed(_this: Id, _cmd: Sel, _notification: Id) {
    crate::platform::native::note_changed();
}

/// Have `view` hear about dark mode and accent changes: dark mode and accent
/// changes are distributed notifications (they come from System Settings), and
/// AppKit posts `NSSystemColorsDidChangeNotification` once its colors follow.
///
/// # Safety
/// `view` must be a live `StippleView`, which stays alive for the process.
unsafe fn observe_appearance(view: Id) {
    let add: unsafe extern "C" fn(Id, Sel, Id, Sel, Id, Id) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    let selector = sel("stippleAppearanceChanged:");
    let observe = sel("addObserver:selector:name:object:");
    unsafe {
        let distributed = msg_id(
            class("NSDistributedNotificationCenter"),
            sel("defaultCenter"),
        );
        for name in [
            "AppleInterfaceThemeChangedNotification",
            "AppleColorPreferencesChangedNotification",
        ] {
            add(
                distributed,
                observe,
                view,
                selector,
                nsstring(name),
                std::ptr::null_mut(),
            );
        }
        let local = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        let name = nsstring("NSSystemColorsDidChangeNotification");
        add(local, observe, view, selector, name, std::ptr::null_mut());
    }
}

/// Give `window` the dark or light appearance, which its title bar follows.
///
/// # Safety
/// `window` must be a live `NSWindow`.
unsafe fn set_window_dark(window: Id, dark: bool) {
    let name = nsstring(if dark {
        "NSAppearanceNameDarkAqua"
    } else {
        "NSAppearanceNameAqua"
    });
    unsafe {
        let appearance = msg_id_id(class("NSAppearance"), sel("appearanceNamed:"), name);
        msg_void_id(window, sel("setAppearance:"), appearance);
    }
}

// ---- NSAccessibility ----------------------------------------------------
//
// Stipple draws every control itself, so to AppKit the window is one opaque view.
// We override the NSAccessibility protocol methods on our view to vend semantic
// info (the macOS half of the cross-platform a11y bridge; AT-SPI is the Linux
// half). This first step exposes the view as an accessible group with the
// window's title as its label; exposing the full element tree builds on it.

/// `isAccessibilityElement` => YES (the view is a real accessibility element).
extern "C" fn acc_is_element(_this: Id, _cmd: Sel) -> bool {
    true
}

/// `accessibilityRole` => `NSAccessibilityGroupRole` ("AXGroup"): a self-drawn
/// container.
extern "C" fn acc_role(_this: Id, _cmd: Sel) -> Id {
    nsstring("AXGroup")
}

/// `accessibilityLabel` => the window's accessible label.
extern "C" fn acc_label(_this: Id, _cmd: Sel) -> Id {
    CTX.with(|c| nsstring(&c.borrow().a11y_label))
}

/// Map a neutral [`A11yRole`](crate::platform::access::A11yRole) to its `NSAccessibility`
/// role constant. The window root stays the view's own `AXGroup` (see
/// [`acc_role`]); its descendants get specific control roles here.
fn ax_role(role: crate::platform::access::A11yRole) -> &'static str {
    use crate::platform::access::A11yRole::*;
    match role {
        Window | Group => "AXGroup",
        Button => "AXButton",
        TextField => "AXTextField",
        Text => "AXStaticText",
        List => "AXList",
        Cell => "AXCell",
        Table => "AXTable",
        ListItem | Row | TreeItem => "AXRow",
        ColumnHeader => "AXButton",
        Tree => "AXOutline",
        MenuBar => "AXMenuBar",
        Menu => "AXMenu",
        MenuItem => "AXMenuItem",
        // Not named to NSAccessibility yet: the closest role above.
        ComboBox | CheckBox | Switch => "AXButton",
        SpinButton => "AXTextField",
        Splitter | Toolbar | Separator | Alert => "AXGroup",
    }
}

/// Build a native `NSAccessibilityElement` for `node` (parented to `parent`),
/// recursing so the whole subtree is exposed. Uses the built-in convenience
/// constructor `+accessibilityElementWithRole:frame:label:parent:`, so no custom
/// element class or ivars are needed.
///
/// `frame` is reported in the node's logical bounds for now; mapping it to screen
/// coordinates (which VoiceOver's cursor needs) is follow-up depth.
unsafe fn build_ax_element(node: &crate::platform::access::A11yNode, parent: Id) -> Id {
    let (x, y, w, h) = node.bounds;
    let frame = CgRect {
        origin: CgPoint { x, y },
        size: CgSize {
            width: w,
            height: h,
        },
    };
    let make: unsafe extern "C" fn(Id, Sel, Id, CgRect, Id, Id) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    let el = unsafe {
        make(
            class("NSAccessibilityElement"),
            sel("accessibilityElementWithRole:frame:label:parent:"),
            nsstring(ax_role(node.role)),
            frame,
            nsstring(&node.name),
            parent,
        )
    };
    if !node.children.is_empty() {
        let arr = unsafe { msg_id(class("NSMutableArray"), sel("array")) };
        for child in &node.children {
            let ce = unsafe { build_ax_element(child, el) };
            unsafe { msg_void_id(arr, sel("addObject:"), ce) };
        }
        unsafe { msg_void_id(el, sel("setAccessibilityChildren:"), arr) };
    }
    el
}

/// Recursively walk the *native* AppKit accessibility element tree (the same
/// `-accessibilityChildren` / `-accessibilityRole` / `-accessibilityLabel`
/// messages an assistive client sends), printing each element. Used by the CI
/// self-check to prove the whole hierarchy — not just the root — is vended.
unsafe fn dump_ax(el: Id, depth: usize) {
    let role = nsstring_to_string(unsafe { msg_id(el, sel("accessibilityRole")) });
    let label = nsstring_to_string(unsafe { msg_id(el, sel("accessibilityLabel")) });
    let indent = "  ".repeat(depth);
    println!("Cocoa a11y:{indent} role={role} label={label}");
    let children = unsafe { msg_id(el, sel("accessibilityChildren")) };
    if !children.is_null() {
        let n = unsafe { msg_u64(children, sel("count")) };
        for i in 0..n {
            let child = unsafe { msg_id_u64(children, sel("objectAtIndex:"), i) };
            unsafe { dump_ax(child, depth + 1) };
        }
    }
}

/// `accessibilityChildren` => the window root's children as native elements, so
/// AppKit exposes the full Stipple element tree to assistive technologies. The
/// root node maps to the view itself (an `AXGroup`), so we vend *its* children.
extern "C" fn acc_children(this: Id, _cmd: Sel) -> Id {
    CTX.with(|c| {
        let c = c.borrow();
        let Some(root) = c.a11y_tree.as_ref() else {
            return std::ptr::null_mut();
        };
        unsafe {
            let arr = msg_id(class("NSMutableArray"), sel("array"));
            for child in &root.children {
                let el = build_ax_element(child, this);
                msg_void_id(arr, sel("addObject:"), el);
            }
            arr
        }
    })
}

/// Copy an `NSString` out to a Rust `String` (via `-UTF8String`).
fn nsstring_to_string(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> *const i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = f(s, sel("UTF8String"));
        if p.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

// ---- The menu bar ------------------------------------------------------------

/// The tag of the application menu's Quit item, which ends the event loop.
const MENU_QUIT_TAG: i64 = -1;

/// `stippleMenuAction:` — a menu bar command was picked: report its tag (the
/// command id) to the App, or quit.
extern "C" fn menu_action(_this: Id, _cmd: Sel, sender: Id) {
    let tag = unsafe { msg_i64(sender, sel("tag")) };
    if tag == MENU_QUIT_TAG {
        CTX.with(|c| c.borrow_mut().should_close = true);
        return;
    }
    // The App matches shortcuts itself, from the key event the loop hands it
    // before AppKit sees it (as on every platform). When AppKit then matches
    // the same key to this item's key equivalent, it's a repeat: drop it.
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let ev = msg_id(app, sel("currentEvent"));
        if !ev.is_null() && msg_u64(ev, sel("type")) == NS_KEY_DOWN {
            let key = nsstring_to_string(msg_id(sender, sel("keyEquivalent")));
            let typed = event_string(ev, "charactersIgnoringModifiers").unwrap_or_default();
            if !key.is_empty() && key.to_lowercase() == typed.to_lowercase() {
                return;
            }
        }
    }
    CTX.with(|c| c.borrow_mut().pending.push(Event::MenuCommand(tag as u32)));
}

/// `validateMenuItem:` — AppKit asks before showing (or key-matching) an item
/// whether it is enabled; the check mark is refreshed at the same time.
extern "C" fn validate_menu_item(_this: Id, _cmd: Sel, item: Id) -> bool {
    let tag = unsafe { msg_i64(item, sel("tag")) };
    if tag == MENU_QUIT_TAG {
        return true;
    }
    let state = CTX.with(|c| c.borrow().menu_states.get(&tag).copied());
    let Some((enabled, checked)) = state else {
        return false;
    };
    let on = i64::from(checked == Some(true)); // NSControlStateValueOn = 1
    unsafe { msg_void_i64(item, sel("setState:"), on) };
    enabled
}

/// The `NSObject` subclass whose instance is the target of the menu bar's
/// items.
fn menu_target() -> Id {
    let existing = CTX.with(|c| c.borrow().menu_target);
    if !existing.is_null() {
        return existing;
    }
    let name = c"StippleMenuTarget";
    unsafe {
        let mut cls = objc_getClass(name.as_ptr());
        if cls.is_null() {
            cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            class_addMethod(
                cls,
                sel("stippleMenuAction:"),
                menu_action as *const c_void,
                c"v@:@".as_ptr(),
            );
            class_addMethod(
                cls,
                sel("validateMenuItem:"),
                validate_menu_item as *const c_void,
                c"c@:@".as_ptr(),
            );
            objc_registerClassPair(cls);
        }
        let target = msg_id(msg_id(cls, sel("alloc")), sel("init"));
        CTX.with(|c| c.borrow_mut().menu_target = target);
        target
    }
}

/// A new `NSMenuItem` (retained: add it to a menu, then release it).
unsafe fn menu_item(title: &str, action: Option<Sel>, key: &str, mask: u64) -> Id {
    let init: unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    let item = unsafe {
        init(
            msg_id(class("NSMenuItem"), sel("alloc")),
            sel("initWithTitle:action:keyEquivalent:"),
            nsstring(title),
            action.unwrap_or(std::ptr::null_mut()),
            nsstring(key),
        )
    };
    if mask != 0 {
        unsafe { msg_void_u64(item, sel("setKeyEquivalentModifierMask:"), mask) };
    }
    item
}

/// A new `NSMenu` titled `title` (retained).
unsafe fn ns_menu(title: &str) -> Id {
    unsafe {
        msg_id_id(
            msg_id(class("NSMenu"), sel("alloc")),
            sel("initWithTitle:"),
            nsstring(title),
        )
    }
}

/// Add `item` to `menu`, which retains it, and drop our reference.
unsafe fn add_item(menu: Id, item: Id) {
    unsafe {
        msg_void_id(menu, sel("addItem:"), item);
        msg_void(item, sel("release"));
    }
}

/// Add an item that opens `submenu` (retained; released here).
unsafe fn add_submenu(menu: Id, title: &str, submenu: Id) {
    unsafe {
        let item = menu_item(title, None, "", 0);
        msg_void_id(item, sel("setSubmenu:"), submenu);
        msg_void(submenu, sel("release"));
        add_item(menu, item);
    }
}

/// Fill `menu` with `items`, targeting the menu target and recording each
/// command's state for validation.
unsafe fn fill_menu(menu: Id, items: &[NativeMenuItem], target: Id) {
    for item in items {
        match item {
            NativeMenuItem::Action {
                id,
                label,
                shortcut,
                enabled,
                checked,
            } => {
                let (key, mask) = shortcut
                    .as_ref()
                    .and_then(input::mac_key_equivalent)
                    .unwrap_or_default();
                let tag = i64::from(*id);
                unsafe {
                    let it = menu_item(
                        &mnemonic(label).0,
                        Some(sel("stippleMenuAction:")),
                        &key,
                        mask,
                    );
                    msg_void_id(it, sel("setTarget:"), target);
                    msg_void_i64(it, sel("setTag:"), tag);
                    msg_void_i64(it, sel("setState:"), i64::from(*checked == Some(true)));
                    add_item(menu, it);
                }
                CTX.with(|c| {
                    c.borrow_mut().menu_states.insert(tag, (*enabled, *checked));
                });
            }
            NativeMenuItem::Separator => unsafe {
                let sep = msg_id(class("NSMenuItem"), sel("separatorItem"));
                msg_void_id(menu, sel("addItem:"), sep);
            },
            NativeMenuItem::Submenu(sub) => unsafe {
                let title = mnemonic(&sub.label).0;
                let m = ns_menu(&title);
                fill_menu(m, &sub.items, target);
                add_submenu(menu, &title, m);
            },
        }
    }
}

/// Install `bar` as the application's main menu: the standard application
/// menu (About, Hide, Hide Others, Show All, Quit) followed by the app's
/// menus.
unsafe fn install_menu_bar(bar: &NativeMenuBar) {
    let target = menu_target();
    CTX.with(|c| c.borrow_mut().menu_states.clear());
    let name = &bar.app_name;
    const CMD: u64 = 1 << 20;
    const OPTION: u64 = 1 << 19;
    unsafe {
        let main = ns_menu("");
        let app_menu = ns_menu(name);
        let about = format!("About {name}");
        add_item(
            app_menu,
            menu_item(&about, Some(sel("orderFrontStandardAboutPanel:")), "", 0),
        );
        msg_void_id(
            app_menu,
            sel("addItem:"),
            msg_id(class("NSMenuItem"), sel("separatorItem")),
        );
        let hide = format!("Hide {name}");
        add_item(app_menu, menu_item(&hide, Some(sel("hide:")), "h", CMD));
        add_item(
            app_menu,
            menu_item(
                "Hide Others",
                Some(sel("hideOtherApplications:")),
                "h",
                CMD | OPTION,
            ),
        );
        add_item(
            app_menu,
            menu_item("Show All", Some(sel("unhideAllApplications:")), "", 0),
        );
        msg_void_id(
            app_menu,
            sel("addItem:"),
            msg_id(class("NSMenuItem"), sel("separatorItem")),
        );
        let quit = format!("Quit {name}");
        let quit = menu_item(&quit, Some(sel("stippleMenuAction:")), "q", CMD);
        msg_void_id(quit, sel("setTarget:"), target);
        msg_void_i64(quit, sel("setTag:"), MENU_QUIT_TAG);
        add_item(app_menu, quit);
        add_submenu(main, name, app_menu);

        for menu in &bar.menus {
            let title = mnemonic(&menu.label).0;
            let m = ns_menu(&title);
            fill_menu(m, &menu.items, target);
            add_submenu(main, &title, m);
        }
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("setMainMenu:"), main);
        msg_void(main, sel("release"));
    }
}

fn register_view_class() -> Class {
    let name = c"StippleView";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let superclass = class("NSView");
        let cls = objc_allocateClassPair(superclass, name.as_ptr(), 0);
        let draw_types = c"v@:{CGRect={CGPoint=dd}{CGSize=dd}}";
        class_addMethod(
            cls,
            sel("drawRect:"),
            draw_rect as *const c_void,
            draw_types.as_ptr(),
        );
        let flip_types = c"c@:";
        class_addMethod(
            cls,
            sel("isFlipped"),
            is_flipped as *const c_void,
            flip_types.as_ptr(),
        );
        // Window delegate method: void return, takes the notification id (v@:@).
        let close_types = c"v@:@";
        class_addMethod(
            cls,
            sel("windowWillClose:"),
            window_will_close as *const c_void,
            close_types.as_ptr(),
        );
        // Notification observer: also v@:@.
        class_addMethod(
            cls,
            sel("stippleAppearanceChanged:"),
            appearance_changed as *const c_void,
            close_types.as_ptr(),
        );
        // NSAccessibility overrides: BOOL (c@:) and id (@@:) returns.
        let bool_types = c"c@:";
        let id_types = c"@@:";
        class_addMethod(
            cls,
            sel("isAccessibilityElement"),
            acc_is_element as *const c_void,
            bool_types.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityRole"),
            acc_role as *const c_void,
            id_types.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityLabel"),
            acc_label as *const c_void,
            id_types.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityChildren"),
            acc_children as *const c_void,
            id_types.as_ptr(),
        );
        text_input::add_methods(cls);
        objc_registerClassPair(cls);
        cls
    }
}

#[derive(Debug)]
struct MacWindow {
    size: PhysicalSize,
}

impl Window for MacWindow {
    fn set_ime(&self, state: Option<&crate::platform::ImeState>) {
        text_input::set_state(state);
    }
    fn id(&self) -> WindowId {
        WindowId(1)
    }
    fn inner_size(&self) -> PhysicalSize {
        self.size
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(CTX.with(|c| c.borrow().scale))
    }
    fn request_redraw(&self) {
        CTX.with(|c| {
            let view = c.borrow().view;
            if !view.is_null() {
                unsafe { msg_void_bool(view, sel("setNeedsDisplay:"), true) };
            }
        });
    }
    fn set_title(&self, title: &str) {
        let window: Id = CTX.with(|c| c.borrow().window);
        if !window.is_null() {
            unsafe { msg_void_id(window, sel("setTitle:"), nsstring(title)) };
        }
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(MacSurface { size: self.size })
    }
    fn clipboard(&self) -> Option<String> {
        crate::platform::clipboard::text()
    }
    fn set_clipboard(&self, text: &str) {
        crate::platform::clipboard::set_text(text);
    }
    fn set_dark(&self, dark: bool) {
        let window: Id = CTX.with(|c| c.borrow().window);
        if !window.is_null() {
            unsafe { set_window_dark(window, dark) };
        }
    }
    fn set_accessibility_tree(&self, root: &crate::platform::access::A11yNode) {
        // Stash the tree; AppKit pulls `accessibilityChildren` lazily, so we just
        // keep the latest one. Adopt the root's name as the view's label too.
        CTX.with(|c| {
            let mut c = c.borrow_mut();
            if !root.name.is_empty() {
                c.a11y_label = root.name.clone();
            }
            c.a11y_tree = Some(root.clone());
        });
    }
    fn set_cursor(&self, cursor: CursorIcon) {
        CTX.with(|c| c.borrow_mut().cursor = cursor);
        unsafe { msg_void(ns_cursor(cursor), sel("set")) };
    }
    fn set_min_size(&self, size: Option<Size>) {
        set_content_limit(sel("setContentMinSize:"), size, 0.0);
    }
    fn set_max_size(&self, size: Option<Size>) {
        set_content_limit(sel("setContentMaxSize:"), size, f32::MAX as f64);
    }
    fn set_maximized(&self, maximized: bool) {
        let window: Id = CTX.with(|c| c.borrow().window);
        if !window.is_null() && unsafe { msg_bool(window, sel("isZoomed")) } != maximized {
            unsafe { msg_void_id(window, sel("zoom:"), std::ptr::null_mut()) };
        }
    }
    fn set_fullscreen(&self, fullscreen: bool) {
        let window: Id = CTX.with(|c| c.borrow().window);
        if window.is_null() {
            return;
        }
        let now = unsafe { msg_u64(window, sel("styleMask")) } & NS_WINDOW_STYLE_FULLSCREEN != 0;
        if now != fullscreen {
            unsafe { msg_void_id(window, sel("toggleFullScreen:"), std::ptr::null_mut()) };
        }
    }
    fn set_menu_bar(&self, menu: &NativeMenuBar) -> bool {
        unsafe { install_menu_bar(menu) };
        true
    }
    fn set_icon(&self, icon: Option<&Pixmap>) {
        // macOS has no per-window icons; this is the app's Dock icon. `nil`
        // restores the bundle's icon.
        unsafe {
            let image = icon.map_or(std::ptr::null_mut(), |i| ns_image(i));
            let app = msg_id(class("NSApplication"), sel("sharedApplication"));
            msg_void_id(app, sel("setApplicationIconImage:"), image);
            if !image.is_null() {
                msg_void(image, sel("release"));
            }
        }
    }
}

/// Set the window's content min or max size (in points); `None` resets it to
/// `unbounded`.
fn set_content_limit(setter: Sel, size: Option<Size>, unbounded: f64) {
    let window: Id = CTX.with(|c| c.borrow().window);
    if window.is_null() {
        return;
    }
    let size = size.map_or(
        CgSize {
            width: unbounded,
            height: unbounded,
        },
        |s| CgSize {
            width: s.width,
            height: s.height,
        },
    );
    unsafe { msg_void_size(window, setter, size) };
}

#[derive(Debug)]
struct MacSurface {
    size: PhysicalSize,
}

impl Surface for MacSurface {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, pixmap: &Pixmap, _damage: &[crate::geometry::Rect]) {
        let size = pixmap.size();
        CTX.with(|c| {
            let mut c = c.borrow_mut();
            // Reuse the buffer's capacity instead of reallocating every frame.
            c.fb.clear();
            c.fb.extend_from_slice(pixmap.as_bytes());
            c.w = size.width as usize;
            c.h = size.height as usize;
            if !c.view.is_null() {
                unsafe { msg_void_bool(c.view, sel("setNeedsDisplay:"), true) };
            }
        });
    }
}

/// Create an NSWindow with our drawing view, render the initial frame, and run
/// a hand-rolled Cocoa event loop: each iteration dequeues one `NSEvent`,
/// translates it to a Stipple event for the handler, then forwards it to AppKit.
/// The loop exits when the handler returns [`ControlFlow::Exit`] or the user
/// closes the window (via the `windowWillClose:` delegate).
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let size = ScaleFactor::IDENTITY.to_physical(attrs.logical_size);
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        if app.is_null() {
            return Err(PlatformError::Os("NSApplication unavailable".into()));
        }
        msg_void_i64(app, sel("setActivationPolicy:"), ACTIVATION_REGULAR);

        let content = CgRect {
            origin: CgPoint { x: 0.0, y: 0.0 },
            size: CgSize {
                width: size.width as f64,
                height: size.height as f64,
            },
        };

        // window = [[NSWindow alloc] initWithContentRect:styleMask:backing:defer:]
        let window: Id = {
            let alloc = msg_id(class("NSWindow"), sel("alloc"));
            let init: unsafe extern "C" fn(Id, Sel, CgRect, u64, u64, bool) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            init(
                alloc,
                sel("initWithContentRect:styleMask:backing:defer:"),
                content,
                NS_WINDOW_STYLE_TITLED | NS_WINDOW_STYLE_CLOSABLE | NS_WINDOW_STYLE_RESIZABLE,
                NS_BACKING_BUFFERED,
                false,
            )
        };
        if window.is_null() {
            return Err(PlatformError::Os("NSWindow init failed".into()));
        }

        // view = [[StippleView alloc] initWithFrame:content]
        let view_cls = register_view_class();
        let view: Id = {
            let alloc = msg_id(view_cls, sel("alloc"));
            let init: unsafe extern "C" fn(Id, Sel, CgRect) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            init(alloc, sel("initWithFrame:"), content)
        };
        msg_void_id(window, sel("setContentView:"), view);
        msg_void_id(window, sel("setTitle:"), nsstring(&attrs.title));
        if let Some(dark) = attrs.dark {
            // A window's appearance sets its title bar to match the content.
            set_window_dark(window, dark);
        }
        observe_appearance(view);
        // The view doubles as the window delegate so `windowWillClose:` can flag
        // the event loop to exit. Keep the window object alive on close (we hold
        // the only reference) so the loop can tear down cleanly without a
        // use-after-free on `view`/`window`.
        msg_void_bool(window, sel("setReleasedWhenClosed:"), false);
        // Deliver NSMouseMoved, not just drags: hover, and the wheel's target
        // (the app scrolls what's under the last pointer position), need it.
        msg_void_bool(window, sel("setAcceptsMouseMovedEvents:"), true);
        msg_void_id(window, sel("setDelegate:"), view);
        // The view takes key events, so its input context is the active one.
        msg_id_id(window, sel("makeFirstResponder:"), view);
        // The window is laid out in points; the framebuffer is in physical pixels.
        let scale = msg_f64(window, sel("backingScaleFactor"));
        let size = ScaleFactor::new(scale).to_physical(attrs.logical_size);
        CTX.with(|c| {
            let mut c = c.borrow_mut();
            c.view = view;
            c.window = window;
            c.scale = scale;
            // Expose the window title as the view's accessible label.
            c.a11y_label = attrs.title.clone();
        });

        let mut win = MacWindow { size };
        msg_void_u64(
            window,
            sel("setCollectionBehavior:"),
            NS_COLLECTION_FULLSCREEN_PRIMARY,
        );
        win.set_min_size(attrs.min_size);
        win.set_max_size(attrs.max_size);
        if attrs.icon.is_some() {
            win.set_icon(attrs.icon.as_ref());
        }
        handler(Event::RedrawRequested, &win); // populate the framebuffer + a11y tree

        // Self-check the NSAccessibility wiring (real objc dispatch) for CI:
        // cross-process AX reads need TCC approval the runner won't grant, so we
        // query our own view's accessibility attributes and print them. Runs after
        // the first frame so the App has pushed the accessibility tree.
        if std::env::var("STIPPLE_COCOA_A11Y").is_ok() {
            let role = nsstring_to_string(msg_id(view, sel("accessibilityRole")));
            let label = nsstring_to_string(msg_id(view, sel("accessibilityLabel")));
            let is_el: bool = {
                let f: unsafe extern "C" fn(Id, Sel) -> bool =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(view, sel("isAccessibilityElement"))
            };
            println!("Cocoa a11y: element={is_el} role={role} label={label}");
            // CI captures just this window with `screencapture -l <number>`.
            let number: i64 = {
                let f: unsafe extern "C" fn(Id, Sel) -> i64 =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(window, sel("windowNumber"))
            };
            println!("Cocoa window number: {number}");
            // Walk the vended element tree the same way an assistive client would
            // — `-accessibilityChildren` recursively, reading each element's role +
            // label through real objc dispatch. Proves the *full* tree is exposed,
            // not just the root.
            let children = msg_id(view, sel("accessibilityChildren"));
            let n = if children.is_null() {
                0
            } else {
                msg_u64(children, sel("count"))
            };
            println!("Cocoa a11y: children={n}");
            println!("Cocoa ime: {}", text_input::self_check(view));
            for i in 0..n {
                let child = msg_id_u64(children, sel("objectAtIndex:"), i);
                dump_ax(child, 1);
            }
        }

        msg_void_id(window, sel("center"), std::ptr::null_mut());
        msg_void_id(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), true);
        // Needed for nextEventMatchingMask to dequeue events.
        msg_void(app, sel("finishLaunching"));
        // Zoom and fullscreen act on a window that is on screen.
        if attrs.maximized {
            win.set_maximized(true);
        }
        if attrs.fullscreen {
            win.set_fullscreen(true);
        }

        // Manual event loop: pull each NSEvent, translate it to a Stipple event
        // for the handler, then forward it to AppKit (drawing, window controls).
        // Poll the content view's bounds for live-resize.
        let distant_future = msg_id(class("NSDate"), sel("distantFuture"));
        let mode = nsstring("kCFRunLoopDefaultMode");
        let next_sel = sel("nextEventMatchingMask:untilDate:inMode:dequeue:");
        let next: unsafe extern "C" fn(Id, Sel, u64, Id, Id, bool) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let pool_cls = class("NSAutoreleasePool");
        let mut last = win.size;
        // Other threads end the wait by posting an application-defined event,
        // which AppKit allows from any thread.
        crate::platform::wake::install(std::sync::Arc::new(post_wake_event), None);
        // Animation frames are paced by the display, through a CVDisplayLink.
        display_link::install();
        loop {
            // Drain autoreleased objects (the dequeued NSEvent, NSStrings, etc.)
            // each iteration. This hand-rolled loop stands in for
            // `-[NSApplication run]`, which would otherwise manage the pool for
            // us — without it autoreleased objects accumulate for the app's life.
            let pool = msg_id(msg_id(pool_cls, sel("alloc")), sel("init"));

            let mut exit = false;
            // Wait for an event, or until the next timer is due.
            let until = match crate::platform::wake::timeout(std::time::Instant::now()) {
                Some(t) => {
                    let secs = t.as_secs_f64();
                    let f: unsafe extern "C" fn(Id, Sel, f64) -> Id =
                        std::mem::transmute(objc_msgSend as *const c_void);
                    f(class("NSDate"), sel("dateWithTimeIntervalSinceNow:"), secs)
                }
                None => distant_future,
            };
            let ev = next(app, next_sel, u64::MAX, until, mode, true);
            if !ev.is_null() {
                let etype = msg_u64(ev, sel("type"));
                // Positions stay in points, which are logical pixels.
                let view_h = msg_rect(view, sel("bounds")).size.height;
                // With a text field focused, key-downs go through the input
                // method instead (and not on to AppKit: we've handled them).
                let ime = if etype == NS_KEY_DOWN {
                    text_input::key_down(view, ev)
                } else {
                    None
                };
                if let Some(events) = ime {
                    exit = events
                        .into_iter()
                        .any(|event| handler(event, &win) == ControlFlow::Exit);
                } else {
                    if let Some(event) = translate_event(etype, ev, view_h)
                        && handler(event, &win) == ControlFlow::Exit
                    {
                        exit = true;
                    }
                    // Forward to AppKit (drawing, window controls) unless we're
                    // exiting on this event.
                    if !exit {
                        msg_void_id(app, sel("sendEvent:"), ev);
                    }
                }
                // Deliver what AppKit raised while handling it (menu picks).
                let pending = CTX.with(|c| std::mem::take(&mut c.borrow_mut().pending));
                for event in pending {
                    if !exit && handler(event, &win) == ControlFlow::Exit {
                        exit = true;
                    }
                }
                // Keep the app's cursor over the content: AppKit resets it at
                // the window's edges and title bar.
                if !exit && etype == NS_MOUSE_MOVED {
                    let p = msg_point(ev, sel("locationInWindow"));
                    let b = msg_rect(view, sel("bounds"));
                    let over =
                        p.x >= 0.0 && p.y >= 0.0 && p.x < b.size.width && p.y < b.size.height;
                    let cursor = CTX.with(|c| c.borrow().cursor);
                    if over && cursor != CursorIcon::Default {
                        msg_void(ns_cursor(cursor), sel("set"));
                    }
                }
            }

            if !exit
                && crate::platform::wake::take(std::time::Instant::now())
                && handler(Event::Wake, &win) == ControlFlow::Exit
            {
                exit = true;
            }

            // The window delegate's `windowWillClose:` sets this when the user
            // closes the window. Check before touching `view` below: the window
            // is ordered out and we must not keep polling it.
            if CTX.with(|c| c.borrow().should_close) {
                exit = true;
            }

            if exit {
                msg_void(pool, sel("drain"));
                break;
            }

            // Detect live-resize by polling the view bounds.
            // The scale changes when the window moves to a display of another
            // density, which also changes the physical size.
            let b = msg_rect(view, sel("bounds"));
            let scale = msg_f64(window, sel("backingScaleFactor"));
            let now = PhysicalSize::new(
                (b.size.width.max(1.0) * scale).round() as u32,
                (b.size.height.max(1.0) * scale).round() as u32,
            );
            if now != last || scale != CTX.with(|c| c.borrow().scale) {
                last = now;
                win.size = now;
                CTX.with(|c| c.borrow_mut().scale = scale);
                handler(Event::Resized(now), &win);
            }

            msg_void(pool, sel("drain"));
        }
    }
    Ok(())
}
