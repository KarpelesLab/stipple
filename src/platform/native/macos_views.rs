//! AppKit: the live controls, real `NSControl`s in each window's view (see
//! [`views`](super::views)).
//!
//! Each [`NativeView`] is an `NSButton` (a push button, a checkbox or a
//! radio button), `NSSwitch`, `NSSlider` or `NSProgressIndicator`, a
//! subview of the window's view (which is flipped: frames are top-left, in
//! points, the window's logical pixels). A primary button is tinted with
//! the accent color (`bezelColor`) rather than made the window's default
//! button, so Return stays the app's. The controls refuse first responder:
//! the keyboard stays with the app's view, which draws focus itself. Their
//! target is one shared `StippleNativeTarget`; its action queues an
//! [`Event::NativeView`](crate::platform::Event::NativeView) for the window,
//! delivered once AppKit's event handling returns (a click's tracking loop
//! runs inside it). The backend leaves the mouse presses that land on a
//! control to AppKit ([`owns`]).
//!
//! A select is an `NSPopUpButton` (`macos_select`). The controls in a
//! scroll area are subviews of its clipping container (`apple_clips`),
//! which cuts one half scrolled out at the area's edge.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use super::apple_clips as clips;
use super::objc::{CgRect, Id, Sel, class, msg, nsstring, sel};
use super::os_select as select;
use super::{NativeControl, NativeView, NativeViewEvent, ViewBackend, ViewSet};

/// The live text fields.
#[path = "macos_text.rs"]
mod text;
pub(crate) use text::{editing as text_editing, poll as poll_text};

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct CgPoint {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

const BEZEL_ROUNDED: u64 = 1;
const BUTTON_MOMENTARY_PUSH_IN: u64 = 7;
const BUTTON_SWITCH: u64 = 3;
const BUTTON_RADIO: u64 = 4;

thread_local! {
    /// Each window's controls, by its view.
    static SETS: RefCell<HashMap<usize, ViewSet<Id>>> = RefCell::new(HashMap::new());
    static TARGET: std::cell::Cell<Id> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

fn target() -> Id {
    TARGET.with(|t| {
        if t.get().is_null() {
            let cls = unsafe {
                let name = c"StippleNativeTarget";
                let existing = objc_getClass(name.as_ptr());
                if existing.is_null() {
                    let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
                    class_addMethod(
                        cls,
                        sel("stippleNativeAction:"),
                        action as *const c_void,
                        c"v@:@".as_ptr(),
                    );
                    objc_registerClassPair(cls);
                    cls
                } else {
                    existing
                }
            };
            t.set(unsafe { msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) });
        }
        t.get()
    })
}

/// The window view and id of the control `sender`, with its kind and
/// whether it's a select; marked touched (the next list sets it to what the
/// app's state says, changed or not).
fn find(sender: Id) -> Option<(Id, u64, NativeControl, bool)> {
    SETS.with(|s| {
        let mut s = s.try_borrow_mut().ok()?;
        s.iter_mut().find_map(|(view, set)| {
            let id = set.id_of(&sender)?;
            let (v, _) = set.get(id)?;
            let found = (*view as Id, id, v.control, v.menu.is_some());
            set.touch(id);
            Some(found)
        })
    })
}

/// `stippleNativeAction:`: the user acted on `sender`.
extern "C" fn action(_this: Id, _cmd: Sel, sender: Id) {
    let Some((view, id, control, menu)) = find(sender) else {
        return;
    };
    let event = match control {
        _ if menu => match unsafe { select::picked(sender) } {
            Some(i) => NativeViewEvent::Picked(i),
            None => return,
        },
        NativeControl::Slider { .. } => {
            let v = unsafe { msg!(sender, "doubleValue", () -> f64) };
            NativeViewEvent::Value(v)
        }
        _ => NativeViewEvent::Activated,
    };
    crate::platform::backend::macos::queue_event(
        view,
        crate::platform::Event::NativeView { id, event },
    );
}

/// Where `v` goes in the view it is in (its scroll area's container, or
/// the window's view).
fn frame(v: &NativeView) -> CgRect {
    clips::frame_in_parent(v)
}

unsafe fn set_state(control: Id, on: bool) {
    let state = on as i64;
    unsafe { msg!(control, "setState:", (state: i64) -> ()) };
}

unsafe fn set_common(view: Id, v: &NativeView) {
    unsafe {
        let f = frame(v);
        msg!(view, "setFrame:", (f: CgRect) -> ());
        let enabled = v.enabled;
        // An `NSProgressIndicator` is no `NSControl`: it has no enabled
        // state (and messaging one it doesn't answer throws).
        let s = sel("setEnabled:");
        if msg!(view, "respondsToSelector:", (s: Sel) -> bool) {
            msg!(view, "setEnabled:", (enabled: bool) -> ());
        }
        let name = nsstring(if v.dark {
            "NSAppearanceNameDarkAqua"
        } else {
            "NSAppearanceNameAqua"
        });
        let appearance: Id = msg!(class("NSAppearance"), "appearanceNamed:", (name: Id) -> Id);
        msg!(view, "setAppearance:", (appearance: Id) -> ());
        // NSUserInterfaceLayoutDirectionRightToLeft = 1.
        let dir = v.rtl as i64;
        msg!(view, "setUserInterfaceLayoutDirection:", (dir: i64) -> ());
        let label = if v.label.is_empty() {
            std::ptr::null_mut()
        } else {
            nsstring(&v.label)
        };
        msg!(view, "setAccessibilityLabel:", (label: Id) -> ());
        // A control with a cell is read through it (the view itself is
        // ignored): the cell carries the name too.
        let cell = cell_of(view);
        if !cell.is_null() {
            msg!(cell, "setAccessibilityLabel:", (label: Id) -> ());
        }
    }
}

struct Appkit {
    view: Id,
}

impl ViewBackend for Appkit {
    type Handle = Id;

    fn create(&mut self, v: &NativeView) -> Option<Id> {
        unsafe {
            let f = frame(v);
            let init = |name: &str| -> Id {
                let alloc: Id = msg!(class(name), "alloc", () -> Id);
                msg!(alloc, "initWithFrame:", (f: CgRect) -> Id)
            };
            let button = |kind: u64, title: &str| -> Id {
                let b = init("NSButton");
                let text = nsstring(title);
                msg!(b, "setTitle:", (text: Id) -> ());
                msg!(b, "setButtonType:", (kind: u64) -> ());
                b
            };
            let control = match v.control {
                NativeControl::Button { .. } if v.menu.is_some() => select::create(f, v),
                NativeControl::Button { primary } => {
                    let b = button(BUTTON_MOMENTARY_PUSH_IN, &v.title);
                    msg!(b, "setBezelStyle:", (BEZEL_ROUNDED: u64) -> ());
                    if primary {
                        let accent: Id = msg!(class("NSColor"), "controlAccentColor", () -> Id);
                        msg!(b, "setBezelColor:", (accent: Id) -> ());
                    }
                    b
                }
                NativeControl::Checkbox { checked } => {
                    let b = button(BUTTON_SWITCH, "");
                    set_state(b, checked);
                    b
                }
                NativeControl::Radio { selected } => {
                    let b = button(BUTTON_RADIO, "");
                    set_state(b, selected);
                    b
                }
                NativeControl::Switch { on } => {
                    let s = init("NSSwitch");
                    set_state(s, on);
                    s
                }
                NativeControl::Slider { value } => {
                    let s = init("NSSlider");
                    let (min, max, v) = (0.0_f64, 1.0_f64, value as f64 / 1000.0);
                    msg!(s, "setMinValue:", (min: f64) -> ());
                    msg!(s, "setMaxValue:", (max: f64) -> ());
                    msg!(s, "setDoubleValue:", (v: f64) -> ());
                    let yes = true;
                    msg!(s, "setContinuous:", (yes: bool) -> ());
                    s
                }
                NativeControl::Progress { value } => {
                    let p = init("NSProgressIndicator");
                    let no = false;
                    msg!(p, "setIndeterminate:", (no: bool) -> ());
                    let (min, max, v) = (0.0_f64, 1000.0_f64, value as f64);
                    msg!(p, "setMinValue:", (min: f64) -> ());
                    msg!(p, "setMaxValue:", (max: f64) -> ());
                    msg!(p, "setDoubleValue:", (v: f64) -> ());
                    p
                }
                NativeControl::TextField => text::create(self.view, v)?,
            };
            if control.is_null() {
                return None;
            }
            if !matches!(
                v.control,
                NativeControl::Progress { .. } | NativeControl::TextField
            ) {
                let t = target();
                msg!(control, "setTarget:", (t: Id) -> ());
                let a = sel("stippleNativeAction:");
                msg!(control, "setAction:", (a: Sel) -> ());
                let yes = true;
                msg!(control, "setRefusesFirstResponder:", (yes: bool) -> ());
            }
            set_common(control, v);
            let parent = clips::parent_of(self.view, v);
            msg!(parent, "addSubview:", (control: Id) -> ());
            // The text, the focus: once it is in the window.
            if matches!(v.control, NativeControl::TextField) {
                let mut c = control;
                text::update(self.view, &mut c, v);
            }
            // Over a Metal layer the view may carry.
            let layer: Id = msg!(control, "layer", () -> Id);
            if !layer.is_null() {
                let z = 1.0_f64;
                msg!(layer, "setZPosition:", (z: f64) -> ());
            }
            Some(control)
        }
    }

    fn update(&mut self, h: &mut Id, old: &NativeView, new: &NativeView) {
        if let Some(t) = new.text.as_deref() {
            // One line, a password or several lines: another control.
            if !text::fits(*h, t)
                && let Some(fresh) = self.create(new)
            {
                self.destroy(std::mem::replace(h, fresh));
                return;
            }
            text::update(self.view, h, new);
            // Typing: only the text changed, which that took care of.
            if old.same_but_text(new) {
                return;
            }
        }
        let h = *h;
        unsafe {
            // Into the container of the scroll area it is in now.
            if old.clips.last().map(|c| c.id) != new.clips.last().map(|c| c.id) {
                clips::reparent(h, clips::parent_of(self.view, new));
            }
            set_common(h, new);
            match new.control {
                NativeControl::Button { .. } if new.menu.is_some() => select::update(h, old, new),
                NativeControl::Button { .. } => {
                    if old.title != new.title {
                        let text = nsstring(&new.title);
                        msg!(h, "setTitle:", (text: Id) -> ());
                    }
                }
                NativeControl::Checkbox { checked: on }
                | NativeControl::Radio { selected: on }
                | NativeControl::Switch { on } => set_state(h, on),
                NativeControl::Slider { value } => {
                    let shown: f64 = msg!(h, "doubleValue", () -> f64);
                    let v = value as f64 / 1000.0;
                    if (shown - v).abs() > 0.0015 {
                        msg!(h, "setDoubleValue:", (v: f64) -> ());
                    }
                }
                NativeControl::Progress { value } => {
                    let v = value as f64;
                    msg!(h, "setDoubleValue:", (v: f64) -> ());
                }
                NativeControl::TextField => {}
            }
        }
    }

    fn destroy(&mut self, h: Id) {
        text::destroy(h);
        unsafe {
            msg!(h, "removeFromSuperview", () -> ());
            msg!(h, "release", () -> ());
        }
    }
}

/// The controls that are live here: all of them.
pub(crate) fn live_kind(_control: NativeControl) -> bool {
    true
}

/// Show exactly `views` in window view `view` (`Window::set_native_views`).
pub(crate) fn sync(view: Id, views: &[NativeView]) {
    let mut backend = Appkit { view };
    let mut set = SETS.with(|s| s.borrow_mut().remove(&(view as usize)).unwrap_or_default());
    clips::prepare(view, views);
    let restack = set.sync(&mut backend, views);
    clips::finish(view, views);
    if restack {
        // Paint order: each (and the containers it is in) on top of the
        // ones before it.
        let mut raised = Vec::new();
        for (v, h) in set.in_order() {
            clips::raise(view, v, &mut raised);
            unsafe { clips::bring_to_front(*h) };
        }
    }
    SETS.with(|s| s.borrow_mut().insert(view as usize, set));
}

/// Whether window view `view` shows any live control (its Metal layer then
/// presents with Core Animation's transaction).
pub(crate) fn any_live(view: Id) -> bool {
    SETS.with(|s| {
        s.try_borrow()
            .is_ok_and(|s| s.get(&(view as usize)).is_some_and(|s| s.len() > 0))
    })
}

/// Forget window view `view`'s controls (it closed; AppKit releases its
/// subviews with it).
pub(crate) fn forget(view: Id) {
    let set = SETS.with(|s| s.borrow_mut().remove(&(view as usize)));
    if let Some(mut set) = set {
        set.sync(&mut Appkit { view }, &[]);
    }
    clips::forget(view);
}

/// `control`'s cell, if it has one (an `NSButton`'s, an `NSSlider`'s; not
/// an `NSSwitch`'s or an `NSProgressIndicator`'s), else nil.
unsafe fn cell_of(control: Id) -> Id {
    unsafe {
        let s = sel("cell");
        if msg!(control, "respondsToSelector:", (s: Sel) -> bool) {
            msg!(control, "cell", () -> Id)
        } else {
            std::ptr::null_mut()
        }
    }
}

/// The accessibility element of the control shown for `id` in window view
/// `view`, for the view's accessibility children: its cell, which AppKit
/// reads a cell-based control through, else the control.
pub(crate) fn handle_of(view: Id, id: u64) -> Option<Id> {
    let control = SETS.with(|s| {
        let s = s.try_borrow().ok()?;
        s.get(&(view as usize))?.get(id).map(|(_, h)| *h)
    })?;
    let cell = unsafe { cell_of(control) };
    Some(if cell.is_null() { control } else { cell })
}

/// Whether a mouse press at `at` (in window view `view`'s coordinates)
/// lands on one of its live controls, which AppKit then handles.
pub(crate) fn owns(view: Id, at: CgPoint) -> bool {
    let any = SETS.with(|s| {
        s.borrow()
            .get(&(view as usize))
            .is_some_and(|s| s.len() > 0)
    });
    any && unsafe { hit(view, at) }.is_some_and(|h| h != view)
}

/// Where a scroll-wheel or trackpad scroll goes (see [`wheel_route`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WheelRoute {
    /// To the app (which scrolls what is under it) and on to AppKit, as
    /// everywhere off a slider: AppKit's controls pass it on.
    Both,
    /// To the app only: up or down over a live slider, which would move.
    App,
    /// To AppKit only: sideways over a live slider, which moves it.
    Slider,
}

thread_local! {
    /// The trackpad gesture under way, from its first event: `None` while
    /// none is, else whether it began over a live slider, and once it moved,
    /// its route (kept through its momentum).
    static GESTURE: std::cell::Cell<Option<(bool, Option<WheelRoute>)>> =
        const { std::cell::Cell::new(None) };
}

/// Where a scroll event at `at` (in window view `view`'s coordinates)
/// goes: over a live slider, a mostly vertical scroll scrolls the list
/// (the app's) and a sideways one moves the slider (AppKit's), as in
/// AppKit's own lists; a trackpad gesture keeps the route its first moves
/// gave it, momentum included. `phase` and `momentum` are the event's
/// (`NSEventPhase`), `dx` and `dy` its deltas.
pub(crate) fn wheel_route(
    view: Id,
    at: CgPoint,
    phase: u64,
    momentum: u64,
    dx: f64,
    dy: f64,
) -> WheelRoute {
    const BEGAN: u64 = 1;
    const ENDED: u64 = 8;
    const CANCELLED: u64 = 16;
    const MAY_BEGIN: u64 = 32;
    let sideways = |dx: f64, dy: f64| {
        if dx.abs() > dy.abs() {
            WheelRoute::Slider
        } else {
            WheelRoute::App
        }
    };
    if phase == 0 && momentum == 0 {
        // A mouse's wheel: each step by itself.
        GESTURE.with(|g| g.set(None));
        let route = if over_slider(view, at) {
            sideways(dx, dy)
        } else {
            WheelRoute::Both
        };
        super::text::trace(|| format!("wheel {dx},{dy} at {},{}: {route:?}", at.x, at.y));
        return route;
    }
    GESTURE.with(|g| {
        if phase & (BEGAN | MAY_BEGIN) != 0 && !matches!(g.get(), Some((_, Some(_)))) {
            g.set(Some((over_slider(view, at), None)));
        }
        let route = match g.get() {
            // A gesture that began before the window had it.
            None => WheelRoute::Both,
            Some((_, Some(route))) => route,
            Some((false, None)) if dx != 0.0 || dy != 0.0 => {
                g.set(Some((false, Some(WheelRoute::Both))));
                WheelRoute::Both
            }
            Some((true, None)) if dx != 0.0 || dy != 0.0 => {
                let route = sideways(dx, dy);
                g.set(Some((true, Some(route))));
                route
            }
            // Not moving yet: the app has it (nothing to scroll).
            Some((on, None)) => {
                if on {
                    WheelRoute::App
                } else {
                    WheelRoute::Both
                }
            }
        };
        // It ended without momentum, or its momentum did.
        if momentum & (ENDED | CANCELLED) != 0 || (phase & CANCELLED != 0 && momentum == 0) {
            g.set(None);
        }
        super::text::trace(|| format!("wheel {dx},{dy} phase {phase}/{momentum}: {route:?}"));
        route
    })
}

/// Whether `at` (in window view `view`'s coordinates) is over one of its
/// live sliders.
fn over_slider(view: Id, at: CgPoint) -> bool {
    let sliders: Vec<Id> = SETS.with(|s| {
        s.try_borrow().map_or(Vec::new(), |s| {
            s.get(&(view as usize)).map_or(Vec::new(), |set| {
                set.in_order()
                    .filter(|(v, _)| matches!(v.control, NativeControl::Slider { .. }))
                    .map(|(_, h)| *h)
                    .collect()
            })
        })
    });
    !sliders.is_empty() && unsafe { hit(view, at) }.is_some_and(|h| sliders.contains(&h))
}

/// The live control (or the view itself) a press at `at` (in `view`'s
/// coordinates) lands on.
unsafe fn hit(view: Id, at: CgPoint) -> Option<Id> {
    unsafe {
        let parent: Id = msg!(view, "superview", () -> Id);
        let p: CgPoint = if parent.is_null() {
            at
        } else {
            msg!(view, "convertPoint:toView:", (at: CgPoint, parent: Id) -> CgPoint)
        };
        let mut h: Id = msg!(view, "hitTest:", (p: CgPoint) -> Id);
        // Up to the control (a control's cell may answer for it).
        loop {
            if h.is_null() || h == view {
                return (!h.is_null()).then_some(h);
            }
            let up: Id = msg!(h, "superview", () -> Id);
            if up == view {
                return Some(h);
            }
            h = up;
        }
    }
}

pub(crate) fn live_controls() -> Vec<super::LiveControl> {
    let shown: Vec<(Id, u64, Id)> = SETS.with(|s| {
        s.borrow()
            .iter()
            .flat_map(|(view, set)| {
                set.in_order()
                    .map(|(v, h)| (*view as Id, v.id, *h))
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    shown
        .into_iter()
        .map(|(view, id, h)| unsafe {
            // In the window view's coordinates (it may be in a container).
            let b: CgRect = msg!(h, "bounds", () -> CgRect);
            let f: CgRect = msg!(h, "convertRect:toView:", (b: CgRect, view: Id) -> CgRect);
            let (container, visible) = clips::shown(view, h);
            let at = CgPoint {
                x: visible.min_x() + visible.width() / 2.0,
                y: visible.min_y() + visible.height() / 2.0,
            };
            let s = sel("title");
            let title = if msg!(h, "respondsToSelector:", (s: Sel) -> bool) {
                super::objc::rust_string(msg!(h, "title", () -> Id)).unwrap_or_default()
            } else {
                String::new()
            };
            let (value, focused) = text::value_of(h).unwrap_or_default();
            super::LiveControl {
                id,
                class: super::objc::class_name(h),
                title,
                value,
                focused,
                frame: crate::geometry::Rect::from_xywh(f.x, f.y, f.width, f.height),
                visible,
                hit: visible.width() > 0.0 && visible.height() > 0.0 && lands_on(view, at, h),
                container,
                app_touches: false,
            }
        })
        .collect()
}

/// Whether a click at `at` (in `view`'s coordinates) lands on `control`
/// (or a part of it), through any containers.
unsafe fn lands_on(view: Id, at: CgPoint, control: Id) -> bool {
    unsafe {
        let parent: Id = msg!(view, "superview", () -> Id);
        let p: CgPoint = if parent.is_null() {
            at
        } else {
            msg!(view, "convertPoint:toView:", (at: CgPoint, parent: Id) -> CgPoint)
        };
        let mut h: Id = msg!(view, "hitTest:", (p: CgPoint) -> Id);
        while !h.is_null() && h != view && h != control {
            h = msg!(h, "superview", () -> Id);
        }
        h == control
    }
}

/// An `NSPopUpButton`'s menu is AppKit's own, opened by its click.
pub(crate) fn menu_shown(_dismiss: bool) -> bool {
    false
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateScrollWheelEvent2(
        source: *const c_void,
        units: u32,
        count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> *mut c_void;
    fn CGEventSetLocation(event: *mut c_void, at: CgPoint);
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGWarpMouseCursorPosition(at: CgPoint) -> i32;
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        kind: u32,
        at: CgPoint,
        button: u32,
    ) -> *mut c_void;
    fn CFRelease(obj: *const c_void);
}

/// Post a mouse wheel's scroll at `at` (logical pixels, top-left, in the
/// first window that shows live controls) through the window server
/// (`kCGHIDEventTap`), which sends it to the view it lands on.
pub(crate) fn post_scroll(at: crate::geometry::Point, dx: f64, dy: f64) -> bool {
    let Some(view) = SETS.with(|s| s.borrow().keys().next().map(|v| *v as Id)) else {
        return false;
    };
    unsafe {
        let window: Id = msg!(view, "window", () -> Id);
        if window.is_null() {
            return false;
        }
        // The view's (flipped) point, in the window, on the screen
        // (bottom-left), then in the window server's top-left coordinates.
        let p = CgPoint { x: at.x, y: at.y };
        let nil: Id = std::ptr::null_mut();
        let in_window: CgPoint =
            msg!(view, "convertPoint:toView:", (p: CgPoint, nil: Id) -> CgPoint);
        let r = CgRect {
            x: in_window.x,
            y: in_window.y,
            width: 0.0,
            height: 0.0,
        };
        let on_screen: CgRect = msg!(window, "convertRectToScreen:", (r: CgRect) -> CgRect);
        let screens: Id = msg!(class("NSScreen"), "screens", () -> Id);
        let main: Id = msg!(screens, "firstObject", () -> Id);
        if main.is_null() {
            return false;
        }
        let frame: CgRect = msg!(main, "frame", () -> CgRect);
        let global = CgPoint {
            x: on_screen.x,
            y: frame.height - on_screen.y,
        };
        // kCGScrollEventUnitPixel; wheel 1 is vertical, 2 horizontal.
        let ev = CGEventCreateScrollWheelEvent2(
            std::ptr::null(),
            0,
            2,
            dy.round() as i32,
            dx.round() as i32,
            0,
        );
        if ev.is_null() {
            return false;
        }
        // The window server sends a wheel to the window under the pointer,
        // and the app scrolls what is under its last move: the pointer
        // goes there first (a warp moves it without a move event).
        CGWarpMouseCursorPosition(global);
        const MOUSE_MOVED: u32 = 5;
        let moved = CGEventCreateMouseEvent(std::ptr::null(), MOUSE_MOVED, global, 0);
        if !moved.is_null() {
            CGEventPost(0, moved);
            CFRelease(moved);
        }
        CGEventSetLocation(ev, global);
        CGEventPost(0, ev);
        CFRelease(ev);
    }
    true
}

pub(crate) fn poke_text(id: u64, poke: super::TextPoke) -> bool {
    let found = SETS.with(|s| {
        s.borrow()
            .values()
            .find_map(|set| set.get(id).map(|(v, h)| (v.control, *h)))
    });
    match found {
        Some((NativeControl::TextField, h)) => text::poke(h, poke),
        _ => false,
    }
}

pub(crate) fn poke(id: u64, value: Option<f64>) -> bool {
    let found = SETS.with(|s| {
        s.borrow()
            .values()
            .find_map(|set| set.get(id).map(|(v, h)| (v.control, v.menu.is_some(), *h)))
    });
    let Some((control, menu, h)) = found else {
        return false;
    };
    unsafe {
        match control {
            NativeControl::Button { .. } if menu => {
                return select::pick(h, value.unwrap_or(0.0) as usize);
            }
            NativeControl::Slider { .. } => {
                let v = value.unwrap_or(0.5);
                msg!(h, "setDoubleValue:", (v: f64) -> ());
                let a: Sel = msg!(h, "action", () -> Sel);
                let t: Id = msg!(h, "target", () -> Id);
                msg!(h, "sendAction:to:", (a: Sel, t: Id) -> bool);
            }
            NativeControl::Progress { .. } | NativeControl::TextField => {}
            _ => {
                let nil: Id = std::ptr::null_mut();
                msg!(h, "performClick:", (nil: Id) -> ());
            }
        }
    }
    true
}
