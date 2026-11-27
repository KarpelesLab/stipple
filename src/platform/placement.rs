//! Where a window was, to bring it back there next time: its restored
//! position and size and whether it was maximized ([`WindowPlacement`]),
//! written as a few lines of text, and kept on a monitor that is still
//! there when it comes back ([`keep_on_screen`]).
//!
//! [`App::remember_window`](crate::App::remember_window) (or
//! [`WindowAttributes::remember`]) does it all; this is the part an app
//! that keeps its own settings can use directly.

// Only desktop backends place windows on monitors.
#![cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "fullrust",
        target_os = "windows",
        target_os = "macos"
    )),
    allow(dead_code)
)]

use crate::geometry::{Point, Rect, Size};
use crate::platform::{WindowAttributes, WindowInfo};

/// A window's placement: the bounds it has when neither maximized nor
/// fullscreen, and whether it is maximized. Fullscreen isn't kept: a window
/// that was fullscreen comes back in its restored bounds.
///
/// ```
/// use stipple::geometry::{Point, Size};
/// use stipple::platform::WindowPlacement;
///
/// let p = WindowPlacement::new(Size::new(800.0, 600.0))
///     .position(Point::new(40.0, 30.0))
///     .maximized(true);
/// assert_eq!(WindowPlacement::from_text(&p.to_text()), Some(p));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WindowPlacement {
    /// The outer top-left corner, in desktop logical pixels, or `None` where
    /// the system doesn't tell (Wayland): the window manager then places it.
    pub position: Option<Point>,
    /// The content area's size, logical pixels.
    pub size: Size,
    /// Whether it was maximized (it comes back maximized, and restores to
    /// [`position`](Self::position) and [`size`](Self::size)).
    pub maximized: bool,
}

/// No real window is larger than this, in logical pixels: a larger value
/// read back is a damaged file.
const LARGEST: f64 = 100_000.0;

impl WindowPlacement {
    /// A window `size` big (its content area, logical pixels), placed by
    /// the window manager and not maximized.
    pub fn new(size: Size) -> Self {
        Self {
            position: None,
            size,
            maximized: false,
        }
    }

    /// At outer top-left corner `position`, in desktop logical pixels.
    pub fn position(mut self, position: Point) -> Self {
        self.position = Some(position);
        self
    }

    /// Maximized or not.
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.maximized = maximized;
        self
    }

    /// The placement of a window as `info` says it is: its restored
    /// position and size (see [`WindowInfo::restored_position`]) and
    /// whether it is maximized.
    pub fn of(info: &WindowInfo) -> Self {
        Self {
            position: info.restored_position,
            size: info.restored_size,
            maximized: info.maximized,
        }
    }

    /// `attrs` opening at this placement: at its position (where there is
    /// one), its size, maximized or not, and not at the content's natural
    /// size (the user chose this one).
    pub fn apply(&self, mut attrs: WindowAttributes) -> WindowAttributes {
        if let Some(p) = self.position {
            attrs.position = Some((p.x.round() as i32, p.y.round() as i32));
        }
        attrs.logical_size = self.size;
        attrs.maximized = self.maximized;
        attrs.natural_size = false;
        attrs
    }

    /// The placement as text, one `key=value` per line.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(p) = self.position {
            out += &format!("x={}\ny={}\n", p.x, p.y);
        }
        out += &format!(
            "width={}\nheight={}\nmaximized={}\n",
            self.size.width, self.size.height, self.maximized
        );
        out
    }

    /// Read [`to_text`](Self::to_text)'s text back; `None` without a size,
    /// or with numbers no window could have. Unknown lines are skipped.
    pub fn from_text(text: &str) -> Option<Self> {
        let (mut x, mut y, mut w, mut h, mut maximized) = (None, None, None, None, false);
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let num = || value.parse::<f64>().ok().filter(|v| v.abs() <= LARGEST);
            match key.trim() {
                "x" => x = num(),
                "y" => y = num(),
                "width" => w = num(),
                "height" => h = num(),
                "maximized" => maximized = value == "true",
                _ => {}
            }
        }
        let (w, h) = (w?, h?);
        if w < 1.0 || h < 1.0 {
            return None;
        }
        Some(Self {
            position: x.zip(y).map(|(x, y)| Point::new(x, y)),
            size: Size::new(w, h),
            maximized,
        })
    }
}

/// How much of a window's top edge must show on a monitor for the user to
/// reach its title bar and drag it: this tall, and this wide (or all of a
/// narrower window).
const TITLE_BAND: f64 = 32.0;
const REACHABLE_WIDTH: f64 = 64.0;
/// How far above a monitor's top a window may start and still count as on
/// it (frames and shadows outside the title bar).
const TOP_SLACK: f64 = 8.0;

/// `window` (outer bounds) kept on the monitors whose work `areas` are
/// given, in the same units: no larger than the monitor it is mostly on
/// (but never under `min`), and moved fully onto it when its title bar
/// can't be reached (off every monitor, or above one's top). A window whose
/// title bar shows somewhere stays where it is. With no areas known it is
/// left as it is.
pub(crate) fn keep_on_screen(window: Rect, min: Option<Size>, areas: &[Rect]) -> Rect {
    let overlap = |a: &Rect| {
        window
            .intersection(*a)
            .map_or(0.0, |r| r.width() * r.height())
    };
    let Some(target) = areas
        .iter()
        .copied()
        .filter(|a| a.width() > 0.0 && a.height() > 0.0)
        .max_by(|a, b| overlap(a).total_cmp(&overlap(b)))
    else {
        return window;
    };
    // The monitor it is most on, or (on none) the first.
    let target = if overlap(&target) > 0.0 {
        target
    } else {
        areas
            .iter()
            .copied()
            .find(|a| a.width() > 0.0)
            .unwrap_or(target)
    };
    let min = min.unwrap_or(Size::ZERO);
    let w = window.width().min(target.width()).max(min.width);
    let h = window.height().min(target.height()).max(min.height);
    let (x, y) = (window.min_x(), window.min_y());
    let band = Rect::from_xywh(x, y, w, TITLE_BAND.min(h));
    let reachable = areas.iter().any(|a| {
        band.intersection(*a).is_some_and(|r| {
            r.width() >= REACHABLE_WIDTH.min(w) && r.height() >= (TITLE_BAND / 2.0).min(h)
        }) && y >= a.min_y() - TOP_SLACK
    });
    if reachable {
        return Rect::from_xywh(x, y, w, h);
    }
    let clamp = |v: f64, lo: f64, room: f64, len: f64| {
        if len >= room {
            lo
        } else {
            v.clamp(lo, lo + room - len)
        }
    };
    Rect::from_xywh(
        clamp(x, target.min_x(), target.width(), w),
        clamp(y, target.min_y(), target.height(), h),
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::from_xywh(x, y, w, h)
    }

    #[test]
    fn a_placement_round_trips_through_its_text() {
        let p = WindowPlacement::new(Size::new(800.0, 600.0))
            .position(Point::new(-12.0, 40.5))
            .maximized(true);
        assert_eq!(WindowPlacement::from_text(&p.to_text()), Some(p));
        // Without a position (Wayland).
        let p = WindowPlacement {
            position: None,
            ..p
        };
        assert_eq!(p.to_text(), "width=800\nheight=600\nmaximized=true\n");
        assert_eq!(WindowPlacement::from_text(&p.to_text()), Some(p));
    }

    #[test]
    fn a_damaged_placement_is_none() {
        assert_eq!(WindowPlacement::from_text(""), None);
        assert_eq!(WindowPlacement::from_text("width=800\n"), None);
        assert_eq!(WindowPlacement::from_text("width=0\nheight=10"), None);
        assert_eq!(WindowPlacement::from_text("width=nan\nheight=10"), None);
        assert_eq!(WindowPlacement::from_text("width=1e9\nheight=10"), None);
        // An x without a y is no position.
        let p = WindowPlacement::from_text("x=5\nwidth=10\nheight=10\nnew=1").unwrap();
        assert_eq!(p.position, None);
    }

    #[test]
    fn it_applies_to_the_attributes() {
        let p = WindowPlacement::new(Size::new(640.0, 480.0))
            .position(Point::new(10.4, 20.6))
            .maximized(true);
        let attrs = p.apply(WindowAttributes::new().natural_size(true));
        assert_eq!(attrs.position, Some((10, 21)));
        assert_eq!(attrs.logical_size, Size::new(640.0, 480.0));
        assert!(attrs.maximized && !attrs.natural_size);
        // No position: the window manager's choice stays.
        let p = WindowPlacement {
            position: None,
            ..p
        };
        let attrs = p.apply(WindowAttributes::new().position(1, 2));
        assert_eq!(attrs.position, Some((1, 2)));
    }

    #[test]
    fn a_window_on_a_monitor_stays_where_it_is() {
        let areas = [r(0.0, 0.0, 1920.0, 1040.0), r(1920.0, 0.0, 1280.0, 1024.0)];
        let w = r(100.0, 100.0, 800.0, 600.0);
        assert_eq!(keep_on_screen(w, None, &areas), w);
        // Straddling the two, or hanging off the right with its title bar
        // in reach.
        let w = r(1500.0, 100.0, 800.0, 600.0);
        assert_eq!(keep_on_screen(w, None, &areas), w);
        let w = r(2900.0, 100.0, 800.0, 600.0);
        assert_eq!(keep_on_screen(w, None, &areas), w);
    }

    #[test]
    fn a_window_on_a_monitor_now_gone_comes_onto_one_that_is_there() {
        // It was on a second monitor, at the right.
        let areas = [r(0.0, 0.0, 1920.0, 1040.0)];
        let w = r(2100.0, 300.0, 800.0, 600.0);
        assert_eq!(
            keep_on_screen(w, None, &areas),
            r(1120.0, 300.0, 800.0, 600.0)
        );
        // Far away: onto the first monitor, inside it.
        let w = r(-5000.0, -5000.0, 800.0, 600.0);
        assert_eq!(keep_on_screen(w, None, &areas), r(0.0, 0.0, 800.0, 600.0));
        // Its title bar above the top of the screen.
        let w = r(100.0, -200.0, 800.0, 600.0);
        assert_eq!(keep_on_screen(w, None, &areas), r(100.0, 0.0, 800.0, 600.0));
        // Only a sliver showing at the left edge.
        let w = r(-790.0, 100.0, 800.0, 600.0);
        assert_eq!(keep_on_screen(w, None, &areas), r(0.0, 100.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_larger_than_the_monitor_shrinks_but_not_below_its_minimum() {
        let areas = [r(0.0, 0.0, 1366.0, 728.0)];
        let w = r(0.0, 0.0, 2560.0, 1400.0);
        assert_eq!(keep_on_screen(w, None, &areas), r(0.0, 0.0, 1366.0, 728.0));
        let min = Some(Size::new(1500.0, 300.0));
        assert_eq!(keep_on_screen(w, min, &areas), r(0.0, 0.0, 1500.0, 728.0));
    }

    #[test]
    fn with_no_monitors_known_nothing_changes() {
        let w = r(-5000.0, 0.0, 10.0, 10.0);
        assert_eq!(keep_on_screen(w, None, &[]), w);
    }
}
