//! The reactive runtime: interaction context, retained layout tree, and event
//! dispatch.
//!
//! Turns the static [`Element`](crate::core::Element) IR into an interactive UI. A
//! [`Cx`] is threaded through the view-building closure so widgets can register
//! `on_tap` (pointer) and `on_key` (keyboard/focus) handlers; building yields
//! an [`Element`] tree plus a parallel [`Handlers`] table. Laying the tree out
//! produces a retained [`LayoutNode`] tree that [`hit_test`] (pointer) and
//! [`focus_at`] / [`collect_focusables`] (keyboard focus) query to route events
//! back to the registered handlers.
//!
//! Handlers stay out of the [`Element`] IR (they live in the `Cx` tables,
//! addressed by [`ActionId`] / [`FocusId`]) so `Element` stays `Clone`/`Debug`
//! and the IR remains diff-friendly for a future reconciler.

use crate::core::a11y::Semantics;
use crate::core::element::{Align, BoxStyle, Element};
use crate::core::handle::AppHandle;
use crate::geometry::{Affine, Point, Rect, Size};
use crate::platform::{CursorIcon, DragOffer, DropData};
use crate::render::Color;
use crate::style::Theme;
use std::collections::{HashMap, HashSet};

/// A boxed pointer-tap handler that mutates the app state `S`.
type TapFn<S> = Box<dyn FnMut(&mut S)>;
/// A boxed keyboard handler: receives the [`KeyInput`] for the focused element.
type KeyFn<S> = Box<dyn FnMut(&mut S, &KeyInput)>;
/// A boxed raw key handler (see
/// [`Element::on_raw_key`](crate::core::Element::on_raw_key)).
type RawKeyFn<S> = Box<dyn FnMut(&mut S, &crate::platform::KeyEvent) -> crate::core::keys::Handled>;
/// A boxed drag handler: receives the [`DragMotion`] (fraction handlers
/// registered with [`Cx::register_drag`] read [`DragMotion::fraction_x`]).
type DragFn<S> = Box<dyn FnMut(&mut S, &DragMotion)>;

/// A pointer drag in progress, delivered to handlers registered with
/// [`Element::on_drag_motion`](crate::core::Element::on_drag_motion) on the
/// press and on every move until release. Positions are window logical pixels;
/// both rectangles are as laid out when the press landed, so they don't move
/// under the pointer while the drag reshapes the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragMotion {
    /// Where the pointer is now.
    pub position: Point,
    /// Where the press landed.
    pub origin: Point,
    /// The dragged element's bounds at the press.
    pub bounds: Rect,
    /// The bounds of the dragged element's parent at the press (the track a
    /// divider or thumb moves along); the element's own bounds at the root.
    pub parent: Rect,
}

impl DragMotion {
    /// The pointer's x position as a fraction (0..=1) across the element's
    /// press-time bounds: what [`Element::on_drag`](crate::core::Element::on_drag)
    /// handlers receive.
    pub fn fraction_x(&self) -> f64 {
        if self.bounds.width() > 0.0 {
            ((self.position.x - self.bounds.min_x()) / self.bounds.width()).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// How far the pointer has moved since the press.
    pub fn delta(&self) -> (f64, f64) {
        (
            self.position.x - self.origin.x,
            self.position.y - self.origin.y,
        )
    }

    /// This motion reflected left to right about the middle of
    /// [`parent`](Self::parent): every x measured from the other side. The
    /// `App` hands this to the handlers of an element laid out right to left,
    /// so a handler written for a left-to-right layout works in a mirrored
    /// one unchanged: [`fraction_x`](Self::fraction_x) counts from the right
    /// edge, and dragging left is a positive x [`delta`](Self::delta).
    pub fn mirrored(&self) -> DragMotion {
        let axis = self.parent.min_x() + self.parent.max_x();
        let point = |p: Point| Point::new(axis - p.x, p.y);
        let rect = |r: Rect| Rect::from_xywh(axis - r.max_x(), r.min_y(), r.width(), r.height());
        DragMotion {
            position: point(self.position),
            origin: point(self.origin),
            bounds: rect(self.bounds),
            parent: rect(self.parent),
        }
    }
}
/// A boxed text-pointer handler: receives a resolved byte index into the
/// element's text and whether the gesture *extends* a selection (drag) or
/// *places* the caret (initial press).
type TextPosFn<S> = Box<dyn FnMut(&mut S, usize, bool)>;
/// A boxed secondary-click (context) handler: receives the click position in
/// logical pixels, so it can open a context menu there.
type ContextFn<S> = Box<dyn FnMut(&mut S, Point)>;
/// A boxed drop handler: receives the dropped data and the drop position.
type DropFn<S> = Box<dyn FnMut(&mut S, &DropData, Point)>;
/// A boxed drag-hover handler: told when a drag starts (`true`) and stops
/// (`false`) hovering its drop target.
type DropHoverFn<S> = Box<dyn FnMut(&mut S, bool)>;
/// A boxed drag-source handler: returns the payload of an in-app drag starting
/// on its element (`None` declines the drag).
type DragSourceFn<S> = Box<dyn FnMut(&mut S) -> Option<DropData>>;

/// A registered drop target: what it accepts and its handlers.
struct DropTarget<S> {
    accepts: DragOffer,
    on_drop: DropFn<S>,
    on_hover: Option<DropHoverFn<S>>,
}

/// An opaque handle to a registered tap handler, stamped onto the element that
/// owns it and resolved against the [`Cx`] tap table on dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActionId(pub(crate) u32);

/// An opaque handle to a focusable element with a registered key handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FocusId(pub(crate) u32);

/// An opaque handle to an element with a registered drag handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DragId(pub(crate) u32);

/// An opaque handle to an editable text element with a registered text-pointer
/// handler (click-to-position / drag-to-select).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextPosId(pub(crate) u32);

/// An opaque handle to an element with a registered secondary-click (context)
/// handler, resolved against the [`Cx`] context table on a right-click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContextId(pub(crate) u32);

/// An opaque handle to an element registered as a drop target (see
/// [`Element::on_drop`](crate::core::Element::on_drop)), resolved against the
/// [`Cx`] drop table while a drag hovers it and when it is dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropId(pub(crate) u32);

/// An opaque handle to an element that starts in-app drags (see
/// [`Element::drag_source`](crate::core::Element::drag_source)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DragSourceId(pub(crate) u32);

/// An opaque handle to a scroll container. The app keeps a scroll offset per id
/// (adjusted by wheel events) and re-applies it each frame; the id is stable as
/// long as the view registers scroll containers in the same order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScrollId(pub(crate) u32);

/// A cloneable handle for asking the app to move scroll containers, from an
/// event handler (which only sees the state). Get one from
/// [`Cx::scroll_requests`]; requests apply before the next frame is built, so
/// [`Cx::scroll_offset`] reads them back. Used by the virtualized list to keep
/// the keyboard cursor in view.
#[derive(Clone, Debug, Default)]
pub struct ScrollRequests(std::rc::Rc<std::cell::RefCell<Vec<(ScrollId, f64)>>>);

impl ScrollRequests {
    /// Ask for container `id` to scroll to `offset` logical pixels from the top
    /// (clamped to its content when applied).
    pub fn scroll_to(&self, id: ScrollId, offset: f64) {
        self.0.borrow_mut().push((id, offset));
    }

    /// Take the pending requests, oldest first.
    pub fn take(&self) -> Vec<(ScrollId, f64)> {
        std::mem::take(&mut self.0.borrow_mut())
    }
}

/// A **caller-chosen** handle to an embedded-content viewport — a rectangle the
/// app fills with externally-rendered pixels (a browser page, video frame, or a
/// sandboxed content process's GPU surface). Unlike the auto-registered handler
/// ids, the value is chosen by the app so it stays stable across frames and can
/// be correlated with the content source that feeds it (see
/// [`Element::viewport`](crate::core::Element::viewport)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewportId(pub u32);

/// Where an [`OverlaySpec`] is positioned within the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Place the overlay's top-left at an absolute window point (e.g. a menu
    /// dropped below its button).
    At(Point),
    /// Center the overlay in the window (e.g. a modal dialog).
    Center,
    /// Drop the overlay just below the element carrying this tap action (a
    /// select's option list under its button), at least as wide as it. It
    /// flips above when there's no room below, and is kept inside the window.
    /// Falls back to the window's top-left when no such element is laid out.
    Below(ActionId),
    /// Place the overlay's top-left at a window point (a context menu at the
    /// pointer), moved left or up as needed to keep it inside the window.
    Popup(Point),
    /// Place the overlay to the right of the element carrying this tap action,
    /// top-aligned with it (a submenu beside its item), or to its left when
    /// there's no room on the right. Kept inside the window. Falls back to the
    /// window's top-left when no such element is laid out (in the main tree or
    /// an earlier overlay).
    RightOf(ActionId),
    /// Place the overlay against the window's edges: `horizontal` and
    /// `vertical` pick start, center or end (`Stretch` counts as start), inset
    /// by `margin` logical pixels (e.g. toasts in the bottom-right corner).
    Aligned {
        horizontal: Align,
        vertical: Align,
        margin: f64,
    },
}

/// A floating layer drawn above the main tree — a menu, popover, tooltip, or
/// dialog. Declared during a build via [`Cx::overlay`]; the app lays it out at
/// its [`Anchor`] and paints it last (topmost). Its `content`'s handlers
/// register through the same [`Cx`], so taps/keys inside it work normally.
#[derive(Clone, Debug)]
pub struct OverlaySpec {
    pub content: Element,
    pub anchor: Anchor,
    /// When `true`, a translucent scrim is painted behind the overlay and blocks
    /// pointer events from reaching the main tree (a modal dialog).
    pub modal: bool,
    /// Action fired when the scrim (modal) or the area outside the overlay
    /// (non-modal) is pressed — typically a dismiss handler.
    pub dismiss: Option<ActionId>,
}

/// A platform-neutral keyboard input, delivered to the focused element. The
/// app/platform layer translates raw key events into these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyInput {
    /// Committed text (one or more characters), e.g. from a key press or IME.
    Text(String),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Caret motion that *extends the selection* (Shift held). The `Select*`
    /// variants mirror the plain motions but keep the selection anchor.
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectHome,
    SelectEnd,
    /// Select everything (e.g. Ctrl/Cmd+A).
    SelectAll,
    /// Copy the selection to the clipboard (Ctrl/Cmd+C).
    Copy,
    /// Cut the selection to the clipboard (Ctrl/Cmd+X).
    Cut,
    /// Paste the clipboard at the caret, replacing the selection (Ctrl/Cmd+V).
    Paste,
    /// Move the caret to the previous word start (Ctrl+Left; Alt+Left on macOS).
    WordLeft,
    /// Move the caret to the next word end (Ctrl+Right; Alt+Right on macOS).
    WordRight,
    /// Extend the selection to the previous word start (with Shift).
    SelectWordLeft,
    /// Extend the selection to the next word end (with Shift).
    SelectWordRight,
    /// Delete the selection, or back to the previous word start
    /// (Ctrl+Backspace; Alt+Backspace on macOS).
    DeleteWordBackward,
    /// Delete the selection, or forward to the next word end
    /// (Ctrl+Delete; Alt+Delete on macOS).
    DeleteWordForward,
    /// Select the word (or run of punctuation / spaces) at the caret, as a
    /// double-click does.
    SelectWord,
    /// Select the line containing the caret, as a triple-click does. Single-line
    /// fields treat it as [`SelectAll`](KeyInput::SelectAll).
    SelectLine,
    /// Undo the last edit group (Ctrl/Cmd+Z).
    Undo,
    /// Redo the last undone edit group (Ctrl+Y, Ctrl/Cmd+Shift+Z).
    Redo,
    Enter,
    Escape,
    /// Input-method composition (preedit) text to show at the caret, not yet
    /// part of the text; `cursor` is the byte range within it the IME
    /// highlights. Empty `text` ends the composition. A following
    /// [`Text`](KeyInput::Text) commits the final text in its place.
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    /// Delete `before` bytes before the caret and `after` bytes after it, as an
    /// input method asks (Wayland's `delete_surrounding_text`).
    DeleteSurrounding {
        before: usize,
        after: usize,
    },
}

/// A platform-neutral input event forwarded to an embedded
/// [`viewport`](crate::core::Element::viewport)'s content — a sandboxed browser/content
/// process that renders into the viewport. Pointer positions are in
/// **viewport-local** logical pixels (origin at the viewport's top-left), so the
/// content can route them without knowing where the viewport sits in the window.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewportEvent {
    /// Pointer pressed at `local`. `button`: 0 = primary/left, 1 = secondary/
    /// right, 2 = middle.
    PointerDown { local: Point, button: u8 },
    /// Pointer released at `local` (same `button` encoding as [`PointerDown`]).
    ///
    /// [`PointerDown`]: ViewportEvent::PointerDown
    PointerUp { local: Point, button: u8 },
    /// Pointer moved to `local` while over the viewport.
    PointerMove { local: Point },
    /// Wheel scrolled by `delta_y` logical pixels with the pointer at `local`.
    Wheel { local: Point, delta_y: f64 },
    /// Keyboard input delivered while this viewport held input focus (acquired
    /// when the content was last pressed).
    Key(KeyInput),
}

/// Build context threaded through a view closure.
///
/// Carries the active [`Theme`] and accumulates event handlers. Registering a
/// handler returns an id the caller stamps onto an element (see
/// [`Element::on_tap`](crate::core::Element::on_tap) /
/// [`Element::on_key`](crate::core::Element::on_key)).
pub struct Cx<'a, S> {
    theme: &'a Theme,
    taps: Vec<TapFn<S>>,
    keys: Vec<KeyFn<S>>,
    raw_keys: Vec<RawKeyFn<S>>,
    drags: Vec<DragFn<S>>,
    text_pos: Vec<TextPosFn<S>>,
    contexts: Vec<ContextFn<S>>,
    drops: Vec<DropTarget<S>>,
    drag_sources: Vec<DragSourceFn<S>>,
    pointers: crate::core::PointerHandlers<S>,
    /// The drag-over state of drop targets as of this build (seeded by the
    /// app), and the targets whose view read theirs.
    drop_states: HashMap<DropId, crate::core::dnd::DropState>,
    drop_queried: Vec<DropId>,
    /// Keyed elements' interaction states as of this build (seeded by the
    /// app), and the keys the view read.
    interactions: HashMap<crate::core::ElementKey, crate::core::Interaction>,
    interaction_queried: Vec<crate::core::ElementKey>,
    /// Next scroll-container id to hand out (scroll offsets live in the app, not
    /// here, so we only need a stable per-frame counter).
    next_scroll: u32,
    /// Floating layers (menus/dialogs/…) declared this frame via [`Cx::overlay`].
    overlays: Vec<OverlaySpec>,
    /// Context menus attached this frame with
    /// [`Element::context_menu`](crate::core::Element::context_menu).
    context_menus: Vec<(ContextId, crate::core::menu::Menu<S>)>,
    /// Cross-frame cache for [`Cx::memo`]: cached subtrees by key, plus the keys
    /// touched this frame (so stale entries can be evicted afterward).
    memo: HashMap<u64, Element>,
    memo_used: HashSet<u64>,
    /// Scroll offsets as of this build (seeded by the app), and the queue
    /// handlers push scroll requests onto.
    scroll_offsets: HashMap<ScrollId, f64>,
    scroll_requests: ScrollRequests,
    /// The running app's handle, for timers and background work.
    handle: AppHandle<S>,
    /// The frame's time on the animation timeline, and whether the view asked
    /// for another frame. See [`Cx::frame_time`] and [`Cx::request_frame`].
    frame: FrameInfo,
    /// The window being built, what is known of it, whether the view read
    /// that, whether it has keyboard focus, and the title the view gave it.
    window: crate::platform::WindowId,
    window_info: crate::platform::WindowInfo,
    window_info_read: bool,
    window_focused: bool,
    window_title: Option<String>,
    /// The window's size in logical pixels; zero when unknown.
    window_size: Size,
}

/// Per-build animation bookkeeping for a [`Cx`].
#[derive(Clone, Copy, Debug, Default)]
struct FrameInfo {
    time: std::time::Duration,
    requested: bool,
}

impl<'a, S> Cx<'a, S> {
    /// Create a context borrowing `theme`, with a handle not attached to any app.
    pub fn new(theme: &'a Theme) -> Self {
        Self::with_handle(theme, AppHandle::new())
    }

    /// Create a context for the app that `handle` belongs to.
    pub fn with_handle(theme: &'a Theme, handle: AppHandle<S>) -> Self {
        Self {
            handle,
            theme,
            taps: Vec::new(),
            keys: Vec::new(),
            raw_keys: Vec::new(),
            drags: Vec::new(),
            text_pos: Vec::new(),
            contexts: Vec::new(),
            drops: Vec::new(),
            drag_sources: Vec::new(),
            pointers: Default::default(),
            drop_states: HashMap::new(),
            drop_queried: Vec::new(),
            interactions: HashMap::new(),
            interaction_queried: Vec::new(),
            next_scroll: 0,
            overlays: Vec::new(),
            context_menus: Vec::new(),
            memo: HashMap::new(),
            memo_used: HashSet::new(),
            scroll_offsets: HashMap::new(),
            scroll_requests: ScrollRequests::default(),
            frame: FrameInfo {
                time: crate::platform::frame::elapsed().unwrap_or_default(),
                requested: false,
            },
            window: crate::platform::WindowId::PRIMARY,
            window_info: Default::default(),
            window_info_read: false,
            window_focused: true,
            window_title: None,
            window_size: Size::new(0.0, 0.0),
        }
    }

    /// Tell the view which window it builds, what is known of it, and
    /// whether it has keyboard focus.
    pub(crate) fn set_window(
        &mut self,
        id: crate::platform::WindowId,
        info: crate::platform::WindowInfo,
        focused: bool,
    ) {
        self.window = id;
        self.window_info = info;
        self.window_focused = focused;
    }

    /// The window this view is being built for: [`WindowId::PRIMARY`]
    /// for the app's first window, or the id
    /// [`App::open_window`](crate::App::open_window) /
    /// [`AppHandle::open_window`] returned. Lets one view function show
    /// different content in different windows. It is the id
    /// [`App::on_close_requested`](crate::App::on_close_requested) and
    /// [`App::on_window_focus`](crate::App::on_window_focus) are given, and
    /// [`AppHandle::close_window`] takes.
    ///
    /// [`WindowId::PRIMARY`]: crate::platform::WindowId::PRIMARY
    pub fn window_id(&self) -> crate::platform::WindowId {
        self.window
    }

    /// Where the window being built is and what state it is in (its size,
    /// position, maximized or fullscreen...). A view that reads it is built
    /// again when it changes.
    pub fn window_info(&mut self) -> crate::platform::WindowInfo {
        self.window_info_read = true;
        self.window_info
    }

    /// Whether the view read [`window_info`](Self::window_info) this build.
    pub fn window_info_read(&self) -> bool {
        self.window_info_read
    }

    /// Set the title of the window being built, as a function of the state:
    /// the app retitles the window whenever the title a build gives differs
    /// from the last one. A view that never calls this leaves the title to
    /// [`WindowAttributes::title`](crate::platform::WindowAttributes::title)
    /// and [`WindowRequest::Title`](crate::platform::WindowRequest::Title).
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # struct State { file: String, dirty: bool }
    /// fn view(s: &State, cx: &mut Cx<State>) -> Element {
    ///     cx.window_title(format!("{}{} — Editor", s.file, if s.dirty { "*" } else { "" }));
    ///     // ...
    /// #   spacer()
    /// }
    /// ```
    pub fn window_title(&mut self, title: impl Into<String>) {
        self.window_title = Some(title.into());
    }

    /// Take the title the view set with [`window_title`](Self::window_title).
    pub fn take_window_title(&mut self) -> Option<String> {
        self.window_title.take()
    }

    /// Whether the window being built has keyboard focus (it is the active
    /// window). Draw a hollow caret, or dim the selection, while it hasn't.
    /// `true` until the platform says otherwise, and always on platforms that
    /// don't report focus. The app rebuilds the view when it changes; see
    /// also [`App::on_window_focus`](crate::App::on_window_focus).
    pub fn window_focused(&self) -> bool {
        self.window_focused
    }

    /// The size of the window being built for, in logical pixels: what a
    /// widget that fits itself to the window (a [`tab_bar`] deciding how many
    /// tabs fit) measures against. Zero outside a running app, until
    /// [`set_window_size`](Self::set_window_size).
    ///
    /// [`tab_bar`]: crate::widgets::tab_bar
    pub fn window_size(&self) -> Size {
        self.window_size
    }

    /// Set what [`window_size`](Self::window_size) reports (the app does
    /// this before each build).
    pub fn set_window_size(&mut self, size: Size) {
        self.window_size = size;
    }

    /// The time of the frame being built, on the app's animation timeline (it
    /// starts when the first frame is built). The same for the whole build, so
    /// every animated widget in a frame agrees. Zero on targets without a clock
    /// (the web, for now), where nothing animates.
    pub fn frame_time(&self) -> std::time::Duration {
        self.frame.time
    }

    /// Ask for another frame at the display's next refresh, to animate. Call it
    /// in every build that still has motion to show; once builds stop asking,
    /// frames stop and an idle app uses no CPU. See `docs/animation.md`.
    pub fn request_frame(&mut self) {
        self.frame.requested = true;
    }

    /// Whether motion is reduced: the OS asks for it, or the app said so with
    /// [`anim::set_reduced_motion`](crate::anim::set_reduced_motion). Snap or
    /// cross-fade instead of moving things; durations scaled with
    /// [`anim::motion_duration`](crate::anim::motion_duration) are already
    /// zero.
    pub fn reduced_motion(&self) -> bool {
        crate::anim::reduced_motion()
    }

    /// Whether this build called [`request_frame`](Self::request_frame).
    pub fn frame_requested(&self) -> bool {
        self.frame.requested
    }

    /// A handle to the running app, to change its state from a timer or another
    /// thread. Clone it into a handler to start background work from there.
    pub fn handle(&self) -> AppHandle<S> {
        self.handle.clone()
    }

    /// The active theme.
    pub fn theme(&self) -> &Theme {
        self.theme
    }

    /// Register a pointer-tap handler, returning its [`ActionId`].
    pub fn register(&mut self, handler: impl FnMut(&mut S) + 'static) -> ActionId {
        let id = ActionId(self.taps.len() as u32);
        self.taps.push(Box::new(handler));
        id
    }

    /// Register a keyboard handler for a focusable element, returning its
    /// [`FocusId`].
    pub fn register_key(&mut self, handler: impl FnMut(&mut S, &KeyInput) + 'static) -> FocusId {
        let id = FocusId(self.keys.len() as u32);
        self.keys.push(Box::new(handler));
        id
    }

    /// Register a raw key handler (see
    /// [`Element::on_raw_key`](crate::core::Element::on_raw_key)), returning
    /// its [`RawKeyId`](crate::core::keys::RawKeyId).
    pub fn register_raw_key(
        &mut self,
        handler: impl FnMut(&mut S, &crate::platform::KeyEvent) -> crate::core::keys::Handled + 'static,
    ) -> crate::core::keys::RawKeyId {
        let id = crate::core::keys::RawKeyId(self.raw_keys.len() as u32);
        self.raw_keys.push(Box::new(handler));
        id
    }

    /// Register a drag handler, returning its [`DragId`]. The handler receives
    /// the pointer's fractional x position (0..=1) across the element.
    pub fn register_drag(&mut self, mut handler: impl FnMut(&mut S, f64) + 'static) -> DragId {
        self.register_drag_motion(move |s, m| handler(s, m.fraction_x()))
    }

    /// Register a drag handler that receives the full [`DragMotion`], returning
    /// its [`DragId`].
    pub fn register_drag_motion(
        &mut self,
        handler: impl FnMut(&mut S, &DragMotion) + 'static,
    ) -> DragId {
        let id = DragId(self.drags.len() as u32);
        self.drags.push(Box::new(handler));
        id
    }

    /// Register a text-pointer handler, returning its [`TextPosId`]. The handler
    /// receives a resolved byte index into the element's text and an `extend`
    /// flag (`false` = place caret, `true` = extend selection).
    pub fn register_text_pos(
        &mut self,
        handler: impl FnMut(&mut S, usize, bool) + 'static,
    ) -> TextPosId {
        let id = TextPosId(self.text_pos.len() as u32);
        self.text_pos.push(Box::new(handler));
        id
    }

    /// Register a secondary-click (context) handler, returning its
    /// [`ContextId`]. The handler receives the right-click position in logical
    /// pixels — typically used to open a context menu there via [`Cx::overlay`].
    pub fn register_context(&mut self, handler: impl FnMut(&mut S, Point) + 'static) -> ContextId {
        let id = ContextId(self.contexts.len() as u32);
        self.contexts.push(Box::new(handler));
        id
    }

    /// Register a drop target accepting the kinds in `accepts`, returning its
    /// [`DropId`]. `handler` receives the dropped data and the drop position
    /// in logical pixels. Prefer [`Element::on_drop`](crate::core::Element::on_drop).
    pub fn register_drop(
        &mut self,
        accepts: DragOffer,
        handler: impl FnMut(&mut S, &DropData, Point) + 'static,
    ) -> DropId {
        let id = DropId(self.drops.len() as u32);
        self.drops.push(DropTarget {
            accepts,
            on_drop: Box::new(handler),
            on_hover: None,
        });
        id
    }

    /// Tell drop target `id` when an acceptable drag starts (`true`) and stops
    /// (`false`) hovering it, replacing any earlier hover handler. Prefer
    /// [`Element::on_drop_hover`](crate::core::Element::on_drop_hover).
    pub fn set_drop_hover(&mut self, id: DropId, handler: impl FnMut(&mut S, bool) + 'static) {
        if let Some(target) = self.drops.get_mut(id.0 as usize) {
            target.on_hover = Some(Box::new(handler));
        }
    }

    /// What a drag is doing to drop target `id` in this frame: whether it is
    /// over it, whether the target accepts it, the pointer within it, and an
    /// eased `level` to animate with. Register the target first (its id is
    /// stable across builds as long as the view registers drop targets in the
    /// same order), read its state, build the element from it and stamp the
    /// id with [`Element::drop_target`](crate::core::Element::drop_target).
    /// The app then leaves the drawing of the drag-over look to the view (it
    /// paints its own highlight only on targets whose state isn't read), and
    /// rebuilds on every change. See `docs/drag-and-drop.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct St { dropped: usize }
    /// fn view(_: &St, cx: &mut Cx<St>) -> Element {
    ///     let theme = *cx.theme();
    ///     let zone = cx.register_drop(DragOffer::FILES, |s: &mut St, _, _| s.dropped += 1);
    ///     let drag = cx.drop_state(zone);
    ///     let grow = 8.0 * drag.level;
    ///     label(&theme, "Drop here")
    ///         .padding(Insets::uniform(16.0 + grow))
    ///         .drop_target(zone)
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # assert!(view(&St { dropped: 0 }, &mut cx).drop.is_some());
    /// ```
    pub fn drop_state(&mut self, id: DropId) -> crate::core::dnd::DropState {
        if !self.drop_queried.contains(&id) {
            self.drop_queried.push(id);
        }
        self.drop_states.get(&id).copied().unwrap_or_default()
    }

    /// Seed this frame's drop-target states (the app does this before each
    /// build).
    pub fn set_drop_states(&mut self, states: HashMap<DropId, crate::core::dnd::DropState>) {
        self.drop_states = states;
    }

    /// The drop targets whose state the view read this build. Call before
    /// [`into_handlers`](Cx::into_handlers).
    pub fn take_drop_queried(&mut self) -> Vec<DropId> {
        std::mem::take(&mut self.drop_queried)
    }

    /// The interaction states of the element keyed `key` (see
    /// [`Element::key`](crate::core::Element::key)) as of this frame: whether
    /// it is hovered, pressed, focused (and focus-visible), dragged, or
    /// dragged over (accepted or refused). They come from where the element
    /// was in the frame on screen, so build it with the same key each time.
    /// The app rebuilds when any of them changes for an element whose states
    /// the view read. `DISABLED` and `SELECTED` are the view's own to set and
    /// aren't reported here. See `docs/animation.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// fn view(_: &(), cx: &mut Cx<()>) -> Element {
    ///     let theme = *cx.theme();
    ///     let states = cx.interaction("card");
    ///     let text = if states.pressed() { "Let go to open" } else { "Open" };
    ///     label(&theme, text).key("card").on_tap(cx, |_| {})
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&(), &mut cx);
    /// ```
    pub fn interaction(&mut self, key: impl std::hash::Hash) -> crate::core::Interaction {
        let key = crate::core::ElementKey::new(key);
        if !self.interaction_queried.contains(&key) {
            self.interaction_queried.push(key);
        }
        self.interactions.get(&key).copied().unwrap_or_default()
    }

    /// Seed this frame's interaction states (the app does this before each
    /// build).
    pub(crate) fn set_interactions(
        &mut self,
        states: HashMap<crate::core::ElementKey, crate::core::Interaction>,
    ) {
        self.interactions = states;
    }

    /// The keys whose states the view read this build, with those states.
    pub(crate) fn take_interaction_queried(
        &mut self,
    ) -> Vec<(crate::core::ElementKey, crate::core::Interaction)> {
        let keys = std::mem::take(&mut self.interaction_queried);
        keys.into_iter()
            .map(|k| (k, self.interactions.get(&k).copied().unwrap_or_default()))
            .collect()
    }

    /// Register a drag source, returning its [`DragSourceId`]. `handler` runs
    /// when a press on the element moves far enough to start a drag, and
    /// returns its payload (`None` declines). Prefer
    /// [`Element::drag_source`](crate::core::Element::drag_source).
    pub fn register_drag_source(
        &mut self,
        handler: impl FnMut(&mut S) -> Option<DropData> + 'static,
    ) -> DragSourceId {
        let id = DragSourceId(self.drag_sources.len() as u32);
        self.drag_sources.push(Box::new(handler));
        id
    }

    /// Register a pointer handler on `id` (an element's existing
    /// [`PointerId`](crate::core::PointerId), or `None` for a new one),
    /// returning the id. Prefer
    /// [`Element::on_pointer`](crate::core::Element::on_pointer).
    pub fn register_pointer(
        &mut self,
        id: Option<crate::core::PointerId>,
        handler: impl FnMut(&crate::core::PointerEvent, &mut S) + 'static,
    ) -> crate::core::PointerId {
        self.pointers.set_pointer(id, handler)
    }

    /// Register a wheel handler on `id` (or a new id), returning the id.
    /// Prefer [`Element::on_wheel`](crate::core::Element::on_wheel).
    pub fn register_wheel(
        &mut self,
        id: Option<crate::core::PointerId>,
        handler: impl FnMut(&crate::core::WheelEvent, &mut S) + 'static,
    ) -> crate::core::PointerId {
        self.pointers.set_wheel(id, handler)
    }

    /// Register `menu` as the context menu of an element, returning the
    /// [`ContextId`] to stamp on it. The app opens it on a right-click (or the
    /// Menu key) over the element. Prefer
    /// [`Element::context_menu`](crate::core::Element::context_menu).
    pub fn register_context_menu(&mut self, menu: crate::core::menu::Menu<S>) -> ContextId {
        let id = self.register_context(|_, _| {});
        self.context_menus.push((id, menu));
        id
    }

    /// Take the context menus registered this frame (the app keeps the one
    /// that is open). Call before [`into_handlers`](Cx::into_handlers).
    pub fn take_context_menus(&mut self) -> Vec<(ContextId, crate::core::menu::Menu<S>)> {
        std::mem::take(&mut self.context_menus)
    }

    /// Register a scroll container, returning a stable [`ScrollId`]. The app
    /// keeps the scroll offset for this id and re-applies it each frame; there is
    /// no handler closure (scrolling adjusts the offset directly).
    pub fn register_scroll(&mut self) -> ScrollId {
        let id = ScrollId(self.next_scroll);
        self.next_scroll += 1;
        id
    }

    /// The current offset of scroll container `id` (logical pixels from the
    /// top; 0 if it hasn't scrolled). Not yet clamped to the content, so a
    /// widget that depends on it clamps it itself. A widget that builds only
    /// what is visible (see [`virtual_list`](crate::widgets::virtual_list))
    /// reads it right after [`register_scroll`](Cx::register_scroll).
    pub fn scroll_offset(&self, id: ScrollId) -> f64 {
        self.scroll_offsets
            .get(&id)
            .copied()
            .unwrap_or(0.0)
            .max(0.0)
    }

    /// A handle for handlers to move scroll containers (see [`ScrollRequests`]).
    pub fn scroll_requests(&self) -> ScrollRequests {
        self.scroll_requests.clone()
    }

    /// Seed this frame's scroll offsets and the request queue (the app does
    /// this before each build).
    pub fn set_scroll_state(&mut self, offsets: HashMap<ScrollId, f64>, requests: ScrollRequests) {
        self.scroll_offsets = offsets;
        self.scroll_requests = requests;
    }

    /// Declare a floating overlay layer (menu/popover/tooltip/dialog) drawn above
    /// the main tree this frame. Build `spec.content` with this same `Cx` first
    /// so its handlers register normally.
    pub fn overlay(&mut self, spec: OverlaySpec) {
        self.overlays.push(spec);
    }

    /// Take the overlays declared this frame (the app lays them out + paints them
    /// on top). Call before [`into_handlers`](Cx::into_handlers).
    pub fn take_overlays(&mut self) -> Vec<OverlaySpec> {
        std::mem::take(&mut self.overlays)
    }

    /// Return a cached, **static** subtree for `key`, building it with `build`
    /// only when the key is new (or after the cache was seeded from a prior
    /// frame). On an unchanged key the `build` closure is skipped entirely — the
    /// previous frame's [`Element`] is cloned — so unchanged branches aren't
    /// rebuilt. `build` receives no [`Cx`], so a memoized subtree can't register
    /// event handlers (their ids would desync); use it for display-only content
    /// like icons, labels, or decorative panels whose look depends on `key`.
    pub fn memo(&mut self, key: u64, build: impl FnOnce() -> Element) -> Element {
        self.memo_used.insert(key);
        if let Some(cached) = self.memo.get(&key) {
            return cached.clone();
        }
        let element = build();
        self.memo.insert(key, element.clone());
        element
    }

    /// Seed the memo cache from the previous frame (see [`Cx::memo`]).
    pub fn set_memo_cache(&mut self, cache: HashMap<u64, Element>) {
        self.memo = cache;
        self.memo_used.clear();
    }

    /// Take the memo cache back, dropping entries not touched this frame so it
    /// doesn't grow without bound.
    pub fn take_memo_cache(&mut self) -> HashMap<u64, Element> {
        let used = std::mem::take(&mut self.memo_used);
        let mut cache = std::mem::take(&mut self.memo);
        cache.retain(|k, _| used.contains(k));
        cache
    }

    /// Consume the context, yielding the accumulated [`Handlers`] table.
    pub fn into_handlers(self) -> Handlers<S> {
        Handlers {
            taps: self.taps,
            keys: self.keys,
            raw_keys: self.raw_keys,
            drags: self.drags,
            text_pos: self.text_pos,
            contexts: self.contexts,
            drops: self.drops,
            drag_sources: self.drag_sources,
            pointers: self.pointers,
        }
    }
}

/// Driving animations from a build: sample at the frame time, and ask for the
/// next frame only while something still moves.
impl<S> Cx<'_, S> {
    /// The value of `anim` at this frame, asking for another frame while it's
    /// still moving. See `docs/animation.md`.
    pub fn animate<T: crate::anim::Animatable>(&mut self, anim: &crate::anim::Animated<T>) -> T {
        let now = self.frame_time();
        if anim.is_animating(now) {
            self.request_frame();
        }
        anim.value(now)
    }

    /// Sample `anim`, started at `started` on the frame timeline (a
    /// `frame_time()` or `anim::now()` taken when it began), asking for
    /// another frame until it finishes.
    pub fn play<A: crate::anim::Animation>(
        &mut self,
        anim: &A,
        started: std::time::Duration,
    ) -> A::Value {
        let t = self.frame_time().as_secs_f64() - started.as_secs_f64();
        if !anim.is_finished(t) {
            self.request_frame();
        }
        anim.sample(t)
    }
}

impl<S> core::fmt::Debug for Cx<'_, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cx")
            .field("taps", &self.taps.len())
            .field("keys", &self.keys.len())
            .field("drags", &self.drags.len())
            .finish_non_exhaustive()
    }
}

/// The handler tables produced by building a frame. Dispatch resolves an
/// [`ActionId`] / [`FocusId`] (from [`hit_test`] / [`focus_at`]) to its handler
/// and invokes it against the app state.
pub struct Handlers<S> {
    taps: Vec<TapFn<S>>,
    keys: Vec<KeyFn<S>>,
    raw_keys: Vec<RawKeyFn<S>>,
    drags: Vec<DragFn<S>>,
    text_pos: Vec<TextPosFn<S>>,
    contexts: Vec<ContextFn<S>>,
    drops: Vec<DropTarget<S>>,
    drag_sources: Vec<DragSourceFn<S>>,
    pointers: crate::core::PointerHandlers<S>,
}

impl<S> Handlers<S> {
    /// Invoke the tap handler for `id`. Returns `true` if one existed and ran.
    pub fn dispatch(&mut self, id: ActionId, state: &mut S) -> bool {
        if let Some(handler) = self.taps.get_mut(id.0 as usize) {
            handler(state);
            true
        } else {
            false
        }
    }

    /// Invoke the key handler for focused element `id` with `input`. Returns
    /// `true` if one existed and ran.
    pub fn dispatch_key(&mut self, id: FocusId, input: &KeyInput, state: &mut S) -> bool {
        if let Some(handler) = self.keys.get_mut(id.0 as usize) {
            handler(state, input);
            true
        } else {
            false
        }
    }

    /// Invoke raw key handler `id` with `event`. `None` if there is no such
    /// handler.
    pub fn dispatch_raw_key(
        &mut self,
        id: crate::core::keys::RawKeyId,
        event: &crate::platform::KeyEvent,
        state: &mut S,
    ) -> Option<crate::core::keys::Handled> {
        let handler = self.raw_keys.get_mut(id.0 as usize)?;
        Some(handler(state, event))
    }

    /// Invoke the drag handler for `id` with `fraction` (0..=1 across the
    /// element width). Returns `true` if one existed and ran.
    pub fn dispatch_drag(&mut self, id: DragId, fraction: f64, state: &mut S) -> bool {
        // A unit-wide element, so `fraction_x` is `fraction` again.
        let unit = Rect::from_xywh(0.0, 0.0, 1.0, 1.0);
        let at = Point::new(fraction, 0.0);
        let motion = DragMotion {
            position: at,
            origin: at,
            bounds: unit,
            parent: unit,
        };
        self.dispatch_drag_motion(id, &motion, state)
    }

    /// Invoke the drag handler for `id` with `motion`. Returns `true` if one
    /// existed and ran.
    pub fn dispatch_drag_motion(&mut self, id: DragId, motion: &DragMotion, state: &mut S) -> bool {
        if let Some(handler) = self.drags.get_mut(id.0 as usize) {
            handler(state, motion);
            true
        } else {
            false
        }
    }

    /// Invoke the text-pointer handler for `id` with a resolved byte `index` and
    /// the `extend` flag. Returns `true` if one existed and ran.
    pub fn dispatch_text_pos(
        &mut self,
        id: TextPosId,
        index: usize,
        extend: bool,
        state: &mut S,
    ) -> bool {
        if let Some(handler) = self.text_pos.get_mut(id.0 as usize) {
            handler(state, index, extend);
            true
        } else {
            false
        }
    }

    /// Invoke the context (secondary-click) handler for `id` with the click
    /// `pos`. Returns `true` if one existed and ran.
    pub fn dispatch_context(&mut self, id: ContextId, pos: Point, state: &mut S) -> bool {
        if let Some(handler) = self.contexts.get_mut(id.0 as usize) {
            handler(state, pos);
            true
        } else {
            false
        }
    }

    /// Whether drop target `id` exists and accepts something `offer` carries.
    pub fn drop_accepts(&self, id: DropId, offer: DragOffer) -> bool {
        self.drops
            .get(id.0 as usize)
            .is_some_and(|t| t.accepts.accepts(offer))
    }

    /// Invoke drop target `id`'s drop handler. Returns `true` if it ran.
    pub fn dispatch_drop(
        &mut self,
        id: DropId,
        data: &DropData,
        pos: Point,
        state: &mut S,
    ) -> bool {
        match self.drops.get_mut(id.0 as usize) {
            Some(target) => {
                (target.on_drop)(state, data, pos);
                true
            }
            None => false,
        }
    }

    /// Tell drop target `id` a drag started (`true`) or stopped (`false`)
    /// hovering it. Returns `true` if it has a hover handler, which ran.
    pub fn dispatch_drop_hover(&mut self, id: DropId, hovering: bool, state: &mut S) -> bool {
        match self
            .drops
            .get_mut(id.0 as usize)
            .and_then(|t| t.on_hover.as_mut())
        {
            Some(handler) => {
                handler(state, hovering);
                true
            }
            None => false,
        }
    }

    /// Ask drag source `id` for its payload. `None` if it declined or doesn't
    /// exist.
    pub fn dispatch_drag_source(&mut self, id: DragSourceId, state: &mut S) -> Option<DropData> {
        self.drag_sources.get_mut(id.0 as usize)?(state)
    }

    /// The pointer and wheel handlers.
    pub(crate) fn pointers(&mut self) -> &mut crate::core::PointerHandlers<S> {
        &mut self.pointers
    }

    pub(crate) fn pointers_ref(&self) -> &crate::core::PointerHandlers<S> {
        &self.pointers
    }

    /// Total number of registered handlers (taps + keys + drags + text-pointer
    /// + context + pointer).
    pub fn len(&self) -> usize {
        self.taps.len()
            + self.keys.len()
            + self.drags.len()
            + self.text_pos.len()
            + self.contexts.len()
            + self.pointers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<S> Default for Handlers<S> {
    fn default() -> Self {
        Self {
            taps: Vec::new(),
            keys: Vec::new(),
            raw_keys: Vec::new(),
            drags: Vec::new(),
            text_pos: Vec::new(),
            contexts: Vec::new(),
            drops: Vec::new(),
            drag_sources: Vec::new(),
            pointers: Default::default(),
        }
    }
}

impl<S> core::fmt::Debug for Handlers<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Handlers")
            .field("taps", &self.taps.len())
            .field("keys", &self.keys.len())
            .field("drags", &self.drags.len())
            .field("text_pos", &self.text_pos.len())
            .finish()
    }
}

/// Paintable leaf content carried by a [`LayoutNode`] beyond its decoration.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum NodeContent {
    /// Decoration only (the common case).
    #[default]
    None,
    /// Text, painted from the node's bounds origin (right-to-left paragraphs
    /// against its right edge).
    Text {
        text: String,
        size: f64,
        color: Color,
        direction: crate::render::TextDirection,
        /// A byte range drawn underlined (see [`Element::underline`]).
        ///
        /// [`Element::underline`]: crate::core::Element::underline
        underline: Option<(usize, usize)>,
        /// Font, weight, slant and decoration lines.
        style: crate::core::TextStyle,
    },
    /// An embedded-content viewport: the node's bounds reserve an area the
    /// compositor fills with externally-rendered pixels for this
    /// [`ViewportId`]. Painted via
    /// [`Scene::fill_viewport`](crate::render::Scene::fill_viewport).
    Viewport(ViewportId),
    /// An image placed per `fit` in the node's bounds less `padding`. Compares
    /// by image identity, so swapping the image damages the node.
    Image {
        source: crate::render::ImageSource,
        fit: crate::render::ImageFit,
        padding: crate::geometry::Insets,
    },
    /// A custom-paint canvas: the node's bounds are painted by `paint` (see
    /// [`Element::canvas`](crate::core::Element::canvas)). Compares by id and
    /// revision only, so it is damaged by invalidation, not by a rebuild.
    Canvas {
        id: crate::core::CanvasId,
        paint: crate::core::CanvasPainter,
        revision: u64,
    },
}

/// A laid-out, retained node: absolute bounds, paint decoration, optional text
/// content, the optional tap/focus handles it routes to, and laid-out children.
/// Produced by [`layout`](crate::core::layout) and consumed by paint, [`hit_test`],
/// and the focus queries.
#[derive(Clone, Debug)]
pub struct LayoutNode {
    pub bounds: Rect,
    pub decoration: BoxStyle,
    pub content: NodeContent,
    pub action: Option<ActionId>,
    pub focus: Option<FocusId>,
    pub drag: Option<DragId>,
    /// Secondary-click (context) handle: this element opens a context menu on
    /// right-click.
    pub context: Option<ContextId>,
    /// Drop-target handle: drags from other apps (and in-app drags) can be
    /// dropped here.
    pub drop: Option<DropId>,
    /// Drag-source handle: a press-and-move here starts an in-app drag.
    pub drag_source: Option<DragSourceId>,
    /// Pointer and wheel handle: raw pointer events and wheel scrolls over
    /// this node go to its handlers (see `Element::on_pointer`).
    pub pointer: Option<crate::core::PointerId>,
    /// Caret byte index for an editable text leaf (drawn by the focus overlay).
    pub caret: Option<usize>,
    /// Selected byte range `[start, end)` for an editable text leaf (the focus
    /// overlay highlights it).
    pub selection: Option<(usize, usize)>,
    /// Byte range of input-method composition (preedit) text inside an editable
    /// text leaf (the focus overlay underlines it).
    pub preedit: Option<(usize, usize)>,
    /// The element's keyboard settings (raw key handler, Tab, IME, Option as
    /// Meta), copied from [`Element::keyboard`](crate::core::Element::keyboard).
    pub keyboard: Option<Box<crate::core::keys::KeyConfig>>,
    /// Text-pointer handle: this element resolves pointer presses/drags to a
    /// byte index in its text (click-to-position / drag-to-select).
    pub text_pos: Option<TextPosId>,
    /// When `true`, the text content word-wraps to `bounds.width` when painted.
    pub wrap: bool,
    /// When set, the OS draws this node's chrome. See [`Element::native`].
    ///
    /// [`Element::native`]: crate::core::Element
    pub native: Option<crate::platform::native::NativeDeco>,
    /// Scroll container handle: wheel events over this node adjust the app's
    /// offset for `id`, and its children are laid out at natural size + shifted.
    pub scroll: Option<ScrollId>,
    /// When `true`, children are clipped to this node's `bounds` when painted
    /// (set for scroll containers and overlay panels).
    pub clip: bool,
    /// The cursor this element asked for (see [`Element::cursor`]); `None`
    /// picks one from its handlers in [`cursor_at`].
    ///
    /// [`Element::cursor`]: crate::core::Element::cursor
    pub cursor: Option<CursorIcon>,
    /// Declared accessibility semantics (role, name, item state), copied from
    /// the element. See [`Element::role`].
    ///
    /// [`Element::role`]: crate::core::Element::role
    pub semantics: Option<Semantics>,
    /// Whether this node was laid out right to left (its effective
    /// [`LayoutDirection`](crate::layout::LayoutDirection)): its children are
    /// mirrored, `Auto` text in it lines up on the right, and a drag on it
    /// reports mirrored x positions.
    pub rtl: bool,
    /// How this node and its subtree are drawn moved, scaled or rotated (see
    /// [`Element::transform`]). `bounds` stay the laid-out box; the transform
    /// is resolved against them by [`LayoutNode::affine`], so scrolling a
    /// transformed node keeps its origin in place. Pointer queries map points
    /// through its inverse.
    ///
    /// [`Element::transform`]: crate::core::Element::transform
    pub transform: Option<crate::geometry::Transform>,
    /// Group opacity of this node and its subtree (see [`Element::opacity`]).
    ///
    /// [`Element::opacity`]: crate::core::Element::opacity
    pub opacity: f32,
    /// The element's identity, if it uses interaction states or transitions
    /// (see [`Element::key`](crate::core::Element::key)): what the app finds
    /// it by to work out its [`Interaction`](crate::core::Interaction).
    pub key: Option<crate::core::ElementKey>,
    /// A copy of an element that has left, drawn while its
    /// [exit](crate::core::Element::exit) effect runs: stripped of handlers,
    /// identity and semantics, and skipped by hover and accessibility.
    pub ghost: bool,
    pub children: Vec<LayoutNode>,
}

impl LayoutNode {
    /// A bare container: bounds + `children`, no decoration or handlers. Used to
    /// stack the main tree and overlay layers under one routable/paintable root.
    pub fn container(bounds: Rect, children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode {
            bounds,
            decoration: BoxStyle::default(),
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
            scroll: None,
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
}

impl LayoutNode {
    /// The matrix mapping this node's layout coordinates (its `bounds` and its
    /// subtree's) to where they are drawn in its parent's coordinates, or
    /// `None` when it has no transform.
    pub fn affine(&self) -> Option<Affine> {
        self.transform.map(|t| t.to_affine(self.bounds))
    }

    /// `point`, given in the parent's coordinates, in this node's layout
    /// coordinates (through the inverse of its transform), or `None` when the
    /// transform is singular (scaled to nothing, so nothing in it can be hit).
    pub fn to_local(&self, point: Point) -> Option<Point> {
        match self.affine() {
            None => Some(point),
            Some(a) => a.inverse().map(|inv| inv.apply(point)),
        }
    }
}

/// `point` (in window coordinates) in the layout coordinates of the first node
/// of `root`'s tree, depth-first, that `is` matches: through the inverse of
/// every transform from the root down to and including that node's own. With
/// no transforms on the way it is `point` itself; `None` if no node matches or
/// a transform on the way is singular.
///
/// Pointer handling uses it to read a pointer against a node found earlier
/// (a drag's draggable, a text field being selected in), where the node's
/// `bounds` are in its layout space.
pub fn local_point(
    root: &LayoutNode,
    is: &dyn Fn(&LayoutNode) -> bool,
    point: Point,
) -> Option<Point> {
    fn walk(
        node: &LayoutNode,
        is: &dyn Fn(&LayoutNode) -> bool,
        point: Point,
    ) -> Option<Option<Point>> {
        let local = node.to_local(point);
        if is(node) {
            return Some(local);
        }
        node.children.iter().find_map(|c| match local {
            Some(p) => walk(c, is, p),
            // Under a singular transform: still find the node, but it has no
            // local point.
            None => walk(c, is, point).map(|_| None),
        })
    }
    walk(root, is, point).flatten()
}

/// The matrix mapping the layout coordinates of the first node `is` matches
/// (depth-first) to window coordinates — the product of the transforms from
/// the root down to and including that node's own — together with the node
/// and the product of the group opacities on the way. `None` if no node
/// matches. The matrix is the identity where nothing is transformed.
///
/// Overlays painted outside the tree (the hover highlight, the focus ring)
/// use it to draw where the node is drawn, and damage tracking to know where
/// that is.
pub fn node_to_window<'a>(
    root: &'a LayoutNode,
    is: &dyn Fn(&LayoutNode) -> bool,
) -> Option<(&'a LayoutNode, Affine, f32)> {
    fn walk<'a>(
        node: &'a LayoutNode,
        is: &dyn Fn(&LayoutNode) -> bool,
        outer: Affine,
        opacity: f32,
    ) -> Option<(&'a LayoutNode, Affine, f32)> {
        let m = match node.affine() {
            Some(a) => a.then(outer),
            None => outer,
        };
        let opacity = opacity * node.opacity;
        if is(node) {
            return Some((node, m, opacity));
        }
        node.children.iter().find_map(|c| walk(c, is, m, opacity))
    }
    walk(root, is, Affine::IDENTITY, 1.0)
}

/// Whether `point` falls outside a clipping node, where nothing under it is
/// drawn — so nothing under it can be hit (a scrolled-away row of a scroll
/// container, say, lying under a header).
pub(crate) fn clipped_out(node: &LayoutNode, point: Point) -> bool {
    node.clip && !node.bounds.contains(point)
}

/// Find the [`ActionId`] of the top-most tappable node containing `point`.
///
/// Children are painted after (on top of) their parent, so they are tested
/// first, last-to-first, mirroring paint order.
///
/// `point` is in window coordinates, and a node under a
/// [transform](LayoutNode::transform) is hit where it is drawn — as in every
/// `*_at` query here. Bounds those return ([`drag_at`], [`drop_at`],
/// [`viewport_at`]) are layout bounds: read a pointer against them with
/// [`local_point`].
pub fn hit_test(node: &LayoutNode, point: Point) -> Option<ActionId> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(id) = hit_test(child, point) {
            return Some(id);
        }
    }
    if node.action.is_some() && node.bounds.contains(point) {
        node.action
    } else {
        None
    }
}

/// The cursor to show with the pointer at `point`: that of the top-most node
/// under it that has one. A node's own [`cursor`](LayoutNode::cursor) wins;
/// otherwise editable text shows the I-beam and tap or drag targets the
/// pointing hand. Elsewhere it is [`CursorIcon::Default`].
pub fn cursor_at(node: &LayoutNode, point: Point) -> CursorIcon {
    fn find(node: &LayoutNode, point: Point) -> Option<CursorIcon> {
        let point = node.to_local(point)?;
        let inside = node.bounds.contains(point);
        // A clipped node hides its children outside its bounds; an unclipped
        // one may have children that overflow it.
        if node.clip && !inside {
            return None;
        }
        for child in node.children.iter().rev() {
            if let Some(c) = find(child, point) {
                return Some(c);
            }
        }
        if !inside {
            return None;
        }
        node.cursor.or(if node.text_pos.is_some() {
            Some(CursorIcon::Text)
        } else if node.action.is_some() || node.drag.is_some() {
            Some(CursorIcon::Pointer)
        } else {
            None
        })
    }
    find(node, point).unwrap_or_default()
}

/// Find the [`ContextId`] of the top-most node with a secondary-click handler
/// containing `point` (mirrors [`hit_test`], for right-clicks).
pub fn context_at(node: &LayoutNode, point: Point) -> Option<ContextId> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(id) = context_at(child, point) {
            return Some(id);
        }
    }
    if node.context.is_some() && node.bounds.contains(point) {
        node.context
    } else {
        None
    }
}

/// Find the top-most drop target containing `point`, returning its
/// [`DropId`] and bounds (mirrors [`hit_test`], for drags). Whether it takes
/// the drag is up to its handler ([`Handlers::drop_accepts`]).
pub fn drop_at(node: &LayoutNode, point: Point) -> Option<(DropId, Rect)> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(hit) = drop_at(child, point) {
            return Some(hit);
        }
    }
    match node.drop {
        Some(id) if node.bounds.contains(point) => Some((id, node.bounds)),
        _ => None,
    }
}

/// Find the node carrying drop target `id`, if present.
pub fn find_drop(node: &LayoutNode, id: DropId) -> Option<&LayoutNode> {
    if node.drop == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_drop(c, id))
}

/// Find the [`DragSourceId`] of the top-most drag source containing `point`.
pub fn drag_source_at(node: &LayoutNode, point: Point) -> Option<DragSourceId> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(id) = drag_source_at(child, point) {
            return Some(id);
        }
    }
    match node.drag_source {
        Some(id) if node.bounds.contains(point) => Some(id),
        _ => None,
    }
}

/// Find the top-most text-pointer node containing `point`, returning its
/// [`TextPosId`] and the node (so the caller can resolve a byte index from the
/// node's text and bounds).
pub fn text_pos_at(node: &LayoutNode, point: Point) -> Option<(TextPosId, &LayoutNode)> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(hit) = text_pos_at(child, point) {
            return Some(hit);
        }
    }
    match node.text_pos {
        Some(id) if node.bounds.contains(point) => Some((id, node)),
        // A press anywhere in a text field reaches its text, which can be
        // narrower than the field (or empty, and zero-width): the caret goes
        // to the nearest position.
        _ if node.focus.is_some() && node.bounds.contains(point) => first_text_pos(node),
        _ => None,
    }
}

/// The first node with a [`TextPosId`] in `node`'s subtree, depth-first.
fn first_text_pos(node: &LayoutNode) -> Option<(TextPosId, &LayoutNode)> {
    if let Some(id) = node.text_pos {
        return Some((id, node));
    }
    node.children.iter().find_map(first_text_pos)
}

/// Find the [`FocusId`] of the top-most focusable node containing `point`
/// (used for click-to-focus).
pub fn focus_at(node: &LayoutNode, point: Point) -> Option<FocusId> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(id) = focus_at(child, point) {
            return Some(id);
        }
    }
    if node.focus.is_some() && node.bounds.contains(point) {
        node.focus
    } else {
        None
    }
}

/// Find the top-most draggable node containing `point`, returning its
/// [`DragId`] and bounds (so the caller can compute the drag fraction).
pub fn drag_at(node: &LayoutNode, point: Point) -> Option<(DragId, Rect)> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(hit) = drag_at(child, point) {
            return Some(hit);
        }
    }
    match node.drag {
        Some(id) if node.bounds.contains(point) => Some((id, node.bounds)),
        _ => None,
    }
}

/// Like [`drag_at`], but also returns the bounds of the draggable's parent
/// (its own bounds when it is `node` itself), for [`DragMotion::parent`],
/// and whether the draggable was laid out right to left ([`LayoutNode::rtl`]).
pub fn drag_hit_at(node: &LayoutNode, point: Point) -> Option<(DragId, Rect, Rect, bool)> {
    fn walk(node: &LayoutNode, parent: Rect, point: Point) -> Option<(DragId, Rect, Rect, bool)> {
        let point = node.to_local(point)?;
        for child in node.children.iter().rev() {
            if let Some(hit) = walk(child, node.bounds, point) {
                return Some(hit);
            }
        }
        match node.drag {
            Some(id) if node.bounds.contains(point) => Some((id, node.bounds, parent, node.rtl)),
            _ => None,
        }
    }
    walk(node, node.bounds, point)
}

/// Find the [`ScrollId`] of the top-most scroll container containing `point`
/// (the wheel target). Children are tested first so a nested scroll area wins.
pub fn scroll_at(node: &LayoutNode, point: Point) -> Option<ScrollId> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(id) = scroll_at(child, point) {
            return Some(id);
        }
    }
    match node.scroll {
        Some(id) if node.bounds.contains(point) => Some(id),
        _ => None,
    }
}

/// Find the scroll-container node carrying `id`, if present.
pub fn find_scroll(node: &LayoutNode, id: ScrollId) -> Option<&LayoutNode> {
    if node.scroll == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_scroll(c, id))
}

/// Find the node carrying focus `id`, if present.
pub fn find_focus(node: &LayoutNode, id: FocusId) -> Option<&LayoutNode> {
    if node.focus == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_focus(c, id))
}

/// Find the node carrying tap-action `id`, if present (for hover highlight).
pub fn find_action(node: &LayoutNode, id: ActionId) -> Option<&LayoutNode> {
    if node.action == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_action(c, id))
}

/// Find the node carrying text-pointer `id`, if present (for continuing a
/// drag-selection after the pointer leaves the element bounds).
pub fn find_text_pos(node: &LayoutNode, id: TextPosId) -> Option<&LayoutNode> {
    if node.text_pos == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find_text_pos(c, id))
}

/// The first text-bearing [`LayoutNode`] at or under `node`, in tree order.
/// Used to position the caret and selection highlight inside a focused text
/// field (read its `content` text/size plus `bounds`/`caret`/`selection`).
pub fn first_text(node: &LayoutNode) -> Option<&LayoutNode> {
    if matches!(node.content, NodeContent::Text { .. }) {
        return Some(node);
    }
    node.children.iter().find_map(first_text)
}

/// Collect every embedded-content viewport in the tree as `(id, bounds)`, in
/// paint order, so the app can composite each one's registered content into its
/// laid-out rect (and route input landing inside it to that content). Bounds are
/// in absolute logical pixels.
pub fn collect_viewports(node: &LayoutNode, out: &mut Vec<(ViewportId, Rect)>) {
    if let NodeContent::Viewport(id) = node.content {
        out.push((id, node.bounds));
    }
    for child in &node.children {
        collect_viewports(child, out);
    }
}

/// Find the top-most embedded-content viewport containing `point`, returning its
/// [`ViewportId`] and bounds (so the app can forward the event with
/// viewport-local coordinates). Children are tested first, mirroring paint order.
pub fn viewport_at(node: &LayoutNode, point: Point) -> Option<(ViewportId, Rect)> {
    // A transformed node is hit where it's drawn: read the point in its
    // layout space (and its subtree's).
    let point = node.to_local(point)?;
    if clipped_out(node, point) {
        return None;
    }
    for child in node.children.iter().rev() {
        if let Some(hit) = viewport_at(child, point) {
            return Some(hit);
        }
    }
    match node.content {
        NodeContent::Viewport(id) if node.bounds.contains(point) => Some((id, node.bounds)),
        _ => None,
    }
}

/// Collect every focusable [`FocusId`] in paint/tree order, for Tab traversal.
pub fn collect_focusables(node: &LayoutNode, out: &mut Vec<FocusId>) {
    if let Some(id) = node.focus {
        out.push(id);
    }
    for child in &node.children {
        collect_focusables(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirrored_drag_reads_from_the_right() {
        let motion = DragMotion {
            position: Point::new(130.0, 7.0),
            origin: Point::new(120.0, 5.0),
            bounds: Rect::from_xywh(100.0, 0.0, 100.0, 20.0),
            parent: Rect::from_xywh(50.0, 0.0, 200.0, 20.0),
        };
        assert!((motion.fraction_x() - 0.3).abs() < 1e-9);
        let m = motion.mirrored();
        assert!((m.fraction_x() - 0.7).abs() < 1e-9);
        // Moving right is moving towards the start: a negative x delta.
        assert_eq!(m.delta(), (-10.0, 2.0));
        // The parent stays put; the element is reflected inside it.
        assert_eq!(m.parent, motion.parent);
        assert_eq!(m.bounds, Rect::from_xywh(100.0, 0.0, 100.0, 20.0));
        assert_eq!(m.mirrored(), motion);
    }

    fn leaf(bounds: Rect, action: Option<ActionId>, focus: Option<FocusId>) -> LayoutNode {
        LayoutNode {
            bounds,
            decoration: BoxStyle::default(),
            content: NodeContent::None,
            action,
            focus,
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
            scroll: None,
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

    #[derive(Default)]
    struct St {
        n: i32,
        s: String,
    }

    #[test]
    fn dispatch_tap_and_key() {
        let theme = Theme::light();
        let mut cx = Cx::new(&theme);
        let tap = cx.register(|st: &mut St| st.n += 5);
        let focus = cx.register_key(|st: &mut St, k: &KeyInput| {
            if let KeyInput::Text(t) = k {
                st.s.push_str(t);
            }
        });
        let mut handlers = cx.into_handlers();

        let mut st = St::default();
        assert!(handlers.dispatch(tap, &mut st));
        assert_eq!(st.n, 5);

        assert!(handlers.dispatch_key(focus, &KeyInput::Text("hi".into()), &mut st));
        assert_eq!(st.s, "hi");
        assert!(!handlers.dispatch_key(FocusId(99), &KeyInput::Backspace, &mut st));
    }

    #[test]
    fn memo_skips_rebuild_for_unchanged_keys() {
        use std::cell::Cell;
        let theme = Theme::light();
        let builds = Cell::new(0);
        let make = |cx: &mut Cx<St>, key: u64| {
            cx.memo(key, || {
                builds.set(builds.get() + 1);
                Element::text("static", 14.0, Color::BLACK)
            })
        };

        // Frame 1: builds the subtree once.
        let mut cache = std::collections::HashMap::new();
        let mut cx = Cx::<St>::new(&theme);
        cx.set_memo_cache(cache);
        let _ = make(&mut cx, 1);
        cache = cx.take_memo_cache();
        assert_eq!(builds.get(), 1);
        assert!(cache.contains_key(&1));

        // Frame 2: same key → the closure is skipped (cache hit).
        let mut cx = Cx::<St>::new(&theme);
        cx.set_memo_cache(cache);
        let _ = make(&mut cx, 1);
        cache = cx.take_memo_cache();
        assert_eq!(builds.get(), 1, "unchanged key must not rebuild");

        // Frame 3: a different key rebuilds, and the now-unused key 1 is evicted.
        let mut cx = Cx::<St>::new(&theme);
        cx.set_memo_cache(cache);
        let _ = make(&mut cx, 2);
        cache = cx.take_memo_cache();
        assert_eq!(builds.get(), 2, "changed key rebuilds");
        assert!(cache.contains_key(&2));
        assert!(!cache.contains_key(&1), "stale key should be evicted");
    }

    #[test]
    fn hit_test_and_focus_prefer_topmost() {
        let root = LayoutNode {
            bounds: Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            decoration: BoxStyle::default(),
            content: NodeContent::None,
            action: Some(ActionId(0)),
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
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            rtl: false,
            transform: None,
            opacity: 1.0,
            key: None,
            ghost: false,
            children: vec![
                leaf(
                    Rect::from_xywh(10.0, 10.0, 30.0, 30.0),
                    Some(ActionId(1)),
                    Some(FocusId(0)),
                ),
                leaf(
                    Rect::from_xywh(20.0, 20.0, 30.0, 30.0),
                    Some(ActionId(2)),
                    Some(FocusId(1)),
                ),
            ],
        };
        assert_eq!(hit_test(&root, Point::new(25.0, 25.0)), Some(ActionId(2)));
        assert_eq!(focus_at(&root, Point::new(12.0, 12.0)), Some(FocusId(0)));

        let mut focusables = Vec::new();
        collect_focusables(&root, &mut focusables);
        assert_eq!(focusables, vec![FocusId(0), FocusId(1)]);
    }

    #[test]
    fn cursor_follows_handlers_and_explicit_choice() {
        let mut button = leaf(
            Rect::from_xywh(0.0, 0.0, 50.0, 20.0),
            Some(ActionId(0)),
            None,
        );
        // A label inside the button picks no cursor of its own.
        button
            .children
            .push(leaf(Rect::from_xywh(5.0, 5.0, 20.0, 10.0), None, None));
        let mut field = leaf(
            Rect::from_xywh(0.0, 30.0, 50.0, 20.0),
            None,
            Some(FocusId(0)),
        );
        field.text_pos = Some(TextPosId(0));
        let mut splitter = leaf(
            Rect::from_xywh(60.0, 0.0, 4.0, 50.0),
            Some(ActionId(1)),
            None,
        );
        splitter.cursor = Some(CursorIcon::EwResize);
        let root = LayoutNode::container(
            Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            vec![button, field, splitter],
        );
        assert_eq!(
            cursor_at(&root, Point::new(10.0, 10.0)),
            CursorIcon::Pointer
        );
        assert_eq!(cursor_at(&root, Point::new(10.0, 40.0)), CursorIcon::Text);
        assert_eq!(
            cursor_at(&root, Point::new(61.0, 10.0)),
            CursorIcon::EwResize
        );
        assert_eq!(
            cursor_at(&root, Point::new(90.0, 90.0)),
            CursorIcon::Default
        );
    }

    #[test]
    fn clipped_children_cannot_be_hit_outside_the_clip() {
        // A row scrolled up out of its clipping container, over a header.
        let mut row = leaf(
            Rect::from_xywh(0.0, 0.0, 100.0, 40.0),
            Some(ActionId(1)),
            None,
        );
        row.drag = Some(DragId(0));
        let mut body = leaf(Rect::from_xywh(0.0, 20.0, 100.0, 80.0), None, None);
        body.clip = true;
        body.children = vec![row];
        let header = leaf(
            Rect::from_xywh(0.0, 0.0, 100.0, 20.0),
            Some(ActionId(0)),
            None,
        );
        let root =
            LayoutNode::container(Rect::from_xywh(0.0, 0.0, 100.0, 100.0), vec![header, body]);
        assert_eq!(hit_test(&root, Point::new(10.0, 10.0)), Some(ActionId(0)));
        assert_eq!(hit_test(&root, Point::new(10.0, 30.0)), Some(ActionId(1)));
        assert!(drag_at(&root, Point::new(10.0, 10.0)).is_none());
    }

    #[test]
    fn context_handler_resolves_and_receives_the_click_point() {
        struct St {
            at: Option<Point>,
        }
        let theme = Theme::light();
        let mut cx = Cx::<St>::new(&theme);
        // A right-click handler that records where it was invoked.
        let id = cx.register_context(|s: &mut St, p: Point| s.at = Some(p));
        let mut handlers = cx.into_handlers();

        // A node carrying that context handle.
        let mut node = leaf(Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None, None);
        node.context = Some(id);

        // context_at finds it; a point outside misses.
        assert_eq!(context_at(&node, Point::new(50.0, 50.0)), Some(id));
        assert_eq!(context_at(&node, Point::new(150.0, 50.0)), None);

        // Dispatch passes the click position through to the handler.
        let mut st = St { at: None };
        assert!(handlers.dispatch_context(id, Point::new(12.0, 34.0), &mut st));
        assert_eq!(st.at, Some(Point::new(12.0, 34.0)));
    }
}
