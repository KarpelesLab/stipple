//! A modal bottom sheet: a panel that slides up from the bottom edge over a
//! scrim, as wide as a phone's screen, with rounded top corners and a grab
//! handle. It rests on detents (its content's height, half the window, all
//! of it), follows a drag on its handle (or, on a touch screen, on its
//! content scrolled to the top) and settles on the nearest detent, or goes
//! away when dragged or flung down; a tap on the scrim, Back and Escape
//! close it too. On a wide window it shows as a centered dialog instead.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::anim::{Animated, Motion};
use crate::core::{
    Align, Anchor, Axis, BackHandler, BoxStyle, Cx, Effect, Element, OverlaySpec, PointerEventKind,
    Role, Transition,
};
use crate::geometry::{Insets, Transform};
use crate::platform::PointerButton;

use super::navigation::NavLook;

/// A height a [`bottom_sheet`] rests at.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum SheetDetent {
    /// As tall as its content, up to the tallest a sheet gets (scrolling
    /// inside beyond that).
    Fit,
    /// Half the window's height (a `UISheetPresentationController`'s
    /// `.medium()`).
    Half,
    /// The tallest a sheet gets: the window's height less a strip at the top
    /// where the page behind still shows (`.large()`).
    Full,
    /// This many logical pixels, up to the tallest a sheet gets.
    Height(f64),
}

impl SheetDetent {
    /// The height in logical pixels, in a window `window` tall where a
    /// sheet gets `max` tall; `fit` is the content's height, if known.
    fn px(self, window: f64, max: f64, fit: Option<f64>) -> Option<f64> {
        match self {
            Self::Fit => fit.map(|h| h.min(max)),
            Self::Half => Some((window * 0.5).min(max)),
            Self::Full => Some(max),
            Self::Height(h) => Some(h.clamp(0.0, max)),
        }
    }
}

/// How far below the window's top a sheet stops (logical pixels).
const TOP_GAP: f64 = 48.0;
/// From this window width (logical pixels) a sheet shows as a centered
/// dialog, unless [`BottomSheetView::adaptive`] says otherwise.
pub const SHEET_WIDE: f64 = 640.0;
/// The widest a sheet gets on a window narrower than [`SHEET_WIDE`] (Material's
/// bottom sheet maximum).
const MAX_WIDTH: f64 = 640.0;
/// How far a mouse or a finger on the handle moves before it drags.
const ENGAGE: f64 = 3.0;
/// How far ahead (seconds) a release's velocity projects where it lands.
const PROJECT: f64 = 0.2;

/// A drag of the sheet in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    /// Where the pointer was (window y) when the drag engaged, or pressed.
    origin: f64,
    /// The sheet's height then.
    start: f64,
    /// The press landed on the handle (or title): a mouse drags from there.
    header: bool,
    /// It moved enough to drag.
    engaged: bool,
}

/// A [`bottom_sheet`]'s state, kept in the app state: whether it's open, the
/// detent it rests on, and a drag in progress.
#[derive(Debug, Default)]
pub struct BottomSheet {
    open: bool,
    detent: usize,
    /// The sheet's height while a drag holds it or it settles after one;
    /// at rest the view uses the detent's.
    height: Animated<f64>,
    drag: Option<Drag>,
    /// The content's height, measured when the sheet was pressed while
    /// resting on [`SheetDetent::Fit`].
    fit: Option<f64>,
}

impl BottomSheet {
    /// A closed sheet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open it on its first detent.
    pub fn open(&mut self) {
        self.open_at(0);
    }

    /// Open it on detent `index` of the sheet's
    /// [`detents`](BottomSheetView::detents).
    pub fn open_at(&mut self, index: usize) {
        self.open = true;
        self.detent = index;
        self.drag = None;
        self.height = Animated::new(0.0);
    }

    /// Close it: it slides away.
    pub fn dismiss(&mut self) {
        self.open = false;
        self.drag = None;
    }

    /// Whether it's open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The index of the detent it rests on (or last rested on, while it's
    /// dragged).
    pub fn detent(&self) -> usize {
        self.detent
    }

    /// Whether a drag holds it.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some_and(|d| d.engaged)
    }
}

/// What closing the sheet runs: its state's `dismiss`, then the app's
/// `on_dismiss`.
pub(super) type Close<S> = Rc<dyn Fn(&mut S)>;
type OnDismiss<S> = Rc<RefCell<dyn FnMut(&mut S)>>;
pub(super) type Access<S> = Rc<dyn Fn(&mut S) -> &mut BottomSheet>;

/// A modal bottom sheet over a scrim, showing the content added with
/// [`child`](BottomSheetView::child) or [`children`](BottomSheetView::children)
/// while `sheet` [`is_open`](BottomSheet::is_open); declare it with
/// [`open`](BottomSheetView::open) on each build, as a [`dialog`](super::dialog).
///
/// It slides up from the bottom edge (on the theme's `dialog` motion, its
/// scrim fading in) to its first detent ([`SheetDetent::Fit`] unless
/// [`detents`](BottomSheetView::detents) says otherwise), as wide as the
/// window up to 640 px. Dragging its handle, or on a touch screen pulling its
/// content down while that's scrolled to the top, moves it with the pointer;
/// let go, it settles on the detent the release heads for (on the theme's
/// `sheet` motion), or slides away if that's below the lowest. A tap on the
/// scrim, Back (the button, the gesture, a gamepad's B) and Escape close it,
/// each running [`on_dismiss`](BottomSheetView::on_dismiss) after
/// [`BottomSheet::dismiss`]. Its content scrolls beyond the tallest detent.
///
/// Focus stays inside it while it's up, and assistive technologies see a
/// [`Role::Dialog`] named by its [`title`](BottomSheetView::title). On a
/// window at least [`SHEET_WIDE`] wide it shows as a centered dialog instead, with
/// no handle or detents ([`adaptive`](BottomSheetView::adaptive) turns that
/// off).
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{BottomSheet, SheetDetent, bottom_sheet};
///
/// #[derive(Default)]
/// struct App { result: BottomSheet }
///
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     bottom_sheet(cx, &state.result, |s| &mut s.result)
///         .title("QR code")
///         .detents(vec![SheetDetent::Fit, SheetDetent::Full])
///         .children(vec![
///             label(cx, "https://example.com"),
///             button(cx, "Open").on_tap(cx, |s| s.result.dismiss()),
///         ])
///         .open(cx);
///     button(cx, "Show result").on_tap(cx, |s| s.result.open())
/// }
/// # let theme = Theme::light();
/// # let mut app = App::default();
/// # app.result.open();
/// # let _ = view(&app, &mut Cx::new(&theme));
/// ```
pub fn bottom_sheet<'a, S: 'static>(
    cx: &mut Cx<S>,
    sheet: &'a BottomSheet,
    access: impl Fn(&mut S) -> &mut BottomSheet + 'static,
) -> BottomSheetView<'a, S> {
    let _ = cx;
    BottomSheetView {
        sheet,
        access: Rc::new(access),
        id: String::new(),
        title: None,
        content: Vec::new(),
        detents: vec![SheetDetent::Fit],
        handle: true,
        adaptive: true,
        bottom_inset: 0.0,
        on_dismiss: None,
    }
}

/// A bottom sheet being built: see [`bottom_sheet`]. It shows once
/// [`open`](Self::open) declares it (and its state is open).
#[must_use = "a sheet shows only once `.open(cx)` declares it"]
pub struct BottomSheetView<'a, S> {
    sheet: &'a BottomSheet,
    access: Access<S>,
    id: String,
    title: Option<String>,
    content: Vec<Element>,
    detents: Vec<SheetDetent>,
    handle: bool,
    adaptive: bool,
    bottom_inset: f64,
    on_dismiss: Option<OnDismiss<S>>,
}

impl<S> std::fmt::Debug for BottomSheetView<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BottomSheetView")
            .field("sheet", &self.sheet)
            .field("id", &self.id)
            .field("title", &self.title)
            .field("detents", &self.detents)
            .field("handle", &self.handle)
            .field("adaptive", &self.adaptive)
            .field("bottom_inset", &self.bottom_inset)
            .finish_non_exhaustive()
    }
}

impl<S: 'static> BottomSheetView<'_, S> {
    /// Tells this sheet's animations from another's in the same window.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// A title over the content, under the handle (dragging it drags the
    /// sheet), and the sheet's accessible name.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Add `child` to the content.
    pub fn child(mut self, child: Element) -> Self {
        self.content.push(child);
        self
    }

    /// Add `children` to the content.
    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.content.extend(children);
        self
    }

    /// The heights it rests at, lowest first ([`SheetDetent::Fit`] alone by
    /// default). [`BottomSheet::open_at`] picks one by index.
    pub fn detents(mut self, detents: Vec<SheetDetent>) -> Self {
        if !detents.is_empty() {
            self.detents = detents;
        }
        self
    }

    /// Show the grab handle at the top (`true` by default).
    pub fn handle(mut self, handle: bool) -> Self {
        self.handle = handle;
        self
    }

    /// Show as a centered dialog on a window at least [`SHEET_WIDE`] wide (`true`
    /// by default).
    pub fn adaptive(mut self, adaptive: bool) -> Self {
        self.adaptive = adaptive;
        self
    }

    /// Pad the content `inset` logical pixels above the bottom edge: the
    /// home indicator's area (until the window's safe area is reported,
    /// [#38](https://github.com/KarpelesLab/stipple/issues/38)).
    pub fn bottom_inset(mut self, inset: f64) -> Self {
        self.bottom_inset = inset.max(0.0);
        self
    }

    /// Run `on_dismiss` after the sheet closes itself (a drag down, the
    /// scrim, Back, Escape).
    pub fn on_dismiss(mut self, on_dismiss: impl FnMut(&mut S) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(RefCell::new(on_dismiss)));
        self
    }

    /// Declare the sheet as an overlay on this build, if its state is open.
    pub fn open(self, cx: &mut Cx<S>) {
        let sheet = self.sheet;
        if !sheet.open {
            return;
        }
        let theme = super::theme_of(cx);
        let p = theme.palette;
        let m = theme.motion;
        let win = cx.window_size();
        let close: Close<S> = {
            let access = self.access.clone();
            let on_dismiss = self.on_dismiss.clone();
            Rc::new(move |s: &mut S| {
                access(s).dismiss();
                if let Some(f) = &on_dismiss {
                    (f.borrow_mut())(s);
                }
            })
        };
        let dismiss = {
            let close = close.clone();
            cx.register(move |s| close(s))
        };
        {
            let close = close.clone();
            cx.on_back(BackHandler::new(move |s| close(s)).escape(true));
        }
        let label = self.title.clone().unwrap_or_else(|| "Sheet".into());
        let key = ("stipple-sheet", self.id.clone());

        if self.adaptive && win.width >= SHEET_WIDE {
            // A centered dialog: the title, then the content scrolling.
            let mut children = Vec::with_capacity(2);
            if let Some(title) = self.title {
                children.push(Element::text(title, theme.typography.heading, p.text));
            }
            let body = super::scroll(cx)
                .child(Element::stack(Axis::Vertical, self.content).gap(theme.spacing.md));
            children.push(body.shrink(1.0));
            let content = super::panel_frame(&theme, children)
                .width((win.width - 48.0).clamp(0.0, 560.0))
                .max_height((win.height * 0.85).max(0.0))
                .role(Role::Dialog)
                .a11y_label(label)
                .key(key)
                .enter(super::motion::dialog_effect(&m))
                .exit(super::motion::dialog_exit(&m));
            cx.overlay(OverlaySpec {
                content,
                anchor: Anchor::Center,
                modal: true,
                dismiss: Some(dismiss),
            });
            return;
        }

        let ios = NavLook::of(&theme) == NavLook::Ios;
        let max = (win.height - TOP_GAP).max(0.0);
        let px: Vec<Option<f64>> = self
            .detents
            .iter()
            .map(|d| d.px(win.height, max, sheet.fit))
            .collect();
        let detent = sheet.detent.min(px.len() - 1);
        let now = cx.frame_time();
        let moving = sheet.height.is_dragging() || sheet.height.is_animating(now);
        let height = if moving {
            Some(cx.animate(&sheet.height))
        } else {
            px[detent]
        };
        // Below the lowest detent the sheet keeps that height and slides
        // down instead of shrinking.
        let lowest = px.iter().flatten().copied().fold(f64::INFINITY, f64::min);
        let lowest = if lowest.is_finite() { lowest } else { 0.0 };
        let (laid, dy) = match height {
            Some(h) => (Some(h.max(lowest)), (lowest - h).max(0.0)),
            None => (None, 0.0),
        };

        // The handle and the title: where a mouse drags the sheet from.
        let mut header = Vec::with_capacity(2);
        let mut header_h = 0.0;
        if self.handle {
            let (w, h, pad) = if ios {
                (36.0, 5.0, 5.0)
            } else {
                (32.0, 4.0, 22.0)
            };
            header.push(
                Element::stack(
                    Axis::Horizontal,
                    vec![
                        Element::boxed(BoxStyle {
                            fill: Some(p.text_muted.with_alpha(if ios { 0x66 } else { 0x99 })),
                            radius: h / 2.0,
                            border: None,
                        })
                        .width(w)
                        .height(h),
                    ],
                )
                .padding(Insets::symmetric(0.0, pad))
                .align(Align::Center, Align::Center),
            );
            header_h += h + 2.0 * pad;
        } else {
            header_h += theme.spacing.md;
            header.push(super::spacer().height(theme.spacing.md));
        }
        if let Some(title) = self.title {
            let size = if ios { 17.0 } else { 22.0 };
            header.push(
                Element::stack(
                    Axis::Horizontal,
                    vec![Element::text(title, size, p.text).role(Role::Text)],
                )
                .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.sm))
                .align(
                    if ios { Align::Center } else { Align::Start },
                    Align::Center,
                ),
            );
            header_h += size * 1.4 + 2.0 * theme.spacing.sm;
        }
        let body = super::scroll(cx).child(
            Element::stack(Axis::Vertical, self.content)
                .gap(theme.spacing.md)
                .padding(Insets::new(
                    theme.spacing.sm,
                    theme.spacing.lg,
                    theme.spacing.lg + self.bottom_inset,
                    theme.spacing.lg,
                )),
        );
        let mut children = header;
        children.push(body.grow(1.0).shrink(1.0));
        let radius = if ios { 12.0 } else { 28.0 };
        let mut el = Element::stack(Axis::Vertical, children)
            .fill(if ios {
                p.surface
            } else {
                p.surface.mix(p.primary, 0.05)
            })
            .radius(radius)
            .width(win.width.min(MAX_WIDTH));
        el = match laid {
            Some(h) => el.height(h),
            None => el.max_height(max),
        };
        if dy > 0.0 {
            el = el.transform(Transform::IDENTITY.translate(0.0, dy));
        }
        let drag = Dragger {
            access: self.access.clone(),
            close,
            detents: self.detents.clone(),
            window: win.height,
            max,
            header: header_h,
            motion: m.sheet,
        };
        let mut el = el.on_pointer(cx, move |ev, s| drag.pointer(ev, s, crate::anim::now()));
        if let Some(id) = el.pointer {
            cx.pull_down(id);
        }
        // It slides up from below the window, and back down.
        let travel = laid.or(sheet.fit).unwrap_or(max) + dy;
        el = el
            .role(Role::Dialog)
            .a11y_label(label)
            .key(key)
            .enter(Effect::slide(0.0, travel).transition(Transition(m.dialog)))
            .exit(Effect::slide(0.0, travel).transition(Transition(m.dialog_exit)));
        cx.overlay(OverlaySpec {
            content: el,
            anchor: Anchor::Aligned {
                horizontal: Align::Center,
                vertical: Align::End,
                margin: 0.0,
            },
            modal: true,
            dismiss: Some(dismiss),
        });
    }
}

/// The sheet's pointer handler: drags it and settles it.
pub(super) struct Dragger<S> {
    pub(super) access: Access<S>,
    pub(super) close: Close<S>,
    pub(super) detents: Vec<SheetDetent>,
    pub(super) window: f64,
    pub(super) max: f64,
    /// The height of the handle and title, from the sheet's top.
    pub(super) header: f64,
    pub(super) motion: Motion,
}

impl<S> Dragger<S> {
    pub(super) fn pointer(&self, ev: &crate::core::PointerEvent, s: &mut S, now: Duration) {
        match ev.kind {
            PointerEventKind::Down if ev.button == Some(PointerButton::Left) => {
                let st = (self.access)(s);
                let moving = st.height.is_dragging() || st.height.is_animating(now);
                let start = if moving {
                    st.height.value(now)
                } else {
                    ev.size.height
                };
                if !moving && self.detents.get(st.detent) == Some(&SheetDetent::Fit) {
                    st.fit = Some(ev.size.height);
                }
                st.drag = Some(Drag {
                    origin: ev.window_position.y,
                    start,
                    header: ev.position.y < self.header,
                    engaged: false,
                });
            }
            PointerEventKind::Move => {
                let pulling = crate::core::touch::pulling();
                let st = (self.access)(s);
                let Some(d) = st.drag.as_mut() else {
                    return;
                };
                let y = ev.window_position.y;
                if !d.engaged {
                    // A mouse drags from the handle; a finger also from the
                    // content pulled down at its top.
                    if !(d.header || pulling) || (!pulling && (y - d.origin).abs() < ENGAGE) {
                        return;
                    }
                    d.engaged = true;
                    d.origin = y;
                    crate::core::claim_press();
                }
                let raw = d.start - (y - d.origin);
                // Past the tallest it resists, as a rubber band.
                let h = if raw > self.max {
                    self.max + (raw - self.max) * 0.2
                } else {
                    raw.max(0.0)
                };
                st.height.drag_to(h, now);
            }
            PointerEventKind::Up => {
                let st = (self.access)(s);
                let Some(d) = st.drag.take() else {
                    return;
                };
                if !d.engaged {
                    return;
                }
                match self.settle(st, now) {
                    Some((index, h)) => {
                        st.detent = index;
                        st.height.release(h, self.motion, now);
                    }
                    None => (self.close)(s),
                }
            }
            _ => {}
        }
    }

    /// Where a released sheet goes: the detent (index and height) nearest
    /// where its velocity heads, or `None` to close.
    fn settle(&self, st: &BottomSheet, now: Duration) -> Option<(usize, f64)> {
        let h = st.height.value(now);
        let heading = h + st.height.velocity(now) * PROJECT;
        let mut best: Option<(usize, f64)> = None;
        let mut lowest = f64::INFINITY;
        for (i, d) in self.detents.iter().enumerate() {
            let Some(px) = d.px(self.window, self.max, st.fit) else {
                continue;
            };
            lowest = lowest.min(px);
            if best.is_none_or(|(_, b)| (px - heading).abs() < (b - heading).abs()) {
                best = Some((i, px));
            }
        }
        // Closing counts as a detent at half the lowest one's height below
        // it: heading under half the lowest closes.
        match best {
            Some(_) if heading < lowest * 0.5 => None,
            best => best,
        }
    }
}
