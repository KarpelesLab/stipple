//! Moving keyboard focus from the view and from handlers: an element that
//! takes focus when it first shows ([`Element::autofocus`]), focus by an
//! element's key ([`Cx::focus`], [`AppHandle::focus`]), and the action Enter
//! runs in a dialog (see [`Dialog::default_action`](crate::widgets::Dialog::default_action)).
//! See `docs/keyboard.md`.

use super::Cx;
use crate::core::{AppHandle, Element, ElementKey, FocusId};
use crate::platform::WindowId;

/// A focus change asked for by key, applied after the next build's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusRequest {
    /// Focus the element keyed so (or the first focusable inside it).
    Key(ElementKey),
    /// Focus nothing.
    Blur,
}

/// A handler more than one thing runs (a dialog's button and Enter).
pub(crate) type SharedFn<S> = std::rc::Rc<std::cell::RefCell<dyn FnMut(&mut S)>>;

/// What Enter runs in a dialog while focus is in it (or nowhere): see
/// [`Cx::default_action`].
pub(crate) struct DefaultAction<S> {
    /// The element (the dialog's panel) focus must be inside of.
    pub(crate) scope: ElementKey,
    pub(crate) run: SharedFn<S>,
}

impl Element {
    /// Take keyboard focus the first time this element is laid out: a
    /// search field in a popup that opens, the name field of a dialog. It
    /// focuses the element itself if it takes focus, else the first
    /// focusable inside it. Once it has shown, it doesn't take focus again
    /// until it leaves and comes back; when it leaves while focus is in it,
    /// focus goes back to where it was before.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Find { open: bool, query: TextInput }
    /// fn view(s: &Find, cx: &mut Cx<Find>) -> Element {
    ///     if s.open {
    ///         let field = text_field(cx, &s.query, |s| &mut s.query).autofocus();
    ///         cx.overlay(OverlaySpec::new(field, Anchor::Center));
    ///     }
    ///     button(cx, "Find…").on_tap(cx, |s| s.open = true)
    /// }
    /// # let theme = Theme::light();
    /// # let _ = view(&Find { open: true, query: TextInput::new() }, &mut Cx::new(&theme));
    /// ```
    pub fn autofocus(mut self) -> Self {
        // An identity, so it is known again on the next build.
        self.reactive_mut();
        self.keyboard.get_or_insert_with(Default::default).autofocus = true;
        self
    }
}

impl<S> Cx<'_, S> {
    /// Move keyboard focus to the element keyed `key` (see
    /// [`Element::key`]), or to the first focusable inside it, once this
    /// build is laid out. Call it when something opens rather than on every
    /// build, or focus can't leave; a handler uses
    /// [`AppHandle::focus`] instead. Nothing happens if no element has that
    /// key.
    pub fn focus(&mut self, key: impl std::hash::Hash) {
        self.focus_requests
            .push(FocusRequest::Key(ElementKey::new(key)));
    }

    /// Take keyboard focus away from whatever has it, once this build is
    /// laid out.
    pub fn blur(&mut self) {
        self.focus_requests.push(FocusRequest::Blur);
    }

    /// Take the focus changes this build asked for.
    pub(crate) fn take_focus_requests(&mut self) -> Vec<FocusRequest> {
        std::mem::take(&mut self.focus_requests)
    }

    /// Run `run` on an Enter that focus inside the element keyed `scope`
    /// (or nowhere) leaves alone: not one a button, a multi-line field or a
    /// field with its own `on_submit` takes. The last declared wins.
    pub(crate) fn default_action(&mut self, scope: ElementKey, run: SharedFn<S>) {
        self.defaults.push(DefaultAction { scope, run });
    }

    /// Take the default actions this build declared, last on top.
    pub(crate) fn take_default_actions(&mut self) -> Vec<DefaultAction<S>> {
        std::mem::take(&mut self.defaults)
    }

    /// The focusable text fields of this build that run something on Enter
    /// ([`Element::on_submit`]).
    pub(crate) fn submitting_fields(&self) -> Vec<FocusId> {
        self.field_hooks
            .iter()
            .filter(|(_, h)| h.submits())
            .map(|(id, _)| *id)
            .collect()
    }
}

impl<S> AppHandle<S> {
    /// Move keyboard focus in window `window` to the element keyed `key`
    /// (see [`Element::key`]), or to the first focusable inside it: from a
    /// handler that opens a find bar, or to give focus back to a terminal's
    /// canvas when an overlay closes. It applies after the window's next
    /// build, so an element the same handler made appear can take it.
    /// Nothing happens if no element has that key.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Term { finding: bool }
    /// fn view(s: &Term, cx: &mut Cx<Term>) -> Element {
    ///     let (h, win) = (cx.handle(), cx.window_id());
    ///     button(cx, "Close find bar").on_tap(cx, move |s| {
    ///         s.finding = false;
    ///         h.focus(win, "terminal");
    ///     })
    /// }
    /// # let theme = Theme::light();
    /// # let _ = view(&Term { finding: true }, &mut Cx::new(&theme));
    /// ```
    pub fn focus(&self, window: WindowId, key: impl std::hash::Hash) {
        self.request_focus(window, FocusRequest::Key(ElementKey::new(key)));
    }

    /// Take keyboard focus in window `window` away from whatever has it.
    pub fn blur(&self, window: WindowId) {
        self.request_focus(window, FocusRequest::Blur);
    }
}
