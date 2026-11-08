//! Toasts: transient notifications stacked in a corner of the window, each
//! dismissible, optionally dismissing themselves after a while.

use std::time::Duration;

use crate::core::{Align, Anchor, AppHandle, Axis, BoxStyle, Cx, Element, OverlaySpec, Role};
use crate::geometry::Insets;
use crate::style::Theme;

/// At most this many toasts show at once (the newest); the rest wait.
const MAX_SHOWN: usize = 4;
/// The width of a toast, in logical pixels.
const WIDTH: f64 = 320.0;

/// Identifies a toast, to [`dismiss`](Toasts::dismiss) it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

/// What a toast is about, which sets its accent color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// One notification.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: ToastId,
    pub message: String,
    pub kind: ToastKind,
}

/// The toasts currently up, oldest first. Keep one in your app state and show
/// it with [`toasts`].
///
/// ```
/// use stipple::widgets::{ToastKind, Toasts};
/// let mut t = Toasts::new();
/// let id = t.push("Saved", ToastKind::Success);
/// assert_eq!(t.len(), 1);
/// assert!(t.dismiss(id));
/// assert!(t.is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    /// No toasts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Show `message` until it's dismissed (by its × button or
    /// [`dismiss`](Self::dismiss)).
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) -> ToastId {
        let id = ToastId(self.next);
        self.next += 1;
        self.items.push(Toast {
            id,
            message: message.into(),
            kind,
        });
        id
    }

    /// Show `message` and dismiss it after `duration`, with a timer on the
    /// app's `handle` (from [`Cx::handle`], cloned into your handler). `field`
    /// is an accessor returning these toasts inside the app state.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// struct App { toasts: Toasts }
    /// fn view(_: &App, cx: &mut Cx<App>) -> Element {
    ///     let theme = *cx.theme();
    ///     let handle = cx.handle();
    ///     button_labeled(&theme, "Save").on_tap(cx, move |s: &mut App| {
    ///         s.toasts.push_for(&handle, Duration::from_secs(4), |s: &mut App| &mut s.toasts,
    ///             "Saved", ToastKind::Success);
    ///     })
    /// }
    /// ```
    pub fn push_for<S>(
        &mut self,
        handle: &AppHandle<S>,
        duration: Duration,
        field: impl Fn(&mut S) -> &mut Toasts + Send + 'static,
        message: impl Into<String>,
        kind: ToastKind,
    ) -> ToastId {
        let id = self.push(message, kind);
        handle.after(duration, move |s| {
            field(s).dismiss(id);
        });
        id
    }

    /// Take the toast `id` down. Returns `false` if it was already gone.
    pub fn dismiss(&mut self, id: ToastId) -> bool {
        let before = self.items.len();
        self.items.retain(|t| t.id != id);
        self.items.len() != before
    }

    /// Take every toast down.
    pub fn clear(&mut self) {
        self.items.clear();
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
}

/// Show `state`'s toasts stacked in the window's bottom-right corner, above
/// everything else, newest at the bottom (at most four at a time). Each has an
/// accent bar in its [`ToastKind`]'s color and a × button that dismisses it.
/// Call it once per frame, anywhere in the view; it adds an overlay rather
/// than returning an element. Presses outside the toasts reach the UI below.
///
/// `field` is an accessor returning the [`Toasts`] inside the app state.
///
/// Self-drawn under every theme. Each toast is reported to assistive
/// technologies as a [`Role::Alert`] named by its message.
pub fn toasts<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &Toasts,
    field: impl Fn(&mut S) -> &mut Toasts + Copy + 'static,
) {
    if state.is_empty() {
        return;
    }
    let shown = &state.items[state.items.len().saturating_sub(MAX_SHOWN)..];
    let cards = shown
        .iter()
        .map(|t| toast_card(cx, theme, t, field))
        .collect();
    let content = Element::stack(Axis::Vertical, cards)
        .gap(theme.spacing.sm)
        .align(Align::Start, Align::Stretch)
        .width(WIDTH);
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::Aligned {
            horizontal: Align::End,
            vertical: Align::End,
            margin: theme.spacing.lg,
        },
        modal: false,
        dismiss: None,
    });
}

fn toast_card<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    toast: &Toast,
    field: impl Fn(&mut S) -> &mut Toasts + Copy + 'static,
) -> Element {
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
    let close = Element::stack(
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
    .min_height(24.0)
    .a11y_label("Dismiss")
    .on_tap(cx, move |s: &mut S| {
        field(s).dismiss(id);
    });
    let message = Element::stack(Axis::Horizontal, vec![message])
        .align(Align::Start, Align::Center)
        .grow(1.0)
        .shrink(1.0);
    Element::stack(Axis::Horizontal, vec![bar, message, close])
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.md))
        .gap(theme.spacing.md)
        .align(Align::Start, Align::Stretch)
        .role(Role::Alert)
        .a11y_label(toast.message.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::geometry::Size;
    use crate::platform::time::Instant;

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
    fn push_for_dismisses_on_its_timer() {
        struct State {
            toasts: Toasts,
        }
        let handle = AppHandle::<State>::new();
        let mut state = State {
            toasts: Toasts::new(),
        };
        let start = Instant::now();
        state.toasts.push_for(
            &handle,
            Duration::from_millis(50),
            |s: &mut State| &mut s.toasts,
            "brief",
            ToastKind::Info,
        );
        state.toasts.push("sticky", ToastKind::Info);
        handle.pump(&mut state, start);
        assert_eq!(state.toasts.len(), 2, "not due yet");
        handle.pump(&mut state, start + Duration::from_millis(60));
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(state.toasts.items()[0].message, "sticky");
    }

    #[derive(Default)]
    struct App1 {
        toasts: Toasts,
        clicks: u32,
    }

    #[test]
    fn toasts_float_bottom_right_and_dismiss_on_click() {
        let mut state = App1::default();
        for i in 0..6 {
            state.toasts.push(format!("note {i}"), ToastKind::Success);
        }
        let mut app = App::new(state, |s: &App1, cx: &mut Cx<App1>| {
            let theme = *cx.theme();
            toasts(cx, &theme, &s.toasts, |s: &mut App1| &mut s.toasts);
            Element::stack(Axis::Vertical, vec![])
                .grow(1.0)
                .on_tap(cx, |s: &mut App1| s.clicks += 1)
        })
        .logical_size(Size::new(600.0, 500.0));
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let alerts: Vec<_> = tree
            .descendants()
            .into_iter()
            .filter(|n| n.role == Role::Alert)
            .cloned()
            .collect();
        assert_eq!(alerts.len(), MAX_SHOWN, "only the newest few show");
        assert_eq!(alerts[0].name, "note 2");
        let last = alerts.last().unwrap().bounds;
        assert!(last.max_x() > 560.0 && last.max_y() > 460.0, "{last:?}");
        // A click beside the toasts reaches the UI below (no catcher).
        assert!(app.click_at(crate::geometry::Point::new(20.0, 20.0)));
        assert_eq!(app.state().clicks, 1);
        // The newest toast's × dismisses it.
        let close = alerts
            .last()
            .unwrap()
            .children
            .iter()
            .find(|c| c.name == "Dismiss")
            .unwrap()
            .bounds
            .center();
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
}
