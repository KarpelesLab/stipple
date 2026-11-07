//! A split pane: two panes side by side (or stacked) with a draggable divider
//! between them.

use crate::core::{Align, Axis, BoxStyle, Cx, DragMotion, Element, KeyInput, Role, Track};
use crate::style::Theme;

/// The divider's thickness, in logical pixels.
const HANDLE: f64 = 7.0;
/// How far one arrow key moves the divider, as a fraction of the split.
const KEY_STEP: f64 = 0.05;

/// The state of a [`split_pane`]: its direction, where the divider sits, and
/// the smallest each pane may get. Keep one in your app state per split.
///
/// ```
/// use stipple::widgets::SplitPane;
/// let split = SplitPane::horizontal(0.3).min_sizes(120.0, 200.0);
/// assert_eq!(split.fraction(), 0.3);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitPane {
    axis: Axis,
    fraction: f64,
    min_first: f64,
    min_second: f64,
    /// The space the two panes shared at the last drag (logical px), so keys
    /// can honor the minimum sizes too; `0` until the first drag.
    extent: f64,
}

impl SplitPane {
    /// A split along `axis` (`Horizontal`: panes side by side, divider
    /// vertical; `Vertical`: panes stacked) giving the first pane `fraction`
    /// (0..=1) of the space.
    pub fn new(axis: Axis, fraction: f64) -> Self {
        Self {
            axis,
            fraction: clamp_unit(fraction),
            min_first: 0.0,
            min_second: 0.0,
            extent: 0.0,
        }
    }

    /// Panes side by side, with a vertical divider.
    pub fn horizontal(fraction: f64) -> Self {
        Self::new(Axis::Horizontal, fraction)
    }

    /// Panes stacked, with a horizontal divider.
    pub fn vertical(fraction: f64) -> Self {
        Self::new(Axis::Vertical, fraction)
    }

    /// Never let dragging make the first pane smaller than `first` or the
    /// second smaller than `second` logical pixels (along the split's axis).
    pub fn min_sizes(mut self, first: f64, second: f64) -> Self {
        self.min_first = first.max(0.0);
        self.min_second = second.max(0.0);
        self
    }

    /// The direction the panes are laid out in.
    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// The first pane's share of the space (0..=1).
    pub fn fraction(&self) -> f64 {
        self.fraction
    }

    /// Move the divider to give the first pane `fraction` of the space.
    pub fn set_fraction(&mut self, fraction: f64) {
        self.fraction = clamp_unit(fraction);
    }

    /// `first` (the first pane's size in px out of `extent`) kept between the
    /// minimum sizes; when they can't both fit, split in their proportion.
    fn clamp_px(&self, first: f64, extent: f64) -> f64 {
        let (lo, hi) = (self.min_first, extent - self.min_second);
        if lo <= hi {
            first.clamp(lo, hi)
        } else if self.min_first + self.min_second > 0.0 {
            extent * self.min_first / (self.min_first + self.min_second)
        } else {
            extent / 2.0
        }
    }

    /// Follow a drag of the divider: the divider stays under the point it
    /// was grabbed by.
    pub fn drag(&mut self, motion: &DragMotion) {
        let (pos, start, grab, total) = match self.axis {
            Axis::Horizontal => (
                motion.position.x,
                motion.parent.min_x(),
                motion.origin.x - motion.bounds.min_x(),
                motion.parent.width(),
            ),
            Axis::Vertical => (
                motion.position.y,
                motion.parent.min_y(),
                motion.origin.y - motion.bounds.min_y(),
                motion.parent.height(),
            ),
        };
        let extent = total - HANDLE;
        if extent <= 0.0 {
            return;
        }
        self.extent = extent;
        let first = self.clamp_px(pos - start - grab, extent);
        self.fraction = clamp_unit(first / extent);
    }

    /// Handle a key pressed while the divider has focus. Returns `true` if it
    /// moved. Left/Up and Right/Down move it by 5%; Home and End move it as
    /// far as the minimum sizes allow.
    pub fn apply(&mut self, key: &KeyInput) -> bool {
        let before = self.fraction;
        let target = match key {
            KeyInput::Left | KeyInput::Up => self.fraction - KEY_STEP,
            KeyInput::Right | KeyInput::Down => self.fraction + KEY_STEP,
            KeyInput::Home => 0.0,
            KeyInput::End => 1.0,
            _ => return false,
        };
        self.fraction = if self.extent > 0.0 {
            clamp_unit(self.clamp_px(target * self.extent, self.extent) / self.extent)
        } else {
            clamp_unit(target)
        };
        self.fraction != before
    }
}

fn clamp_unit(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// A split pane: `first` and `second` side by side (or stacked, per the
/// [`SplitPane`]'s axis), with a divider between them that the pointer drags
/// and the arrow keys move while it has focus. Each pane is clipped to its
/// share. The split grows to fill its container (`grow(1.0)`); a vertical
/// split needs a height from its container to share out.
///
/// `field` is an accessor returning the [`SplitPane`] inside the app state.
///
/// Self-drawn under every theme (no OS has a matching control to draw), in the
/// theme's colors. The divider is reported to assistive technologies as a
/// [`Role::Splitter`].
pub fn split_pane<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &SplitPane,
    field: impl Fn(&mut S) -> &mut SplitPane + Copy + 'static,
    first: Element,
    second: Element,
) -> Element {
    let p = &theme.palette;
    let f = state.fraction;
    let tracks = vec![Track::fr(f), Track::px(HANDLE), Track::fr(1.0 - f)];
    let pane = |content: Element| {
        Element::stack(Axis::Vertical, vec![content])
            .align(Align::Start, Align::Stretch)
            .clip()
    };
    // A hairline along the divider, broken by a short grip in its middle.
    let (cross, grip) = match state.axis {
        Axis::Horizontal => (Axis::Vertical, (3.0, 24.0)),
        Axis::Vertical => (Axis::Horizontal, (24.0, 3.0)),
    };
    let segment = || {
        let el = Element::boxed(BoxStyle::default()).fill(p.border).grow(1.0);
        match state.axis {
            Axis::Horizontal => el.width(1.0),
            Axis::Vertical => el.height(1.0),
        }
    };
    let grip = Element::boxed(BoxStyle::default())
        .fill(p.text_muted)
        .radius(1.5)
        .width(grip.0)
        .height(grip.1);
    let handle = Element::stack(cross, vec![segment(), grip, segment()])
        .align(Align::Start, Align::Center)
        .role(Role::Splitter)
        .on_key(cx, move |s: &mut S, k| {
            field(s).apply(k);
        })
        .on_drag_motion(cx, move |s: &mut S, m| field(s).drag(m));
    let children = vec![pane(first), handle, pane(second)];
    let grid = match state.axis {
        Axis::Horizontal => Element::grid(tracks, children).rows(vec![Track::fr(1.0)]),
        Axis::Vertical => Element::grid(vec![Track::fr(1.0)], children).rows(tracks),
    };
    grid.grow(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::geometry::{Point, Rect, Size};

    fn motion(x: f64, origin_x: f64, handle_x: f64) -> DragMotion {
        DragMotion {
            position: Point::new(x, 50.0),
            origin: Point::new(origin_x, 50.0),
            bounds: Rect::from_xywh(handle_x, 0.0, HANDLE, 100.0),
            parent: Rect::from_xywh(0.0, 0.0, 400.0 + HANDLE, 100.0),
        }
    }

    #[test]
    fn drag_follows_the_grab_point_and_honors_minimums() {
        let mut s = SplitPane::horizontal(0.5).min_sizes(100.0, 50.0);
        // Grabbed 3px into the handle at x=200; drag to x=103 → first = 100.
        s.drag(&motion(103.0, 203.0, 200.0));
        assert_eq!(s.fraction(), 0.25);
        // Far left stops at the first pane's minimum (100 of 400).
        s.drag(&motion(-50.0, 203.0, 200.0));
        assert_eq!(s.fraction(), 0.25);
        // Far right stops at the second's (400 - 50 = 350).
        s.drag(&motion(1000.0, 203.0, 200.0));
        assert_eq!(s.fraction(), 0.875);
    }

    #[test]
    fn keys_move_the_divider_within_bounds() {
        let mut s = SplitPane::vertical(0.5);
        assert!(s.apply(&KeyInput::Down));
        assert!((s.fraction() - 0.55).abs() < 1e-9);
        assert!(s.apply(&KeyInput::Home));
        assert_eq!(s.fraction(), 0.0);
        assert!(!s.apply(&KeyInput::Up), "already at the start");
        assert!(!s.apply(&KeyInput::Enter));
        // Once a drag has measured the split, keys honor the minimums too.
        let mut h = SplitPane::horizontal(0.5).min_sizes(100.0, 100.0);
        h.drag(&motion(203.0, 203.0, 200.0));
        h.apply(&KeyInput::End);
        assert_eq!(h.fraction(), 0.75);
    }

    #[test]
    fn mins_that_cannot_fit_split_proportionally() {
        let mut s = SplitPane::horizontal(0.5).min_sizes(300.0, 300.0);
        s.drag(&motion(10.0, 203.0, 200.0));
        assert_eq!(s.fraction(), 0.5);
    }

    struct State {
        split: SplitPane,
    }

    #[test]
    fn dragging_the_divider_resizes_the_panes() {
        let mut app = App::new(
            State {
                split: SplitPane::horizontal(0.5),
            },
            |s: &State, cx: &mut Cx<State>| {
                let theme = *cx.theme();
                let a = Element::boxed(BoxStyle::default()).height(10.0);
                let b = Element::boxed(BoxStyle::default()).height(10.0);
                super::super::row(vec![split_pane(
                    cx,
                    &theme,
                    &s.split,
                    |s: &mut State| &mut s.split,
                    a,
                    b,
                )])
                .align(Align::Start, Align::Stretch)
            },
        )
        .logical_size(Size::new(400.0 + HANDLE, 100.0));
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let handle = tree
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::Splitter)
            .unwrap()
            .bounds;
        assert_eq!(handle.min_x(), 200.0, "the divider sits at half");
        let grab = handle.center();
        assert!(app.drag_at_point(grab));
        assert!(app.drag_at_point(Point::new(grab.x - 100.0, grab.y)));
        app.end_drag();
        assert_eq!(app.state().split.fraction(), 0.25);
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let handle = tree
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::Splitter)
            .unwrap()
            .bounds;
        assert_eq!(handle.min_x(), 100.0);
    }
}
