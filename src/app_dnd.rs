//! Drag and drop in the [`App`]: drags from other applications
//! ([`Event::DragEnter`](crate::platform::Event::DragEnter) and friends) and
//! drags started inside the app on an
//! [`Element::drag_source`](crate::core::Element::drag_source), both routed to
//! the [`Element::on_drop`](crate::core::Element::on_drop) target under the
//! pointer. An in-app drag of files or text that leaves the window becomes a
//! drag for other applications ([`Window::start_drag`]). An in-app drag of an
//! [`Internal`](DropData::Internal) payload that leaves its window goes on
//! over the app's other windows ([`CrossDrag`]), and one released outside them
//! all tells its source ([`Element::on_drop_outside`](crate::core::Element::on_drop_outside)).
//! See `docs/drag-and-drop.md`.

use std::sync::Arc;

use crate::core::{DragSourceId, DropId, Font, LayoutNode, drag_source_at, drop_at, find_drop};
use crate::geometry::{Point, Rect, Size};
use crate::platform::{
    DragLift, DragOffer, DropData, Window, WindowAttributes, WindowId, WindowInfo,
};
use crate::render::{Color, Pixmap, Scene};
use crate::style::Theme;
use crate::{App, Pane};

/// How far (logical pixels) a press on a drag source moves before it becomes
/// an in-app drag.
const DRAG_THRESHOLD: f64 = 4.0;

/// The drag-and-drop state of one window.
#[derive(Debug, Default)]
pub(crate) struct DndState {
    /// What the drag over the window carries, while one is.
    offer: Option<DragOffer>,
    /// The drop target under the drag, whether it accepts it, and the
    /// pointer within it.
    pub(crate) over: Option<(DropId, bool, Point)>,
    /// The drop target under the drag, if it accepts it.
    pub(crate) target: Option<DropId>,
    /// The targets the app highlighted in the frame just built, each with
    /// its drag-over level (the highlight fades in and out with it), and the
    /// ones on screen (for damage).
    pub(crate) highlight: Vec<(DropId, f64)>,
    pub(crate) painted: Vec<(DropId, f64)>,
    /// The drop targets' easing drag-over levels.
    pub(crate) fades: crate::core::DropFades,
    /// The drop targets whose view read their state in the last build.
    pub(crate) queried: Vec<DropId>,
    /// A press on a drag source, not yet moved far enough to drag.
    press: Option<(DragSourceId, Point)>,
    /// Where the press that started the in-app drag in progress went down.
    origin: Point,
    /// The payload of the in-app drag in progress, and the drag source it
    /// started on.
    internal: Option<DropData>,
    pub(crate) source: Option<DragSourceId>,
}

impl DndState {
    /// Whether an in-app drag is in progress (it owns pointer moves and the
    /// release).
    pub(crate) fn dragging_internally(&self) -> bool {
        self.internal.is_some()
    }
}

impl<S> Pane<S> {
    /// Whether a gesture latched on a press (a text selection, a drag or an
    /// in-app drag) owns the pointer's moves.
    pub(crate) fn pointer_latched(&self) -> bool {
        self.text_selecting.is_some()
            || self.read_selecting
            || self.dragging.is_some()
            || self.dnd.dragging_internally()
    }
}

/// Paint the hover highlight of drop target `id`: the theme's hover overlay
/// with a focus-colored border, so the target reads as "drop here", at
/// `level` (0 to 1) of its full strength.
pub(crate) fn paint_drop_target(
    tree: &LayoutNode,
    id: DropId,
    scene: &mut Scene,
    theme: &Theme,
    level: f64,
) {
    let Some(node) = find_drop(tree, id) else {
        return;
    };
    // Drawn where the target is drawn, if it's transformed.
    let to_window = crate::core::node_to_window(tree, &|n| n.drop == Some(id))
        .map_or(crate::geometry::Affine::IDENTITY, |(_, m, _)| m);
    let level = level.clamp(0.0, 1.0);
    let layer = !to_window.is_identity() || level < 1.0;
    if layer {
        scene.push_layer(to_window, level as f32);
    }
    paint_drop_target_at(node, scene, theme);
    if layer {
        scene.pop_layer();
    }
}

/// The drop targets the app highlights, from the targets' drag-over `states`:
/// the ones that take the drag over them (or that just left, while their
/// level eases out) and draw no look of their own (`unstyled`), each with its
/// level.
pub(crate) fn highlighted(
    states: &[(DropId, crate::core::DropState)],
    unstyled: impl Fn(DropId) -> bool,
) -> Vec<(DropId, f64)> {
    states
        .iter()
        .filter(|(id, s)| s.accepted && s.level > 0.0 && unstyled(*id))
        .map(|(id, s)| (*id, s.level))
        .collect()
}

/// [`paint_drop_target`] for the target `node`, in its layout coordinates.
fn paint_drop_target_at(node: &LayoutNode, scene: &mut Scene, theme: &Theme) {
    let fill = theme.palette.hover_overlay;
    let ring = theme.palette.focus_ring;
    let b = node.bounds;
    let r = node.decoration.radius;
    if r > 0.0 {
        scene.fill_round_rect(b, r, fill);
    } else {
        scene.fill_rect(b, fill);
    }
    // A 2 px border inside the bounds, as four bars.
    let w = 2.0f64.min(b.width() / 2.0).min(b.height() / 2.0);
    let bars = [
        Rect::from_xywh(b.min_x(), b.min_y(), b.width(), w),
        Rect::from_xywh(b.min_x(), b.max_y() - w, b.width(), w),
        Rect::from_xywh(b.min_x(), b.min_y() + w, w, b.height() - 2.0 * w),
        Rect::from_xywh(b.max_x() - w, b.min_y() + w, w, b.height() - 2.0 * w),
    ];
    for bar in bars {
        scene.fill_rect(bar, Color { a: 0xff, ..ring });
    }
}

impl<S: 'static> Pane<S> {
    /// Move the drag (carrying `self.dnd.offer`) to `pos`: find the drop
    /// target under it and whether it accepts the drag, telling the old and
    /// new accepting targets' hover handlers when that changes, and asking
    /// for a rebuild when a view reading the state has something new to show.
    /// Returns whether a target accepts it.
    fn dnd_hover(&mut self, state: &mut S, theme: &Theme, font: Option<&Font>, pos: Point) -> bool {
        let Some(offer) = self.dnd.offer else {
            return false;
        };
        self.last_pointer = pos;
        self.ensure_tree(state, theme, font);
        let over = self
            .tree
            .as_ref()
            .and_then(|t| drop_at(t, pos).map(|hit| (t, hit)))
            .map(|(t, (id, b))| {
                // Through any transform on the way to the target.
                let at = crate::core::local_point(t, &|n| n.drop == Some(id), pos).unwrap_or(pos);
                let local = Point::new(at.x - b.min_x(), at.y - b.min_y());
                (id, self.handlers.drop_accepts(id, offer), local)
            });
        let target = over.filter(|o| o.1).map(|o| o.0);
        if target != self.dnd.target {
            if let Some(old) = self.dnd.target {
                self.handlers.dispatch_drop_hover(old, false, state);
            }
            if let Some(new) = target {
                self.handlers.dispatch_drop_hover(new, true, state);
            }
            self.dnd.target = target;
            self.dirty = true;
        }
        let key = |o: Option<(DropId, bool, Point)>| o.map(|(id, ok, _)| (id, ok));
        // Entering, leaving or a new verdict rebuilds; so does a move over a
        // target whose view follows the pointer.
        let moved = over != self.dnd.over && over.is_some_and(|o| self.dnd.queried.contains(&o.0));
        if key(over) != key(self.dnd.over) || moved {
            self.dirty = true;
        }
        self.dnd.over = over;
        target.is_some()
    }

    /// A drag carrying `offer` entered the window at `pos`. Returns whether a
    /// drop target there accepts it.
    pub(crate) fn drag_enter(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
        offer: DragOffer,
    ) -> bool {
        self.dnd.offer = Some(offer);
        self.dnd_hover(state, theme, font, pos)
    }

    /// The drag moved to `pos`. Returns whether a drop target there accepts it.
    pub(crate) fn drag_over(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
    ) -> bool {
        self.dnd_hover(state, theme, font, pos)
    }

    /// The drag left the window (or was cancelled).
    pub(crate) fn drag_leave(&mut self, state: &mut S) {
        if let Some(old) = self.dnd.target.take() {
            self.handlers.dispatch_drop_hover(old, false, state);
        }
        if self.dnd.over.take().is_some() {
            self.dirty = true;
        }
        self.dnd.offer = None;
    }

    /// `data` was dropped at `pos`: hand it to the drop target there that
    /// accepts it. Returns whether one did.
    pub(crate) fn drop_data(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
        data: &DropData,
    ) -> bool {
        // The drop's own kind decides (a drag offering files and text drops
        // one of them), so look again with it.
        self.dnd.offer = Some(data.kind());
        self.dnd_hover(state, theme, font, pos);
        let target = self.dnd.target;
        self.drag_leave(state);
        let Some(id) = target else {
            return false;
        };
        self.handlers.dispatch_drop(id, data, pos, state);
        self.dirty = true;
        true
    }

    /// A left press at `pos`: latch the drag source under it, if any, so
    /// moving far enough starts an in-app drag.
    pub(crate) fn source_press(&mut self, pos: Point) {
        self.dnd.press = self
            .tree
            .as_ref()
            .and_then(|t| drag_source_at(t, pos))
            .map(|id| (id, pos));
    }

    /// The pointer moved to `pos` with the button down. Starts an in-app drag
    /// once a latched press has moved far enough, and moves one in progress.
    /// Returns whether an in-app drag owns the move.
    pub(crate) fn source_move(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
    ) -> bool {
        if self.dnd.internal.is_some() {
            self.dnd_hover(state, theme, font, pos);
            return true;
        }
        let Some((id, origin)) = self.dnd.press else {
            return false;
        };
        if (pos.x - origin.x).hypot(pos.y - origin.y) < DRAG_THRESHOLD {
            return false;
        }
        self.dnd.press = None;
        self.dnd.origin = origin;
        let Some(data) = self.handlers.dispatch_drag_source(id, state) else {
            return false;
        };
        // The press became a drag: it no longer taps or drags its element.
        self.pressed = None;
        self.press_at = None;
        self.dragging = None;
        self.dnd.source = Some(id);
        self.dirty = true;
        self.dnd.offer = Some(data.kind());
        self.dnd.internal = Some(data);
        self.dnd_hover(state, theme, font, pos);
        true
    }

    /// The button was released at `pos`. Drops an in-app drag in progress on
    /// the target there. Returns whether an in-app drag owned the release.
    pub(crate) fn source_release(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
    ) -> bool {
        self.dnd.press = None;
        self.dnd.source = None;
        match self.dnd.internal.take() {
            Some(data) => {
                self.drop_data(state, theme, font, pos, &data);
                true
            }
            None => false,
        }
    }

    /// The payload of the in-app drag in progress if the pointer, at `pos`,
    /// has left the window and the payload can leave the app with it (files
    /// or text; see [`DropData::can_leave_app`]).
    pub(crate) fn leaving_payload(&self, pos: Point) -> Option<&DropData> {
        let size = self.attrs.logical_size;
        let outside = pos.x < 0.0 || pos.y < 0.0 || pos.x >= size.width || pos.y >= size.height;
        self.dnd
            .internal
            .as_ref()
            .filter(|data| outside && data.can_leave_app())
    }

    /// The in-app drag became the platform's: end it here, like a drag that
    /// left the window (targets stop hovering). If it comes back over the
    /// window it arrives like any drag from another application.
    pub(crate) fn handed_off(&mut self, state: &mut S) {
        self.dnd.press = None;
        self.dnd.source = None;
        self.dirty = true;
        // The system's drag has the pointer now; its release won't come here.
        self.pointer.release_all();
        if self.dnd.internal.take().is_some() {
            self.drag_leave(state);
        }
    }

    /// End the in-app drag in progress here without dropping it (it goes to
    /// another window, or outside them all), returning its payload and the
    /// drag source it started on.
    pub(crate) fn take_internal(
        &mut self,
        state: &mut S,
    ) -> Option<(DropData, Option<DragSourceId>)> {
        self.dnd.press = None;
        let source = self.dnd.source.take();
        let data = self.dnd.internal.take()?;
        self.drag_leave(state);
        self.dirty = true;
        Some((data, source))
    }

    /// Whether the in-app drag in progress started on a drag source that
    /// takes being dropped outside the app's windows.
    fn source_drops_outside(&self) -> bool {
        self.dnd
            .source
            .is_some_and(|s| self.handlers.drops_outside(s))
    }

    /// Whether `pos` (window coordinates) is inside the window's content.
    fn contains(&self, pos: Point) -> bool {
        let size = self.attrs.logical_size;
        pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
    }

    /// A picture of the in-app drag's source element, for the drag image
    /// that follows the pointer outside the windows: the pixels, their size
    /// in logical pixels, and where in it the press went down.
    fn source_picture(&self, font: Option<&Font>) -> Option<(Arc<Pixmap>, Size, Point)> {
        let (pixmap, rect) = self.picture_of(self.dnd.source?, font)?;
        let size = rect.size;
        let grab = Point::new(
            (self.dnd.origin.x - rect.min_x()).clamp(0.0, size.width),
            (self.dnd.origin.y - rect.min_y()).clamp(0.0, size.height),
        );
        Some((pixmap, size, grab))
    }

    /// A picture of drag source `source` (at most 600 by 200 logical pixels,
    /// from its top-left corner), and where that is in the window.
    fn picture_of(&self, source: DragSourceId, font: Option<&Font>) -> Option<(Arc<Pixmap>, Rect)> {
        let tree = self.tree.as_ref()?;
        let (node, to_window, _) =
            crate::core::node_to_window(tree, &|n| n.drag_source == Some(source))?;
        let b = node.bounds;
        if b.width() < 1.0 || b.height() < 1.0 {
            return None;
        }
        let size = Size::new(b.width().min(600.0), b.height().min(200.0));
        let mut scene = Scene::new(size);
        scene.push_layer(
            crate::geometry::Affine::translate(crate::geometry::Vec2::new(-b.min_x(), -b.min_y())),
            1.0,
        );
        crate::core::paint(node, &mut scene, font);
        scene.pop_layer();
        let pixmap = crate::render::SoftwareRenderer::new().render(scene, self.scale);
        let top_left = to_window.apply(Point::new(b.min_x(), b.min_y()));
        Some((
            Arc::new(pixmap),
            Rect::from_xywh(top_left.x, top_left.y, size.width, size.height),
        ))
    }

    /// What a touch lifting at `pos` takes out of the app: the in-app drag
    /// in progress if its payload can leave the app, else the payload of the
    /// drag source under `pos` if it can; with the drag source.
    fn lift_payload(&mut self, state: &mut S, pos: Point) -> Option<(DropData, DragSourceId)> {
        if let Some(data) = self.dnd.internal.as_ref() {
            // A drag in progress keeps the finger: lifted, or nothing.
            return data
                .can_leave_app()
                .then(|| Some((data.clone(), self.dnd.source?)))
                .flatten();
        }
        let id = self.tree.as_ref().and_then(|t| drag_source_at(t, pos))?;
        let data = self
            .handlers
            .dispatch_drag_source(id, state)
            .filter(DropData::can_leave_app)?;
        Some((data, id))
    }

    /// Cancel an in-app drag in progress (Escape). Returns whether there was
    /// one.
    pub(crate) fn source_cancel(&mut self, state: &mut S) -> bool {
        self.dnd.press = None;
        self.dnd.source = None;
        if self.dnd.internal.take().is_some() {
            self.drag_leave(state);
            true
        } else {
            false
        }
    }
}

/// Where a window's content area is on the desktop (its top-left corner, in
/// logical pixels), from what the backend reports: the frame's corner plus
/// the frame's borders (the same on the left, right and bottom; the rest on
/// top, the title bar). `None` where window positions are unknown (Wayland).
pub(crate) fn content_origin(info: &WindowInfo) -> Option<Point> {
    let at = info.position?;
    let side = ((info.outer_size.width - info.inner_size.width) / 2.0).max(0.0);
    let top = (info.outer_size.height - info.inner_size.height - side).max(0.0);
    Some(Point::new(at.x + side, at.y + top))
}

/// Whether the platform can show a drag image of its own outside the app's
/// windows: a borderless window following the pointer. X11 and Windows (and
/// not Wayland, which keeps window positions from apps; macOS keeps a title
/// bar on every window).
const DRAG_IMAGE_WINDOW: bool = cfg!(any(
    target_os = "windows",
    all(
        unix,
        not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        ))
    )
));

/// An in-app drag that left the window it started in: the window it is over
/// now, and the drag image following the pointer outside every window.
#[derive(Debug, Default)]
pub(crate) struct CrossDrag {
    /// The other window (pane index) the drag is over, and the pointer in it.
    over: Option<(usize, Point)>,
    /// The drag image: the source's picture, its size, the grab point in it,
    /// and its window while it shows.
    image: Option<(Arc<Pixmap>, Size, Point)>,
    window: Option<WindowId>,
}

impl<S: 'static> App<S> {
    /// The window (pane index, other than `except` and the drag image's)
    /// whose content is under desktop point `screen`, and the point in it.
    fn window_at(&self, screen: Point, except: usize) -> Option<(usize, Point)> {
        let image = self.cross_drag.window;
        self.panes.iter().enumerate().find_map(|(j, pane)| {
            if j == except || Some(pane.id) == image || pane.info.minimized {
                return None;
            }
            let origin = content_origin(&pane.info)?;
            let local = Point::new(screen.x - origin.x, screen.y - origin.y);
            pane.contains(local).then_some((j, local))
        })
    }

    /// The in-app drag in window `idx` moved to `pos`: outside the window,
    /// carry it over the app's window there (its drop targets hover), or
    /// show the drag image when it's over none. Only for payloads that stay
    /// in the app (the others leave it: see [`pane_drag_out`](Self::pane_drag_out)).
    pub(crate) fn cross_drag_move(&mut self, idx: usize, pos: Point) {
        self.cross_drag_track(idx, pos, true);
    }

    /// [`cross_drag_move`](Self::cross_drag_move), showing the drag image
    /// outside the windows when `image` is set.
    fn cross_drag_track(&mut self, idx: usize, pos: Point, image: bool) {
        let Some(offer) = self.panes[idx]
            .dnd
            .internal
            .as_ref()
            .filter(|d| !d.can_leave_app())
            .map(DropData::kind)
        else {
            return;
        };
        let inside = self.panes[idx].contains(pos);
        let screen = (!inside)
            .then(|| content_origin(&self.panes[idx].info))
            .flatten()
            .map(|o| Point::new(o.x + pos.x, o.y + pos.y));
        let target = screen.and_then(|p| self.window_at(p, idx));
        let was = self.cross_drag.over.map(|(j, _)| j);
        if let Some(j) = was
            && target.map(|t| t.0) != Some(j)
        {
            self.pane_drag_leave(j);
            self.panes[j].dirty = true;
            crate::platform::wake::wake();
        }
        if let Some((j, local)) = target {
            if was == Some(j) {
                self.pane_drag_over(j, local);
            } else {
                self.pane_drag_enter(j, local, offer);
            }
            self.panes[j].dirty = true;
            crate::platform::wake::wake();
        }
        self.cross_drag.over = target;
        match screen {
            Some(at) if image && target.is_none() && self.panes[idx].source_drops_outside() => {
                self.show_drag_image(idx, at)
            }
            _ => self.hide_drag_image(),
        }
    }

    /// The button was released at `pos` during an in-app drag in window
    /// `idx`: drop it on the other window it's over, or tell its source it
    /// was let go outside every window. `None` when it's neither (a release
    /// in the window it started in, which drops as usual).
    pub(crate) fn cross_drag_release(&mut self, idx: usize, pos: Point) -> Option<bool> {
        let stays = self.panes[idx]
            .dnd
            .internal
            .as_ref()
            .is_some_and(|d| !d.can_leave_app());
        if !stays {
            return None;
        }
        self.cross_drag_track(idx, pos, false);
        self.cross_drag.image = None;
        if let Some((j, local)) = self.cross_drag.over.take() {
            let App { state, panes, .. } = self;
            let (data, _) = panes[idx].take_internal(state)?;
            crate::app_dbg(format_args!("drop in window {j} at {local:?}: {data:?}"));
            self.pane_drop(j, local, &data);
            self.panes[j].dirty = true;
            crate::platform::wake::wake();
            return Some(true);
        }
        if self.panes[idx].contains(pos) {
            return None;
        }
        let screen =
            content_origin(&self.panes[idx].info).map(|o| Point::new(o.x + pos.x, o.y + pos.y));
        let App { state, panes, .. } = self;
        let pane = &mut panes[idx];
        let (data, source) = pane.take_internal(state)?;
        crate::app_dbg(format_args!("dropped outside at {screen:?}: {data:?}"));
        if let Some(source) = source {
            pane.handlers
                .dispatch_drop_outside(source, &data, screen, state);
        }
        Some(true)
    }

    /// The in-app drag ended (cancelled): leave the other window it was over
    /// and close the drag image.
    pub(crate) fn cross_drag_end(&mut self) {
        if let Some((j, _)) = self.cross_drag.over.take() {
            self.pane_drag_leave(j);
            self.panes[j].dirty = true;
            crate::platform::wake::wake();
        }
        self.hide_drag_image();
        self.cross_drag.image = None;
    }

    /// Show the drag image (a picture of window `idx`'s drag source) with
    /// its grab point at desktop point `at`, opening its window or moving it.
    fn show_drag_image(&mut self, idx: usize, at: Point) {
        if !DRAG_IMAGE_WINDOW {
            return;
        }
        if self.cross_drag.image.is_none() {
            self.cross_drag.image = self.panes[idx].source_picture(self.font.get());
        }
        let Some((pixmap, size, grab)) = self.cross_drag.image.clone() else {
            return;
        };
        let corner = Point::new(at.x - grab.x, at.y - grab.y);
        let handle = self.panes[idx].handle.clone();
        match self.cross_drag.window {
            Some(id) => handle.window_to(id, crate::platform::WindowRequest::Position(corner)),
            None => {
                let attrs = WindowAttributes::new()
                    .title("")
                    .logical_size(size)
                    .position(corner.x.round() as i32, corner.y.round() as i32)
                    .decorations(false)
                    .resizable(false)
                    .focus(false);
                let id = handle.open_window(attrs, move |_, _| {
                    crate::core::Element::image(pixmap.clone())
                        .width(size.width)
                        .height(size.height)
                        .opacity(0.85)
                });
                self.cross_drag.window = Some(id);
            }
        }
    }

    fn hide_drag_image(&mut self) {
        if let Some(id) = self.cross_drag.window.take() {
            self.panes[0].handle.close_window(id);
        }
    }

    /// A drag from another application, carrying `offer`, entered the primary
    /// window at `pos`. Returns whether a drop target there accepts it. (The
    /// event loop calls this for [`Event::DragEnter`](crate::platform::Event::DragEnter);
    /// public for tests.)
    pub fn drag_enter(&mut self, pos: Point, offer: DragOffer) -> bool {
        self.pane_drag_enter(0, pos, offer)
    }

    /// The drag over the primary window moved to `pos`. Returns whether a drop
    /// target there accepts it.
    pub fn drag_over(&mut self, pos: Point) -> bool {
        self.pane_drag_over(0, pos)
    }

    /// The drag left the primary window without dropping.
    pub fn drag_leave(&mut self) {
        let App { state, panes, .. } = self;
        panes[0].drag_leave(state);
    }

    /// Drop `data` at `pos` in the primary window. Returns whether a drop
    /// target took it.
    pub fn drop_data(&mut self, pos: Point, data: &DropData) -> bool {
        self.pane_drop(0, pos, data)
    }

    /// The drop target an acceptable drag is hovering in the primary window.
    pub fn drop_target(&self) -> Option<DropId> {
        self.panes[0].dnd.target
    }

    /// Whether the primary window's next frame needs building (state, hover,
    /// focus or a drag changed, or something animates).
    pub fn needs_frame(&self) -> bool {
        self.panes[0].dirty || self.panes[0].animating
    }

    pub(crate) fn pane_drag_enter(&mut self, idx: usize, pos: Point, offer: DragOffer) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].drag_enter(state, theme, font.get(), pos, offer)
    }

    pub(crate) fn pane_drag_over(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].drag_over(state, theme, font.get(), pos)
    }

    pub(crate) fn pane_drag_leave(&mut self, idx: usize) {
        let App { state, panes, .. } = self;
        panes[idx].drag_leave(state);
    }

    pub(crate) fn pane_drop(&mut self, idx: usize, pos: Point, data: &DropData) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].drop_data(state, theme, font.get(), pos, data)
    }

    pub(crate) fn pane_source_move(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].source_move(state, theme, font.get(), pos)
    }

    pub(crate) fn pane_source_release(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].source_release(state, theme, font.get(), pos)
    }

    pub(crate) fn pane_source_cancel(&mut self, idx: usize) -> bool {
        let App { state, panes, .. } = self;
        panes[idx].source_cancel(state)
    }

    /// The pointer, dragging in window `idx`, is at `pos`: if an in-app drag
    /// of files or text has left the window, hand it to `window` as a drag
    /// other applications can take ([`Window::start_drag`]). Returns whether
    /// it did; where the platform can't, the drag stays in the app.
    pub(crate) fn pane_drag_out(&mut self, idx: usize, pos: Point, window: &dyn Window) -> bool {
        let Some(data) = self.panes[idx].leaving_payload(pos) else {
            return false;
        };
        if !window.start_drag(data) {
            return false;
        }
        crate::app_dbg(format_args!("drag out at {pos:?}: {data:?}"));
        let App { state, panes, .. } = self;
        panes[idx].handed_off(state);
        true
    }

    /// A touch lifts what is at `pos` in window `idx` (a long press, or the
    /// platform's own lift gesture asking with
    /// [`Event::DragLift`](crate::platform::Event::DragLift)): hand the drag
    /// source's payload there to `window` as a drag other applications can
    /// take ([`Window::lift_drag`]), when it can leave the app. Returns
    /// whether the platform took it; the press then no longer taps, and an
    /// in-app drag of it ends here.
    pub(crate) fn pane_lift(&mut self, idx: usize, pos: Point, window: &dyn Window) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        pane.ensure_tree(state, theme, font.get());
        let Some((data, source)) = pane.lift_payload(state, pos) else {
            return false;
        };
        let (image, rect) = match pane.picture_of(source, font.get()) {
            Some((pixmap, rect)) => (Some(pixmap), rect),
            None => (None, Rect::from_xywh(pos.x, pos.y, 0.0, 0.0)),
        };
        let lift = DragLift {
            data,
            source: rect,
            image,
        };
        if !window.lift_drag(&lift) {
            return false;
        }
        crate::app_dbg(format_args!("lifted at {pos:?}: {:?}", lift.data));
        pane.cancel_press();
        pane.handed_off(state);
        true
    }

    /// Press the left button at `pos` in the primary window as the event loop
    /// does (taps, drags, text selection and drag sources latch), for tests
    /// of in-app drags.
    pub fn pointer_press(&mut self, pos: Point) {
        self.pane_pointer_down(0, pos);
    }

    /// Move the pointer to `pos` in the primary window with the button down,
    /// as the event loop does. Returns whether anything changed.
    pub fn pointer_drag_to(&mut self, pos: Point) -> bool {
        self.pane_pointer_move(0, pos)
    }

    /// Release the left button at `pos` in the primary window, as the event
    /// loop does.
    pub fn pointer_release(&mut self, pos: Point) {
        self.pane_pointer_up(0, pos);
    }
}

#[cfg(test)]
mod tests {
    use crate::App;
    use crate::core::{BoxStyle, Cx, DropState, Element};
    use crate::geometry::{Point, Size};
    use crate::platform::{DragOffer, DropData};
    use crate::widgets::column;
    use std::cell::Cell;
    use std::path::PathBuf;

    #[derive(Default)]
    struct St {
        files: Vec<PathBuf>,
        moved: Vec<String>,
        hovering: bool,
        taps: u32,
        outside: Vec<(DropData, Option<Point>)>,
    }

    thread_local! {
        static SEEN: Cell<DropState> = Cell::new(DropState::default());
    }

    fn block() -> Element {
        Element::boxed(BoxStyle::default())
            .width(200.0)
            .height(40.0)
    }

    /// A draggable item on top (y 0..40) and a files-and-internal drop zone
    /// below it (y 40..80) whose view reads its drop state.
    fn app() -> App<St> {
        App::new(St::default(), |_: &St, cx: &mut Cx<St>| {
            let item = block()
                .on_tap(cx, |s| s.taps += 1)
                .drag_source(cx, |_| Some(DropData::Internal("item-1".into())));
            let accepts = DragOffer {
                files: true,
                text: false,
                internal: true,
            };
            let zone = cx.register_drop(accepts, |s, data, _| match data {
                DropData::Files(f) => s.files.extend(f.iter().cloned()),
                DropData::Internal(id) => s.moved.push(id.clone()),
                DropData::Text(_) => unreachable!("refused"),
            });
            cx.set_drop_hover(zone, |s, on| s.hovering = on);
            SEEN.with(|c| c.set(cx.drop_state(zone)));
            column(vec![item, block().drop_target(zone)])
        })
        .no_font()
        .logical_size(Size::new(200.0, 80.0))
    }

    #[test]
    fn os_drag_hovers_accepts_and_drops_files() {
        let mut app = app();
        let _ = app.render_frame();
        // Over the item: no drop target there.
        assert!(!app.drag_enter(Point::new(10.0, 10.0), DragOffer::FILES));
        assert_eq!(app.drop_target(), None);
        // Onto the zone: accepted, hovered, and the view sees it.
        assert!(app.drag_over(Point::new(30.0, 50.0)));
        assert!(app.state().hovering);
        assert!(app.needs_frame(), "entering rebuilds");
        let _ = app.render_frame();
        let seen = SEEN.with(Cell::get);
        assert!(seen.over && seen.accepted);
        assert_eq!(seen.position, Some(Point::new(30.0, 10.0)));
        // A move over a target whose view reads its state rebuilds too.
        assert!(app.drag_over(Point::new(35.0, 55.0)));
        assert!(app.needs_frame());
        let _ = app.render_frame();
        assert_eq!(SEEN.with(Cell::get).position, Some(Point::new(35.0, 15.0)));
        // The drop lands, ends the hover and rebuilds.
        let dropped = DropData::Files(vec![PathBuf::from("/tmp/a.txt")]);
        assert!(app.drop_data(Point::new(35.0, 55.0), &dropped));
        assert_eq!(app.state().files, vec![PathBuf::from("/tmp/a.txt")]);
        assert!(!app.state().hovering);
        assert!(app.needs_frame());
        let _ = app.render_frame();
        assert!(!SEEN.with(Cell::get).over);
    }

    #[test]
    fn refused_kind_is_over_but_not_accepted() {
        let mut app = app();
        let _ = app.render_frame();
        assert!(!app.drag_enter(Point::new(30.0, 50.0), DragOffer::TEXT));
        assert!(!app.state().hovering);
        let _ = app.render_frame();
        let seen = SEEN.with(Cell::get);
        assert!(seen.over && !seen.accepted, "{seen:?}");
        assert!(!app.drop_data(Point::new(30.0, 50.0), &DropData::Text("x".into())));
        app.drag_leave();
        let _ = app.render_frame();
        assert!(!SEEN.with(Cell::get).over);
    }

    #[test]
    fn in_app_drag_moves_an_item_without_tapping() {
        let mut app = app();
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        // Under the threshold: still a press.
        app.pointer_drag_to(Point::new(22.0, 21.0));
        assert_eq!(app.drop_target(), None);
        app.pointer_drag_to(Point::new(20.0, 30.0));
        app.pointer_drag_to(Point::new(20.0, 60.0));
        assert!(app.drop_target().is_some());
        assert!(app.state().hovering);
        app.pointer_release(Point::new(20.0, 60.0));
        assert_eq!(app.state().moved, vec!["item-1".to_string()]);
        assert_eq!(app.state().taps, 0, "a press that drags doesn't tap");
        assert!(!app.state().hovering);
        // A plain click still taps.
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_release(Point::new(20.0, 20.0));
        assert_eq!(app.state().taps, 1);
    }

    /// A window that records the drags handed to it, and takes them when
    /// `takes` is set.
    struct DragWindow {
        takes: bool,
        started: std::cell::RefCell<Vec<DropData>>,
    }

    impl crate::platform::Window for DragWindow {
        fn id(&self) -> crate::platform::WindowId {
            crate::platform::WindowId(1)
        }
        fn inner_size(&self) -> crate::geometry::PhysicalSize {
            crate::geometry::PhysicalSize::new(200, 80)
        }
        fn scale_factor(&self) -> crate::geometry::ScaleFactor {
            crate::geometry::ScaleFactor::IDENTITY
        }
        fn request_redraw(&self) {}
        fn set_title(&self, _title: &str) {}
        fn create_surface(&self) -> Box<dyn crate::render::Surface> {
            unreachable!()
        }
        fn start_drag(&self, data: &DropData) -> bool {
            self.started.borrow_mut().push(data.clone());
            self.takes
        }
    }

    /// One draggable item (y 0..40) carrying `payload`, over a zone taking
    /// everything (y 40..80).
    fn source_app(payload: DropData) -> App<St> {
        App::new(St::default(), move |_: &St, cx: &mut Cx<St>| {
            let payload = payload.clone();
            let item = block().drag_source(cx, move |_| Some(payload.clone()));
            let zone = cx.register_drop(DragOffer::ANY, |s, data, _| match data {
                DropData::Files(f) => s.files.extend(f.iter().cloned()),
                DropData::Text(t) | DropData::Internal(t) => s.moved.push(t.clone()),
            });
            cx.set_drop_hover(zone, |s, on| s.hovering = on);
            column(vec![item, block().drop_target(zone)])
        })
        .no_font()
        .logical_size(Size::new(200.0, 80.0))
    }

    #[test]
    fn files_dragged_out_of_the_window_go_to_the_platform() {
        let file = DropData::Files(vec![PathBuf::from("/tmp/out.txt")]);
        let mut app = source_app(file.clone());
        let _ = app.render_frame();
        let window = DragWindow {
            takes: true,
            started: Default::default(),
        };
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 50.0));
        assert!(app.state().hovering, "an in-app drag while inside");
        // Still inside: nothing for the platform.
        assert!(!app.pane_drag_out(0, Point::new(20.0, 50.0), &window));
        // Out past the right edge: the platform takes over.
        app.pointer_drag_to(Point::new(260.0, 50.0));
        assert!(app.pane_drag_out(0, Point::new(260.0, 50.0), &window));
        assert_eq!(*window.started.borrow(), vec![file]);
        assert!(!app.state().hovering, "targets stop hovering");
        assert!(!app.panes[0].dnd.dragging_internally());
        assert_eq!(app.panes[0].dnd.source, None, "no longer styled as dragged");
        // The release (if the platform passes it on) drops nothing in-app.
        app.pointer_release(Point::new(20.0, 50.0));
        assert!(app.state().files.is_empty());
    }

    /// A touch-screen window that records the lifts handed to it, and takes
    /// them when `takes` is set.
    struct LiftWindow {
        takes: bool,
        lifted: std::cell::RefCell<Vec<crate::platform::DragLift>>,
    }

    impl crate::platform::Window for LiftWindow {
        fn id(&self) -> crate::platform::WindowId {
            crate::platform::WindowId(1)
        }
        fn inner_size(&self) -> crate::geometry::PhysicalSize {
            crate::geometry::PhysicalSize::new(200, 80)
        }
        fn scale_factor(&self) -> crate::geometry::ScaleFactor {
            crate::geometry::ScaleFactor::IDENTITY
        }
        fn request_redraw(&self) {}
        fn set_title(&self, _title: &str) {}
        fn create_surface(&self) -> Box<dyn crate::render::Surface> {
            unreachable!()
        }
        fn lift_drag(&self, lift: &crate::platform::DragLift) -> bool {
            self.lifted.borrow_mut().push(lift.clone());
            self.takes
        }
    }

    fn lift_window(takes: bool) -> LiftWindow {
        LiftWindow {
            takes,
            lifted: Default::default(),
        }
    }

    fn finger(phase: crate::platform::TouchPhase, at: Point) -> crate::platform::Touch {
        crate::platform::Touch {
            id: 1,
            phase,
            position: at,
        }
    }

    #[test]
    fn a_long_press_lifts_files_out_through_the_platform() {
        use crate::platform::TouchPhase;
        let file = DropData::Files(vec![PathBuf::from("/tmp/lifted.txt")]);
        let mut app = source_app(file.clone());
        let _ = app.render_frame();
        let window = lift_window(true);
        app.touch(finger(TouchPhase::Started, Point::new(30.0, 10.0)));
        assert!(
            app.panes[0].long_press_timer.is_some(),
            "a drag source long-presses"
        );
        assert!(!app.pane_long_press(0, Some(&window)), "not yet");
        app.panes[0].touch_epoch -= std::time::Duration::from_secs_f64(0.6);
        assert!(app.pane_long_press(0, Some(&window)));
        let lifted = window.lifted.borrow();
        assert_eq!(lifted.len(), 1);
        assert_eq!(lifted[0].data, file);
        // The element's place and picture, for the platform's preview.
        assert_eq!(
            lifted[0].source,
            crate::geometry::Rect::from_xywh(0.0, 0.0, 200.0, 40.0)
        );
        let image = lifted[0].image.as_ref().expect("a picture");
        assert_eq!((image.size().width, image.size().height), (200, 40));
        drop(lifted);
        // The finger lifting afterwards neither drops nor taps in the app.
        app.touch(finger(TouchPhase::Moved, Point::new(30.0, 60.0)));
        app.touch(finger(TouchPhase::Ended, Point::new(30.0, 60.0)));
        assert!(app.state().files.is_empty());
        assert!(!app.panes[0].dnd.dragging_internally());
    }

    #[test]
    fn internal_payloads_and_refusing_platforms_dont_lift() {
        use crate::platform::TouchPhase;
        // An internal payload never leaves: the platform isn't asked.
        let mut app = source_app(DropData::Internal("item".into()));
        let _ = app.render_frame();
        let window = lift_window(true);
        app.touch(finger(TouchPhase::Started, Point::new(30.0, 10.0)));
        app.panes[0].touch_epoch -= std::time::Duration::from_secs_f64(0.6);
        assert!(app.pane_long_press(0, Some(&window)));
        assert!(window.lifted.borrow().is_empty());
        app.touch(finger(TouchPhase::Ended, Point::new(30.0, 10.0)));

        // Text on a platform that doesn't lift: asked, and nothing else.
        let mut app = source_app(DropData::Text("hi".into()));
        let _ = app.render_frame();
        let window = lift_window(false);
        assert!(!app.pane_lift(0, Point::new(30.0, 10.0), &window));
        assert_eq!(window.lifted.borrow().len(), 1);
        // Nowhere near a drag source: not asked.
        assert!(!app.pane_lift(0, Point::new(30.0, 60.0), &window));
        assert_eq!(window.lifted.borrow().len(), 1);
    }

    #[test]
    fn the_platforms_lift_takes_over_an_in_app_drag() {
        // iOS's lift asks (Event::DragLift) while the finger already drags
        // the text in the app: the platform takes it from there.
        let mut app = source_app(DropData::Text("quote".into()));
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 50.0));
        assert!(app.panes[0].dnd.dragging_internally());
        let window = lift_window(true);
        assert!(app.pane_lift(0, Point::new(20.0, 50.0), &window));
        assert_eq!(
            window.lifted.borrow()[0].data,
            DropData::Text("quote".into())
        );
        assert!(!app.panes[0].dnd.dragging_internally());
        assert!(!app.state().hovering);
        app.pointer_release(Point::new(20.0, 50.0));
        assert!(app.state().moved.is_empty(), "not dropped in the app");
    }

    #[test]
    fn internal_payloads_and_refusing_platforms_keep_the_drag_in_the_app() {
        let mut app = source_app(DropData::Internal("item".into()));
        let _ = app.render_frame();
        let window = DragWindow {
            takes: true,
            started: Default::default(),
        };
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, -30.0));
        assert!(!app.pane_drag_out(0, Point::new(20.0, -30.0), &window));
        assert!(window.started.borrow().is_empty(), "internal stays");
        app.pointer_release(Point::new(20.0, 60.0));
        assert_eq!(app.state().moved, vec!["item".to_string()]);

        // Text over a platform without drags out: still dropped in-app.
        let mut app = source_app(DropData::Text("hi".into()));
        let _ = app.render_frame();
        let window = DragWindow {
            takes: false,
            started: Default::default(),
        };
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(-5.0, 30.0));
        assert!(!app.pane_drag_out(0, Point::new(-5.0, 30.0), &window));
        assert_eq!(window.started.borrow().len(), 1, "asked");
        app.pointer_release(Point::new(20.0, 60.0));
        assert_eq!(app.state().moved, vec!["hi".to_string()]);
    }

    /// Two windows side by side, each one draggable item (y 0..40) over a
    /// zone taking everything (y 40..80), the first's item telling the state
    /// where it was dropped outside the windows: the primary window's content
    /// at (100, 100) on the desktop, the other's at (400, 100); `positions`
    /// off, where are unknown (Wayland).
    fn two_windows(positions: bool) -> App<St> {
        use crate::platform::WindowAttributes;
        let view = |_: &St, cx: &mut Cx<St>| {
            let item = block()
                .drag_source(cx, |_| Some(DropData::Internal("tab".into())))
                .on_drop_outside(cx, |s: &mut St, data, at| {
                    s.outside.push((data.clone(), at));
                });
            let zone = cx.register_drop(DragOffer::ANY, |s, data, at| {
                if let DropData::Internal(t) = data {
                    s.moved.push(format!("{t}@{},{}", at.x, at.y));
                }
            });
            cx.set_drop_hover(zone, |s, on| s.hovering = on);
            column(vec![item, block().drop_target(zone)])
        };
        let mut app = App::new(St::default(), view)
            .no_font()
            .logical_size(Size::new(200.0, 80.0));
        app.open_window(
            WindowAttributes::new().logical_size(Size::new(200.0, 80.0)),
            view,
        );
        let size = Size::new(200.0, 80.0);
        for (pane, x) in app.panes.iter_mut().zip([100.0, 400.0]) {
            pane.info.inner_size = size;
            // A 4 px border and a 24 px title bar around the content.
            pane.info.outer_size = Size::new(208.0, 108.0);
            pane.info.position = positions.then_some(Point::new(x - 4.0, 100.0 - 24.0));
        }
        app
    }

    #[test]
    fn content_sits_inside_the_frame() {
        let mut info = crate::platform::WindowInfo::from_size(
            crate::geometry::PhysicalSize::new(200, 80),
            crate::geometry::ScaleFactor::IDENTITY,
        );
        assert_eq!(super::content_origin(&info), None, "position unknown");
        info.position = Some(Point::new(10.0, 20.0));
        assert_eq!(super::content_origin(&info), Some(Point::new(10.0, 20.0)));
        info.outer_size = Size::new(208.0, 108.0);
        assert_eq!(super::content_origin(&info), Some(Point::new(14.0, 44.0)));
    }

    #[test]
    fn an_in_app_drag_goes_on_in_another_window() {
        let mut app = two_windows(true);
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 30.0));
        assert!(app.panes[0].dnd.dragging_internally());
        // Over the other window's zone: (320, 50) here is (420, 150) on the
        // desktop, (20, 50) there.
        app.pointer_drag_to(Point::new(320.0, 50.0));
        assert!(app.state().hovering, "the other window's zone hovers");
        assert!(app.panes[1].dnd.target.is_some());
        assert!(app.panes[1].dirty, "and shows it");
        assert_eq!(app.panes[0].dnd.target, None);
        // Between the windows: nothing hovers.
        app.pointer_drag_to(Point::new(250.0, 50.0));
        assert!(!app.state().hovering);
        assert_eq!(app.panes[1].dnd.target, None);
        // Back over it, and let go: it drops there, at the point in that window.
        app.pointer_drag_to(Point::new(330.0, 60.0));
        assert!(app.state().hovering);
        app.pointer_release(Point::new(330.0, 60.0));
        assert_eq!(app.state().moved, vec!["tab@30,60".to_string()]);
        assert!(app.state().outside.is_empty());
        assert!(!app.state().hovering);
        assert!(!app.panes[0].dnd.dragging_internally());
        assert_eq!(app.panes[0].dnd.source, None);
    }

    #[test]
    fn a_drag_let_go_outside_every_window_tells_its_source_where() {
        let mut app = two_windows(true);
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 30.0));
        app.pointer_drag_to(Point::new(50.0, 300.0));
        app.pointer_release(Point::new(50.0, 300.0));
        assert_eq!(
            app.state().outside,
            vec![(
                DropData::Internal("tab".into()),
                Some(Point::new(150.0, 400.0))
            )]
        );
        assert!(app.state().moved.is_empty());
        assert!(!app.panes[0].dnd.dragging_internally());

        // Where windows' positions are unknown, it's let go somewhere.
        let mut app = two_windows(false);
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 30.0));
        app.pointer_drag_to(Point::new(320.0, 50.0));
        assert!(!app.state().hovering, "no telling it's over the other");
        app.pointer_release(Point::new(320.0, 50.0));
        assert_eq!(
            app.state().outside,
            vec![(DropData::Internal("tab".into()), None)]
        );

        // Let go inside its own window, over nothing: nothing happens.
        let mut app = two_windows(true);
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 30.0));
        app.pointer_drag_to(Point::new(260.0, 30.0));
        app.pointer_drag_to(Point::new(150.0, 20.0));
        app.pointer_release(Point::new(150.0, 20.0));
        assert!(app.state().outside.is_empty() && app.state().moved.is_empty());
    }

    #[test]
    fn escape_ends_a_drag_over_another_window() {
        let mut app = two_windows(true);
        let _ = app.render_frame();
        app.pointer_press(Point::new(20.0, 20.0));
        app.pointer_drag_to(Point::new(20.0, 30.0));
        app.pointer_drag_to(Point::new(320.0, 50.0));
        assert!(app.state().hovering);
        app.pane_source_cancel(0);
        app.cross_drag_end();
        assert!(!app.state().hovering);
        assert_eq!(app.panes[1].dnd.target, None);
        app.pointer_release(Point::new(320.0, 50.0));
        assert!(app.state().moved.is_empty() && app.state().outside.is_empty());
    }

    #[test]
    fn an_unread_target_gets_the_app_highlight() {
        let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
            block().height(80.0).on_drop(cx, |_, _, _| {})
        })
        .no_font()
        .logical_size(Size::new(200.0, 80.0));
        let (before, _) = app.render_frame();
        assert!(app.drag_enter(Point::new(10.0, 10.0), DragOffer::TEXT));
        let (during, damage) = app.render_frame();
        assert_eq!(damage, crate::core::Damage::Full);
        assert_ne!(before.as_bytes(), during.as_bytes(), "highlighted");
        app.drag_leave();
        // It fades out with the target's drag-over level, then it's gone.
        let _ = app.render_frame();
        assert!(app.animating(), "fading out");
        std::thread::sleep(crate::core::DROP_FADE * 2);
        let (after, _) = app.render_frame();
        assert_eq!(before.as_bytes(), after.as_bytes(), "highlight gone");
    }
}
