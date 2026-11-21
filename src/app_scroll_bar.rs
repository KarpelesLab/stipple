//! Scroll bars in the [`App`](crate::App): which bar the pointer is over, a
//! thumb being dragged, overlay bars showing while their container scrolls
//! and fading after, and writing each frame's bars on the laid-out tree. The
//! geometry is [`core::scroll_bar`](crate::core::scroll_bar)'s.
//!
//! A left press on a bar (on the topmost layer under the pointer) is the
//! bar's, ahead of the element handlers: on the thumb it starts a drag that
//! scrolls the container proportionally until the release; on the track it
//! pages a viewport towards the press. Its moves and release are the bar's
//! too. Bars that aren't [interactive](crate::style::ScrollBarLook::interactive)
//! (touch platforms') take nothing, and neither does a finger anywhere: it
//! scrolls by dragging the content.

use std::collections::HashMap;
use std::time::Duration;

use crate::Pane;
use crate::core::ScrollId;
use crate::core::scroll_bar::{self, Part};
use crate::geometry::{Affine, Point, Rect};
use crate::style::Theme;

/// A press held on a scroll bar.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    id: ScrollId,
    /// For a drag of the thumb: where along the thumb it was grabbed.
    grab: Option<f64>,
}

/// One window's scroll bars between frames.
#[derive(Debug, Default)]
pub(crate) struct ScrollBars {
    /// The bar under the pointer, and the part of it.
    hover: Option<(ScrollId, Part)>,
    /// The press held on a bar.
    held: Option<Held>,
    /// When each overlay bar was last active (its container scrolled, or
    /// the pointer left it), on the frame clock.
    active: HashMap<ScrollId, Duration>,
    /// Each container's offset as last drawn, to see it scroll.
    seen: HashMap<ScrollId, f64>,
}

impl ScrollBars {
    /// Whether a press on a bar is held (its moves and release are the
    /// bar's).
    pub(crate) fn holding(&self) -> bool {
        self.held.is_some()
    }

    /// How visible container `id`'s bar is at `now`: fully for a classic
    /// bar; an overlay one while under the pointer or held, and for a moment
    /// after its container scrolled, fading out along the theme's motion.
    fn level(&self, id: ScrollId, theme: &Theme, now: Option<Duration>) -> f64 {
        if !theme.scroll_bar.overlay
            || self.hover.is_some_and(|(h, _)| h == id)
            || self.held.is_some_and(|h| h.id == id)
        {
            return 1.0;
        }
        match (self.active.get(&id), now) {
            (Some(&at), Some(now)) => scroll_bar::overlay_level(
                now.saturating_sub(at).as_secs_f64(),
                theme.motion.scroll_bar,
            ),
            _ => 0.0,
        }
    }

    /// Whether an overlay bar is still showing or fading at `now`, so frames
    /// must go on. Forgets the bars that have faded.
    pub(crate) fn animating(&mut self, theme: &Theme, now: Option<Duration>) -> bool {
        if !theme.scroll_bar.overlay {
            self.active.clear();
            return false;
        }
        let Some(now) = now else {
            return false;
        };
        let motion = theme.motion.scroll_bar;
        self.active.retain(|_, at| {
            scroll_bar::overlay_level(now.saturating_sub(*at).as_secs_f64(), motion) > 0.0
        });
        !self.active.is_empty()
    }

    /// Note the containers that scrolled since the last frame: their overlay
    /// bars show (from `now`). A container seen for the first time doesn't.
    fn note_offsets(&mut self, offsets: &HashMap<ScrollId, f64>, now: Option<Duration>) {
        for (&id, &off) in offsets {
            if let Some(old) = self.seen.insert(id, off)
                && (old - off).abs() > f64::EPSILON
                && let Some(now) = now
            {
                self.active.insert(id, now);
            }
        }
    }
}

impl<S: 'static> Pane<S> {
    /// Write this frame's scroll bars on `tree` (laid out and scrolled) as of
    /// `now`, returning where they changed (window coordinates).
    pub(crate) fn stamp_scroll_bars(
        &mut self,
        tree: &mut crate::core::LayoutNode,
        theme: &Theme,
        now: Option<Duration>,
    ) -> Vec<Rect> {
        self.scroll_bars.note_offsets(&self.scroll_offsets, now);
        let bars = &self.scroll_bars;
        let level = |id| bars.level(id, theme, now);
        let stamp = scroll_bar::Stamp {
            look: theme.scroll_bar,
            palette: theme.palette,
            offsets: &self.scroll_offsets,
            hover: bars.hover,
            pressed: bars.held.filter(|h| h.grab.is_some()).map(|h| h.id),
            level: &level,
        };
        let mut changed = Vec::new();
        scroll_bar::stamp(tree, &stamp, Affine::IDENTITY, &mut changed);
        changed
    }

    /// An animation frame (no build): the overlay bars as of `now`, on the
    /// tree and on the frame on screen, and where they changed.
    pub(crate) fn restamp_scroll_bars(&mut self, theme: &Theme, now: Duration) -> Vec<Rect> {
        let mut changed = Vec::new();
        // Classic bars don't change with time.
        if !theme.scroll_bar.overlay {
            return changed;
        }
        if let Some(mut tree) = self.tree.take() {
            changed = self.stamp_scroll_bars(&mut tree, theme, Some(now));
            self.tree = Some(tree);
        }
        if let Some(mut shown) = self.presented.take() {
            self.stamp_scroll_bars(&mut shown, theme, Some(now));
            self.presented = Some(shown);
        }
        changed
    }

    /// The bar under `pos` (window coordinates) that the pointer can use,
    /// and the part of it, in the layer a press there reaches.
    fn bar_under(&self, theme: &Theme, pos: Point) -> Option<(ScrollId, Part)> {
        let look = theme.scroll_bar;
        // A finger scrolls the content itself, and doesn't hover.
        if !look.interactive || self.touch.primary().is_some() {
            return None;
        }
        let tree = self.tree.as_ref()?;
        let layer = crate::press_layer(tree, self.overlay_active, pos).0;
        scroll_bar::bar_at(layer, pos, &look, &self.scroll_offsets)
    }

    /// Container `id`'s bar and `pos` in its layout coordinates.
    fn bar_geometry(
        &self,
        theme: &Theme,
        id: ScrollId,
        pos: Point,
    ) -> Option<(scroll_bar::Geometry, Point)> {
        let tree = self.tree.as_ref()?;
        let (node, m, _) = crate::core::node_to_window(tree, &|n| {
            n.scroll == Some(id) || (n.scroll.is_some() && n.scroll_x == Some(id))
        })?;
        let local = m.inverse()?.apply(pos);
        let g = scroll_bar::geometry_for(node, id, &self.scroll_offsets, &theme.scroll_bar)?;
        Some((g, local))
    }

    /// Scroll container `id` to `offset`. Returns whether it moved.
    fn scroll_bar_to(&mut self, id: ScrollId, offset: f64) -> bool {
        let old = self.scroll_offsets.insert(id, offset);
        let moved = old.is_none_or(|o| (o - offset).abs() > f64::EPSILON);
        self.dirty |= moved;
        moved
    }

    /// A left press at `pos`: if it is on a scroll bar, it's the bar's — a
    /// press on the thumb starts dragging it, one on the track pages towards
    /// the press. Returns whether the bar took it.
    pub(crate) fn scroll_bar_press(&mut self, theme: &Theme, pos: Point) -> bool {
        let Some((id, part)) = self.bar_under(theme, pos) else {
            return false;
        };
        // An overlay bar that has faded out isn't there to press, unless
        // the pointer brought it back.
        if self
            .scroll_bars
            .level(id, theme, crate::platform::frame::elapsed())
            <= 0.0
        {
            return false;
        }
        let Some((g, local)) = self.bar_geometry(theme, id, pos) else {
            return false;
        };
        let grab = match part {
            Part::Thumb => Some(g.along(local) - g.thumb_start()),
            Part::Before | Part::After => {
                self.scroll_bar_to(id, g.paged(part));
                None
            }
        };
        self.scroll_bars.held = Some(Held { id, grab });
        self.scroll_bars.hover = Some((id, part));
        self.dirty = true;
        true
    }

    /// The pointer moved to `pos` with a press held on a bar: a dragged
    /// thumb follows it. Returns whether to repaint.
    pub(crate) fn scroll_bar_drag(&mut self, theme: &Theme, pos: Point) -> bool {
        let Some(Held {
            id,
            grab: Some(grab),
        }) = self.scroll_bars.held
        else {
            return false;
        };
        let Some((g, local)) = self.bar_geometry(theme, id, pos) else {
            return false;
        };
        self.scroll_bar_to(id, g.offset_for(g.along(local) - grab))
    }

    /// The press held on a bar lifted at `pos`. Returns whether to repaint.
    pub(crate) fn scroll_bar_release(&mut self, theme: &Theme, pos: Point) -> bool {
        let Some(held) = self.scroll_bars.held.take() else {
            return false;
        };
        if let Some(now) = crate::platform::frame::elapsed() {
            self.scroll_bars.active.insert(held.id, now);
        }
        self.scroll_bars.hover = self.bar_under(theme, pos);
        self.dirty = true;
        true
    }

    /// The pointer moved to `pos` (or left the window, `None`) with no press
    /// held on a bar: the bar it is over shows its hover look (and an
    /// overlay bar shows). Returns whether to repaint.
    pub(crate) fn scroll_bar_hover(&mut self, theme: &Theme, pos: Option<Point>) -> bool {
        let over = pos.and_then(|p| self.bar_under(theme, p));
        let old = self.scroll_bars.hover;
        if over == old {
            return false;
        }
        // An overlay bar the pointer leaves lingers, then fades.
        if let Some((id, _)) = old
            && over.is_none_or(|(o, _)| o != id)
            && let Some(now) = crate::platform::frame::elapsed()
        {
            self.scroll_bars.active.insert(id, now);
        }
        self.scroll_bars.hover = over;
        self.dirty = true;
        true
    }
}
