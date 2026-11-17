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

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use super::objc::{CgRect, Id, Sel, class, msg, nsstring, sel};
use super::{NativeControl, NativeView, NativeViewEvent, ViewBackend, ViewSet};

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

/// The window view and id of the control `sender`, with its kind.
fn find(sender: Id) -> Option<(Id, u64, NativeControl)> {
    SETS.with(|s| {
        let s = s.try_borrow().ok()?;
        s.iter().find_map(|(view, set)| {
            let id = set.id_of(&sender)?;
            Some((*view as Id, id, set.get(id)?.0.control))
        })
    })
}

/// `stippleNativeAction:`: the user acted on `sender`.
extern "C" fn action(_this: Id, _cmd: Sel, sender: Id) {
    let Some((view, id, control)) = find(sender) else {
        return;
    };
    let event = match control {
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

fn frame(v: &NativeView) -> CgRect {
    CgRect {
        x: v.frame.min_x(),
        y: v.frame.min_y(),
        width: v.frame.width(),
        height: v.frame.height(),
    }
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
                NativeControl::TextField => return None,
            };
            if control.is_null() {
                return None;
            }
            if !matches!(v.control, NativeControl::Progress { .. }) {
                let t = target();
                msg!(control, "setTarget:", (t: Id) -> ());
                let a = sel("stippleNativeAction:");
                msg!(control, "setAction:", (a: Sel) -> ());
                let yes = true;
                msg!(control, "setRefusesFirstResponder:", (yes: bool) -> ());
            }
            set_common(control, v);
            let parent = self.view;
            msg!(parent, "addSubview:", (control: Id) -> ());
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
        let h = *h;
        unsafe {
            set_common(h, new);
            match new.control {
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
        unsafe {
            msg!(h, "removeFromSuperview", () -> ());
            msg!(h, "release", () -> ());
        }
    }
}

/// The controls that are live here: all but a text field.
pub(crate) fn live_kind(control: NativeControl) -> bool {
    !matches!(control, NativeControl::TextField)
}

/// Show exactly `views` in window view `view` (`Window::set_native_views`).
pub(crate) fn sync(view: Id, views: &[NativeView]) {
    let mut backend = Appkit { view };
    let mut set = SETS.with(|s| s.borrow_mut().remove(&(view as usize)).unwrap_or_default());
    let restack = set.sync(&mut backend, views);
    if restack {
        // Paint order: each on top of the ones before it.
        for (_, h) in set.in_order() {
            let h = *h;
            unsafe {
                msg!(h, "retain", () -> Id);
                msg!(h, "removeFromSuperview", () -> ());
                msg!(view, "addSubview:", (h: Id) -> ());
                msg!(h, "release", () -> ());
            }
        }
    }
    SETS.with(|s| s.borrow_mut().insert(view as usize, set));
}

/// Forget window view `view`'s controls (it closed; AppKit releases its
/// subviews with it).
pub(crate) fn forget(view: Id) {
    let set = SETS.with(|s| s.borrow_mut().remove(&(view as usize)));
    if let Some(mut set) = set {
        set.sync(&mut Appkit { view }, &[]);
    }
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
            let f: CgRect = msg!(h, "frame", () -> CgRect);
            let at = CgPoint {
                x: f.x + f.width / 2.0,
                y: f.y + f.height / 2.0,
            };
            let s = sel("title");
            let title = if msg!(h, "respondsToSelector:", (s: Sel) -> bool) {
                super::objc::rust_string(msg!(h, "title", () -> Id)).unwrap_or_default()
            } else {
                String::new()
            };
            super::LiveControl {
                id,
                class: super::objc::class_name(h),
                title,
                frame: crate::geometry::Rect::from_xywh(f.x, f.y, f.width, f.height),
                hit: hit(view, at) == Some(h),
            }
        })
        .collect()
}

pub(crate) fn poke(id: u64, value: Option<f64>) -> bool {
    let found = SETS.with(|s| {
        s.borrow()
            .values()
            .find_map(|set| set.get(id).map(|(v, h)| (v.control, *h)))
    });
    let Some((control, h)) = found else {
        return false;
    };
    unsafe {
        match control {
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
