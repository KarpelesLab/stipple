//! A stack of pages a phone app drills into (a list, then an item's details,
//! then its settings): [`NavStack`] keeps the pages in the app state,
//! [`nav_stack`] shows the top one with the one below kept alive under it,
//! animates pushes and pops in the platform's way, and lets Back, the iOS
//! edge swipe and Android's predictive back gesture pop it, the page
//! following the finger. [`app_bar`] is the bar along the top of each page:
//! a back button, the title and trailing actions.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crate::anim::{Animated, Motion};
use crate::core::{Align, Axis, BackHandler, BoxStyle, Cx, Element, Role};
use crate::geometry::{Insets, Transform};
use crate::platform::BackGesture;
use crate::render::{Color, FontWeight, ImageSource};
use crate::style::{MotionTokens, Theme};

use super::navigation::{NavLook, tinted};

/// How a [`nav_stack`] moves between two pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PageTransition {
    /// `UINavigationController`'s: the new page slides in from the trailing
    /// edge over the old one, which shifts a third of the way the other way
    /// and dims. The back swipe drags the page back off with the finger.
    Slide,
    /// Material's shared X axis: the pages cross-fade as they shift 30 px
    /// sideways. The predictive back gesture shrinks the page into a card
    /// that follows the finger, then fades it away.
    SharedAxis,
    /// A short cross-fade, the new page sliding 16 px in from the trailing
    /// side as it fades in over the old: desktops and the web.
    Fade,
}

impl PageTransition {
    /// The transition for `theme`: [`Slide`](Self::Slide) under
    /// [`Theme::native`] on iOS, [`SharedAxis`](Self::SharedAxis) under the
    /// Material motion ([`MotionTokens::material`]: the Material themes, and
    /// `Theme::native()` on Android), [`Fade`](Self::Fade) otherwise.
    pub fn of(theme: &Theme) -> Self {
        if theme.native && cfg!(target_os = "ios") {
            Self::Slide
        } else if theme.motion == MotionTokens::material() {
            Self::SharedAxis
        } else {
            Self::Fade
        }
    }

    /// How far the upper page shows (1 all of it, 0 none) at back gesture
    /// `progress`: the slide follows the finger, Material's preview only
    /// shrinks the page into a card (the first fifth of the way).
    fn gesture_shown(self, progress: f64) -> f64 {
        let p = progress.clamp(0.0, 1.0);
        match self {
            Self::SharedAxis => 1.0 - PREVIEW * p,
            Self::Slide | Self::Fade => 1.0 - p,
        }
    }
}

/// How far Material's predictive back preview goes (in "shown" terms)
/// before the gesture commits.
const PREVIEW: f64 = 0.2;

/// The pages of a [`nav_stack`], bottom to top, kept in the app state: the
/// root first, the page showing last. [`push`](Self::push) and
/// [`pop`](Self::pop) animate on the theme's `page` motion (from whatever
/// is moving, so a pop mid-push turns round smoothly); under reduced motion
/// they switch at once.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{NavStack, app_bar, nav_stack};
///
/// #[derive(Clone, Copy)]
/// enum Page { History, Details(usize) }
///
/// struct App { nav: NavStack<Page> }
///
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     nav_stack(cx, &state.nav, |s| &mut s.nav, |cx, page| match *page {
///         Page::History => column(vec![
///             app_bar(cx, "History").view(cx),
///             button(cx, "Scan #1").on_tap(cx, |s| s.nav.push(Page::Details(1))),
///         ]),
///         Page::Details(n) => column(vec![
///             app_bar(cx, format!("Scan #{n}")).on_back(|s| { s.nav.pop(); }).view(cx),
///             label(cx, "Decoded text…"),
///         ]),
///     })
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App { nav: NavStack::new(Page::History) }, &mut Cx::new(&theme));
/// ```
#[derive(Debug)]
pub struct NavStack<P> {
    pages: Vec<P>,
    /// The page just popped, drawn over the new top while it leaves.
    leaving: Option<P>,
    /// How much of the upper page of the transition shows: 1 all of it (at
    /// rest after a push), 0 none (at rest after a pop).
    shown: Animated<f64>,
    /// The back gesture holds `shown`.
    gesture: bool,
    /// The transition under way is the back gesture's (Material's preview
    /// look carries through its commit).
    predictive: bool,
    /// The transition to use instead of [`PageTransition::of`] the theme.
    transition: Option<PageTransition>,
    /// The `page` motion of the theme the stack was last shown under, for
    /// pushes and pops from handlers (which have no theme).
    motion: Cell<Motion>,
}

impl<P> NavStack<P> {
    /// A stack holding just `root`.
    pub fn new(root: P) -> Self {
        Self {
            pages: vec![root],
            leaving: None,
            shown: Animated::new(1.0),
            gesture: false,
            predictive: false,
            transition: None,
            motion: Cell::new(MotionTokens::standard().page),
        }
    }

    /// Move between pages with `transition` instead of the one
    /// [`PageTransition::of`] the theme picks.
    pub fn transition(mut self, transition: PageTransition) -> Self {
        self.transition = Some(transition);
        self
    }

    /// Move between pages with `transition` from now on, or with the one
    /// [`PageTransition::of`] the theme picks (`None`).
    pub fn set_transition(&mut self, transition: Option<PageTransition>) {
        self.transition = transition;
    }

    /// How many pages are stacked (1: just the root).
    pub fn depth(&self) -> usize {
        self.pages.len()
    }

    /// Whether there is a page under the top one to go back to.
    pub fn can_pop(&self) -> bool {
        self.pages.len() > 1
    }

    /// The page showing.
    pub fn top(&self) -> &P {
        self.pages.last().expect("a nav stack always has its root")
    }

    /// The pages, root first.
    pub fn pages(&self) -> &[P] {
        &self.pages
    }

    /// Show `page` over the current one, sliding (or fading) it in.
    pub fn push(&mut self, page: P) {
        self.push_at(page, crate::anim::now());
    }

    /// Go back to the page under the top one, sliding the top one away.
    /// Returns whether there was one (the root stays).
    pub fn pop(&mut self) -> bool {
        self.pop_at(crate::anim::now())
    }

    /// Go back to the root, the top page leaving over it (the ones between
    /// go at once). Returns whether there was anything above the root.
    pub fn pop_to_root(&mut self) -> bool {
        if self.pages.len() <= 2 {
            return self.pop();
        }
        let top = self.pages.pop();
        self.pages.truncate(1);
        self.pages.extend(top);
        self.pop()
    }

    pub(crate) fn push_at(&mut self, page: P, now: Duration) {
        self.pages.push(page);
        self.leaving = None;
        self.gesture = false;
        self.predictive = false;
        self.shown.set(0.0);
        self.shown.animate_to(1.0, self.motion.get(), now);
    }

    pub(crate) fn pop_at(&mut self, now: Duration) -> bool {
        if self.pages.len() <= 1 {
            return false;
        }
        // A pop while a push still runs turns it round from where it is; a
        // pop while an earlier one leaves starts the new one from the top.
        if self.popping() {
            self.shown.set(1.0);
        }
        self.leaving = self.pages.pop();
        self.predictive = self.gesture;
        self.gesture = false;
        self.shown.animate_to(0.0, self.motion.get(), now);
        true
    }

    /// The back gesture moved on: follow it with the top page, or settle.
    pub(crate) fn back_progress_at(
        &mut self,
        gesture: BackGesture,
        transition: PageTransition,
        now: Duration,
    ) {
        match gesture {
            BackGesture::Started { .. } if self.can_pop() => {
                if self.popping() {
                    self.leaving = None;
                    self.shown.set(1.0);
                }
                self.gesture = true;
                self.predictive = true;
                self.shown.drag_to(transition.gesture_shown(0.0), now);
            }
            BackGesture::Progressed { progress } if self.gesture => {
                self.shown
                    .drag_to(transition.gesture_shown(f64::from(progress)), now);
            }
            BackGesture::Cancelled if self.gesture => {
                self.gesture = false;
                self.shown.release(1.0, self.motion.get(), now);
            }
            _ => {}
        }
    }

    /// Whether a page is leaving (or has left) over the top one.
    fn popping(&self) -> bool {
        self.leaving.is_some() && self.shown.target() == 0.0
    }
}

/// One page's look in a transition.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    /// Sideways shift toward the trailing edge (logical pixels; mirrored
    /// right to left).
    dx: f64,
    scale: f64,
    opacity: f64,
    /// The black laid over it, 0 to 1.
    dim: f64,
}

const REST: Pose = Pose {
    dx: 0.0,
    scale: 1.0,
    opacity: 1.0,
    dim: 0.0,
};

/// The upper and lower pages' looks with `t` of the upper showing, on a
/// stack `width` wide; `predictive` for the back gesture's.
fn poses(transition: PageTransition, t: f64, width: f64, predictive: bool) -> (Pose, Pose) {
    let ramp = |v: f64| v.clamp(0.0, 1.0);
    match transition {
        PageTransition::Slide => (
            Pose {
                dx: (1.0 - t) * width,
                ..REST
            },
            Pose {
                dx: -0.3 * width * t,
                dim: 0.1 * t,
                ..REST
            },
        ),
        PageTransition::SharedAxis if predictive => {
            // Shrinks into a card over the first fifth, then slides on and
            // fades as the pop commits.
            let gone = 1.0 - t;
            let card = ramp(gone / PREVIEW);
            let away = ramp((gone - PREVIEW) / (1.0 - PREVIEW));
            (
                Pose {
                    dx: card * (width / 20.0 - 8.0).max(0.0) + away * width * 0.1,
                    scale: 1.0 - 0.1 * card,
                    opacity: 1.0 - away,
                    dim: 0.0,
                },
                Pose {
                    dim: 0.2 * t,
                    ..REST
                },
            )
        }
        PageTransition::SharedAxis => (
            Pose {
                dx: 30.0 * (1.0 - t),
                opacity: ramp((t - 0.35) / 0.65),
                ..REST
            },
            Pose {
                dx: -30.0 * t,
                opacity: ramp(1.0 - t / 0.35),
                ..REST
            },
        ),
        PageTransition::Fade => (
            Pose {
                dx: 16.0 * (1.0 - t),
                opacity: t,
                ..REST
            },
            REST,
        ),
    }
}

/// A stack of pages: the top page of `nav` fills it, built by `page`, and the
/// pages under it are built too (in order, every frame) but kept off stage
/// (see [`Element::offstage`]): not drawn, hit, focused or read by assistive
/// technologies, yet keeping their scroll offsets and the text being typed,
/// so going back finds the page as it was left.
///
/// [`NavStack::push`] and [`NavStack::pop`] move between pages the
/// platform's way ([`PageTransition::of`] the theme, on its `page` motion),
/// both pages on stage while they move. While there's a page to go back to,
/// the stack takes Back for the app (see [`Cx::on_back`]): Android's Back,
/// a gamepad's B, and the back gestures, whose progress the top page
/// follows (iOS's edge swipe drags it off, Android 14's predictive back
/// shrinks it into a card) until it lifts and the page settles, gone or
/// back in place. Put the back button in the page's [`app_bar`].
pub fn nav_stack<S: 'static, P: 'static>(
    cx: &mut Cx<S>,
    nav: &NavStack<P>,
    access: impl Fn(&mut S) -> &mut NavStack<P> + 'static,
    mut page: impl FnMut(&mut Cx<S>, &P) -> Element,
) -> Element {
    let theme = super::theme_of(cx);
    let transition = nav.transition.unwrap_or_else(|| PageTransition::of(&theme));
    nav.motion.set(theme.motion.page);
    let access = Rc::new(access);
    // Declared before the pages, so a sheet a page opens closes first.
    if nav.can_pop() {
        let pop = access.clone();
        let progress = access.clone();
        cx.on_back(
            BackHandler::new(move |s| {
                pop(s).pop();
            })
            .on_progress(move |s, g| {
                progress(s).back_progress_at(g, transition, crate::anim::now());
            }),
        );
    }
    let t = cx.animate(&nav.shown).clamp(0.0, 1.0);
    let now = cx.frame_time();
    let moving = nav.shown.is_dragging() || nav.shown.is_animating(now);
    let n = nav.pages.len();
    let popping = nav.popping();
    // The pair of pages moving, by index (the leaving page is index `n`).
    let pair = match (moving, popping) {
        (true, true) => Some((n - 1, n)),
        (true, false) if n >= 2 => Some((n - 2, n - 1)),
        _ => None,
    };
    let width = cx.window_size().width.max(1.0);
    let (upper, lower) = poses(transition, t, width, nav.predictive);
    let rtl = theme.is_rtl();
    let bg = theme.palette.background;
    // The leaving page is built only while it shows, last (so the pages
    // under it keep their ids).
    let leaving = nav.leaving.as_ref().filter(|_| popping && pair.is_some());
    let mut layers = Vec::with_capacity(n + 1);
    for (i, p) in nav.pages.iter().chain(leaving).enumerate() {
        let pose = match pair {
            Some((l, _)) if l == i => Some(lower),
            Some((_, u)) if u == i => Some(upper),
            _ => None,
        };
        let content = page(cx, p);
        let shown = pose.is_some() || (pair.is_none() && i == n - 1);
        layers.push(page_frame(content, i, pose.unwrap_or(REST), rtl, bg).offstage(!shown));
    }
    Element::layers(layers).grow(1.0).shrink(1.0).clip()
}

/// Page `i`'s frame: its content over the theme's background, with a dimming
/// layer over it, drawn at `pose`. The same structure in every pose, so the
/// page keeps its elements' identities as it moves.
fn page_frame(content: Element, i: usize, pose: Pose, rtl: bool, bg: Color) -> Element {
    let dim = Element::boxed(BoxStyle {
        fill: (pose.dim > 0.0).then(|| Color::rgba(0, 0, 0, (pose.dim * 255.0).round() as u8)),
        ..BoxStyle::default()
    })
    .hit_through();
    let dx = if rtl { -pose.dx } else { pose.dx };
    Element::layers(vec![content, dim])
        .fill(bg)
        .key(("stipple-nav-page", i))
        .transform(Transform::IDENTITY.translate(dx, 0.0).scale(pose.scale))
        .opacity(pose.opacity as f32)
}

/// The back arrow of Material's top app bar (`arrow_back`).
const ARROW_BACK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/></svg>"#;
/// The chevron of a `UINavigationBar` back button.
const CHEVRON_BACK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 21"><path d="M10.5 1.5 1.5 10.5l9 9" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;

/// What the back button runs.
type OnBack<S> = Box<dyn FnMut(&mut S)>;

/// The bar along the top of a page: see [`app_bar`].
#[must_use = "an app bar shows once `.view(cx)` builds it"]
pub struct AppBar<S> {
    title: String,
    on_back: Option<OnBack<S>>,
    back_label: Option<String>,
    actions: Vec<Element>,
    look: Option<NavLook>,
    top_inset: f64,
}

impl<S> std::fmt::Debug for AppBar<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppBar")
            .field("title", &self.title)
            .field("back", &self.on_back.is_some())
            .field("back_label", &self.back_label)
            .field("actions", &self.actions.len())
            .field("look", &self.look)
            .field("top_inset", &self.top_inset)
            .finish()
    }
}

/// The bar along the top of a page: a back button (with
/// [`on_back`](AppBar::on_back)), the `title`, and trailing
/// [`actions`](AppBar::actions). It draws as [`NavLook::of`] the theme picks:
/// Material 3's small top app bar (64 px, a back arrow, the title after it)
/// everywhere it draws itself, and `UINavigationBar`'s proportions (44 px, a
/// chevron and the previous page's title in the accent, the title centered
/// over a hairline) under [`Theme::native`] on iOS. Build it with
/// [`view`](AppBar::view).
///
/// Make it part of each page in a [`nav_stack`], so it moves with its page.
pub fn app_bar<S>(cx: &Cx<S>, title: impl Into<String>) -> AppBar<S> {
    let _ = cx;
    AppBar {
        title: title.into(),
        on_back: None,
        back_label: None,
        actions: Vec::new(),
        look: None,
        top_inset: 0.0,
    }
}

impl<S: 'static> AppBar<S> {
    /// Show a back button that runs `on_back` (pop the page:
    /// `|s| { s.nav.pop(); }`).
    pub fn on_back(mut self, on_back: impl FnMut(&mut S) + 'static) -> Self {
        self.on_back = Some(Box::new(on_back));
        self
    }

    /// The text beside the iOS chevron: the previous page's title, or
    /// "Back". Material's back button is the arrow alone (its accessible
    /// name is still "Back").
    pub fn back_label(mut self, label: impl Into<String>) -> Self {
        self.back_label = Some(label.into());
        self
    }

    /// Put `actions` (icon buttons, a menu button) at the bar's end.
    pub fn actions(mut self, actions: Vec<Element>) -> Self {
        self.actions.extend(actions);
        self
    }

    /// Draw it as `look` instead of the one [`NavLook::of`] picks.
    pub fn look(mut self, look: NavLook) -> Self {
        self.look = Some(look);
        self
    }

    /// Extend the bar's background `inset` logical pixels above it: the
    /// status bar's height, where the bar's content must not go (until the
    /// window's safe area is reported,
    /// [#38](https://github.com/KarpelesLab/stipple/issues/38)).
    pub fn top_inset(mut self, inset: f64) -> Self {
        self.top_inset = inset.max(0.0);
        self
    }

    /// The bar's element.
    pub fn view(self, cx: &mut Cx<S>) -> Element {
        let theme = super::theme_of(cx);
        let look = self.look.unwrap_or_else(|| NavLook::of(&theme));
        let p = theme.palette;
        let rtl = theme.is_rtl();
        let ios = look == NavLook::Ios;
        let back = self.on_back.map(|on_back| {
            let (svg, size, ink) = if ios {
                (CHEVRON_BACK, (12.0, 21.0), p.primary)
            } else {
                (ARROW_BACK, (24.0, 24.0), p.text)
            };
            let icon = match tinted(svg, ink) {
                Some(doc) => Element::image(ImageSource::Svg(doc)),
                None => super::spacer(),
            }
            .width(size.0)
            .height(size.1);
            // The arrow points the way back: right in a right-to-left UI.
            let icon = if rtl {
                icon.transform(Transform::IDENTITY.scale_xy(-1.0, 1.0))
            } else {
                icon
            };
            let mut parts = vec![icon];
            if ios {
                let text = self.back_label.clone().unwrap_or_else(|| "Back".into());
                parts.push(Element::text(text, 17.0, p.primary));
            }
            let button = Element::stack(Axis::Horizontal, parts)
                .gap(6.0)
                .align(Align::Center, Align::Center)
                .radius(if ios { theme.radius } else { 24.0 })
                .padding(if ios {
                    Insets::symmetric(8.0, 6.0)
                } else {
                    Insets::uniform(12.0)
                });
            super::pressable(button, &theme, None)
                .on_tap(cx, on_back)
                .role(Role::Button)
                .a11y_label("Back")
        });
        let title_size = if ios { 17.0 } else { 22.0 };
        let title = Element::text(self.title, title_size, p.text)
            .weight(if ios {
                FontWeight::SEMI_BOLD
            } else {
                FontWeight::NORMAL
            })
            .role(Role::Text);
        let actions = Element::stack(Axis::Horizontal, self.actions)
            .gap(if ios { 8.0 } else { 0.0 })
            .align(Align::End, Align::Center);
        let bar = if ios {
            // The title centered across the whole bar, the back button and
            // the actions over its ends.
            let ends = Element::stack(
                Axis::Horizontal,
                vec![
                    back.unwrap_or_else(super::spacer),
                    super::spacer().grow(1.0),
                    actions,
                ],
            )
            .padding(Insets::symmetric(8.0, 0.0))
            .align(Align::Start, Align::Center);
            let centered = Element::stack(Axis::Horizontal, vec![title])
                .align(Align::Center, Align::Center)
                .hit_through();
            Element::layers(vec![centered, ends]).height(IOS_BAR)
        } else {
            let mut row = Vec::with_capacity(3);
            let lead = if back.is_some() { 4.0 } else { 16.0 };
            row.extend(back);
            row.push(title.grow(1.0).shrink(1.0));
            row.push(actions);
            Element::stack(Axis::Horizontal, row)
                .gap(4.0)
                .padding(Insets::new(0.0, 4.0, 0.0, lead))
                .align(Align::Start, Align::Center)
                .height(MATERIAL_BAR)
        };
        let fill = if ios {
            p.surface.mix(p.background, 0.5)
        } else {
            p.background
        };
        let mut parts = Vec::with_capacity(3);
        if self.top_inset > 0.0 {
            parts.push(super::spacer().height(self.top_inset));
        }
        parts.push(bar);
        if ios {
            parts.push(
                Element::boxed(BoxStyle {
                    fill: Some(p.border),
                    ..BoxStyle::default()
                })
                .height(0.5),
            );
        }
        Element::stack(Axis::Vertical, parts)
            .fill(fill)
            .role(Role::Toolbar)
    }
}

/// `UINavigationBar`'s height, without the status bar.
const IOS_BAR: f64 = 44.0;
/// Material 3's small top app bar.
const MATERIAL_BAR: f64 = 64.0;
