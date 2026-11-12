//! A native iOS backend over the raw Objective-C runtime + UIKit/CoreGraphics.
//!
//! No `objc`/`uikit` crate — direct `objc_msgSend` FFI and framework links, the
//! same approach as the macOS backend (workspace policy in `ROADMAP.md` §1).
//! This module opts into `unsafe` for the crate.
//!
//! `UIApplicationMain` boots the app with a hand-built delegate class; the
//! delegate creates a `UIWindow` hosting a custom `UIView` whose `drawRect:`
//! blits the software [`Pixmap`] as a `CGImage`. `UIApplicationMain` never
//! returns, so the Rust handler (which outlives the call) is reached through a
//! stashed raw pointer.
//!
//! Touches arrive as [`Event::Touch`] for every finger, the first also as the
//! pointer. The view adopts `UIKeyInput` and holds the keyboard (first
//! responder) while a text input has focus.
//!
//! **Verification:** build-checked for `aarch64-apple-ios` by the `mobile`
//! cross-compile CI job; the `visual-ios` job runs it on a simulator, where
//! `touchdemo`'s self-test drives the keyboard methods and synthetic touches
//! through [`simulate`].
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::{CString, c_void};

use crate::geometry::{PhysicalSize, Point, ScaleFactor};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{
    ButtonState, Event, KeyCode, Modifiers, PointerButton, Touch, TouchPhase, WindowId,
};
use crate::platform::input;
use crate::platform::window::{Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

/// Accessibility: the view as a `UIAccessibilityContainer`.
mod a11y;
/// The accessibility tree as VoiceOver reads it (UIKit-free, tested on hosts).
pub(crate) mod a11y_tree;
/// Pacing animation frames by the screen (`CADisplayLink`).
mod display_link;
/// The document the keyboard reads (UIKit-free, tested on hosts).
pub(crate) mod doc;
/// Background, foreground, memory warnings and appearance changes.
mod lifecycle;
/// Input methods: `UITextInput` on the view.
mod text_input;

pub use a11y::AccessibilityElement;

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
struct EdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
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
    fn class_addProtocol(cls: Class, protocol: *mut c_void) -> bool;
    fn objc_getProtocol(name: *const i8) -> *mut c_void;
    fn objc_msgSendSuper();
}

/// `struct objc_super`, for messaging a method's UIKit implementation.
#[repr(C)]
struct ObjcSuper {
    receiver: Id,
    super_class: Class,
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIApplicationMain(
        argc: i32,
        argv: *mut *mut i8,
        principal_class_name: Id,
        delegate_class_name: Id,
    ) -> i32;
    fn UIGraphicsGetCurrentContext() -> *mut c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
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

// kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big — R,G,B,A in memory.
const CG_BITMAP_RGBA8: u32 = 1 | (4 << 12);

fn class(name: &str) -> Class {
    let c = CString::new(name).unwrap();
    unsafe { objc_getClass(c.as_ptr()) }
}
fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}

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
unsafe fn msg_rect(obj: Id, s: Sel) -> CgRect {
    let f: unsafe extern "C" fn(Id, Sel) -> CgRect =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_bool(obj: Id, s: Sel) -> bool {
    let f: unsafe extern "C" fn(Id, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) != 0 }
}
unsafe fn msg_usize(obj: Id, s: Sel) -> usize {
    let f: unsafe extern "C" fn(Id, Sel) -> usize =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_id_usize(obj: Id, s: Sel, a: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, usize) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_f64(obj: Id, s: Sel) -> f64 {
    let f: unsafe extern "C" fn(Id, Sel) -> f64 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_insets(obj: Id, s: Sel) -> EdgeInsets {
    let f: unsafe extern "C" fn(Id, Sel) -> EdgeInsets =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s) }
}
unsafe fn msg_void_rect(obj: Id, s: Sel, a: CgRect) {
    let f: unsafe extern "C" fn(Id, Sel, CgRect) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe { f(obj, s, a) }
}
unsafe fn msg_alloc_init_frame(cls: Class, frame: CgRect) -> Id {
    unsafe {
        let alloc = msg_id(cls, sel("alloc"));
        let f: unsafe extern "C" fn(Id, Sel, CgRect) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(alloc, sel("initWithFrame:"), frame)
    }
}

/// `[NSString stringWithUTF8String:s]`.
fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap();
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, *const i8) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(class("NSString"), sel("stringWithUTF8String:"), c.as_ptr())
    }
}

/// An `NSString`'s text (empty for `nil`).
unsafe fn string_of(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> *const std::ffi::c_char =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = f(s, sel("UTF8String"));
        if p.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// The objects of an `NSSet`.
unsafe fn set_objects(set: Id) -> Vec<Id> {
    if set.is_null() {
        return Vec::new();
    }
    unsafe {
        let all = msg_id(set, sel("allObjects"));
        (0..msg_usize(all, sel("count")))
            .map(|i| msg_id_usize(all, sel("objectAtIndex:"), i))
            .collect()
    }
}

// ---- Shared state -----------------------------------------------------------

type DynHandler = *mut dyn FnMut(Event, &dyn Window) -> ControlFlow;

#[derive(Default)]
struct IosCtx {
    fb: Vec<u8>, // straight RGBA8, top row first
    w: usize,
    h: usize,
    view: Id,
    window: Id,
    size: PhysicalSize,
    /// Physical pixels per logical pixel: `UIScreen.scale`.
    scale: f64,
    /// What shows around the safe area.
    background: Option<crate::render::Color>,
    /// Raw pointer to the (type-erased) handler. `UIApplicationMain` never
    /// returns, so the handler in `run`'s frame lives for the process lifetime.
    handler: *mut DynHandler,
    /// The touch that is also the pointer: the first finger down, until it
    /// lifts.
    primary: Option<u64>,
    /// Whether a text input has focus, so the view holds the keyboard.
    text_input: bool,
    /// The next frame's background (its top-left pixel) goes around the safe
    /// area, after the theme changed look.
    match_background: bool,
}

thread_local! {
    static CTX: RefCell<IosCtx> = RefCell::new(IosCtx::default());
}

/// `drawRect:` for our UIView subclass: blit the framebuffer as a CGImage.
extern "C" fn draw_rect(_this: Id, _cmd: Sel, _rect: CgRect) {
    CTX.with(|c| {
        let c = c.borrow();
        if c.fb.is_empty() {
            return;
        }
        unsafe {
            let cg = UIGraphicsGetCurrentContext();
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
            // The context is in points; the framebuffer is in pixels.
            let (width, height) = (c.w as f64 / c.scale, c.h as f64 / c.scale);
            let rect = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize { width, height },
            };
            // The UIView draw context draws CGImages bottom-up; flip the CTM so
            // our top-row-first framebuffer lands upright (as in the macОS view).
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

/// `color` as a `UIColor` (the system background for none).
unsafe fn ui_color(color: Option<crate::render::Color>) -> Id {
    unsafe {
        match color {
            Some(color) => {
                let c = |v: u8| v as f64 / 255.0;
                let f: unsafe extern "C" fn(Id, Sel, f64, f64, f64, f64) -> Id =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(
                    class("UIColor"),
                    sel("colorWithRed:green:blue:alpha:"),
                    c(color.r),
                    c(color.g),
                    c(color.b),
                    c(color.a),
                )
            }
            None => msg_id(class("UIColor"), sel("systemBackgroundColor")),
        }
    }
}

/// Show `color` around the safe area: the background of `window`'s root view.
unsafe fn set_root_background(window: Id, color: Option<crate::render::Color>) {
    if window.is_null() {
        return;
    }
    unsafe {
        let root = msg_id(msg_id(window, sel("rootViewController")), sel("view"));
        if !root.is_null() {
            msg_void_id(root, sel("setBackgroundColor:"), ui_color(color));
        }
    }
}

/// `application:didFinishLaunchingWithOptions:` — build the window + view, draw
/// the first frame, and show it.
extern "C" fn did_finish_launching(_this: Id, _cmd: Sel, _app: Id, _opts: Id) -> bool {
    unsafe {
        let bounds = msg_rect(msg_id(class("UIScreen"), sel("mainScreen")), sel("bounds"));
        let scale = msg_f64(msg_id(class("UIScreen"), sel("mainScreen")), sel("scale"));
        let window = msg_alloc_init_frame(class("UIWindow"), bounds);
        // The root view fills the screen behind the status bar and home indicator;
        // our view sits inside the safe area on top of it.
        let root = msg_alloc_init_frame(class("UIView"), bounds);
        msg_void_id(
            root,
            sel("setBackgroundColor:"),
            ui_color(CTX.with(|c| c.borrow().background)),
        );
        let view = msg_alloc_init_frame(register_view_class(), bounds);
        // Every finger, not just the first.
        msg_void_bool(view, sel("setMultipleTouchEnabled:"), true);
        msg_void_id(root, sel("addSubview:"), view);
        observe_reduce_motion(view);
        lifecycle::observe(view);
        let vc = msg_id(msg_id(class("UIViewController"), sel("alloc")), sel("init"));
        msg_void_id(vc, sel("setView:"), root);
        msg_void_id(window, sel("setRootViewController:"), vc);

        CTX.with(|c| {
            let mut c = c.borrow_mut();
            c.view = view;
            c.window = window;
            c.scale = scale;
        });

        // Show the window first: the safe area is only known once it's visible,
        // and OS-drawn controls are captured through it.
        msg_void(window, sel("makeKeyAndVisible"));
        let safe = msg_insets(window, sel("safeAreaInsets"));
        let frame = CgRect {
            origin: CgPoint {
                x: bounds.origin.x + safe.left,
                y: bounds.origin.y + safe.top,
            },
            size: CgSize {
                width: bounds.size.width - safe.left - safe.right,
                height: bounds.size.height - safe.top - safe.bottom,
            },
        };
        msg_void_rect(view, sel("setFrame:"), frame);

        let size = PhysicalSize::new(
            (frame.size.width * scale).round() as u32,
            (frame.size.height * scale).round() as u32,
        );
        CTX.with(|c| c.borrow_mut().size = size);
        let win = IosWindow { size };
        // The app starts with its requested size; on iOS the screen decides.
        call_handler(Event::Resized(size), &win);
        call_handler(Event::RedrawRequested, &win);

        // UIKit owns the run loop, so wakeups and timers go through the main
        // dispatch queue, which it drains.
        crate::platform::wake::install(
            std::sync::Arc::new(|| {
                dispatch_async_f(main_queue(), std::ptr::null_mut(), on_wake);
            }),
            Some(std::sync::Arc::new(|at: std::time::Instant| {
                let delay = at.saturating_duration_since(std::time::Instant::now());
                let ns = delay.as_nanos().min(i64::MAX as u128) as i64;
                dispatch_after_f(
                    dispatch_time(DISPATCH_TIME_NOW, ns),
                    main_queue(),
                    std::ptr::null_mut(),
                    on_wake,
                );
            })),
        );
        // Animation frames are paced by the screen, through a CADisplayLink.
        display_link::install();
        msg_void_bool(view, sel("setNeedsDisplay"), true);

        // Runtime marker for CI: confirms the UIKit backend booted, built the
        // window, and the handler rendered a frame into the shared framebuffer.
        // Written to the app's tmp dir (which CI reads back from the simulator
        // container) since app stdout isn't reliably captured headlessly.
        let (w, h) = CTX.with(|c| {
            let c = c.borrow();
            (c.w, c.h)
        });
        let marker = format!("Stipple iOS: window shown, framebuffer {w}x{h}");
        println!("{marker}");
        let _ = std::fs::write(std::env::temp_dir().join("stipple-ios.txt"), &marker);
    }
    true
}

/// Each touch in `touches` (an `NSSet` of `UITouch`): its id (the `UITouch`,
/// which stays the same object while the finger is down) and its position in
/// `view`.
unsafe fn touches_in(view: Id, touches: Id) -> Vec<(u64, Point)> {
    unsafe {
        let location: unsafe extern "C" fn(Id, Sel, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        set_objects(touches)
            .into_iter()
            .map(|touch| {
                let p = location(touch, sel("locationInView:"), view);
                (touch as u64, Point::new(p.x, p.y))
            })
            .collect()
    }
}

fn current_window() -> IosWindow {
    IosWindow {
        size: CTX.with(|c| c.borrow().size),
    }
}

/// Deliver touches that changed: each as an [`Event::Touch`], then the
/// pointer events of the first finger, which is also the pointer.
fn touch_input(phase: TouchPhase, touches: &[(u64, Point)]) {
    let win = current_window();
    for &(id, position) in touches {
        call_handler(
            Event::Touch(Touch {
                id,
                phase,
                position,
            }),
            &win,
        );
    }
    let primary = CTX.with(|c| c.borrow().primary);
    let pressed = |position| Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Pressed,
        position,
    };
    let released = |position| Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Released,
        position,
    };
    match phase {
        TouchPhase::Started => {
            if let (None, Some(&(id, position))) = (primary, touches.first()) {
                CTX.with(|c| c.borrow_mut().primary = Some(id));
                call_handler(Event::PointerMoved { position }, &win);
                call_handler(pressed(position), &win);
            }
        }
        TouchPhase::Moved => {
            if let Some(&(_, position)) = touches.iter().find(|t| Some(t.0) == primary) {
                call_handler(Event::PointerMoved { position }, &win);
            }
        }
        TouchPhase::Ended | TouchPhase::Cancelled => {
            if let Some(&(_, position)) = touches.iter().find(|t| Some(t.0) == primary) {
                CTX.with(|c| c.borrow_mut().primary = None);
                // A cancelled touch is released far away, so it doesn't click.
                let at = if phase == TouchPhase::Cancelled {
                    Point::new(-1.0e9, -1.0e9)
                } else {
                    position
                };
                call_handler(released(at), &win);
                // A finger that lifts leaves no hover behind.
                call_handler(Event::PointerLeft, &win);
            }
        }
    }
}

extern "C" fn touches_began(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    touch_input(TouchPhase::Started, &unsafe { touches_in(this, touches) });
}

extern "C" fn touches_moved(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    touch_input(TouchPhase::Moved, &unsafe { touches_in(this, touches) });
}

extern "C" fn touches_ended(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    touch_input(TouchPhase::Ended, &unsafe { touches_in(this, touches) });
}

extern "C" fn touches_cancelled(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    touch_input(TouchPhase::Cancelled, &unsafe { touches_in(this, touches) });
}

// ---- The keyboard (UIKeyInput / UITextInput, in `text_input`) ----------------

/// `canBecomeFirstResponder`: the view takes the keyboard while a text input
/// has focus.
extern "C" fn can_become_first_responder(_this: Id, _cmd: Sel) -> bool {
    true
}

fn key_tap(code: KeyCode, modifiers: Modifiers, win: &IosWindow) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        call_handler(
            Event::Key {
                code,
                state,
                modifiers,
            },
            win,
        );
    }
}

/// `pressesBegan:withEvent:` and friends, from a hardware keyboard while the
/// view is first responder. Keys the text system doesn't type (arrows,
/// Home/End, Escape, Tab, F-keys, shortcuts) become [`Event::Key`]; the rest go
/// on to UIKit, which types them through `insertText:` / `deleteBackward`.
unsafe fn presses(this: Id, cmd: Sel, presses: Id, event: Id, state: ButtonState) {
    let win = current_window();
    let mut to_uikit = false;
    for press in unsafe { set_objects(presses) } {
        let key = unsafe { msg_id(press, sel("key")) };
        if key.is_null() {
            to_uikit = true;
            continue;
        }
        let usage = unsafe { msg_usize(key, sel("keyCode")) } as isize;
        let modifiers = input::ios_modifiers(unsafe { msg_usize(key, sel("modifierFlags")) });
        let chars = unsafe { string_of(msg_id(key, sel("charactersIgnoringModifiers"))) };
        // Raw first: a key the app uses goes neither on as a `Key` nor to
        // UIKit's text system.
        let typed = unsafe { string_of(msg_id(key, sel("characters"))) };
        if raw_key(usage, state, modifiers, &typed, &chars, &win) {
            continue;
        }
        match input::ios_key(usage, &chars, modifiers) {
            Some(code) => call_handler(
                Event::Key {
                    code,
                    state,
                    modifiers,
                },
                &win,
            ),
            None => to_uikit = true,
        }
    }
    if to_uikit {
        unsafe {
            let sup = ObjcSuper {
                receiver: this,
                super_class: class("UIView"),
            };
            let f: unsafe extern "C" fn(*const ObjcSuper, Sel, Id, Id) =
                std::mem::transmute(objc_msgSendSuper as *const c_void);
            f(&sup, cmd, presses, event);
        }
    }
}

thread_local! {
    /// Left/right modifiers, from the modifier keys' own presses.
    static SIDES: std::cell::Cell<crate::platform::key::SideTracker> =
        std::cell::Cell::new(crate::platform::key::SideTracker::default());
}

/// Deliver a hardware key press or release (`UIKey`, whose `keyCode` is the
/// HID usage) as a raw key. Returns whether the app used it.
fn raw_key(
    usage: isize,
    state: ButtonState,
    modifiers: Modifiers,
    typed: &str,
    ignoring: &str,
    win: &IosWindow,
) -> bool {
    use crate::platform::key::{KeyEvent, PhysicalKey, deliver_raw, key_text};
    let physical = PhysicalKey(u16::try_from(usage).unwrap_or(0));
    let pressed = state == ButtonState::Pressed;
    let logical = input::raw_logical(None, physical, Some(ignoring), usage as u32);
    let mut ev = KeyEvent::new(state, logical, physical);
    ev.modifiers = modifiers;
    ev.sides = SIDES.with(|s| {
        let mut t = s.get();
        let sides = t.update(physical, state, modifiers);
        s.set(t);
        sides
    });
    if pressed {
        ev.text_ignoring_modifiers = key_text(ignoring);
        ev.text = if modifiers.ctrl || modifiers.meta {
            ev.text_ignoring_modifiers.clone()
        } else {
            key_text(typed)
        };
    }
    ev.scancode = usage as u32;
    let ptr = CTX.with(|c| c.borrow().handler);
    if ptr.is_null() || IN_HANDLER.with(|h| h.get()) {
        return false;
    }
    IN_HANDLER.with(|h| h.set(true));
    let used = deliver_raw(ev, |e| unsafe {
        let fat = *ptr;
        let handler: &mut dyn FnMut(Event, &dyn Window) -> ControlFlow = &mut *fat;
        handler(e, win)
    })
    .0;
    IN_HANDLER.with(|h| h.set(false));
    used
}

extern "C" fn presses_began(this: Id, cmd: Sel, set: Id, event: Id) {
    unsafe { presses(this, cmd, set, event, ButtonState::Pressed) }
}

extern "C" fn presses_ended(this: Id, cmd: Sel, set: Id, event: Id) {
    unsafe { presses(this, cmd, set, event, ButtonState::Released) }
}

/// Hold the keyboard (become first responder) while a text input has focus,
/// and let it go when focus leaves.
fn set_text_input(active: bool) {
    let (view, was) = CTX.with(|c| {
        let mut c = c.borrow_mut();
        let was = c.text_input;
        c.text_input = active;
        (c.view, was)
    });
    if view.is_null() || was == active {
        return;
    }
    unsafe {
        if active {
            msg_bool(view, sel("becomeFirstResponder"));
        } else {
            msg_bool(view, sel("resignFirstResponder"));
        }
    }
}

/// Drive the view the way UIKit does, for self-tests on a simulator, which
/// can't script touches or the keyboard: a touch through the same path as
/// `touchesBegan:withEvent:` and friends, and text and Backspace through
/// `insertText:` and `deleteBackward` messages sent to the view, composition
/// through its `UITextInput` methods, and VoiceOver's activation through its
/// accessibility elements. The actions run on the main queue after the current
/// event, so they can be called from an app's callbacks; they run in the order
/// they're called. The readers (`*_status`, [`accessibility_elements`]) run at
/// once.
pub mod simulate {
    use super::*;

    fn view() -> Id {
        CTX.with(|c| c.borrow().view)
    }

    /// Compose `text` as an input method does:
    /// `[view setMarkedText:text selectedRange:selected]` (UTF-16 units
    /// within `text`). An empty `text` ends the composition.
    pub fn set_marked_text(text: &str, selected: (usize, usize)) {
        let text = text.to_string();
        later(move || {
            let view = view();
            if view.is_null() {
                return;
            }
            let range = text_input::NsRange {
                location: selected.0,
                length: selected.1.saturating_sub(selected.0),
            };
            unsafe {
                let f: unsafe extern "C" fn(Id, Sel, Id, text_input::NsRange) =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(
                    view,
                    sel("setMarkedText:selectedRange:"),
                    nsstring(&text),
                    range,
                );
            }
        });
    }

    /// Commit the composition as it stands, as `[view unmarkText]`.
    pub fn unmark_text() {
        later(|| {
            let view = view();
            if !view.is_null() {
                unsafe { msg_void(view, sel("unmarkText")) };
            }
        });
    }

    /// Replace UTF-16 offsets `start..end` of the document with `text`, as
    /// autocorrect does: `[view replaceRange:withText:]`, the range made
    /// through the view's own `UITextInput` position methods.
    pub fn replace_range(start: usize, end: usize, text: &str) {
        let text = text.to_string();
        later(move || {
            let view = view();
            if !view.is_null() {
                unsafe { text_input::replace(view, start, end, &text) };
            }
        });
    }

    /// The view's `UITextInput` state, read through objc dispatch as the
    /// keyboard reads it: whether it conforms, the document text, the
    /// selected range, the marked text, `firstRectForRange:` (of the marked
    /// text, else the selection) and `caretRectForPosition:`.
    pub fn text_input_status() -> String {
        let view = view();
        if view.is_null() {
            return "no view".into();
        }
        crate::platform::native::objc::with_pool(|| unsafe { text_input::status(view) })
    }

    /// The view's accessibility elements, read through objc dispatch as
    /// VoiceOver reads them (`accessibilityElementCount`,
    /// `accessibilityElementAtIndex:`, then each one's label, value, traits
    /// and `accessibilityFrame`).
    pub fn accessibility_elements() -> Vec<AccessibilityElement> {
        let view = view();
        if view.is_null() {
            return Vec::new();
        }
        crate::platform::native::objc::with_pool(|| unsafe { a11y::read_elements(view) })
    }

    /// Activate the element at `index`, as VoiceOver's double tap does
    /// (`accessibilityActivate`).
    pub fn activate_accessibility_element(index: usize) {
        later(move || {
            let view = view();
            if !view.is_null() {
                let done = unsafe { a11y::activate_at(view, index) };
                crate::platform::window::debug_window(format_args!(
                    "accessibilityActivate {index}: {done}"
                ));
            }
        });
    }

    /// The view's frame on screen, in points (where accessibility frames are),
    /// converted through its window to the screen's coordinate space.
    pub fn view_frame_on_screen() -> crate::geometry::Rect {
        let view = view();
        if view.is_null() {
            return crate::geometry::Rect::ZERO;
        }
        let r = unsafe {
            let b = msg_rect(view, sel("bounds"));
            a11y::to_screen(view, (b.origin.x, b.origin.y, b.size.width, b.size.height))
        };
        crate::geometry::Rect::from_xywh(r.origin.x, r.origin.y, r.size.width, r.size.height)
    }

    /// Post `UIApplicationDidReceiveMemoryWarningNotification`, as the system
    /// does when memory runs short.
    pub fn memory_warning() {
        later(lifecycle::post_memory_warning);
    }

    /// Read the app's notification permission from `UNUserNotificationCenter`
    /// (`not_determined`, `denied`, `authorized`, …); `done` is called on
    /// another thread with the answer.
    pub fn notification_status(done: fn(&'static str)) {
        crate::platform::shell::ios::authorization_status(done);
    }

    /// The class name of the view controller presented on top (a document
    /// picker is `UIDocumentPickerViewController`), or `""`.
    pub fn presented_controller() -> String {
        crate::platform::dialog::picker_ios::presented_class()
    }

    /// Cancel the document picker on top, as its Cancel button would: the
    /// app's callback gets `None` (or `false`, for a save). `false` if no
    /// picker is showing.
    pub fn cancel_document_picker() -> bool {
        crate::platform::dialog::picker_ios::cancel_presented()
    }

    /// A finger `id` at `position` (logical pixels, in the app's content).
    pub fn touch(id: u64, phase: TouchPhase, position: Point) {
        later(move || touch_input(phase, &[(id, position)]));
    }

    /// Type `text`, as `[view insertText:text]`.
    pub fn insert_text(text: &str) {
        let text = text.to_string();
        later(move || {
            let view = CTX.with(|c| c.borrow().view);
            if !view.is_null() {
                unsafe { msg_void_id(view, sel("insertText:"), nsstring(&text)) };
            }
        });
    }

    /// Press the keyboard's delete key, as `[view deleteBackward]`.
    pub fn delete_backward() {
        later(|| {
            let view = CTX.with(|c| c.borrow().view);
            if !view.is_null() {
                unsafe { msg_void(view, sel("deleteBackward")) };
            }
        });
    }

    /// The view's keyboard state, read through objc dispatch: whether it
    /// conforms to `UIKeyInput`, can become and is first responder (holding
    /// the keyboard), and has text.
    pub fn keyboard_status() -> String {
        let view = CTX.with(|c| c.borrow().view);
        if view.is_null() {
            return "no view".into();
        }
        unsafe {
            let protocol = objc_getProtocol(c"UIKeyInput".as_ptr());
            let conforms = {
                let f: unsafe extern "C" fn(Id, Sel, *mut c_void) -> i8 =
                    std::mem::transmute(objc_msgSend as *const c_void);
                !protocol.is_null() && f(view, sel("conformsToProtocol:"), protocol) != 0
            };
            format!(
                "key_input={conforms} can_become={} first_responder={} has_text={}",
                msg_bool(view, sel("canBecomeFirstResponder")),
                msg_bool(view, sel("isFirstResponder")),
                msg_bool(view, sel("hasText")),
            )
        }
    }
}

/// Run `f` on the main queue, after whatever is running now.
fn later(f: impl FnOnce() + 'static) {
    extern "C" fn trampoline(context: *mut c_void) {
        let f = unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce()>>()) };
        f();
    }
    let f: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe { dispatch_async_f(main_queue(), Box::into_raw(f).cast(), trampoline) };
}

// libdispatch, part of libSystem.
const DISPATCH_TIME_NOW: u64 = 0;
unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_time(when: u64, delta_ns: i64) -> u64;
}

/// `dispatch_get_main_queue()`, which is a macro over this symbol.
fn main_queue() -> *const c_void {
    (&raw const _dispatch_main_q).cast()
}

/// Runs on the main queue: deliver a wake if one is due. A timer that was
/// rescheduled may still fire here early, which `take` sees and ignores.
extern "C" fn on_wake(_context: *mut c_void) {
    if crate::platform::wake::take(std::time::Instant::now()) {
        call_handler(Event::Wake, &current_window());
    }
}

thread_local! {
    /// Whether the handler is running. UIKit calls back into the view from
    /// inside calls the handler makes (resigning first responder commits the
    /// keyboard's marked text, for one); what those deliver waits here.
    static IN_HANDLER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DEFERRED: RefCell<std::collections::VecDeque<Event>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}

/// Invoke the stashed Rust handler (if any). Called from inside the handler,
/// the event is delivered once it returns.
fn call_handler(event: Event, win: &dyn Window) {
    let ptr = CTX.with(|c| c.borrow().handler);
    if ptr.is_null() {
        return;
    }
    if IN_HANDLER.with(|h| h.get()) {
        DEFERRED.with(|d| d.borrow_mut().push_back(event));
        later(|| {
            while let Some(event) = DEFERRED.with(|d| d.borrow_mut().pop_front()) {
                call_handler(event, &current_window());
            }
        });
        return;
    }
    IN_HANDLER.with(|h| h.set(true));
    unsafe {
        let fat = *ptr;
        let handler: &mut dyn FnMut(Event, &dyn Window) -> ControlFlow = &mut *fat;
        let _ = handler(event, win);
    }
    IN_HANDLER.with(|h| h.set(false));
}

/// `stippleMotionChanged:` — *Reduce Motion* was switched; the next wake
/// reports it to the App, which reads the setting again.
extern "C" fn motion_changed(_this: Id, _cmd: Sel, _notification: Id) {
    crate::platform::native::note_changed();
}

/// Have `view` hear `UIAccessibilityReduceMotionStatusDidChangeNotification`.
///
/// # Safety
/// `view` must be a live `StippleUIView`, which stays alive for the process.
unsafe fn observe_reduce_motion(view: Id) {
    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        static UIAccessibilityReduceMotionStatusDidChangeNotification: Id;
    }
    let add: unsafe extern "C" fn(Id, Sel, Id, Sel, Id, Id) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe {
        let center = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        add(
            center,
            sel("addObserver:selector:name:object:"),
            view,
            sel("stippleMotionChanged:"),
            UIAccessibilityReduceMotionStatusDidChangeNotification,
            std::ptr::null_mut(),
        );
    }
}

fn register_view_class() -> Class {
    let name = c"StippleUIView";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("UIView"), name.as_ptr(), 0);
        let draw_types = c"v@:{CGRect={CGPoint=dd}{CGSize=dd}}";
        class_addMethod(
            cls,
            sel("drawRect:"),
            draw_rect as *const c_void,
            draw_types.as_ptr(),
        );
        let touch_types = c"v@:@@";
        let touches: [(&str, extern "C" fn(Id, Sel, Id, Id)); 7] = [
            ("touchesBegan:withEvent:", touches_began),
            ("touchesMoved:withEvent:", touches_moved),
            ("touchesEnded:withEvent:", touches_ended),
            ("touchesCancelled:withEvent:", touches_cancelled),
            ("pressesBegan:withEvent:", presses_began),
            ("pressesEnded:withEvent:", presses_ended),
            ("pressesCancelled:withEvent:", presses_ended),
        ];
        for (name, imp) in touches {
            class_addMethod(cls, sel(name), imp as *const c_void, touch_types.as_ptr());
        }
        // The keyboard: UITextInput, so the view can hold it as first
        // responder and input methods compose.
        class_addMethod(
            cls,
            sel("canBecomeFirstResponder"),
            can_become_first_responder as *const c_void,
            c"B@:".as_ptr(),
        );
        text_input::add_methods(cls);
        a11y::add_methods(cls);
        lifecycle::add_methods(cls);
        class_addMethod(
            cls,
            sel("stippleMotionChanged:"),
            motion_changed as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

fn register_app_delegate() -> Class {
    let name = c"StippleAppDelegate";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("UIResponder"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("application:didFinishLaunchingWithOptions:"),
            did_finish_launching as *const c_void,
            c"c@:@@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

#[derive(Debug)]
struct IosWindow {
    size: PhysicalSize,
}

impl Window for IosWindow {
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
                unsafe { msg_void_bool(view, sel("setNeedsDisplay"), true) };
            }
        });
    }
    fn set_title(&self, _title: &str) {}
    fn clipboard(&self) -> Option<String> {
        crate::platform::clipboard::text()
    }
    fn set_clipboard(&self, text: &str) {
        crate::platform::clipboard::set_text(text);
    }
    fn set_text_input_active(&self, active: bool) {
        set_text_input(active);
    }
    fn set_ime(&self, state: Option<&crate::platform::ImeState>) {
        text_input::set_state(state);
    }
    fn set_accessibility_tree(&self, root: &crate::platform::access::A11yNode) {
        a11y::set_tree(root);
    }
    fn set_dark(&self, _dark: bool) {
        // The theme changed look: the area around the safe area takes the
        // new background from the next frame.
        CTX.with(|c| c.borrow_mut().match_background = true);
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(IosSurface { size: self.size })
    }
}

#[derive(Debug)]
struct IosSurface {
    size: PhysicalSize,
}

impl Surface for IosSurface {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, pixmap: &Pixmap, _damage: &[crate::geometry::Rect]) {
        let size = pixmap.size();
        let (view, window, background) = CTX.with(|c| {
            let mut c = c.borrow_mut();
            c.fb = pixmap.as_bytes().to_vec();
            c.w = size.width as usize;
            c.h = size.height as usize;
            let mut background = None;
            if std::mem::take(&mut c.match_background) && c.fb.len() >= 4 {
                let [r, g, b, a] = [c.fb[0], c.fb[1], c.fb[2], c.fb[3]];
                c.background = Some(crate::render::Color { r, g, b, a });
                background = c.background;
            }
            (c.view, c.window, background)
        });
        // Messaged with the context released: UIKit may call the view back.
        if background.is_some() {
            unsafe { set_root_background(window, background) };
        }
        if !view.is_null() {
            unsafe { msg_void_bool(view, sel("setNeedsDisplay"), true) };
        }
    }
}

/// Boot the iOS app: stash the handler, register our classes, and hand control
/// to `UIApplicationMain` (which does not return).
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let size = ScaleFactor::IDENTITY.to_physical(attrs.logical_size);

    // Type-erase the handler and stash a raw pointer. `H` is not `'static`, but
    // `UIApplicationMain` never returns, so `handler` lives for the process
    // lifetime — we launder the borrow lifetime to the `'static` raw pointer the
    // stash holds, which is therefore sound in practice.
    let dyn_ref: &mut (dyn FnMut(Event, &dyn Window) -> ControlFlow + '_) = &mut handler;
    let fat: DynHandler = unsafe { std::mem::transmute(dyn_ref) };
    let boxed: *mut DynHandler = Box::into_raw(Box::new(fat));

    CTX.with(|c| {
        let mut c = c.borrow_mut();
        c.size = size;
        c.background = attrs.background;
        c.handler = boxed;
    });

    register_view_class();
    register_app_delegate();

    unsafe {
        let delegate_name = nsstring("StippleAppDelegate");
        // UIApplicationMain owns argv for the process lifetime; pass none.
        UIApplicationMain(0, std::ptr::null_mut(), std::ptr::null_mut(), delegate_name);
    }
    Ok(())
}
