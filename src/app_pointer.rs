//! Raw pointer and wheel events in the [`App`]: every button, move and wheel
//! scroll reaches the [`Element::on_pointer`](crate::core::Element::on_pointer)
//! and [`Element::on_wheel`](crate::core::Element::on_wheel) handler under
//! the pointer (or the one holding the capture) ahead of the app's own
//! handling, with click counts, the buttons held and the modifiers; the
//! pointer leaving the window clears every hover. See
//! `docs/pointer-events.md`.

use crate::core::{
    PointerButtons, PointerEvent, PointerEventKind, PointerId, WheelEvent, WheelTarget, pointer_at,
    pointer_local, wheel_target_at,
};
use crate::geometry::Point;
use crate::platform::time::Instant;
use crate::platform::{ButtonState, Event, PointerButton, ScrollDelta};
use crate::{App, ClickCounter, Pane, press_layer};

/// The pointer as the pointer handlers of one window see it.
#[derive(Debug, Default)]
pub(crate) struct PointerTrack {
    /// The buttons held.
    buttons: PointerButtons,
    /// The element a press went down on, which gets the moves and releases
    /// until the last button is up.
    capture: Option<PointerId>,
    /// The element the pointer is over, to tell it when it leaves.
    hover: Option<PointerId>,
    /// Consecutive presses of `last_button`, and the last one's count (for its
    /// release).
    clicks: ClickCounter,
    last_button: Option<PointerButton>,
    count: u32,
}

impl PointerTrack {
    /// Forget the buttons held and the capture: their releases won't come
    /// here (a drag handed to the system took the pointer).
    pub(crate) fn release_all(&mut self) {
        self.buttons = PointerButtons::NONE;
        self.capture = None;
    }
}

/// What [`App::pane_pointer_input`] did with an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PointerInput {
    /// A handler ran, or a hover changed: repaint.
    pub(crate) redraw: bool,
    /// Nothing else handles the event (the wheel went to a handler, or a
    /// button the app has no use for of its own).
    pub(crate) done: bool,
}

impl<S: 'static> Pane<S> {
    /// The element with a pointer handler under `pos`, in the layer a press
    /// there reaches.
    fn pointer_target(&self, pos: Point) -> Option<PointerId> {
        let tree = self.tree.as_ref()?;
        let layer = press_layer(tree, self.overlay_active, pos).0;
        let pointers = &self.handlers;
        pointer_at(layer, pos, &|id| pointers.pointers_ref().has_pointer(id))
    }

    /// Run `id`'s pointer handler with an event at `pos`. Returns whether it
    /// ran.
    #[allow(clippy::too_many_arguments)]
    fn send_pointer(
        &mut self,
        state: &mut S,
        id: PointerId,
        kind: PointerEventKind,
        button: Option<PointerButton>,
        pos: Point,
        click_count: u32,
    ) -> bool {
        let Some((position, size)) = self.tree.as_ref().and_then(|t| pointer_local(t, id, pos))
        else {
            return false;
        };
        let ev = PointerEvent {
            kind,
            button,
            buttons_held: self.pointer.buttons,
            position,
            window_position: pos,
            size,
            modifiers: crate::platform::pointer_modifiers(),
            click_count,
        };
        crate::app_dbg(format_args!("pointer {ev:?} to {id:?}"));
        let ran = self.handlers.pointers().dispatch_pointer(id, &ev, state);
        if ran {
            self.dirty = true;
        }
        ran
    }

    /// Tell the element the pointer was over that it left. Returns whether a
    /// handler ran.
    fn pointer_leave(&mut self, state: &mut S, pos: Point) -> bool {
        match self.pointer.hover.take() {
            Some(id) => self.send_pointer(state, id, PointerEventKind::Leave, None, pos, 0),
            None => false,
        }
    }

    /// The pointer moved to `pos` with no capture: leave the element it was
    /// over if it's on another now, and move over the one under it.
    fn pointer_hover(&mut self, state: &mut S, pos: Point) -> bool {
        let under = self.pointer_target(pos);
        let mut ran = false;
        if under != self.pointer.hover {
            ran |= self.pointer_leave(state, pos);
            self.pointer.hover = under;
        }
        if let Some(id) = under {
            ran |= self.send_pointer(state, id, PointerEventKind::Move, None, pos, 0);
        }
        ran
    }

    /// Whether `pos` is outside the window.
    fn outside_window(&self, pos: Point) -> bool {
        let size = self.attrs.logical_size;
        pos.x < 0.0 || pos.y < 0.0 || pos.x >= size.width || pos.y >= size.height
    }
}

impl<S: 'static> App<S> {
    /// Deliver a pointer button, move or wheel `event` to the pointer and
    /// wheel handlers (ahead of the app's own handling, which still follows
    /// unless [`PointerInput::done`]).
    pub(crate) fn pane_pointer_input(
        &mut self,
        idx: usize,
        event: &Event,
        now: Instant,
    ) -> PointerInput {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        let mut out = PointerInput::default();
        match *event {
            Event::PointerButton {
                button,
                state: ButtonState::Pressed,
                position,
            } => {
                pane.ensure_tree(state, theme, font.as_ref());
                pane.pointer_in = true;
                pane.last_pointer = position;
                let track = &mut pane.pointer;
                if track.buttons.contains(button) {
                    // Its release never came (the window lost the pointer):
                    // start afresh.
                    track.release_all();
                }
                track.buttons = track.buttons.with(button);
                if track.last_button != Some(button) {
                    track.clicks.reset();
                }
                track.last_button = Some(button);
                track.count = track.clicks.press(now, position);
                let count = track.count;
                let target = pane
                    .pointer
                    .capture
                    .or_else(|| pane.pointer_target(position));
                if let Some(id) = target {
                    pane.pointer.capture = Some(id);
                    pane.pointer.hover = Some(id);
                    out.redraw = pane.send_pointer(
                        state,
                        id,
                        PointerEventKind::Down,
                        Some(button),
                        position,
                        count,
                    );
                }
                // The app presses with the left button and opens context
                // menus with the right one; the middle one pastes the primary
                // selection into editable text, unless an `on_pointer`
                // element took it. Nothing else.
                out.done = match button {
                    PointerButton::Left | PointerButton::Right => false,
                    PointerButton::Middle => target.is_some(),
                    _ => true,
                };
            }
            Event::PointerButton {
                button,
                state: ButtonState::Released,
                position,
            } => {
                pane.ensure_tree(state, theme, font.as_ref());
                pane.last_pointer = position;
                pane.pointer.buttons = pane.pointer.buttons.without(button);
                let count = if pane.pointer.last_button == Some(button) {
                    pane.pointer.count
                } else {
                    1
                };
                let target = pane
                    .pointer
                    .capture
                    .or_else(|| pane.pointer_target(position));
                if let Some(id) = target {
                    out.redraw |= pane.send_pointer(
                        state,
                        id,
                        PointerEventKind::Up,
                        Some(button),
                        position,
                        count,
                    );
                }
                if pane.pointer.buttons.is_empty() {
                    pane.pointer.capture = None;
                    if pane.outside_window(position) {
                        // Released outside the window: the pointer left it
                        // while the press held it.
                        out.redraw |= pane.pointer_leave(state, position);
                        out.redraw |= pane.hovered.take().is_some();
                        pane.pointer_in = false;
                    } else if pane.pointer_target(position) != pane.pointer.hover {
                        // Released over something else: that's where it is.
                        out.redraw |= pane.pointer_hover(state, position);
                    }
                }
                out.done = button != PointerButton::Left;
            }
            Event::PointerMoved { position } => {
                pane.ensure_tree(state, theme, font.as_ref());
                match pane.pointer.capture {
                    Some(id) => {
                        out.redraw =
                            pane.send_pointer(state, id, PointerEventKind::Move, None, position, 0);
                    }
                    None => out.redraw = pane.pointer_hover(state, position),
                }
            }
            Event::Scroll { delta } => {
                pane.ensure_tree(state, theme, font.as_ref());
                let at = pane.last_pointer;
                let target = pane.tree.as_ref().and_then(|t| {
                    let layer = press_layer(t, pane.overlay_active, at).0;
                    let pointers = &pane.handlers;
                    wheel_target_at(layer, at, &|id| pointers.pointers_ref().has_wheel(id))
                });
                if let Some(WheelTarget::Handler(id)) = target {
                    out.redraw = pane.send_wheel(state, id, delta, at);
                    out.done = true;
                }
            }
            _ => {}
        }
        out
    }

    /// The pointer left window `idx`: nothing is hovered any more, unless a
    /// press holds the pointer (its release outside the window ends the
    /// hover then). Returns whether to repaint.
    pub(crate) fn pane_pointer_left(&mut self, idx: usize) -> bool {
        let App { state, panes, .. } = self;
        let pane = &mut panes[idx];
        if pane.pointer_latched() || pane.press_at.is_some() || !pane.pointer.buttons.is_empty() {
            return false;
        }
        let at = pane.last_pointer;
        let mut redraw = pane.pointer_leave(state, at);
        // The theme's hover highlight, and `.hovered` looks.
        redraw |= pane.hovered.take().is_some();
        pane.pointer_in = false;
        redraw | pane.refresh_interactions()
    }
}

impl<S: 'static> Pane<S> {
    /// Run `id`'s wheel handler with `delta`, the pointer at `pos`.
    fn send_wheel(&mut self, state: &mut S, id: PointerId, delta: ScrollDelta, pos: Point) -> bool {
        let Some((position, _)) = self.tree.as_ref().and_then(|t| pointer_local(t, id, pos)) else {
            return false;
        };
        let ev = WheelEvent {
            dx: delta.dx,
            dy: delta.dy,
            unit: delta.unit,
            phase: delta.phase,
            position,
            window_position: pos,
            modifiers: crate::platform::pointer_modifiers(),
        };
        crate::app_dbg(format_args!("wheel {ev:?} to {id:?}"));
        let ran = self.handlers.pointers().dispatch_wheel(id, &ev, state);
        if ran {
            self.dirty = true;
        }
        ran
    }
}

/// App-level helpers for tests and headless drivers.
impl<S: 'static> App<S> {
    /// Feed the primary window a pointer `event` (a button, move, leave or
    /// scroll) as the run loop would, minus repainting. Returns whether the
    /// window needs a repaint.
    pub fn pointer_event(&mut self, event: Event) -> bool {
        let now = Instant::now();
        let input = self.pane_pointer_input(0, &event, now);
        if input.done {
            return input.redraw;
        }
        input.redraw
            | match event {
                Event::PointerButton {
                    button: PointerButton::Left,
                    state: ButtonState::Pressed,
                    position,
                } => self.pane_pointer_down(0, position),
                Event::PointerButton {
                    button: PointerButton::Left,
                    state: ButtonState::Released,
                    position,
                } => self.pane_pointer_up(0, position),
                Event::PointerButton {
                    button: PointerButton::Right,
                    state: ButtonState::Pressed,
                    position,
                } => self.pane_context_at(0, position),
                Event::PointerMoved { position } => {
                    self.panes[0].last_pointer = position;
                    self.pane_pointer_move(0, position)
                }
                Event::PointerLeft => self.pane_pointer_left(0),
                Event::Scroll { delta } => self.pane_scroll_by(0, delta.in_pixels().1),
                _ => false,
            }
    }
}
