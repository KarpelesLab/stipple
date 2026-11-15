//! Back declared by the view: a widget that has something to go back from (a
//! page stack deeper than its root, an open sheet) says so while it's built,
//! and the [`App`](crate::App) routes Back, the back gesture's progress and,
//! when asked, an unused Escape to it before its own
//! [`on_back`](crate::App::on_back) hook. See `docs/window-lifecycle.md`.

use super::Cx;
use crate::platform::BackGesture;

type OnBack<S> = Box<dyn FnMut(&mut S)>;
type OnProgress<S> = Box<dyn FnMut(&mut S, BackGesture)>;

/// What a view does when the user goes back while it's on screen: see
/// [`Cx::on_back`].
pub struct BackHandler<S> {
    on_back: OnBack<S>,
    on_progress: Option<OnProgress<S>>,
    escape: bool,
}

impl<S> std::fmt::Debug for BackHandler<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackHandler")
            .field("progress", &self.on_progress.is_some())
            .field("escape", &self.escape)
            .finish_non_exhaustive()
    }
}

impl<S> BackHandler<S> {
    /// Go back with `on_back`: close what's on top (pop a page, dismiss a
    /// sheet).
    pub fn new(on_back: impl FnMut(&mut S) + 'static) -> Self {
        Self {
            on_back: Box::new(on_back),
            on_progress: None,
            escape: false,
        }
    }

    /// Hear the back gesture's progress (Android 14's predictive back, iOS's
    /// edge swipe) to preview going back: [`BackGesture::Started`], then
    /// [`BackGesture::Progressed`], then either [`BackGesture::Cancelled`] or
    /// `on_back` when the finger lifts.
    pub fn on_progress(mut self, on_progress: impl FnMut(&mut S, BackGesture) + 'static) -> Self {
        self.on_progress = Some(Box::new(on_progress));
        self
    }

    /// Go back on an Escape that nothing focused used, too (a sheet or a
    /// dialog closing). Off by default: a page stack doesn't pop on Escape.
    pub fn escape(mut self, escape: bool) -> Self {
        self.escape = escape;
        self
    }

    /// Run `on_back`.
    pub(crate) fn back(&mut self, state: &mut S) {
        (self.on_back)(state);
    }

    /// Hand `gesture` on, if this handler hears progress. Returns whether it
    /// did.
    pub(crate) fn progress(&mut self, state: &mut S, gesture: BackGesture) -> bool {
        match self.on_progress.as_mut() {
            Some(f) => {
                f(state, gesture);
                true
            }
            None => false,
        }
    }

    /// Whether an unused Escape goes back through it.
    pub(crate) fn takes_escape(&self) -> bool {
        self.escape
    }
}

impl<S> Cx<'_, S> {
    /// Take Back while this build is on screen: Android's Back button and
    /// predictive back gesture, iOS's swipe from the leading edge, a
    /// gamepad's B, and (with [`BackHandler::escape`]) an Escape nothing
    /// focused used. The App tells the system it would go back (as
    /// [`App::back_enabled`](crate::App::back_enabled) does) while any
    /// build declares one, and the last one declared in the build gets it:
    /// declare in the order things stack, so a sheet opened over a page
    /// stack closes before the stack pops. An open menu still closes first,
    /// and the App's own [`on_back`](crate::App::on_back) hook runs only
    /// when no view declared one. [`nav_stack`](crate::widgets::nav_stack)
    /// and [`bottom_sheet`](crate::widgets::bottom_sheet) declare theirs.
    ///
    /// Not inside a [`memo`](Cx::memo)ized subtree: a reused one isn't built,
    /// so it declares nothing.
    pub fn on_back(&mut self, handler: BackHandler<S>) {
        self.backs.push(handler);
    }

    /// Take the back handlers this build declared, last on top.
    pub(crate) fn take_back_handlers(&mut self) -> Vec<BackHandler<S>> {
        std::mem::take(&mut self.backs)
    }
}
