//! Bringing a node into view before acting on it: what an assistive
//! technology's "scroll to" asks (AT-SPI's `ScrollTo`, UIA's
//! `ScrollIntoView`, Android's `ACTION_SHOW_ON_SCREEN`), and what every
//! activation or focus of a node does first, so a click on an item
//! scrolled out of its list lands on the item rather than on whatever is
//! drawn where it would be (or outside the window).
//!
//! Every scroll container on the way down to the node scrolls, outermost
//! first, by the least that shows the node whole (its start when it is
//! longer than the viewport), with a wheel over a part of the container
//! that shows and isn't another container scrolling the same way. The
//! node's place moves with each scroll, so the click that follows lands at
//! its middle where it will be, within what all the containers show.

// Not every bridge (nor every target) uses every helper; the tests do.
#![allow(dead_code)]

use super::{A11yNode, A11yRole, A11yScroll};
use crate::geometry::Point;
use crate::platform::event::{Event, ScrollDelta};

/// `(x, y, width, height)`.
type Bounds = (f64, f64, f64, f64);

fn intersect(a: Bounds, b: Bounds) -> Option<Bounds> {
    let (x0, y0) = (a.0.max(b.0), a.1.max(b.1));
    let (x1, y1) = ((a.0 + a.2).min(b.0 + b.2), (a.1 + a.3).min(b.1 + b.3));
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
}

fn shifted(b: Bounds, (dx, dy): (f64, f64)) -> Bounds {
    (b.0 + dx, b.1 + dy, b.2, b.3)
}

fn middle(b: Bounds) -> Point {
    Point::new(b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
}

fn inside(p: Point, b: Bounds) -> bool {
    p.x >= b.0 && p.x < b.0 + b.2 && p.y >= b.1 && p.y < b.1 + b.3
}

/// How far the content must move towards its start (positive moves it up
/// or left) to show `target` in `view` along the axis.
fn needed(s: &A11yScroll, view: Bounds, target: Bounds) -> f64 {
    let (v0, v1, t0, t1) = if s.horizontal {
        (view.0, view.0 + view.2, target.0, target.0 + target.2)
    } else {
        (view.1, view.1 + view.3, target.1, target.1 + target.3)
    };
    if t0 < v0 {
        t0 - v0
    } else if t1 > v1 {
        (t1 - v1).min(t0 - v0)
    } else {
        0.0
    }
}

/// The scroll containers under `n` (not `n`) that scroll the way `s` does,
/// their bounds moved by `shift`: where a wheel meant for `n` mustn't go.
fn same_way(n: &A11yNode, s: &A11yScroll, shift: (f64, f64), out: &mut Vec<Bounds>) {
    for c in &n.children {
        match c.scroll {
            Some(cs) if cs.horizontal == s.horizontal && cs.scrolls() => {
                out.push(shifted(c.bounds, shift));
            }
            _ => same_way(c, s, shift, out),
        }
    }
}

/// Where to put the wheel for the container showing `shown`: its middle,
/// or else the first of a grid of points over it that no inner container
/// scrolling the same way covers.
fn wheel_point(shown: Bounds, avoid: &[Bounds]) -> Point {
    let (x, y, w, h) = shown;
    let free = |p: Point| !avoid.iter().any(|&b| inside(p, b));
    let mut candidates = vec![middle(shown)];
    for fy in [0.1, 0.5, 0.9] {
        for fx in [0.1, 0.5, 0.9] {
            candidates.push(Point::new(x + w * fx, y + h * fy));
        }
    }
    candidates
        .iter()
        .copied()
        .find(|&p| free(p))
        .unwrap_or(candidates[0])
}

/// What bringing a node into view does: the events, and where the node's
/// middle (within what shows of it) is afterwards.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reveal {
    pub(crate) events: Vec<Event>,
    pub(crate) at: Point,
}

/// The ways container `n` scrolls: its own scroll, and sideways too when
/// it scrolls both ways (read from its horizontal bar, as a table's body,
/// whose vertical scroll its table took, still has it).
fn ways(n: &A11yNode) -> Vec<A11yScroll> {
    let mut out: Vec<A11yScroll> = n.scroll.filter(A11yScroll::scrolls).into_iter().collect();
    if !n.scroll.is_some_and(|s| s.horizontal) {
        out.extend(
            n.children
                .iter()
                .filter(|c| c.role == A11yRole::ScrollBar)
                .filter_map(|c| c.scroll_bar)
                .filter(|b| b.horizontal && b.scrolls())
                .take(1),
        );
    }
    out
}

/// Where the frozen columns of the table on the way to the cell at `path`
/// are, sideways (`(left, right)`), when the cell is in another column: a
/// sideways scroll brings it clear of them, not under them.
fn frozen_span(root: &A11yNode, path: &[usize]) -> Option<(f64, f64)> {
    fn spans(n: &A11yNode, frozen: usize, out: &mut Option<(f64, f64)>) {
        for c in &n.children {
            if matches!(c.role, A11yRole::Cell | A11yRole::ColumnHeader)
                && c.cell.is_some_and(|at| at.column < frozen)
            {
                let (x0, x1) = (c.bounds.0, c.bounds.0 + c.bounds.2);
                *out = Some(out.map_or((x0, x1), |(a, b)| (a.min(x0), b.max(x1))));
            } else if c.role != A11yRole::Table {
                spans(c, frozen, out);
            }
        }
    }
    let mut at = root;
    let (mut frozen, mut row) = (0, None);
    for &i in path {
        at = at.children.get(i)?;
        match at.role {
            A11yRole::Table => (frozen, row) = (at.table.map_or(0, |t| t.frozen), None),
            A11yRole::Row => row = Some(at),
            _ => {}
        }
    }
    let column = at.cell.filter(|_| at.role != A11yRole::Row)?.column;
    if frozen == 0 || column < frozen {
        return None;
    }
    let mut out = None;
    spans(row?, frozen, &mut out);
    out
}

impl A11yNode {
    /// Bring the node at `path` (under this root) into view in every scroll
    /// container around it (see the module docs). `None` when there is no
    /// node there.
    pub(crate) fn reveal(&self, path: &[usize]) -> Option<Reveal> {
        let target = self.at_path(path)?;
        let frozen = frozen_span(self, path);
        let mut events = Vec::new();
        // How far everything inside the containers scrolled so far moved.
        let mut shift = (0.0, 0.0);
        let mut clip = self.bounds;
        let mut at = self;
        for &i in path {
            let ways = ways(at);
            if !ways.is_empty() {
                let cb = shifted(at.bounds, shift);
                let view = intersect(cb, clip).unwrap_or(cb);
                let mut shows = cb;
                for s in ways {
                    // Sideways, what the frozen columns cover doesn't show
                    // the cell (they stay put: the shift doesn't move them).
                    let mut cb = cb;
                    if s.horizontal
                        && let Some((x0, x1)) = frozen
                    {
                        let (x0, x1) = (x0 + shift.0, x1 + shift.0);
                        let (l, r) = (cb.0, cb.0 + cb.2);
                        let (l, r) = if s.rtl {
                            (l, r.min(x0))
                        } else {
                            (l.max(x1), r)
                        };
                        if r > l {
                            cb = (l, cb.1, r - l, cb.3);
                            shows = intersect(shows, cb).unwrap_or(shows);
                        }
                    }
                    let raw = needed(&s, cb, shifted(target.bounds, shift));
                    // In offset terms (a right-to-left strip's grows
                    // leftwards), clamped to the range.
                    let ask = if s.horizontal && s.rtl { -raw } else { raw };
                    let by = s.to(s.offset + ask);
                    if by.abs() >= 0.5 {
                        let moved = if s.horizontal && s.rtl { -by } else { by };
                        let mut avoid = Vec::new();
                        same_way(at, &s, shift, &mut avoid);
                        let (dx, dy) = s.wheel(by);
                        events.push(Event::PointerMoved {
                            position: wheel_point(view, &avoid),
                        });
                        events.push(Event::Scroll {
                            delta: ScrollDelta::pixels(dx, dy),
                        });
                        if s.horizontal {
                            shift.0 -= moved;
                        } else {
                            shift.1 -= moved;
                        }
                    }
                }
                clip = intersect(clip, shows).unwrap_or(clip);
            }
            at = at.children.get(i)?;
        }
        let tb = shifted(target.bounds, shift);
        let at = intersect(tb, clip).map_or_else(|| middle(tb), middle);
        Some(Reveal { events, at })
    }

    /// The events of a click on the node at `path` (under this root), after
    /// it is brought into view: a screen reader's activation of it.
    pub(crate) fn activate_events(&self, path: &[usize]) -> Option<Vec<Event>> {
        let r = self.reveal(path)?;
        let mut events = r.events;
        events.extend(click_at(r.at));
        Some(events)
    }

    /// Focus the node at `path` (under this root): bring it into view, and
    /// click it unless it has the focus.
    pub(crate) fn focus_events_at(&self, path: &[usize]) -> Option<Vec<Event>> {
        let focused = self.at_path(path)?.focused;
        let r = self.reveal(path)?;
        let mut events = r.events;
        if !focused {
            events.extend(click_at(r.at));
        }
        Some(events)
    }
}

/// A click at `position`.
fn click_at(position: Point) -> Vec<Event> {
    use crate::platform::event::{ButtonState, PointerButton};
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scroller(offset: f64, max: f64, bounds: Bounds, horizontal: bool) -> A11yNode {
        A11yNode {
            scroll: Some(A11yScroll {
                offset,
                max,
                viewport: if horizontal { bounds.2 } else { bounds.3 },
                horizontal,
                rtl: false,
            }),
            ..A11yNode::new(A11yRole::Group, "", bounds)
        }
    }

    fn wheels(events: &[Event]) -> Vec<(f64, f64)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Scroll { delta } => Some((delta.dx, delta.dy)),
                _ => None,
            })
            .collect()
    }

    fn click(events: &[Event]) -> Option<Point> {
        match events.last() {
            Some(Event::PointerButton { position, .. }) => Some(*position),
            _ => None,
        }
    }

    #[test]
    fn an_item_below_the_viewport_is_scrolled_to_then_clicked() {
        let row = |y: f64| A11yNode::new(A11yRole::Button, "row", (0.0, y, 100.0, 20.0));
        let list = A11yNode {
            children: vec![row(10.0), row(250.0)],
            ..scroller(0.0, 500.0, (0.0, 0.0, 100.0, 100.0), false)
        };
        let root = A11yNode {
            children: vec![list],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 100.0, 300.0))
        };
        let ev = root.activate_events(&[0, 1]).unwrap();
        // 250..270 shows at the bottom of 0..100: 170 up.
        assert_eq!(wheels(&ev), vec![(0.0, 170.0)]);
        assert_eq!(click(&ev), Some(Point::new(50.0, 90.0)));
        // An item in view is clicked where it is, with no scroll.
        let ev = root.activate_events(&[0, 0]).unwrap();
        assert!(wheels(&ev).is_empty());
        assert_eq!(click(&ev), Some(Point::new(50.0, 20.0)));
        // Focusing a focused item only brings it into view.
        let mut focused = root.clone();
        focused.children[0].children[1].focused = true;
        let ev = focused.focus_events_at(&[0, 1]).unwrap();
        assert_eq!(ev.len(), 2, "a wheel, no click: {ev:?}");
        assert!(root.activate_events(&[0, 5]).is_none());
    }

    #[test]
    fn nested_containers_scroll_outermost_first() {
        // A page scrolling down, holding a strip scrolling sideways whose
        // last chip is out of view both ways.
        let chip = A11yNode::new(A11yRole::Button, "chip", (300.0, 400.0, 50.0, 20.0));
        let strip = A11yNode {
            children: vec![chip],
            ..scroller(0.0, 400.0, (0.0, 400.0, 200.0, 20.0), true)
        };
        let page = A11yNode {
            children: vec![strip],
            ..scroller(0.0, 1000.0, (0.0, 0.0, 200.0, 200.0), false)
        };
        let root = A11yNode {
            children: vec![page],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 200.0, 200.0))
        };
        let ev = root.activate_events(&[0, 0, 0]).unwrap();
        assert_eq!(wheels(&ev), vec![(0.0, 220.0), (150.0, 0.0)]);
        // The strip's wheel goes where it shows after the page scrolled
        // (180..200), and the click on the chip there too.
        let strip_wheel = ev
            .iter()
            .filter_map(|e| match e {
                Event::PointerMoved { position } => Some(*position),
                _ => None,
            })
            .nth(1)
            .unwrap();
        assert_eq!(strip_wheel, Point::new(100.0, 190.0));
        assert_eq!(click(&ev), Some(Point::new(175.0, 190.0)));
    }

    #[test]
    fn the_wheel_avoids_an_inner_container_scrolling_the_same_way() {
        let inner = A11yNode {
            children: vec![],
            ..scroller(0.0, 100.0, (0.0, 25.0, 100.0, 50.0), false)
        };
        let row = A11yNode::new(A11yRole::Button, "far", (0.0, 300.0, 100.0, 20.0));
        let outer = A11yNode {
            children: vec![inner, row],
            ..scroller(0.0, 500.0, (0.0, 0.0, 100.0, 100.0), false)
        };
        let root = A11yNode {
            children: vec![outer],
            ..A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 100.0, 100.0))
        };
        let ev = root.activate_events(&[0, 1]).unwrap();
        let Event::PointerMoved { position } = ev[0] else {
            panic!("{ev:?}");
        };
        assert!(position.y < 25.0 || position.y >= 75.0, "{position:?}");
    }
}
