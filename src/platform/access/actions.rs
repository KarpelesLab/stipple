//! What an assistive technology's request turns into: the events each
//! bridge hands the app for a click, a text change, a selection or a scroll,
//! the same on every platform (UIA's patterns, NSAccessibility's setters,
//! AT-SPI's interfaces, Android's actions, iOS's `accessibilityScroll:`).
//!
//! A click is a pointer move, press and release at the node's middle; text
//! goes through the input-method path a focused field already takes
//! (focused by a click first); a scroll is a wheel at the container's
//! middle, which the app gives the innermost container under it that scrolls
//! that way.

// Not every bridge (nor every target) uses every helper; the tests do.
#![allow(dead_code)]

use super::{A11yNode, A11yRole, A11yScroll};
use crate::geometry::Point;
use crate::platform::event::{ButtonState, Event, Ime, PointerButton, ScrollDelta};

/// Whether a node of `role` takes typed text.
pub(crate) fn editable(role: A11yRole) -> bool {
    matches!(
        role,
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField
    )
}

/// Where a scroll's step lands, as a fraction of the viewport: a "page" an
/// assistive technology asks for, short of a whole viewport so a line of
/// context stays in view (as Android's own lists scroll).
const PAGE: f64 = 0.8;

impl A11yScroll {
    /// How far it is scrolled, `0..=1` (0 when it can't scroll).
    pub(crate) fn fraction(&self) -> f64 {
        if self.max > 0.0 {
            (self.offset / self.max).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Whether its content overflows, so it scrolls at all.
    pub(crate) fn scrolls(&self) -> bool {
        self.max > 0.5
    }

    /// One page: what "scroll forward" moves.
    pub(crate) fn page(&self) -> f64 {
        (self.viewport * PAGE).max(crate::platform::event::SCROLL_LINE_PX)
    }

    /// The share of the content in view, `0..=1` (UIA's view size, in
    /// percent there).
    pub(crate) fn view_fraction(&self) -> f64 {
        let total = self.viewport + self.max;
        if total > 0.0 {
            (self.viewport / total).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// The wheel delta `(dx, dy)` that moves the offset by `by` (positive:
    /// towards the end). Sideways, a right-to-left strip's offset grows to
    /// the left, which a wheel reaches scrolling left.
    pub(crate) fn wheel(&self, by: f64) -> (f64, f64) {
        match (self.horizontal, self.rtl) {
            (false, _) => (0.0, by),
            (true, false) => (by, 0.0),
            (true, true) => (-by, 0.0),
        }
    }

    /// The change in offset that reaches `offset` (clamped to the range).
    pub(crate) fn to(&self, offset: f64) -> f64 {
        offset.clamp(0.0, self.max.max(0.0)) - self.offset
    }
}

impl A11yNode {
    /// The middle of the node, in the window's logical pixels.
    pub(crate) fn center(&self) -> Point {
        let (x, y, w, h) = self.bounds;
        Point::new(x + w / 2.0, y + h / 2.0)
    }

    /// The pointer events of a click at the node's middle.
    pub(crate) fn click_events(&self) -> Vec<Event> {
        let position = self.center();
        vec![
            Event::PointerMoved { position },
            Event::PointerButton {
                button: PointerButton::Left,
                state: ButtonState::Pressed,
                position,
            },
            Event::PointerButton {
                button: PointerButton::Left,
                state: ButtonState::Released,
                position,
            },
        ]
    }

    /// A click to focus the node, unless it has the focus.
    fn focus_events(&self) -> Vec<Event> {
        if self.focused {
            Vec::new()
        } else {
            self.click_events()
        }
    }

    /// Select the bytes `anchor..caret` of an editable field's text (a plain
    /// caret when equal): focus it unless it has the focus, then have the
    /// field select them. `None` when the tree doesn't carry the field's
    /// caret (its name isn't its text) or it isn't a field.
    pub(crate) fn select_events(&self, anchor: usize, caret: usize) -> Option<Vec<Event>> {
        if !editable(self.role) || self.caret.is_none() {
            return None;
        }
        let snap = |i: usize| {
            let mut i = i.min(self.name.len());
            while !self.name.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let mut out = self.focus_events();
        out.push(Event::Ime(Ime::SetSelection {
            anchor: snap(anchor),
            caret: snap(caret),
        }));
        Some(out)
    }

    /// Replace a field's text: focus it unless it has the focus, then delete
    /// all of its text and type the new one.
    pub(crate) fn set_text_events(&self, s: &str) -> Vec<Event> {
        let mut out = self.focus_events();
        out.push(Event::Ime(Ime::DeleteSurrounding {
            before: 1 << 30,
            after: 1 << 30,
        }));
        if !s.is_empty() {
            out.push(Event::Ime(Ime::Commit(s.to_string())));
        }
        out
    }

    /// Scroll this scroll container (or the container of this scroll bar)
    /// by `by` logical pixels along its axis (positive: towards the end): a
    /// wheel at its middle. `None` when it isn't a scroll container or bar,
    /// or `by` goes nowhere.
    pub(crate) fn scroll_by_events(&self, by: f64) -> Option<Vec<Event>> {
        let s = self.scroll.or(self.scroll_bar)?;
        // Clamp to the range, so a request past an end does what it can.
        let by = s.to(s.offset + by);
        if by.abs() < 0.5 {
            return None;
        }
        let (dx, dy) = s.wheel(by);
        Some(vec![
            Event::PointerMoved {
                position: self.center(),
            },
            Event::Scroll {
                delta: ScrollDelta::pixels(dx, dy),
            },
        ])
    }

    /// Scroll this container a page forward (towards its end) or back.
    pub(crate) fn scroll_page_events(&self, forward: bool) -> Option<Vec<Event>> {
        let page = self.scroll.or(self.scroll_bar)?.page();
        self.scroll_by_events(if forward { page } else { -page })
    }

    /// Scroll this container to `fraction` (`0..=1`) of its range.
    pub(crate) fn scroll_to_fraction_events(&self, fraction: f64) -> Option<Vec<Event>> {
        let s = self.scroll.or(self.scroll_bar)?;
        self.scroll_by_events(s.to(fraction.clamp(0.0, 1.0) * s.max))
    }

    /// The scroll that brings the node at `path` (under this one) into view
    /// in the nearest scroll container around it: the least movement that
    /// shows it whole (its start, when it's longer than the viewport).
    /// `None` when it is in view already or no container holds it.
    pub(crate) fn scroll_into_view_events(&self, path: &[usize]) -> Option<Vec<Event>> {
        let target = self.at_path(path)?;
        // The innermost container on the way down (not the node itself).
        let mut container = None;
        let mut at = self;
        for &i in path {
            if at.scroll.is_some() {
                container = Some(at);
            }
            at = at.children.get(i)?;
        }
        let c = container?;
        let s = c.scroll?;
        let (cx, cy, cw, ch) = c.bounds;
        let (tx, ty, tw, th) = target.bounds;
        // Along the axis: the viewport's span and the target's.
        let (v0, v1, t0, t1) = if s.horizontal {
            (cx, cx + cw, tx, tx + tw)
        } else {
            (cy, cy + ch, ty, ty + th)
        };
        // How far the content must move towards the start (positive scrolls
        // forward, which moves content up / left).
        let mut by = if t0 < v0 {
            t0 - v0
        } else if t1 > v1 {
            (t1 - v1).min(t0 - v0)
        } else {
            0.0
        };
        // Right to left, the offset grows to the left.
        if s.horizontal && s.rtl {
            by = -by;
        }
        c.scroll_by_events(by)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scroller(offset: f64, max: f64, horizontal: bool, rtl: bool) -> A11yNode {
        A11yNode {
            scroll: Some(A11yScroll {
                offset,
                max,
                viewport: 100.0,
                horizontal,
                rtl,
            }),
            ..A11yNode::new(A11yRole::Group, "", (0.0, 0.0, 100.0, 100.0))
        }
    }

    fn wheel(events: &[Event]) -> (f64, f64) {
        match events.last() {
            Some(Event::Scroll { delta }) => (delta.dx, delta.dy),
            other => panic!("not a wheel: {other:?}"),
        }
    }

    #[test]
    fn scrolls_by_pages_within_the_range() {
        let s = scroller(0.0, 300.0, false, false);
        let ev = s.scroll_page_events(true).unwrap();
        assert_eq!(
            ev[0],
            Event::PointerMoved {
                position: Point::new(50.0, 50.0)
            }
        );
        assert_eq!(wheel(&ev), (0.0, 80.0));
        // At the start, back goes nowhere; near the end, forward stops there.
        assert!(s.scroll_page_events(false).is_none());
        let near = scroller(290.0, 300.0, false, false);
        assert_eq!(wheel(&near.scroll_page_events(true).unwrap()), (0.0, 10.0));
        // A container that doesn't scroll, or isn't one.
        assert!(
            scroller(0.0, 0.0, false, false)
                .scroll_page_events(true)
                .is_none()
        );
        let plain = A11yNode::new(A11yRole::Group, "", (0.0, 0.0, 1.0, 1.0));
        assert!(plain.scroll_page_events(true).is_none());
    }

    #[test]
    fn sideways_and_right_to_left_wheels() {
        let s = scroller(50.0, 200.0, true, false);
        assert_eq!(
            wheel(&s.scroll_to_fraction_events(1.0).unwrap()),
            (150.0, 0.0)
        );
        let rtl = scroller(50.0, 200.0, true, true);
        assert_eq!(
            wheel(&rtl.scroll_to_fraction_events(0.0).unwrap()),
            (50.0, 0.0)
        );
        assert_eq!(rtl.scroll.unwrap().fraction(), 0.25);
        assert!((rtl.scroll.unwrap().view_fraction() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn brings_a_child_into_view() {
        let row = |y: f64| A11yNode::new(A11yRole::ListItem, "row", (0.0, y, 100.0, 20.0));
        let list = A11yNode {
            children: vec![row(-30.0), row(10.0), row(150.0)],
            ..scroller(40.0, 300.0, false, false)
        };
        let root = A11yNode {
            children: vec![list],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 100.0, 200.0))
        };
        // Above the viewport: scroll back to its top; below: to its bottom.
        assert_eq!(
            wheel(&root.scroll_into_view_events(&[0, 0]).unwrap()),
            (0.0, -30.0)
        );
        assert!(root.scroll_into_view_events(&[0, 1]).is_none(), "in view");
        assert_eq!(
            wheel(&root.scroll_into_view_events(&[0, 2]).unwrap()),
            (0.0, 70.0)
        );
        // Nothing scrolls around the list itself.
        assert!(root.scroll_into_view_events(&[0]).is_none());
    }

    #[test]
    fn text_goes_through_the_input_method_path() {
        let field = A11yNode {
            caret: Some(0),
            ..A11yNode::new(A11yRole::TextField, "héllo", (0.0, 0.0, 10.0, 10.0))
        };
        let ev = field.select_events(2, 9).unwrap();
        assert_eq!(ev.len(), 4, "a click focuses it first");
        assert_eq!(
            ev[3],
            Event::Ime(Ime::SetSelection {
                anchor: 1,
                caret: 6
            }),
            "snapped to characters and the text"
        );
        let focused = A11yNode {
            focused: true,
            ..field.clone()
        };
        assert_eq!(
            focused.set_text_events("x"),
            vec![
                Event::Ime(Ime::DeleteSurrounding {
                    before: 1 << 30,
                    after: 1 << 30
                }),
                Event::Ime(Ime::Commit("x".into()))
            ]
        );
        let label = A11yNode::new(A11yRole::Text, "x", (0.0, 0.0, 1.0, 1.0));
        assert!(label.select_events(0, 0).is_none());
    }
}
