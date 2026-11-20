//! UIKit: the live controls, real `UIControl`s in the app's view (see
//! [`views`](super::views)).
//!
//! Each [`NativeView`] is a `UIButton` (iOS 26's glass configurations where
//! UIKit has them, else filled and gray), `UISwitch`, `UISlider` or
//! `UIProgressView`, a subview of the app's view at the control's frame (in
//! points, which are the window's logical pixels), above the frame's own
//! layer and any Metal layer over it (`zPosition` 1). Its target is one
//! shared `StippleNativeTarget`, whose action turns a tap
//! (`UIControlEventTouchUpInside`) or a new value
//! (`UIControlEventValueChanged`) into an
//! [`Event::NativeView`](crate::platform::Event::NativeView). A switch set
//! by the app moves with UIKit's animation; a slider the user is dragging
//! isn't set back under their finger.
//!
//! A button's, a switch's, a slider's and a select's touches are the app's
//! view's (`userInteractionEnabled` off): a finger on one goes to the app
//! as on any element, so a tap runs the element's handler, a drag scrolls
//! the list it is in (and flings it), a long press opens its context menu,
//! and an edge swipe is the back gesture, as with the controls drawn as
//! pictures. The button shows its pressed look while the app holds it
//! ([`NativeView::pressed`]). On a slider in a list a drag up or down
//! scrolls the list and a sideways one moves the slider (the app's drag
//! handler, whose new value the slider is set to); a tap on a select opens
//! its menu (`performPrimaryAction`, iOS 17.4 and later; [`open_menu`]),
//! where on earlier versions a select keeps its own touches.
//!
//! VoiceOver reads the control itself (the view's accessibility container
//! vends it in place of the node it stands for), named with the node's name,
//! a button with the button trait, and its frame on screen set each frame;
//! its double tap is a tap at the control's middle, which the app's view
//! takes.
//!
//! A select is a `UIButton` with a `UIMenu` (`ios_select`). The controls in
//! a scroll area are subviews of its clipping container (`apple_clips`),
//! which cuts one half scrolled out at the area's edge.

use std::cell::RefCell;
use std::ffi::c_void;

use super::apple_clips as clips;
use super::objc::{CgRect, Id, Sel, class, msg, nsstring, sel};
use super::os_select as select;
use super::{NativeControl, NativeView, NativeViewEvent, ViewBackend, ViewSet};

/// The live text fields.
#[path = "ios_text.rs"]
mod text;
pub(crate) use text::add_key_commands;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIAccessibilityTraitButton: u64;
    static UIAccessibilityTraitNotEnabled: u64;
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct CgPoint {
    x: f64,
    y: f64,
}

const TOUCH_UP_INSIDE: u64 = 1 << 6;
const VALUE_CHANGED: u64 = 1 << 12;

thread_local! {
    static SET: RefCell<ViewSet<Id>> = RefCell::new(ViewSet::default());
    static TARGET: std::cell::Cell<Id> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The object every live control sends its actions to.
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

/// `stippleNativeAction:`: the user acted on `sender`.
extern "C" fn action(_this: Id, _cmd: Sel, sender: Id) {
    let Some((id, control)) = SET.with(|s| {
        let s = s.try_borrow().ok()?;
        let id = s.id_of(&sender)?;
        Some((id, s.get(id)?.0.control))
    }) else {
        return;
    };
    let event = match control {
        NativeControl::Slider { .. } => {
            let v = unsafe { msg!(sender, "value", () -> f32) };
            NativeViewEvent::Value(v as f64)
        }
        _ => NativeViewEvent::Activated,
    };
    touched(id);
    crate::platform::backend::ios::send_event(crate::platform::Event::NativeView { id, event });
}

/// Where `v` goes in the view it is in (its scroll area's container, or
/// the app's view).
fn frame(v: &NativeView) -> CgRect {
    clips::frame_in_parent(v)
}

/// The user acted on control `id`: the next list sets it to what the app's
/// state says, changed or not.
pub(super) fn touched(id: u64) {
    SET.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.touch(id);
        }
    });
}

/// The button configuration for a primary or regular button: iOS 26's
/// glass ones where UIKit has them, else filled and gray. The snapshot
/// path draws the same (`native::ios`).
pub(super) unsafe fn button_config(primary: bool) -> Id {
    unsafe {
        let cls = class("UIButtonConfiguration");
        let glass = if primary {
            "prominentGlassButtonConfiguration"
        } else {
            "glassButtonConfiguration"
        };
        let name = if responds(cls, glass) {
            glass
        } else if primary {
            "filledButtonConfiguration"
        } else {
            "grayButtonConfiguration"
        };
        msg!(cls, name, () -> Id)
    }
}

/// Whether `obj` (a class or an instance) answers `selector`.
unsafe fn responds(obj: Id, selector: &str) -> bool {
    let s = sel(selector);
    unsafe { msg!(obj, "respondsToSelector:", (s: Sel) -> bool) }
}

pub(super) unsafe fn set_title(button: Id, title: &str) {
    unsafe {
        let config: Id = msg!(button, "configuration", () -> Id);
        if config.is_null() {
            return;
        }
        let text = nsstring(title);
        msg!(config, "setTitle:", (text: Id) -> ());
        msg!(button, "setConfiguration:", (config: Id) -> ());
    }
}

/// Set what every control shows the same way: its frame, whether it's
/// enabled, its look, its direction and its accessible name.
unsafe fn set_common(view: Id, v: &NativeView) {
    unsafe {
        let f = frame(v);
        msg!(view, "setFrame:", (f: CgRect) -> ());
        let enabled = v.enabled;
        if responds(view, "setEnabled:") {
            msg!(view, "setEnabled:", (enabled: bool) -> ());
        }
        // UIUserInterfaceStyleDark = 2, Light = 1.
        let style = if v.dark { 2_i64 } else { 1_i64 };
        msg!(view, "setOverrideUserInterfaceStyle:", (style: i64) -> ());
        // UISemanticContentAttributeForceRightToLeft = 4, ForceLeftToRight = 3.
        let dir = if v.rtl { 4_i64 } else { 3_i64 };
        msg!(view, "setSemanticContentAttribute:", (dir: i64) -> ());
        let label = if v.label.is_empty() {
            std::ptr::null_mut()
        } else {
            nsstring(&v.label)
        };
        msg!(view, "setAccessibilityLabel:", (label: Id) -> ());
        if let NativeControl::Button { .. } = v.control {
            // What UIKit's accessibility runtime says of a button when it's
            // loaded (with VoiceOver on), said always, so every reader of
            // the elements (and the app's self-tests) gets it.
            let mut traits = UIAccessibilityTraitButton;
            if !v.enabled {
                traits |= UIAccessibilityTraitNotEnabled;
            }
            msg!(view, "setAccessibilityTraits:", (traits: u64) -> ());
            let pressed = v.pressed;
            msg!(view, "setHighlighted:", (pressed: bool) -> ());
        }
    }
}

/// Whether the app's view takes the touches of `control` (a select's,
/// `menu`; see the module's docs).
fn app_takes_touches(control: NativeControl, menu: bool) -> bool {
    match control {
        // A tap opens its menu from the app (`open_menu`), where UIKit can.
        NativeControl::Button { .. } if menu => opens_menus(),
        NativeControl::Button { .. }
        | NativeControl::Switch { .. }
        | NativeControl::Slider { .. } => true,
        _ => false,
    }
}

/// Whether a button's menu can be opened from code
/// (`UIControl.performPrimaryAction`, iOS 17.4).
fn opens_menus() -> bool {
    thread_local! {
        static OPENS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }
    OPENS.with(|o| {
        if o.get().is_none() {
            let s = sel("performPrimaryAction");
            let cls = class("UIButton");
            o.set(Some(unsafe {
                msg!(cls, "instancesRespondToSelector:", (s: Sel) -> bool)
            }));
        }
        o.get() == Some(true)
    })
}

/// Open live select `id`'s menu, as a tap on it does (the app's view took
/// the tap: `Window::open_native_menu`). Returns whether it did.
pub(crate) fn open_menu(id: u64) -> bool {
    let Some(h) = SET.with(|s| {
        let s = s.try_borrow().ok()?;
        s.get(id).filter(|(v, _)| v.menu.is_some()).map(|(_, h)| *h)
    }) else {
        return false;
    };
    if !opens_menus() {
        return false;
    }
    unsafe { msg!(h, "performPrimaryAction", () -> ()) };
    true
}

/// Set `control`'s accessibility frame: its bounds on screen, as VoiceOver
/// reads it (UIKit's accessibility runtime works it out by itself, but only
/// while VoiceOver is on).
unsafe fn set_screen_frame(control: Id) {
    unsafe {
        let view = control;
        let f: CgRect = msg!(control, "bounds", () -> CgRect);
        let window: Id = msg!(view, "window", () -> Id);
        let screen: Id = if window.is_null() {
            std::ptr::null_mut()
        } else {
            msg!(window, "screen", () -> Id)
        };
        let on_screen: CgRect = if screen.is_null() {
            let nil: Id = std::ptr::null_mut();
            msg!(view, "convertRect:toView:", (f: CgRect, nil: Id) -> CgRect)
        } else {
            let space: Id = msg!(screen, "coordinateSpace", () -> Id);
            msg!(view, "convertRect:toCoordinateSpace:", (f: CgRect, space: Id) -> CgRect)
        };
        msg!(control, "setAccessibilityFrame:", (on_screen: CgRect) -> ());
    }
}

/// The app's view and the controls in it.
struct Uikit {
    view: Id,
}

impl ViewBackend for Uikit {
    type Handle = Id;

    fn create(&mut self, v: &NativeView) -> Option<Id> {
        if self.view.is_null() {
            return None;
        }
        unsafe {
            let f = frame(v);
            let init = |name: &str| -> Id {
                let alloc: Id = msg!(class(name), "alloc", () -> Id);
                msg!(alloc, "initWithFrame:", (f: CgRect) -> Id)
            };
            let (control, events) = match v.control {
                // A select: its menu's actions are its events.
                NativeControl::Button { .. } if v.menu.is_some() => (select::create(v), 0),
                NativeControl::Button { primary } => {
                    let kind = 1_i64; // UIButtonTypeSystem
                    let b: Id = msg!(class("UIButton"), "buttonWithType:", (kind: i64) -> Id);
                    let b: Id = msg!(b, "retain", () -> Id);
                    let config = button_config(primary);
                    msg!(b, "setConfiguration:", (config: Id) -> ());
                    set_title(b, &v.title);
                    (b, TOUCH_UP_INSIDE)
                }
                NativeControl::Switch { on } => {
                    let s = init("UISwitch");
                    let animated = false;
                    msg!(s, "setOn:animated:", (on: bool, animated: bool) -> ());
                    (s, VALUE_CHANGED)
                }
                NativeControl::Slider { value } => {
                    let s = init("UISlider");
                    let v = value as f32 / 1000.0;
                    msg!(s, "setValue:", (v: f32) -> ());
                    (s, VALUE_CHANGED)
                }
                NativeControl::Progress { value } => {
                    let style = 0_i64; // UIProgressViewStyleDefault
                    let alloc: Id = msg!(class("UIProgressView"), "alloc", () -> Id);
                    let p: Id = msg!(alloc, "initWithProgressViewStyle:", (style: i64) -> Id);
                    let v = value as f32 / 1000.0;
                    msg!(p, "setProgress:", (v: f32) -> ());
                    (p, 0)
                }
                NativeControl::TextField => (text::create(v)?, 0),
                // Checkboxes and radios aren't UIKit's.
                _ => return None,
            };
            if control.is_null() {
                return None;
            }
            if events != 0 {
                let t = target();
                let a = sel("stippleNativeAction:");
                msg!(control, "addTarget:action:forControlEvents:", (t: Id, a: Sel, events: u64) -> ());
            }
            set_common(control, v);
            if app_takes_touches(v.control, v.menu.is_some()) {
                let no = false;
                msg!(control, "setUserInteractionEnabled:", (no: bool) -> ());
            }
            // Over the frame's layer, and over a Metal layer added later.
            let layer: Id = msg!(control, "layer", () -> Id);
            let z = 1.0_f64;
            msg!(layer, "setZPosition:", (z: f64) -> ());
            let parent = clips::parent_of(self.view, v);
            msg!(parent, "addSubview:", (control: Id) -> ());
            // The text, the focus: once it is in the view.
            if matches!(v.control, NativeControl::TextField) {
                let mut c = control;
                text::update(self.view, &mut c, v);
            }
            Some(control)
        }
    }

    fn update(&mut self, h: &mut Id, old: &NativeView, new: &NativeView) {
        if let Some(t) = new.text.as_deref() {
            // One line or several: another control.
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
                        set_title(h, &new.title);
                    }
                }
                NativeControl::Switch { on } => {
                    let shown: bool = msg!(h, "isOn", () -> bool);
                    if shown != on {
                        let animated = true;
                        msg!(h, "setOn:animated:", (on: bool, animated: bool) -> ());
                    }
                }
                NativeControl::Slider { value } => {
                    let tracking: bool = msg!(h, "isTracking", () -> bool);
                    let shown: f32 = msg!(h, "value", () -> f32);
                    let v = value as f32 / 1000.0;
                    if !tracking && (shown - v).abs() > 0.0015 {
                        msg!(h, "setValue:", (v: f32) -> ());
                    }
                }
                NativeControl::Progress { value } => {
                    let v = value as f32 / 1000.0;
                    let animated = true;
                    msg!(h, "setProgress:animated:", (v: f32, animated: bool) -> ());
                }
                _ => {}
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

/// The controls that are live here (UIKit has no checkbox or radio button).
pub(crate) fn live_kind(control: NativeControl) -> bool {
    matches!(
        control,
        NativeControl::Button { .. }
            | NativeControl::Switch { .. }
            | NativeControl::Slider { .. }
            | NativeControl::Progress { .. }
            | NativeControl::TextField
    )
}

/// Show exactly `views` in the app's view (`Window::set_native_views`).
pub(crate) fn sync(views: &[NativeView]) {
    let mut backend = Uikit {
        view: crate::platform::backend::ios::app_view(),
    };
    if backend.view.is_null() {
        return;
    }
    clips::prepare(backend.view, views);
    let restack = SET.with(|s| s.borrow_mut().sync(&mut backend, views));
    clips::finish(backend.view, views);
    if restack {
        // Paint order: each (and the containers it is in) over the ones
        // before it.
        let order: Vec<(NativeView, Id)> = SET.with(|s| {
            s.borrow()
                .in_order()
                .map(|(v, h)| (v.clone(), *h))
                .collect()
        });
        let mut raised = Vec::new();
        for (v, h) in order {
            clips::raise(backend.view, &v, &mut raised);
            unsafe { clips::bring_to_front(h) };
        }
    }
    // Each frame: the view itself may have moved on screen.
    let shown: Vec<Id> = SET.with(|s| s.borrow().in_order().map(|(_, h)| *h).collect());
    for h in shown {
        unsafe { set_screen_frame(h) };
    }
}

/// Whether any control is live (the Metal layer then presents with Core
/// Animation's transaction, `MetalLayer::present_with_transaction`).
pub(crate) fn any_live() -> bool {
    SET.with(|s| s.try_borrow().is_ok_and(|s| s.len() > 0))
}

/// The control shown for `id`, for the accessibility container to vend in
/// its node's place.
pub(crate) fn handle_of(id: u64) -> Option<Id> {
    SET.with(|s| s.try_borrow().ok()?.get(id).map(|(_, h)| *h))
}

pub(crate) fn live_controls() -> Vec<super::LiveControl> {
    let view = crate::platform::backend::ios::app_view();
    let shown: Vec<(u64, NativeControl, bool, Id)> = SET.with(|s| {
        s.borrow()
            .in_order()
            .map(|(v, h)| (v.id, v.control, v.menu.is_some(), *h))
            .collect()
    });
    shown
        .into_iter()
        .map(|(id, control, menu, h)| unsafe {
            let name = super::objc::class_name(h);
            // A configured button's title is its configuration's.
            let config: Id = if responds(h, "configuration") {
                msg!(h, "configuration", () -> Id)
            } else {
                std::ptr::null_mut()
            };
            let title = if config.is_null() {
                String::new()
            } else {
                super::objc::rust_string(msg!(config, "title", () -> Id)).unwrap_or_default()
            };
            // In the app view's coordinates (it may be in a container).
            let b: CgRect = msg!(h, "bounds", () -> CgRect);
            let f: CgRect = msg!(h, "convertRect:toView:", (b: CgRect, view: Id) -> CgRect);
            let (container, visible) = clips::shown(view, h);
            let at = CgPoint {
                x: visible.min_x() + visible.width() / 2.0,
                y: visible.min_y() + visible.height() / 2.0,
            };
            let nil: Id = std::ptr::null_mut();
            let mut hit: Id = msg!(view, "hitTest:withEvent:", (at: CgPoint, nil: Id) -> Id);
            // A tap there reaches the control: through the app's view, for
            // one whose touches it takes.
            let app_touches = app_takes_touches(control, menu);
            if hit == view && app_touches {
                hit = h;
            }
            while !hit.is_null() && hit != h {
                hit = msg!(hit, "superview", () -> Id);
            }
            let (value, focused) = text::value_of(h).unwrap_or_default();
            super::LiveControl {
                id,
                class: name,
                title,
                value,
                focused,
                frame: crate::geometry::Rect::from_xywh(f.x, f.y, f.width, f.height),
                visible,
                hit: !hit.is_null() && visible.width() > 0.0 && visible.height() > 0.0,
                container,
                app_touches,
            }
        })
        .collect()
}

/// No mouse wheel to post on the simulator (`simulate::trackpad_scroll`
/// drives the trackpad's recognizer instead).
pub(crate) fn post_scroll(_at: crate::geometry::Point, _dx: f64, _dy: f64) -> bool {
    false
}

/// Whether a context menu (a select's `UIMenu`) shows in the app's
/// window scene: a view of UIKit's context-menu classes in one of its
/// windows. With `dismiss`, each live select's menu interaction closes
/// its menu (`dismissMenu`).
pub(crate) fn menu_shown(dismiss: bool) -> bool {
    let view = crate::platform::backend::ios::app_view();
    if view.is_null() {
        return false;
    }
    unsafe fn any_menu(v: Id, depth: u32) -> bool {
        unsafe {
            if super::objc::class_name(v).contains("ContextMenu") {
                return true;
            }
            if depth == 0 {
                return false;
            }
            let subs: Id = msg!(v, "subviews", () -> Id);
            let n: usize = msg!(subs, "count", () -> usize);
            (0..n).any(|i| {
                let s: Id = msg!(subs, "objectAtIndex:", (i: usize) -> Id);
                any_menu(s, depth - 1)
            })
        }
    }
    let shown = unsafe {
        let window: Id = msg!(view, "window", () -> Id);
        let scene: Id = if window.is_null() {
            std::ptr::null_mut()
        } else {
            msg!(window, "windowScene", () -> Id)
        };
        let windows: Id = if scene.is_null() {
            std::ptr::null_mut()
        } else {
            msg!(scene, "windows", () -> Id)
        };
        let n: usize = if windows.is_null() {
            0
        } else {
            msg!(windows, "count", () -> usize)
        };
        (0..n).any(|i| {
            let w: Id = msg!(windows, "objectAtIndex:", (i: usize) -> Id);
            any_menu(w, 12)
        })
    };
    if dismiss {
        let selects: Vec<Id> = SET.with(|s| {
            s.borrow()
                .in_order()
                .filter(|(v, _)| v.menu.is_some())
                .map(|(_, h)| *h)
                .collect()
        });
        for h in selects {
            unsafe {
                if responds(h, "contextMenuInteraction") {
                    let i: Id = msg!(h, "contextMenuInteraction", () -> Id);
                    if !i.is_null() {
                        msg!(i, "dismissMenu", () -> ());
                    }
                }
            }
        }
    }
    shown
}

pub(crate) fn poke_text(id: u64, poke: super::TextPoke) -> bool {
    let h = SET.with(|s| s.borrow().get(id).map(|(_, h)| *h));
    h.is_some_and(|h| text::poke(h, poke))
}

pub(crate) fn poke(id: u64, value: Option<f64>) -> bool {
    let Some((control, menu, h)) = SET.with(|s| {
        let s = s.borrow();
        s.get(id).map(|(v, h)| (v.control, v.menu.is_some(), *h))
    }) else {
        return false;
    };
    unsafe {
        let events = match control {
            NativeControl::Button { .. } if menu => {
                return select::pick(h, value.unwrap_or(0.0) as usize);
            }
            NativeControl::Button { .. } => TOUCH_UP_INSIDE,
            NativeControl::Switch { .. } => {
                let on: bool = msg!(h, "isOn", () -> bool);
                let to = !on;
                let animated = true;
                msg!(h, "setOn:animated:", (to: bool, animated: bool) -> ());
                VALUE_CHANGED
            }
            NativeControl::Slider { .. } => {
                let v = value.unwrap_or(0.5) as f32;
                msg!(h, "setValue:", (v: f32) -> ());
                VALUE_CHANGED
            }
            _ => return true,
        };
        msg!(h, "sendActionsForControlEvents:", (events: u64) -> ());
    }
    true
}
