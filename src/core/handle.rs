//! [`AppHandle`]: changing app state from timers and other threads.

use crate::platform::time::Instant;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::core::canvas::{CanvasId, CanvasInvalidation};
use crate::core::runtime::ViewportId;
use crate::core::{Cx, Element};
use crate::geometry::Rect;
use crate::platform::wake;
use crate::platform::{UserAttention, WindowId, WindowRequest};
use crate::platform::{WindowAttributes, WindowInfo};
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
            let mut p = PasswordField::new().with_reveal_button(true);
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
/// [`ViewFn`](crate::ViewFn), but able to cross threads.
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
    viewports: Vec<(ViewportId, Option<Pixmap>)>,
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
///     handle.update(move |s: &mut State| s.result = Some(result));
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
                due: Instant::now() + delay,
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
    /// button_labeled(cx.theme(), "Fullscreen")
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
    /// let id = handle.open_window(WindowAttributes::new().with_title("Untitled"), editor);
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
    /// button, bounce the Dock icon, mark the window urgent. See
    /// [`Window::request_attention`](crate::platform::Window::request_attention).
    pub fn request_attention(&self, kind: UserAttention) {
        self.window(WindowRequest::Attention(Some(kind)));
    }

    /// Sound the system beep, as a terminal's audible bell. See
    /// [`Window::beep`](crate::platform::Window::beep).
    pub fn beep(&self) {
        self.window(WindowRequest::Beep);
    }

    /// Show `label` as a badge on the application's Dock icon (macOS), or
    /// remove it with `None`. See
    /// [`Window::set_badge`](crate::platform::Window::set_badge).
    pub fn set_badge(&self, label: Option<&str>) {
        self.window(WindowRequest::Badge(label.map(str::to_string)));
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
        self.with(|s| s.canvases.push((id, None)));
        wake::wake();
    }

    /// Repaint the part `rect` (in the canvas's own coordinates) of
    /// [canvas](crate::core::Element::canvas) `id`, without rebuilding the
    /// view: only that area of the window is re-rasterized and presented, and
    /// the canvas's paint callback is asked for just that area. A terminal
    /// passes the rows that changed. Safe from any thread.
    pub fn invalidate_canvas_rect(&self, id: CanvasId, rect: Rect) {
        self.with(|s| s.canvases.push((id, Some(rect))));
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
        self.with(|s| s.viewports.push((id, Some(content))));
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
    pub(crate) fn take_viewport_content(&self) -> Vec<(ViewportId, Option<Pixmap>)> {
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
                crate::platform::dialog::message(&mb, done).is_ok()
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

    /// The drawn message box up now (the first asked), to draw.
    pub(crate) fn alert_view(&self) -> Option<AlertView> {
        self.with(|s| {
            s.drawn.first().map(|d| AlertView {
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
            }
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_run_in_order_on_pump() {
        let h = AppHandle::<Vec<u32>>::new();
        h.update(|s| s.push(1));
        h.update(|s| s.push(2));
        let mut state = Vec::new();
        let (ran, next) = h.pump(&mut state, Instant::now());
        assert!(ran);
        assert_eq!(state, [1, 2]);
        assert_eq!(next, None);
        assert!(!h.pump(&mut state, Instant::now()).0, "updates run once");
    }

    #[test]
    fn timers_fire_when_due_and_repeat() {
        let h = AppHandle::<u32>::new();
        let start = Instant::now();
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
        h.pump(&mut n, Instant::now());
        assert_eq!(n, 1);
        h.pump(&mut n, Instant::now());
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
        h.pump(&mut n, Instant::now());
        assert_eq!(n, 7);
    }

    #[cfg(all(feature = "tokio", not(target_arch = "wasm32")))]
    #[test]
    fn spawn_applies_the_output() {
        let h = AppHandle::<u32>::new();
        let task = h.spawn(async { 21 * 2 }, |s, v| *s = v);
        runtime().block_on(task).unwrap();
        let mut n = 0;
        h.pump(&mut n, Instant::now());
        assert_eq!(n, 42);
    }
}
