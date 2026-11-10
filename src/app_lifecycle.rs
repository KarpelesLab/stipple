//! Window and application lifecycle in the [`App`]: asking before a window
//! closes or the app quits ([`App::on_close_requested`],
//! [`App::on_quit_requested`]) and following which window has keyboard focus
//! ([`App::on_window_focus`], [`Cx::window_focused`](crate::core::Cx::window_focused)),
//! and a mobile app going to the background and coming back
//! ([`App::on_lifecycle`]). See `docs/windows-and-cursors.md` and
//! `docs/window-lifecycle.md`.

use crate::App;
use crate::platform::{Lifecycle, WindowId};

/// The answer to a close or quit request (see [`App::on_close_requested`] and
/// [`App::on_quit_requested`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CloseResponse {
    /// Go ahead: close the window, or quit.
    #[default]
    Close,
    /// Keep the window open (or the app running). To close it later, once
    /// the user confirmed or a background check came back, call
    /// [`AppHandle::close_window`](crate::core::AppHandle::close_window) (or
    /// [`AppHandle::exit`](crate::core::AppHandle::exit) to quit).
    Cancel,
}

type CloseFn<S> = Box<dyn FnMut(&mut S, WindowId) -> CloseResponse>;
type QuitFn<S> = Box<dyn FnMut(&mut S) -> CloseResponse>;
type FocusFn<S> = Box<dyn FnMut(&mut S, WindowId, bool)>;
type LifecycleFn<S> = Box<dyn FnMut(&mut S, Lifecycle)>;

/// The app's lifecycle hooks.
pub(crate) struct Hooks<S> {
    close: Option<CloseFn<S>>,
    quit: Option<QuitFn<S>>,
    focus: Option<FocusFn<S>>,
    lifecycle: Option<LifecycleFn<S>>,
    /// Whether the app is in the background (between
    /// [`Lifecycle::Background`] and [`Lifecycle::Foreground`]).
    pub(crate) background: bool,
}

impl<S> Default for Hooks<S> {
    fn default() -> Self {
        Self {
            close: None,
            quit: None,
            focus: None,
            lifecycle: None,
            background: false,
        }
    }
}

impl<S> Hooks<S> {
    /// Whether the app answers quit requests itself: with a quit hook, or
    /// with a close hook, which a quit asks for each window.
    fn confirms_quit(&self) -> bool {
        self.close.is_some() || self.quit.is_some()
    }
}

impl<S: 'static> App<S> {
    /// Decide whether a window the user asked to close (its close button,
    /// Alt+F4, ⌘W, the window manager) closes. `hook` gets the state and the
    /// window's id and answers [`CloseResponse::Close`] or
    /// [`CloseResponse::Cancel`]; it may change the state (to show a "close
    /// anyway?" dialog, say), and the window re-renders.
    ///
    /// Answer `Cancel` to ask first, then close the window yourself with
    /// [`AppHandle::close_window`](crate::core::AppHandle::close_window),
    /// which doesn't ask again. Closing the last window ends the app.
    ///
    /// Without an [`on_quit_requested`](Self::on_quit_requested) hook, quitting
    /// the app (⌘Q, logging out) asks this hook for each open window and
    /// quits only if all of them close.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # use stipple::CloseResponse;
    /// struct State { busy: bool, confirm: Option<stipple::platform::WindowId> }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { busy: true, confirm: None }, view)
    ///     .on_close_requested(|s, id| {
    ///         if s.busy {
    ///             s.confirm = Some(id); // the view shows "Close anyway?"
    ///             CloseResponse::Cancel
    ///         } else {
    ///             CloseResponse::Close
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn on_close_requested(
        mut self,
        hook: impl FnMut(&mut S, WindowId) -> CloseResponse + 'static,
    ) -> Self {
        self.lifecycle.close = Some(Box::new(hook));
        crate::platform::set_app_confirms_quit(self.lifecycle.confirms_quit());
        self
    }

    /// Decide whether the app quits when the user asks it to as a whole: ⌘Q,
    /// the Dock's Quit or logging out on macOS, ending the session on
    /// Windows. Answer [`CloseResponse::Cancel`] to keep running (and, to
    /// quit later, call [`AppHandle::exit`](crate::core::AppHandle::exit)).
    ///
    /// On macOS a cancelled logout stays cancelled: the user logs out again
    /// once the app has quit. Closing the last window is a
    /// [close request](Self::on_close_requested), not a quit.
    pub fn on_quit_requested(
        mut self,
        hook: impl FnMut(&mut S) -> CloseResponse + 'static,
    ) -> Self {
        self.lifecycle.quit = Some(Box::new(hook));
        crate::platform::set_app_confirms_quit(self.lifecycle.confirms_quit());
        self
    }

    /// Hear when a window gains (`true`) or loses (`false`) keyboard focus,
    /// with its id: to send a terminal's focus reports, or clear a bell
    /// once the user looks. The view reads the same through
    /// [`Cx::window_focused`](crate::core::Cx::window_focused), and is
    /// rebuilt when it changes.
    ///
    /// Reported on X11 (`FocusIn`/`FocusOut`), Wayland (the keyboard's
    /// `enter`/`leave`), Windows (`WM_SETFOCUS`/`WM_KILLFOCUS`), macOS (the
    /// window becoming or resigning key), Android and the web.
    pub fn on_window_focus(mut self, hook: impl FnMut(&mut S, WindowId, bool) + 'static) -> Self {
        self.lifecycle.focus = Some(Box::new(hook));
        self
    }

    /// Hear when the app goes to the background, comes back, or the system
    /// runs short of memory ([`Lifecycle`]), on mobile platforms: iOS
    /// (`UIApplicationDidEnterBackgroundNotification`,
    /// `WillEnterForeground`, `DidReceiveMemoryWarning`). Desktops and the
    /// web don't send them.
    ///
    /// Save what must survive on [`Lifecycle::Background`]: the system may end
    /// a backgrounded app without warning. While in the background the app
    /// asks for no animation frames and doesn't repaint; timers and
    /// [`AppHandle`](crate::core::AppHandle) updates still run while the OS
    /// lets the process run. On coming back every window rebuilds and
    /// repaints. The hook may change the state; the view is rebuilt.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// # use stipple::platform::Lifecycle;
    /// struct State { draft: String, cache: Vec<u8> }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { draft: String::new(), cache: Vec::new() }, view)
    ///     .on_lifecycle(|s, change| match change {
    ///         Lifecycle::Background => {
    ///             let _ = std::fs::write("draft.txt", &s.draft);
    ///         }
    ///         Lifecycle::LowMemory => s.cache.clear(),
    ///         _ => {}
    ///     })
    ///     .run();
    /// ```
    pub fn on_lifecycle(mut self, hook: impl FnMut(&mut S, Lifecycle) + 'static) -> Self {
        self.lifecycle.lifecycle = Some(Box::new(hook));
        self
    }

    /// The app went to the background, came back or is short of memory:
    /// stop or restart frames, and tell the hook. Returns whether the app
    /// should repaint now (it came back, or the hook ran while on screen).
    pub(crate) fn lifecycle_changed(&mut self, change: Lifecycle) -> bool {
        let was = self.lifecycle.background;
        match change {
            Lifecycle::Background => self.lifecycle.background = true,
            Lifecycle::Foreground => self.lifecycle.background = false,
            _ => {}
        }
        let background = self.lifecycle.background;
        if background != was {
            crate::platform::frame::set_suspended(background);
        }
        // A memory warning drops the bitmaps the OS drew, which are redrawn
        // on demand.
        if change == Lifecycle::LowMemory {
            crate::platform::native::clear_cache();
        }
        let ran = match self.lifecycle.lifecycle.as_mut() {
            Some(hook) => {
                hook(&mut self.state, change);
                true
            }
            None => false,
        };
        // Back on screen, everything is shown afresh.
        if ran || (was && !background) {
            for pane in &mut self.panes {
                pane.dirty = true;
            }
        }
        !background && (ran || was)
    }

    /// Whether the app is in the background (see [`on_lifecycle`](Self::on_lifecycle)).
    pub(crate) fn in_background(&self) -> bool {
        self.lifecycle.background
    }

    /// Ask the close hook whether window `id` closes. A hook
    /// that ran may have changed the state: every pane rebuilds.
    pub(crate) fn ask_close(&mut self, id: WindowId) -> bool {
        let Some(hook) = self.lifecycle.close.as_mut() else {
            return true;
        };
        let answer = hook(&mut self.state, id);
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        answer == CloseResponse::Close
    }

    /// Ask whether the app quits: the quit hook, or else the close hook for
    /// each open window (`open`).
    pub(crate) fn ask_quit(&mut self, open: &[WindowId]) -> bool {
        if let Some(hook) = self.lifecycle.quit.as_mut() {
            let answer = hook(&mut self.state);
            for pane in &mut self.panes {
                pane.dirty = true;
            }
            return answer == CloseResponse::Close;
        }
        // Every window is asked, so each can show what it has to.
        let mut quit = true;
        for &id in open {
            quit &= self.ask_close(id);
        }
        quit
    }

    /// Window `id` (pane `idx`) gained or lost keyboard focus. Returns
    /// whether that changed anything (the pane then rebuilds).
    pub(crate) fn window_focus_changed(&mut self, idx: usize, id: WindowId, focused: bool) -> bool {
        let Some(pane) = self.panes.get_mut(idx) else {
            return false;
        };
        if pane.window_focused == focused {
            return false;
        }
        pane.window_focused = focused;
        pane.dirty = true;
        if let Some(hook) = self.lifecycle.focus.as_mut() {
            hook(&mut self.state, id, focused);
            for pane in &mut self.panes {
                pane.dirty = true;
            }
        }
        true
    }

    /// Ask to close the primary window, as its close button would, without a
    /// running backend (for tests). Returns whether it would close.
    pub fn request_close(&mut self) -> bool {
        let id = self.panes[0].id;
        self.ask_close(id)
    }

    /// Ask the app to quit, as ⌘Q would, without a running backend (for
    /// tests). Returns whether it would quit.
    pub fn request_quit(&mut self) -> bool {
        let open: Vec<WindowId> = self.panes.iter().map(|p| p.id).collect();
        self.ask_quit(&open)
    }

    /// Give the primary window keyboard focus, or take it away, as the
    /// platform would (for tests). Returns whether that changed anything.
    pub fn set_window_focused(&mut self, focused: bool) -> bool {
        let id = self.panes[0].id;
        self.window_focus_changed(0, id, focused)
    }

    /// Whether the primary window has keyboard focus, as far as the app
    /// knows (see [`Cx::window_focused`](crate::core::Cx::window_focused)).
    pub fn window_focused(&self) -> bool {
        self.panes[0].window_focused
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Cx, Element};
    use crate::widgets::spacer;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Default)]
    struct State {
        busy: bool,
        asked: Vec<WindowId>,
        quits: u32,
        focus: Vec<bool>,
        changes: Vec<Lifecycle>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> Element {
        spacer()
    }

    #[test]
    fn without_hooks_everything_closes() {
        let mut app = App::new(State::default(), view);
        assert!(app.request_close());
        assert!(app.request_quit());
    }

    #[test]
    fn the_close_hook_can_veto_and_change_the_state() {
        let mut app = App::new(
            State {
                busy: true,
                ..Default::default()
            },
            view,
        )
        .on_close_requested(|s, id| {
            s.asked.push(id);
            if s.busy {
                CloseResponse::Cancel
            } else {
                CloseResponse::Close
            }
        });
        app.render_frame();
        assert!(!app.request_close(), "busy: cancelled");
        assert_eq!(app.state().asked.len(), 1);
        assert!(app.panes[0].dirty, "the hook's change rebuilds the view");
        app.state.busy = false;
        assert!(app.request_close());
    }

    #[test]
    fn quitting_asks_the_quit_hook_or_else_every_window() {
        // A quit hook decides alone.
        let mut app = App::new(State::default(), view).on_quit_requested(|s| {
            s.quits += 1;
            if s.quits > 1 {
                CloseResponse::Close
            } else {
                CloseResponse::Cancel
            }
        });
        assert!(!app.request_quit());
        assert!(app.request_quit());
        assert_eq!(app.state().quits, 2);

        // Without one, each window's close hook is asked; one veto keeps the
        // app running.
        let mut app = App::new(
            State {
                busy: true,
                ..Default::default()
            },
            view,
        )
        .on_close_requested(|s, id| {
            s.asked.push(id);
            if s.busy && s.asked.len() == 2 {
                CloseResponse::Cancel
            } else {
                CloseResponse::Close
            }
        });
        app.open_window(Default::default(), view);
        assert!(!app.request_quit());
        assert_eq!(app.state().asked.len(), 2, "both windows were asked");
    }

    #[test]
    fn focus_changes_reach_the_hook_and_the_view() {
        let seen = Rc::new(Cell::new(None));
        let record = seen.clone();
        let mut app = App::new(State::default(), move |_: &State, cx: &mut Cx<State>| {
            record.set(Some(cx.window_focused()));
            spacer()
        })
        .on_window_focus(|s, _, focused| s.focus.push(focused));
        app.render_frame();
        assert_eq!(seen.get(), Some(true), "focused until told otherwise");
        assert!(app.window_focused());

        assert!(app.set_window_focused(false));
        assert!(!app.set_window_focused(false), "no change, no call");
        app.render_frame();
        assert_eq!(seen.get(), Some(false));
        assert!(app.set_window_focused(true));
        assert_eq!(app.state().focus, [false, true]);
    }

    #[test]
    fn lifecycle_changes_reach_the_hook_and_stop_frames() {
        let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut app =
            App::new(State::default(), view).on_lifecycle(|s, change| s.changes.push(change));
        app.render_frame();
        assert!(!app.in_background());
        assert!(
            !app.lifecycle_changed(Lifecycle::Background),
            "nothing to show"
        );
        assert!(app.in_background());
        assert!(crate::platform::frame::suspended(), "frames stop");
        crate::platform::frame::request();
        assert!(!crate::platform::frame::wanted(), "no frames while away");
        assert!(!app.lifecycle_changed(Lifecycle::LowMemory));
        assert!(
            app.lifecycle_changed(Lifecycle::Foreground),
            "back: repaint"
        );
        assert!(!crate::platform::frame::suspended());
        assert!(app.panes[0].dirty, "every window rebuilds");
        assert_eq!(
            app.state().changes,
            [
                Lifecycle::Background,
                Lifecycle::LowMemory,
                Lifecycle::Foreground
            ]
        );
        // Without a hook, coming back still repaints, and a warning doesn't.
        let mut app = App::new(State::default(), view);
        app.render_frame();
        assert!(!app.lifecycle_changed(Lifecycle::LowMemory));
        assert!(!app.panes[0].dirty);
        app.lifecycle_changed(Lifecycle::Background);
        assert!(app.lifecycle_changed(Lifecycle::Foreground));
    }
}
