//! Drag and drop in the [`App`]: drags from other applications
//! ([`Event::DragEnter`](crate::platform::Event::DragEnter) and friends) and
//! drags started inside the app on an
//! [`Element::drag_source`](crate::core::Element::drag_source), both routed to
//! the [`Element::on_drop`](crate::core::Element::on_drop) target under the
//! pointer. See `docs/drag-and-drop.md`.

use crate::core::{DragSourceId, DropId, Font, LayoutNode, drag_source_at, drop_at, find_drop};
use crate::geometry::{Point, Rect};
use crate::platform::{DragOffer, DropData};
use crate::render::{Color, Scene};
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
    /// The target the app highlighted in the frame just built, and in the one
    /// on screen (for damage).
    pub(crate) highlight: Option<DropId>,
    pub(crate) painted: Option<DropId>,
    /// The drop targets' easing drag-over levels.
    pub(crate) fades: crate::core::DropFades,
    /// The drop targets whose view read their state in the last build.
    pub(crate) queried: Vec<DropId>,
    /// A press on a drag source, not yet moved far enough to drag.
    press: Option<(DragSourceId, Point)>,
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
        self.text_selecting.is_some() || self.dragging.is_some() || self.dnd.dragging_internally()
    }
}

/// Paint the hover highlight of drop target `id`: the theme's hover overlay
/// with a focus-colored border, so the target reads as "drop here".
pub(crate) fn paint_drop_target(tree: &LayoutNode, id: DropId, scene: &mut Scene, theme: &Theme) {
    let Some(node) = find_drop(tree, id) else {
        return;
    };
    // Drawn where the target is drawn, if it's transformed.
    let to_window = crate::core::node_to_window(tree, &|n| n.drop == Some(id))
        .map_or(crate::geometry::Affine::IDENTITY, |(_, m, _)| m);
    let layer = !to_window.is_identity();
    if layer {
        scene.push_layer(to_window, 1.0);
    }
    paint_drop_target_at(node, scene, theme);
    if layer {
        scene.pop_layer();
    }
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

impl<S: 'static> App<S> {
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
        panes[idx].drag_enter(state, theme, font.as_ref(), pos, offer)
    }

    pub(crate) fn pane_drag_over(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].drag_over(state, theme, font.as_ref(), pos)
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
        panes[idx].drop_data(state, theme, font.as_ref(), pos, data)
    }

    pub(crate) fn pane_source_move(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].source_move(state, theme, font.as_ref(), pos)
    }

    pub(crate) fn pane_source_release(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].source_release(state, theme, font.as_ref(), pos)
    }

    pub(crate) fn pane_source_cancel(&mut self, idx: usize) -> bool {
        let App { state, panes, .. } = self;
        panes[idx].source_cancel(state)
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
                .on_tap(cx, |s: &mut St| s.taps += 1)
                .drag_source(cx, |_| Some(DropData::Internal("item-1".into())));
            let accepts = DragOffer {
                files: true,
                text: false,
                internal: true,
            };
            let zone = cx.register_drop(accepts, |s: &mut St, data, _| match data {
                DropData::Files(f) => s.files.extend(f.iter().cloned()),
                DropData::Internal(id) => s.moved.push(id.clone()),
                DropData::Text(_) => unreachable!("refused"),
            });
            cx.set_drop_hover(zone, |s: &mut St, on| s.hovering = on);
            SEEN.with(|c| c.set(cx.drop_state(zone)));
            column(vec![item, block().drop_target(zone)])
        })
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

    #[test]
    fn an_unread_target_gets_the_app_highlight() {
        let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
            block().height(80.0).on_drop(cx, |_, _, _| {})
        })
        .logical_size(Size::new(200.0, 80.0));
        let (before, _) = app.render_frame();
        assert!(app.drag_enter(Point::new(10.0, 10.0), DragOffer::TEXT));
        let (during, damage) = app.render_frame();
        assert_eq!(damage, crate::core::Damage::Full);
        assert_ne!(before.as_bytes(), during.as_bytes(), "highlighted");
        app.drag_leave();
        let (after, _) = app.render_frame();
        assert_eq!(before.as_bytes(), after.as_bytes(), "highlight gone");
    }
}
