//! Which OS-drawn controls are real OS controls this frame (see
//! `docs/native-theme.md`, *Real controls*).
//!
//! Under [`Theme::native`](crate::style::Theme::native), in a window that
//! hosts them (UIKit, AppKit, Win32), each OS-backed control is a real
//! `UIView` / `NSView` / child `HWND` over its laid-out box: the OS draws
//! it live, with its own effects, and handles its input. The window's
//! controls sit above everything Stipple draws, so a control is live only
//! where that's what the frame would show. It gets the OS's picture drawn
//! into the frame instead (the snapshot path, as off screen) for as long as:
//!
//! - something Stipple paints later overlaps it, or something that takes
//!   input does (an overlay, a menu, a dialog's scrim or a popup's
//!   click-catcher, a toast, a scroll bar's track, a badge); an overlay
//!   scroll indicator's thumb alone passes under it;
//! - it, or anything around it, is scaled, rotated or translucent (a page
//!   transition, a fade); a plain move it follows;
//! - a clip that isn't a scroll area's (a rounded panel's) cuts part of it
//!   off. A scroll area is a clipping container of the OS's (see
//!   [`NativeClip`]): a control half scrolled out of it stays live, and the
//!   OS cuts it at the edge; one wholly out of it isn't shown;
//! - it holds keyboard focus with the focus ring showing (the OS's own
//!   focus look is then drawn in the picture);
//! - it is leaving ([ghost](LayoutNode::ghost)) or off stage;
//! - the window cross-fades between themes;
//! - it is a button whose content isn't a single line of text (the OS
//!   control shows a title, not Stipple's content), or a text field (whose
//!   editing stays Stipple's; only its frame is the OS's).
//!
//! A live control is marked on its node (`native_live`): the painter skips
//! it and its subtree (its label is the OS control's title), and the
//! accessibility tree names it by the same id (`native_a11y`, on the node
//! a screen reader reads as the control: a checkbox's row), so the
//! platform's bridge exposes the OS control there instead of a node of its
//! own.
//!
//! A control is known across frames by its element's
//! [key](crate::core::Element::key), else its tap, drag or focus handle
//! (as `native_fade` knows it), else its place in the tree, together with
//! its kind; events from the OS come back by that id and reach the
//! element's handler through the [`Route`] kept for it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::core::runtime::{ActionId, DragId, FocusId, LayoutNode, NodeContent};
use crate::geometry::{Affine, Rect};
use crate::platform::native::{NativeClip, NativeControl, NativeMenu, NativeView};

/// How an OS control's events reach its element.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Route {
    /// The tap handler a press or a flip runs (a checkbox's row's).
    pub(crate) action: Option<ActionId>,
    /// The drag handler a slider's new value goes to.
    pub(crate) drag: Option<DragId>,
    /// The control's box, in its layout coordinates (a slider's value is a
    /// position across it).
    pub(crate) bounds: Rect,
    /// Where it is in the window (a tap's pointer position).
    pub(crate) frame: Rect,
    /// A select's: the tap handler a pick of each option runs.
    pub(crate) picks: Vec<ActionId>,
}

/// What the App needs to decide this frame's live controls.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PlanInput {
    /// The window hosts real OS controls.
    pub(crate) enabled: bool,
    /// Every control takes the snapshot path this frame (a theme
    /// cross-fade).
    pub(crate) suspend: bool,
    /// The element with keyboard focus, and whether its ring shows.
    pub(crate) focused: Option<FocusId>,
    pub(crate) focus_visible: bool,
}

/// One window's live OS controls.
#[derive(Debug, Default)]
pub(crate) struct NativeHost {
    views: Vec<NativeView>,
    routes: HashMap<u64, Route>,
    /// The list changed since the window was last given it.
    changed: bool,
}

impl NativeHost {
    /// Work out which of `tree`'s OS-drawn controls are live this frame,
    /// mark them on the tree, and keep their specs for the window.
    pub(crate) fn plan(&mut self, tree: &mut LayoutNode, input: PlanInput) {
        let mut views = Vec::new();
        let mut routes = HashMap::new();
        if input.enabled && (!self.views.is_empty() || has_native(tree)) {
            let mut walk = Walk {
                input,
                cands: Vec::new(),
                path: Vec::new(),
            };
            walk.visit(
                tree,
                Affine::IDENTITY,
                1.0,
                &mut Clips::default(),
                Owner::default(),
            );

            let live: HashMap<Vec<u32>, (u64, Vec<u32>)> = walk
                .cands
                .iter()
                .filter(|c| c.ok && !c.covered)
                .map(|c| (c.path.clone(), (c.view.id, c.owner_path.clone())))
                .collect();
            for c in walk.cands.into_iter().filter(|c| c.ok && !c.covered) {
                routes.insert(c.view.id, c.route);
                views.push(c.view);
            }
            mark(tree, &live);
        }
        if views != self.views {
            self.changed = true;
        }
        self.views = views;
        self.routes = routes;
    }

    /// The live controls, in paint order, if they changed since the last
    /// call (and always on the first).
    pub(crate) fn take_changed(&mut self) -> Option<&[NativeView]> {
        std::mem::take(&mut self.changed).then_some(&self.views[..])
    }

    /// How control `id`'s events reach its element.
    pub(crate) fn route(&self, id: u64) -> Option<Route> {
        self.routes.get(&id).cloned()
    }

    /// Hand the window the list again on the next frame, changed or not:
    /// the user acted on a control, which then shows what the app's state
    /// says (a pick or a flip the app refused goes back).
    pub(crate) fn resend(&mut self) {
        self.changed = true;
    }

    /// Whether any control is live.
    pub(crate) fn any_live(&self) -> bool {
        !self.views.is_empty()
    }

    /// The live controls this frame, in paint order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn views(&self) -> &[NativeView] {
        &self.views
    }
}

fn has_native(node: &LayoutNode) -> bool {
    node.native.is_some() || node.children.iter().any(has_native)
}

/// Write the plan on the tree: `native_live` on each live control's node,
/// `native_a11y` on the node read as it.
fn mark(tree: &mut LayoutNode, live: &HashMap<Vec<u32>, (u64, Vec<u32>)>) {
    let owners: HashMap<&[u32], u64> = live
        .values()
        .map(|(id, owner)| (owner.as_slice(), *id))
        .collect();
    fn walk(
        node: &mut LayoutNode,
        path: &mut Vec<u32>,
        live: &HashMap<Vec<u32>, (u64, Vec<u32>)>,
        owners: &HashMap<&[u32], u64>,
    ) {
        node.native_live = live.get(path.as_slice()).map(|(id, _)| *id);
        node.native_a11y = owners.get(path.as_slice()).copied();
        for (i, child) in node.children.iter_mut().enumerate() {
            path.push(i as u32);
            walk(child, path, live, owners);
            path.pop();
        }
    }
    walk(tree, &mut Vec::new(), live, &owners);
}

/// The nearest node around a control with a tap handle: what a box that is
/// part of a labeled control (a checkbox's, a radio button's or a switch's,
/// in its row) belongs to, for its input and accessibility.
#[derive(Clone, Copy, Debug, Default)]
struct Owner<'a> {
    node: Option<&'a LayoutNode>,
    /// How deep in the tree it is: its path is the walk's path up to here.
    depth: usize,
}

#[derive(Debug)]
struct Cand {
    view: NativeView,
    route: Route,
    path: Vec<u32>,
    owner_path: Vec<u32>,
    /// It can be live, as far as it goes (no transform, clip or focus in
    /// the way).
    ok: bool,
    /// Something painted after it overlaps it.
    covered: bool,
    /// The part of it its scroll areas show (what something painted later
    /// can cover), in window coordinates.
    visible: Rect,
    /// Its subtree is being walked (what's in it is its own content, not
    /// over it).
    open: bool,
}

/// The clips around a node, in window coordinates.
#[derive(Clone, Debug, Default)]
struct Clips {
    /// The ones that aren't scroll areas', intersected: a control must be
    /// wholly inside them to be live.
    hard: Option<Rect>,
    /// The scroll areas, outermost first: the OS clips a live control to
    /// them (each is a container of its own).
    areas: Vec<NativeClip>,
}

impl Clips {
    /// The part of `rect` they show (what's drawn there), if any.
    fn cut(&self, rect: Rect) -> Option<Rect> {
        let r = match self.hard {
            Some(h) => rect.intersection(h)?,
            None => rect,
        };
        self.areas
            .iter()
            .try_fold(r, |r, a| r.intersection(a.frame))
    }
}

struct Walk {
    input: PlanInput,
    cands: Vec<Cand>,
    path: Vec<u32>,
}

/// Whether `a` is a plain move (what a live control can follow).
fn is_translation(a: Affine) -> bool {
    let [a, b, c, d, _, _] = a.as_array();
    a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0
}

/// Whether `node` puts anything in the frame of its own (or takes input
/// there), not counting its children.
fn paints(node: &LayoutNode) -> bool {
    let d = &node.decoration;
    d.fill.is_some_and(|c| c.a > 0)
        || d.border.is_some()
        || node.background.is_some()
        || !matches!(node.content, NodeContent::None)
        || node.native.is_some()
        || (!node.hit_through
            && (node.action.is_some()
                || node.drag.is_some()
                || node.pointer.is_some()
                || node.drag_source.is_some()
                || node.text_pos.is_some()))
}

/// The controls that can be live here (a text field's editing stays
/// Stipple's; and not every OS has every control: `os_views::live_kind`).
fn live_kind(control: NativeControl) -> bool {
    // The tests plan as for a window that shows every kind live.
    if cfg!(test) {
        !matches!(control, NativeControl::TextField)
    } else {
        crate::platform::native::os_views::live_kind(control)
    }
}

/// A button's title and its size: its content, when that is one line of
/// text and nothing else drawn.
fn sole_text(node: &LayoutNode) -> Option<(String, f64)> {
    fn walk<'a>(n: &'a LayoutNode, found: &mut Option<&'a LayoutNode>) -> bool {
        let d = &n.decoration;
        if n.native.is_some()
            || n.background.is_some()
            || d.fill.is_some_and(|c| c.a > 0)
            || d.border.is_some()
        {
            return false;
        }
        match &n.content {
            NodeContent::Text { .. } if found.is_none() => *found = Some(n),
            NodeContent::None => {}
            _ => return false,
        }
        n.children.iter().all(|c| walk(c, found))
    }
    let mut found = None;
    if !node.children.iter().all(|c| walk(c, &mut found)) {
        return None;
    }
    match found.map(|n| &n.content) {
        Some(NodeContent::Text {
            text, style, size, ..
        }) if !text.contains('\n') => Some((style.plain(text, 0..text.len()), *size)),
        _ => None,
    }
}

/// The size of the first text in `node`.
fn text_size(node: &LayoutNode) -> Option<f64> {
    match &node.content {
        NodeContent::Text { size, .. } => Some(*size),
        _ => node.children.iter().find_map(text_size),
    }
}

impl Walk {
    fn visit<'a>(
        &mut self,
        node: &'a LayoutNode,
        outer: Affine,
        opacity: f32,
        clips: &mut Clips,
        owner: Owner<'a>,
    ) {
        let m = crate::core::diff::to_window(node, outer);
        let opacity = opacity * node.opacity;
        let rect = m.map_rect(node.bounds);
        // Over the controls before it?
        if paints(node) {
            self.cover(clips.cut(rect));
        }
        let owner = if node.action.is_some() {
            Owner {
                node: Some(node),
                depth: self.path.len(),
            }
        } else {
            owner
        };
        let mut opened = None;
        if let Some(deco) = node.native.filter(|d| live_kind(d.control)) {
            opened = Some(self.cands.len());
            let cand = self.candidate(node, deco, m, rect, opacity, clips, owner);
            self.cands.push(cand);
        }
        // A scroll area clips its content as a container of the OS's;
        // another clip (a panel's) the controls must be inside of.
        let outer_hard = clips.hard;
        let area = node.clip && node.scroll.is_some();
        if area {
            clips.areas.push(NativeClip {
                id: area_identity(node, &self.path),
                frame: rect,
            });
        } else if node.clip {
            clips.hard = Some(clips.hard.map_or(rect, |c| {
                c.intersection(rect).unwrap_or(Rect::from_xywh(
                    rect.min_x(),
                    rect.min_y(),
                    0.0,
                    0.0,
                ))
            }));
        }
        for (i, child) in node.children.iter().enumerate() {
            self.path.push(i as u32);
            self.visit(child, m, opacity, clips, owner);
            self.path.pop();
        }
        if area {
            clips.areas.pop();
        }
        clips.hard = outer_hard;
        if let Some(i) = opened {
            self.cands[i].open = false;
        }
        // A scroll bar, over the container's content. An overlay
        // indicator's thumb alone (no track: iOS's, macOS's overlay
        // scrollers' while scrolling) passes under the live controls
        // rather than turning every row it crosses into a picture for as
        // long as the list moves.
        if let Some(bar) = &node.scroll_bar
            && let Some((track, _)) = bar.track
        {
            self.cover(clips.cut(m.map_rect(track)));
            if bar.thumb_color.a > 0 && bar.thumb.width() > 0.0 {
                self.cover(clips.cut(m.map_rect(bar.thumb)));
            }
        }
    }

    /// Something drawn at `rect` (window coordinates, the part its clips
    /// show) covers the controls it overlaps that were drawn before it.
    fn cover(&mut self, rect: Option<Rect>) {
        let Some(rect) = rect else {
            return;
        };
        for c in &mut self.cands {
            if !c.open && !c.covered && rect.intersection(c.visible).is_some() {
                c.covered = true;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn candidate(
        &self,
        node: &LayoutNode,
        deco: crate::platform::native::NativeDeco,
        m: Affine,
        rect: Rect,
        opacity: f32,
        clips: &Clips,
        owner: Owner<'_>,
    ) -> Cand {
        let input = &self.input;
        // A box in a labeled control is the control's; a button, a slider
        // or a progress bar is its own (whatever row it sits in).
        let part = matches!(
            deco.control,
            NativeControl::Checkbox { .. }
                | NativeControl::Radio { .. }
                | NativeControl::Switch { .. }
        ) && node.action.is_none();
        let owner = if part { owner } else { Owner::default() };
        let of_owner = |f: fn(&LayoutNode) -> Option<_>| owner.node.and_then(f);
        let mut ok = !input.suspend && !node.ghost && !node.offstage;
        // Only a plain move, fully opaque.
        ok &= is_translation(m) && opacity >= 1.0;
        // Wholly inside the clips that aren't scroll areas', and at least
        // partly in view in those that are (which the OS clips it to).
        if let Some(c) = clips.hard {
            ok &= contains(c, rect);
        }
        let visible = clips
            .areas
            .iter()
            .try_fold(rect, |r, c| r.intersection(c.frame));
        ok &= visible.is_some();
        let visible = visible.unwrap_or(Rect::from_xywh(rect.min_x(), rect.min_y(), 0.0, 0.0));
        // A select's options, which its OS control (a pop-up button) shows.
        let choices = node.semantics.as_ref().and_then(|s| s.choices.clone());
        // Focused with the ring showing: the OS's focus look, in the
        // picture.
        let focus = node.focus.or(owner.node.and_then(|o| o.focus));
        ok &= !(input.focus_visible && focus.is_some() && focus == input.focused);
        let (title, font_size) = match deco.control {
            // A select's title is its chosen option, in its text's size.
            NativeControl::Button { .. } if choices.is_some() => {
                let c = choices.as_deref().expect("choices");
                let title = c.selected.and_then(|i| c.labels.get(i));
                (
                    title.cloned().unwrap_or_else(|| "\u{2014}".into()),
                    text_size(node).unwrap_or(0.0),
                )
            }
            NativeControl::Button { .. } => sole_text(node).unwrap_or_else(|| {
                ok = false;
                (String::new(), 0.0)
            }),
            _ => (String::new(), 0.0),
        };
        let path = self.path.clone();
        let id = identity(node, owner, &path, deco.control);
        let action = node.action.or(of_owner(|o| o.action));
        // Stipple draws a control without a handler as it does one with, so
        // the OS control looks enabled too.
        let enabled = true;
        // A select is named by its label, if it has one (its text is its
        // value, which the OS control reads out itself).
        let label = match &choices {
            Some(_) => node
                .semantics
                .as_ref()
                .and_then(|s| s.label.clone())
                .unwrap_or_default(),
            None => crate::core::a11y::name_of(owner.node.unwrap_or(node)),
        };
        let owner_path = match owner.node {
            Some(_) => path[..owner.depth].to_vec(),
            None => path.clone(),
        };
        Cand {
            view: NativeView {
                id,
                control: deco.control,
                frame: rect,
                title,
                font_size,
                label,
                enabled,
                dark: deco.dark,
                rtl: node.rtl,
                menu: choices.as_ref().map(|c| NativeMenu {
                    options: c.labels.clone(),
                    selected: c.selected,
                }),
                clips: clips.areas.clone(),
            },
            route: Route {
                action,
                drag: node.drag,
                bounds: node.bounds,
                frame: rect,
                picks: choices.map(|c| c.picks.clone()).unwrap_or_default(),
            },
            path,
            owner_path,
            ok,
            covered: false,
            visible,
            open: true,
        }
    }
}

/// Whether `outer` contains `inner` (within a hair, for rounding).
fn contains(outer: Rect, inner: Rect) -> bool {
    const E: f64 = 0.01;
    inner.min_x() >= outer.min_x() - E
        && inner.min_y() >= outer.min_y() - E
        && inner.max_x() <= outer.max_x() + E
        && inner.max_y() <= outer.max_y() + E
}

/// What scroll area `node` (a clipping container) is known by across
/// frames: its scroll handle, else its key, else its place in the tree.
fn area_identity(node: &LayoutNode, path: &[u32]) -> u64 {
    let mut h = DefaultHasher::new();
    "scroll area".hash(&mut h);
    if let Some(key) = node.key {
        (0u8, key).hash(&mut h);
    } else if let Some(id) = node.scroll {
        (1u8, id).hash(&mut h);
    } else {
        (2u8, path).hash(&mut h);
    }
    h.finish()
}

/// What control `node` is known by across frames.
fn identity(node: &LayoutNode, owner: Owner<'_>, path: &[u32], control: NativeControl) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(&control).hash(&mut h);
    let of_owner = |f: fn(&LayoutNode) -> Option<_>| owner.node.and_then(f);
    if let Some(key) = node.key.or(owner.node.and_then(|o| o.key)) {
        (0u8, key).hash(&mut h);
    } else if let Some(a) = node.action.or(of_owner(|o| o.action)) {
        (1u8, a.0).hash(&mut h);
    } else if let Some(d) = node.drag {
        (2u8, d.0).hash(&mut h);
    } else if let Some(f) = node.focus.or(owner.node.and_then(|o| o.focus)) {
        (3u8, f.0).hash(&mut h);
    } else {
        (4u8, path).hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Axis, Cx, Element, layout};
    use crate::geometry::{Insets, Size};
    use crate::platform::native::{NativeDeco, NativeState};
    use crate::render::Color;
    use crate::style::Theme;

    fn deco(control: NativeControl) -> NativeDeco {
        NativeDeco {
            control,
            dark: false,
            state: NativeState::default(),
        }
    }

    const BUTTON: NativeControl = NativeControl::Button { primary: false };

    /// A button as `widgets::button` makes it under the native theme.
    fn button(cx: &mut Cx<u32>, title: &str) -> Element {
        Element::stack(
            Axis::Horizontal,
            vec![Element::text(title, 14.0, Color::BLACK)],
        )
        .native(deco(BUTTON))
        .width(80.0)
        .height(30.0)
        .on_tap(cx, |s| *s += 1)
    }

    fn lay(el: Element) -> LayoutNode {
        let win = Rect::from_xywh(0.0, 0.0, 400.0, 300.0);
        layout(&el, win, None)
    }

    fn on() -> PlanInput {
        PlanInput {
            enabled: true,
            ..PlanInput::default()
        }
    }

    fn live(tree: &LayoutNode) -> Vec<u64> {
        let mut out = Vec::new();
        fn walk(n: &LayoutNode, out: &mut Vec<u64>) {
            out.extend(n.native_live);
            n.children.iter().for_each(|c| walk(c, out));
        }
        walk(tree, &mut out);
        out
    }

    #[test]
    fn a_plain_button_is_live_with_its_title_and_route() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let el = Element::stack(Axis::Vertical, vec![button(&mut cx, "Save")])
            .padding(Insets::uniform(10.0));
        let mut tree = lay(el);
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let views = host.views().to_vec();
        assert_eq!(views.len(), 1);
        let v = &views[0];
        assert_eq!(v.title, "Save");
        assert_eq!(v.label, "Save");
        assert!(v.enabled);
        assert_eq!(v.frame, Rect::from_xywh(10.0, 10.0, 80.0, 30.0));
        assert_eq!(live(&tree), [v.id]);
        let route = host.route(v.id).unwrap();
        assert!(route.action.is_some());
        // The first frame's list goes to the window, then only changes.
        assert!(host.take_changed().is_some());
        host.plan(&mut tree, on());
        assert!(host.take_changed().is_none());
        // The same control next frame keeps its id.
        let mut cx = Cx::<u32>::new(&theme);
        let mut again = lay(
            Element::stack(Axis::Vertical, vec![button(&mut cx, "Saved")])
                .padding(Insets::uniform(10.0)),
        );
        host.plan(&mut again, on());
        assert_eq!(host.views()[0].id, v.id);
        assert_eq!(host.take_changed().unwrap()[0].title, "Saved");
    }

    #[test]
    fn nothing_is_live_where_the_window_hosts_no_controls() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let mut tree = lay(button(&mut cx, "Save"));
        let mut host = NativeHost::default();
        host.plan(&mut tree, PlanInput::default());
        assert!(host.views().is_empty());
        assert!(live(&tree).is_empty());
        // A cross-fade suspends them all.
        host.plan(
            &mut tree,
            PlanInput {
                suspend: true,
                ..on()
            },
        );
        assert!(host.views().is_empty());
    }

    #[test]
    fn something_drawn_over_a_control_makes_it_a_picture() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        // A badge drawn after the button, over its corner.
        let save = lay(button(&mut cx, "Save"));
        let mut other = lay(button(&mut cx, "Other"));
        crate::core::translate(&mut other, crate::geometry::Vec2::new(0.0, 100.0));
        let mut badge = lay(Element::boxed(crate::core::BoxStyle {
            fill: Some(Color::rgb(255, 0, 0)),
            radius: 0.0,
            border: None,
        })
        .width(20.0)
        .height(20.0));
        crate::core::translate(&mut badge, crate::geometry::Vec2::new(70.0, -5.0));
        let mut tree = LayoutNode::container(
            Rect::from_xywh(0.0, 0.0, 400.0, 300.0),
            vec![save, badge, other],
        );
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let titles: Vec<&str> = host.views().iter().map(|v| v.title.as_str()).collect();
        assert_eq!(titles, ["Other"]);
    }

    #[test]
    fn an_overlay_over_the_window_makes_the_controls_under_it_pictures() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let main = lay(button(&mut cx, "Save"));
        // A popup's invisible click-catcher across the window, then the
        // popup with a button of its own.
        let mut catcher = lay(Element::boxed(Default::default())
            .width(400.0)
            .height(300.0));
        catcher.action = Some(ActionId(99));
        let mut popup = lay(button(&mut cx, "In popup"));
        crate::core::translate(&mut popup, crate::geometry::Vec2::new(200.0, 100.0));
        let mut tree = LayoutNode::container(
            Rect::from_xywh(0.0, 0.0, 400.0, 300.0),
            vec![main, catcher, popup],
        );
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let titles: Vec<&str> = host.views().iter().map(|v| v.title.as_str()).collect();
        assert_eq!(titles, ["In popup"]);
    }

    #[test]
    fn a_transform_a_fade_or_a_partial_clip_makes_a_picture_and_a_move_does_not() {
        let theme = Theme::light();
        let scaled = {
            let mut cx = Cx::<u32>::new(&theme);
            button(&mut cx, "Save").transform(crate::geometry::Transform::IDENTITY.scale(0.9))
        };
        let faded = {
            let mut cx = Cx::<u32>::new(&theme);
            button(&mut cx, "Save").opacity(0.5)
        };
        let moved = {
            let mut cx = Cx::<u32>::new(&theme);
            button(&mut cx, "Save")
                .transform(crate::geometry::Transform::IDENTITY.translate(5.0, 7.0))
        };
        let mut host = NativeHost::default();
        for (el, live_after) in [(scaled, false), (faded, false), (moved, true)] {
            let mut tree = lay(el);
            host.plan(&mut tree, on());
            assert_eq!(host.any_live(), live_after);
        }
        let mut tree = lay(Element::stack(Axis::Vertical, Vec::new()));
        host.plan(&mut tree, on());
        // The moved one shows where it's drawn.
        let mut cx = Cx::<u32>::new(&theme);
        let mut tree = lay(button(&mut cx, "Save")
            .transform(crate::geometry::Transform::IDENTITY.translate(5.0, 7.0)));
        host.plan(&mut tree, on());
        assert_eq!(host.views()[0].frame.origin.x, 5.0);
        assert_eq!(host.views()[0].frame.origin.y, 7.0);
        // A scroll container showing half of the second button.
        let mut cx = Cx::<u32>::new(&theme);
        let list = Element::stack(
            Axis::Vertical,
            vec![button(&mut cx, "One"), button(&mut cx, "Two")],
        )
        .height(45.0);
        let mut tree = lay(Element::stack(Axis::Vertical, vec![list]));
        tree.children[0].clip = true;
        host.plan(&mut tree, on());
        let titles: Vec<&str> = host.views().iter().map(|v| v.title.as_str()).collect();
        assert_eq!(titles, ["One"]);
    }

    #[test]
    fn a_checkbox_row_is_read_as_its_box_and_its_box_flips_the_row() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let boxed = Element::boxed(Default::default())
            .native(deco(NativeControl::Checkbox { checked: true }))
            .width(20.0)
            .height(20.0);
        let row = Element::stack(
            Axis::Horizontal,
            vec![
                boxed,
                Element::text("Show hidden files", 14.0, Color::BLACK),
            ],
        )
        .role(crate::core::Role::CheckBox)
        .on_tap(&mut cx, |s| *s += 1);
        let mut tree = lay(row);
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let v = host.views()[0].clone();
        assert_eq!(v.label, "Show hidden files");
        assert!(v.title.is_empty());
        assert!(v.enabled);
        // The row is what accessibility reads as the box.
        assert_eq!(tree.native_a11y, Some(v.id));
        assert_eq!(tree.children[0].native_live, Some(v.id));
        assert_eq!(host.route(v.id).unwrap().action, tree.action);
        let a11y = crate::core::accessibility_tree(&tree, None);
        assert_eq!(a11y.native_view, Some(v.id));
    }

    #[test]
    fn a_focused_control_with_its_ring_showing_is_a_picture() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let el = button(&mut cx, "Save").on_key(&mut cx, |_, _| {});
        let mut tree = lay(el);
        let focus = tree.focus;
        assert!(focus.is_some());
        let mut host = NativeHost::default();
        host.plan(
            &mut tree,
            PlanInput {
                focused: focus,
                focus_visible: true,
                ..on()
            },
        );
        assert!(!host.any_live());
        host.plan(
            &mut tree,
            PlanInput {
                focused: focus,
                focus_visible: false,
                ..on()
            },
        );
        assert!(host.any_live());
    }

    #[test]
    fn a_button_with_more_than_a_title_and_a_text_field_stay_pictures() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let two = Element::stack(
            Axis::Horizontal,
            vec![
                Element::text("A", 14.0, Color::BLACK),
                Element::text("B", 14.0, Color::BLACK),
            ],
        )
        .native(deco(BUTTON))
        .on_tap(&mut cx, |s| *s += 1);
        let field = Element::stack(
            Axis::Horizontal,
            vec![Element::text("typed", 14.0, Color::BLACK)],
        )
        .native(deco(NativeControl::TextField));
        let mut tree = lay(Element::stack(Axis::Vertical, vec![two, field]));
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        assert!(!host.any_live());
        let _ = Size::ZERO;
    }

    /// A list of `n` buttons 30 high, 8 apart, scrolled `offset` down, in a
    /// scroll area `height` high at y 0.
    fn scrolled_list(
        cx: &mut Cx<u32>,
        n: usize,
        height: f64,
        offset: f64,
        after: Option<Element>,
    ) -> LayoutNode {
        let id = cx.register_scroll();
        let rows = (1..=n).map(|i| button(cx, &format!("Row {i}"))).collect();
        let list = Element::stack(Axis::Vertical, rows)
            .gap(8.0)
            .height(height)
            .scrollable(id);
        let mut items = vec![list];
        items.extend(after);
        let mut tree = lay(Element::stack(Axis::Vertical, items));
        let mut offsets = HashMap::from([(id, offset)]);
        crate::core::apply_scroll(&mut tree, &mut offsets);
        tree
    }

    fn titled<'a>(host: &'a NativeHost, title: &str) -> Option<&'a NativeView> {
        host.views().iter().find(|v| v.title == title)
    }

    #[test]
    fn a_control_half_scrolled_out_stays_live_clipped_by_its_scroll_area() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        // Rows at 0, 38, 76, 114: a 95-high area shows two and a half.
        let mut tree = scrolled_list(&mut cx, 4, 95.0, 0.0, None);
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let titles: Vec<&str> = host.views().iter().map(|v| v.title.as_str()).collect();
        assert_eq!(titles, ["Row 1", "Row 2", "Row 3"]);
        let third = titled(&host, "Row 3").unwrap();
        assert_eq!(third.clips.len(), 1);
        assert_eq!(third.clips[0].frame.height(), 95.0);
        assert_eq!(third.frame, Rect::from_xywh(0.0, 76.0, 80.0, 30.0));
        assert_eq!(
            third.visible(),
            Some(Rect::from_xywh(0.0, 76.0, 80.0, 19.0))
        );
        let first = titled(&host, "Row 1").unwrap().clone();
        // Every row of the area is in the same container.
        assert!(host.views().iter().all(|v| v.clips == first.clips));
        // Scrolled 20 down: the first is cut at the top, the fourth comes in
        // at the bottom; the area (the container) stays where it is.
        let mut cx = Cx::<u32>::new(&theme);
        let mut tree = scrolled_list(&mut cx, 4, 95.0, 20.0, None);
        host.plan(&mut tree, on());
        let titles: Vec<&str> = host.views().iter().map(|v| v.title.as_str()).collect();
        assert_eq!(titles, ["Row 1", "Row 2", "Row 3", "Row 4"]);
        let moved = titled(&host, "Row 1").unwrap();
        assert_eq!(moved.id, first.id);
        assert_eq!(moved.clips, first.clips);
        assert_eq!(moved.frame.min_y(), -20.0);
        // Scrolled past it: wholly out, it isn't shown.
        let mut cx = Cx::<u32>::new(&theme);
        let mut tree = scrolled_list(&mut cx, 4, 95.0, 40.0, None);
        host.plan(&mut tree, on());
        assert!(titled(&host, "Row 1").is_none());
        assert!(titled(&host, "Row 2").is_some());
    }

    #[test]
    fn what_is_drawn_after_a_scroll_area_covers_only_the_part_it_shows() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        // A box right under the area, over Row 3's hidden part only.
        let under = Element::boxed(crate::core::BoxStyle {
            fill: Some(Color::rgb(255, 0, 0)),
            radius: 0.0,
            border: None,
        })
        .width(80.0)
        .height(40.0);
        let mut tree = scrolled_list(&mut cx, 4, 95.0, 0.0, Some(under));
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        assert!(titled(&host, "Row 3").is_some());
    }

    #[test]
    fn a_row_scrolled_out_over_a_control_above_its_area_does_not_cover_it() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let id = cx.register_scroll();
        let above = button(&mut cx, "Above");
        let rows = (1..=3)
            .map(|i| button(&mut cx, &format!("Row {i}")))
            .collect();
        let list = Element::stack(Axis::Vertical, rows)
            .gap(8.0)
            .height(60.0)
            .scrollable(id);
        // The list right under the button: its first row, scrolled 13 up,
        // reaches over the button's box but is cut at the area's top.
        let mut tree = lay(Element::stack(Axis::Vertical, vec![above, list]));
        let mut offsets = HashMap::from([(id, 13.0)]);
        crate::core::apply_scroll(&mut tree, &mut offsets);
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        assert!(titled(&host, "Above").is_some());
        assert!(titled(&host, "Row 1").is_some());
    }

    #[test]
    fn an_overlay_scroll_thumb_passes_under_live_rows_and_a_track_covers_them() {
        use crate::core::scroll_bar::BarPaint;
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let mut tree = scrolled_list(&mut cx, 2, 95.0, 0.0, None);
        // A thumb over the rows' right edge, as iOS draws its indicator.
        let thumb = Rect::from_xywh(74.0, 2.0, 4.0, 40.0);
        let mut bar = BarPaint {
            track: None,
            thumb,
            thumb_color: Color::BLACK,
        };
        tree.children[0].scroll_bar = Some(Box::new(bar.clone()));
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        assert!(titled(&host, "Row 1").is_some());
        bar.track = Some((Rect::from_xywh(72.0, 0.0, 8.0, 95.0), Color::WHITE));
        tree.children[0].scroll_bar = Some(Box::new(bar));
        host.plan(&mut tree, on());
        assert!(titled(&host, "Row 1").is_none());
    }

    #[test]
    fn nested_scroll_areas_nest_their_containers_and_other_clips_stay_strict() {
        let theme = Theme::light();
        let mut cx = Cx::<u32>::new(&theme);
        let inner_id = cx.register_scroll();
        let outer_id = cx.register_scroll();
        let inner = Element::stack(
            Axis::Vertical,
            vec![button(&mut cx, "In 1"), button(&mut cx, "In 2")],
        )
        .gap(8.0)
        .height(50.0)
        .scrollable(inner_id);
        let outer = Element::stack(Axis::Vertical, vec![button(&mut cx, "Top"), inner])
            .gap(8.0)
            .height(200.0)
            .scrollable(outer_id);
        let mut tree = lay(Element::stack(Axis::Vertical, vec![outer]));
        let mut offsets = HashMap::new();
        crate::core::apply_scroll(&mut tree, &mut offsets);
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let top = titled(&host, "Top").unwrap();
        let in2 = titled(&host, "In 2").unwrap();
        assert_eq!(top.clips.len(), 1);
        assert_eq!(in2.clips.len(), 2);
        assert_eq!(in2.clips[0], top.clips[0]);
        assert_ne!(in2.clips[1].id, in2.clips[0].id);
        // In 2 at 38 + 38 = 76, its area from 38 to 88: 12 of it shows.
        assert_eq!(in2.visible().unwrap().height(), 12.0);
        // A clip that isn't a scroll area's (a panel's) still keeps a
        // control it cuts a picture.
        let mut cx = Cx::<u32>::new(&theme);
        let panel = Element::stack(
            Axis::Vertical,
            vec![button(&mut cx, "One"), button(&mut cx, "Two")],
        )
        .height(45.0)
        .clip();
        let mut tree = lay(Element::stack(Axis::Vertical, vec![panel]));
        host.plan(&mut tree, on());
        let titles: Vec<&str> = host.views().iter().map(|v| v.title.as_str()).collect();
        assert_eq!(titles, ["One"]);
        assert!(host.views()[0].clips.is_empty());
    }

    #[test]
    fn a_select_is_a_pop_up_button_with_its_options_and_one_pick_each() {
        use crate::widgets::Select;
        // The native theme's deco, whatever this OS draws.
        let theme = Theme::light();
        let mut cx = Cx::<Select>::new(&theme);
        let state = Select::new(Some(1));
        let tap = cx.register(|s: &mut Select| s.toggle());
        let picks: Vec<ActionId> = (0..3)
            .map(|i| cx.register(move |s: &mut Select| s.choose(i)))
            .collect();
        let el = Element::stack(
            Axis::Horizontal,
            vec![
                Element::text("Medium", 14.0, Color::BLACK),
                Element::text("\u{25BE}", 14.0, Color::BLACK),
            ],
        )
        .native(deco(BUTTON))
        .width(200.0)
        .height(30.0)
        .a11y_label("Size")
        .native_choices(crate::core::a11y::Choices {
            labels: vec!["Small".into(), "Medium".into(), "Large".into()],
            selected: Some(1),
            picks: picks.clone(),
        });
        let mut el = el;
        el.action = Some(tap);
        let mut tree = lay(el);
        let mut host = NativeHost::default();
        host.plan(&mut tree, on());
        let v = &host.views()[0];
        let menu = v.menu.as_ref().unwrap();
        assert_eq!(menu.options, ["Small", "Medium", "Large"]);
        assert_eq!(menu.selected, Some(1));
        assert_eq!(v.title, "Medium");
        assert_eq!(v.font_size, 14.0);
        assert_eq!(v.label, "Size");
        let route = host.route(v.id).unwrap();
        assert_eq!(route.picks, picks);
        assert_eq!(route.action, Some(tap));
        let mut handlers = cx.into_handlers();
        let mut s = state.clone();
        handlers.dispatch(route.picks[2], &mut s);
        assert_eq!(s.selected(), Some(2));
    }
}
