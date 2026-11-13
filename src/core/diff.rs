//! Frame reconciliation: diff two retained [`LayoutNode`] trees to find what
//! visually changed, so the platform can re-present only the changed region
//! instead of the whole window.
//!
//! The diff is **paint-oriented**: it compares the things that produce pixels —
//! each node's `bounds`, `decoration`, and text `content` — and ignores the
//! routing handles (`action`/`focus`/`drag`), which never draw. A node whose
//! visuals differ contributes the union of its old and new bounds; a node whose
//! child *count* differs contributes its whole subtree (covering inserts and
//! removals). Matching children are compared pairwise.
//!
//! Regions are where things are *drawn*: a node under a
//! [transform](LayoutNode::transform) contributes the box around its
//! transformed bounds, and a node whose transform (as resolved against its
//! box, so including a box change that moves its origin) or group opacity
//! changed contributes the drawn extent of its whole subtree, old and new.
//!
//! Damage is always **conservative**: over-reporting a region is merely a
//! larger repaint, while under-reporting would leave stale pixels on screen, so
//! every ambiguous case widens the region rather than narrowing it.

use crate::core::runtime::LayoutNode;
use crate::geometry::{Affine, Rect, ScaleFactor};

/// What changed between two frames, in **logical** pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Damage {
    /// Nothing changed; the previous frame is still valid (skip present).
    None,
    /// Only these rectangles changed.
    Regions(Vec<Rect>),
    /// Everything must be repainted (first frame, resize, or an overlay change
    /// the tree diff can't localize).
    Full,
}

impl Damage {
    /// `true` if nothing changed.
    pub fn is_empty(&self) -> bool {
        matches!(self, Damage::None)
    }

    /// The single rectangle covering all damage, or `None` for [`Damage::None`].
    /// [`Damage::Full`] has no finite bound, so it also returns `None`.
    pub fn bounding(&self) -> Option<Rect> {
        match self {
            Damage::Regions(rs) => rs.iter().copied().reduce(|a, b| a.union(b)),
            _ => None,
        }
    }

    /// Convert to physical-pixel rectangles for [`Surface::present`], clamped to
    /// a `bounds` (the surface size) and rounded outward to whole pixels.
    ///
    /// [`Damage::None`] and [`Damage::Full`] both yield an empty list — which the
    /// [`Surface`] contract reads as "assume everything changed". Callers that
    /// want to skip a no-op present should check [`Damage::is_empty`] first.
    ///
    /// [`Surface`]: crate::render::Surface
    /// [`Surface::present`]: crate::render::Surface::present
    pub fn to_physical(&self, scale: ScaleFactor, bounds: Rect) -> Vec<Rect> {
        let Damage::Regions(regions) = self else {
            return Vec::new();
        };
        let s = scale.get();
        regions
            .iter()
            .filter_map(|r| {
                let phys = Rect::from_xywh(
                    (r.min_x() * s).floor(),
                    (r.min_y() * s).floor(),
                    (r.width() * s).ceil(),
                    (r.height() * s).ceil(),
                );
                // Snap the far edge outward too, then clip to the surface.
                let snapped = Rect::from_points(
                    crate::geometry::Point::new(phys.min_x(), phys.min_y()),
                    crate::geometry::Point::new((r.max_x() * s).ceil(), (r.max_y() * s).ceil()),
                );
                snapped.intersection(bounds).filter(|c| !c.is_empty())
            })
            .collect()
    }
}

/// Diff a previously-presented tree against the freshly built one, returning the
/// damaged region. Pass trees laid out at the same root size; a size change
/// should be treated as [`Damage::Full`] by the caller.
pub fn diff_trees(old: &LayoutNode, new: &LayoutNode) -> Damage {
    let mut regions = Vec::new();
    diff_node(old, new, Affine::IDENTITY, Affine::IDENTITY, &mut regions);
    if regions.is_empty() {
        Damage::None
    } else {
        Damage::Regions(coalesce(regions))
    }
}

/// `true` if two nodes paint the same pixels for themselves (ignoring children).
///
/// `caret` and `selection` are included because the focus overlay paints them,
/// so moving the caret or changing the selection (text otherwise unchanged) must
/// still damage the node.
fn visuals_equal(a: &LayoutNode, b: &LayoutNode) -> bool {
    a.bounds == b.bounds
        && a.decoration == b.decoration
        && a.native == b.native
        && a.native_from == b.native_from
        && a.content == b.content
        && a.caret == b.caret
        && a.selection == b.selection
        && a.preedit == b.preedit
}

/// `outer_old` / `outer_new` map the parents' layout coordinates to the
/// window in each frame (the identity where nothing is transformed).
fn diff_node(
    old: &LayoutNode,
    new: &LayoutNode,
    outer_old: Affine,
    outer_new: Affine,
    out: &mut Vec<Rect>,
) {
    let (mo, mn) = (to_window(old, outer_old), to_window(new, outer_new));
    if mo != mn || old.opacity != new.opacity {
        // Moved, scaled, turned or faded as a whole: everything it draws,
        // where it was and where it is.
        out.push(subtree_extent(old, outer_old));
        out.push(subtree_extent(new, outer_new));
        return;
    }
    if !visuals_equal(old, new) {
        out.push(drawn(mo, bleed(old)).union(drawn(mn, bleed(new))));
    }
    if old.children.len() != new.children.len() {
        // Structural change: a child was inserted or removed. We can't align
        // the lists cheaply, so repaint the whole subtree (both extents).
        out.push(drawn(mo, old.bounds).union(drawn(mn, new.bounds)));
        return;
    }
    for (o, n) in old.children.iter().zip(&new.children) {
        diff_node(o, n, mo, mn, out);
    }
}

/// The matrix mapping `node`'s layout coordinates to the window, given the
/// one for its parent's.
pub(crate) fn to_window(node: &LayoutNode, outer: Affine) -> Affine {
    match node.affine() {
        Some(a) => a.then(outer),
        None => outer,
    }
}

/// What `node` paints itself over: its bounds, and a pixel around them when
/// it has a border, whose anti-aliased edge reaches just outside.
fn bleed(node: &LayoutNode) -> Rect {
    let b = node.bounds;
    if node.decoration.border.is_some() {
        Rect::from_xywh(
            b.min_x() - 1.0,
            b.min_y() - 1.0,
            b.width() + 2.0,
            b.height() + 2.0,
        )
    } else {
        b
    }
}

/// Where `rect` (in layout coordinates mapped by `m`) is drawn: itself when
/// untransformed, else the box around it, a pixel wider for anti-aliasing.
fn drawn(m: Affine, rect: Rect) -> Rect {
    if m.is_identity() {
        rect
    } else {
        let r = m.map_rect(rect);
        Rect::from_xywh(
            r.min_x() - 1.0,
            r.min_y() - 1.0,
            r.width() + 2.0,
            r.height() + 2.0,
        )
    }
}

/// Where a change to `node` shows, given the matrix for its parent's
/// coordinates: everything its subtree draws when it moved or faded as a
/// whole (`whole`), else what it paints itself.
pub(crate) fn changed_extent(node: &LayoutNode, outer: Affine, whole: bool) -> Rect {
    if whole {
        subtree_extent(node, outer)
    } else {
        drawn(to_window(node, outer), bleed(node))
    }
}

/// The box around everything `node`'s subtree draws (each node's drawn
/// bounds), given the matrix for its parent's coordinates.
pub(crate) fn subtree_extent(node: &LayoutNode, outer: Affine) -> Rect {
    let m = to_window(node, outer);
    node.children
        .iter()
        .map(|c| subtree_extent(c, m))
        .fold(drawn(m, node.bounds), Rect::union)
}

/// Merge rectangles that overlap or touch into fewer, larger ones. A small
/// fixed-point loop keeps the region list short without a full rectangle-set
/// algorithm — over-merging only enlarges the repaint, which is always safe.
pub(crate) fn coalesce(mut rects: Vec<Rect>) -> Vec<Rect> {
    let mut merged = true;
    while merged {
        merged = false;
        let mut i = 0;
        while i < rects.len() {
            let mut j = i + 1;
            while j < rects.len() {
                if overlaps_or_touches(rects[i], rects[j]) {
                    rects[i] = rects[i].union(rects[j]);
                    rects.swap_remove(j);
                    merged = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }
    rects
}

/// `true` if the rectangles share area or abut (so their union wastes no space
/// worth keeping them separate for).
fn overlaps_or_touches(a: Rect, b: Rect) -> bool {
    a.min_x() <= b.max_x()
        && b.min_x() <= a.max_x()
        && a.min_y() <= b.max_y()
        && b.min_y() <= a.max_y()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::element::BoxStyle;
    use crate::core::runtime::NodeContent;
    use crate::render::Color;

    fn node(bounds: Rect, fill: Option<Color>, children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode {
            bounds,
            decoration: BoxStyle {
                fill,
                radius: 0.0,
                border: None,
            },
            content: NodeContent::None,
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            native_from: None,
            scroll: None,
            scroll_horizontal: false,
            clip: false,
            cursor: None,
            semantics: None,
            rtl: false,
            transform: None,
            opacity: 1.0,
            key: None,
            ghost: false,
            children,
        }
    }

    fn text_node(bounds: Rect, text: &str) -> LayoutNode {
        LayoutNode {
            bounds,
            decoration: BoxStyle::default(),
            content: NodeContent::Text {
                text: text.into(),
                size: 14.0,
                color: Color::BLACK,
                direction: Default::default(),
                underline: None,
                style: Default::default(),
            },
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            native_from: None,
            scroll: None,
            scroll_horizontal: false,
            clip: false,
            cursor: None,
            semantics: None,
            rtl: false,
            transform: None,
            opacity: 1.0,
            key: None,
            ghost: false,
            children: Vec::new(),
        }
    }

    fn root(children: Vec<LayoutNode>) -> LayoutNode {
        node(Rect::from_xywh(0.0, 0.0, 200.0, 100.0), None, children)
    }

    #[test]
    fn identical_trees_have_no_damage() {
        let a = root(vec![text_node(
            Rect::from_xywh(10.0, 10.0, 40.0, 20.0),
            "hi",
        )]);
        let b = a.clone();
        assert_eq!(diff_trees(&a, &b), Damage::None);
        assert!(diff_trees(&a, &b).is_empty());
    }

    #[test]
    fn changed_text_damages_only_that_node() {
        let a = root(vec![
            text_node(Rect::from_xywh(10.0, 10.0, 40.0, 20.0), "0"),
            text_node(Rect::from_xywh(10.0, 40.0, 40.0, 20.0), "stable"),
        ]);
        let mut b = a.clone();
        b.children[0] = text_node(Rect::from_xywh(10.0, 10.0, 40.0, 20.0), "1");

        let dmg = diff_trees(&a, &b);
        // Exactly the first child's box, not the whole 200x100 root.
        assert_eq!(
            dmg,
            Damage::Regions(vec![Rect::from_xywh(10.0, 10.0, 40.0, 20.0)])
        );
        let bound = dmg.bounding().unwrap();
        assert!(bound.width() < 200.0 && bound.height() < 100.0);
    }

    #[test]
    fn caret_move_alone_is_damage() {
        // Text unchanged, only the caret index moves — must still repaint so the
        // focus overlay's caret bar redraws at the new position.
        let mut a = root(vec![text_node(Rect::from_xywh(0.0, 0.0, 40.0, 20.0), "ab")]);
        a.children[0].caret = Some(2);
        let mut b = a.clone();
        b.children[0].caret = Some(1);
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 40.0, 20.0)])
        );
    }

    #[test]
    fn selection_change_alone_is_damage() {
        // Text + caret unchanged, only the selection range differs — must
        // repaint so the highlight redraws.
        let mut a = root(vec![text_node(
            Rect::from_xywh(0.0, 0.0, 40.0, 20.0),
            "abc",
        )]);
        a.children[0].selection = Some((0, 1));
        let mut b = a.clone();
        b.children[0].selection = Some((0, 3));
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 40.0, 20.0)])
        );
    }

    #[test]
    fn fill_change_is_detected() {
        let a = root(vec![node(
            Rect::from_xywh(0.0, 0.0, 50.0, 50.0),
            Some(Color::rgb(10, 10, 10)),
            vec![],
        )]);
        let mut b = a.clone();
        b.children[0].decoration.fill = Some(Color::rgb(200, 0, 0));
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 50.0, 50.0)])
        );
    }

    #[test]
    fn a_border_change_damages_its_anti_aliased_edge_too() {
        let a = root(vec![node(
            Rect::from_xywh(10.0, 10.0, 50.0, 50.0),
            None,
            vec![],
        )]);
        let mut b = a.clone();
        b.children[0].decoration.border = Some((Color::BLACK, 1.0));
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(9.0, 9.0, 52.0, 52.0)])
        );
    }

    #[test]
    fn moved_node_damages_old_and_new_position() {
        let a = root(vec![node(
            Rect::from_xywh(0.0, 0.0, 20.0, 20.0),
            Some(Color::BLACK),
            vec![],
        )]);
        let mut b = a.clone();
        b.children[0].bounds = Rect::from_xywh(80.0, 0.0, 20.0, 20.0);
        // Union spans from x=0 to x=100.
        let dmg = diff_trees(&a, &b);
        assert_eq!(
            dmg,
            Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 100.0, 20.0)])
        );
    }

    #[test]
    fn added_child_repaints_subtree() {
        let a = root(vec![text_node(Rect::from_xywh(0.0, 0.0, 20.0, 20.0), "a")]);
        let b = root(vec![
            text_node(Rect::from_xywh(0.0, 0.0, 20.0, 20.0), "a"),
            text_node(Rect::from_xywh(0.0, 30.0, 20.0, 20.0), "b"),
        ]);
        // Child count differs at the root → repaint the root's extent.
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 200.0, 100.0)])
        );
    }

    #[test]
    fn coalesce_merges_touching_regions() {
        let a = root(vec![
            node(
                Rect::from_xywh(0.0, 0.0, 50.0, 20.0),
                Some(Color::BLACK),
                vec![],
            ),
            node(
                Rect::from_xywh(50.0, 0.0, 50.0, 20.0),
                Some(Color::BLACK),
                vec![],
            ),
        ]);
        let mut b = a.clone();
        b.children[0].decoration.fill = Some(Color::rgb(1, 2, 3));
        b.children[1].decoration.fill = Some(Color::rgb(1, 2, 3));
        // Two abutting damaged boxes coalesce into one.
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 100.0, 20.0)])
        );
    }

    #[test]
    fn to_physical_scales_and_clips() {
        let dmg = Damage::Regions(vec![Rect::from_xywh(10.0, 10.0, 40.0, 20.0)]);
        let phys = dmg.to_physical(
            ScaleFactor::new(2.0),
            Rect::from_xywh(0.0, 0.0, 400.0, 400.0),
        );
        assert_eq!(phys, vec![Rect::from_xywh(20.0, 20.0, 80.0, 40.0)]);

        // Full and None carry no rects (caller treats empty as "whole surface").
        assert!(
            Damage::Full
                .to_physical(ScaleFactor::IDENTITY, Rect::from_xywh(0.0, 0.0, 10.0, 10.0))
                .is_empty()
        );
        assert!(
            Damage::None
                .to_physical(ScaleFactor::IDENTITY, Rect::from_xywh(0.0, 0.0, 10.0, 10.0))
                .is_empty()
        );
    }

    /// Images compare by identity: the same `Arc` is no damage, a different
    /// one (even with equal pixels) damages exactly the image's box.
    #[test]
    fn swapped_image_damages_its_node() {
        use crate::geometry::{Insets, PhysicalSize};
        use crate::render::{ImageFit, ImageSource, Pixmap};
        use std::sync::Arc;

        let img = |pm: &Arc<Pixmap>| {
            let mut n = node(Rect::from_xywh(10.0, 10.0, 16.0, 16.0), None, vec![]);
            n.content = NodeContent::Image {
                source: ImageSource::from(pm),
                fit: ImageFit::Contain,
                padding: Insets::default(),
            };
            n
        };
        let a_pm = Arc::new(Pixmap::new(PhysicalSize::new(4, 4)));
        let b_pm = Arc::new(Pixmap::new(PhysicalSize::new(4, 4)));
        let a = root(vec![img(&a_pm)]);
        assert_eq!(diff_trees(&a, &root(vec![img(&a_pm)])), Damage::None);
        assert_eq!(
            diff_trees(&a, &root(vec![img(&b_pm)])),
            Damage::Regions(vec![Rect::from_xywh(10.0, 10.0, 16.0, 16.0)])
        );
    }

    /// A box in the corner holding a child, for the transform tests.
    fn boxed_child() -> LayoutNode {
        node(
            Rect::from_xywh(20.0, 20.0, 20.0, 20.0),
            Some(Color::WHITE),
            vec![node(
                Rect::from_xywh(25.0, 25.0, 10.0, 10.0),
                Some(Color::BLACK),
                Vec::new(),
            )],
        )
    }

    #[test]
    fn a_transform_change_damages_where_it_was_and_is_drawn() {
        use crate::geometry::Transform;
        let a = root(vec![boxed_child()]);
        let mut b = a.clone();
        // Doubled about its center: (20, 20, 20, 20) draws over (10, 10, 40, 40).
        b.children[0].transform = Some(Transform::IDENTITY.scale(2.0));
        let bound = diff_trees(&a, &b).bounding().unwrap();
        assert!(bound.min_x() <= 10.0 && bound.min_y() <= 10.0, "{bound:?}");
        assert!(bound.max_x() >= 50.0 && bound.max_y() >= 50.0, "{bound:?}");
        assert!(bound.max_x() < 100.0, "stays local: {bound:?}");
    }

    #[test]
    fn a_change_inside_a_transformed_node_damages_its_drawn_place() {
        use crate::geometry::Transform;
        let mut a = root(vec![boxed_child()]);
        // Moved 100 px right: its child is drawn at (125, 25).
        a.children[0].transform = Some(Transform::IDENTITY.translate(100.0, 0.0));
        let mut b = a.clone();
        b.children[0].children[0].decoration.fill = Some(Color::rgb(255, 0, 0));
        let bound = diff_trees(&a, &b).bounding().unwrap();
        assert!(
            bound.min_x() <= 125.0 && bound.max_x() >= 135.0,
            "{bound:?}"
        );
        assert!(
            bound.min_x() > 100.0,
            "not where it was laid out: {bound:?}"
        );
    }

    #[test]
    fn an_opacity_change_damages_the_subtree() {
        let a = root(vec![boxed_child()]);
        let mut b = a.clone();
        b.children[0].opacity = 0.5;
        assert_eq!(
            diff_trees(&a, &b),
            Damage::Regions(vec![Rect::from_xywh(20.0, 20.0, 20.0, 20.0)])
        );
    }
}
