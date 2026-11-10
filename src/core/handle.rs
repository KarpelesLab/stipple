//! [`AppHandle`]: changing app state from timers and other threads.

use crate::platform::time::Instant;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::platform::wake;
use crate::platform::{UserAttention, WindowId, WindowRequest};

type Update<S> = Box<dyn FnOnce(&mut S) + Send>;
type Tick<S> = Box<dyn FnMut(&mut S) + Send>;

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
    /// Window changes queued by [`AppHandle::window`].
    window: Vec<WindowRequest>,
    /// Windows to close without asking, queued by [`AppHandle::close_window`].
    close: Vec<WindowId>,
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
                close: Vec::new(),
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

    /// Change the primary window while the app runs: maximize it, make it
    /// fullscreen, change its size limits or icon. Applied on the UI thread
    /// between events; a no-op on backends without that control.
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
        self.with(|s| s.window.push(request));
        wake::wake();
    }

    /// Close window `id` without asking
    /// [`App::on_close_requested`](crate::App::on_close_requested): what an
    /// app calls once the user confirmed a close it cancelled earlier.
    /// Closing the last window ends the app, as when the user closes it.
    /// The id is the one the close hook was given (or
    /// [`Cx::window_id`](crate::core::Cx::window_id)).
    pub fn close_window(&self, id: WindowId) {
        self.with(|s| s.close.push(id));
        wake::wake();
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

    /// Take the windows queued by [`close_window`](Self::close_window).
    pub(crate) fn take_close_requests(&self) -> Vec<WindowId> {
        self.with(|s| std::mem::take(&mut s.close))
    }

    /// Take the window changes queued by [`window`](Self::window).
    pub(crate) fn take_window_requests(&self) -> Vec<WindowRequest> {
        self.with(|s| std::mem::take(&mut s.window))
    }

    /// Apply queued updates and due timers to `state`, as of `now`. Returns
    /// whether anything ran, and the next timer's deadline.
    pub(crate) fn pump(&self, state: &mut S, now: Instant) -> (bool, Option<Instant>) {
        let mut ran = false;
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
