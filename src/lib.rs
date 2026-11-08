//! Stipple — a cross-platform, self-drawn UI toolkit in Rust.
//!
//! This umbrella crate ties the layers together and provides the [`App`]
//! entry point and a [`prelude`]. See `ROADMAP.md` for the architecture and
//! phased plan.
//!
//! ```
//! use stipple::prelude::*;
//!
//! struct Counter {
//!     n: i64,
//! }
//!
//! let mut app = App::new(Counter { n: 0 }, |state: &Counter, cx: &mut Cx<Counter>| {
//!     let theme = *cx.theme();
//!     panel(
//!         &theme,
//!         vec![button(&theme).on_tap(cx, |s: &mut Counter| s.n += 1)],
//!     )
//! })
//! .title("Counter")
//! .logical_size(Size::new(360.0, 200.0));
//!
//! // Render one frame off-screen (no window needed) and inspect it.
//! let frame = app.render_once();
//! assert_eq!(frame.size().width, 360);
//! ```

#![deny(unsafe_code)] // `platform` and `gpu` opt in where they talk to the OS

pub mod anim;
pub mod core;
pub mod geometry;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod layout;
pub mod platform;
pub mod render;
pub mod style;
mod web_app;
pub mod widgets;

use crate::core::{
    ActionId, Anchor, AppHandle, BoxStyle, Cx, Damage, DragId, Element, FocusId, Handlers,
    KeyInput, LayoutNode, TextPosId, ViewportEvent, ViewportId, caret_index_at, collect_focusables,
    collect_viewports, context_at, drag_hit_at, find_action, find_focus, find_text_pos, focus_at,
    hit_test, layout_with_direction, measure, paint, paint_focus, paint_hover, text_pos_at,
    viewport_at,
};
use crate::geometry::{PhysicalSize, Point, Rect, ScaleFactor, Size};
use crate::platform::{
    ButtonState, ControlFlow, CursorIcon, Event, Ime, KeyCode, Modifiers, PointerButton,
    WindowAttributes, WindowId, backend,
};
use crate::render::{Color, Font, Pixmap, Scene, SoftwareRenderer, Surface};
use crate::style::Theme;
use std::collections::HashMap;

/// A Stipple application.
///
/// Owns the global app `state` and shared presentation (the active [`Theme`] and
/// font), plus one [`Pane`] per OS window. The primary window's `build` closure
/// is passed to [`App::new`]; further windows are added with
/// [`App::open_window`], each with its own view onto the same shared state.
/// Every pane maps state (via a [`Cx`] for registering event handlers) to an
/// [`Element`] tree; pointer/keyboard events route through that window's laid-out
/// tree back to the handlers the view registered.
///
/// [`App::run`] drives the platform backend (native X11 when `$DISPLAY` is set,
/// else a one-shot headless present) through the full build → layout → paint →
/// rasterize → present path for each window, opening every registered window as
/// a real native window where the backend supports it (X11) and ending when the
/// last one closes.
pub struct App<S> {
    // Global application state, shared by every window's view.
    state: S,
    // Shared presentation settings used by all windows.
    theme: Theme,
    font: Option<Font>,
    // One [`Pane`] per OS window. There is always at least one (the primary,
    // index 0); additional windows are added via [`App::open_window`].
    panes: Vec<Pane<S>>,
    // Externally-rendered content for embedded viewports, keyed by the
    // caller-chosen [`ViewportId`]. After a frame is rasterized, each viewport's
    // content (sized to the viewport's physical extent) is blitted over its
    // placeholder — the CPU analog of a GPU backend sampling an imported
    // (dma-buf / IOSurface / shared-handle) content texture into the rect.
    viewport_content: HashMap<ViewportId, Pixmap>,
    /// Updates and timers from outside widget handlers, applied between events.
    handle: AppHandle<S>,
    // Sink for input that lands in an embedded viewport (pointer/keys/wheel),
    // forwarded with viewport-local coordinates to the content process. `None`
    // = viewports are display-only and input routes to the Stipple chrome.
    viewport_input: Option<ViewportInputFn>,
    // The menu bar, as a function of the state (see [`App::menu`]).
    menu: Option<MenuFn<S>>,
    // Draw the menu bar in the window even where a native one exists.
    menu_in_window: bool,
    // The menu bar last handed to the native backend, so it's only rebuilt on
    // a change; and whether the backend turned out to have none.
    native_menu: Option<crate::platform::NativeMenuBar>,
    native_menu_unsupported: bool,
    // The backend has no native context menus: draw them.
    native_popup_unsupported: bool,
    // Alt went down with no other key since: its release focuses the menu bar.
    alt_armed: bool,
    // The tray icon, as a function of the state (see [`App::tray`]), and what
    // was last handed to the backend.
    tray: Option<TrayFn<S>>,
    native_tray: Option<crate::platform::NativeTray>,
}

/// A tray icon declared as a function of the app state (see [`App::tray`]).
pub type TrayFn<S> = std::rc::Rc<dyn Fn(&S) -> crate::core::Tray<S>>;

/// A menu bar declared as a function of the app state (see [`App::menu`]).
pub type MenuFn<S> = std::rc::Rc<dyn Fn(&S) -> crate::core::MenuBar<S>>;

/// A sink for input forwarded to an embedded viewport's content: receives the
/// target [`ViewportId`] and a [`ViewportEvent`] (pointer/keys/wheel) in
/// viewport-local coordinates. Set via [`App::on_viewport_input`].
pub type ViewportInputFn = Box<dyn FnMut(ViewportId, ViewportEvent)>;

/// A pointer transition forwarded to a viewport (internal helper for
/// [`App::try_forward_pointer`]).
#[derive(Clone, Copy)]
enum PointerKind {
    /// Pressed with the given button code (0 = left, 1 = right, 2 = middle).
    Down(u8),
    /// Released with the given button code.
    Up(u8),
    /// Moved over the viewport.
    Move,
}

/// A pluggable rasterizer: turns a built [`Scene`] (with a background color, at a
/// scale factor) into the [`Pixmap`] the [`Surface`] presents. Set via
/// [`App::render_with`] to route frames through a GPU backend.
pub type FrameRenderer = Box<dyn FnMut(&Scene, Color, ScaleFactor) -> Pixmap>;

/// A view closure mapping the shared state (and a [`Cx`] for registering event
/// handlers) to an [`Element`] tree. Boxed so each window can hold its own view.
pub type ViewFn<S> = Box<dyn FnMut(&S, &mut Cx<'_, S>) -> Element>;

/// The longest gap between presses that still counts as a multi-click. 500 ms
/// is the default double-click time on Windows, GTK and macOS; the platforms
/// let users change it, but Stipple doesn't read that setting (yet).
const MULTI_CLICK_TIME: std::time::Duration = std::time::Duration::from_millis(500);

/// How far (logical px, on each axis) the pointer may drift between presses of
/// a multi-click.
const MULTI_CLICK_SLOP: f64 = 4.0;

/// Counts consecutive presses at (nearly) the same spot within
/// [`MULTI_CLICK_TIME`]: 1 for a single click, 2 for a double, 3 for a triple;
/// a fourth press starts over at 1.
#[derive(Clone, Copy, Debug, Default)]
struct ClickCounter {
    last: Option<(crate::platform::time::Instant, Point)>,
    count: u32,
}

impl ClickCounter {
    /// Register a press at `pos` at time `now`, returning its click count.
    fn press(&mut self, now: crate::platform::time::Instant, pos: Point) -> u32 {
        let repeat = self.last.is_some_and(|(t, p)| {
            now.saturating_duration_since(t) <= MULTI_CLICK_TIME
                && (pos.x - p.x).abs() <= MULTI_CLICK_SLOP
                && (pos.y - p.y).abs() <= MULTI_CLICK_SLOP
        });
        self.count = if repeat && self.count < 3 {
            self.count + 1
        } else {
            1
        };
        self.last = Some((now, pos));
        self.count
    }

    /// Forget the previous press (e.g. it landed somewhere else).
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Logical-pixel margin added around a focused element's bounds so the damage
/// region covers the focus ring (a 2px stroke straddling the bounds).
const FOCUS_RING_MARGIN: f64 = 2.0;

/// Grow `r` by `m` logical pixels on every side.
fn inflate(r: Rect, m: f64) -> Rect {
    Rect::from_xywh(
        r.min_x() - m,
        r.min_y() - m,
        r.width() + 2.0 * m,
        r.height() + 2.0 * m,
    )
}

/// Fold `extra` damage rects into `base`. [`Damage::Full`] absorbs everything;
/// otherwise the rects extend (or become) the region list. An empty `extra`
/// leaves `base` unchanged.
fn merge_regions(base: Damage, extra: Vec<Rect>) -> Damage {
    if matches!(base, Damage::Full) || extra.is_empty() {
        return base;
    }
    let mut regions = match base {
        Damage::Regions(rs) => rs,
        Damage::None => Vec::new(),
        Damage::Full => unreachable!("handled above"),
    };
    regions.extend(extra);
    Damage::Regions(regions)
}

/// Everything specific to one OS window: its view onto the shared state, its
/// window attributes, and all the retained per-window render/event state (the
/// laid-out tree, handlers, focus/hover/drag/selection, scroll offsets, the
/// on-screen diff baseline, and an optional custom rasterizer).
///
/// Multiple panes read and mutate the same `App::state`, so they stay in sync;
/// each maintains its own input focus and damage tracking independently.
struct Pane<S> {
    view: ViewFn<S>,
    /// The app's handle, given to each frame's [`Cx`].
    handle: AppHandle<S>,
    attrs: WindowAttributes,
    scale: ScaleFactor,
    // Optional GPU (or other) rasterizer used in place of the software renderer
    // to turn each frame's `Scene` into the `Pixmap` that is presented. Lets a
    // GPU backend (stipple::gpu) drive on-screen present through the `Surface` seam
    // without stipple depending on it. `None` = software rasterization.
    frame_renderer: Option<FrameRenderer>,
    // Retained from the last frame build, for routing pointer events.
    tree: Option<LayoutNode>,
    handlers: Handlers<S>,
    pressed: Option<ActionId>,
    focused: Option<FocusId>,
    hovered: Option<ActionId>,
    // The latched drag: its id, its and its parent's bounds, the press point, and
    // whether it was laid out right to left.
    dragging: Option<(DragId, Rect, Rect, Point, bool)>,
    // The text element currently being selected by a pointer drag.
    text_selecting: Option<TextPosId>,
    // Consecutive presses on text, for double-click (word) and triple-click
    // (line) selection.
    clicks: ClickCounter,
    // Set when state/focus/hover changed so the next build rebuilds the tree.
    dirty: bool,
    // The tree + overlay state that is currently on screen, used as the diff
    // baseline so a present can be limited to the region that actually changed.
    presented: Option<LayoutNode>,
    // The full-window pixmap currently on screen (software path only). Retained
    // so a partial repaint can re-rasterize just the damaged rects into it and
    // present those, instead of rebuilding the whole frame's pixels.
    last_pixmap: Option<Pixmap>,
    // The hover/press highlights, fading between states, and the overlay alpha
    // each highlighted element was given in the frame just built and in the one
    // on screen (for damage).
    highlights: crate::core::highlight::Highlights,
    highlight_alphas: Vec<(ActionId, u8)>,
    painted_highlights: Vec<(ActionId, u8)>,
    // Whether the frame just built asked for another (something animates).
    animating: bool,
    painted_focused: Option<FocusId>,
    // Cross-frame memo cache for `Cx::memo` (static subtree reuse).
    memo_cache: std::collections::HashMap<u64, Element>,
    // Per-container scroll offsets (vertical, logical px), adjusted by wheel
    // events and re-applied + clamped each frame by `apply_scroll`.
    scroll_offsets: std::collections::HashMap<crate::core::ScrollId, f64>,
    // Scroll requests from handlers (`Cx::scroll_requests`), applied to
    // `scroll_offsets` before the next build.
    scroll_requests: crate::core::ScrollRequests,
    // Last pointer position, so a `Scroll` event (which carries only a delta)
    // can find the scroll container under the cursor.
    last_pointer: Point,
    // Whether the frame just built / on screen has overlay layers — overlays
    // can't be localized by the tree diff, so their presence forces full damage.
    overlay_active: bool,
    painted_overlay_active: bool,
    // The embedded viewport that currently holds input focus (set when its
    // content was last pressed), so keyboard input forwards to it until focus
    // moves elsewhere. `None` = the Stipple chrome owns keyboard focus.
    input_viewport: Option<ViewportId>,
    // The cursor last set on the window, so it is only changed on a change.
    cursor: CursorIcon,
    // What the window's input method was last told (`None`: disabled), so
    // `Window::set_ime` is only called on a change.
    ime_sent: Option<crate::platform::ImeState>,
    // The element showing input-method composition text, so it can be told to
    // drop it when focus moves away mid-composition.
    preedit_owner: Option<FocusId>,
    // The menus Stipple draws for this window: which is open and highlighted.
    menu_ui: std::rc::Rc<std::cell::RefCell<crate::widgets::menus::MenuUi>>,
    // The menu bar to draw at the top of the window (the primary window, where
    // the platform shows no native one).
    menu_bar: Option<MenuFn<S>>,
    // The elements with a context menu in the frame just built, and their
    // menus.
    context_menus: Vec<(crate::core::ContextId, crate::core::Menu<S>)>,
    // The dismiss actions of the overlays' press-outside catchers.
    catchers: Vec<ActionId>,
}

impl<S> std::fmt::Debug for App<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `state` and the view closures are not required to be Debug.
        f.debug_struct("App")
            .field("theme", &self.theme)
            .field("windows", &self.panes.len())
            .finish_non_exhaustive()
    }
}

impl<S: 'static> Pane<S> {
    /// Create a pane for `view` with default window attributes.
    fn new(view: ViewFn<S>, handle: AppHandle<S>) -> Self {
        Self {
            view,
            handle,
            attrs: WindowAttributes::new(),
            scale: ScaleFactor::IDENTITY,
            frame_renderer: None,
            tree: None,
            handlers: Handlers::default(),
            pressed: None,
            focused: None,
            hovered: None,
            dragging: None,
            text_selecting: None,
            clicks: ClickCounter::default(),
            dirty: true,
            presented: None,
            last_pixmap: None,
            highlights: Default::default(),
            highlight_alphas: Vec::new(),
            painted_highlights: Vec::new(),
            animating: false,
            painted_focused: None,
            ime_sent: None,
            preedit_owner: None,
            memo_cache: std::collections::HashMap::new(),
            scroll_offsets: std::collections::HashMap::new(),
            scroll_requests: crate::core::ScrollRequests::default(),
            last_pointer: Point::new(0.0, 0.0),
            overlay_active: false,
            painted_overlay_active: false,
            input_viewport: None,
            cursor: CursorIcon::Default,
            menu_ui: Default::default(),
            menu_bar: None,
            context_menus: Vec::new(),
            catchers: Vec::new(),
        }
    }
}

impl<S: 'static> App<S> {
    /// Create an app from initial `state` and a primary-window `build` closure.
    pub fn new(state: S, build: impl FnMut(&S, &mut Cx<'_, S>) -> Element + 'static) -> Self {
        let handle = AppHandle::new();
        Self {
            state,
            theme: Theme::light(),
            font: None,
            panes: vec![Pane::new(Box::new(build), handle.clone())],
            viewport_content: HashMap::new(),
            viewport_input: None,
            handle,
            menu: None,
            menu_in_window: false,
            native_menu: None,
            native_menu_unsupported: false,
            native_popup_unsupported: false,
            alt_armed: false,
            tray: None,
            native_tray: None,
        }
    }

    /// A handle for changing the state from timers and other threads, once the
    /// app runs. See [`AppHandle`].
    pub fn handle(&self) -> AppHandle<S> {
        self.handle.clone()
    }

    /// The primary window's pane (index 0), which the builder methods and the
    /// single-window public API operate on. Always present.
    fn primary(&mut self) -> &mut Pane<S> {
        &mut self.panes[0]
    }

    /// Register an additional OS window with its own `view` onto the shared
    /// state. Stipple drives every registered window once [`App::run`] starts.
    ///
    /// Note: true OS multi-window presentation is wired per backend (X11 first);
    /// on backends that don't yet support it only the primary window is shown.
    pub fn open_window(
        &mut self,
        attrs: WindowAttributes,
        view: impl FnMut(&S, &mut Cx<'_, S>) -> Element + 'static,
    ) {
        let mut pane = Pane::new(Box::new(view), self.handle.clone());
        pane.attrs = attrs;
        self.panes.push(pane);
    }

    /// Set the font used to render text. Without one, text elements are laid
    /// out as zero-size and not painted.
    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Route frame rasterization through a custom renderer — e.g. a GPU backend
    /// (`stipple::gpu`) that turns each frame's [`Scene`] into a [`Pixmap`] on the
    /// GPU, which is then presented through the platform [`Surface`]. This wires
    /// GPU rendering into the live present path without stipple depending on any
    /// GPU crate. Without it, frames are rasterized on the CPU.
    pub fn render_with(
        mut self,
        renderer: impl FnMut(&Scene, Color, ScaleFactor) -> Pixmap + 'static,
    ) -> Self {
        self.primary().frame_renderer = Some(Box::new(renderer));
        self
    }

    /// Set the window title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.primary().attrs.title = title.into();
        self
    }

    /// Set the initial logical window size.
    pub fn logical_size(mut self, size: Size) -> Self {
        self.primary().attrs.logical_size = size;
        self
    }

    /// Set the smallest logical size the user can resize the window to.
    pub fn min_size(mut self, size: Size) -> Self {
        self.primary().attrs.min_size = Some(size);
        self
    }

    /// Set the largest logical size the user can resize the window to.
    pub fn max_size(mut self, size: Size) -> Self {
        self.primary().attrs.max_size = Some(size);
        self
    }

    /// Open the window maximized.
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.primary().attrs.maximized = maximized;
        self
    }

    /// Open the window fullscreen.
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.primary().attrs.fullscreen = fullscreen;
        self
    }

    /// Set the window icon from a straight-alpha RGBA image (on macOS, the
    /// application's Dock icon). See [`WindowAttributes::icon`].
    pub fn icon(mut self, icon: Pixmap) -> Self {
        self.primary().attrs.icon = Some(icon);
        self
    }

    /// Declare the application's menu bar, as a function of the state: it is
    /// rebuilt as the state changes, so labels and enabled and checked states
    /// follow it. Picking an item (or pressing its shortcut, from any window)
    /// runs its action against the state.
    ///
    /// macOS shows it as the native menu bar, with the standard application
    /// menu (About, Hide, Quit) in front; Windows as the window's native menu
    /// bar; elsewhere Stipple draws it across the top of the primary window.
    /// See `docs/menus.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Doc { dirty: bool }
    /// let app = App::new(Doc { dirty: true }, |_: &Doc, cx: &mut Cx<Doc>| {
    ///     let theme = *cx.theme();
    ///     label(&theme, "Hello")
    /// })
    /// .menu(|doc: &Doc| {
    ///     MenuBar::new().menu(
    ///         Menu::new("&File").item(
    ///             MenuItem::new("&Save", |d: &mut Doc| d.dirty = false)
    ///                 .shortcut(Shortcut::cmd('s'))
    ///                 .enabled(doc.dirty),
    ///         ),
    ///     )
    /// });
    /// ```
    pub fn menu(mut self, menu: impl Fn(&S) -> crate::core::MenuBar<S> + 'static) -> Self {
        let menu: MenuFn<S> = std::rc::Rc::new(menu);
        self.panes[0].menu_bar = Some(menu.clone());
        self.menu = Some(menu);
        self
    }

    /// Give the app a tray icon: a status item on macOS, a notification-area
    /// icon on Windows, a `StatusNotifierItem` on Linux (which needs a desktop
    /// that hosts one). Like [`menu`](App::menu), `tray` is a function of the
    /// state, called again as it changes; its menu's picks and its click
    /// action run against the state. Not shown on iOS, Android or the web.
    /// See `docs/os-integration.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct State { muted: bool }
    /// let app = App::new(State { muted: false }, |_: &State, cx: &mut Cx<State>| {
    ///     let theme = *cx.theme();
    ///     label(&theme, "Hello")
    /// })
    /// .tray(|s: &State| {
    ///     Tray::new("My App").menu(
    ///         Menu::new("").item(MenuItem::new("&Mute", |s: &mut State| s.muted = !s.muted).checked(s.muted)),
    ///     )
    /// });
    /// ```
    pub fn tray(mut self, tray: impl Fn(&S) -> crate::core::Tray<S> + 'static) -> Self {
        self.tray = Some(std::rc::Rc::new(tray));
        self
    }

    /// Draw the menus in the window, as on Linux, even on macOS and Windows
    /// where the platform would show native ones: the menu bar, and the
    /// context menus (which are otherwise the system's popup menus there).
    /// For menus in a custom theme's colors.
    pub fn menu_bar_in_window(mut self, in_window: bool) -> Self {
        self.menu_in_window = in_window;
        self
    }

    /// Set the active theme.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Override the DPI scale used for off-screen rendering (default 1×).
    pub fn scale(mut self, scale: ScaleFactor) -> Self {
        self.primary().scale = scale;
        self
    }

    /// Access the current state.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Seed the content for an embedded [`viewport`](Element::viewport) at build
    /// time (builder form of [`set_viewport_content`](App::set_viewport_content)).
    pub fn with_viewport_content(mut self, id: ViewportId, content: Pixmap) -> Self {
        self.viewport_content.insert(id, content);
        self
    }

    /// Register (or replace) the externally-rendered `content` for the viewport
    /// `id`. After each frame is rasterized, this content — which should be sized
    /// to the viewport's physical extent — is composited over the viewport's
    /// placeholder. This is the CPU content path; a GPU backend imports a shared
    /// texture (dma-buf / `IOSurface` / shared handle) into the same rect
    /// instead. Marks every window dirty so the change is presented next frame.
    pub fn set_viewport_content(&mut self, id: ViewportId, content: Pixmap) {
        self.viewport_content.insert(id, content);
        for pane in &mut self.panes {
            pane.dirty = true;
        }
    }

    /// Remove any registered content for the viewport `id` (reverting it to the
    /// placeholder). Marks every window dirty.
    pub fn clear_viewport_content(&mut self, id: ViewportId) {
        if self.viewport_content.remove(&id).is_some() {
            for pane in &mut self.panes {
                pane.dirty = true;
            }
        }
    }

    /// Forward input that lands in an embedded [`viewport`](Element::viewport) to
    /// `sink` instead of routing it to the Stipple chrome. The sink receives the
    /// target [`ViewportId`] and a [`ViewportEvent`] (pointer press/release/move,
    /// wheel, or — while the viewport holds input focus — keys) in viewport-local
    /// coordinates, so a sandboxed content process can handle it. Pressing a
    /// viewport's content gives it keyboard focus (dropping any Stipple focus)
    /// until a press lands elsewhere. Typically the sink feeds the content
    /// process, which renders a new frame and calls
    /// [`set_viewport_content`](App::set_viewport_content).
    pub fn on_viewport_input(
        mut self,
        sink: impl FnMut(ViewportId, ViewportEvent) + 'static,
    ) -> Self {
        self.viewport_input = Some(Box::new(sink));
        self
    }

    /// If `pos` lands in a viewport and an input sink is registered, forward the
    /// pointer transition `kind` to it (in viewport-local coordinates) and return
    /// `true` — the event is consumed by the embedded content. A `Down` also
    /// gives the viewport keyboard focus (dropping any Stipple focus).
    fn try_forward_pointer(&mut self, idx: usize, pos: Point, kind: PointerKind) -> bool {
        if self.viewport_input.is_none() {
            return false;
        }
        let Some(tree) = self.panes[idx].tree.as_ref() else {
            return false;
        };
        let Some((id, bounds)) = viewport_at(tree, pos) else {
            return false;
        };
        let local = Point::new(pos.x - bounds.min_x(), pos.y - bounds.min_y());
        let event = match kind {
            PointerKind::Down(button) => {
                self.panes[idx].input_viewport = Some(id);
                self.panes[idx].focused = None;
                ViewportEvent::PointerDown { local, button }
            }
            PointerKind::Up(button) => ViewportEvent::PointerUp { local, button },
            PointerKind::Move => ViewportEvent::PointerMove { local },
        };
        if let Some(sink) = self.viewport_input.as_mut() {
            sink(id, event);
        }
        true
    }

    /// If `pos` lands in a viewport with an input sink, forward a wheel scroll of
    /// `dy` logical pixels (viewport-local) and return `true` (consumed).
    fn try_forward_wheel(&mut self, idx: usize, pos: Point, dy: f64) -> bool {
        if self.viewport_input.is_none() {
            return false;
        }
        let Some(tree) = self.panes[idx].tree.as_ref() else {
            return false;
        };
        let Some((id, bounds)) = viewport_at(tree, pos) else {
            return false;
        };
        let local = Point::new(pos.x - bounds.min_x(), pos.y - bounds.min_y());
        if let Some(sink) = self.viewport_input.as_mut() {
            sink(id, ViewportEvent::Wheel { local, delta_y: dy });
        }
        true
    }

    /// If a viewport currently holds input focus (its content was last pressed),
    /// forward keyboard `input` to it and return `true` (consumed). The Stipple
    /// chrome doesn't see the key.
    fn try_forward_key(&mut self, idx: usize, input: KeyInput) -> bool {
        let Some(id) = self.panes[idx].input_viewport else {
            return false;
        };
        if let Some(sink) = self.viewport_input.as_mut() {
            sink(id, ViewportEvent::Key(input));
            true
        } else {
            false
        }
    }
}

impl<S: 'static> Pane<S> {
    /// Rasterize a built `scene` into a `Pixmap` — through the custom
    /// [`render_with`](App::render_with) renderer if set, else the software path.
    fn rasterize(&mut self, scene: Scene, theme: &Theme, scale: ScaleFactor) -> Pixmap {
        let bg = theme.palette.background;
        match self.frame_renderer.as_mut() {
            Some(render) => render(&scene, bg, scale),
            None => SoftwareRenderer::new()
                .with_background(bg)
                .render(scene, scale),
        }
    }

    /// Composite registered embedded content over each viewport's placeholder in
    /// `pixmap`. For every viewport in the laid-out tree with content registered
    /// for its id, blit that content — assumed sized to the viewport's physical
    /// extent — at the viewport's physical top-left. A straight replace over the
    /// placeholder the scene painted; the CPU analog of a GPU backend sampling an
    /// imported content texture into the rect.
    fn composite_viewports(
        &self,
        pixmap: &mut Pixmap,
        content: &HashMap<ViewportId, Pixmap>,
        scale: ScaleFactor,
    ) {
        if content.is_empty() {
            return;
        }
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let mut views = Vec::new();
        collect_viewports(tree, &mut views);
        let s = scale.get();
        for (id, bounds) in views {
            if let Some(src) = content.get(&id) {
                let x = (bounds.min_x() * s).round() as u32;
                let y = (bounds.min_y() * s).round() as u32;
                pixmap.blit(src, x, y);
            }
        }
    }

    /// Whether the laid-out tree contains any embedded-content viewport. Content
    /// is composited post-rasterize (not via the area-repaint sub-renderer), so a
    /// frame with a viewport opts out of partial repaint to avoid a damaged rect
    /// over the viewport erasing its content back to the placeholder.
    fn has_viewports(&self) -> bool {
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        let mut views = Vec::new();
        collect_viewports(tree, &mut views);
        !views.is_empty()
    }

    /// Build the element tree for the current state, lay it out to fill the
    /// window, retain the layout tree + handlers for event routing, and return
    /// the painted [`Scene`].
    fn build_frame(&mut self, state: &S, theme: &Theme, font: Option<&Font>) -> Scene {
        let theme = *theme; // Theme is Copy; avoids borrowing through `theme` in `cx`.
        let mut cx = Cx::with_handle(&theme, self.handle.clone());
        cx.set_memo_cache(std::mem::take(&mut self.memo_cache));
        for (id, offset) in self.scroll_requests.take() {
            self.scroll_offsets.insert(id, offset.max(0.0));
        }
        cx.set_scroll_state(self.scroll_offsets.clone(), self.scroll_requests.clone());
        let mut element = (self.view)(state, &mut cx);
        // The menus Stipple draws: the menu bar above the view, and the open
        // bar menu or context menu as overlays above everything else.
        match self.menu_bar.clone() {
            Some(menu) => {
                let bar = menu(state);
                element =
                    crate::widgets::menus::with_bar(&mut cx, &theme, &bar, &self.menu_ui, element);
            }
            None => {
                let mut ui = self.menu_ui.borrow_mut();
                if matches!(ui.root, Some(crate::widgets::menus::MenuRoot::Bar(_)))
                    || ui.bar_focus.is_some()
                {
                    ui.close();
                }
            }
        }
        self.context_menus = cx.take_context_menus();
        crate::widgets::menus::context(&mut cx, &theme, &self.context_menus, &self.menu_ui);
        // The animation timeline, or `None` where there's no clock.
        let now = crate::platform::frame::now().map(|_| cx.frame_time());
        let frame_requested = cx.frame_requested();
        self.memo_cache = cx.take_memo_cache();
        let overlays = cx.take_overlays();
        self.handlers = cx.into_handlers();

        let size = self.attrs.logical_size;
        let win = Rect::from_xywh(0.0, 0.0, size.width, size.height);
        // A right-to-left theme mirrors the view and its overlays.
        let dir = theme.layout_direction;
        let rtl = dir.is_rtl();
        let main = layout_with_direction(&element, win, font, dir);

        // Compose the main tree with any floating overlay layers under one
        // routable/paintable root: each overlay (with a scrim behind a modal
        // one) is a later child, so it paints on top and hit-tests first.
        let mut tree = if overlays.is_empty() {
            main
        } else {
            let mut roots = Vec::with_capacity(overlays.len() * 2 + 1);
            roots.push(main);
            for spec in &overlays {
                // A full-window catcher behind the overlay: a dark scrim for a
                // modal, or an invisible click-catcher for a non-modal that wants
                // outside-press dismissal. Either way it carries the dismiss
                // action and blocks clicks from reaching the tree below.
                if spec.modal || spec.dismiss.is_some() {
                    let mut catcher = Element::boxed(BoxStyle {
                        fill: spec.modal.then(|| Color::rgba(0, 0, 0, 0x80)),
                        radius: 0.0,
                        border: None,
                    })
                    .width(size.width)
                    .height(size.height);
                    catcher.action = spec.dismiss;
                    // The plain arrow over the scrim, not the tap target's hand
                    // or whatever lies beneath.
                    catcher.cursor = Some(CursorIcon::Default);
                    roots.push(layout_with_direction(&catcher, win, font, dir));
                }
                let mut m = measure(&spec.content, size, font);
                // Right to left, popups are placed in a mirrored frame (x from
                // the window's right edge): a menu opens leftwards from the
                // pointer, a submenu left of its item, a dropdown lines up with
                // its button's right edge. `At` and `Center` stay as they are.
                let flip_x = |x: f64| if rtl { size.width - x } else { x };
                let flip = |r: Rect| {
                    if rtl {
                        Rect::from_xywh(size.width - r.max_x(), r.min_y(), r.width(), r.height())
                    } else {
                        r
                    }
                };
                let origin = match spec.anchor {
                    Anchor::At(p) => p,
                    Anchor::Center => Point::new(
                        ((size.width - m.width) / 2.0).max(0.0),
                        ((size.height - m.height) / 2.0).max(0.0),
                    ),
                    Anchor::Popup(p) => {
                        let p = Point::new(flip_x(p.x), p.y);
                        // Flip left or up when the menu would cross the edge.
                        let x = if p.x + m.width > size.width && p.x - m.width >= 0.0 {
                            p.x - m.width
                        } else {
                            p.x.min(size.width - m.width)
                        };
                        let y = if p.y + m.height > size.height && p.y - m.height >= 0.0 {
                            p.y - m.height
                        } else {
                            p.y.min(size.height - m.height)
                        };
                        Point::new(x.max(0.0), y.max(0.0))
                    }
                    Anchor::RightOf(id) => match roots.iter().find_map(|r| find_action(r, id)) {
                        Some(node) => {
                            let b = flip(node.bounds);
                            // Line the first row up with the item: back off the
                            // panel's padding.
                            let inset = 4.0;
                            let x =
                                if b.max_x() + m.width > size.width && b.min_x() - m.width >= 0.0 {
                                    b.min_x() - m.width
                                } else {
                                    b.max_x().min(size.width - m.width)
                                };
                            let y = (b.min_y() - inset).min(size.height - m.height);
                            Point::new(x.max(0.0), y.max(0.0))
                        }
                        None => Point::new(0.0, 0.0),
                    },
                    Anchor::Below(id) => match find_action(&roots[0], id) {
                        Some(node) => {
                            let b = flip(node.bounds);
                            m.width = m.width.max(b.width());
                            let below = b.max_y() + 2.0;
                            let y = if below + m.height > size.height
                                && b.min_y() - 2.0 - m.height >= 0.0
                            {
                                b.min_y() - 2.0 - m.height
                            } else {
                                below
                            };
                            let x = b.min_x().min(size.width - m.width).max(0.0);
                            Point::new(x, y)
                        }
                        None => Point::new(0.0, 0.0),
                    },
                    Anchor::Aligned {
                        horizontal,
                        vertical,
                        margin,
                    } => {
                        let place = |align, free: f64| match align {
                            crate::core::Align::Center => (free / 2.0).max(0.0),
                            crate::core::Align::End => (free - margin).max(0.0),
                            crate::core::Align::Start | crate::core::Align::Stretch => margin,
                        };
                        Point::new(
                            place(horizontal, size.width - m.width),
                            place(vertical, size.height - m.height),
                        )
                    }
                };
                let origin = match spec.anchor {
                    Anchor::At(_) | Anchor::Center => origin,
                    _ if rtl => Point::new(size.width - origin.x - m.width, origin.y),
                    _ => origin,
                };
                let bounds = Rect::from_xywh(origin.x, origin.y, m.width, m.height);
                roots.push(layout_with_direction(&spec.content, bounds, font, dir));
            }
            LayoutNode::container(win, roots)
        };
        self.overlay_active = !overlays.is_empty();
        self.catchers = overlays
            .iter()
            .filter(|o| o.modal || o.dismiss.is_some())
            .filter_map(|o| o.dismiss)
            .collect();
        // A press-outside catcher covers the window: it takes clicks but
        // shows no hover or press highlight.
        let lit = |id: Option<ActionId>| id.filter(|id| !self.catchers.contains(id));
        let (hovered, pressed) = (lit(self.hovered), lit(self.pressed));
        // Apply (and clamp) scroll-container offsets to the laid-out tree before
        // painting, so the retained tree matches what's drawn for event routing.
        crate::core::apply_scroll(&mut tree, &mut self.scroll_offsets);
        crate::core::mark_native_states(&mut tree, hovered, pressed, self.focused);
        let mut scene = Scene::new(size);
        scene.set_scale(self.scale.get());
        paint(&tree, &mut scene, font);
        // Lighten the hovered tappable element with the theme's overlay, twice
        // over while pressed, fading between states.
        self.highlights.update(hovered, pressed, now);
        let overlay = theme.palette.hover_overlay;
        self.highlight_alphas = self
            .highlights
            .levels(now)
            .into_iter()
            .map(|(id, level)| (id, (overlay.a as f64 * level).round().min(255.0) as u8))
            .filter(|(_, a)| *a > 0)
            .collect();
        for &(id, a) in &self.highlight_alphas {
            paint_hover(&tree, id, &mut scene, Color { a, ..overlay });
        }
        self.animating = frame_requested || self.highlights.animating(now);
        if self.animating {
            crate::platform::frame::request();
        }
        // Overlay a focus ring + caret on the focused element.
        if let Some(fid) = self.focused {
            paint_focus(
                &tree,
                fid,
                &mut scene,
                font,
                theme.palette.focus_ring,
                theme.palette.text,
                theme.palette.selection,
            );
            crate::core::paint_preedit(&tree, fid, &mut scene, font, theme.palette.text);
        }
        self.tree = Some(tree);
        self.dirty = false;
        scene
    }

    /// Compute the [`Damage`] of the frame just built (in `self.tree`) relative
    /// to what is currently on screen (`self.presented`), then adopt the new
    /// frame as the on-screen baseline.
    ///
    /// The tree diff localizes content changes. The hover highlight and focus
    /// ring paint *outside* the tree, so a change to either is localized
    /// separately: the union of the affected element's old and new bounds is
    /// merged into the damage (the focus rect is inflated to cover the ring
    /// stroke). The first frame, a root-size (resize) change, a floating-overlay
    /// frame, and any case where a referenced node can't be found all fall back
    /// to [`Damage::Full`].
    fn take_damage(&mut self) -> Damage {
        // Floating overlays paint above the tree and can't be localized by the
        // diff, so any frame with overlays (or that just had them) is full.
        let overlay_changed = self.overlay_active || self.painted_overlay_active;
        let hover_changed = self.highlight_alphas != self.painted_highlights;
        let focus_changed = self.focused != self.painted_focused;

        let damage = match (&self.presented, &self.tree) {
            (Some(old), Some(new)) if !overlay_changed && old.bounds == new.bounds => {
                let base = crate::core::diff_trees(old, new);
                match self.overlay_damage_rects(old, new, hover_changed, focus_changed) {
                    Some(rects) => merge_regions(base, rects),
                    None => Damage::Full, // a referenced node vanished; repaint all
                }
            }
            _ => Damage::Full,
        };
        self.presented = self.tree.clone();
        self.painted_highlights = self.highlight_alphas.clone();
        self.painted_focused = self.focused;
        self.painted_overlay_active = self.overlay_active;
        damage
    }

    /// Logical rects whose hover-highlight or focus-ring overlay changed between
    /// the on-screen frame (`old`, carrying the `painted_*` ids) and the new
    /// frame (`new`, carrying the current ids). Returns the (possibly empty) set
    /// of rects, or `None` if a referenced node is missing — which the caller
    /// treats as [`Damage::Full`].
    fn overlay_damage_rects(
        &self,
        old: &LayoutNode,
        new: &LayoutNode,
        hover_changed: bool,
        focus_changed: bool,
    ) -> Option<Vec<Rect>> {
        let mut rects = Vec::new();
        if hover_changed {
            // Every element whose highlight changed, where it was and where it is.
            let (was, is) = (&self.painted_highlights, &self.highlight_alphas);
            for &(id, a) in was {
                if !is.contains(&(id, a)) {
                    rects.push(find_action(old, id)?.bounds);
                }
            }
            for &(id, a) in is {
                if !was.contains(&(id, a)) {
                    rects.push(find_action(new, id)?.bounds);
                }
            }
        }
        if focus_changed {
            // The focus ring is a 2px stroke centered on the bounds, so inflate
            // to cover the half that sits outside (plus a safety margin).
            if let Some(id) = self.painted_focused {
                rects.push(inflate(find_focus(old, id)?.bounds, FOCUS_RING_MARGIN));
            }
            if let Some(id) = self.focused {
                rects.push(inflate(find_focus(new, id)?.bounds, FOCUS_RING_MARGIN));
            }
        }
        Some(rects)
    }

    /// Build, paint, and rasterize the next frame, returning the [`Pixmap`] and
    /// the [`Damage`] (changed region, in logical pixels) relative to the
    /// previously returned frame. The first call always reports [`Damage::Full`].
    fn render_frame(
        &mut self,
        state: &S,
        theme: &Theme,
        font: Option<&Font>,
        content: &HashMap<ViewportId, Pixmap>,
    ) -> (Pixmap, Damage) {
        let scene = self.build_frame(state, theme, font);
        let scale = self.scale;
        let mut pixmap = self.rasterize(scene, theme, scale);
        self.composite_viewports(&mut pixmap, content, scale);
        let damage = self.take_damage();
        (pixmap, damage)
    }

    /// Build the [accessibility tree](crate::core::accessibility_tree) for the
    /// current frame, or `None` until one has been built.
    fn accessibility_tree(&self) -> Option<crate::core::AccessNode> {
        self.tree
            .as_ref()
            .map(|t| crate::core::accessibility_tree(t, self.focused))
    }

    fn ensure_tree(&mut self, state: &S, theme: &Theme, font: Option<&Font>) {
        if self.tree.is_none() || self.dirty {
            let _ = self.build_frame(state, theme, font);
        }
    }

    /// Route a completed click at `pos` (logical pixels): update keyboard focus
    /// to the focusable under the cursor (if any), then dispatch the hit
    /// element's tap handler. Returns `true` if a tap handler ran.
    fn click_at(&mut self, state: &mut S, theme: &Theme, font: Option<&Font>, pos: Point) -> bool {
        self.ensure_tree(state, theme, font);
        // With a bar menu open, a click on another title switches menus (and on
        // its own title closes it) instead of landing on the dismiss catcher.
        if let Some(index) = self.menu_title_at(pos) {
            self.menu_ui.borrow_mut().toggle_bar(index);
            self.dirty = true;
            return true;
        }
        let overlays = self.overlay_active;
        let focused = self.focused;
        let (hit, foc) = self
            .tree
            .as_ref()
            .map(|t| {
                // A press inside an overlay (or on its dismiss catcher) leaves
                // focus where it was unless it lands on a focusable there.
                let (layer, main) = press_layer(t, overlays, pos);
                let foc = focus_at(layer, pos);
                (
                    hit_test(layer, pos),
                    if main { foc } else { foc.or(focused) },
                )
            })
            .unwrap_or((None, None));

        // Clicking moves focus (to the focusable under the cursor, or away).
        if foc != self.focused {
            self.focused = foc;
            self.dirty = true;
        }
        match hit {
            Some(id) => {
                crate::core::set_pointer_position(pos, None);
                let ran = self.handlers.dispatch(id, state);
                if ran {
                    self.dirty = true;
                }
                ran
            }
            None => false,
        }
    }

    /// Route a secondary (right) click at `pos` to the context handler under the
    /// cursor, passing the click position (so it can open a context menu there).
    /// Returns `true` if a handler ran.
    fn context_at(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
    ) -> bool {
        self.ensure_tree(state, theme, font);
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        // A right-click inside a menu or dialog looks there; one on a
        // non-modal overlay's catcher (outside an open menu) closes the menu
        // and looks in the main tree; a modal scrim blocks it.
        let (layer, main) = press_layer(tree, self.overlay_active, pos);
        let catcher = !main && layer.bounds == tree.bounds;
        let target = match (catcher, layer.decoration.fill) {
            (true, Some(_)) => None,
            (true, None) => tree.children.first(),
            (false, _) => Some(layer),
        };
        let id = target.and_then(|t| context_at(t, pos));
        let closed = catcher && self.menu_ui.borrow().root.is_some();
        if closed {
            self.menu_ui.borrow_mut().close();
            self.dirty = true;
        }
        let Some(id) = id else {
            return closed;
        };
        if self.context_menus.iter().any(|(m, _)| *m == id) {
            self.menu_ui.borrow_mut().open_context(id, pos, false);
        }
        self.handlers.dispatch_context(id, pos, state);
        self.dirty = true;
        true
    }

    /// Open the context menu of the element under the pointer (or of the
    /// focused element, below it) from the keyboard: the Menu key or
    /// Shift+F10. Returns whether one opened.
    fn context_by_key(&mut self, state: &mut S, theme: &Theme, font: Option<&Font>) -> bool {
        self.ensure_tree(state, theme, font);
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        let main = if self.overlay_active {
            match tree.children.first() {
                Some(main) => main,
                None => return false,
            }
        } else {
            tree
        };
        let pos = self.last_pointer;
        let hit = context_at(main, pos).map(|id| (id, pos)).or_else(|| {
            let node = find_focus(main, self.focused?)?;
            Some((
                node.context?,
                Point::new(node.bounds.min_x(), node.bounds.max_y()),
            ))
        });
        let Some((id, at)) = hit else {
            return false;
        };
        if self.context_menus.iter().any(|(m, _)| *m == id) {
            self.menu_ui.borrow_mut().open_context(id, at, true);
        }
        self.handlers.dispatch_context(id, at, state);
        self.dirty = true;
        true
    }

    /// The tap target at `pos`: in the topmost layer there, so a press inside
    /// an overlay never falls through to its catcher or the tree below.
    fn tap_at(&self, pos: Point) -> Option<ActionId> {
        let tree = self.tree.as_ref()?;
        hit_test(press_layer(tree, self.overlay_active, pos).0, pos)
    }

    /// With a bar menu open, the menu bar title under `pos`, if any.
    fn menu_title_at(&self, pos: Point) -> Option<usize> {
        let ui = self.menu_ui.borrow();
        if !matches!(ui.root, Some(crate::widgets::menus::MenuRoot::Bar(_))) {
            return None;
        }
        let tree = self.tree.as_ref()?;
        ui.title_actions()
            .position(|id| find_action(tree, id).is_some_and(|n| n.bounds.contains(pos)))
    }

    /// Whether the focused element edits text, so editing shortcuts (Ctrl+C,
    /// Ctrl+Z…) go to it before the menu bar's.
    fn focused_is_editable(&self) -> bool {
        fn edits(n: &LayoutNode) -> bool {
            n.text_pos.is_some() || n.children.iter().any(edits)
        }
        match (self.focused, self.tree.as_ref()) {
            (Some(id), Some(tree)) => find_focus(tree, id).is_some_and(edits),
            _ => false,
        }
    }

    /// Deliver committed `text` to the focused element. Returns `true` if a
    /// focused key handler consumed it.
    fn type_text(&mut self, state: &mut S, theme: &Theme, font: Option<&Font>, text: &str) -> bool {
        self.send_key(state, theme, font, KeyInput::Text(text.to_string()))
    }

    /// Deliver an editing key (backspace, arrows, …) to the focused element.
    fn press_key(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        input: KeyInput,
    ) -> bool {
        self.send_key(state, theme, font, input)
    }

    fn send_key(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        input: KeyInput,
    ) -> bool {
        self.ensure_tree(state, theme, font);
        let Some(id) = self.focused else { return false };
        let ran = self.handlers.dispatch_key(id, &input, state);
        if ran {
            self.dirty = true;
        }
        ran
    }

    /// If focus left the element showing composition text, tell it to drop
    /// the preedit (the input method is disabled with the focus change, and
    /// may never end the composition itself).
    fn drop_stale_preedit(&mut self, state: &mut S) {
        if let Some(owner) = self.preedit_owner
            && Some(owner) != self.focused
        {
            let clear = KeyInput::Preedit {
                text: String::new(),
                cursor: None,
            };
            self.handlers.dispatch_key(owner, &clear, state);
            self.preedit_owner = None;
            self.dirty = true;
        }
    }

    /// Tell the window's input method about the focused text field (or that
    /// there is none), when that changed since the last frame.
    fn sync_ime(&mut self, window: &dyn crate::platform::Window, font: Option<&Font>) {
        let ime = match (&self.tree, self.input_viewport) {
            (Some(tree), None) => crate::core::ime_state(tree, self.focused, font),
            _ => None,
        };
        if ime != self.ime_sent {
            window.set_ime(ime.as_ref());
            self.ime_sent = ime;
        }
    }

    /// Move focus to the next focusable element in tree order (wrapping),
    /// like pressing Tab. Returns `true` if focus changed.
    fn focus_next(&mut self, state: &S, theme: &Theme, font: Option<&Font>) -> bool {
        self.ensure_tree(state, theme, font);
        let mut order = Vec::new();
        if let Some(t) = self.tree.as_ref() {
            collect_focusables(t, &mut order);
        }
        if order.is_empty() {
            return false;
        }
        let next = match self.focused {
            Some(cur) => match order.iter().position(|f| *f == cur) {
                Some(i) => order[(i + 1) % order.len()],
                None => order[0],
            },
            None => order[0],
        };
        let changed = self.focused != Some(next);
        self.focused = Some(next);
        self.dirty = true;
        changed
    }

    /// Begin or continue a pointer drag at `pos` (logical pixels). On the first
    /// call it latches onto the draggable element under the cursor; subsequent
    /// calls feed it the pointer's fractional x position until [`App::end_drag`].
    /// Returns `true` if a drag handler ran.
    fn drag_at_point(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
    ) -> bool {
        self.ensure_tree(state, theme, font);
        if self.dragging.is_none() {
            let overlays = self.overlay_active;
            self.dragging = self
                .tree
                .as_ref()
                .and_then(|t| drag_hit_at(press_layer(t, overlays, pos).0, pos))
                .map(|(id, bounds, parent, rtl)| (id, bounds, parent, pos, rtl));
        }
        let Some((id, bounds, parent, origin, rtl)) = self.dragging else {
            return false;
        };
        let mut motion = crate::core::DragMotion {
            position: pos,
            origin,
            bounds,
            parent,
        };
        // A mirrored element's handler sees the drag as it would left to
        // right: a slider fills from the right, a divider follows the pointer.
        if rtl {
            motion = motion.mirrored();
        }
        crate::core::set_pointer_position(pos, Some(origin));
        let ran = self.handlers.dispatch_drag_motion(id, &motion, state);
        if ran {
            self.dirty = true;
        }
        ran
    }

    /// End the current drag (pointer released).
    fn end_drag(&mut self) {
        self.dragging = None;
    }

    /// Begin a pointer text interaction at `pos` (logical pixels) pressed at
    /// time `now`: if an editable text element is under the cursor, focus it,
    /// resolve the caret byte index from the pointer x, place the caret there
    /// (clearing any selection), and latch a drag-selection. A double-click
    /// then selects the word there and a triple-click the line (sent to the
    /// focused element as [`KeyInput::SelectWord`] / [`KeyInput::SelectLine`]).
    /// Returns `true` if a text element was hit.
    fn text_press_at(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
        now: crate::platform::time::Instant,
    ) -> bool {
        self.ensure_tree(state, theme, font);
        let overlays = self.overlay_active;
        let Some((id, index, foc)) = self.tree.as_ref().and_then(|t| {
            let t = press_layer(t, overlays, pos).0;
            let (id, node) = text_pos_at(t, pos)?;
            let index = caret_index_at(node, pos, font)?;
            Some((id, index, focus_at(t, pos)))
        }) else {
            self.clicks.reset();
            return false;
        };
        if foc != self.focused {
            self.focused = foc;
        }
        self.text_selecting = Some(id);
        self.handlers.dispatch_text_pos(id, index, false, state);
        match self.clicks.press(now, pos) {
            2 => {
                self.send_key(state, theme, font, KeyInput::SelectWord);
            }
            3 => {
                self.send_key(state, theme, font, KeyInput::SelectLine);
            }
            _ => {}
        }
        self.dirty = true;
        true
    }

    /// Continue a latched text drag-selection at `pos`: resolve the caret index
    /// from the pointer x and extend the selection to it. Returns `true` if a
    /// selection is active and was updated.
    fn text_drag_at(
        &mut self,
        state: &mut S,
        theme: &Theme,
        font: Option<&Font>,
        pos: Point,
    ) -> bool {
        let Some(id) = self.text_selecting else {
            return false;
        };
        self.ensure_tree(state, theme, font);
        let Some(index) = self
            .tree
            .as_ref()
            .and_then(|t| find_text_pos(t, id))
            .and_then(|node| caret_index_at(node, pos, font))
        else {
            return false;
        };
        let ran = self.handlers.dispatch_text_pos(id, index, true, state);
        if ran {
            self.dirty = true;
        }
        ran
    }

    /// End the current pointer text selection (pointer released).
    fn end_text_select(&mut self) {
        self.text_selecting = None;
    }

    /// The cursor the window should show with the pointer at `pos`, if it
    /// differs from the one last set. Kept as is during a drag or text
    /// selection, which latch the cursor they started with.
    fn cursor_change(&mut self, pos: Point) -> Option<CursorIcon> {
        if self.dragging.is_some() || self.text_selecting.is_some() {
            return None;
        }
        let now = self
            .tree
            .as_ref()
            .map_or(CursorIcon::Default, |t| crate::core::cursor_at(t, pos));
        (now != self.cursor).then(|| {
            self.cursor = now;
            now
        })
    }

    /// Update the hovered element to whatever tappable sits under `pos`.
    /// Returns `true` if the hovered element changed (the UI should repaint).
    fn hover_at(&mut self, state: &S, theme: &Theme, font: Option<&Font>, pos: Point) -> bool {
        // The wheel targets the container under the pointer.
        self.last_pointer = pos;
        self.ensure_tree(state, theme, font);
        let now = self.tap_at(pos);
        let changed = now != self.hovered;
        self.hovered = now;
        // An open menu follows the pointer: rows highlight (opening their
        // submenus), and bar titles switch menus.
        let mut menu_changed = false;
        if changed && let Some(id) = now {
            menu_changed |= self.menu_ui.borrow_mut().hover(id);
        }
        let title = self.menu_title_at(pos);
        if let Some(index) = title {
            let mut ui = self.menu_ui.borrow_mut();
            if ui.root != Some(crate::widgets::menus::MenuRoot::Bar(index)) {
                ui.open_bar(index, false);
                menu_changed = true;
            }
        }
        if menu_changed {
            self.dirty = true;
        }
        changed || menu_changed
    }

    /// Scroll the container under the last pointer position by `dy` logical
    /// pixels (positive = reveal content further down). Returns whether anything
    /// scrolled (the offset is re-clamped to the content during the next build).
    fn scroll_by(&mut self, state: &S, theme: &Theme, font: Option<&Font>, dy: f64) -> bool {
        self.ensure_tree(state, theme, font);
        let Some(id) = self
            .tree
            .as_ref()
            .and_then(|t| crate::core::scroll_at(t, self.last_pointer))
        else {
            return false;
        };
        let off = self.scroll_offsets.entry(id).or_insert(0.0);
        let before = *off;
        *off = (*off + dy).max(0.0);
        // A scroll always rebuilds (apply_scroll re-clamps to the content); only
        // report movement when the unclamped offset actually changed.
        let moved = (*off - before).abs() > f64::EPSILON;
        if moved {
            self.dirty = true;
        }
        moved
    }
}

/// The part of `tree` a press at `pos` reaches, and whether it is the main tree.
/// With overlays up, the root holds the main tree then each overlay's layers
/// (a dismiss catcher or modal scrim, then its content); only the topmost layer
/// under the pointer is reachable, so a catcher blocks the tree below it.
fn press_layer(tree: &LayoutNode, overlays: bool, pos: Point) -> (&LayoutNode, bool) {
    if !overlays {
        return (tree, true);
    }
    match tree
        .children
        .iter()
        .enumerate()
        .rev()
        .find(|(_, layer)| layer.bounds.contains(pos))
    {
        Some((i, layer)) => (layer, i == 0),
        None => (tree, true),
    }
}

impl<S: 'static> App<S> {
    /// Build, paint, and rasterize the primary window's next frame, returning
    /// the [`Pixmap`] and the [`Damage`] (changed region, logical pixels)
    /// relative to the previously returned frame. The first call reports
    /// [`Damage::Full`].
    pub fn render_frame(&mut self) -> (Pixmap, Damage) {
        let App {
            state,
            theme,
            font,
            panes,
            viewport_content,
            ..
        } = self;
        panes[0].render_frame(state, theme, font.as_ref(), viewport_content)
    }

    /// Render a single frame off-screen and return it as a [`Pixmap`]. Needs no
    /// window — used for tests, thumbnails, and golden-image comparisons.
    pub fn render_once(&mut self) -> Pixmap {
        self.render_frame().0
    }

    /// Whether the primary window's last frame asked for another: something on
    /// it animates (a spinner, a fading highlight). An idle app returns `false`
    /// and gets no frames.
    pub fn animating(&self) -> bool {
        self.panes[0].animating
    }

    /// The primary window's currently focused element, if any.
    pub fn focused(&self) -> Option<FocusId> {
        self.panes[0].focused
    }

    /// Build the [accessibility tree](crate::core::accessibility_tree) for the
    /// primary window's current frame — the semantic view a platform AT backend
    /// would expose. Returns `None` until a frame has been built (call
    /// [`render_once`](App::render_once) or any event-routing method first).
    pub fn accessibility_tree(&self) -> Option<crate::core::AccessNode> {
        self.panes[0].accessibility_tree()
    }

    /// Route a completed click at `pos` (logical pixels) to the primary window.
    pub fn click_at(&mut self, pos: Point) -> bool {
        self.pane_click_at(0, pos)
    }

    /// Deliver committed `text` to the primary window's focused element.
    pub fn type_text(&mut self, text: &str) -> bool {
        self.pane_type_text(0, text)
    }

    /// Deliver an editing key to the primary window's focused element.
    pub fn press_key(&mut self, input: KeyInput) -> bool {
        self.pane_press_key(0, input)
    }

    /// Move focus to the next focusable element in the primary window (Tab).
    pub fn focus_next(&mut self) -> bool {
        self.pane_focus_next(0)
    }

    /// Begin or continue a pointer drag at `pos` in the primary window.
    pub fn drag_at_point(&mut self, pos: Point) -> bool {
        self.pane_drag_at_point(0, pos)
    }

    /// End the current drag (pointer released).
    pub fn end_drag(&mut self) {
        self.panes[0].end_drag();
    }

    /// Begin a pointer text interaction at `pos` in the primary window.
    pub fn text_press_at(&mut self, pos: Point) -> bool {
        self.pane_text_press_at(0, pos)
    }

    /// Continue a latched text drag-selection at `pos` in the primary window.
    pub fn text_drag_at(&mut self, pos: Point) -> bool {
        self.pane_text_drag_at(0, pos)
    }

    /// End the current pointer text selection (pointer released).
    pub fn end_text_select(&mut self) {
        self.panes[0].end_text_select();
    }

    /// Update the primary window's hovered element to whatever sits under `pos`.
    pub fn hover_at(&mut self, pos: Point) -> bool {
        self.pane_hover_at(0, pos)
    }

    /// Scroll the container under the last pointer position in the primary
    /// window by `dy` logical pixels.
    pub fn scroll_by(&mut self, dy: f64) -> bool {
        self.pane_scroll_by(0, dy)
    }

    // --- Per-pane event routing (by window index) --------------------------
    // The live multi-window loop routes each event to the pane that owns the
    // window it arrived on; the public methods above are these against pane 0.

    fn pane_click_at(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].click_at(state, theme, font.as_ref(), pos)
    }

    fn pane_context_at(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].context_at(state, theme, font.as_ref(), pos)
    }

    fn pane_type_text(&mut self, idx: usize, text: &str) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].type_text(state, theme, font.as_ref(), text)
    }

    fn pane_press_key(&mut self, idx: usize, input: KeyInput) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].press_key(state, theme, font.as_ref(), input)
    }

    fn pane_focus_next(&mut self, idx: usize) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].focus_next(state, theme, font.as_ref())
    }

    fn pane_drag_at_point(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].drag_at_point(state, theme, font.as_ref(), pos)
    }

    fn pane_text_press_at(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].text_press_at(
            state,
            theme,
            font.as_ref(),
            pos,
            crate::platform::time::Instant::now(),
        )
    }

    fn pane_text_drag_at(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].text_drag_at(state, theme, font.as_ref(), pos)
    }

    fn pane_hover_at(&mut self, idx: usize, pos: Point) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].hover_at(state, theme, font.as_ref(), pos)
    }

    fn pane_scroll_by(&mut self, idx: usize, dy: f64) -> bool {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        panes[idx].scroll_by(state, theme, font.as_ref(), dy)
    }

    /// Run the app against the platform backend ([`backend::run`]): native X11
    /// when `$DISPLAY` is set, else a one-shot headless present. Frames are
    /// rendered into each window's [`Surface`]; pointer/keyboard events route
    /// through the same dispatch path used by the headless tests.
    ///
    /// Every window registered with [`open_window`](App::open_window) is opened
    /// as a real OS window on backends that support it (X11 today); each renders
    /// its own pane onto the shared state, and events are routed to the window
    /// they arrived on. Backends without multi-window show only the primary
    /// window. The loop ends when the last window closes.
    ///
    /// On the web this returns once the first frame is up, and the page drives
    /// the app from then on (see [`web_app!`]).
    pub fn run(mut self)
    where
        S: 'static,
    {
        // Map the semantic accessibility tree into the platform-neutral mirror the
        // backend's OS bridge consumes (NSAccessibility / UIA / AT-SPI).
        fn to_platform_a11y(node: &crate::core::AccessNode) -> crate::platform::A11yNode {
            use crate::core::Role;
            use crate::platform::A11yRole;
            let role = match node.role {
                Role::Window => A11yRole::Window,
                Role::Group => A11yRole::Group,
                Role::Button => A11yRole::Button,
                Role::TextField => A11yRole::TextField,
                Role::Text => A11yRole::Text,
                Role::List => A11yRole::List,
                Role::ListItem => A11yRole::ListItem,
                Role::Table => A11yRole::Table,
                Role::Row => A11yRole::Row,
                Role::Cell => A11yRole::Cell,
                Role::ColumnHeader => A11yRole::ColumnHeader,
                Role::Tree => A11yRole::Tree,
                Role::TreeItem => A11yRole::TreeItem,
                Role::MenuBar => A11yRole::MenuBar,
                Role::Menu => A11yRole::Menu,
                Role::MenuItem => A11yRole::MenuItem,
                Role::ComboBox => A11yRole::ComboBox,
                Role::SpinButton => A11yRole::SpinButton,
                Role::CheckBox => A11yRole::CheckBox,
                Role::Switch => A11yRole::Switch,
                Role::Splitter => A11yRole::Splitter,
                Role::Toolbar => A11yRole::Toolbar,
                Role::Separator => A11yRole::Separator,
                Role::Alert => A11yRole::Alert,
            };
            crate::platform::A11yNode {
                role,
                name: node.name.clone(),
                bounds: (
                    node.bounds.min_x(),
                    node.bounds.min_y(),
                    node.bounds.width(),
                    node.bounds.height(),
                ),
                focused: node.focused,
                selected: node.selected,
                expanded: node.expanded,
                checked: node.checked,
                children: node.children.iter().map(to_platform_a11y).collect(),
            }
        }

        // A native theme follows the system's look while the app runs.
        if self.theme.follows_system {
            crate::platform::native::watch();
        }
        // Title bars match the content under every theme.
        let dark = self.theme.is_dark();
        let background = self.theme.palette.background;
        for pane in &mut self.panes {
            pane.attrs.dark.get_or_insert(dark);
            pane.attrs.background.get_or_insert(background);
        }
        let primary_attrs = self.panes[0].attrs.clone();
        // Per-pane surfaces, created lazily on first present, parallel to panes.
        let mut surfaces: Vec<Option<Box<dyn Surface>>> =
            (0..self.panes.len()).map(|_| None).collect();
        // native window id -> pane index, filled as windows are created.
        let mut id_to_pane: std::collections::HashMap<WindowId, usize> =
            std::collections::HashMap::new();
        let mut open_count = 0usize;
        let mut opened_extras = false;

        // `force` presents the whole frame regardless of computed damage — used
        // for expose/resize, where the window's pixels were lost and a partial
        // update would leave stale or blank regions.
        let present = |app: &mut Self,
                       surfaces: &mut [Option<Box<dyn Surface>>],
                       idx: usize,
                       window: &dyn crate::platform::Window,
                       force: bool| {
            if idx == 0 {
                app.sync_menu_bar(window);
                app.sync_tray(window);
            }
            app.native_context_menu(idx, window);
            let App {
                state,
                theme,
                font,
                panes,
                viewport_content,
                ..
            } = app;
            let pane = &mut panes[idx];
            let scale = window.scale_factor();
            pane.scale = scale;
            pane.drop_stale_preedit(state);
            let scene = pane.build_frame(state, theme, font.as_ref());
            pane.sync_ime(window, font.as_ref());

            // Push the freshly-built accessibility tree to the OS bridge so a
            // screen reader sees the current element hierarchy and focus.
            if let Some(tree) = pane.accessibility_tree() {
                window.set_accessibility_tree(&to_platform_a11y(&tree));
                // A focused text input asks for the on-screen keyboard.
                fn typing(node: &crate::core::AccessNode) -> bool {
                    use crate::core::Role;
                    (node.focused && matches!(node.role, Role::TextField | Role::SpinButton))
                        || node.children.iter().any(typing)
                }
                window.set_text_input_active(typing(&tree));
            }

            let damage = pane.take_damage();

            // Nothing changed since the last present: render nothing, upload
            // nothing. (Skips the whole rasterize, not just the upload.)
            if !force && damage.is_empty() {
                app_dbg(format_args!("present: no damage"));
                return;
            }
            app_dbg(format_args!("present: {damage:?}"));

            let phys_size = window.inner_size();
            let surface = surfaces[idx].get_or_insert_with(|| window.create_surface());
            surface.resize(phys_size);
            let bounds = Rect::from_xywh(0.0, 0.0, phys_size.width as f64, phys_size.height as f64);

            // Area repaint: re-rasterize only the damaged rects into the
            // retained pixmap and upload those. Requires the software path, a
            // localizable (non-forced, region) damage, and a retained pixmap
            // matching the current surface size.
            let can_partial = !force
                && pane.frame_renderer.is_none()
                && !pane.has_viewports()
                && matches!(damage, Damage::Regions(_))
                && pane.last_pixmap.as_ref().map(Pixmap::size) == Some(phys_size);

            if can_partial {
                let regions = damage.to_physical(scale, bounds);
                if regions.is_empty() {
                    return; // damage fell entirely outside the surface
                }
                let s = scale.get();
                let renderer = SoftwareRenderer::new().with_background(theme.palette.background);
                let pixmap = pane.last_pixmap.as_mut().expect("checked by can_partial");
                for r in &regions {
                    let (pw, ph) = (r.width() as u32, r.height() as u32);
                    if pw == 0 || ph == 0 {
                        continue;
                    }
                    // `regions` are integer device-pixel rects; the matching
                    // logical view is exactly that rect divided by the scale, so
                    // the sub-render lands 1:1 on the destination pixels.
                    let view = Rect::from_xywh(
                        r.min_x() / s,
                        r.min_y() / s,
                        r.width() / s,
                        r.height() / s,
                    );
                    let sub =
                        renderer.render_region(scene.clone(), view, PhysicalSize::new(pw, ph));
                    pixmap.blit(&sub, r.min_x() as u32, r.min_y() as u32);
                }
                surface.present(pixmap, &regions);
            } else {
                // Full render: forced (expose/resize), GPU path, first frame, a
                // size change, or unlocalizable (`Damage::Full`) damage.
                let mut pixmap = pane.rasterize(scene, theme, scale);
                pane.composite_viewports(&mut pixmap, viewport_content, scale);
                let regions = if force {
                    Vec::new()
                } else {
                    damage.to_physical(scale, bounds)
                };
                surface.present(&pixmap, &regions);
                pane.last_pixmap = Some(pixmap);
            }
        };

        backend::run(primary_attrs, move |event, window| {
            let wid = window.id();
            // On the first event we learn the primary window's id; register it
            // and ask the backend to open the remaining panes as OS windows.
            if !opened_extras {
                opened_extras = true;
                id_to_pane.insert(wid, 0);
                open_count = 1;
                for idx in 1..self.panes.len() {
                    let attrs = self.panes[idx].attrs.clone();
                    if let Some(id) = window.open_window(attrs) {
                        id_to_pane.insert(id, idx);
                        open_count += 1;
                    }
                }
            }
            let idx = id_to_pane.get(&wid).copied().unwrap_or(0);
            // Window changes queued through `AppHandle::window` go to the
            // primary window (its events, including the wake they cause).
            if idx == 0 {
                for request in self.handle.take_window_requests() {
                    request.apply(window);
                }
            }
            match event {
                Event::RedrawRequested => {
                    present(&mut self, &mut surfaces, idx, window, true);
                    ControlFlow::Wait
                }
                Event::Resized(size) => {
                    self.panes[idx].attrs.logical_size = window.scale_factor().to_logical(size);
                    self.panes[idx].dirty = true;
                    present(&mut self, &mut surfaces, idx, window, true);
                    ControlFlow::Wait
                }
                // Secondary (right) click: route to a context handler, which
                // typically opens a context menu at the click position.
                Event::PointerButton {
                    button: PointerButton::Right,
                    state: ButtonState::Pressed,
                    position,
                } => {
                    // A right-press inside a viewport forwards to its content
                    // (button 1) instead of opening a Stipple context menu.
                    if self.try_forward_pointer(idx, position, PointerKind::Down(1))
                        || self.pane_context_at(idx, position)
                    {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::PointerButton {
                    button: PointerButton::Left,
                    state: ButtonState::Pressed,
                    position,
                } => {
                    self.alt_armed = false;
                    // A press inside a viewport forwards to its content (button 0)
                    // and grabs key focus; a press elsewhere drops that focus.
                    if self.try_forward_pointer(idx, position, PointerKind::Down(0)) {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    self.panes[idx].input_viewport = None;
                    self.panes[idx].pressed = self.panes[idx].tap_at(position);
                    app_dbg(format_args!(
                        "press at {position:?}: target {:?}",
                        self.panes[idx].pressed
                    ));
                    if self.panes[idx].pressed.is_some() {
                        // Repaint for the pressed look: the press highlight, or
                        // the OS's own for an OS-drawn control.
                        self.panes[idx].dirty = true;
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    // Editable text under the cursor starts a click/drag
                    // selection; otherwise latch a drag if a draggable sits there.
                    if self.pane_text_press_at(idx, position)
                        || self.pane_drag_at_point(idx, position)
                    {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::PointerMoved { position } => {
                    self.panes[idx].last_pointer = position;
                    if let Some(cursor) = self.panes[idx].cursor_change(position) {
                        window.set_cursor(cursor);
                    }
                    // Forward moves over a viewport to its content, unless a Stipple
                    // drag/selection latched on a press outside it is in progress.
                    if self.panes[idx].text_selecting.is_none()
                        && self.panes[idx].dragging.is_none()
                        && self.try_forward_pointer(idx, position, PointerKind::Move)
                    {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    if self.panes[idx].text_selecting.is_some() {
                        if self.pane_text_drag_at(idx, position) {
                            present(&mut self, &mut surfaces, idx, window, false);
                        }
                    } else if self.panes[idx].dragging.is_some() {
                        if self.pane_drag_at_point(idx, position) {
                            present(&mut self, &mut surfaces, idx, window, false);
                        }
                    } else if self.pane_hover_at(idx, position) {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::PointerButton {
                    button: PointerButton::Left,
                    state: ButtonState::Released,
                    position,
                } => {
                    // Release inside a viewport forwards to its content (button 0).
                    if self.try_forward_pointer(idx, position, PointerKind::Up(0)) {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    if self.panes[idx].text_selecting.is_some() {
                        self.panes[idx].end_text_select();
                    } else if self.panes[idx].dragging.is_some() {
                        self.panes[idx].end_drag();
                    } else {
                        let down = self.panes[idx].pressed.take();
                        let up = self.panes[idx].tap_at(position);
                        app_dbg(format_args!(
                            "release at {position:?}: pressed {down:?}, here {up:?}"
                        ));
                        if down.is_some() && down == up {
                            self.pane_click_at(idx, position);
                        }
                        if down.is_some() {
                            // Clear the pressed look, whether or not it clicked.
                            self.panes[idx].dirty = true;
                            present(&mut self, &mut surfaces, idx, window, false);
                        }
                    }
                    ControlFlow::Wait
                }
                Event::Text(text) => {
                    // An open menu takes typed letters as mnemonics.
                    if self.menu_text(idx, &text) {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    // A viewport with input focus consumes typed text.
                    if self.try_forward_key(idx, KeyInput::Text(text.clone())) {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    if self.pane_type_text(idx, &text) {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::Ime(ime) => {
                    let input = match ime {
                        Ime::Preedit { text, cursor } => KeyInput::Preedit { text, cursor },
                        Ime::Commit(text) => KeyInput::Text(text),
                        Ime::DeleteSurrounding { before, after } => {
                            KeyInput::DeleteSurrounding { before, after }
                        }
                    };
                    if self.try_forward_key(idx, input.clone()) {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    let composing =
                        matches!(&input, KeyInput::Preedit { text, .. } if !text.is_empty());
                    let ran = self.pane_press_key(idx, input);
                    let pane = &mut self.panes[idx];
                    pane.preedit_owner = if composing && ran { pane.focused } else { None };
                    if ran {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::Key {
                    code: KeyCode::Alt,
                    state: ButtonState::Released,
                    ..
                } => {
                    if self.alt_release(idx) {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::Key {
                    code: KeyCode::Tab,
                    state: ButtonState::Pressed,
                    modifiers,
                } => {
                    if self.menu_key(idx, KeyCode::Tab, modifiers) || self.pane_focus_next(idx) {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::Key {
                    code,
                    state: ButtonState::Pressed,
                    modifiers,
                } => {
                    // Menus first: an open menu, the menu bar's keys, context
                    // menu keys and shortcuts.
                    if self.menu_key(idx, code, modifiers) {
                        present(&mut self, &mut surfaces, idx, window, false);
                        return ControlFlow::Wait;
                    }
                    if let Some(input) = map_key(code, modifiers) {
                        // A viewport with input focus consumes editing/navigation
                        // keys too (no clipboard sync — that's the content's job).
                        if self.try_forward_key(idx, input.clone()) {
                            present(&mut self, &mut surfaces, idx, window, false);
                            return ControlFlow::Wait;
                        }
                        // Pull the OS clipboard into the mirror before a paste,
                        // and push the mirror to the OS after a copy/cut, so
                        // editing interoperates with other apps (the mirror alone
                        // covers the in-app case + headless).
                        if input == KeyInput::Paste
                            && let Some(text) = window.clipboard()
                        {
                            crate::core::set_clipboard_text(&text);
                        }
                        let writes_clipboard = matches!(input, KeyInput::Copy | KeyInput::Cut);
                        if self.pane_press_key(idx, input) {
                            present(&mut self, &mut surfaces, idx, window, false);
                        }
                        if writes_clipboard {
                            window.set_clipboard(&crate::core::clipboard_text());
                        }
                    }
                    ControlFlow::Wait
                }
                Event::Scroll { delta } => {
                    // A wheel over a viewport scrolls its content, not the chrome.
                    let at = self.panes[idx].last_pointer;
                    if self.try_forward_wheel(idx, at, delta.dy)
                        || self.pane_scroll_by(idx, delta.dy)
                    {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::Wake => {
                    // Apply updates and due timers from the handle, then re-render
                    // if anything ran. The loop wakes again at the next timer.
                    let (ran, next) = self
                        .handle
                        .pump(&mut self.state, crate::platform::time::Instant::now());
                    crate::platform::wake::set_deadline(next);
                    if self.handle.exit_requested() {
                        return ControlFlow::Exit;
                    }
                    if !ran && !self.panes[idx].animating {
                        // Repeated, this means a deadline that keeps coming due.
                        app_dbg(format_args!("wake with nothing to do"));
                    }
                    // A frame for an animation (or a fade) re-renders too.
                    if ran || self.panes[idx].animating {
                        for pane in &mut self.panes {
                            pane.dirty |= ran || pane.animating;
                        }
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::ThemeChanged => {
                    // The system's look changed: forget the controls the OS drew
                    // in the old one, follow it if the theme is native, and
                    // repaint.
                    crate::platform::native::clear_cache();
                    if self.theme.follows_system {
                        self.theme =
                            Theme::native().with_layout_direction(self.theme.layout_direction);
                        let dark = self.theme.is_dark();
                        for pane in &mut self.panes {
                            pane.attrs.dark = Some(dark);
                            pane.attrs.background = Some(self.theme.palette.background);
                        }
                        window.set_dark(dark);
                    }
                    for pane in &mut self.panes {
                        pane.dirty = true;
                    }
                    present(&mut self, &mut surfaces, idx, window, true);
                    ControlFlow::Wait
                }
                Event::TrayActivated => {
                    if self.run_tray_activate() {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::MenuCommand(id) => {
                    if self.run_menu_command(id) {
                        present(&mut self, &mut surfaces, idx, window, false);
                    }
                    ControlFlow::Wait
                }
                Event::FocusChanged(_) => {
                    self.alt_armed = false;
                    // Alt+Tab away leaves no underlines behind.
                    let pane = &mut self.panes[idx];
                    let mut ui = pane.menu_ui.borrow_mut();
                    if ui.cues && !ui.is_active() {
                        ui.cues = false;
                        pane.dirty = true;
                    }
                    ControlFlow::Wait
                }
                Event::CloseRequested => {
                    // Close just this window; end the loop only when the last
                    // window goes away.
                    id_to_pane.remove(&wid);
                    open_count = open_count.saturating_sub(1);
                    if open_count == 0 {
                        ControlFlow::Exit
                    } else {
                        window.close_window();
                        ControlFlow::Wait
                    }
                }
                _ => ControlFlow::Wait,
            }
        });
    }
}

impl<S: 'static> App<S> {
    /// Route a key press through the menus: an open menu or a focused menu bar
    /// takes every key; then the menu bar's own keys (F10, Alt+letter), the
    /// context-menu keys (Menu, Shift+F10) and the menu bar's shortcuts. Alt on
    /// its own only arms [`alt_release`](Self::alt_release). Returns whether
    /// the key was used.
    fn menu_key(&mut self, idx: usize, code: KeyCode, mods: Modifiers) -> bool {
        if code == KeyCode::Alt {
            let pane = &mut self.panes[idx];
            self.alt_armed = pane.menu_bar.is_some() || pane.menu_ui.borrow().is_active();
            // Holding Alt shows the drawn menu bar's mnemonics.
            let mut ui = pane.menu_ui.borrow_mut();
            let show = pane.menu_bar.is_some() && !ui.cues;
            ui.cues |= show;
            pane.dirty |= show;
            return show;
        }
        self.alt_armed = false;
        if let Some(used) = self.menu_nav(idx, menu_key_of(code, self.theme.is_rtl())) {
            return used;
        }
        let App {
            state,
            theme,
            font,
            panes,
            menu,
            ..
        } = self;
        let font = font.as_ref();
        let pane = &mut panes[idx];
        let plain = !(mods.shift || mods.ctrl || mods.alt || mods.meta);
        let has_bar = pane.menu_bar.is_some();
        if code == KeyCode::F10 && plain && has_bar && pane.menu_ui.borrow_mut().focus_bar() {
            pane.dirty = true;
            return true;
        }
        let shift_only = mods.shift && !(mods.ctrl || mods.alt || mods.meta);
        if (code == KeyCode::ContextMenu || (code == KeyCode::F10 && shift_only))
            && pane.context_by_key(state, theme, font)
        {
            return true;
        }
        if has_bar
            && mods.alt
            && !(mods.ctrl || mods.meta)
            && let KeyCode::Char(c) = code
            && pane.menu_ui.borrow_mut().open_by_mnemonic(c)
        {
            pane.dirty = true;
            return true;
        }
        // Shortcuts, unless the key edits the focused text (Ctrl+C in a field
        // copies its selection rather than running Edit > Copy).
        let Some(menu) = menu.clone() else {
            return false;
        };
        if pane.focused_is_editable() && map_key(code, mods).is_some() {
            return false;
        }
        let bar = menu(state);
        match bar.find_shortcut(code, mods) {
            Some(item) => {
                item.activate(state);
                for pane in panes.iter_mut() {
                    pane.dirty = true;
                }
                true
            }
            None => false,
        }
    }

    /// Typed text while a menu is open: its first character is a mnemonic.
    fn menu_text(&mut self, idx: usize, text: &str) -> bool {
        let key = text
            .chars()
            .next()
            .and_then(|c| c.to_lowercase().next())
            .map(crate::widgets::menus::MenuKey::Char);
        self.alt_armed = false;
        self.menu_nav(idx, key).unwrap_or(false)
    }

    /// Hand `key` to pane `idx`'s open menu or focused menu bar. `None` when no
    /// menu is active there; otherwise whether the key was used (keys menus
    /// don't navigate with are swallowed while one is open).
    fn menu_nav(
        &mut self,
        idx: usize,
        key: Option<crate::widgets::menus::MenuKey>,
    ) -> Option<bool> {
        use crate::widgets::menus::KeyResult;
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        if !pane.menu_ui.borrow().is_active() {
            return None;
        }
        pane.ensure_tree(state, theme, font.as_ref());
        let result = match key {
            Some(key) => pane.menu_ui.borrow_mut().handle_key(key),
            None => KeyResult::Handled,
        };
        match result {
            KeyResult::Ignored => return Some(false),
            KeyResult::Handled => pane.dirty = true,
            KeyResult::Activate(id) => {
                pane.handlers.dispatch(id, state);
                for pane in panes.iter_mut() {
                    pane.dirty = true;
                }
            }
        }
        Some(true)
    }

    /// Alt was released: if nothing else was pressed since it went down, focus
    /// the menu bar (or leave the menus). Returns whether anything changed.
    fn alt_release(&mut self, idx: usize) -> bool {
        if !std::mem::take(&mut self.alt_armed) {
            // Alt went with another key: hide the cues it showed, unless the
            // keyboard went on into the menus.
            let pane = &mut self.panes[idx];
            let mut ui = pane.menu_ui.borrow_mut();
            let hide = ui.cues && !ui.is_active();
            ui.cues &= !hide;
            pane.dirty |= hide;
            return hide;
        }
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        pane.ensure_tree(state, theme, font.as_ref());
        let changed = {
            let mut ui = pane.menu_ui.borrow_mut();
            if ui.is_active() {
                ui.close();
                true
            } else {
                ui.focus_bar()
            }
        };
        pane.dirty |= changed;
        changed
    }

    /// Run the menu bar command with native id `id` (from
    /// [`Event::MenuCommand`]). Returns whether it ran.
    fn run_menu_command(&mut self, id: u32) -> bool {
        let ran = if id >= crate::platform::TRAY_ID_BASE {
            let Some(tray) = self.tray.clone() else {
                return false;
            };
            tray(&self.state).activate_command(id, &mut self.state)
        } else {
            let Some(menu) = self.menu.clone() else {
                return false;
            };
            menu(&self.state).activate(id, &mut self.state)
        };
        if ran {
            for pane in &mut self.panes {
                pane.dirty = true;
            }
        }
        ran
    }

    /// Run the tray icon's click action ([`Event::TrayActivated`]). Returns
    /// whether it ran.
    fn run_tray_activate(&mut self) -> bool {
        let Some(tray) = self.tray.clone() else {
            return false;
        };
        let ran = tray(&self.state).activate(&mut self.state);
        if ran {
            for pane in &mut self.panes {
                pane.dirty = true;
            }
        }
        ran
    }

    /// Hand the tray icon to `window` when it changed.
    fn sync_tray(&mut self, window: &dyn crate::platform::Window) {
        let Some(tray) = self.tray.clone() else {
            return;
        };
        let native = tray(&self.state).to_native();
        if self.native_tray.as_ref() == Some(&native) {
            return;
        }
        let shown = window.set_tray(Some(&native));
        crate::platform::shell::set_tray_shown(shown);
        self.native_tray = Some(native);
    }

    /// Hand the menu bar to `window` as a native one when it changed. Where
    /// the platform shows it, the primary window stops drawing its own.
    fn sync_menu_bar(&mut self, window: &dyn crate::platform::Window) {
        if self.menu_in_window || self.native_menu_unsupported {
            return;
        }
        let Some(menu) = self.menu.clone() else {
            return;
        };
        let native = menu(&self.state).to_native(&self.panes[0].attrs.title);
        if self.native_menu.as_ref() == Some(&native) {
            return;
        }
        let shown = window.set_menu_bar(&native);
        self.native_menu = Some(native);
        self.native_menu_unsupported = !shown;
        let pane = &mut self.panes[0];
        if shown && pane.menu_bar.is_some() {
            pane.menu_bar = None;
            pane.dirty = true;
        }
    }

    /// A context menu just opened in pane `idx`: show it as the platform's
    /// own popup menu where there is one (and the menus aren't drawn in the
    /// window by choice), and run what was picked. The drawn menu stays open
    /// where the platform has none.
    fn native_context_menu(&mut self, idx: usize, window: &dyn crate::platform::Window) {
        use crate::platform::MenuPopup;
        use crate::widgets::menus::MenuRoot;
        if self.menu_in_window || self.native_popup_unsupported {
            return;
        }
        let pane = &mut self.panes[idx];
        let Some(MenuRoot::Context { id, at }) = pane.menu_ui.borrow().root else {
            return;
        };
        let Some((_, menu)) = pane.context_menus.iter().find(|(m, _)| *m == id) else {
            return;
        };
        let native = menu.to_native();
        let keyboard = pane.menu_ui.borrow().cues;
        // Not drawn meanwhile (and the next frame shows it closed).
        pane.menu_ui.borrow_mut().close();
        pane.dirty = true;
        match window.popup_menu(&native, at) {
            MenuPopup::Unsupported => {
                self.native_popup_unsupported = true;
                pane.menu_ui.borrow_mut().open_context(id, at, keyboard);
            }
            MenuPopup::Dismissed => {}
            MenuPopup::Picked(cmd) => {
                let action = menu.command(cmd).and_then(|item| item.action());
                if let Some(action) = action {
                    (action.borrow_mut())(&mut self.state);
                    for pane in &mut self.panes {
                        pane.dirty = true;
                    }
                }
            }
        }
    }

    /// Press `code` with `modifiers` in the primary window, as a key event
    /// from the platform would: menus and shortcuts first, then the focused
    /// element (Tab moves focus). For tests and scripted input. Returns whether
    /// anything handled it.
    pub fn press_key_code(&mut self, code: KeyCode, modifiers: Modifiers) -> bool {
        if self.menu_key(0, code, modifiers) {
            return true;
        }
        if code == KeyCode::Tab {
            return self.pane_focus_next(0);
        }
        map_key(code, modifiers).is_some_and(|input| self.pane_press_key(0, input))
    }

    /// Release `code` in the primary window: Alt released on its own focuses
    /// the menu bar. Returns whether anything changed.
    pub fn release_key_code(&mut self, code: KeyCode) -> bool {
        code == KeyCode::Alt && self.alt_release(0)
    }

    /// Route a secondary (right) click at `pos` in the primary window: it opens
    /// the context menu of the element there. Returns whether anything
    /// handled it.
    pub fn right_click_at(&mut self, pos: Point) -> bool {
        self.pane_context_at(0, pos)
    }

    /// Run the menu bar command with native id `id`, as a pick from a native
    /// menu bar does (see [`MenuBar::commands`](crate::core::MenuBar::commands)).
    pub fn menu_command(&mut self, id: u32) -> bool {
        self.run_menu_command(id)
    }

    /// Click the tray icon, as [`Event::TrayActivated`] does. Returns whether
    /// a click action ran.
    pub fn tray_activate(&mut self) -> bool {
        self.run_tray_activate()
    }

    /// The tray icon as it's handed to the platform, if the app has one.
    pub fn native_tray(&self) -> Option<crate::platform::NativeTray> {
        self.tray.as_ref().map(|t| t(&self.state).to_native())
    }
}

/// The menu navigation key for `code`, if menus use it. In a mirrored (`rtl`)
/// UI the menu bar runs right to left and submenus open leftwards, so Left and
/// Right swap.
fn menu_key_of(code: KeyCode, rtl: bool) -> Option<crate::widgets::menus::MenuKey> {
    use crate::widgets::menus::MenuKey;
    Some(match code {
        KeyCode::ArrowUp => MenuKey::Up,
        KeyCode::ArrowDown => MenuKey::Down,
        KeyCode::ArrowLeft if rtl => MenuKey::Right,
        KeyCode::ArrowRight if rtl => MenuKey::Left,
        KeyCode::ArrowLeft => MenuKey::Left,
        KeyCode::ArrowRight => MenuKey::Right,
        KeyCode::Home => MenuKey::Home,
        KeyCode::End => MenuKey::End,
        KeyCode::Enter => MenuKey::Enter,
        KeyCode::Escape => MenuKey::Escape,
        KeyCode::Tab => MenuKey::Tab,
        KeyCode::Space => MenuKey::Char(' '),
        KeyCode::Char(c) => MenuKey::Char(c.to_lowercase().next()?),
        _ => return None,
    })
}

/// Translate a platform [`KeyCode`] (plus active `modifiers`) into a core
/// [`KeyInput`] editing command. Character input arrives via [`Event::Text`], so
/// only editing/navigation keys map here. Shift turns caret motions into
/// selection-extending motions; Ctrl/Cmd+A selects all; Ctrl/Cmd+Z undoes and
/// Ctrl+Y or Ctrl/Cmd+Shift+Z redoes. The word modifier (Ctrl, or Alt on macOS)
/// makes Left/Right move by word and Backspace/Delete delete a word.
fn map_key(code: KeyCode, modifiers: Modifiers) -> Option<KeyInput> {
    map_key_for(code, modifiers, cfg!(target_os = "macos"))
}

/// [`map_key`] with the platform convention explicit, so both can be tested:
/// on macOS (`mac`) the word modifier is Alt (Option), elsewhere Ctrl.
fn map_key_for(code: KeyCode, modifiers: Modifiers, mac: bool) -> Option<KeyInput> {
    let shift = modifiers.shift;
    let command = modifiers.ctrl || modifiers.meta;
    let word = if mac { modifiers.alt } else { modifiers.ctrl };
    Some(match code {
        KeyCode::Backspace if word => KeyInput::DeleteWordBackward,
        KeyCode::Backspace => KeyInput::Backspace,
        KeyCode::Delete if word => KeyInput::DeleteWordForward,
        KeyCode::Delete => KeyInput::Delete,
        KeyCode::ArrowLeft if word && shift => KeyInput::SelectWordLeft,
        KeyCode::ArrowLeft if word => KeyInput::WordLeft,
        KeyCode::ArrowRight if word && shift => KeyInput::SelectWordRight,
        KeyCode::ArrowRight if word => KeyInput::WordRight,
        KeyCode::ArrowLeft if shift => KeyInput::SelectLeft,
        KeyCode::ArrowLeft => KeyInput::Left,
        KeyCode::ArrowRight if shift => KeyInput::SelectRight,
        KeyCode::ArrowRight => KeyInput::Right,
        KeyCode::ArrowUp if shift => KeyInput::SelectUp,
        KeyCode::ArrowUp => KeyInput::Up,
        KeyCode::ArrowDown if shift => KeyInput::SelectDown,
        KeyCode::ArrowDown => KeyInput::Down,
        KeyCode::Home if shift => KeyInput::SelectHome,
        KeyCode::Home => KeyInput::Home,
        KeyCode::End if shift => KeyInput::SelectEnd,
        KeyCode::End => KeyInput::End,
        KeyCode::Char('a') | KeyCode::Char('A') if modifiers.ctrl || modifiers.meta => {
            KeyInput::SelectAll
        }
        KeyCode::Char('c') | KeyCode::Char('C') if modifiers.ctrl || modifiers.meta => {
            KeyInput::Copy
        }
        KeyCode::Char('x') | KeyCode::Char('X') if modifiers.ctrl || modifiers.meta => {
            KeyInput::Cut
        }
        KeyCode::Char('v') | KeyCode::Char('V') if modifiers.ctrl || modifiers.meta => {
            KeyInput::Paste
        }
        // With Shift held the backend may report the shifted keysym ('Z').
        KeyCode::Char('z') | KeyCode::Char('Z') if command && shift => KeyInput::Redo,
        KeyCode::Char('z') | KeyCode::Char('Z') if command => KeyInput::Undo,
        KeyCode::Char('y') | KeyCode::Char('Y') if modifiers.ctrl && !mac => KeyInput::Redo,
        KeyCode::Enter => KeyInput::Enter,
        KeyCode::Escape => KeyInput::Escape,
        _ => return None,
    })
}

/// The common imports for building a Stipple app.
pub mod prelude {
    pub use crate::App;
    pub use crate::anim::{Easing, Spring, Tween};
    pub use crate::core::ViewportId;
    pub use crate::core::{
        Align, Anchor, AppHandle, Axis, BoxStyle, Cx, Element, KeyInput, LayoutDirection,
        OverlaySpec, TimerId, Track, View,
    };
    pub use crate::core::{Menu, MenuBar, MenuItem, Shortcut, Tray};
    pub use crate::geometry::{Insets, PhysicalSize, Point, Rect, ScaleFactor, Size};
    pub use crate::platform::Notification;
    pub use crate::platform::{CursorIcon, WindowRequest};
    pub use crate::platform::{KeyCode, Modifiers};
    pub use crate::render::{Color, Font, ImageFit, Pixmap, Svg, TextDirection};
    pub use crate::style::Theme;
    pub use crate::style::{Palette, Spacing, Typography};
    pub use crate::widgets::{
        Column, ListState, SelectionMode, SortOrder, TableSpec, TableState, TreeModel, TreeSpec,
        TreeState, VirtualList, table, tree, virtual_list,
    };
    pub use crate::widgets::{
        EditBuffer, Variant, button, button_labeled, button_variant, checkbox, column, divider,
        edit_string, grid, heading, label, menu, menu_item, open_dialog, open_menu, panel,
        paragraph, progress_bar, radio, row, scroll, setting_row, slider, spacer, spinner, swatch,
        switch, tabs, text_area, text_editor, text_field, tooltip, viewport,
    };
    pub use crate::widgets::{
        NumberInput, Select, SplitPane, ToastKind, Toasts, number_input, select, split_pane,
        toasts, tool_button, tool_icon_button, tool_separator, tool_toggle, toolbar,
    };
    pub use crate::widgets::{image, svg};
    pub use std::time::Duration;
}

/// A diagnostic line on stderr when `STIPPLE_APP_DEBUG` is set: what a press,
/// a release and each present did (CI turns it on to explain a missed click).
fn app_dbg(args: std::fmt::Arguments<'_>) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("STIPPLE_APP_DEBUG").is_some()) {
        eprintln!("stipple app: {args}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{button, column};

    struct Counter {
        n: i64,
    }

    /// A counter view: a single 200×80 button at the window origin that
    /// increments the count. Fixed geometry keeps the hit point predictable.
    fn counter_app() -> App<Counter> {
        App::new(
            Counter { n: 0 },
            |_state: &Counter, cx: &mut Cx<Counter>| {
                let theme = *cx.theme();
                column(vec![
                    button(&theme)
                        .width(200.0)
                        .height(80.0)
                        .on_tap(cx, |s: &mut Counter| s.n += 1),
                ])
            },
        )
        .logical_size(Size::new(200.0, 80.0))
    }

    #[test]
    fn click_dispatches_handler_and_mutates_state() {
        let mut app = counter_app();
        assert_eq!(app.state().n, 0);
        // Click inside the button.
        assert!(app.click_at(Point::new(100.0, 40.0)));
        assert_eq!(app.state().n, 1);
        app.click_at(Point::new(100.0, 40.0));
        assert_eq!(app.state().n, 2);
    }

    #[test]
    fn click_outside_any_handler_is_a_noop() {
        let mut app = counter_app().logical_size(Size::new(400.0, 400.0));
        // (300, 300) is outside the 200×80 button.
        assert!(!app.click_at(Point::new(300.0, 300.0)));
        assert_eq!(app.state().n, 0);
    }

    #[test]
    fn render_once_matches_window_size() {
        let mut app = counter_app();
        assert_eq!(
            app.render_once().size(),
            crate::geometry::PhysicalSize::new(200, 80)
        );
    }

    #[test]
    fn input_forwards_to_a_viewport_sink_in_local_coords() {
        use crate::core::{ViewportEvent, ViewportId, collect_viewports};
        use std::cell::RefCell;
        use std::rc::Rc;

        let vid = ViewportId(7);
        let log: Rc<RefCell<Vec<(ViewportId, ViewportEvent)>>> = Rc::new(RefCell::new(Vec::new()));
        let sink_log = log.clone();
        let mut app = App::new((), move |_s: &(), _cx: &mut Cx<()>| {
            column(vec![Element::viewport(vid).width(100.0).height(80.0)])
        })
        .logical_size(Size::new(200.0, 150.0))
        .on_viewport_input(move |id, ev| sink_log.borrow_mut().push((id, ev)));

        // Build the tree, then find the viewport's on-screen rect.
        app.render_once();
        let mut vps = Vec::new();
        collect_viewports(app.panes[0].tree.as_ref().unwrap(), &mut vps);
        let (_, bounds) = vps[0];
        let inside = Point::new(bounds.min_x() + 10.0, bounds.min_y() + 5.0);

        // Press inside: forwarded as local (10,5) and grabs keyboard focus.
        assert!(app.try_forward_pointer(0, inside, PointerKind::Down(0)));
        assert_eq!(app.panes[0].input_viewport, Some(vid));
        // Typed text now routes to the viewport (it holds input focus)...
        assert!(app.try_forward_key(0, KeyInput::Text("x".into())));
        // ...and release forwards too.
        assert!(app.try_forward_pointer(0, inside, PointerKind::Up(0)));
        // A point outside the viewport is never forwarded.
        let outside = Point::new(bounds.max_x() + 50.0, bounds.max_y() + 50.0);
        assert!(!app.try_forward_pointer(0, outside, PointerKind::Move));

        let got = log.borrow();
        assert_eq!(got.len(), 3, "down + key + up");
        assert!(
            matches!(&got[0], (v, ViewportEvent::PointerDown { local, button: 0 })
                if *v == vid && *local == Point::new(10.0, 5.0)),
            "press forwarded with local coords, got {:?}",
            got[0]
        );
        assert!(matches!(&got[1].1, ViewportEvent::Key(KeyInput::Text(t)) if t == "x"));
        assert!(matches!(
            got[2].1,
            ViewportEvent::PointerUp { button: 0, .. }
        ));
    }

    #[test]
    fn viewport_content_is_composited_over_the_placeholder() {
        use crate::core::ViewportId;
        use crate::geometry::PhysicalSize;
        use crate::render::Color;
        use crate::widgets::viewport;

        let vid = ViewportId(1);
        // A 100×60 window holding a 40×30 viewport inset at (10,10) by a 10px pad.
        let mut app = App::new((), move |_s: &(), _cx: &mut Cx<()>| {
            column(vec![viewport(&Theme::light(), vid, 40.0, 30.0)])
                .padding(crate::geometry::Insets::uniform(10.0))
        })
        .logical_size(Size::new(100.0, 60.0));

        // No content yet: the viewport shows the dark-slate placeholder.
        let before = app.render_once();
        assert_eq!(before.pixel(20, 20), Some([0x20, 0x24, 0x2c, 0xff]));

        // Register solid-magenta content sized to the viewport's physical extent.
        let mut content = Pixmap::new(PhysicalSize::new(40, 30));
        for px in content.as_bytes_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[255, 0, 255, 255]);
        }
        app.set_viewport_content(vid, content);

        // Now the viewport interior is the content color, and pixels outside the
        // viewport (e.g. the padding at (2,2)) are untouched.
        let after = app.render_once();
        assert_eq!(
            after.pixel(20, 20),
            Some([255, 0, 255, 255]),
            "content blitted"
        );
        assert_eq!(
            after.pixel(30, 25),
            Some([255, 0, 255, 255]),
            "interior filled"
        );
        assert_ne!(
            after.pixel(2, 2),
            Some([255, 0, 255, 255]),
            "padding outside the viewport is not painted with content"
        );
        let _ = Color::WHITE;
    }

    /// Two stacked 200×40 boxes; tapping the top one flips a flag that only
    /// recolors the *bottom* box. The layout never moves, so a state change
    /// should damage just the bottom box — not the whole window.
    fn damage_app() -> App<bool> {
        use crate::core::BoxStyle;
        use crate::render::Color;
        App::new(false, |flipped: &bool, cx: &mut Cx<bool>| {
            let bottom = if *flipped {
                Color::rgb(200, 0, 0)
            } else {
                Color::rgb(0, 0, 200)
            };
            column(vec![
                Element::boxed(BoxStyle {
                    fill: Some(Color::rgb(20, 20, 20)),
                    radius: 0.0,
                    border: None,
                })
                .width(200.0)
                .height(40.0)
                .on_tap(cx, |f: &mut bool| *f = !*f),
                Element::boxed(BoxStyle {
                    fill: Some(bottom),
                    radius: 0.0,
                    border: None,
                })
                .width(200.0)
                .height(40.0),
            ])
        })
        .logical_size(Size::new(200.0, 80.0))
    }

    #[test]
    fn a_spinner_animates_and_a_static_view_idles() {
        let mut busy = App::new((), |_: &(), cx: &mut Cx<()>| {
            let theme = *cx.theme();
            crate::widgets::spinner(cx, &theme)
        });
        let _ = busy.render_frame();
        assert!(busy.animating(), "a spinner keeps asking for frames");

        let mut still = damage_app();
        let _ = still.render_frame();
        assert!(!still.animating(), "nothing moves, so no frames");
        // A hover starts a fade, which animates until it settles.
        assert!(still.hover_at(Point::new(100.0, 20.0)));
        let _ = still.render_frame();
        assert!(still.animating(), "the hover highlight fades in");
    }

    #[test]
    fn first_frame_is_full_then_unchanged_is_none() {
        let mut app = damage_app();
        let (_p, d0) = app.render_frame();
        assert_eq!(d0, Damage::Full, "first frame must repaint everything");
        // No state change between frames → nothing to repaint.
        let (_p, d1) = app.render_frame();
        assert_eq!(d1, Damage::None);
        assert!(d1.is_empty());
    }

    #[test]
    fn state_change_damages_only_the_changed_box() {
        let mut app = damage_app();
        let _ = app.render_frame(); // prime the baseline (Full)

        // Tap the top box; only the bottom box's color changes.
        assert!(app.click_at(Point::new(100.0, 20.0)));
        let (_p, d) = app.render_frame();

        let bound = match d {
            Damage::Regions(_) => d.bounding().expect("some region"),
            other => panic!("expected localized regions, got {other:?}"),
        };
        // Damage is confined to the bottom box (y in 40..80), not the full 80px.
        assert!(bound.min_y() >= 40.0, "damage strayed above the bottom box");
        assert!(bound.height() <= 40.0, "damage taller than the bottom box");
        assert!(bound.width() <= 200.0);
    }

    #[test]
    fn hover_change_damages_only_the_hovered_box() {
        let mut app = damage_app();
        let _ = app.render_frame(); // prime the baseline (Full)

        // Hover the top (tappable) box: the highlight appears on it alone. The
        // tree is unchanged, so the damage comes purely from hover localization.
        assert!(app.hover_at(Point::new(100.0, 20.0)));
        let (_p, d) = app.render_frame();
        let bound = match &d {
            Damage::Regions(_) => d.bounding().expect("some region"),
            other => panic!("expected localized regions, got {other:?}"),
        };
        // Confined to the top box (y in 0..40), not the full 80px window.
        assert!(
            bound.max_y() <= 40.0,
            "hover damage strayed into the bottom box"
        );
        assert!(bound.height() <= 40.0, "hover damage taller than the box");

        // Let the fade-in settle first: mid-fade, the first fade-out frame can
        // land on the very alpha just painted, so nothing would change.
        std::thread::sleep(crate::core::highlight::HOVER_FADE * 2);
        let _ = app.render_frame();

        // Moving the hover off any tappable clears the highlight, damaging the
        // box it just left — again localized, not full.
        assert!(app.hover_at(Point::new(100.0, 60.0)));
        let (_p, d) = app.render_frame();
        let bound = match &d {
            Damage::Regions(_) => d.bounding().expect("some region"),
            other => panic!("expected localized regions on unhover, got {other:?}"),
        };
        assert!(
            bound.max_y() <= 40.0,
            "unhover damage strayed past the old box"
        );
    }

    #[derive(Default)]
    struct Form {
        name: String,
    }

    /// A form with a single text field filling the window.
    fn form_app() -> App<Form> {
        App::new(Form::default(), |state: &Form, cx: &mut Cx<Form>| {
            let theme = *cx.theme();
            crate::widgets::text_field(cx, &theme, &state.name, |s: &mut Form, k| {
                crate::widgets::edit_string(&mut s.name, k)
            })
            .width(300.0)
            .height(100.0)
        })
        .logical_size(Size::new(300.0, 100.0))
    }

    #[test]
    fn focus_and_type_edits_state() {
        let mut app = form_app();
        // Typing with nothing focused is a no-op.
        assert!(!app.type_text("x"));
        assert_eq!(app.state().name, "");

        // Focus the field by clicking it, then type.
        app.click_at(Point::new(150.0, 50.0));
        assert!(app.focused().is_some());
        assert!(app.type_text("Ada"));
        assert_eq!(app.state().name, "Ada");

        // Backspace removes the last character.
        assert!(app.press_key(KeyInput::Backspace));
        assert_eq!(app.state().name, "Ad");
    }

    #[derive(Default)]
    struct Toggles {
        on: bool,
    }

    #[test]
    fn checkbox_toggles_on_click() {
        let mut app = App::new(Toggles::default(), |s: &Toggles, cx: &mut Cx<Toggles>| {
            let theme = *cx.theme();
            // A checkbox filling the window for a predictable hit point.
            crate::widgets::checkbox(cx, &theme, s.on, |t: &mut Toggles| t.on = !t.on)
                .width(100.0)
                .height(100.0)
        })
        .logical_size(Size::new(100.0, 100.0));

        assert!(!app.state().on);
        app.click_at(Point::new(50.0, 50.0));
        assert!(app.state().on);
        app.click_at(Point::new(50.0, 50.0));
        assert!(!app.state().on);
    }

    #[derive(Default)]
    struct Volume {
        level: f64,
    }

    #[test]
    fn slider_drag_sets_value_from_position() {
        let mut app = App::new(Volume::default(), |s: &Volume, cx: &mut Cx<Volume>| {
            let theme = *cx.theme();
            crate::widgets::slider(cx, &theme, s.level, |v: &mut Volume, f| v.level = f)
                .width(200.0)
        })
        .logical_size(Size::new(200.0, 40.0));

        // Press at x=150 of a 200-wide slider -> fraction 0.75.
        app.drag_at_point(Point::new(150.0, 20.0));
        assert!(
            (app.state().level - 0.75).abs() < 1e-9,
            "got {}",
            app.state().level
        );
        // Drag to x=50 -> 0.25.
        app.drag_at_point(Point::new(50.0, 20.0));
        assert!((app.state().level - 0.25).abs() < 1e-9);
        app.end_drag();
    }

    #[test]
    fn a_mirrored_slider_fills_and_drags_from_the_right() {
        use crate::layout::LayoutDirection;
        let mut app = App::new(Volume { level: 0.25 }, |s: &Volume, cx: &mut Cx<Volume>| {
            let theme = *cx.theme();
            crate::widgets::slider(cx, &theme, s.level, |v: &mut Volume, f| v.level = f)
                .width(200.0)
        })
        .theme(Theme::light().with_layout_direction(LayoutDirection::Rtl))
        .logical_size(Size::new(200.0, 40.0));
        app.render_once();
        // The knob (the slider's 16-wide child) sits a quarter of the way in
        // from the right: 3px padding, then 0.25 of the 178px of travel.
        let tree = app.panes[0].tree.clone().unwrap();
        let slider = &tree;
        assert!(slider.rtl);
        let knob = slider
            .children
            .iter()
            .find(|c| c.bounds.width() == 16.0)
            .unwrap()
            .bounds;
        assert!(
            (knob.max_x() - (200.0 - 3.0 - 0.25 * 178.0)).abs() < 1e-9,
            "{knob:?}"
        );
        // A press near the right end is near 0; near the left end, near 1.
        app.drag_at_point(Point::new(150.0, 20.0));
        assert!((app.state().level - 0.25).abs() < 1e-9);
        app.drag_at_point(Point::new(20.0, 20.0));
        assert!((app.state().level - 0.9).abs() < 1e-9);
        app.end_drag();
    }

    #[test]
    fn tab_focuses_first_field() {
        let mut app = form_app();
        assert!(app.focused().is_none());
        assert!(app.focus_next());
        assert!(app.focused().is_some());
        assert!(app.type_text("hi"));
        assert_eq!(app.state().name, "hi");
    }

    #[test]
    fn accessibility_tree_reflects_focus() {
        use crate::core::Role;
        // A container root holding a field — the realistic shape (the root is
        // the Window, the field a nested TextField).
        let mut app = App::new(Form::default(), |state: &Form, cx: &mut Cx<Form>| {
            let theme = *cx.theme();
            let field = crate::widgets::text_field(cx, &theme, &state.name, |s: &mut Form, k| {
                crate::widgets::edit_string(&mut s.name, k)
            });
            column(vec![field])
        })
        .logical_size(Size::new(300.0, 100.0));
        app.focus_next(); // focus the field
        let tree = app.accessibility_tree().expect("a frame was built");
        assert_eq!(tree.role, Role::Window);
        // Exactly one node is focused, and it's a text field.
        let focused: Vec<_> = tree
            .descendants()
            .into_iter()
            .filter(|n| n.focused)
            .collect();
        assert_eq!(focused.len(), 1);
        assert_eq!(focused[0].role, Role::TextField);
    }

    fn mods(shift: bool, ctrl: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            shift,
            ctrl,
            alt,
            meta,
        }
    }

    #[test]
    fn undo_redo_shortcuts_map() {
        let none = Modifiers::default();
        let ctrl = mods(false, true, false, false);
        let ctrl_shift = mods(true, true, false, false);
        let cmd = mods(false, false, false, true);
        let cmd_shift = mods(true, false, false, true);
        for mac in [false, true] {
            let m = |c, md| map_key_for(KeyCode::Char(c), md, mac);
            assert_eq!(m('z', ctrl), Some(KeyInput::Undo));
            assert_eq!(m('z', cmd), Some(KeyInput::Undo));
            assert_eq!(m('z', ctrl_shift), Some(KeyInput::Redo));
            assert_eq!(m('Z', ctrl_shift), Some(KeyInput::Redo));
            assert_eq!(m('Z', cmd_shift), Some(KeyInput::Redo));
            assert_eq!(m('z', none), None);
        }
        assert_eq!(
            map_key_for(KeyCode::Char('y'), ctrl, false),
            Some(KeyInput::Redo)
        );
        // Ctrl+Y isn't redo on macOS.
        assert_eq!(map_key_for(KeyCode::Char('y'), ctrl, true), None);
    }

    #[test]
    fn word_shortcuts_use_ctrl_or_mac_alt() {
        let ctrl = mods(false, true, false, false);
        let ctrl_shift = mods(true, true, false, false);
        let alt = mods(false, false, true, false);
        let alt_shift = mods(true, false, true, false);
        let cases = [
            (
                KeyCode::ArrowLeft,
                KeyInput::WordLeft,
                KeyInput::SelectWordLeft,
            ),
            (
                KeyCode::ArrowRight,
                KeyInput::WordRight,
                KeyInput::SelectWordRight,
            ),
        ];
        for (code, plain, shifted) in cases {
            assert_eq!(map_key_for(code, ctrl, false), Some(plain.clone()));
            assert_eq!(map_key_for(code, ctrl_shift, false), Some(shifted.clone()));
            assert_eq!(map_key_for(code, alt, true), Some(plain));
            assert_eq!(map_key_for(code, alt_shift, true), Some(shifted));
        }
        assert_eq!(
            map_key_for(KeyCode::Backspace, ctrl, false),
            Some(KeyInput::DeleteWordBackward)
        );
        assert_eq!(
            map_key_for(KeyCode::Delete, ctrl, false),
            Some(KeyInput::DeleteWordForward)
        );
        assert_eq!(
            map_key_for(KeyCode::Backspace, alt, true),
            Some(KeyInput::DeleteWordBackward)
        );
        assert_eq!(
            map_key_for(KeyCode::Delete, alt, true),
            Some(KeyInput::DeleteWordForward)
        );
        // The other platform's word modifier is a plain motion.
        assert_eq!(
            map_key_for(KeyCode::ArrowLeft, alt, false),
            Some(KeyInput::Left)
        );
        assert_eq!(
            map_key_for(KeyCode::Backspace, ctrl, true),
            Some(KeyInput::Backspace)
        );
    }

    #[test]
    fn click_counter_counts_nearby_quick_presses() {
        use crate::platform::time::Instant;
        use std::time::Duration;
        let t0 = Instant::now();
        let p = Point::new(10.0, 10.0);
        let mut c = ClickCounter::default();
        assert_eq!(c.press(t0, p), 1);
        assert_eq!(
            c.press(t0 + Duration::from_millis(200), Point::new(12.0, 9.0)),
            2
        );
        assert_eq!(c.press(t0 + Duration::from_millis(400), p), 3);
        // A fourth press starts over.
        assert_eq!(c.press(t0 + Duration::from_millis(600), p), 1);
        // Too slow: the gap is measured from the previous press.
        assert_eq!(c.press(t0 + Duration::from_millis(1200), p), 1);
        assert_eq!(c.press(t0 + Duration::from_millis(1700), p), 2);
        // Too far away.
        assert_eq!(
            c.press(t0 + Duration::from_millis(1800), Point::new(20.0, 10.0)),
            1
        );
        // Reset forgets the last press.
        c.reset();
        assert_eq!(
            c.press(t0 + Duration::from_millis(1850), Point::new(20.0, 10.0)),
            1
        );
    }

    #[derive(Default)]
    struct Editor {
        buf: crate::widgets::EditBuffer,
    }

    /// A window that records what the App tells its input method.
    struct ImeWindow(std::cell::RefCell<Vec<Option<crate::platform::ImeState>>>);

    impl crate::platform::Window for ImeWindow {
        fn id(&self) -> WindowId {
            WindowId(1)
        }
        fn inner_size(&self) -> PhysicalSize {
            PhysicalSize::new(300, 100)
        }
        fn scale_factor(&self) -> ScaleFactor {
            ScaleFactor::IDENTITY
        }
        fn request_redraw(&self) {}
        fn set_title(&self, _title: &str) {}
        fn create_surface(&self) -> Box<dyn Surface> {
            unimplemented!("not presented in this test")
        }
        fn set_ime(&self, state: Option<&crate::platform::ImeState>) {
            self.0.borrow_mut().push(state.cloned());
        }
    }

    #[derive(Default)]
    struct TwoFields {
        a: crate::widgets::EditBuffer,
        b: crate::widgets::EditBuffer,
    }

    #[test]
    fn ime_follows_text_focus_and_drops_a_stale_preedit() {
        let mut app = App::new(
            TwoFields::default(),
            |s: &TwoFields, cx: &mut Cx<TwoFields>| {
                let theme = *cx.theme();
                let a = crate::widgets::text_editor(cx, &theme, &s.a, |s: &mut TwoFields| &mut s.a);
                let b = crate::widgets::text_editor(cx, &theme, &s.b, |s: &mut TwoFields| &mut s.b);
                column(vec![a, b])
            },
        )
        .logical_size(Size::new(300.0, 100.0));
        let window = ImeWindow(Default::default());
        app.render_once();
        app.panes[0].sync_ime(&window, None);
        // Nothing focused: the IME stays off, and isn't told again.
        assert!(window.0.borrow().is_empty());

        // Focusing a field enables it.
        assert!(app.focus_next());
        app.render_once();
        app.panes[0].sync_ime(&window, None);
        assert_eq!(window.0.borrow().len(), 1);
        assert!(window.0.borrow()[0].is_some());

        // Composition shows in the first field, then focus moves on.
        let preedit = KeyInput::Preedit {
            text: "か".into(),
            cursor: Some((3, 3)),
        };
        assert!(app.press_key(preedit));
        app.panes[0].preedit_owner = app.panes[0].focused;
        assert_eq!(app.state().a.preedit(), Some("か"));
        assert!(app.focus_next());
        let App { state, panes, .. } = &mut app;
        panes[0].drop_stale_preedit(state);
        assert_eq!(app.state().a.preedit(), None);
        assert_eq!(app.state().a.text(), "");
        assert_eq!(app.panes[0].preedit_owner, None);
    }

    #[test]
    fn double_click_selects_word_and_triple_click_selects_all() {
        use crate::platform::time::Instant;
        use std::time::Duration;
        let Some(font) = Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let state = Editor {
            buf: crate::widgets::EditBuffer::from_text("hello world"),
        };
        let mut app = App::new(state, |s: &Editor, cx: &mut Cx<Editor>| {
            let theme = *cx.theme();
            crate::widgets::text_editor(cx, &theme, &s.buf, |s: &mut Editor| &mut s.buf)
                .width(300.0)
                .height(40.0)
        })
        .logical_size(Size::new(300.0, 40.0))
        .font(font);
        app.render_once();
        // Find a point over "world": the far right of the text, but well inside
        // the field (the text starts after the field's padding).
        let tree = app.panes[0].tree.clone().unwrap();
        let leaf = {
            fn first_text(n: &LayoutNode) -> Option<&LayoutNode> {
                if matches!(n.content, crate::core::NodeContent::Text { .. }) {
                    return Some(n);
                }
                n.children.iter().find_map(first_text)
            }
            first_text(&tree).unwrap().bounds
        };
        let at = Point::new(leaf.max_x() - 8.0, leaf.center().y);
        let press = |app: &mut App<Editor>, t: Instant| {
            let App {
                state,
                theme,
                font,
                panes,
                ..
            } = app;
            assert!(panes[0].text_press_at(state, theme, font.as_ref(), at, t));
            panes[0].end_text_select();
        };
        let t0 = Instant::now();
        press(&mut app, t0);
        assert_eq!(app.state().buf.selection(), None);
        press(&mut app, t0 + Duration::from_millis(150));
        assert_eq!(app.state().buf.selected_text().as_deref(), Some("world"));
        press(&mut app, t0 + Duration::from_millis(300));
        assert_eq!(
            app.state().buf.selected_text().as_deref(),
            Some("hello world")
        );
        // A slow second press is a fresh single click.
        press(&mut app, t0 + Duration::from_millis(2000));
        press(&mut app, t0 + Duration::from_millis(2600));
        assert_eq!(app.state().buf.selection(), None);
    }
}
