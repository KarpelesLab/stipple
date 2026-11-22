//! [`AppHandle`]: changing app state from timers and other threads.

use crate::platform::time::Instant;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::core::canvas::{CanvasChange, CanvasId, CanvasInvalidation};
use crate::core::runtime::ViewportId;
use crate::core::{Cx, Element};
use crate::geometry::{Rect, Vec2};
use crate::platform::wake;
use crate::platform::{UserAttention, WindowId, WindowRequest};
use crate::platform::{WindowAttributes, WindowInfo};
use crate::render::ExternalImage;
use crate::render::Pixmap;
use std::collections::HashMap;

type Update<S> = Box<dyn FnOnce(&mut S) + Send>;
type Tick<S> = Box<dyn FnMut(&mut S) + Send>;
/// What a message box's answer does to the state.
pub(crate) type Answer<S> = Box<dyn FnOnce(&mut S, MessageResponse) + Send>;

use crate::platform::dialog::{MessageBox, MessageResponse};
use crate::widgets::{EditBuffer, PasswordField};

/// A message box asked for, not shown yet.
struct Asked<S> {
    mb: MessageBox,
    answer: Answer<S>,
}

/// The text field of a drawn message box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AlertInput {
    Plain(EditBuffer),
    Secure(PasswordField),
}

impl AlertInput {
    fn new(input: &crate::platform::dialog::MessageInput) -> Self {
        if input.secure {
            let mut p = PasswordField::new().reveal_button(true);
            p.set_text(input.value.clone());
            AlertInput::Secure(p)
        } else {
            let mut b = EditBuffer::from_text(input.value.clone());
            b.select_all();
            AlertInput::Plain(b)
        }
    }

    pub(crate) fn text(&self) -> &str {
        match self {
            AlertInput::Plain(b) => b.text(),
            AlertInput::Secure(p) => p.text(),
        }
    }
}

/// A message box drawn in the window (where there's no native alert), up
/// until answered.
struct Drawn<S> {
    id: u64,
    mb: MessageBox,
    input: Option<AlertInput>,
    answer: Answer<S>,
}

/// What the view needs to draw the first drawn message box.
#[derive(Clone, Debug)]
pub(crate) struct AlertView {
    pub id: u64,
    pub mb: MessageBox,
    pub input: Option<AlertInput>,
}

/// A window's view, as [`AppHandle::open_window`] takes it: like
/// `ViewFn`, but able to cross threads.
pub type SendViewFn<S> = Box<dyn FnMut(&S, &mut Cx<'_, S>) -> Element + Send>;

/// A window [`AppHandle::open_window`] asked for, waiting for the UI thread.
pub(crate) struct OpenRequest<S> {
    pub(crate) id: WindowId,
    pub(crate) attrs: WindowAttributes,
    pub(crate) view: SendViewFn<S>,
}

/// Identifies a timer, to [`cancel`](AppHandle::cancel) it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

struct Timer<S> {
    id: TimerId,
    due: Instant,
    /// `Some` for a repeating timer.
    every: Option<Duration>,
    tick: Tick<S>,
}

struct Shared<S> {
    updates: Vec<Update<S>>,
    timers: Vec<Timer<S>>,
    next_id: u64,
    exit: bool,
    /// Canvas areas to repaint, queued by [`AppHandle::invalidate_canvas`].
    canvases: Vec<CanvasInvalidation>,
    /// Viewport content queued by [`AppHandle::set_viewport_content`]
    /// (`None` clears it).
    viewports: Vec<(ViewportId, Option<ViewportContent>)>,
    /// Message boxes asked for, and those drawn in the window.
    asked: Vec<Asked<S>>,
    drawn: Vec<Drawn<S>>,
    /// Window changes queued by [`AppHandle::window`] and
    /// [`AppHandle::window_to`], with the window they are for.
    window: Vec<(WindowId, WindowRequest)>,
    /// Windows to open and close, queued by [`AppHandle::open_window`] and
    /// [`AppHandle::close_window`].
    opens: Vec<OpenRequest<S>>,
    closes: Vec<WindowId>,
    /// The next [`WindowId`] to hand out ([`WindowId::PRIMARY`] is 0).
    next_window: u64,
    /// What the app last learned of each open window.
    info: HashMap<WindowId, WindowInfo>,
    /// The backend's id for each open window, for native message boxes.
    natives: HashMap<WindowId, WindowId>,
    /// Focus changes queued by [`AppHandle::focus`] and [`AppHandle::blur`].
    focus: Vec<(WindowId, crate::core::runtime::FocusRequest)>,
}

/// A handle to a running app, for changing its state from outside a widget
/// handler: on a timer, or from another thread.
///
/// Every change runs on the UI thread, between events, as a closure over the
/// state, so the state needs no locks. After changes run, the app re-renders.
///
/// Get one from [`App::handle`](crate::App::handle) or, inside a view,
/// [`Cx::handle`](crate::core::Cx::handle). Handles are cheap to clone and can
/// be sent to any thread.
///
/// ```no_run
/// # use stipple::prelude::*;
/// # struct State { result: Option<String> }
/// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
/// let app = App::new(State { result: None }, view);
/// let handle = app.handle();
/// std::thread::spawn(move || {
///     let result = String::from("loaded"); // slow work, off the UI thread
///     handle.update(move |s| s.result = Some(result));
/// });
/// app.run();
/// ```
pub struct AppHandle<S> {
    shared: Arc<Mutex<Shared<S>>>,
}

impl<S> Clone for AppHandle<S> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<S> std::fmt::Debug for AppHandle<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppHandle").finish_non_exhaustive()
    }
}

impl<S> Default for AppHandle<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> AppHandle<S> {
    /// A handle not yet attached to a running app. Changes queue until one runs.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                updates: Vec::new(),
                timers: Vec::new(),
                next_id: 1,
                exit: false,
                window: Vec::new(),
                canvases: Vec::new(),
                viewports: Vec::new(),
                asked: Vec::new(),
                drawn: Vec::new(),
                opens: Vec::new(),
                closes: Vec::new(),
                next_window: 1,
                info: HashMap::new(),
                focus: Vec::new(),
                natives: HashMap::new(),
            })),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Shared<S>) -> R) -> R {
        // A panicking update can't leave the queue half-written, so a poisoned
        // lock is still usable.
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut shared)
    }

    /// Change the state on the UI thread, then re-render.
    pub fn update(&self, f: impl FnOnce(&mut S) + Send + 'static) {
        self.with(|s| s.updates.push(Box::new(f)));
        wake::wake();
    }

    /// Ask the user for `permission`
    /// ([`platform::permission::request`](crate::platform::permission::request)),
    /// then apply the answer to the state on the UI thread with `answer`. If
    /// it was decided already, no prompt shows and the answer comes at once
    /// (on the next turn of the loop). Call it from the UI thread (a handler).
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::platform::permission::Permission;
    /// # struct State { camera_allowed: bool }
    /// # fn ask(cx: &mut Cx<State>) {
    /// cx.handle().request_permission(Permission::Camera, |s, status| {
    ///     s.camera_allowed = status.is_granted();
    /// });
    /// # }
    /// ```
    pub fn request_permission(
        &self,
        permission: crate::platform::permission::Permission,
        answer: impl FnOnce(&mut S, crate::platform::permission::PermissionStatus) + Send + 'static,
    ) where
        S: 'static,
    {
        let handle = self.clone();
        crate::platform::permission::request(permission, move |status| {
            handle.update(move |s| answer(s, status));
        });
    }

    /// Read the packaged file at `path` ([`platform::assets`](crate::platform::assets))
    /// and `decode` it off the UI thread, then `apply` the result to the
    /// state on the UI thread: a scene's art loads behind a loading screen,
    /// and goes when the state drops it. On the web the file is fetched, and
    /// decoded on the page's thread once it arrives.
    ///
    /// `decode` takes the bytes: `Pixmap::decode`, `Svg::parse`,
    /// `Sound::decode`, or the app's own; its error becomes
    /// [`AssetError::Decode`](crate::platform::AssetError::Decode).
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # use std::sync::Arc;
    /// # struct State { background: Option<Arc<Pixmap>> }
    /// # fn enter(cx: &mut Cx<State>) {
    /// cx.handle().load_asset("scenes/forest.png", Pixmap::decode, |s: &mut State, r| {
    ///     match r {
    ///         Ok(pixmap) => s.background = Some(Arc::new(pixmap)),
    ///         Err(e) => eprintln!("{e}"),
    ///     }
    /// });
    /// # }
    /// ```
    pub fn load_asset<T, E>(
        &self,
        path: &str,
        decode: impl FnOnce(&[u8]) -> Result<T, E> + Send + 'static,
        apply: impl FnOnce(&mut S, Result<T, crate::platform::AssetError>) + Send + 'static,
    ) where
        T: Send + 'static,
        E: std::fmt::Display,
        S: 'static,
    {
        let handle = self.clone();
        let owned = path.to_owned();
        crate::platform::assets().load(path, move |bytes| {
            let result = bytes.and_then(|b| {
                decode(&b).map_err(|e| crate::platform::AssetError::Decode {
                    path: owned,
                    message: e.to_string(),
                })
            });
            handle.update(move |s| apply(s, result));
        });
    }

    /// Run `f` on the state once, after `delay`.
    pub fn after(&self, delay: Duration, f: impl FnOnce(&mut S) + Send + 'static) -> TimerId {
        let mut f = Some(f);
        self.add_timer(delay, None, move |s| {
            if let Some(f) = f.take() {
                f(s);
            }
        })
    }

    /// Run `f` on the state every `interval`, until [`cancel`](Self::cancel)led.
    pub fn every(&self, interval: Duration, f: impl FnMut(&mut S) + Send + 'static) -> TimerId {
        self.add_timer(interval, Some(interval), f)
    }

    fn add_timer(
        &self,
        delay: Duration,
        every: Option<Duration>,
        f: impl FnMut(&mut S) + Send + 'static,
    ) -> TimerId {
        let id = self.with(|s| {
            let id = TimerId(s.next_id);
            s.next_id += 1;
            s.timers.push(Timer {
                id,
                due: crate::platform::time::now() + delay,
                every,
                tick: Box::new(f),
            });
            id
        });
        wake::wake(); // so the loop picks up the new deadline
        id
    }

    /// Stop a timer. Does nothing if it already fired or was cancelled.
    pub fn cancel(&self, id: TimerId) {
        self.with(|s| s.timers.retain(|t| t.id != id));
    }

    /// End the app's event loop, as closing its last window would.
    pub fn exit(&self) {
        self.with(|s| s.exit = true);
        wake::wake();
    }

    /// Change the primary window while the app runs: retitle, move or resize
    /// it, maximize it, make it fullscreen, change its size limits or icon.
    /// Applied on the UI thread between events; a no-op on backends without
    /// that control. Use [`window_to`](Self::window_to) for other windows.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # use stipple::platform::WindowRequest;
    /// # struct State;
    /// # fn view(_: &State, cx: &mut Cx<State>) -> Element {
    /// let handle = cx.handle();
    /// button(cx, "Fullscreen")
    ///     .on_tap(cx, move |_| handle.window(WindowRequest::Fullscreen(true)))
    /// # }
    /// ```
    pub fn window(&self, request: WindowRequest) {
        self.window_to(WindowId::PRIMARY, request);
    }

    /// Change window `id` while the app runs (see [`window`](Self::window)).
    /// Requests for a window that has closed are dropped.
    pub fn window_to(&self, id: WindowId, request: WindowRequest) {
        self.with(|s| s.window.push((id, request)));
        wake::wake();
    }

    /// Open another window while the app runs, showing `view` onto the shared
    /// state, and return its id. Callable from any thread; the window opens
    /// on the UI thread at the next turn of the event loop. Inside the view,
    /// [`Cx::window_id`](crate::core::Cx::window_id) tells which window is
    /// being built, so one view function can serve several windows.
    ///
    /// Backends with one window (iOS, Android, the web) ignore it.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # use stipple::platform::WindowAttributes;
    /// # struct State;
    /// # fn editor(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// # fn f(handle: AppHandle<State>) {
    /// let id = handle.open_window(WindowAttributes::new().title("Untitled"), editor);
    /// // ...later
    /// handle.close_window(id);
    /// # }
    /// ```
    pub fn open_window(
        &self,
        attrs: WindowAttributes,
        view: impl FnMut(&S, &mut Cx<'_, S>) -> Element + Send + 'static,
    ) -> WindowId {
        let id = self.with(|s| {
            let id = WindowId(s.next_window);
            s.next_window += 1;
            s.opens.push(OpenRequest {
                id,
                attrs,
                view: Box::new(view),
            });
            id
        });
        wake::wake();
        id
    }

    /// Close window `id` without asking
    /// [`App::on_close_requested`](crate::App::on_close_requested): what an
    /// app calls once the user confirmed a close it cancelled earlier, or to
    /// close a window it opened. The id is the one
    /// [`App::open_window`](crate::App::open_window) / [`open_window`](Self::open_window)
    /// returned, the close hook was given, or
    /// [`Cx::window_id`](crate::core::Cx::window_id) says. Closing the last
    /// window ends the app unless
    /// [`App::exit_on_last_window_closed`](crate::App::exit_on_last_window_closed)
    /// says otherwise. Does nothing if it already closed.
    pub fn close_window(&self, id: WindowId) {
        self.with(|s| s.closes.push(id));
        wake::wake();
    }

    /// Where window `id` is and what state it is in, as of the last event
    /// the app handled for it; `None` once it has closed (or before it
    /// opened).
    pub fn window_info(&self, id: WindowId) -> Option<WindowInfo> {
        self.with(|s| s.info.get(&id).copied())
    }

    /// The ids of the open windows, in the order they opened.
    pub fn windows(&self) -> Vec<WindowId> {
        self.with(|s| {
            let mut ids: Vec<WindowId> = s.info.keys().copied().collect();
            ids.sort_by_key(|id| id.0);
            ids
        })
    }

    /// Ask for the user's attention without stealing focus: flash the taskbar
    /// button, bounce the Dock icon, mark the window urgent.
    pub fn request_attention(&self, kind: UserAttention) {
        self.window(WindowRequest::Attention(Some(kind)));
    }

    /// Lock the primary window's pointer (`true`): the cursor hides and stays
    /// put, and mouse motion reaches `on_pointer` handlers as
    /// [`PointerEventKind::Motion`](crate::core::PointerEventKind::Motion)
    /// events with their [`movement`](crate::core::PointerEvent::movement);
    /// or release it (`false`). The lock ends by itself when the window
    /// loses focus; [`App::on_pointer_lock`](crate::App::on_pointer_lock)
    /// hears when it starts, ends or is refused, and
    /// [`Cx::pointer_locked`](crate::core::Cx::pointer_locked) tells the
    /// view. On the web, lock from a click or key handler: browsers only
    /// lock during a user gesture. Other windows: [`window_to`](Self::window_to)
    /// with [`WindowRequest::PointerLock`]. See `docs/pointer-events.md`.
    pub fn set_pointer_lock(&self, locked: bool) {
        self.window(WindowRequest::PointerLock(locked));
    }

    /// Keep the screen from dimming and locking while the app shows (`true`),
    /// as a game the player watches, a recipe being cooked from or a
    /// presentation want; `false` (the default) lets it sleep again. A phone
    /// keeps it only while the app is in the foreground. iOS
    /// (`idleTimerDisabled`), Android (`FLAG_KEEP_SCREEN_ON`), the web (the
    /// Screen Wake Lock API, asked again whenever the page shows), macOS (a
    /// power assertion), Windows (`SetThreadExecutionState`) and Linux (the
    /// desktop portal's or the screen saver's `Inhibit`).
    pub fn set_keep_awake(&self, on: bool) {
        self.window(WindowRequest::KeepAwake(on));
    }

    /// Let a phone or tablet turn the app only these ways from now on (see
    /// [`App::orientations`](crate::App::orientations)): lock a minigame to
    /// landscape, let it turn freely again with `Orientations::ALL`. If the
    /// device is turned a way that isn't allowed, the app turns to one that
    /// is. A no-op on desktops and the web.
    pub fn set_orientations(&self, set: crate::platform::Orientations) {
        self.window(WindowRequest::Orientations(set));
    }

    /// Sound the system beep, as a terminal's audible bell.
    pub fn beep(&self) {
        self.window(WindowRequest::Beep);
    }

    /// Show `label` as a badge on the application's icon, or remove it with
    /// `None`: the Dock tile's badge on macOS, an overlay icon on the
    /// taskbar button on Windows (the number, or a dot for a longer label),
    /// the launcher's count on Linux (numbers only). See
    /// `docs/window-lifecycle.md`.
    pub fn set_badge(&self, label: Option<&str>) {
        self.window(WindowRequest::Badge(label.map(str::to_string)));
    }

    /// Show how far a long task has got, from 0 to 1, as a bar on the
    /// application's icon (the Dock tile, the taskbar button, the Linux
    /// launcher), or remove it with `None`.
    pub fn set_progress(&self, progress: Option<f64>) {
        self.window(WindowRequest::Progress(progress));
    }

    /// Open `url` in the user's default app for it (a browser for `https:`, the
    /// file's app for `file:`). See
    /// [`platform::open_url`](crate::platform::open_url).
    pub fn open_url(&self, url: &str) -> Result<(), crate::platform::PlatformError> {
        crate::platform::open_url(url)
    }

    /// Show a desktop notification. See
    /// [`platform::notify`](crate::platform::notify).
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # struct State;
    /// # fn f(handle: AppHandle<State>) {
    /// let _ = handle.notify(&Notification::new("Export finished").body("report.pdf"));
    /// # }
    /// ```
    pub fn notify(
        &self,
        notification: &crate::platform::Notification,
    ) -> Result<(), crate::platform::PlatformError> {
        crate::platform::notify(notification)
    }

    /// Show the system's share sheet with `share`, and apply how it ended to
    /// the state: `done` runs on the UI thread as an update. Call it from a
    /// handler (the button the user pressed); see
    /// [`platform::share`](crate::platform::share) for each platform.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::platform::{Share, ShareOutcome};
    /// # struct State { status: String }
    /// # fn f(handle: AppHandle<State>, png: Vec<u8>) {
    /// handle.share(
    ///     Share::new().text("My code").file("code.png", png, "image/png"),
    ///     |s: &mut State, outcome| {
    ///         s.status = match outcome {
    ///             ShareOutcome::Shared => "Shared".into(),
    ///             ShareOutcome::Unavailable => "Can't share here".into(),
    ///             _ => String::new(),
    ///         };
    ///     },
    /// );
    /// # }
    /// ```
    pub fn share(
        &self,
        share: crate::platform::Share,
        done: impl FnOnce(&mut S, crate::platform::ShareOutcome) + Send + 'static,
    ) where
        S: 'static,
    {
        let handle = self.clone();
        crate::platform::share(&share, move |outcome| {
            handle.update(move |s| done(s, outcome));
        });
    }

    /// The app's [store](crate::platform::store), with callbacks that run as
    /// state updates on the UI thread.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::platform::{ProductId, PurchaseResult};
    /// # struct State { prices: Vec<String>, status: String }
    /// # fn f(handle: AppHandle<State>) {
    /// handle.store().products(vec![ProductId::consumable("gems_100")], |s: &mut State, r| {
    ///     s.prices = r.map(|ps| ps.into_iter().map(|p| p.price).collect()).unwrap_or_default();
    /// });
    /// handle.store().purchase("gems_100", |s: &mut State, r| {
    ///     s.status = match r {
    ///         PurchaseResult::Failed(e) => e.to_string(),
    ///         _ => String::new(), // the gems arrive at App::on_transaction
    ///     };
    /// });
    /// # }
    /// ```
    pub fn store(&self) -> StoreHandle<S> {
        StoreHandle {
            handle: self.clone(),
        }
    }

    /// Repaint all of [canvas](crate::core::Element::canvas) `id`, without
    /// rebuilding the view: its paint callback runs again and only its area
    /// is re-rasterized and presented. Safe from any thread; repaints
    /// queued before the app gets to them are coalesced.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # use std::sync::{Arc, Mutex};
    /// # struct State;
    /// # fn f(handle: AppHandle<State>, lines: Arc<Mutex<Vec<String>>>) {
    /// std::thread::spawn(move || {
    ///     lines.lock().unwrap().push("new output".into());
    ///     handle.invalidate_canvas(CanvasId(1));
    /// });
    /// # }
    /// ```
    pub fn invalidate_canvas(&self, id: CanvasId) {
        self.with(|s| s.canvases.push((id, CanvasChange::Repaint(None))));
        wake::wake();
    }

    /// Repaint the part `rect` (in the canvas's own coordinates) of
    /// [canvas](crate::core::Element::canvas) `id`, without rebuilding the
    /// view: only that area of the window is re-rasterized and presented, and
    /// the canvas's paint callback is asked for just that area. A terminal
    /// passes the rows that changed. Safe from any thread.
    pub fn invalidate_canvas_rect(&self, id: CanvasId, rect: Rect) {
        self.with(|s| s.canvases.push((id, CanvasChange::Repaint(Some(rect)))));
        wake::wake();
    }

    /// Move what [canvas](crate::core::Element::canvas) `id` shows in `rect`
    /// (its own coordinates) by `delta`, without repainting it: the pixels
    /// already on screen are copied, and the paint callback is asked only
    /// for the part of `rect` they uncover. What a terminal does when its
    /// output scrolls a line: `delta` is one row up, and the callback paints
    /// the new bottom row. Safe from any thread.
    ///
    /// It applies to the canvas as the app last painted it, so paint the
    /// data that moved before calling it, and invalidate what changed
    /// besides ([`invalidate_canvas_rect`](Self::invalidate_canvas_rect)),
    /// before or after: a pending repaint moves with the pixels. Pixels move
    /// with whatever shows through the canvas there, so `rect` should be
    /// one the canvas paints opaquely (a terminal fills its background).
    /// When they can't be moved as they are (a canvas drawn rotated, scaled,
    /// faded, or by a fraction of a device pixel), the whole of `rect` is
    /// repainted instead.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::geometry::Vec2;
    /// # use std::sync::{Arc, Mutex};
    /// # struct State;
    /// # const ROW: f64 = 16.0;
    /// # fn f(handle: AppHandle<State>, lines: Arc<Mutex<Vec<String>>>, size: Size) {
    /// // A new line at the bottom of a terminal: the rest scrolls up a row.
    /// lines.lock().unwrap().push("new output".into());
    /// let screen = Rect::from_xywh(0.0, 0.0, size.width, size.height);
    /// handle.scroll_canvas_rect(CanvasId(1), screen, Vec2::new(0.0, -ROW));
    /// # }
    /// ```
    pub fn scroll_canvas_rect(&self, id: CanvasId, rect: Rect, delta: Vec2) {
        self.with(|s| s.canvases.push((id, CanvasChange::Scroll { rect, delta })));
        wake::wake();
    }

    /// Set (or replace) the externally-rendered content of the embedded
    /// [viewport](crate::core::Element::viewport) `id` while the app runs,
    /// from any thread: what a background renderer calls with each new frame.
    /// The content should be sized to the viewport's physical extent; it is
    /// composited over the viewport's placeholder and only the viewport's
    /// area is presented. See
    /// [`App::set_viewport_content`](crate::App::set_viewport_content).
    pub fn set_viewport_content(&self, id: ViewportId, content: Pixmap) {
        self.with(|s| {
            s.viewports
                .push((id, Some(ViewportContent::Pixels(content))))
        });
        wake::wake();
    }

    /// Show the external `image` (a `dma-buf`, an `IOSurface`, a DXGI
    /// shared handle, an `AHardwareBuffer`: a content process's or a video
    /// decoder's frame) in the embedded viewport `id` while the app runs,
    /// from any thread. A GPU renderer imports it without a copy; see
    /// [`App::set_viewport_external`](crate::App::set_viewport_external).
    pub fn set_viewport_external(&self, id: ViewportId, image: Arc<ExternalImage>) {
        self.with(|s| {
            s.viewports
                .push((id, Some(ViewportContent::External(image))))
        });
        wake::wake();
    }

    /// Remove the content of viewport `id` while the app runs, reverting it
    /// to its placeholder.
    pub fn clear_viewport_content(&self, id: ViewportId) {
        self.with(|s| s.viewports.push((id, None)));
        wake::wake();
    }

    /// Take the canvas repaints queued by
    /// [`invalidate_canvas`](Self::invalidate_canvas).
    pub(crate) fn take_canvas_invalidations(&self) -> Vec<CanvasInvalidation> {
        self.with(|s| std::mem::take(&mut s.canvases))
    }

    /// Take the viewport content queued by
    /// [`set_viewport_content`](Self::set_viewport_content).
    pub(crate) fn take_viewport_content(&self) -> Vec<(ViewportId, Option<ViewportContent>)> {
        self.with(|s| std::mem::take(&mut s.viewports))
    }

    /// Ask the user something with a message box, and apply the answer to
    /// the state: `answer` runs on the UI thread with the
    /// [`MessageResponse`] once the user presses a button, then the app
    /// re-renders. Callable from any thread; nothing blocks.
    ///
    /// Where the platform has an alert (Windows, macOS, iOS, the web; see
    /// [`dialog::message`](crate::platform::dialog::message)) it shows that.
    /// Elsewhere (Linux, Android), or for a box marked
    /// [`drawn`](MessageBox::drawn), it's drawn in the primary window in the
    /// theme: a modal panel over a scrim, with the default button focused
    /// (Enter presses it, Escape the cancel button). Boxes asked for while
    /// one is up wait their turn.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// use stipple::platform::dialog::{MessageBox, MessageKind};
    /// # struct State { tabs: Vec<String> }
    /// # fn f(handle: AppHandle<State>) {
    /// handle.message(
    ///     MessageBox::new("Close this tab?")
    ///         .detail("A process is still running in it.")
    ///         .kind(MessageKind::Question)
    ///         .buttons(["Close", "Cancel"])
    ///         .cancel(1),
    ///     |s: &mut State, r| {
    ///         if r.button == Some(0) {
    ///             s.tabs.pop();
    ///         }
    ///     },
    /// );
    /// # }
    /// ```
    ///
    /// A worker thread that must wait for the answer sends it back over a
    /// channel from `answer`, and waits on that instead of on the UI.
    pub fn message(
        &self,
        mb: MessageBox,
        answer: impl FnOnce(&mut S, MessageResponse) + Send + 'static,
    ) {
        self.with(|s| {
            s.asked.push(Asked {
                mb,
                answer: Box::new(answer),
            })
        });
        wake::wake();
    }

    /// Show the message boxes asked for: natively where possible, else queue
    /// them to be drawn. Returns whether any is to be drawn.
    fn start_messages(&self) -> bool
    where
        S: 'static,
    {
        let asked = self.with(|s| std::mem::take(&mut s.asked));
        let mut drawn = false;
        for Asked { mb, answer } in asked {
            let answer = std::sync::Arc::new(Mutex::new(Some(answer)));
            // The platform knows the window by its own id.
            let mut native_mb = mb.clone();
            native_mb.window = mb
                .window
                .and_then(|w| self.with(|s| s.natives.get(&w).copied()));
            let native = if crate::platform::dialog::native_allowed() {
                let handle = self.clone();
                let a = answer.clone();
                let done: crate::platform::dialog::MessageDone = Box::new(move |r| {
                    handle.update(move |s| {
                        if let Some(f) = a.lock().ok().and_then(|mut a| a.take()) {
                            f(s, r);
                        }
                    })
                });
                crate::platform::dialog::message(&native_mb, done).is_ok()
            } else {
                false
            };
            if native {
                continue;
            }
            let Some(answer) = answer.lock().ok().and_then(|mut a| a.take()) else {
                continue;
            };
            self.with(|s| {
                let id = s.next_id;
                s.next_id += 1;
                s.drawn.push(Drawn {
                    id,
                    input: mb.input.as_ref().map(AlertInput::new),
                    mb,
                    answer,
                })
            });
            drawn = true;
        }
        drawn
    }

    /// The drawn message box up now in window `window` (the first asked
    /// for it), to draw. The primary window also shows those for no window,
    /// or for one that has closed.
    pub(crate) fn alert_view(&self, window: WindowId, primary: bool) -> Option<AlertView> {
        self.with(|s| {
            let here = |d: &&Drawn<S>| match d.mb.window {
                Some(w) if s.info.contains_key(&w) => w == window,
                _ => primary,
            };
            s.drawn.iter().find(here).map(|d| AlertView {
                id: d.id,
                mb: d.mb.clone(),
                input: d.input.clone(),
            })
        })
    }

    /// Change drawn message box `id`'s text field.
    pub(crate) fn alert_input(&self, id: u64, f: impl FnOnce(&mut AlertInput)) {
        self.with(|s| {
            if let Some(input) = s
                .drawn
                .iter_mut()
                .find(|d| d.id == id)
                .and_then(|d| d.input.as_mut())
            {
                f(input);
            }
        });
    }

    /// Answer drawn message box `id` with `button` (`None`: dismissed), and
    /// apply the answer to `state`.
    pub(crate) fn answer_alert(&self, id: u64, button: Option<usize>, state: &mut S) {
        let Some(d) = self.with(|s| {
            let i = s.drawn.iter().position(|d| d.id == id)?;
            Some(s.drawn.remove(i))
        }) else {
            return;
        };
        let response = MessageResponse {
            button,
            input: d.input.as_ref().map(|i| i.text().to_string()),
        };
        (d.answer)(state, response);
    }

    /// Queue a focus change for window `id`, and re-render.
    pub(crate) fn request_focus(&self, id: WindowId, request: crate::core::runtime::FocusRequest) {
        self.with(|s| {
            s.focus.push((id, request));
            // An update that does nothing, so the window builds again.
            s.updates.push(Box::new(|_| {}));
        });
        wake::wake();
    }

    /// Take the focus changes queued for window `id`.
    pub(crate) fn take_focus_requests(
        &self,
        id: WindowId,
    ) -> Vec<crate::core::runtime::FocusRequest> {
        self.with(|s| {
            let (mine, rest) = std::mem::take(&mut s.focus)
                .into_iter()
                .partition(|(to, _)| *to == id);
            s.focus = rest;
            mine.into_iter().map(|(_, r)| r).collect()
        })
    }

    /// Take the window changes queued for window `id`.
    pub(crate) fn take_window_requests(&self, id: WindowId) -> Vec<WindowRequest> {
        self.with(|s| {
            let (mine, rest) = std::mem::take(&mut s.window)
                .into_iter()
                .partition(|(to, _)| *to == id);
            s.window = rest;
            mine.into_iter().map(|(_, r)| r).collect()
        })
    }

    /// Drop queued changes for windows that aren't open (any more).
    pub(crate) fn drop_window_requests(&self, open: &[WindowId]) {
        self.with(|s| s.window.retain(|(to, _)| open.contains(to)));
    }

    /// A new window id, for a window the app opens itself.
    pub(crate) fn next_window_id(&self) -> WindowId {
        self.with(|s| {
            let id = WindowId(s.next_window);
            s.next_window += 1;
            id
        })
    }

    /// Take the windows queued to open.
    pub(crate) fn take_opens(&self) -> Vec<OpenRequest<S>> {
        self.with(|s| std::mem::take(&mut s.opens))
    }

    /// Take the windows queued to close.
    pub(crate) fn take_closes(&self) -> Vec<WindowId> {
        self.with(|s| std::mem::take(&mut s.closes))
    }

    /// Record what is known of window `id` (`None`: it closed).
    pub(crate) fn set_window_info(&self, id: WindowId, info: Option<WindowInfo>) {
        self.with(|s| match info {
            Some(info) => {
                s.info.insert(id, info);
            }
            None => {
                s.info.remove(&id);
                s.natives.remove(&id);
            }
        });
    }

    /// Record the backend's id for window `id`.
    pub(crate) fn set_native(&self, id: WindowId, native: WindowId) {
        self.with(|s| s.natives.insert(id, native));
    }

    /// Apply queued updates and due timers to `state`, as of `now`. Returns
    /// whether anything ran, and the next timer's deadline.
    pub(crate) fn pump(&self, state: &mut S, now: Instant) -> (bool, Option<Instant>)
    where
        S: 'static,
    {
        let mut ran = self.start_messages();
        // Take the work out before running it, so closures can use the handle
        // (queue an update, start a timer) without deadlocking.
        let updates = self.with(|s| std::mem::take(&mut s.updates));
        for update in updates {
            update(state);
            ran = true;
        }
        let due: Vec<Timer<S>> = self.with(|s| {
            let (due, later) = std::mem::take(&mut s.timers)
                .into_iter()
                .partition(|t| t.due <= now);
            s.timers = later;
            due
        });
        for mut timer in due {
            (timer.tick)(state);
            ran = true;
            if let Some(every) = timer.every {
                // Skip missed ticks rather than firing a burst after a stall.
                let mut next = timer.due + every;
                if next <= now {
                    next = now + every;
                }
                timer.due = next;
                self.with(|s| s.timers.push(timer));
            }
        }
        let next = self.with(|s| s.timers.iter().map(|t| t.due).min());
        (ran, next)
    }

    /// Whether [`exit`](Self::exit) was called.
    pub(crate) fn exit_requested(&self) -> bool {
        self.with(|s| s.exit)
    }
}

/// Run a future on a background [tokio](https://tokio.rs) runtime, then apply its
/// output to the state on the UI thread.
#[cfg(all(feature = "tokio", not(target_arch = "wasm32")))]
impl<S> AppHandle<S> {
    /// Run `future` on the shared tokio runtime (see [`runtime`]); when it
    /// finishes, `apply` its output to the state on the UI thread.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # struct State { loading: bool, page: String }
    /// # async fn fetch() -> String { String::new() }
    /// # fn tap(cx: &mut Cx<State>) {
    /// let handle = cx.handle();
    /// handle.spawn(fetch(), |s: &mut State, page| {
    ///     s.loading = false;
    ///     s.page = page;
    /// });
    /// # }
    /// ```
    pub fn spawn<F>(
        &self,
        future: F,
        apply: impl FnOnce(&mut S, F::Output) + Send + 'static,
    ) -> tokio::task::JoinHandle<()>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
        S: 'static,
    {
        let handle = self.clone();
        runtime().spawn(async move {
            let output = future.await;
            handle.update(move |s| apply(s, output));
        })
    }
}

/// The tokio runtime [`AppHandle::spawn`] runs futures on: multi-threaded, with
/// every driver your tokio features enable, started on first use. Its threads
/// are separate from the UI thread, which stays the OS's event loop.
#[cfg(all(feature = "tokio", not(target_arch = "wasm32")))]
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("stipple-tokio")
            .enable_all()
            .build()
            .expect("start the tokio runtime")
    })
}

/// The app's [store](crate::platform::store) from an [`AppHandle`]: the same
/// calls as [`Store`](crate::platform::Store), with callbacks that change the
/// state (run as [updates](AppHandle::update) on the UI thread).
pub struct StoreHandle<S> {
    handle: AppHandle<S>,
}

impl<S> Clone for StoreHandle<S> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
        }
    }
}

impl<S> std::fmt::Debug for StoreHandle<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreHandle").finish_non_exhaustive()
    }
}

impl<S: 'static> StoreHandle<S> {
    /// Which store this is.
    pub fn kind(&self) -> crate::platform::StoreKind {
        crate::platform::store().kind()
    }

    /// [`Store::products`](crate::platform::Store::products), applying the
    /// answer to the state.
    pub fn products(
        &self,
        ids: Vec<crate::platform::ProductId>,
        done: impl FnOnce(&mut S, Result<Vec<crate::platform::Product>, crate::platform::StoreError>)
        + Send
        + 'static,
    ) {
        let handle = self.handle.clone();
        crate::platform::store().products(&ids, move |r| handle.update(move |s| done(s, r)));
    }

    /// [`Store::purchase`](crate::platform::Store::purchase), applying how it
    /// ended to the state. Grant purchases in
    /// [`App::on_transaction`](crate::App::on_transaction), not here.
    pub fn purchase(
        &self,
        product_id: &str,
        done: impl FnOnce(&mut S, crate::platform::PurchaseResult) + Send + 'static,
    ) {
        let handle = self.handle.clone();
        crate::platform::store().purchase(product_id, move |r| handle.update(move |s| done(s, r)));
    }

    /// [`Store::finish`](crate::platform::Store::finish).
    pub fn finish(&self, transaction: &crate::platform::Transaction) {
        crate::platform::store().finish(transaction);
    }

    /// [`Store::restore_purchases`](crate::platform::Store::restore_purchases).
    pub fn restore_purchases(&self) {
        crate::platform::store().restore_purchases();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_run_in_order_on_pump() {
        let h = AppHandle::<Vec<u32>>::new();
        h.update(|s| s.push(1));
        h.update(|s| s.push(2));
        let mut state = Vec::new();
        let (ran, next) = h.pump(&mut state, crate::platform::time::now());
        assert!(ran);
        assert_eq!(state, [1, 2]);
        assert_eq!(next, None);
        assert!(
            !h.pump(&mut state, crate::platform::time::now()).0,
            "updates run once"
        );
    }

    #[test]
    fn timers_fire_when_due_and_repeat() {
        let h = AppHandle::<u32>::new();
        let start = crate::platform::time::now();
        h.after(Duration::from_millis(10), |s| *s += 100);
        let tick = h.every(Duration::from_millis(5), |s| *s += 1);
        let mut n = 0;
        let (_, next) = h.pump(&mut n, start);
        assert_eq!(n, 0, "nothing due yet");
        assert!(next.is_some());
        h.pump(&mut n, start + Duration::from_millis(6));
        assert_eq!(n, 1);
        h.pump(&mut n, start + Duration::from_millis(12));
        assert_eq!(n, 102, "the repeat and the one-shot");
        h.cancel(tick);
        h.pump(&mut n, start + Duration::from_secs(1));
        assert_eq!(n, 102, "cancelled, and the one-shot is gone");
        assert_eq!(h.pump(&mut n, start + Duration::from_secs(2)).1, None);
    }

    #[test]
    fn updates_can_use_the_handle() {
        let h = AppHandle::<u32>::new();
        let again = h.clone();
        h.update(move |s| {
            *s += 1;
            again.update(|s| *s += 10); // runs on the next pump, no deadlock
        });
        let mut n = 0;
        h.pump(&mut n, crate::platform::time::now());
        assert_eq!(n, 1);
        h.pump(&mut n, crate::platform::time::now());
        assert_eq!(n, 11);
    }

    #[test]
    fn window_requests_go_to_their_window() {
        let h = AppHandle::<u32>::new();
        let other = h.open_window(WindowAttributes::new(), |_, _| crate::widgets::spacer());
        assert_ne!(other, WindowId::PRIMARY);
        h.window(WindowRequest::Title("main".into()));
        h.window_to(other, WindowRequest::Title("other".into()));
        h.window_to(WindowId(99), WindowRequest::Maximized(true));
        assert_eq!(
            h.take_window_requests(other),
            [WindowRequest::Title("other".into())]
        );
        h.drop_window_requests(&[WindowId::PRIMARY]);
        assert_eq!(
            h.take_window_requests(WindowId::PRIMARY),
            [WindowRequest::Title("main".into())]
        );
        let opens = h.take_opens();
        assert_eq!(opens.len(), 1);
        assert_eq!(opens[0].id, other);
        assert!(h.take_window_requests(WindowId(99)).is_empty(), "dropped");
        // Ids keep counting up, from any thread.
        let remote = h.clone();
        let third = std::thread::spawn(move || {
            remote.open_window(WindowAttributes::new(), |_, _| crate::widgets::spacer())
        })
        .join()
        .unwrap();
        assert!(third.0 > other.0);
        h.close_window(third);
        assert_eq!(h.take_closes(), [third]);
    }

    #[test]
    fn window_info_is_kept_per_open_window() {
        let h = AppHandle::<u32>::new();
        assert_eq!(h.window_info(WindowId::PRIMARY), None);
        let info = WindowInfo::default();
        h.set_window_info(WindowId::PRIMARY, Some(info));
        h.set_window_info(WindowId(3), Some(info));
        assert_eq!(h.window_info(WindowId::PRIMARY), Some(info));
        assert_eq!(h.windows(), [WindowId::PRIMARY, WindowId(3)]);
        h.set_window_info(WindowId(3), None);
        assert_eq!(h.windows(), [WindowId::PRIMARY]);
    }

    #[test]
    fn handles_cross_threads() {
        let h = AppHandle::<u32>::new();
        let remote = h.clone();
        std::thread::spawn(move || remote.update(|s| *s = 7))
            .join()
            .unwrap();
        let mut n = 0;
        h.pump(&mut n, crate::platform::time::now());
        assert_eq!(n, 7);
    }

    #[cfg(all(feature = "tokio", not(target_arch = "wasm32")))]
    #[test]
    fn spawn_applies_the_output() {
        let h = AppHandle::<u32>::new();
        let task = h.spawn(async { 21 * 2 }, |s, v| *s = v);
        runtime().block_on(task).unwrap();
        let mut n = 0;
        h.pump(&mut n, crate::platform::time::now());
        assert_eq!(n, 42);
    }
}

/// What an embedded viewport shows: pixels, or an image in another API's
/// memory that a GPU renderer imports without a copy.
#[derive(Clone, Debug)]
pub(crate) enum ViewportContent {
    Pixels(Pixmap),
    External(Arc<ExternalImage>),
}

impl ViewportContent {
    /// The pixels, read from an external image on the CPU if it is one
    /// (`None` if it can't be read here).
    pub(crate) fn pixels(&self) -> Option<std::borrow::Cow<'_, Pixmap>> {
        match self {
            ViewportContent::Pixels(p) => Some(std::borrow::Cow::Borrowed(p)),
            ViewportContent::External(e) => e
                .pixmap()
                .map(|p| std::borrow::Cow::Owned(Arc::unwrap_or_clone(p))),
        }
    }
}
