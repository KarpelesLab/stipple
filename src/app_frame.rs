//! Animation frames that skip the view: when the only thing that changed
//! since the frame on screen is the passing of time, and what moves are
//! implicit [transitions](crate::core::Transition) of looks that don't
//! affect layout (colors, borders, corner radius, opacity, transforms) and
//! hover or press highlights fading, the next frame is the laid-out tree on
//! screen with those looks written on it. The view isn't built, nothing is
//! laid out, and the damage is the moved elements' old and new extents.
//!
//! Anything else falls back to a full build: a view that asked for the frame
//! itself ([`Cx::request_frame`](crate::core::Cx::request_frame)), padding or
//! sizes in motion, enter/exit/layout animations, OS-drawn controls
//! cross-fading, a fling, a drag's drop-target fade, or an element moving into or out of the pointer's way
//! when its look or the view depends on that.

use crate::Pane;
use crate::core::ActionId;
use crate::core::{Damage, ElementKey, LayoutNode, transition::Painted};
use crate::geometry::{Affine, Rect};
use crate::render::{Font, Scene};
use crate::style::Theme;
use std::collections::HashMap;

impl<S: 'static> Pane<S> {
    /// The next animation frame drawn from the tree on screen, and its damage,
    /// or `None` when it has to be built.
    pub(crate) fn animation_frame(
        &mut self,
        theme: &Theme,
        font: Option<&Font>,
    ) -> Option<(Scene, Damage)> {
        crate::platform::frame::now()?;
        let now = crate::platform::frame::elapsed().unwrap_or_default();
        if self.dirty
            || self.view_animating
            || self.tree.is_none()
            || self.presented.is_none()
            || self.built_theme != Some(*theme)
            || self.presence.animating()
            || self.native_fades.animating()
            || self.touch.flinging()
            || self.dnd.fades.animating(Some(now))
            || self.transitions.layout_moving()
        {
            return None;
        }
        let painted = self.transitions.advance(now);
        let mut rects = Vec::new();
        if !painted.is_empty() {
            let tree = self.tree.as_mut()?;
            patch(tree, &painted, Affine::IDENTITY, &mut Some(&mut rects));
            // Something moving under the pointer may change what is hovered:
            // rebuild if a look or the view depends on that. (The frame on
            // screen is still the baseline to diff against.)
            if !self.watch.is_empty() && self.watch.changed(&self.interactions()) {
                return None;
            }
            let presented = self.presented.as_mut()?;
            patch(presented, &painted, Affine::IDENTITY, &mut None);
        }
        self.update_highlights(theme, Some(now));
        self.crossfade.advance(Some(now));
        self.animating = self.transitions.animating()
            || self.highlights.animating(Some(now))
            || self.crossfade.active();
        if self.animating {
            crate::platform::frame::request();
        }
        // What is drawn over the tree, as `take_damage` would.
        let overlays = self.overlay_active || self.painted_overlay_active;
        let hover_changed = self.highlight_alphas != self.painted_highlights;
        let damage = if overlays {
            Damage::Full
        } else {
            let (old, new) = (self.presented.as_ref()?, self.tree.as_ref()?);
            match self.overlay_damage_rects(old, new, hover_changed, false) {
                Some(more) => {
                    rects.extend(more);
                    if rects.is_empty() {
                        Damage::None
                    } else {
                        Damage::Regions(crate::core::coalesce(rects))
                    }
                }
                None => Damage::Full,
            }
        };
        let damage = crate::merge_regions(damage, self.take_content_damage());
        self.painted_highlights = self.highlight_alphas.clone();
        self.painted_overlay_active = self.overlay_active;
        crate::app_dbg(format_args!("present: animated without a build"));
        Some((self.paint_scene(theme, font), damage))
    }

    /// Point the hover and press highlights at the elements hovered and
    /// pressed, as of `now`, and work out how strongly each is drawn: the
    /// theme's overlay lightens the hovered tappable element, twice over while
    /// pressed, fading between states. (Not on the press-outside catchers of
    /// overlays, nor on elements that style their own hover and press looks.)
    pub(crate) fn update_highlights(&mut self, theme: &Theme, now: Option<std::time::Duration>) {
        let lit = |id: Option<ActionId>| {
            id.filter(|id| !self.catchers.contains(id) && !self.styled_actions.contains(id))
        };
        let (hovered, pressed) = (lit(self.hovered), lit(self.pressed));
        self.highlights.update(hovered, pressed, now);
        let overlay = theme.palette.hover_overlay;
        self.highlight_alphas = self
            .highlights
            .levels(now)
            .into_iter()
            .map(|(id, level)| (id, (overlay.a as f64 * level).round().min(255.0) as u8))
            .filter(|(_, a)| *a > 0)
            .collect();
    }
}

/// Write the `painted` looks on the nodes of `node`'s subtree with their
/// identities (`outer` maps its parent's coordinates to the window), adding
/// where each change shows, before and after, to `rects` if given.
fn patch(
    node: &mut LayoutNode,
    painted: &HashMap<ElementKey, Painted>,
    outer: Affine,
    rects: &mut Option<&mut Vec<Rect>>,
) {
    if let Some(p) = node.key.and_then(|k| painted.get(&k)) {
        let whole = p.moves(node);
        if let Some(rects) = rects {
            rects.push(crate::core::diff::changed_extent(node, outer, whole));
        }
        p.apply(node);
        if let Some(rects) = rects {
            rects.push(crate::core::diff::changed_extent(node, outer, whole));
        }
    }
    let inner = crate::core::diff::to_window(node, outer);
    for child in &mut node.children {
        patch(child, painted, inner, rects);
    }
}
