//! Pull to refresh: drag a vertical list down past its top on a touch
//! screen and an indicator follows the finger; let go past a threshold and
//! the app is asked to refresh, the indicator spinning until the app says
//! it's done.
//!
//! [`pull_to_refresh`] wraps a [`scroll`](super::scroll) or a
//! [`virtual_list`](super::virtual_list); its state is a [`RefreshState`] the
//! app keeps. The touch recognizer hands it the finger when the list is
//! scrolled to its top and the drag goes down (the hand-off a bottom sheet
//! uses), so a drag that starts anywhere else scrolls as usual.
//!
//! The look follows the platform: under the iOS look the content slides
//! down and a spinner turns in the gap above it; elsewhere a Material
//! circular indicator, a disc with an arc that grows as it's pulled, drops
//! from the top over the content.
//!
//! A mouse has no pull (desktop apps put a refresh button or a menu item in
//! their window); for the keyboard and assistive technologies, "Refresh" is
//! in the context menu of the list and of the rows inside it (the Menu key,
//! Shift+F10, a right-click, a long press).

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::anim::{Animated, Motion};
use crate::core::{Align, Axis, BoxStyle, Cx, Element, PointerEvent, PointerEventKind, Role};
use crate::geometry::Transform;
use crate::platform::PointerButton;
use crate::render::{Color, Svg};

use super::navigation::NavLook;

/// How far (logical pixels) the indicator has to come down for a release to
/// refresh.
pub const REFRESH_THRESHOLD: f64 = 64.0;
/// Where the indicator rests while refreshing (logical pixels pulled).
const REST: f64 = 64.0;
/// The furthest the indicator comes down, however far the finger goes: it
/// follows the finger less and less on the way.
const REACH: f64 = 160.0;
/// The Material indicator's disc.
const DISC: f64 = 40.0;
/// The arc or spinner inside it, and the iOS spinner.
const GLYPH: f64 = 24.0;
/// Distinct steps per turn and per pull, as the spinner has.
const STEPS: u32 = 60;
/// One turn while refreshing takes this long (seconds).
const PERIOD: f64 = 1.0;

/// A [`pull_to_refresh`]'s state, kept in the app state: whether the app is
/// refreshing, and a pull in progress.
#[derive(Debug)]
pub struct RefreshState {
    refreshing: bool,
    /// How far the indicator is pulled down (logical pixels).
    pull: Animated<f64>,
    /// A press: where it went down (window y) and whether it pulls yet.
    drag: Option<(f64, bool)>,
    /// How the indicator settles and goes (the theme's `swipe` motion, as
    /// of the last pull).
    motion: Motion,
    label: String,
}

impl Default for RefreshState {
    fn default() -> Self {
        Self::new()
    }
}

impl RefreshState {
    /// Not refreshing.
    pub fn new() -> Self {
        Self {
            refreshing: false,
            pull: Animated::new(0.0),
            drag: None,
            motion: crate::style::MotionTokens::standard().swipe,
            label: "Refresh".into(),
        }
    }

    /// Name the refresh action in menus and for assistive technologies
    /// (`"Refresh"` by default), in the user's language.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Rename the refresh action from now on (the user's language changed).
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }

    /// Whether the app is refreshing: from a pull (or [`begin`](Self::begin))
    /// until [`finish`](Self::finish). The indicator spins meanwhile.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    /// Whether a finger is pulling the list down.
    pub fn is_pulling(&self) -> bool {
        self.drag.is_some_and(|(_, pulling)| pulling)
    }

    /// Show the indicator spinning without a pull (a refresh the app
    /// started itself, or the keyboard's Refresh). It doesn't run
    /// `on_refresh`.
    pub fn begin(&mut self) {
        self.refreshing = true;
        self.pull.release(REST, self.motion, crate::anim::now());
    }

    /// The refresh is done: the indicator goes away.
    pub fn finish(&mut self) {
        self.refreshing = false;
        if self.drag.is_none() {
            self.pull.release(0.0, self.motion, crate::anim::now());
        }
    }

    /// How far down the indicator is at `now`.
    fn pulled(&self, now: Duration) -> f64 {
        self.pull.value(now).max(0.0)
    }
}

/// How far the indicator comes down for a finger `d` pixels down: all of it
/// at first, less and less on the way to [`REACH`].
fn rubber(d: f64) -> f64 {
    REACH * (1.0 - (-d.max(0.0) / REACH).exp())
}

/// The refresh action and its label, while the content of a
/// [`pull_to_refresh`] is built.
type Scoped<S> = (String, Rc<dyn Fn(&mut S)>);

thread_local! {
    /// The refresh actions of the `pull_to_refresh`es being built, innermost
    /// last (each a `Scoped<S>`).
    static SCOPE: RefCell<Vec<Rc<dyn Any>>> = const { RefCell::new(Vec::new()) };
}

/// The refresh action of the innermost [`pull_to_refresh`] being built for
/// state `S`, for the context menus of the rows and lists inside it.
pub(crate) fn scoped<S: 'static>() -> Option<Scoped<S>> {
    SCOPE.with(|s| {
        s.borrow()
            .iter()
            .rev()
            .find_map(|e| e.downcast_ref::<Scoped<S>>().cloned())
    })
}

/// A list that pulls to refresh, being built: see [`pull_to_refresh`]. Its
/// content comes in [`child`](Self::child).
#[must_use = "a pull-to-refresh shows only once `.child(content)` builds it"]
pub struct PullToRefresh {
    pull: f64,
    refreshing: bool,
    ios: bool,
    indicator: Option<Element>,
    pointer: crate::core::PointerId,
    context: Option<crate::core::ContextId>,
}

impl std::fmt::Debug for PullToRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PullToRefresh")
            .field("pull", &self.pull)
            .field("refreshing", &self.refreshing)
            .field("ios", &self.ios)
            .finish_non_exhaustive()
    }
}

impl Drop for PullToRefresh {
    fn drop(&mut self) {
        SCOPE.with(|s| s.borrow_mut().pop());
    }
}

/// A vertical list that refreshes when pulled down past its top: wrap a
/// [`scroll`](super::scroll) or a [`virtual_list`](super::virtual_list) with
/// [`child`](PullToRefresh::child).
///
/// `state` is its [`RefreshState`] and `access` reaches it inside the app
/// state. On a touch screen, a finger dragging the list down while it's
/// scrolled to its top pulls an indicator down after it (less and less
/// as it goes); let go past [`REFRESH_THRESHOLD`] and the state starts
/// refreshing and `on_refresh` runs, the indicator spinning in place until
/// the app calls [`RefreshState::finish`]; let go short of it and it goes
/// back up, on the theme's `swipe` motion. Under the iOS look the content
/// slides down and the spinner turns above it; elsewhere a Material disc
/// drops over the content.
///
/// A mouse has no pull. "Refresh" (or the state's
/// [`label`](RefreshState::label)) is in the context menu of the list and
/// of the rows inside it, for the keyboard (the Menu key, Shift+F10) and
/// assistive technologies; it runs `on_refresh` the same way.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{RefreshState, pull_to_refresh};
///
/// #[derive(Default)]
/// struct App { items: Vec<String>, refresh: RefreshState }
///
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     pull_to_refresh(cx, &state.refresh, |s| &mut s.refresh, |s| {
///         // Start loading; call `s.refresh.finish()` when it's in.
///         s.items.insert(0, "New".into());
///         s.refresh.finish();
///     })
///     .child(scroll(cx).children(state.items.iter().map(|i| label(cx, i)).collect()))
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App::default(), &mut Cx::new(&theme));
/// ```
pub fn pull_to_refresh<S: 'static>(
    cx: &mut Cx<S>,
    state: &RefreshState,
    access: impl Fn(&mut S) -> &mut RefreshState + 'static,
    on_refresh: impl Fn(&mut S) + 'static,
) -> PullToRefresh {
    let theme = super::theme_of(cx);
    let ios = NavLook::of(&theme) == NavLook::Ios;
    let motion = theme.motion.swipe;
    let access: Rc<dyn Fn(&mut S) -> &mut RefreshState> = Rc::new(access);
    let on_refresh: Rc<dyn Fn(&mut S)> = Rc::new(on_refresh);

    // The keyboard's and assistive technologies' way in.
    let refresh: Rc<dyn Fn(&mut S)> = {
        let access = access.clone();
        let on_refresh = on_refresh.clone();
        Rc::new(move |s: &mut S| {
            let st = access(s);
            if st.refreshing {
                return;
            }
            st.motion = motion;
            st.begin();
            on_refresh(s);
        })
    };
    let entry: Scoped<S> = (state.label.clone(), refresh);
    SCOPE.with(|s| s.borrow_mut().push(Rc::new(entry)));
    let context = super::swipe::entries_menu::<S>(&Vec::new()).map(|m| cx.register_context_menu(m));

    let puller = Puller {
        access,
        on_refresh,
        motion,
    };
    let pointer = cx.register_pointer(None, move |ev, s| puller.pointer(ev, s, crate::anim::now()));
    cx.pull_down(pointer);

    let pull = if state.pull.is_animating(cx.frame_time()) {
        cx.animate(&state.pull).max(0.0)
    } else {
        state.pulled(cx.frame_time())
    };
    let indicator = (pull > 0.5 || state.refreshing).then(|| {
        indicator(cx, ios, pull, state.refreshing).a11y_label(if state.refreshing {
            format!("{}…", state.label)
        } else {
            state.label.clone()
        })
    });
    PullToRefresh {
        pull,
        refreshing: state.refreshing,
        ios,
        indicator,
        pointer,
        context,
    }
}

impl PullToRefresh {
    /// Build it around `content`: the list (a [`scroll`](super::scroll) or a
    /// [`virtual_list`](super::virtual_list)) it takes the place of in the
    /// layout.
    pub fn child(mut self, content: Element) -> Element {
        // It takes the content's place: a scroll container that fills its
        // column has this fill it instead.
        let fills = content.scroll.is_some() && content.layout.size.height.is_none();
        let (grow, shrink) = if fills {
            (content.layout.grow.max(1.0), content.layout.shrink.max(1.0))
        } else {
            (content.layout.grow, content.layout.shrink)
        };
        let mut front = Element::stack(Axis::Vertical, vec![content]);
        if self.ios && self.pull > 0.5 {
            front = front.transform(Transform::IDENTITY.translate(0.0, self.pull));
        }
        let mut layers = Vec::with_capacity(2);
        let over = self.indicator.take().map(|ind| {
            let y = if self.ios {
                // Centered in the gap above the content.
                (self.pull - GLYPH) / 2.0
            } else {
                self.pull - DISC - 8.0
            };
            Element::stack(Axis::Vertical, vec![ind])
                .align(Align::Start, Align::Center)
                .transform(Transform::IDENTITY.translate(0.0, y))
                .hit_through()
        });
        if self.ios {
            layers.extend(over);
            layers.push(front);
        } else {
            layers.push(front);
            layers.extend(over);
        }
        let mut el = Element::layers(layers).grow(grow).shrink(shrink).clip();
        el.pointer = Some(self.pointer);
        el.context = self.context;
        el
    }
}

/// The indicator for a pull of `pull` pixels (or spinning, while
/// `refreshing`).
fn indicator<S>(cx: &mut Cx<S>, ios: bool, pull: f64, refreshing: bool) -> Element {
    let theme = super::theme_of(cx);
    let p = theme.palette;
    let step = if refreshing {
        cx.request_frame();
        let turns = cx.frame_time().as_secs_f64() / PERIOD;
        Step::Spin(((turns.fract() * STEPS as f64) as u32).min(STEPS - 1))
    } else {
        let progress = (pull / REFRESH_THRESHOLD).clamp(0.0, 1.0);
        Step::Pull((progress * STEPS as f64).round() as u32)
    };
    let color = if ios { p.text_muted } else { p.primary };
    let glyph = match glyph(ios, color, step) {
        Some(doc) => Element::image(&doc).width(GLYPH).height(GLYPH),
        None => Element::boxed(BoxStyle::default())
            .width(GLYPH)
            .height(GLYPH),
    };
    let el = if ios {
        glyph
    } else {
        Element::stack(Axis::Vertical, vec![glyph])
            .align(Align::Center, Align::Center)
            .width(DISC)
            .height(DISC)
            .radius(DISC / 2.0)
            .fill(p.surface)
            .border(p.border, 1.0)
    };
    el.role(Role::Text)
}

/// Which frame of the indicator: a pull's progress, or a turn's position,
/// out of [`STEPS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Step {
    Pull(u32),
    Spin(u32),
}

/// The indicator's glyph for `step`, built once and kept, as the spinner's.
fn glyph(ios: bool, color: Color, step: Step) -> Option<Arc<Svg>> {
    thread_local! {
        static GLYPHS: RefCell<HashMap<(bool, Color, Step), Arc<Svg>>> =
            RefCell::new(HashMap::new());
    }
    GLYPHS.with(|glyphs| {
        let key = (ios, color, step);
        if let Some(doc) = glyphs.borrow().get(&key) {
            return Some(doc.clone());
        }
        let source = if ios {
            spokes(color, step)
        } else {
            arc(color, step)
        };
        let doc = Arc::new(Svg::parse(source.as_bytes()).ok()?);
        let mut glyphs = glyphs.borrow_mut();
        if glyphs.len() > STEPS as usize * 8 {
            glyphs.clear();
        }
        glyphs.insert(key, doc.clone());
        Some(doc)
    })
}

/// Whether every glyph of both looks parses.
#[cfg(test)]
pub(super) fn glyphs_parse() -> bool {
    let color = Color::rgb(0x20, 0x60, 0xe0);
    (0..=STEPS).all(|n| {
        [Step::Pull(n), Step::Spin(n.min(STEPS - 1))]
            .into_iter()
            .all(|step| glyph(true, color, step).is_some() && glyph(false, color, step).is_some())
    })
}

fn rgb(color: Color) -> (String, f64) {
    (
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
        color.a as f64 / 255.0,
    )
}

/// UIKit's activity indicator: eight spokes. Pulled, they appear one by one
/// around the dial; spinning, a bright spoke goes round with a fading tail.
fn spokes(color: Color, step: Step) -> String {
    let (rgb, alpha) = rgb(color);
    let c = GLYPH / 2.0;
    let mut lines = String::new();
    for i in 0..8u32 {
        let opacity = match step {
            Step::Pull(n) => {
                if i * STEPS < n * 8 {
                    alpha
                } else {
                    0.0
                }
            }
            Step::Spin(n) => {
                let head = n * 8 / STEPS;
                let behind = (head + 8 - i) % 8;
                alpha * (1.0 - behind as f64 / 8.0 * 0.8)
            }
        };
        if opacity <= 0.0 {
            continue;
        }
        let a = (i as f64 * 45.0 - 90.0).to_radians();
        let (x0, y0) = (c + 5.0 * a.cos(), c + 5.0 * a.sin());
        let (x1, y1) = (c + 10.5 * a.cos(), c + 10.5 * a.sin());
        lines.push_str(&format!(
            "<line x1=\"{x0:.3}\" y1=\"{y0:.3}\" x2=\"{x1:.3}\" y2=\"{y1:.3}\" \
             stroke=\"{rgb}\" stroke-opacity=\"{opacity:.3}\" stroke-width=\"2.5\" \
             stroke-linecap=\"round\"/>"
        ));
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{GLYPH}\" height=\"{GLYPH}\" \
         viewBox=\"0 0 {GLYPH} {GLYPH}\">{lines}</svg>"
    )
}

/// Material's circular indicator: pulled, an arc that grows to three
/// quarters of the circle (turning a little); spinning, that arc going
/// round.
fn arc(color: Color, step: Step) -> String {
    let (rgb, alpha) = rgb(color);
    let c = GLYPH / 2.0;
    let r = c - 3.0;
    let (start, sweep) = match step {
        Step::Pull(n) => {
            let t = n as f64 / STEPS as f64;
            (t * 90.0, (t * 270.0).max(1.0))
        }
        Step::Spin(n) => (n as f64 * 360.0 / STEPS as f64, 270.0),
    };
    let point = |deg: f64| {
        let a = (deg - 90.0).to_radians();
        (c + r * a.cos(), c + r * a.sin())
    };
    let (x0, y0) = point(start);
    let (x1, y1) = point(start + sweep);
    let large = if sweep > 180.0 { 1 } else { 0 };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{GLYPH}\" height=\"{GLYPH}\" \
         viewBox=\"0 0 {GLYPH} {GLYPH}\">\
         <path d=\"M {x0:.3} {y0:.3} A {r} {r} 0 {large} 1 {x1:.3} {y1:.3}\" fill=\"none\" \
         stroke=\"{rgb}\" stroke-opacity=\"{alpha:.3}\" stroke-width=\"3\" \
         stroke-linecap=\"round\"/></svg>"
    )
}

/// The pointer handler: follows a pull the touch recognizer hands over and
/// refreshes past the threshold.
pub(super) struct Puller<S> {
    pub(super) access: Rc<dyn Fn(&mut S) -> &mut RefreshState>,
    pub(super) on_refresh: Rc<dyn Fn(&mut S)>,
    pub(super) motion: Motion,
}

impl<S> Puller<S> {
    pub(super) fn pointer(&self, ev: &PointerEvent, s: &mut S, now: Duration) {
        match ev.kind {
            PointerEventKind::Down if ev.button == Some(PointerButton::Left) => {
                let st = (self.access)(s);
                if !st.refreshing {
                    st.drag = Some((ev.window_position.y, false));
                }
            }
            PointerEventKind::Move => {
                let pulling = crate::core::touch::pulling();
                let st = (self.access)(s);
                let Some((origin, engaged)) = st.drag.as_mut() else {
                    return;
                };
                let y = ev.window_position.y;
                if !*engaged {
                    // Only a finger the recognizer handed over pulls (a
                    // mouse drag selects or does nothing, as before).
                    if !pulling {
                        return;
                    }
                    *engaged = true;
                    *origin = y;
                    crate::core::claim_press();
                }
                let to = rubber(y - *origin);
                st.pull.drag_to(to, now);
            }
            PointerEventKind::Up => {
                let st = (self.access)(s);
                let Some((_, engaged)) = st.drag.take() else {
                    return;
                };
                if !engaged {
                    return;
                }
                st.motion = self.motion;
                if st.pull.value(now) >= REFRESH_THRESHOLD {
                    st.refreshing = true;
                    st.pull.release(REST, self.motion, now);
                    (self.on_refresh)(s);
                } else {
                    st.pull.release(0.0, self.motion, now);
                }
            }
            _ => {}
        }
    }
}
