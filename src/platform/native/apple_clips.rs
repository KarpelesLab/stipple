//! UIKit and AppKit: a clipping container per scroll area, for the live
//! controls in it (see [`NativeClip`]).
//!
//! Each scroll area that holds live controls is a `StippleClipView` (a
//! `UIView` / `NSView` that clips its subviews to its bounds, flipped on
//! macOS) at the area's viewport; its controls are its subviews, so the OS
//! cuts one half scrolled out at the edge, live. A nested area's container
//! is a subview of its outer one's. The container itself takes no input:
//! a touch or click that lands on it but on none of its controls goes
//! through to the app's view (`pointInside:withEvent:` / `hitTest:` find
//! only its subviews), so the app still scrolls the area and handles the
//! rest of what's in it.
//!
//! The containers are kept per window view (macOS has one per window; iOS
//! one): [`prepare`] makes, moves and reparents them for a frame's list
//! before the controls are synced, [`finish`] removes the ones no control is
//! in any more.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use super::objc::{CgRect, Id, Sel, class, msg, sel};
use super::{NativeClip, NativeView};
use crate::geometry::Rect;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct CgPoint {
    x: f64,
    y: f64,
}

/// One scroll area's container.
struct Area {
    view: Id,
    /// The area it is in, if any.
    parent: Option<u64>,
    /// Its viewport, in the window's logical pixels.
    frame: Rect,
    /// Its frame in its parent, as last set.
    local: Rect,
}

thread_local! {
    /// Each window view's containers, by area id.
    static AREAS: RefCell<HashMap<usize, HashMap<u64, Area>>> = RefCell::new(HashMap::new());
}

/// The container class, registered once.
fn clip_class() -> Id {
    thread_local! {
        static CLS: std::cell::Cell<Id> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }
    CLS.with(|c| {
        if c.get().is_null() {
            c.set(unsafe { register() });
        }
        c.get()
    })
}

unsafe fn register() -> Id {
    unsafe {
        let name = c"StippleClipView";
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let base = if cfg!(target_os = "ios") {
            "UIView"
        } else {
            "NSView"
        };
        let cls = objc_allocateClassPair(class(base), name.as_ptr(), 0);
        #[cfg(target_os = "ios")]
        class_addMethod(
            cls,
            sel("pointInside:withEvent:"),
            point_inside as *const c_void,
            c"B@:{CGPoint=dd}@".as_ptr(),
        );
        #[cfg(target_os = "macos")]
        {
            class_addMethod(
                cls,
                sel("hitTest:"),
                hit_test as *const c_void,
                c"@@:{CGPoint=dd}".as_ptr(),
            );
            class_addMethod(cls, sel("isFlipped"), yes as *const c_void, c"B@:".as_ptr());
        }
        objc_registerClassPair(cls);
        cls
    }
}

fn contains(r: CgRect, p: CgPoint) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}

/// `pointInside:withEvent:` (UIKit): inside its bounds and on one of its
/// subviews (a control, or a nested container's control), so a touch on
/// the rest of it goes on to the app's view.
#[cfg(target_os = "ios")]
extern "C" fn point_inside(this: Id, _cmd: Sel, p: CgPoint, event: Id) -> bool {
    unsafe {
        let bounds: CgRect = msg!(this, "bounds", () -> CgRect);
        if !contains(bounds, p) {
            return false;
        }
        let subs: Id = msg!(this, "subviews", () -> Id);
        let n: usize = msg!(subs, "count", () -> usize);
        (0..n).rev().any(|i| {
            let sub: Id = msg!(subs, "objectAtIndex:", (i: usize) -> Id);
            let hidden: bool = msg!(sub, "isHidden", () -> bool);
            let takes: bool = msg!(sub, "isUserInteractionEnabled", () -> bool);
            if hidden || !takes {
                return false;
            }
            let q: CgPoint = msg!(this, "convertPoint:toView:", (p: CgPoint, sub: Id) -> CgPoint);
            msg!(sub, "pointInside:withEvent:", (q: CgPoint, event: Id) -> bool)
        })
    }
}

/// `hitTest:` (AppKit; `p` in the superview's coordinates): one of its
/// subviews under `p` inside its frame, else nil (not itself), so a click
/// on the rest of it goes to the app's view.
#[cfg(target_os = "macos")]
extern "C" fn hit_test(this: Id, _cmd: Sel, p: CgPoint) -> Id {
    unsafe {
        let nil: Id = std::ptr::null_mut();
        let hidden: bool = msg!(this, "isHidden", () -> bool);
        let frame: CgRect = msg!(this, "frame", () -> CgRect);
        if hidden || !contains(frame, p) {
            return nil;
        }
        let up: Id = msg!(this, "superview", () -> Id);
        let local: CgPoint = msg!(this, "convertPoint:fromView:", (p: CgPoint, up: Id) -> CgPoint);
        let subs: Id = msg!(this, "subviews", () -> Id);
        let n: usize = msg!(subs, "count", () -> usize);
        for i in (0..n).rev() {
            let sub: Id = msg!(subs, "objectAtIndex:", (i: usize) -> Id);
            let hit: Id = msg!(sub, "hitTest:", (local: CgPoint) -> Id);
            if !hit.is_null() {
                return hit;
            }
        }
        nil
    }
}

#[cfg(target_os = "macos")]
extern "C" fn yes(_this: Id, _cmd: Sel) -> bool {
    true
}

fn cg(r: Rect) -> CgRect {
    CgRect {
        x: r.min_x(),
        y: r.min_y(),
        width: r.width(),
        height: r.height(),
    }
}

/// Where `v` goes in the view it is a subview of (its innermost area's
/// container, else the window's view), in that view's coordinates.
pub(super) fn frame_in_parent(v: &NativeView) -> CgRect {
    let origin = v
        .clips
        .last()
        .map_or((0.0, 0.0), |c| (c.frame.min_x(), c.frame.min_y()));
    let mut f = cg(v.frame);
    f.x -= origin.0;
    f.y -= origin.1;
    f
}

/// The view `v`'s control goes in: its innermost area's container (made by
/// [`prepare`]), else `root`, the window's view.
pub(super) fn parent_of(root: Id, v: &NativeView) -> Id {
    let Some(clip) = v.clips.last() else {
        return root;
    };
    AREAS.with(|a| {
        a.borrow()
            .get(&(root as usize))
            .and_then(|areas| areas.get(&clip.id))
            .map_or(root, |area| area.view)
    })
}

/// Make, move and reparent the containers of the areas `views` are in,
/// outer ones first, in window view `root`.
pub(super) fn prepare(root: Id, views: &[NativeView]) {
    let mut wanted: Vec<(NativeClip, Option<u64>)> = Vec::new();
    for v in views {
        for (i, clip) in v.clips.iter().enumerate() {
            if !wanted.iter().any(|(c, _)| c.id == clip.id) {
                wanted.push((*clip, i.checked_sub(1).map(|p| v.clips[p].id)));
            }
        }
    }
    if wanted.is_empty() && AREAS.with(|a| !a.borrow().contains_key(&(root as usize))) {
        return;
    }
    AREAS.with(|a| {
        let mut all = a.borrow_mut();
        let areas = all.entry(root as usize).or_default();
        // Outer ones first: an area's outer one is listed before it.
        for (clip, parent) in wanted {
            let parent_view = parent.and_then(|p| areas.get(&p)).map_or(root, |a| a.view);
            let origin = parent
                .and_then(|p| areas.get(&p))
                .map_or((0.0, 0.0), |a| (a.frame.min_x(), a.frame.min_y()));
            let mut f = cg(clip.frame);
            f.x -= origin.0;
            f.y -= origin.1;
            unsafe {
                match areas.get_mut(&clip.id) {
                    Some(area) => {
                        if area.parent != parent {
                            reparent(area.view, parent_view);
                            area.parent = parent;
                        }
                        let local = Rect::from_xywh(f.x, f.y, f.width, f.height);
                        if area.local != local {
                            msg!(area.view, "setFrame:", (f: CgRect) -> ());
                            area.local = local;
                        }
                        area.frame = clip.frame;
                    }
                    None => {
                        let view = make(f);
                        msg!(parent_view, "addSubview:", (view: Id) -> ());
                        areas.insert(
                            clip.id,
                            Area {
                                view,
                                parent,
                                frame: clip.frame,
                                local: Rect::from_xywh(f.x, f.y, f.width, f.height),
                            },
                        );
                    }
                }
            }
        }
    });
}

/// Remove the containers no control in `views` is in any more (once the
/// controls have moved out of them).
pub(super) fn finish(root: Id, views: &[NativeView]) {
    AREAS.with(|a| {
        let mut all = a.borrow_mut();
        let Some(areas) = all.get_mut(&(root as usize)) else {
            return;
        };
        let used: std::collections::HashSet<u64> = views
            .iter()
            .flat_map(|v| v.clips.iter().map(|c| c.id))
            .collect();
        areas.retain(|id, area| {
            let keep = used.contains(id);
            if !keep {
                unsafe {
                    msg!(area.view, "removeFromSuperview", () -> ());
                    msg!(area.view, "release", () -> ());
                }
            }
            keep
        });
        if areas.is_empty() {
            all.remove(&(root as usize));
        }
    });
}

/// Bring each of `views`' containers to the front of its parent, in
/// `views`' order (paint order), as each first comes up; then `control`
/// is brought up in its own parent by the caller.
pub(super) fn raise(root: Id, v: &NativeView, raised: &mut Vec<u64>) {
    for clip in &v.clips {
        if raised.contains(&clip.id) {
            continue;
        }
        raised.push(clip.id);
        let view = AREAS.with(|a| {
            a.borrow()
                .get(&(root as usize))
                .and_then(|areas| areas.get(&clip.id))
                .map(|a| a.view)
        });
        if let Some(view) = view {
            unsafe { bring_to_front(view) };
        }
    }
}

/// Put `view` above its siblings.
pub(super) unsafe fn bring_to_front(view: Id) {
    unsafe {
        let parent: Id = msg!(view, "superview", () -> Id);
        if parent.is_null() {
            return;
        }
        if cfg!(target_os = "ios") {
            msg!(parent, "bringSubviewToFront:", (view: Id) -> ());
        } else {
            reparent(view, parent);
        }
    }
}

/// Move `view` into `parent` (on top of its subviews).
pub(super) unsafe fn reparent(view: Id, parent: Id) {
    unsafe {
        msg!(view, "retain", () -> Id);
        msg!(view, "removeFromSuperview", () -> ());
        msg!(parent, "addSubview:", (view: Id) -> ());
        msg!(view, "release", () -> ());
    }
}

/// A new container at `f` (in its parent's coordinates), retained.
unsafe fn make(f: CgRect) -> Id {
    unsafe {
        let alloc: Id = msg!(clip_class(), "alloc", () -> Id);
        let view: Id = msg!(alloc, "initWithFrame:", (f: CgRect) -> Id);
        let yes = true;
        #[cfg(target_os = "macos")]
        msg!(view, "setWantsLayer:", (yes: bool) -> ());
        let s = sel("setClipsToBounds:");
        if msg!(view, "respondsToSelector:", (s: Sel) -> bool) {
            msg!(view, "setClipsToBounds:", (yes: bool) -> ());
        }
        let layer: Id = msg!(view, "layer", () -> Id);
        if !layer.is_null() {
            msg!(layer, "setMasksToBounds:", (yes: bool) -> ());
            // Over the frame's layer and a Metal layer over it, as the
            // controls are.
            let z = 1.0_f64;
            msg!(layer, "setZPosition:", (z: f64) -> ());
        }
        view
    }
}

/// The container `control` is in (its superview, when that is one), for
/// the self-tests: its class name, and the part of `control` it and the
/// containers around it show, in `root`'s coordinates.
pub(super) fn shown(root: Id, control: Id) -> (String, Rect) {
    unsafe {
        let bounds: CgRect = msg!(control, "bounds", () -> CgRect);
        let mut r = in_root(root, control, bounds);
        let mut class_name = String::new();
        let mut v: Id = msg!(control, "superview", () -> Id);
        while !v.is_null() && v != root {
            let name = super::objc::class_name(v);
            if name == "StippleClipView" {
                if class_name.is_empty() {
                    class_name = name;
                }
                let b: CgRect = msg!(v, "bounds", () -> CgRect);
                let c = in_root(root, v, b);
                r = r
                    .intersection(c)
                    .unwrap_or(Rect::from_xywh(r.min_x(), r.min_y(), 0.0, 0.0));
            }
            v = msg!(v, "superview", () -> Id);
        }
        (class_name, r)
    }
}

/// `rect` in `view`'s coordinates, in `root`'s.
unsafe fn in_root(root: Id, view: Id, rect: CgRect) -> Rect {
    unsafe {
        let r: CgRect = msg!(view, "convertRect:toView:", (rect: CgRect, root: Id) -> CgRect);
        Rect::from_xywh(r.x, r.y, r.width, r.height)
    }
}

/// Forget window view `root`'s containers (it closed).
#[cfg(target_os = "macos")]
pub(super) fn forget(root: Id) {
    finish(root, &[]);
}
