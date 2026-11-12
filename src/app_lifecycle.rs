//! Window and application lifecycle in the [`App`]: asking before a window
//! closes or the app quits ([`App::on_close_requested`],
//! [`App::on_quit_requested`]) and following which window has keyboard focus
//! ([`App::on_window_focus`], [`Cx::window_focused`](crate::core::Cx::window_focused)),
//! and on mobile going back ([`App::on_back`], [`App::back_enabled`] for
//! predictive back), to the background and back ([`App::on_lifecycle`]),
//! and keeping the state across process death ([`App::save_state`],
//! [`App::restore_state`]). See `docs/windows-and-cursors.md` and
//! `docs/window-lifecycle.md`.

use crate::App;
use crate::core::Handled;
use crate::platform::{BackGesture, Lifecycle, WindowId};

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
type BackFn<S> = Box<dyn FnMut(&mut S) -> Handled>;
type BackEnabledFn<S> = Box<dyn Fn(&S) -> bool>;
type BackProgressFn<S> = Box<dyn FnMut(&mut S, BackGesture)>;
type SaveFn<S> = Box<dyn FnMut(&S) -> Vec<u8>>;
type RestoreFn<S> = Box<dyn FnMut(&mut S, &[u8])>;

/// The app's lifecycle hooks.
pub(crate) struct Hooks<S> {
    close: Option<CloseFn<S>>,
    quit: Option<QuitFn<S>>,
    focus: Option<FocusFn<S>>,
    lifecycle: Option<LifecycleFn<S>>,
    back: Option<BackFn<S>>,
    back_enabled: Option<BackEnabledFn<S>>,
    back_progress: Option<BackProgressFn<S>>,
    save: Option<SaveFn<S>>,
    restore: Option<RestoreFn<S>>,
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
            back: None,
            back_enabled: None,
            back_progress: None,
            save: None,
            restore: None,
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

    /// Go back when the user asks to (Android's Back button or gesture):
    /// `hook` closes whatever is on top (a dialog, a screen) and answers
    /// [`Handled::Yes`], or answers [`Handled::No`] when there is nothing to
    /// go back from, and the system does its default (on Android, the
    /// activity finishes, or its task moves to the back).
    ///
    /// An open menu closes first, without asking the hook. Without a hook,
    /// Back closes menus and otherwise leaves the app. On Android 13 and
    /// later, say beforehand whether the hook would go back with
    /// [`back_enabled`](Self::back_enabled), so the system can preview the
    /// back gesture when it wouldn't. Raw backend users get
    /// [`Event::BackRequested`](crate::platform::Event::BackRequested) and
    /// answer with [`handle_back`](crate::platform::handle_back).
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// struct State { screens: Vec<&'static str> }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { screens: vec!["home"] }, view)
    ///     .on_back(|s| {
    ///         if s.screens.len() > 1 {
    ///             s.screens.pop();
    ///             Handled::Yes
    ///         } else {
    ///             Handled::No
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn on_back(mut self, hook: impl FnMut(&mut S) -> Handled + 'static) -> Self {
        self.lifecycle.back = Some(Box::new(hook));
        self
    }

    /// Say whether the app would go back now, before the user asks: `hook`
    /// answers from the state (a dialog or a screen is open), as
    /// [`on_back`](Self::on_back) would act on it. Android 13 and later need
    /// to know before the back gesture starts (with predictive back, which
    /// the manifest turns on with `android:enableOnBackInvokedCallback`): when
    /// the app wouldn't go back, the system previews its own animation (to the
    /// home screen, or the previous activity) and goes back itself, without
    /// asking the app. When it would, the gesture reaches `on_back`, and on
    /// Android 14 and later its progress reaches
    /// [`on_back_progress`](Self::on_back_progress).
    ///
    /// The hook runs after each event (keep it cheap); an open menu counts as
    /// something to go back from. Without this hook, an app with an `on_back`
    /// hook takes every Back (no system preview), and one without takes none.
    /// Should the hook say yes and `on_back` then answer [`Handled::No`], the
    /// system's default still runs, without its preview. Before Android 13,
    /// and on other platforms, it changes nothing: every Back reaches
    /// `on_back`.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// struct State { screens: Vec<&'static str> }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { screens: vec!["home"] }, view)
    ///     .back_enabled(|s| s.screens.len() > 1)
    ///     .on_back(|s| {
    ///         s.screens.pop();
    ///         Handled::Yes
    ///     })
    ///     .run();
    /// ```
    pub fn back_enabled(mut self, hook: impl Fn(&S) -> bool + 'static) -> Self {
        self.lifecycle.back_enabled = Some(Box::new(hook));
        self
    }

    /// Hear the back gesture's progress while the app takes Back (see
    /// [`back_enabled`](Self::back_enabled)), to animate what going back will
    /// show: [`BackGesture::Started`], then [`BackGesture::Progressed`] from 0
    /// to 1 as the finger moves, and either [`BackGesture::Cancelled`] or the
    /// [`on_back`](Self::on_back) hook when it lifts. Android 14 and later
    /// (`OnBackAnimationCallback`); elsewhere Back arrives without progress.
    /// The hook may change the state; the view is rebuilt.
    pub fn on_back_progress(mut self, hook: impl FnMut(&mut S, BackGesture) + 'static) -> Self {
        self.lifecycle.back_progress = Some(Box::new(hook));
        self
    }

    /// Whether the app would go back if the user asked now: a menu is open,
    /// or the [`back_enabled`](Self::back_enabled) hook says so (without one,
    /// whether there is an [`on_back`](Self::on_back) hook). What the system
    /// is told before a back gesture.
    pub fn wants_back(&self) -> bool {
        if self.panes.iter().any(|p| p.menu_ui.borrow().is_active()) {
            return true;
        }
        match &self.lifecycle.back_enabled {
            Some(hook) => hook(&self.state),
            None => self.lifecycle.back.is_some(),
        }
    }

    /// Tell the platform whether the app would go back (see
    /// [`wants_back`](Self::wants_back)); only changes reach the system.
    pub(crate) fn sync_back(&self) {
        crate::platform::set_back_enabled(self.wants_back());
    }

    /// The back gesture moved on: tell the hook. Returns whether it ran (the
    /// view then rebuilds).
    pub(crate) fn back_progress(&mut self, gesture: BackGesture) -> bool {
        let Some(hook) = self.lifecycle.back_progress.as_mut() else {
            return false;
        };
        hook(&mut self.state, gesture);
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        true
    }

    /// Keep the state across process death: when the system may end the
    /// app in the background (Android's `onSaveInstanceState`), `hook` turns
    /// what the user would expect to find again (the screen they were on, a
    /// draft, a scroll position) into bytes, in whatever format the app likes;
    /// when the system starts the app again from them,
    /// [`restore_state`](Self::restore_state) gets them back, before the first
    /// frame. Return an empty vector to save nothing.
    ///
    /// This is for the session's state, not the user's data, which belongs
    /// in a file saved as it changes (or on [`Lifecycle::Background`]): the
    /// system only restores the state when it ended the app itself, not when
    /// the user closed it. Keep it small, a few kilobytes: Android carries it
    /// in a binder transaction, which fails past a megabyte for the whole
    /// process.
    ///
    /// Android keeps the bytes in the activity's saved `Bundle` (a
    /// `NativeActivity`'s `onSaveInstanceState`). Other platforms don't ask
    /// yet: iOS could keep them in its state restoration archive
    /// (`application:willEncodeRestorableStateWithCoder:`), and the web in
    /// the tab's `sessionStorage`, which a reload or a discarded and restored
    /// tab keeps; desktops don't end apps on their own.
    ///
    /// ```no_run
    /// # use stipple::prelude::*;
    /// struct State { screen: u8, draft: String }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { screen: 0, draft: String::new() }, view)
    ///     .save_state(|s| {
    ///         let mut bytes = vec![s.screen];
    ///         bytes.extend_from_slice(s.draft.as_bytes());
    ///         bytes
    ///     })
    ///     .restore_state(|s, bytes| {
    ///         if let Some((&screen, draft)) = bytes.split_first() {
    ///             s.screen = screen;
    ///             s.draft = String::from_utf8_lossy(draft).into_owned();
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn save_state(mut self, hook: impl FnMut(&S) -> Vec<u8> + 'static) -> Self {
        self.lifecycle.save = Some(Box::new(hook));
        self
    }

    /// Take back the state [`save_state`](Self::save_state) saved, when the
    /// system starts the app again after ending it: `hook` gets the bytes and
    /// puts them back into the state, before the first frame. It isn't
    /// called on a fresh start. The bytes come from an earlier run, possibly
    /// of an older version of the app: check them, and ignore what doesn't
    /// parse.
    pub fn restore_state(mut self, hook: impl FnMut(&mut S, &[u8]) + 'static) -> Self {
        self.lifecycle.restore = Some(Box::new(hook));
        self
    }

    /// What [`save_state`](Self::save_state) would save now, as the system
    /// asks for it (for tests, too): `None` without a hook, or when it saves
    /// nothing.
    pub fn saved_state(&mut self) -> Option<Vec<u8>> {
        let hook = self.lifecycle.save.as_mut()?;
        let bytes = hook(&self.state);
        (!bytes.is_empty()).then_some(bytes)
    }

    /// Hand `bytes` to the [`restore_state`](Self::restore_state) hook, as
    /// a start from saved state would (for tests, too). Returns whether there
    /// was a hook; every window rebuilds.
    pub fn restore_saved_state(&mut self, bytes: &[u8]) -> bool {
        let Some(hook) = self.lifecycle.restore.as_mut() else {
            return false;
        };
        hook(&mut self.state, bytes);
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        true
    }

    /// At start-up: restore the state the platform was started with, if any.
    pub(crate) fn restore_from_platform(&mut self) {
        if let Some(bytes) = crate::platform::take_restored_state() {
            self.restore_saved_state(&bytes);
        }
    }

    /// The user asked to go back from pane `idx`: close an open menu, or ask
    /// the back hook. Returns whether the app went back.
    pub(crate) fn go_back(&mut self, idx: usize) -> bool {
        use crate::widgets::menus::MenuKey;
        if idx < self.panes.len() && self.menu_nav(idx, Some(MenuKey::Escape)) == Some(true) {
            return true;
        }
        let Some(hook) = self.lifecycle.back.as_mut() else {
            return false;
        };
        let handled = hook(&mut self.state) == Handled::Yes;
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        handled
    }

    /// Ask the app to go back, as Android's Back button would, without a
    /// running backend (for tests). Returns whether it went back.
    pub fn request_back(&mut self) -> bool {
        self.go_back(0)
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
    use crate::platform::BackGesture;
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
    fn back_asks_the_hook_until_there_is_nothing_to_close() {
        let mut app = App::new(State::default(), view);
        assert!(!app.request_back(), "without a hook, back leaves the app");

        let screens = Rc::new(Cell::new(2));
        let left = screens.clone();
        let mut app = App::new(State::default(), view).on_back(move |_| {
            if left.get() == 0 {
                return crate::core::Handled::No;
            }
            left.set(left.get() - 1);
            crate::core::Handled::Yes
        });
        app.render_frame();
        assert!(app.request_back());
        assert!(app.panes[0].dirty, "going back rebuilds the view");
        assert!(app.request_back());
        assert!(!app.request_back(), "nothing left: the system goes back");
        assert_eq!(screens.get(), 0);
    }

    #[test]
    fn the_system_learns_beforehand_whether_back_reaches_the_app() {
        // No hooks: Back is the system's.
        let app = App::new(State::default(), view);
        assert!(!app.wants_back());
        // An `on_back` hook alone takes every Back, as before predictive back.
        let app = App::new(State::default(), view).on_back(|_| Handled::No);
        assert!(app.wants_back());
        // `back_enabled` answers from the state.
        let mut app = App::new(State::default(), view)
            .back_enabled(|s| s.busy)
            .on_back(|s| {
                s.busy = false;
                Handled::Yes
            });
        app.render_frame();
        assert!(!app.wants_back());
        app.state.busy = true;
        assert!(app.wants_back());
        assert!(app.request_back());
        assert!(!app.wants_back(), "gone back: nothing left to go back from");
        // An open menu counts, and Back closes it without asking the hook.
        app.panes[0].menu_ui.borrow_mut().open_context(
            crate::core::ContextId(1),
            crate::geometry::Point::new(0.0, 0.0),
            false,
        );
        assert!(app.wants_back());
        assert!(app.request_back());
        assert!(!app.wants_back());
        assert!(!app.state().busy, "the hook didn't run for the menu");
    }

    #[test]
    fn the_back_gesture_reaches_its_hook() {
        let gestures = Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = gestures.clone();
        let mut app = App::new(State::default(), view);
        assert!(!app.back_progress(BackGesture::Cancelled), "no hook");
        let mut app = App::new(State::default(), view)
            .on_back_progress(move |_, g| seen.borrow_mut().push(g));
        app.render_frame();
        assert!(app.back_progress(BackGesture::Started {
            edge: crate::platform::BackEdge::Left
        }));
        assert!(app.back_progress(BackGesture::Progressed { progress: 0.5 }));
        assert!(app.panes[0].dirty, "the view follows the gesture");
        assert!(app.back_progress(BackGesture::Cancelled));
        assert_eq!(gestures.borrow().len(), 3);
    }

    #[test]
    fn state_saved_comes_back_after_process_death() {
        let mut app = App::new(State::default(), view);
        assert_eq!(app.saved_state(), None, "no hook, nothing saved");
        assert!(!app.restore_saved_state(b"x"));

        let save = |s: &State| s.quits.to_string().into_bytes();
        let restore = |s: &mut State, bytes: &[u8]| {
            if let Some(n) = std::str::from_utf8(bytes).ok().and_then(|t| t.parse().ok()) {
                s.quits = n;
            }
        };
        let mut before = App::new(State::default(), view)
            .save_state(save)
            .restore_state(restore);
        before.state.quits = 42;
        let bytes = before.saved_state().expect("saved");
        assert_eq!(bytes, b"42");

        // The process ends; a new one starts from the bytes.
        let mut after = App::new(State::default(), view)
            .save_state(save)
            .restore_state(restore);
        after.render_frame();
        assert!(after.restore_saved_state(&bytes));
        assert_eq!(after.state().quits, 42);
        assert!(after.panes[0].dirty, "every window rebuilds");
        // Bytes that don't parse are the hook's to ignore.
        assert!(after.restore_saved_state(b"garbage"));
        assert_eq!(after.state().quits, 42);

        // An empty save is no save.
        let mut empty = App::new(State::default(), view).save_state(|_| Vec::new());
        assert_eq!(empty.saved_state(), None);
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
