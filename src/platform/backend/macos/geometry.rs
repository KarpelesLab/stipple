//! Window geometry and state on macOS: where a window is, its size, whether
//! it is zoomed, miniaturized or fullscreen, and the work area of its screen
//! ([`WindowInfo`]), kept up to date from the window delegate's
//! notifications; and moving, resizing and resize increments.
//!
//! AppKit's desktop coordinates have their origin at the bottom-left of the
//! primary screen with y going up; Stipple's have it at the top-left with y
//! going down, so every rectangle is flipped against the primary screen's
//! height.

use std::ffi::c_void;

use super::{
    CTX, CgPoint, CgSize, Class, Id, MacWindow, Sel, class, class_addMethod, msg_bool, msg_f64,
    msg_id, msg_id_u64, msg_rect, msg_u64, msg_void_size, nsstring_to_string, objc_msgSend, sel,
    window_for_view,
};
use crate::geometry::{Point, Rect, ScaleFactor, Size};
use crate::platform::event::Event;
use crate::platform::window::{WindowInfo, debug_window};

/// `NSWindowStyleMaskFullScreen`.
const STYLE_FULLSCREEN: u64 = 1 << 14;

/// A Cocoa rectangle (bottom-left origin, y up) in Stipple's desktop
/// coordinates (top-left origin, y down), given the primary screen's height.
pub(super) fn flip_rect(x: f64, y: f64, w: f64, h: f64, primary_h: f64) -> Rect {
    Rect::from_xywh(x, primary_h - (y + h), w, h)
}

/// The Cocoa point (for `setFrameTopLeftPoint:`) of a window top-left corner
/// at `p` in Stipple's desktop coordinates.
pub(super) fn cocoa_top_left(p: Point, primary_h: f64) -> (f64, f64) {
    (p.x, primary_h - p.y)
}

/// The height of the primary screen (the one with the menu bar), which
/// AppKit's desktop coordinates start from; 0 without screens.
unsafe fn primary_height() -> f64 {
    unsafe {
        let screens = msg_id(class("NSScreen"), sel("screens"));
        if screens.is_null() || msg_u64(screens, sel("count")) == 0 {
            return 0.0;
        }
        let first = msg_id_u64(screens, sel("objectAtIndex:"), 0);
        msg_rect(first, sel("frame")).size.height
    }
}

/// Read `window`'s geometry and state from AppKit.
///
/// # Safety
/// `window` must be a live `NSWindow` and `view` its content view.
pub(super) unsafe fn read_info(window: Id, view: Id) -> WindowInfo {
    unsafe {
        let ph = primary_height();
        let frame = msg_rect(window, sel("frame"));
        let bounds = msg_rect(view, sel("bounds"));
        let outer = flip_rect(
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
            ph,
        );
        let screen = msg_id(window, sel("screen"));
        let work_area = (!screen.is_null()).then(|| {
            let v = msg_rect(screen, sel("visibleFrame"));
            flip_rect(v.origin.x, v.origin.y, v.size.width, v.size.height, ph)
        });
        WindowInfo {
            position: Some(Point::new(outer.min_x(), outer.min_y())),
            inner_size: Size::new(bounds.size.width, bounds.size.height),
            outer_size: Size::new(frame.size.width, frame.size.height),
            scale: ScaleFactor::new(msg_f64(window, sel("backingScaleFactor"))),
            maximized: msg_bool(window, sel("isZoomed")),
            minimized: msg_bool(window, sel("isMiniaturized")),
            fullscreen: msg_u64(window, sel("styleMask")) & STYLE_FULLSCREEN != 0,
            work_area,
        }
    }
}

/// Whether anything but the sizes and scale (which come with
/// [`Event::Resized`]) differs.
fn state_differs(a: &WindowInfo, b: &WindowInfo) -> bool {
    a.position != b.position
        || a.maximized != b.maximized
        || a.minimized != b.minimized
        || a.fullscreen != b.fullscreen
        || a.work_area != b.work_area
}

impl MacWindow {
    /// Read the window's info again; when it moved or changed state, queue
    /// [`Event::WindowChanged`] for it.
    pub(super) fn refresh_info(&self) {
        let now = unsafe { read_info(self.window, self.view) };
        let before = self.info.replace(now);
        if state_differs(&before, &now) {
            debug_window(format_args!(
                "state {:#x} maximized={} minimized={} fullscreen={}",
                self.id, now.maximized, now.minimized, now.fullscreen
            ));
            CTX.with(|c| {
                c.borrow_mut()
                    .window_pending
                    .push((self.view, Event::WindowChanged))
            });
        }
    }

    /// The window's title as AppKit has it.
    pub(super) fn title(&self) -> String {
        nsstring_to_string(unsafe { msg_id(self.window, sel("title")) })
    }

    /// Print the frame as AppKit reports it (with `STIPPLE_DEBUG_WINDOWS`).
    pub(super) fn debug_frame(&self) {
        let info = unsafe { read_info(self.window, self.view) };
        let pos = info.position.unwrap_or(Point::new(0.0, 0.0));
        debug_window(format_args!(
            "frame {:#x} pos={},{} inner={}x{}",
            self.id,
            pos.x.round(),
            pos.y.round(),
            info.inner_size.width.round(),
            info.inner_size.height.round()
        ));
    }

    /// Move the frame's top-left corner to `p` (desktop logical points).
    pub(super) fn move_to(&self, p: Point) {
        unsafe {
            let (x, y) = cocoa_top_left(p, primary_height());
            let set: unsafe extern "C" fn(Id, Sel, CgPoint) =
                std::mem::transmute(objc_msgSend as *const c_void);
            set(self.window, sel("setFrameTopLeftPoint:"), CgPoint { x, y });
        }
    }

    /// Resize the content area to `size` points.
    pub(super) fn resize_content(&self, size: Size) {
        unsafe {
            msg_void_size(
                self.window,
                sel("setContentSize:"),
                CgSize {
                    width: size.width.max(1.0),
                    height: size.height.max(1.0),
                },
            )
        };
    }

    /// Snap resizing to steps of `step` points, or free it (1×1).
    pub(super) fn resize_increments(&self, step: Option<Size>) {
        let s = step.unwrap_or(Size::new(1.0, 1.0));
        unsafe {
            msg_void_size(
                self.window,
                sel("setContentResizeIncrements:"),
                CgSize {
                    width: s.width.max(1.0),
                    height: s.height.max(1.0),
                },
            );
            let get: unsafe extern "C" fn(Id, Sel) -> CgSize =
                std::mem::transmute(objc_msgSend as *const c_void);
            let back = get(self.window, sel("contentResizeIncrements"));
            match step {
                Some(_) => debug_window(format_args!(
                    "increments {:#x} {}x{}",
                    self.id,
                    back.width.round(),
                    back.height.round()
                )),
                None => debug_window(format_args!("increments {:#x} none", self.id)),
            }
        }
    }
}

/// The window delegate's move / zoom / miniaturize / fullscreen / screen
/// notifications (the view is the delegate): refresh the window's info.
extern "C" fn state_changed(this: Id, _cmd: Sel, _notification: Id) {
    if let Some(w) = window_for_view(this) {
        w.refresh_info();
    }
}

/// `windowShouldClose:` — the close button (or ⌘W): ask the app, which closes
/// the window itself with `Window::close_window` if it agrees.
extern "C" fn window_should_close(this: Id, _cmd: Sel, _sender: Id) -> i8 {
    CTX.with(|c| {
        c.borrow_mut()
            .window_pending
            .push((this, Event::CloseRequested))
    });
    0
}

/// `windowWillClose:` — the window is closing (whoever closed it): forget it
/// once the handler returns.
extern "C" fn window_will_close(this: Id, _cmd: Sel, _notification: Id) {
    if let Some(w) = window_for_view(this) {
        w.closing.set(true);
    }
}

/// Add the window-delegate methods to the view class.
///
/// # Safety
/// `cls` must be the view class, not yet registered.
pub(super) unsafe fn add_methods(cls: Class) {
    unsafe {
        for name in [
            "windowDidMove:",
            "windowDidResize:",
            "windowDidMiniaturize:",
            "windowDidDeminiaturize:",
            "windowDidEnterFullScreen:",
            "windowDidExitFullScreen:",
            "windowDidChangeScreen:",
        ] {
            class_addMethod(
                cls,
                sel(name),
                state_changed as *const c_void,
                c"v@:@".as_ptr(),
            );
        }
        class_addMethod(
            cls,
            sel("windowShouldClose:"),
            window_should_close as *const c_void,
            c"c@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("windowWillClose:"),
            window_will_close as *const c_void,
            c"v@:@".as_ptr(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flips_against_the_primary_screen() {
        // A 400x300 frame whose bottom-left is at (100, 200) on a 900-point
        // tall primary screen has its top-left 400 points from the top.
        let r = flip_rect(100.0, 200.0, 400.0, 300.0, 900.0);
        assert_eq!(r, Rect::from_xywh(100.0, 400.0, 400.0, 300.0));
        // A screen below the primary one has negative Cocoa y.
        let r = flip_rect(0.0, -768.0, 1024.0, 768.0, 900.0);
        assert_eq!(r.min_y(), 900.0);
    }

    #[test]
    fn top_left_round_trips() {
        let (x, y) = cocoa_top_left(Point::new(50.0, 120.0), 900.0);
        assert_eq!((x, y), (50.0, 780.0));
        // The frame whose top-left that is flips back to the same point.
        let h = 200.0;
        let r = flip_rect(x, y - h, 300.0, h, 900.0);
        assert_eq!((r.min_x(), r.min_y()), (50.0, 120.0));
    }

    #[test]
    fn only_state_changes_count() {
        let a = WindowInfo::default();
        let mut b = a;
        b.inner_size = Size::new(10.0, 10.0);
        b.scale = ScaleFactor::new(2.0);
        assert!(!state_differs(&a, &b));
        b.minimized = true;
        assert!(state_differs(&a, &b));
    }
}
