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
//! opening or closing, `UIAccessibilityScreenChangedNotification`. An alert
//! (a toast) appearing is announced with
//! `UIAccessibilityAnnouncementNotification`.
//!
//! VoiceOver's three-finger swipe (`accessibilityScroll:`) on an element
//! scrolls the innermost scroll container around it (or the element itself,
//! when it is one) on the swipe's axis, a page at a time, through a wheel at
//! the container's middle, and says which page it's on
//! (`UIAccessibilityPageScrolledNotification`, "Page N of M").
//!
//! An activation of an element scrolled out of view, and VoiceOver's cursor
//! landing on one (`accessibilityElementDidBecomeFocused`), bring it into
//! view first (`access::reveal`: every scroll container on the way to it,
//! by a wheel), so the tap lands on it where it then is.
//!
//! A table's element is a data table (`UIAccessibilityContainerDataTable`,
//! `accessibilityContainerType` `UIAccessibilityContainerTypeDataTable`):
//! it vends the elements inside it, says how many rows and columns it has
//! (all of them, a virtualized table's not built too), gives the cell at a
//! row and column and a column's header; its cells
//! (`UIAccessibilityContainerDataTableCell`) say their row and column. A
//! control labelled by a caption has it in its label.

use std::cell::RefCell;
use std::ffi::c_void;

use super::a11y_tree::{
    self, ADJUSTABLE, BUTTON, Change, HEADER, Item, SELECTED, STATIC_TEXT, TEXT_ENTRY,
};
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
    static UIAccessibilityTraitAdjustable: u64;
    static UIAccessibilityLayoutChangedNotification: u32;
    static UIAccessibilityScreenChangedNotification: u32;
    static UIAccessibilityAnnouncementNotification: u32;
    static UIAccessibilityPageScrolledNotification: u32;
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
    /// For each item, the scroll containers around it, innermost first.
    scrollers: Vec<Vec<a11y_tree::Scroller>>,
    /// The tree the items came from, where an activation finds what to
    /// scroll to bring its node into view.
    root: Option<A11yNode>,
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
            (ADJUSTABLE, UIAccessibilityTraitAdjustable),
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
        class_addMethod(
            cls,
            sel("accessibilityScroll:"),
            scroll as *const c_void,
            c"B@:q".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityIncrement"),
            increment as *const c_void,
            c"v@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityDecrement"),
            decrement as *const c_void,
            c"v@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityElementDidBecomeFocused"),
            became_focused as *const c_void,
            c"v@:".as_ptr(),
        );
        add_table_methods(cls);
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
/// returns (VoiceOver may ask from inside other work). A node scrolled out
/// of view is brought into it first (the scroll containers' wheels), and
/// tapped where it then is.
extern "C" fn activate(this: Id, _cmd: Sel) -> bool {
    let found = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = s.elements.iter().position(|(e, _)| *e == this)?;
        let item = s.items.get(i)?;
        let revealed = s.root.as_ref().and_then(|root| root.reveal(&item.path));
        Some(match revealed {
            Some(r) => (r.events, r.at),
            None => {
                let (x, y, w, h) = item.frame;
                (Vec::new(), Point::new(x + w / 2.0, y + h / 2.0))
            }
        })
    });
    let Some((events, at)) = found else {
        return false;
    };
    super::later(move || {
        for event in events {
            super::call_handler(event, &super::current_window());
        }
        super::touch_input(TouchPhase::Started, &[(ACTIVATE_TOUCH, at)]);
        super::touch_input(TouchPhase::Ended, &[(ACTIVATE_TOUCH, at)]);
    });
    true
}

/// `accessibilityElementDidBecomeFocused`: VoiceOver's cursor landed on the
/// element (a swipe to the next one): bring its node into view when a
/// scroll container hides it, as a `UIScrollView` does for its subviews.
extern "C" fn became_focused(this: Id, _cmd: Sel) {
    let events = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = s.elements.iter().position(|(e, _)| *e == this)?;
        let path = &s.items.get(i)?.path;
        s.root.as_ref()?.scroll_into_view_events(path)
    });
    let Some(events) = events else {
        return;
    };
    super::later(move || {
        for event in events {
            super::call_handler(event, &super::current_window());
        }
    });
}

/// `accessibilityScroll:` (VoiceOver's three-finger swipe): scroll the
/// container around the element on the swipe's axis a page, after this call
/// returns, and say which page it shows. `NO` when none can move that way,
/// so UIKit tries the element's containers.
extern "C" fn scroll(this: Id, _cmd: Sel, direction: i64) -> bool {
    let found = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = s.elements.iter().position(|(e, _)| *e == this)?;
        let (target, forward) = a11y_tree::scroll_target(s.scrollers.get(i)?, direction)?;
        let events = target.scroll_page_events(forward)?;
        Some((events, a11y_tree::page_status(target, forward)))
    });
    let Some((events, status)) = found else {
        return false;
    };
    super::later(move || {
        for event in events {
            super::call_handler(event, &super::current_window());
        }
    });
    unsafe {
        UIAccessibilityPostNotification(UIAccessibilityPageScrolledNotification, nsstring(&status));
    }
    true
}

/// `accessibilityIncrement` / `…Decrement` (VoiceOver's swipe up and down
/// on a slider): step it, after this call returns.
fn adjust(this: Id, up: bool) {
    let events = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = s.elements.iter().position(|(e, _)| *e == this)?;
        s.items.get(i)?.adjust.as_ref()?.step_range_events(up)
    });
    let Some(events) = events else {
        return;
    };
    super::later(move || {
        for event in events {
            super::call_handler(event, &super::current_window());
        }
    });
}

extern "C" fn increment(this: Id, _cmd: Sel) {
    adjust(this, true);
}

extern "C" fn decrement(this: Id, _cmd: Sel) {
    adjust(this, false);
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
    let changed: Vec<(Id, Item, Id)> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let s = &mut *s;
        s.elements.extend(made.into_iter().map(|e| (e, None)));
        let ids: Vec<Id> = s.elements.iter().map(|(e, _)| *e).collect();
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
                // A table's element holds what is inside it; the view the rest.
                let container = item.container.and_then(|t| ids.get(t).copied());
                (*el, item.clone(), container.unwrap_or(view))
            })
            .collect()
    });
    for (el, item, container) in changed {
        unsafe {
            msg_void_id(el, sel("setAccessibilityContainer:"), container);
            show(el, &item, to_screen(view, item.frame));
        }
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
        msg_void_id(el, sel("setAccessibilityHint:"), text(&item.hint));
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
    let scrollers = a11y_tree::scrollers(root);
    let change = STATE.with(|s| {
        let mut s = s.borrow_mut();
        // Offsets change without changing what an element shows.
        s.scrollers = scrollers;
        if s.root.as_ref() != Some(root) {
            s.root = Some(root.clone());
        }
        if s.items == items {
            return None;
        }
        let change = a11y_tree::change(&s.items, &items);
        let alerts = a11y_tree::new_alerts(&s.items, &items);
        s.items = items;
        Some((change, alerts))
    });
    let Some((change, alerts)) = change else {
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
        for text in alerts {
            UIAccessibilityPostNotification(
                UIAccessibilityAnnouncementNotification,
                nsstring(&text.replace('\0', "")),
            );
        }
    }
}

extern "C" fn is_accessibility_element(_this: Id, _cmd: Sel) -> bool {
    false
}

/// What is vended for item `i`: the live OS control that stands for its
/// node, if one is shown, else its element (null when there is none).
fn vend_one(s: &Elements, i: usize) -> Id {
    let Some((el, _)) = s.elements.get(i) else {
        return std::ptr::null_mut();
    };
    s.items
        .get(i)
        .and_then(|item| item.native)
        .and_then(crate::platform::native::os_views::handle_of)
        .unwrap_or(*el)
}

/// What the view vends at each position (the items outside tables).
fn vended(s: &Elements) -> Vec<Id> {
    vended_in(s, None)
}

/// What `container` (a table's item, `None` for the view) vends at each
/// position.
fn vended_in(s: &Elements, container: Option<usize>) -> Vec<Id> {
    let n = s.items.len().min(s.elements.len());
    a11y_tree::vended_by(&s.items[..n], container)
        .into_iter()
        .map(|i| vend_one(s, i))
        .collect()
}

extern "C" fn element_count(this: Id, _cmd: Sel) -> isize {
    sync(this);
    STATE.with(|s| vended(&s.borrow()).len() as isize)
}

extern "C" fn element_at(this: Id, _cmd: Sel, index: isize) -> Id {
    sync(this);
    STATE.with(|s| {
        usize::try_from(index)
            .ok()
            .and_then(|i| vended(&s.borrow()).get(i).copied())
            .unwrap_or(std::ptr::null_mut())
    })
}

extern "C" fn index_of(this: Id, _cmd: Sel, element: Id) -> isize {
    sync(this);
    STATE.with(|s| {
        vended(&s.borrow())
            .iter()
            .position(|e| *e == element)
            .map_or(NOT_FOUND, |i| i as isize)
    })
}

extern "C" fn elements(this: Id, _cmd: Sel) -> Id {
    sync(this);
    let ids: Vec<Id> = STATE.with(|s| vended(&s.borrow()));
    ns_array(&ids)
}

/// An autoreleased `NSArray` of `ids`.
fn ns_array(ids: &[Id]) -> Id {
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

/// The item element `this` shows, by index.
fn item_of(s: &Elements, this: Id) -> Option<usize> {
    s.elements.iter().position(|(e, _)| *e == this)
}

/// For a table's element, what it vends (else nothing: other elements hold
/// no others).
fn table_children(this: Id) -> Vec<Id> {
    STATE.with(|s| {
        let Ok(s) = s.try_borrow() else {
            return Vec::new();
        };
        match item_of(&s, this) {
            Some(i) if s.items.get(i).is_some_and(|it| it.table.is_some()) => {
                vended_in(&s, Some(i))
            }
            _ => Vec::new(),
        }
    })
}

/// A table element's `accessibilityElements` (nil for any other element).
extern "C" fn el_elements(this: Id, _cmd: Sel) -> Id {
    let ids = table_children(this);
    if ids.is_empty() {
        return std::ptr::null_mut();
    }
    ns_array(&ids)
}

extern "C" fn el_count(this: Id, _cmd: Sel) -> isize {
    table_children(this).len() as isize
}

extern "C" fn el_at(this: Id, _cmd: Sel, index: isize) -> Id {
    usize::try_from(index)
        .ok()
        .and_then(|i| table_children(this).get(i).copied())
        .unwrap_or(std::ptr::null_mut())
}

extern "C" fn el_index_of(this: Id, _cmd: Sel, element: Id) -> isize {
    table_children(this)
        .iter()
        .position(|e| *e == element)
        .map_or(NOT_FOUND, |i| i as isize)
}

/// `UIAccessibilityContainerTypeDataTable`.
const CONTAINER_DATA_TABLE: isize = 1;

/// `accessibilityContainerType`: a table's element is a data table.
extern "C" fn container_type(this: Id, _cmd: Sel) -> isize {
    let table = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = item_of(&s, this)?;
        s.items.get(i)?.table
    });
    if table.is_some() {
        CONTAINER_DATA_TABLE
    } else {
        0
    }
}

/// The table (rows, columns) and its item's index, for a table's element.
fn table_of(this: Id) -> Option<(usize, crate::platform::access::A11yTable)> {
    STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let i = item_of(&s, this)?;
        Some((i, s.items.get(i)?.table?))
    })
}

extern "C" fn row_count(this: Id, _cmd: Sel) -> usize {
    table_of(this).map_or(0, |(_, t)| t.rows)
}

extern "C" fn column_count(this: Id, _cmd: Sel) -> usize {
    table_of(this).map_or(0, |(_, t)| t.columns)
}

/// `accessibilityDataTableCellElementForRow:column:`: nil for a row the
/// table hasn't built.
extern "C" fn cell_for(this: Id, _cmd: Sel, row: usize, column: usize) -> Id {
    STATE.with(|s| {
        let Ok(s) = s.try_borrow() else {
            return std::ptr::null_mut();
        };
        let Some(t) = item_of(&s, this).filter(|&i| s.items[i].table.is_some()) else {
            return std::ptr::null_mut();
        };
        a11y_tree::cell_at(&s.items, t, row, column)
            .map_or(std::ptr::null_mut(), |c| vend_one(&s, c))
    })
}

/// `accessibilityHeaderElementsForColumn:`: its column header.
extern "C" fn column_headers(this: Id, _cmd: Sel, column: usize) -> Id {
    let ids: Vec<Id> = STATE.with(|s| {
        let Ok(s) = s.try_borrow() else {
            return Vec::new();
        };
        match item_of(&s, this).filter(|&i| s.items[i].table.is_some()) {
            Some(t) => a11y_tree::column_headers(&s.items, t, column)
                .into_iter()
                .map(|c| vend_one(&s, c))
                .collect(),
            None => Vec::new(),
        }
    });
    ns_array(&ids)
}

/// `accessibilityHeaderElementsForRow:`: rows have no headers.
extern "C" fn row_headers(_this: Id, _cmd: Sel, _row: usize) -> Id {
    ns_array(&[])
}

/// A cell's row or column (`NSRange`), `{NSNotFound, 0}` for anything else.
fn cell_range(this: Id, column: bool) -> super::text_input::NsRange {
    let cell = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let it = s.items.get(item_of(&s, this)?)?;
        it.cell.filter(|_| {
            matches!(
                it.role,
                crate::platform::access::A11yRole::Cell
                    | crate::platform::access::A11yRole::ColumnHeader
            )
        })
    });
    match cell {
        Some(c) => super::text_input::NsRange {
            location: if column { c.column } else { c.row },
            length: 1,
        },
        None => super::text_input::NsRange {
            location: NOT_FOUND as usize,
            length: 0,
        },
    }
}

extern "C" fn row_range(this: Id, _cmd: Sel) -> super::text_input::NsRange {
    cell_range(this, false)
}

extern "C" fn column_range(this: Id, _cmd: Sel) -> super::text_input::NsRange {
    cell_range(this, true)
}

/// The data table methods (on every element: each answers for its own
/// node), and the protocols VoiceOver checks for them.
///
/// # Safety
/// `cls` must be a class pair still being built.
unsafe fn add_table_methods(cls: Class) {
    let methods: [(&str, *const c_void, &std::ffi::CStr); 12] = [
        (
            "accessibilityElements",
            el_elements as *const c_void,
            c"@@:",
        ),
        (
            "accessibilityElementCount",
            el_count as *const c_void,
            c"q@:",
        ),
        (
            "accessibilityElementAtIndex:",
            el_at as *const c_void,
            c"@@:q",
        ),
        (
            "indexOfAccessibilityElement:",
            el_index_of as *const c_void,
            c"q@:@",
        ),
        (
            "accessibilityContainerType",
            container_type as *const c_void,
            c"q@:",
        ),
        ("accessibilityRowCount", row_count as *const c_void, c"Q@:"),
        (
            "accessibilityColumnCount",
            column_count as *const c_void,
            c"Q@:",
        ),
        (
            "accessibilityDataTableCellElementForRow:column:",
            cell_for as *const c_void,
            c"@@:QQ",
        ),
        (
            "accessibilityHeaderElementsForColumn:",
            column_headers as *const c_void,
            c"@@:Q",
        ),
        (
            "accessibilityHeaderElementsForRow:",
            row_headers as *const c_void,
            c"@@:Q",
        ),
        (
            "accessibilityRowRange",
            row_range as *const c_void,
            c"{_NSRange=QQ}@:",
        ),
        (
            "accessibilityColumnRange",
            column_range as *const c_void,
            c"{_NSRange=QQ}@:",
        ),
    ];
    for (name, imp, types) in methods {
        unsafe { class_addMethod(cls, sel(name), imp, types.as_ptr()) };
    }
    for protocol in [
        c"UIAccessibilityContainerDataTable",
        c"UIAccessibilityContainerDataTableCell",
    ] {
        unsafe {
            let p = super::objc_getProtocol(protocol.as_ptr());
            if !p.is_null() {
                super::class_addProtocol(cls, p);
            }
        }
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
    /// `accessibilityHint`: a field's placeholder.
    pub hint: String,
    /// Its traits, by name (`button`, `static_text`, `header`, `selected`),
    /// joined with `|`.
    pub traits: String,
    /// `accessibilityFrame`: in screen coordinates (points).
    pub frame: crate::geometry::Rect,
    /// The object's class: `StippleAccessibilityElement` for the app's
    /// nodes, the control's (`UISwitch`) where a live OS control stands
    /// for one.
    pub class: String,
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
                    hint: string_of(msg_id(el, sel("accessibilityHint"))),
                    traits: names.join("|"),
                    frame: crate::geometry::Rect::from_xywh(
                        f.origin.x,
                        f.origin.y,
                        f.size.width,
                        f.size.height,
                    ),
                    class: crate::platform::native::objc::class_name(el),
                })
            })
            .collect()
    }
}

/// For self-tests: where VoiceOver's double tap lands on the element at
/// `index` when it doesn't activate itself (`accessibilityActivate`
/// answers NO, as a live `UIButton` does): a tap at its activation point
/// (the middle of its `accessibilityFrame`), in `view`'s coordinates, and
/// the view UIKit's hit test delivers it to there.
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn double_tap_at(view: Id, index: usize) -> Option<(Id, Point)> {
    unsafe {
        let el = msg_id_usize(view, sel("accessibilityElementAtIndex:"), index);
        if el.is_null() {
            return None;
        }
        let f = super::msg_rect(el, sel("accessibilityFrame"));
        let mid = CgPoint {
            x: f.origin.x + f.size.width / 2.0,
            y: f.origin.y + f.size.height / 2.0,
        };
        let window = msg_id(view, sel("window"));
        let screen = if window.is_null() {
            std::ptr::null_mut()
        } else {
            msg_id(window, sel("screen"))
        };
        if screen.is_null() {
            return None;
        }
        let space = msg_id(screen, sel("coordinateSpace"));
        let convert: unsafe extern "C" fn(Id, Sel, CgPoint, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let at = convert(view, sel("convertPoint:fromCoordinateSpace:"), mid, space);
        let hit_test: unsafe extern "C" fn(Id, Sel, CgPoint, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let hit = hit_test(view, sel("hitTest:withEvent:"), at, std::ptr::null_mut());
        Some((hit, Point::new(at.x, at.y)))
    }
}

/// For self-tests: the first data table `view` vends, as VoiceOver reads
/// it (see `simulate::accessibility_table_status`).
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn table_status(view: Id) -> String {
    unsafe {
        let int: unsafe extern "C" fn(Id, Sel) -> usize =
            std::mem::transmute(objc_msgSend as *const c_void);
        let at: unsafe extern "C" fn(Id, Sel, usize, usize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let range: unsafe extern "C" fn(Id, Sel) -> super::text_input::NsRange =
            std::mem::transmute(objc_msgSend as *const c_void);
        let count = msg_usize(view, sel("accessibilityElementCount"));
        let elements: Vec<Id> = (0..count)
            .map(|i| msg_id_usize(view, sel("accessibilityElementAtIndex:"), i))
            .filter(|e| !e.is_null())
            .collect();
        let responds = |e: Id, name: &str| {
            let f: unsafe extern "C" fn(Id, Sel, Sel) -> bool =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(e, sel("respondsToSelector:"), sel(name))
        };
        let label = |e: Id| {
            if e.is_null() {
                "nil".to_string()
            } else {
                format!("{:?}", string_of(msg_id(e, sel("accessibilityLabel"))))
            }
        };
        let captioned: Vec<String> = elements
            .iter()
            .filter(|&&e| {
                let value = string_of(msg_id(e, sel("accessibilityValue")));
                !value.is_empty() && value.parse::<f64>().is_ok()
            })
            .map(|&e| {
                format!(
                    "{}={:?}",
                    label(e),
                    string_of(msg_id(e, sel("accessibilityValue")))
                )
            })
            .collect();
        let table = elements.iter().copied().find(|&e| {
            responds(e, "accessibilityContainerType")
                && int(e, sel("accessibilityContainerType")) == CONTAINER_DATA_TABLE as usize
        });
        let Some(table) = table else {
            return format!("no data table; numbers {captioned:?}");
        };
        let rows = int(table, sel("accessibilityRowCount"));
        let columns = int(table, sel("accessibilityColumnCount"));
        let inside = msg_usize(table, sel("accessibilityElementCount"));
        let cell = at(
            table,
            sel("accessibilityDataTableCellElementForRow:column:"),
            1,
            0,
        );
        let (row, column, held) = if cell.is_null() {
            (None, None, false)
        } else {
            let r = range(cell, sel("accessibilityRowRange"));
            let c = range(cell, sel("accessibilityColumnRange"));
            (
                Some((r.location, r.length)),
                Some((c.location, c.length)),
                msg_id(cell, sel("accessibilityContainer")) == table,
            )
        };
        let headers = {
            let f: unsafe extern "C" fn(Id, Sel, usize) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(table, sel("accessibilityHeaderElementsForColumn:"), 0)
        };
        let header = if headers.is_null() || msg_usize(headers, sel("count")) == 0 {
            std::ptr::null_mut()
        } else {
            msg_id_usize(headers, sel("objectAtIndex:"), 0)
        };
        format!(
            "rows={rows} columns={columns} elements={inside} cell(1,0)={} row={row:?} column={column:?} in_table={held} header(0)={}; numbers {captioned:?}",
            label(cell),
            label(header),
        )
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
