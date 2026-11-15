//! Toasts: transient notifications, stacked in a corner of the window or, on
//! a phone, shown one at a time as a bar across the bottom (a snackbar). Each
//! is dismissible (its ×, or a swipe sideways), may carry one action (an
//! "Undo"), and may dismiss itself after a while.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use crate::core::{
    Align, Anchor, AppHandle, Axis, BoxStyle, Cx, Element, OverlaySpec, PointerEvent,
    PointerEventKind, Role, Transition,
};
use crate::geometry::{Insets, Transform};
use crate::platform::PointerButton;
use crate::style::{MotionTokens, Theme};

use super::motion::{self, Memo};
use super::{Button, Variant};

/// At most this many toasts show at once in the corner (the newest); the
/// rest wait.
const MAX_SHOWN: usize = 4;
/// The width of a toast in the corner, in logical pixels.
const WIDTH: f64 = 320.0;
/// The widest a snackbar gets, in logical pixels.
const SNACKBAR_MAX: f64 = 560.0;
/// Windows narrower than this (logical pixels) get snackbars under
/// [`ToastPlacement::Auto`].
pub const NARROW_WINDOW: f64 = 600.0;
/// How far (logical pixels) a press on a toast moves before it counts as a
/// swipe, not a tap (the touch recognizer's slop).
const SWIPE_SLOP: f64 = 8.0;
/// The share of its width a toast has to be swiped to go.
const SWIPE_DISMISS: f64 = 0.35;

/// Identifies a toast, to [`dismiss`](Toasts::dismiss) it or give it an
/// [`action`](Toasts::action).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

/// What a toast is about, which sets its accent color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToastKind {
    /// Neutral news (the theme's primary color).
    #[default]
    Info,
    /// Something worked (the success color).
    Success,
    /// Something needs attention (the warning color).
    Warning,
    /// Something failed (the danger color).
    Error,
}

/// Where the [`toasts`] view puts the toasts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToastPlacement {
    /// [`Bottom`](Self::Bottom) in a window narrower than [`NARROW_WINDOW`]
    /// (a phone, a narrow split), else [`Corner`](Self::Corner).
    #[default]
    Auto,
    /// Stacked in the window's bottom trailing corner (bottom-right, or
    /// bottom-left right to left), up to four, newest at the bottom.
    Corner,
    /// A snackbar: one toast at a time (the newest), centered at the bottom,
    /// as wide as the window less a margin, up to 560 logical pixels.
    Bottom,
}

impl ToastPlacement {
    /// The placement used in a window `width` logical pixels wide: `Auto`
    /// made `Corner` or `Bottom` (a zero width, no window yet, is `Corner`).
    pub fn resolve(self, width: f64) -> ToastPlacement {
        match self {
            ToastPlacement::Auto if width > 0.0 && width < NARROW_WINDOW => ToastPlacement::Bottom,
            ToastPlacement::Auto => ToastPlacement::Corner,
            p => p,
        }
    }
}

/// A toast's action: its button's label and what it runs (an [`ActionFn`]
/// for the app state's type, type-erased so [`Toasts`] needn't name it).
#[derive(Clone)]
struct ToastAction {
    label: String,
    run: Arc<dyn Any + Send + Sync>,
}

/// What an action runs, on the app state `S`.
type ActionFn<S> = Box<dyn Fn(&mut S) + Send + Sync>;

impl std::fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToastAction")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ToastAction {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label && Arc::ptr_eq(&self.run, &other.run)
    }
}

/// One notification.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Toast {
    /// Which toast it is.
    pub id: ToastId,
    /// What it says.
    pub message: String,
    /// What it's about.
    pub kind: ToastKind,
    action: Option<ToastAction>,
}

impl Toast {
    /// The label of its action's button, if it has one (see
    /// [`Toasts::action`]).
    pub fn action_label(&self) -> Option<&str> {
        self.action.as_ref().map(|a| a.label.as_str())
    }
}

/// A finger (or the mouse) dragging a toast sideways.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Swipe {
    id: ToastId,
    /// Where the press landed (window x).
    from: f64,
    /// How far it has moved since.
    dx: f64,
    /// The toast's width.
    width: f64,
}

/// The toasts currently up, oldest first, and where they show. Keep one in
/// your app state and show it with [`toasts`].
///
/// ```
/// use stipple::widgets::{ToastKind, ToastPlacement, Toasts};
/// struct App { notes: Vec<String> }
/// let mut t = Toasts::new()
///     .placement(ToastPlacement::Bottom)
///     .bottom_offset(56.0);
/// let id = t.push("Deleted", ToastKind::Info);
/// t.action(id, "Undo", |s: &mut App| s.notes.push("restored".into()));
/// assert_eq!(t.items()[0].action_label(), Some("Undo"));
/// assert!(t.dismiss(id));
/// assert!(t.is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
    placement: ToastPlacement,
    bottom_offset: f64,
    /// The toast being swiped.
    swipe: Option<Swipe>,
    /// A swipe just ended on this toast: the tap its release makes doesn't
    /// run the toast's action.
    swiped: Option<ToastId>,
    /// What the view showed last, for the toasts' entrances and exits.
    motion: Memo<Shelf>,
}

/// The toasts a [`toasts`] view showed in its last build, and when each came
/// in, plus the ones on their way out.
#[derive(Clone, Debug, Default)]
struct Shelf {
    built: bool,
    /// Shown, in order, with when they came in (`None`: there from the
    /// start, so at rest).
    shown: Vec<(Toast, Option<Duration>)>,
    leaving: Vec<Leaving>,
    /// The toast being swiped in the last build, and how far.
    dragged: Option<(ToastId, f64)>,
    /// A toast let go short of the dismiss distance, springing back from
    /// that far since then.
    settling: Option<Settling>,
}

/// A toast springing back from a swipe: which, from how far, since when.
type Settling = (ToastId, f64, Duration);

/// A toast on its way out: where it was in the stack, since when, and how
/// far it had been swiped (0 if it wasn't).
#[derive(Clone, Debug)]
struct Leaving {
    toast: Toast,
    at: usize,
    since: Duration,
    swiped: f64,
}

impl Toasts {
    /// No toasts, placed [`Auto`](ToastPlacement::Auto).
    pub fn new() -> Self {
        Self::default()
    }

    /// These toasts, shown at `placement` (see [`ToastPlacement::resolve`]).
    pub fn placement(mut self, placement: ToastPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Show the toasts at `placement` from now on.
    pub fn set_placement(&mut self, placement: ToastPlacement) {
        self.placement = placement;
    }

    /// These toasts, raised `px` logical pixels above their usual place (the
    /// theme's margin from the window's bottom edge): above a bottom
    /// navigation bar, a tab bar, or the bottom safe area.
    pub fn bottom_offset(mut self, px: f64) -> Self {
        self.set_bottom_offset(px);
        self
    }

    /// Raise the toasts `px` logical pixels above their usual place from now
    /// on (see [`bottom_offset`](Self::bottom_offset)).
    pub fn set_bottom_offset(&mut self, px: f64) {
        self.bottom_offset = px.max(0.0);
    }

    /// Show `message` until it's dismissed (by its × button, a swipe or
    /// [`dismiss`](Self::dismiss)).
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) -> ToastId {
        let id = ToastId(self.next);
        self.next += 1;
        self.items.push(Toast {
            id,
            message: message.into(),
            kind,
            action: None,
        });
        id
    }

    /// Dismiss the toast `id` after `duration` (say one just
    /// [`push`](Self::push)ed), with a timer on the app's `handle` (from
    /// [`Cx::handle`], cloned into your handler). `field` is an accessor
    /// returning these toasts inside the app state.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// struct App { toasts: Toasts }
    /// fn view(_: &App, cx: &mut Cx<App>) -> Element {
    ///     let handle = cx.handle();
    ///     button(cx, "Save").on_tap(cx, move |s| {
    ///         let id = s.toasts.push("Saved", ToastKind::Success);
    ///         s.toasts.dismiss_after(id, Duration::from_secs(4), &handle, |s| &mut s.toasts);
    ///     })
    /// }
    /// ```
    pub fn dismiss_after<S>(
        &self,
        id: ToastId,
        duration: Duration,
        handle: &AppHandle<S>,
        field: impl Fn(&mut S) -> &mut Toasts + Send + 'static,
    ) {
        handle.after(duration, move |s| {
            field(s).dismiss(id);
        });
    }

    /// Give the toast `id` an action: a button labeled `label` that
    /// dismisses the toast and runs `run` on the app state (of type `S`, the
    /// state the [`toasts`] view is built for; a closure taking another type
    /// never runs). Returns `false` if the toast is gone.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// struct App { toasts: Toasts, notes: Vec<String> }
    /// fn view(_: &App, cx: &mut Cx<App>) -> Element {
    ///     let handle = cx.handle();
    ///     button(cx, "Delete").on_tap(cx, move |s| {
    ///         let Some(note) = s.notes.pop() else { return };
    ///         let id = s.toasts.push("Deleted", ToastKind::Info);
    ///         s.toasts.dismiss_after(id, Duration::from_secs(6), &handle, |s| &mut s.toasts);
    ///         s.toasts.action(id, "Undo", move |s: &mut App| s.notes.push(note.clone()));
    ///     })
    /// }
    /// ```
    pub fn action<S: 'static>(
        &mut self,
        id: ToastId,
        label: impl Into<String>,
        run: impl Fn(&mut S) + Send + Sync + 'static,
    ) -> bool {
        let Some(toast) = self.items.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        let run: ActionFn<S> = Box::new(run);
        toast.action = Some(ToastAction {
            label: label.into(),
            run: Arc::new(run),
        });
        true
    }

    /// Take the toast `id` down. Returns `false` if it was already gone.
    pub fn dismiss(&mut self, id: ToastId) -> bool {
        let before = self.items.len();
        self.items.retain(|t| t.id != id);
        if self.swipe.is_some_and(|s| s.id == id) {
            self.swipe = None;
        }
        self.items.len() != before
    }

    /// Take every toast down.
    pub fn clear(&mut self) {
        self.items.clear();
        self.swipe = None;
    }

    /// The toasts up, oldest first.
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// How many toasts are up.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no toast is up.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// A pointer event on the toast `id`: a press starts a swipe, moves move
    /// it, and the release dismisses the toast if it went far enough.
    fn swipe_event(&mut self, id: ToastId, ev: &PointerEvent) {
        let x = ev.window_position.x;
        match ev.kind {
            PointerEventKind::Down if ev.button == Some(PointerButton::Left) => {
                self.swiped = None;
                self.swipe = Some(Swipe {
                    id,
                    from: x,
                    dx: 0.0,
                    width: ev.size.width,
                });
            }
            PointerEventKind::Move => {
                if let Some(s) = self.swipe.as_mut().filter(|s| s.id == id) {
                    s.dx = x - s.from;
                }
            }
            PointerEventKind::Up if ev.button == Some(PointerButton::Left) => {
                let Some(s) = self.swipe.take().filter(|s| s.id == id) else {
                    return;
                };
                let dx = x - s.from;
                if dx.abs() > SWIPE_SLOP {
                    self.swiped = Some(id);
                }
                // Where it was let go, for the exit (or the spring back) to
                // carry on from.
                self.motion.with(|shelf| {
                    if let Some(d) = shelf.dragged.as_mut().filter(|d| d.0 == id) {
                        d.1 = dx;
                    }
                });
                if dx.abs() >= (s.width * SWIPE_DISMISS).max(SWIPE_SLOP * 4.0) {
                    self.dismiss(id);
                }
            }
            _ => {}
        }
    }

    /// The toast `id`'s action button was pressed: unless that was the end
    /// of a swipe, dismiss the toast and run its action on `state`.
    fn run_action<S: 'static>(state: &mut S, field: impl Fn(&mut S) -> &mut Toasts, id: ToastId) {
        let toasts = field(state);
        if toasts.swiped.take() == Some(id) {
            return;
        }
        let Some(run) = toasts
            .items
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.action.as_ref())
            .map(|a| a.run.clone())
        else {
            return;
        };
        toasts.dismiss(id);
        if let Some(run) = run.downcast_ref::<ActionFn<S>>() {
            run(state);
        }
    }
}

/// Show `state`'s toasts above everything else, where its
/// [`ToastPlacement`] says: stacked in the window's bottom trailing corner,
/// newest at the bottom (at most four at a time), or, in a narrow window, the
/// newest alone as a snackbar across the bottom (older ones wait behind it,
/// their timers running). Each has an accent bar in its [`ToastKind`]'s
/// color, its action's button if it has one, and a × button that dismisses
/// it. Dragging one sideways (a finger or the mouse) moves it along: let go
/// past about a third of its width, it goes; short of that, it springs back.
/// Call it once per frame, anywhere in the view; it adds an overlay rather
/// than returning an element. Presses outside the toasts reach the UI below.
///
/// `field` is an accessor returning the [`Toasts`] inside the app state.
///
/// Self-drawn under every theme. Each toast is reported to assistive
/// technologies as a [`Role::Alert`] named by its message, which the
/// platforms' screen readers are told about when it appears (see the Toasts
/// section of `docs/widgets.md`).
pub fn toasts<S: 'static>(
    cx: &mut Cx<S>,
    state: &Toasts,
    field: impl Fn(&mut S) -> &mut Toasts + Copy + 'static,
) {
    let theme = &super::theme_of(cx);
    let window = cx.window_size();
    let bottom = state.placement.resolve(window.width) == ToastPlacement::Bottom;
    let max = if bottom { 1 } else { MAX_SHOWN };
    let shown = &state.items[state.items.len().saturating_sub(max)..];
    let now = cx.frame_time();
    let m = theme.motion;
    let dragged = state.swipe.map(|s| (s.id, s.dx));
    // Which toasts came in since the last build (and when), which went, and
    // which one springs back from a swipe.
    let (entering, leaving, settling) = state
        .motion
        .with(|shelf| shelf.update(shown, dragged, now, &m));
    if shown.is_empty() && leaving.is_empty() {
        return;
    }
    let margin = theme.spacing.lg;
    let width = if bottom && window.width > 0.0 {
        (window.width - 2.0 * margin).clamp(0.0, SNACKBAR_MAX)
    } else {
        WIDTH
    };
    // Toasts slide in from the window's edge, the one they're stacked at
    // (the bottom one for a snackbar).
    let edge = if theme.is_rtl() { -1.0 } else { 1.0 };
    let from = if bottom {
        (0.0, -m.toast_offset)
    } else {
        (-edge * m.toast_offset, 0.0)
    };
    let mut cards: Vec<Element> = shown
        .iter()
        .zip(entering)
        .map(|(t, since)| {
            let p = motion::progress(cx, m.toast_enter, since);
            // Keyed, the cards above glide up when one comes in below them
            // (and down when one has left).
            let mut card = toast_card(cx, theme, t, Some(field))
                .key(("stipple-toast", t.id.0))
                .animate_position(Transition(m.toast_enter));
            // Following a swipe, or springing back from one.
            let dx = match (dragged, settling) {
                (Some((id, dx)), _) if id == t.id => dx,
                (_, Some((id, dx, since))) if id == t.id => {
                    dx * (1.0 - motion::progress(cx, m.toast_enter, Some(since)))
                }
                _ => 0.0,
            };
            if dx != 0.0 {
                card = swiped(card, dx, width);
            }
            motion::enter(card, p, m.toast_scale, from, (0.5, 0.5))
        })
        .collect();
    // The leaving ones fade and slide back out where they were (or on, the
    // way they were swiped), without their buttons or their alert role.
    for l in leaving {
        let p = motion::progress(cx, m.toast_exit, Some(l.since));
        let card = toast_card(cx, theme, &l.toast, false.then_some(field));
        let card = if l.swiped != 0.0 {
            let dx = l.swiped + l.swiped.signum() * width * 0.5 * p;
            swiped(card, dx, width).opacity((1.0 - p).clamp(0.0, 1.0) as f32)
        } else {
            let toward = if bottom { (0.0, 1.0) } else { (edge, 0.0) };
            exit(card, &m, p, toward)
        };
        cards.insert(l.at.min(cards.len()), card);
    }
    let content = Element::stack(Axis::Vertical, cards)
        .gap(theme.spacing.sm)
        .width(width);
    let insets = Insets {
        top: margin,
        right: margin,
        bottom: margin + state.bottom_offset,
        left: margin,
    };
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::Inset {
            horizontal: if bottom { Align::Center } else { Align::End },
            vertical: Align::End,
            insets,
        },
        modal: false,
        dismiss: None,
    });
}

/// A card moved `dx` sideways by a swipe, fading as it goes (out of a
/// toast `width` wide).
fn swiped(card: Element, dx: f64, width: f64) -> Element {
    let fade = 1.0 - (dx.abs() / width.max(1.0)).min(1.0) * 0.6;
    card.transform(Transform::IDENTITY.translate(dx, 0.0))
        .opacity(fade as f32)
}

impl Shelf {
    /// Note the toasts `shown` in the build at `now`, and the one `dragged`
    /// (and how far): returns when each came in (`None`: at rest), the ones
    /// still leaving, and the one springing back from a swipe.
    fn update(
        &mut self,
        shown: &[Toast],
        dragged: Option<(ToastId, f64)>,
        now: Duration,
        m: &MotionTokens,
    ) -> (Vec<Option<Duration>>, Vec<Leaving>, Option<Settling>) {
        // Toasts there in the first build don't come in: they're there.
        let new = self.built.then(|| motion::start_of(now));
        let entering: Vec<Option<Duration>> = shown
            .iter()
            .map(|t| {
                self.shown
                    .iter()
                    .find(|(s, _)| s.id == t.id)
                    .map_or(new, |(_, since)| *since)
            })
            .collect();
        // The one let go since: it leaves from where it was, or springs back.
        let released = self
            .dragged
            .filter(|&(id, dx)| dx != 0.0 && dragged.is_none_or(|(d, _)| d != id));
        for (i, (t, _)) in self.shown.iter().enumerate() {
            if !shown.iter().any(|s| s.id == t.id) {
                let swiped = released
                    .filter(|&(id, _)| id == t.id)
                    .map_or(0.0, |(_, dx)| dx);
                self.leaving.push(Leaving {
                    toast: t.clone(),
                    at: i,
                    since: motion::start_of(now),
                    swiped,
                });
            }
        }
        if let Some((id, dx)) = released
            && shown.iter().any(|s| s.id == id)
        {
            self.settling = Some((id, dx, motion::start_of(now)));
        }
        if dragged.is_some()
            || self
                .settling
                .is_some_and(|(_, _, since)| !motion::progress_at(m.toast_enter, since, now).1)
        {
            self.settling = None;
        }
        self.dragged = dragged;
        self.built = true;
        self.shown = shown
            .iter()
            .cloned()
            .zip(entering.iter().copied())
            .collect();
        self.leaving
            .retain(|l| motion::progress_at(m.toast_exit, l.since, now).1);
        (entering, self.leaving.clone(), self.settling)
    }
}

/// A toast's card leaving at progress `p`: fading, sliding back toward the
/// edge it came from (`toward`: (±1, 0) right or left, (0, 1) down) and
/// scaling to the theme's toast scale.
fn exit(card: Element, m: &MotionTokens, p: f64, toward: (f64, f64)) -> Element {
    let t = Transform::IDENTITY
        .scale(1.0 + (m.toast_scale - 1.0) * p)
        .translate(toward.0 * m.toast_offset * p, toward.1 * m.toast_offset * p);
    card.opacity((1.0 - p).clamp(0.0, 1.0) as f32).transform(t)
}

/// One toast's card; with `field`, live (its × dismisses it, its action
/// button runs the action, a swipe moves it, and assistive technologies hear
/// it as an alert), else a picture of one leaving.
fn toast_card<S: 'static, F>(
    cx: &mut Cx<S>,
    theme: &Theme,
    toast: &Toast,
    field: Option<F>,
) -> Element
where
    F: Fn(&mut S) -> &mut Toasts + Copy + 'static,
{
    let p = &theme.palette;
    let accent = match toast.kind {
        ToastKind::Info => p.primary,
        ToastKind::Success => p.success,
        ToastKind::Warning => p.warning,
        ToastKind::Error => p.danger,
    };
    let bar = Element::boxed(BoxStyle {
        fill: Some(accent),
        radius: 2.0,
        border: None,
    })
    .width(4.0);
    let message = Element::text(toast.message.clone(), theme.typography.body, p.text)
        .wrap()
        .grow(1.0)
        .shrink(1.0);
    let id = toast.id;
    let mut close = Element::stack(
        Axis::Horizontal,
        vec![Element::text(
            "\u{00D7}",
            theme.typography.title,
            p.text_muted,
        )],
    )
    .radius(theme.radius / 2.0)
    .align(Align::Center, Align::Center)
    .min_width(24.0)
    .min_height(24.0);
    let mut action = toast.action.as_ref().map(|a| {
        Button::new(a.label.clone())
            .variant(Variant::Ghost)
            .view(cx)
    });
    if let Some(field) = field {
        close = close.a11y_label("Dismiss").on_tap(cx, move |s| {
            field(s).dismiss(id);
        });
        action = action.map(|button| button.on_tap(cx, move |s| Toasts::run_action(s, field, id)));
    }
    let message = Element::stack(Axis::Horizontal, vec![message])
        .grow(1.0)
        .shrink(1.0);
    let mut parts = vec![bar, message];
    if let Some(action) = action {
        parts.push(Element::stack(Axis::Horizontal, vec![action]));
    }
    parts.push(close);
    let card = Element::stack(Axis::Horizontal, parts)
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.md))
        .gap(theme.spacing.md)
        .align(Align::Start, Align::Stretch);
    match field {
        Some(field) => card
            .role(Role::Alert)
            .a11y_label(toast.message.clone())
            .on_pointer(cx, move |ev: &PointerEvent, s: &mut S| {
                field(s).swipe_event(id, ev)
            }),
        None => card,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::core::AccessNode;
    use crate::geometry::{Point, Size};
    use crate::platform::time::Instant;
    use crate::platform::{ButtonState, Event};

    #[test]
    fn push_and_dismiss() {
        let mut t = Toasts::new();
        let a = t.push("one", ToastKind::Info);
        let b = t.push("two", ToastKind::Error);
        assert_ne!(a, b);
        assert!(t.dismiss(a));
        assert!(!t.dismiss(a), "already gone");
        assert_eq!(t.items()[0].message, "two");
        t.clear();
        assert!(t.is_empty());
    }

    #[test]
    fn dismiss_after_dismisses_on_its_timer() {
        struct State {
            toasts: Toasts,
        }
        let handle = AppHandle::<State>::new();
        let mut state = State {
            toasts: Toasts::new(),
        };
        let start = Instant::now();
        let id = state.toasts.push("brief", ToastKind::Info);
        state
            .toasts
            .dismiss_after(id, Duration::from_millis(50), &handle, |s| &mut s.toasts);
        state.toasts.push("sticky", ToastKind::Info);
        handle.pump(&mut state, start);
        assert_eq!(state.toasts.len(), 2, "not due yet");
        handle.pump(&mut state, start + Duration::from_millis(60));
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(state.toasts.items()[0].message, "sticky");
    }

    #[test]
    fn placement_resolves_by_window_width() {
        use ToastPlacement::*;
        assert_eq!(Auto.resolve(360.0), Bottom);
        assert_eq!(Auto.resolve(600.0), Corner);
        assert_eq!(Auto.resolve(0.0), Corner, "no window yet");
        assert_eq!(Corner.resolve(360.0), Corner);
        assert_eq!(Bottom.resolve(1200.0), Bottom);
    }

    #[derive(Default)]
    struct App1 {
        toasts: Toasts,
        clicks: u32,
        undone: u32,
    }

    fn build(state: App1, size: Size) -> App<App1> {
        App::new(state, |s: &App1, cx: &mut Cx<App1>| {
            toasts(cx, &s.toasts, |s| &mut s.toasts);
            Element::stack(Axis::Vertical, vec![])
                .grow(1.0)
                .on_tap(cx, |s| s.clicks += 1)
        })
        .no_font()
        .logical_size(size)
    }

    fn alerts(app: &App<App1>) -> Vec<AccessNode> {
        let tree = app.accessibility_tree().unwrap();
        tree.descendants()
            .into_iter()
            .filter(|n| n.role == Role::Alert)
            .cloned()
            .collect()
    }

    fn child_named(node: &AccessNode, name: &str) -> Point {
        node.descendants()
            .into_iter()
            .find(|c| c.name == name && c.role == Role::Button)
            .unwrap_or_else(|| panic!("no {name:?} in {node:?}"))
            .bounds
            .center()
    }

    #[test]
    fn toasts_float_bottom_right_and_dismiss_on_click() {
        let mut state = App1::default();
        for i in 0..6 {
            state.toasts.push(format!("note {i}"), ToastKind::Success);
        }
        let mut app = build(state, Size::new(600.0, 500.0));
        app.render_once();
        let alerts = alerts(&app);
        assert_eq!(alerts.len(), MAX_SHOWN, "only the newest few show");
        assert_eq!(alerts[0].name, "note 2");
        let last = alerts.last().unwrap().bounds;
        assert!(last.max_x() > 560.0 && last.max_y() > 460.0, "{last:?}");
        // A click beside the toasts reaches the UI below (no catcher).
        assert!(app.click_at(Point::new(20.0, 20.0)));
        assert_eq!(app.state().clicks, 1);
        // The newest toast's × dismisses it.
        let close = child_named(alerts.last().unwrap(), "Dismiss");
        assert!(app.click_at(close));
        assert_eq!(app.state().toasts.len(), 5);
        assert!(
            app.state()
                .toasts
                .items()
                .iter()
                .all(|t| t.message != "note 5")
        );
    }

    #[test]
    fn a_narrow_window_shows_one_snackbar_across_the_bottom() {
        let mut state = App1::default();
        state.toasts.set_bottom_offset(56.0);
        state.toasts.push("Copied", ToastKind::Info);
        state
            .toasts
            .push("Batch saved: 3 items", ToastKind::Success);
        let mut app = build(state, Size::new(360.0, 640.0));
        app.render_once();
        let alerts = alerts(&app);
        assert_eq!(alerts.len(), 1, "one at a time");
        assert_eq!(alerts[0].name, "Batch saved: 3 items", "the newest");
        let b = alerts[0].bounds;
        let margin = Theme::light().spacing.lg;
        assert!((b.min_x() - margin).abs() < 0.5, "{b:?}");
        assert!((b.max_x() - (360.0 - margin)).abs() < 0.5, "{b:?}");
        assert!(
            (b.max_y() - (640.0 - margin - 56.0)).abs() < 0.5,
            "above the offset: {b:?}"
        );
        // The bar the offset keeps clear still takes its presses.
        assert!(app.click_at(Point::new(180.0, 620.0)));
        assert_eq!(app.state().clicks, 1);
        // The older one shows once the newest goes.
        let close = child_named(&alerts[0], "Dismiss");
        assert!(app.click_at(close));
        app.render_once();
        assert_eq!(self::alerts(&app)[0].name, "Copied");
    }

    #[test]
    fn a_wide_window_caps_an_explicit_snackbar_and_centers_it() {
        let mut state = App1::default();
        state.toasts.set_placement(ToastPlacement::Bottom);
        state.toasts.push("Deleted", ToastKind::Info);
        let mut app = build(state, Size::new(1000.0, 500.0));
        app.render_once();
        let b = alerts(&app)[0].bounds;
        assert!((b.width() - SNACKBAR_MAX).abs() < 0.5, "{b:?}");
        assert!((b.center().x - 500.0).abs() < 0.5, "{b:?}");
        // And a narrow window can keep the corner.
        let mut state = App1::default();
        state.toasts.set_placement(ToastPlacement::Corner);
        state.toasts.push("a", ToastKind::Info);
        state.toasts.push("b", ToastKind::Info);
        let mut app = build(state, Size::new(360.0, 640.0));
        app.render_once();
        assert_eq!(alerts(&app).len(), 2);
        assert!((alerts(&app)[0].bounds.width() - WIDTH).abs() < 0.5);
    }

    #[test]
    fn the_action_button_runs_on_the_state_and_dismisses() {
        let mut state = App1::default();
        let id = state.toasts.push("Deleted", ToastKind::Info);
        assert!(
            state
                .toasts
                .action(id, "Undo", |s: &mut App1| s.undone += 1)
        );
        state.toasts.push("Copied", ToastKind::Info);
        let mut app = build(state, Size::new(800.0, 500.0));
        app.render_once();
        let alerts = alerts(&app);
        let undo = child_named(&alerts[0], "Undo");
        assert!(
            alerts[1].descendants().iter().all(|n| n.name != "Undo"),
            "only the toast given an action has a button for it"
        );
        assert!(app.click_at(undo));
        assert_eq!(app.state().undone, 1);
        assert_eq!(app.state().toasts.len(), 1);
        assert_eq!(app.state().toasts.items()[0].message, "Copied");
        // A closure for another state type is kept, but never runs.
        let mut t = Toasts::new();
        let id = t.push("x", ToastKind::Info);
        assert!(t.action(id, "Nope", |_: &mut u32| unreachable!()));
        let mut wrong = (t, 0u8);
        Toasts::run_action(&mut wrong, |w: &mut (Toasts, u8)| &mut w.0, id);
        assert!(wrong.0.is_empty(), "still dismissed");
    }

    fn press(app: &mut App<App1>, at: Point) {
        app.pointer_event(Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Pressed,
            position: at,
        });
    }

    fn drag(app: &mut App<App1>, to: Point) {
        app.pointer_event(Event::PointerMoved { position: to });
        app.render_once();
    }

    fn release(app: &mut App<App1>, at: Point) {
        app.pointer_event(Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Released,
            position: at,
        });
        app.render_once();
    }

    #[test]
    fn a_swipe_dismisses_and_a_short_one_springs_back() {
        let mut state = App1::default();
        state.toasts.push("Copied", ToastKind::Info);
        let id = state.toasts.push("Deleted", ToastKind::Info);
        state
            .toasts
            .action(id, "Undo", |s: &mut App1| s.undone += 1);
        let mut app = build(state, Size::new(360.0, 640.0));
        app.render_once();
        let card = alerts(&app)[0].bounds;
        let at = card.center();
        // A short drag (from the action button) springs back, and its
        // release doesn't press the button.
        let undo = child_named(&alerts(&app)[0], "Undo");
        press(&mut app, undo);
        drag(&mut app, Point::new(undo.x + 20.0, undo.y));
        assert_eq!(
            app.state().toasts.swipe.map(|s| s.dx),
            Some(20.0),
            "it follows the pointer"
        );
        assert_eq!(
            app.state().toasts.motion.with(|s| s.dragged),
            Some((id, 20.0)),
            "the view moved it along"
        );
        release(&mut app, Point::new(undo.x + 20.0, undo.y));
        assert_eq!(app.state().toasts.len(), 2, "not far enough");
        assert_eq!(app.state().undone, 0, "the swipe's release isn't a tap");
        assert!(app.state().toasts.motion.with(|s| s.settling.is_some()));
        assert!(app.animating(), "it springs back");
        assert_eq!(
            app.state().toasts.swiped,
            None,
            "the swipe's release used up its tap"
        );
        // A long swipe takes it away, sliding on out, and the next shows.
        press(&mut app, at);
        drag(&mut app, Point::new(at.x - 100.0, at.y));
        assert!(app.state().toasts.motion.with(|s| s.settling.is_none()));
        drag(&mut app, Point::new(at.x - 200.0, at.y));
        release(&mut app, Point::new(at.x - 200.0, at.y));
        assert_eq!(app.state().toasts.len(), 1, "swiped away");
        assert_eq!(app.state().undone, 0);
        assert_eq!(alerts(&app)[0].name, "Copied");
        let leaving = app.state().toasts.motion.with(|s| s.leaving.clone());
        assert_eq!(leaving.len(), 1);
        assert_eq!(leaving[0].toast.message, "Deleted");
        assert!((leaving[0].swiped + 200.0).abs() < 0.5, "{:?}", leaving[0]);
    }
}
