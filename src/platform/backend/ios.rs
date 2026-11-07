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
//! **Verification:** build-checked for `aarch64-apple-ios` by the `mobile`
//! cross-compile CI job; a simulator run + screenshot is a follow-up.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::{CString, c_void};

use crate::geometry::{PhysicalSize, Point, ScaleFactor};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, PointerButton, WindowId};
use crate::platform::window::{Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

/// Pacing animation frames by the screen (`CADisplayLink`).
mod display_link;

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
        let background = match CTX.with(|c| c.borrow().background) {
            Some(color) => {
                let (r, g, b, a) = (color.r, color.g, color.b, color.a);
                let c = |v: u8| v as f64 / 255.0;
                let f: unsafe extern "C" fn(Id, Sel, f64, f64, f64, f64) -> Id =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(
                    class("UIColor"),
                    sel("colorWithRed:green:blue:alpha:"),
                    c(r),
                    c(g),
                    c(b),
                    c(a),
                )
            }
            None => msg_id(class("UIColor"), sel("systemBackgroundColor")),
        };
        msg_void_id(root, sel("setBackgroundColor:"), background);
        let view = msg_alloc_init_frame(register_view_class(), bounds);
        msg_void_id(root, sel("addSubview:"), view);
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

/// Forward one touch as a pointer event, at its position in `view`.
unsafe fn touch_position(view: Id, touches: Id) -> Point {
    unsafe {
        let touch = msg_id(touches, sel("anyObject"));
        let f: unsafe extern "C" fn(Id, Sel, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = f(touch, sel("locationInView:"), view);
        Point::new(p.x, p.y)
    }
}

fn current_window() -> IosWindow {
    IosWindow {
        size: CTX.with(|c| c.borrow().size),
    }
}

extern "C" fn touches_began(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    let position = unsafe { touch_position(this, touches) };
    let win = current_window();
    call_handler(Event::PointerMoved { position }, &win);
    call_handler(
        Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Pressed,
            position,
        },
        &win,
    );
}

extern "C" fn touches_moved(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    let position = unsafe { touch_position(this, touches) };
    call_handler(Event::PointerMoved { position }, &current_window());
}

extern "C" fn touches_ended(this: Id, _cmd: Sel, touches: Id, _event: Id) {
    let position = unsafe { touch_position(this, touches) };
    call_handler(
        Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Released,
            position,
        },
        &current_window(),
    );
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

/// Invoke the stashed Rust handler (if any).
fn call_handler(event: Event, win: &dyn Window) {
    let ptr = CTX.with(|c| c.borrow().handler);
    if ptr.is_null() {
        return;
    }
    unsafe {
        let fat = *ptr;
        let handler: &mut dyn FnMut(Event, &dyn Window) -> ControlFlow = &mut *fat;
        let _ = handler(event, win);
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
        let touches: [(&str, extern "C" fn(Id, Sel, Id, Id)); 4] = [
            ("touchesBegan:withEvent:", touches_began),
            ("touchesMoved:withEvent:", touches_moved),
            ("touchesEnded:withEvent:", touches_ended),
            ("touchesCancelled:withEvent:", touches_ended),
        ];
        for (name, imp) in touches {
            class_addMethod(cls, sel(name), imp as *const c_void, touch_types.as_ptr());
        }
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
        CTX.with(|c| {
            let mut c = c.borrow_mut();
            c.fb = pixmap.as_bytes().to_vec();
            c.w = size.width as usize;
            c.h = size.height as usize;
            if !c.view.is_null() {
                unsafe { msg_void_bool(c.view, sel("setNeedsDisplay"), true) };
            }
        });
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
