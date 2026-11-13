//! Accessibility on iOS: the view as a `UIAccessibilityContainer`.
//!
//! The view isn't an accessibility element itself; it vends one
//! `UIAccessibilityElement` per node of the app's accessibility tree
//! (`accessibilityElements`, and the older `accessibilityElementCount` /
//! `accessibilityElementAtIndex:` / `indexOfAccessibilityElement:`), each with
//! its label, value, traits and frame (`accessibilityFrame`, in screen
//! coordinates: the node's bounds converted through the view's window to its
//! screen's coordinate space). `accessibilityFrameInContainerSpace` isn't
//! used: UIKit only converts it while its accessibility runtime is loaded (a
//! screen reader running), and reads back a zero frame otherwise. The
//! elements are a subclass built at run time (`StippleAccessibilityElement`)
//! whose `accessibilityActivate` (VoiceOver's double tap) taps the middle of
//! its node, through the touch path, and whose `accessibilityTraits` adds a
//! `UITextField`'s to a text field's (UIKit has no public text-entry trait),
//! so VoiceOver says "text field" and XCUITest finds it among `textFields`.
//!
//! Elements are kept and updated in place, by position in the tree's reading
//! order, so VoiceOver's cursor stays on its object across updates. An update
//! that changes what VoiceOver reads posts
//! `UIAccessibilityLayoutChangedNotification`; the first tree, or a dialog
//! opening or closing, `UIAccessibilityScreenChangedNotification`.

use std::cell::RefCell;
use std::ffi::c_void;

use super::a11y_tree::{self, BUTTON, Change, HEADER, Item, SELECTED, STATIC_TEXT, TEXT_ENTRY};
use super::{
    CgPoint, CgRect, CgSize, Class, Id, ObjcSuper, Sel, class, class_addMethod, msg_id,
    msg_id_usize, msg_usize, msg_void, msg_void_bool, msg_void_id, nsstring,
    objc_allocateClassPair, objc_getClass, objc_msgSend, objc_msgSendSuper, objc_registerClassPair,
    sel, string_of,
};
use crate::geometry::Point;
use crate::platform::access::A11yNode;
use crate::platform::event::TouchPhase;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIAccessibilityTraitButton: u64;
    static UIAccessibilityTraitStaticText: u64;
    static UIAccessibilityTraitHeader: u64;
    static UIAccessibilityTraitSelected: u64;
    static UIAccessibilityLayoutChangedNotification: u32;
    static UIAccessibilityScreenChangedNotification: u32;
    fn UIAccessibilityPostNotification(notification: u32, argument: Id);
    fn UIAccessibilityIsVoiceOverRunning() -> bool;
}

/// `NSNotFound`.
const NOT_FOUND: isize = isize::MAX;

/// The touch id accessibility taps use.
const ACTIVATE_TOUCH: u64 = u64::MAX - 1;

/// What an element last showed: its item, and the view's position on screen.
type Shown = (Item, (f64, f64));

#[derive(Default)]
struct Elements {
    /// The tree's nodes, as last set.
    items: Vec<Item>,
    /// The elements (retained), and the item each last showed.
    /// The elements (retained), with the item each last showed and the
    /// view's position on screen then.
    elements: Vec<(Id, Option<Shown>)>,
}

thread_local! {
    static STATE: RefCell<Elements> = RefCell::new(Elements::default());
}

fn uikit_traits(bits: u8) -> u64 {
    let mut traits = 0;
    unsafe {
        for (bit, value) in [
            (BUTTON, UIAccessibilityTraitButton),
            (STATIC_TEXT, UIAccessibilityTraitStaticText),
            (HEADER, UIAccessibilityTraitHeader),
            (SELECTED, UIAccessibilityTraitSelected),
        ] {
            if bits & bit != 0 {
                traits |= value;
            }
        }
    }
    traits
}

/// `StippleAccessibilityElement`: a `UIAccessibilityElement` that activates
/// by tapping its node, and whose traits name a text field as one.
fn element_class() -> Class {
    let name = c"StippleAccessibilityElement";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("UIAccessibilityElement"), name.as_ptr(), 0);
        if cls.is_null() {
            return class("UIAccessibilityElement");
        }
        class_addMethod(
            cls,
            sel("accessibilityActivate"),
            activate as *const c_void,
            c"B@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityTraits"),
            traits as *const c_void,
            c"Q@:".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

thread_local! {
    /// A `UITextField` whose traits a text field's element borrows.
    static TEXT_FIELD: std::cell::Cell<Id> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// `accessibilityTraits`: the traits set on the element, and for a text
/// field the ones a `UITextField` has. UIKit names no public trait for text
/// entry; a `UITextField` reports it (once the accessibility runtime is
/// loaded, as it is while VoiceOver or a UI test runs), so it is asked each
/// time rather than once.
extern "C" fn traits(this: Id, cmd: Sel) -> u64 {
    let base = unsafe {
        let sup = ObjcSuper {
            receiver: this,
            super_class: class("UIAccessibilityElement"),
        };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel) -> u64 =
            std::mem::transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd)
    };
    let field = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let (_, shown) = s.elements.iter().find(|(e, _)| *e == this)?;
        shown
            .as_ref()
            .map(|(item, _)| item.traits & TEXT_ENTRY != 0)
    });
    if field != Some(true) {
        return base;
    }
    let text_field = TEXT_FIELD.with(|f| {
        if f.get().is_null() {
            f.set(unsafe { msg_id(msg_id(class("UITextField"), sel("alloc")), sel("init")) });
        }
        f.get()
    });
    if text_field.is_null() {
        return base;
    }
    let get: unsafe extern "C" fn(Id, Sel) -> u64 =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    base | unsafe { get(text_field, sel("accessibilityTraits")) }
}

/// `accessibilityActivate`: tap the middle of the node, after this call
/// returns (VoiceOver may ask from inside other work).
extern "C" fn activate(this: Id, _cmd: Sel) -> bool {
    let frame = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = s.elements.iter().position(|(e, _)| *e == this)?;
        s.items.get(i).map(|item| item.frame)
    });
    let Some((x, y, w, h)) = frame else {
        return false;
    };
    let at = Point::new(x + w / 2.0, y + h / 2.0);
    super::later(move || {
        super::touch_input(TouchPhase::Started, &[(ACTIVATE_TOUCH, at)]);
        super::touch_input(TouchPhase::Ended, &[(ACTIVATE_TOUCH, at)]);
    });
    true
}

/// Bring the elements up to date with the items: make the missing ones,
/// release the extra ones, and set what changed.
fn sync(view: Id) {
    if view.is_null() {
        return;
    }
    // Work out what to do, then message UIKit with the state released, as it
    // may ask the container back.
    let Some((extra, missing)) = STATE.with(|s| {
        let mut s = s.try_borrow_mut().ok()?;
        let keep = s.items.len().min(s.elements.len());
        let extra: Vec<Id> = s.elements.drain(keep..).map(|(e, _)| e).collect();
        Some((extra, s.items.len() - keep))
    }) else {
        return;
    };
    for el in extra {
        unsafe { msg_void(el, sel("release")) };
    }
    let made: Vec<Id> = (0..missing)
        .map(|_| unsafe {
            let alloc = msg_id(element_class(), sel("alloc"));
            let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            init(alloc, sel("initWithAccessibilityContainer:"), view)
        })
        .collect();
    // Frames are in screen coordinates, so a view that moved (rotation)
    // updates every element.
    let origin = {
        let r = to_screen(view, (0.0, 0.0, 0.0, 0.0));
        (r.origin.x, r.origin.y)
    };
    let changed: Vec<(Id, Item)> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let s = &mut *s;
        s.elements.extend(made.into_iter().map(|e| (e, None)));
        s.elements
            .iter_mut()
            .zip(&s.items)
            .filter(|((_, shown), item)| {
                shown
                    .as_ref()
                    .is_none_or(|(i, o)| i != *item || *o != origin)
            })
            .map(|((el, shown), item)| {
                *shown = Some((item.clone(), origin));
                (*el, item.clone())
            })
            .collect()
    });
    for (el, item) in changed {
        unsafe { show(el, &item, to_screen(view, item.frame)) };
    }
}

/// `(x, y, width, height)` in `view`'s coordinates, in the screen's
/// coordinate space (`convertRect:toCoordinateSpace:` with the window's
/// screen; the window's coordinates if it has none yet).
pub(super) fn to_screen(view: Id, (x, y, width, height): (f64, f64, f64, f64)) -> CgRect {
    let rect = CgRect {
        origin: CgPoint { x, y },
        size: CgSize { width, height },
    };
    unsafe {
        let window = msg_id(view, sel("window"));
        let screen = if window.is_null() {
            std::ptr::null_mut()
        } else {
            msg_id(window, sel("screen"))
        };
        let convert: unsafe extern "C" fn(Id, Sel, CgRect, Id) -> CgRect =
            std::mem::transmute(objc_msgSend as *const c_void);
        if screen.is_null() {
            convert(view, sel("convertRect:toView:"), rect, std::ptr::null_mut())
        } else {
            let space = msg_id(screen, sel("coordinateSpace"));
            convert(view, sel("convertRect:toCoordinateSpace:"), rect, space)
        }
    }
}

/// Set `el`'s properties from `item`, its frame `frame` (screen coordinates).
unsafe fn show(el: Id, item: &Item, frame: CgRect) {
    let text = |s: &str| {
        if s.is_empty() {
            std::ptr::null_mut()
        } else {
            nsstring(s)
        }
    };
    unsafe {
        msg_void_bool(el, sel("setIsAccessibilityElement:"), item.is_element);
        msg_void_id(el, sel("setAccessibilityLabel:"), text(&item.label));
        msg_void_id(el, sel("setAccessibilityValue:"), text(&item.value));
        let set_traits: unsafe extern "C" fn(Id, Sel, u64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set_traits(
            el,
            sel("setAccessibilityTraits:"),
            uikit_traits(item.traits),
        );
        super::msg_void_rect(el, sel("setAccessibilityFrame:"), frame);
    }
}

/// The app's new accessibility tree (from `Window::set_accessibility_tree`).
/// Elements are built when VoiceOver (or anything) asks for them, or right
/// away while VoiceOver runs, so the one it's on reads true.
pub(super) fn set_tree(root: &A11yNode) {
    let items = a11y_tree::flatten(root);
    let change = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.items == items {
            return None;
        }
        let change = a11y_tree::change(&s.items, &items);
        s.items = items;
        Some(change)
    });
    let Some(change) = change else {
        return;
    };
    let view = super::CTX.with(|c| c.borrow().view);
    if unsafe { UIAccessibilityIsVoiceOverRunning() } {
        sync(view);
    }
    unsafe {
        match change {
            Change::None => {}
            Change::Layout => UIAccessibilityPostNotification(
                UIAccessibilityLayoutChangedNotification,
                std::ptr::null_mut(),
            ),
            Change::Screen => UIAccessibilityPostNotification(
                UIAccessibilityScreenChangedNotification,
                std::ptr::null_mut(),
            ),
        }
    }
}

extern "C" fn is_accessibility_element(_this: Id, _cmd: Sel) -> bool {
    false
}

extern "C" fn element_count(this: Id, _cmd: Sel) -> isize {
    sync(this);
    STATE.with(|s| s.borrow().elements.len() as isize)
}

extern "C" fn element_at(this: Id, _cmd: Sel, index: isize) -> Id {
    sync(this);
    STATE.with(|s| {
        usize::try_from(index)
            .ok()
            .and_then(|i| s.borrow().elements.get(i).map(|(e, _)| *e))
            .unwrap_or(std::ptr::null_mut())
    })
}

extern "C" fn index_of(this: Id, _cmd: Sel, element: Id) -> isize {
    sync(this);
    STATE.with(|s| {
        s.borrow()
            .elements
            .iter()
            .position(|(e, _)| *e == element)
            .map_or(NOT_FOUND, |i| i as isize)
    })
}

extern "C" fn elements(this: Id, _cmd: Sel) -> Id {
    sync(this);
    let ids: Vec<Id> = STATE.with(|s| s.borrow().elements.iter().map(|(e, _)| *e).collect());
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, *const Id, usize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(
            class("NSArray"),
            sel("arrayWithObjects:count:"),
            ids.as_ptr(),
            ids.len(),
        )
    }
}

/// Make the view class a `UIAccessibilityContainer`.
///
/// # Safety
/// `cls` must be a class pair still being built (before
/// `objc_registerClassPair`).
pub(super) unsafe fn add_methods(cls: Class) {
    let methods: [(&str, *const c_void, &std::ffi::CStr); 5] = [
        (
            "isAccessibilityElement",
            is_accessibility_element as *const c_void,
            c"B@:",
        ),
        (
            "accessibilityElementCount",
            element_count as *const c_void,
            c"q@:",
        ),
        (
            "accessibilityElementAtIndex:",
            element_at as *const c_void,
            c"@@:q",
        ),
        (
            "indexOfAccessibilityElement:",
            index_of as *const c_void,
            c"q@:@",
        ),
        ("accessibilityElements", elements as *const c_void, c"@@:"),
    ];
    for (name, imp, types) in methods {
        unsafe { class_addMethod(cls, sel(name), imp, types.as_ptr()) };
    }
}

/// An accessibility element as an assistive technology reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibilityElement {
    /// Whether VoiceOver stops on it (`isAccessibilityElement`).
    pub is_element: bool,
    pub label: String,
    pub value: String,
    /// Its traits, by name (`button`, `static_text`, `header`, `selected`),
    /// joined with `|`.
    pub traits: String,
    /// `accessibilityFrame`: in screen coordinates (points).
    pub frame: crate::geometry::Rect,
}

/// For self-tests: every element `view` vends, read through objc dispatch
/// the way VoiceOver does.
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn read_elements(view: Id) -> Vec<AccessibilityElement> {
    unsafe {
        let count = msg_usize(view, sel("accessibilityElementCount"));
        let get_traits: unsafe extern "C" fn(Id, Sel) -> u64 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let is_el: unsafe extern "C" fn(Id, Sel) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        (0..count)
            .filter_map(|i| {
                let el = msg_id_usize(view, sel("accessibilityElementAtIndex:"), i);
                if el.is_null() {
                    return None;
                }
                let bits = get_traits(el, sel("accessibilityTraits"));
                let names: Vec<&str> = [
                    (UIAccessibilityTraitButton, "button"),
                    (UIAccessibilityTraitStaticText, "static_text"),
                    (UIAccessibilityTraitHeader, "header"),
                    (UIAccessibilityTraitSelected, "selected"),
                ]
                .into_iter()
                .filter(|(bit, _)| bits & bit != 0)
                .map(|(_, name)| name)
                .collect();
                let f = super::msg_rect(el, sel("accessibilityFrame"));
                Some(AccessibilityElement {
                    is_element: is_el(el, sel("isAccessibilityElement")) != 0,
                    label: string_of(msg_id(el, sel("accessibilityLabel"))),
                    value: string_of(msg_id(el, sel("accessibilityValue"))),
                    traits: names.join("|"),
                    frame: crate::geometry::Rect::from_xywh(
                        f.origin.x,
                        f.origin.y,
                        f.size.width,
                        f.size.height,
                    ),
                })
            })
            .collect()
    }
}

/// For self-tests: `[[view accessibilityElementAtIndex:index] accessibilityActivate]`.
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn activate_at(view: Id, index: usize) -> bool {
    unsafe {
        let el = msg_id_usize(view, sel("accessibilityElementAtIndex:"), index);
        !el.is_null() && super::msg_bool(el, sel("accessibilityActivate"))
    }
}
