//! An iPad's trackpad and mouse: scrolling, hovering, secondary clicks and
//! the pointer's shape.
//!
//! - **Scrolling**: a `UIPanGestureRecognizer` taking only indirect scroll
//!   input (`allowedScrollTypesMask` = continuous and discrete, iOS 13.4
//!   and later; `allowedTouchTypes` empty, so fingers on the screen never
//!   reach it and touches stay the app's). Two fingers on a trackpad, or a
//!   mouse's wheel, become [`Event::Scroll`] in points (see
//!   [`pan_scroll`](super::pan_scroll)), after an [`Event::PointerMoved`] to
//!   where the pointer is, so the container under it scrolls. Fingers that
//!   lift while moving (the recognizer's `velocityInView:` at its end) leave
//!   the scroll coasting ([`Momentum`] steps about every 16 ms, on the main
//!   queue), as a `UIScrollView` does, until it slows down, the fingers
//!   scroll again or touch the screen.
//! - **Hovering**: a `UIHoverGestureRecognizer` (iOS 13 and later), whose
//!   moves are [`Event::PointerMoved`] while no finger (or click) is down,
//!   and [`Event::PointerLeft`] when the pointer leaves the view or hides.
//! - **Secondary clicks**: a trackpad's two-finger click or a mouse's right
//!   button comes as a `UITouch` of type `UITouchTypeIndirectPointer` whose
//!   event's `buttonMask` has the secondary button (iOS 13.4 and later, with
//!   `UIApplicationSupportsIndirectInputEvents` in the app's Info.plist):
//!   that touch is [`PointerButton::Right`] pressed and released (opening
//!   the context menu there), not a touch.
//! - **The pointer's shape**: a `UIPointerInteraction` (iOS 13.4 and later)
//!   whose delegate gives the app's cursor ([`Window::set_cursor`](crate::platform::Window::set_cursor))
//!   as a `UIPointerStyle`: the I-beam over text, hidden for
//!   [`CursorIcon::Hidden`], the system's round pointer otherwise.
//!
//! Without a trackpad or mouse none of it ever fires.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use super::pan_scroll::{self, Momentum, PanScroll};
use super::{
    CTX, CgPoint, Class, Id, Sel, call_handler, class, class_addMethod, current_window, msg_id,
    msg_void, msg_void_id, objc_allocateClassPair, objc_getClass, objc_msgSend,
    objc_registerClassPair, sel,
};
use crate::geometry::Point;
use crate::platform::CursorIcon;
use crate::platform::event::{ButtonState, Event, PointerButton, TouchPhase};

/// `UIScrollTypeMaskDiscrete | UIScrollTypeMaskContinuous`.
const SCROLL_TYPES_ALL: isize = 3;

thread_local! {
    static PAN: RefCell<PanScroll> = RefCell::default();
    /// The pointer hovers the view (the app was told where).
    static HOVERING: Cell<bool> = const { Cell::new(false) };
    /// The recognizers, for [`status`].
    static RECOGNIZERS: Cell<(Id, Id)> =
        const { Cell::new((std::ptr::null_mut(), std::ptr::null_mut())) };
    /// The scroll coasting after the fingers lifted, and when they did.
    static COAST: RefCell<Option<(Momentum, std::time::Instant)>> = const { RefCell::new(None) };
    /// A step of it is scheduled, and how many ran (for self-tests).
    static COAST_TICKING: Cell<bool> = const { Cell::new(false) };
    static COAST_STEPS: Cell<u32> = const { Cell::new(0) };
    /// The touch of the secondary click in progress (its `UITouch`), or 0.
    static SECONDARY: Cell<u64> = const { Cell::new(0) };
    /// The pointer interaction and the app's cursor.
    static POINTER: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
    static CURSOR: Cell<CursorIcon> = const { Cell::new(CursorIcon::Default) };
}

/// `UITouchTypeIndirectPointer`.
const TOUCH_INDIRECT_POINTER: isize = 3;
/// `UIEventButtonMaskSecondary`.
const BUTTON_SECONDARY: isize = 1 << 1;
/// `UIAxisVertical`.
const AXIS_VERTICAL: isize = 2;

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
        let pointer_class = class("UIPointerInteraction");
        if !pointer_class.is_null() {
            let delegate = msg_id(
                msg_id(register_pointer_delegate(), sel("alloc")),
                sel("init"),
            );
            let with_delegate: unsafe extern "C" fn(Id, Sel, Id) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            let interaction = with_delegate(
                msg_id(pointer_class, sel("alloc")),
                sel("initWithDelegate:"),
                delegate,
            );
            if !interaction.is_null() {
                msg_void_id(view, sel("addInteraction:"), interaction);
                POINTER.with(|p| p.set(interaction));
            }
        }
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
    let velocity = if state == pan_scroll::ENDED {
        unsafe { read(recognizer, "velocityInView:").1 }
    } else {
        Point::new(0.0, 0.0)
    };
    scrolled(state, location, translation, velocity);
}

/// A scroll step: `translation` since the gesture began, the pointer at
/// `location` (both in points); at the end, the fingers' `velocity`
/// (points per second), from which the scroll coasts on.
pub(super) fn scrolled(state: isize, location: Point, translation: Point, velocity: Point) {
    let win = current_window();
    if state == pan_scroll::BEGAN {
        stop_momentum();
        call_handler(Event::PointerMoved { position: location }, &win);
    }
    let delta = PAN.with(|p| p.borrow_mut().update(state, translation.x, translation.y));
    if let Some(delta) = delta {
        call_handler(Event::Scroll { delta }, &win);
    }
    if state == pan_scroll::ENDED
        && let Some(coast) = Momentum::new(velocity.x, velocity.y)
    {
        COAST.with(|c| *c.borrow_mut() = Some((coast, std::time::Instant::now())));
        set_coasting(true);
    }
}

/// Stop the scroll coasting, if it is (fingers on the screen, or scrolling
/// again).
pub(super) fn stop_momentum() {
    if COAST.with(|c| c.borrow_mut().take()).is_some() {
        set_coasting(false);
    }
}

/// How often the coast steps: about a 60 Hz screen's refresh.
const COAST_STEP: std::time::Duration = std::time::Duration::from_micros(16_667);

/// Step the coast now and then every [`COAST_STEP`] while it lasts (`on`),
/// on the main queue. Off, the pending step finds nothing to do.
fn set_coasting(on: bool) {
    if on && !COAST_TICKING.with(|t| t.replace(true)) {
        schedule_coast();
    }
}

fn schedule_coast() {
    let ns = COAST_STEP.as_nanos() as i64;
    unsafe {
        super::dispatch_after_f(
            super::dispatch_time(super::DISPATCH_TIME_NOW, ns),
            super::main_queue(),
            std::ptr::null_mut(),
            coast_action,
        );
    }
}

/// A step of the coast, on the main queue.
extern "C" fn coast_action(_context: *mut c_void) {
    let step = COAST.with(|c| {
        let mut c = c.borrow_mut();
        let (coast, since) = c.as_mut()?;
        let delta = coast.step(since.elapsed().as_secs_f64());
        if coast.done() {
            *c = None;
        }
        delta
    });
    match step {
        Some(delta) => {
            COAST_STEPS.with(|n| n.set(n.get() + 1));
            call_handler(Event::Scroll { delta }, &current_window());
            schedule_coast();
        }
        None => COAST_TICKING.with(|t| t.set(false)),
    }
}

/// Whether the coast is on, and the steps it took so far, for self-tests.
pub(super) fn coasting() -> String {
    format!(
        "{} coast_steps={}",
        COAST.with(|c| c.borrow().is_some()),
        COAST_STEPS.with(Cell::get)
    )
}

/// A secondary click's button at `at` (points): pressed (after a move
/// there) or released.
pub(super) fn secondary(at: Point, state: ButtonState) {
    let win = current_window();
    if state == ButtonState::Pressed {
        call_handler(Event::PointerMoved { position: at }, &win);
    }
    call_handler(
        Event::PointerButton {
            button: PointerButton::Right,
            state,
            position: at,
        },
        &win,
    );
}

/// Take a secondary click's touch out of `touches` (each a `UITouch`'s id
/// and position, from `event`): a touch that begins as one (an indirect
/// pointer's, with the secondary button down) is the right button pressed,
/// its moves the pointer's, its end the button released. Returns the
/// other touches, which are the app's.
///
/// # Safety
/// `event` must be the touches' `UIEvent` (or nil), the ids their live
/// `UITouch`es.
pub(super) unsafe fn without_secondary(
    phase: TouchPhase,
    touches: Vec<(u64, Point)>,
    event: Id,
) -> Vec<(u64, Point)> {
    let current = SECONDARY.with(Cell::get);
    if current == 0 && phase != TouchPhase::Started {
        return touches;
    }
    let secondary_down = || unsafe {
        let int: unsafe extern "C" fn(Id, Sel) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        let responds: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        !event.is_null()
            && responds(event, sel("respondsToSelector:"), sel("buttonMask")) != 0
            && int(event, sel("buttonMask")) & BUTTON_SECONDARY != 0
    };
    let mut rest = Vec::with_capacity(touches.len());
    for (id, at) in touches {
        match phase {
            TouchPhase::Started if current == 0 => {
                let indirect = unsafe {
                    let int: unsafe extern "C" fn(Id, Sel) -> isize =
                        std::mem::transmute(objc_msgSend as *const c_void);
                    int(id as Id, sel("type")) == TOUCH_INDIRECT_POINTER
                };
                if indirect && secondary_down() {
                    SECONDARY.with(|s| s.set(id));
                    secondary(at, ButtonState::Pressed);
                    continue;
                }
            }
            TouchPhase::Moved if id == current => {
                call_handler(Event::PointerMoved { position: at }, &current_window());
                continue;
            }
            TouchPhase::Ended | TouchPhase::Cancelled if id == current => {
                SECONDARY.with(|s| s.set(0));
                secondary(at, ButtonState::Released);
                continue;
            }
            _ => {}
        }
        rest.push((id, at));
    }
    rest
}

/// `Window::set_cursor`: the pointer interaction's style follows.
pub(super) fn set_cursor(cursor: CursorIcon) {
    if CURSOR.with(|c| c.replace(cursor)) != cursor {
        let interaction = POINTER.with(Cell::get);
        if !interaction.is_null() {
            unsafe { msg_void(interaction, sel("invalidate")) };
        }
    }
}

/// What the app's cursor looks like as a pointer style.
fn style_kind(cursor: CursorIcon) -> &'static str {
    match cursor {
        CursorIcon::Text => "beam",
        CursorIcon::Hidden => "hidden",
        _ => "default",
    }
}

/// The `UIPointerStyle` for the app's cursor (`nil`: the system's own).
unsafe fn pointer_style() -> Id {
    unsafe {
        match style_kind(CURSOR.with(Cell::get)) {
            "beam" => {
                let beam: unsafe extern "C" fn(Id, Sel, f64, isize) -> Id =
                    std::mem::transmute(objc_msgSend as *const c_void);
                let shape = beam(
                    class("UIPointerShape"),
                    sel("beamWithPreferredLength:axis:"),
                    20.0,
                    AXIS_VERTICAL,
                );
                if shape.is_null() {
                    return shape;
                }
                let style: unsafe extern "C" fn(Id, Sel, Id, isize) -> Id =
                    std::mem::transmute(objc_msgSend as *const c_void);
                style(
                    class("UIPointerStyle"),
                    sel("styleWithShape:constrainedAxes:"),
                    shape,
                    0,
                )
            }
            "hidden" => msg_id(class("UIPointerStyle"), sel("hiddenPointerStyle")),
            _ => std::ptr::null_mut(),
        }
    }
}

fn register_pointer_delegate() -> Class {
    let name = c"StipplePointerDelegate";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        extern "C" fn region(_this: Id, _cmd: Sel, _i: Id, _request: Id, default: Id) -> Id {
            default
        }
        extern "C" fn style(_this: Id, _cmd: Sel, _i: Id, _region: Id) -> Id {
            unsafe { pointer_style() }
        }
        class_addMethod(
            cls,
            sel("pointerInteraction:regionForRequest:defaultRegion:"),
            region as *const c_void,
            c"@@:@@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("pointerInteraction:styleForRegion:"),
            style as *const c_void,
            c"@@:@@".as_ptr(),
        );
        let protocol = super::objc_getProtocol(c"UIPointerInteractionDelegate".as_ptr());
        if !protocol.is_null() {
            super::class_addProtocol(cls, protocol);
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// For self-tests: the pointer interaction and the style its delegate gives
/// the app's cursor now, asked as UIKit asks.
pub(super) fn style_status() -> String {
    let interaction = POINTER.with(Cell::get);
    if interaction.is_null() {
        return "pointer_interaction=false".into();
    }
    unsafe {
        let delegate = msg_id(interaction, sel("delegate"));
        let ask: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let style = ask(
            delegate,
            sel("pointerInteraction:styleForRegion:"),
            interaction,
            std::ptr::null_mut(),
        );
        let cursor = CURSOR.with(Cell::get);
        format!(
            "pointer_interaction={} cursor={cursor:?} style={} uikit_style={}",
            msg_id(interaction, sel("view")) == super::app_view(),
            style_kind(cursor),
            !style.is_null(),
        )
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
