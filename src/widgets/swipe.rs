//! Swipe actions on list rows: drag a row sideways and it follows the
//! finger, showing the actions behind it (a colored button each, with a
//! label and an icon). Let go past half the buttons and the row stays open
//! on them; short of that it springs back; far enough (a *full swipe*) and
//! the side's first action runs at once. A destructive action's row leaves
//! with an exit animation, and the rows below close the gap.
//!
//! A drag only becomes a swipe once it is mostly sideways, decided where it
//! leaves the touch slop, the same way the touch recognizer picks between a
//! vertical list and a sideways strip: a vertical drag scrolls the list as
//! before. With a mouse, a sideways drag swipes too, and a trackpad's
//! two-finger sideways scroll opens and shuts a row. Every action is in the
//! row's context menu as well (a right-click, a long press, the Menu key or
//! Shift+F10), so it's reachable from the keyboard and by assistive
//! technologies without the gesture.
//!
//! [`swipe_row`] builds a row; its state is a [`SwipeState`] the app keeps,
//! one per list (one row is open at a time). It works in a
//! [`virtual_list`](super::virtual_list)'s row builder and in a plain
//! [`scroll`](super::scroll) of rows.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Duration;

use crate::anim::{Animated, Motion};
use crate::core::touch::SLOP;
use crate::core::{
    Align, Axis, Cx, Effect, Element, PointerEvent, PointerEventKind, Role, Transition, WheelEvent,
};
use crate::core::{Menu, MenuItem};
use crate::geometry::{Insets, Point, Transform};
use crate::platform::{PointerButton, ScrollPhase, ScrollUnit};
use crate::render::{Color, ImageSource};

/// How wide each revealed action button is (logical pixels), as in iOS's
/// table views.
pub const SWIPE_ACTION_WIDTH: f64 = 74.0;
/// How far across the row (a fraction of its width) a full swipe has to go
/// to run the side's first action.
const FULL: f64 = 0.6;
/// How much of the finger's travel past the buttons the row follows when a
/// full swipe is off.
const RESIST: f64 = 0.25;
/// How far ahead (seconds) a release's velocity projects where it lands.
const PROJECT: f64 = 0.15;
/// How far (logical pixels) a sideways wheel or trackpad scroll without a
/// gesture phase travels before it opens or shuts a row; one wheel notch
/// counts as [`LINE`].
const STEP: f64 = 30.0;
/// A wheel notch, in logical pixels.
const LINE: f64 = 40.0;
/// After a phaseless sideways scroll opens or shuts a row, how long (seconds)
/// the rest of that scroll is ignored, so one flick takes one step.
const STEP_PAUSE: f64 = 0.35;

/// One action behind a swipeable row: a label, optionally an icon and a
/// color, and what it runs.
pub struct SwipeAction<S> {
    label: String,
    icon: Option<ImageSource>,
    icon_only: bool,
    color: Option<Color>,
    destructive: bool,
    run: Rc<dyn Fn(&mut S)>,
}

impl<S> Clone for SwipeAction<S> {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            icon: self.icon.clone(),
            icon_only: self.icon_only,
            color: self.color,
            destructive: self.destructive,
            run: self.run.clone(),
        }
    }
}

impl<S> std::fmt::Debug for SwipeAction<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwipeAction")
            .field("label", &self.label)
            .field("color", &self.color)
            .field("destructive", &self.destructive)
            .finish_non_exhaustive()
    }
}

impl<S> SwipeAction<S> {
    /// An action named `label` that runs `run` on the app state.
    pub fn new(label: impl Into<String>, run: impl Fn(&mut S) + 'static) -> Self {
        Self {
            label: label.into(),
            icon: None,
            icon_only: false,
            color: None,
            destructive: false,
            run: Rc::new(run),
        }
    }

    /// Show `icon` (a [`Pixmap`](crate::render::Pixmap) or an
    /// [`Svg`](crate::render::Svg), 22 px, drawn in white) above the label.
    pub fn icon(mut self, icon: impl Into<ImageSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Show the [`icon`](Self::icon) alone, without the label under it (a
    /// trash can, a flag). The label still names the button for assistive
    /// technologies, and the action in the row's context menu.
    pub fn icon_only(mut self) -> Self {
        self.icon_only = true;
        self
    }

    /// The button's color (the theme's danger color for a destructive
    /// action, its primary color for a side's first action and a muted gray
    /// for the others, by default). Its label and icon are white.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Mark it destructive (a delete): red by default, and its row leaves
    /// with an exit animation when it runs, the rows below closing the gap.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// The action's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// The actions behind a swipeable row: the *leading* ones show when it's
/// dragged toward the trailing edge (right, in a left-to-right layout), the
/// *trailing* ones when it's dragged toward the leading edge. Each side's
/// first action is its primary one: it sits at the outer edge and a full
/// swipe runs it.
pub struct SwipeActions<S> {
    leading: Vec<SwipeAction<S>>,
    trailing: Vec<SwipeAction<S>>,
    full_swipe: bool,
}

impl<S> Clone for SwipeActions<S> {
    fn clone(&self) -> Self {
        Self {
            leading: self.leading.clone(),
            trailing: self.trailing.clone(),
            full_swipe: self.full_swipe,
        }
    }
}

impl<S> std::fmt::Debug for SwipeActions<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwipeActions")
            .field("leading", &self.leading)
            .field("trailing", &self.trailing)
            .field("full_swipe", &self.full_swipe)
            .finish()
    }
}

impl<S> Default for SwipeActions<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> SwipeActions<S> {
    /// No actions yet; a full swipe runs the primary action.
    pub fn new() -> Self {
        Self {
            leading: Vec::new(),
            trailing: Vec::new(),
            full_swipe: true,
        }
    }

    /// Add an action on the leading side (the first added is the primary).
    pub fn leading(mut self, action: SwipeAction<S>) -> Self {
        self.leading.push(action);
        self
    }

    /// Add an action on the trailing side (the first added is the primary).
    pub fn trailing(mut self, action: SwipeAction<S>) -> Self {
        self.trailing.push(action);
        self
    }

    /// Whether a swipe across most of the row runs the side's primary action
    /// (`true` by default). Off, the row resists being dragged past its
    /// buttons.
    pub fn full_swipe(mut self, full_swipe: bool) -> Self {
        self.full_swipe = full_swipe;
        self
    }

    /// Whether there are no actions on either side.
    pub fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_empty()
    }

    /// The actions on the side an offset of `sign` reveals (positive: the
    /// leading ones).
    fn side(&self, sign: f64) -> &[SwipeAction<S>] {
        if sign > 0.0 {
            &self.leading
        } else {
            &self.trailing
        }
    }

    /// How far a side's buttons reach, on a row `width` wide.
    fn reveal(&self, sign: f64, width: f64) -> f64 {
        let n = self.side(sign).len() as f64;
        (n * SWIPE_ACTION_WIDTH).min(width * 0.8)
    }

    /// Where the row sits for a finger at `raw` (leading-positive logical
    /// pixels from rest) on a row `width` wide: no further than the buttons
    /// reach (resisting past them) unless a full swipe may run, and not at
    /// all toward a side with no actions.
    fn shape(&self, raw: f64, width: f64) -> f64 {
        self.shape_with(raw, width, self.full_swipe)
    }

    /// [`shape`](Self::shape), resisting past the buttons even when a full
    /// swipe is on (a trackpad, which has no full swipe).
    fn shape_resisting(&self, raw: f64, width: f64) -> f64 {
        self.shape_with(raw, width, false)
    }

    fn shape_with(&self, raw: f64, width: f64, full: bool) -> f64 {
        let sign = if raw >= 0.0 { 1.0 } else { -1.0 };
        if self.side(sign).is_empty() {
            return 0.0;
        }
        let a = raw.abs();
        let reveal = self.reveal(sign, width);
        let at = if full {
            a.min(width)
        } else if a > reveal {
            reveal + (a - reveal) * RESIST
        } else {
            a
        };
        sign * at
    }

    /// Whether a row at `offset` is far enough over for a full swipe.
    fn armed(&self, offset: f64, width: f64) -> bool {
        self.full_swipe && !self.side(offset).is_empty() && offset.abs() >= width * FULL
    }
}

/// A drag of a row in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    row: u64,
    /// Where the pointer was (window coordinates) when it went down, or
    /// where the drag crossed the slop once it swipes.
    origin: Point,
    /// The row's offset then.
    start: f64,
    /// The row's width.
    width: f64,
    /// `None` until it leaves the slop; then whether it swipes (`false`: it
    /// was a vertical drag, a scroll).
    swiping: Option<bool>,
    /// Past the full-swipe line.
    armed: bool,
}

/// A sideways wheel or trackpad scroll over a row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Wheel {
    /// The row a trackpad gesture moves (between its start and its end).
    row: Option<u64>,
    /// Where the fingers would put the row, before shaping.
    raw: f64,
    /// Phaseless travel toward the next step.
    acc: f64,
    /// When the last phaseless step happened (seconds).
    stepped: f64,
}

/// The swipe state of a list's rows, kept in the app state: which row is
/// open (at most one), a drag in progress, and a row springing shut. Rows
/// are told apart by the key given to [`swipe_row`].
#[derive(Debug, Default)]
pub struct SwipeState {
    /// The row a drag holds or that is open (or settling shut).
    row: Option<u64>,
    /// That row's offset: logical pixels, positive revealing the leading
    /// actions.
    offset: Animated<f64>,
    drag: Option<Drag>,
    /// Another row springing shut while this one moves.
    closing: Option<(u64, Animated<f64>)>,
    wheel: Wheel,
}

pub(super) fn hash_key(key: impl Hash) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

impl SwipeState {
    /// No row open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the row keyed `key` is open on its actions (or heading
    /// there).
    pub fn is_open(&self, key: impl Hash) -> bool {
        self.row == Some(hash_key(key)) && !self.is_swiping() && self.offset.target().abs() > 0.5
    }

    /// Whether a finger, the mouse or a trackpad is swiping a row.
    pub fn is_swiping(&self) -> bool {
        self.drag.is_some_and(|d| d.swiping == Some(true)) || self.wheel.row.is_some()
    }

    /// Shut any open row at once.
    pub fn close(&mut self) {
        self.row = None;
        self.offset.set(0.0);
        self.drag = None;
        self.closing = None;
        self.wheel = Wheel::default();
    }

    /// Where row `key` is at `now`.
    #[cfg(test)]
    pub(super) fn offset_of(&self, key: u64, now: Duration) -> f64 {
        if self.row == Some(key) {
            self.offset.value(now)
        } else {
            match &self.closing {
                Some((k, a)) if *k == key => a.value(now),
                _ => 0.0,
            }
        }
    }

    /// Make `key` the row that moves, sending the one that was open (if
    /// another) springing shut.
    fn take_row(&mut self, key: u64, motion: Motion, now: Duration) {
        if self.row == Some(key) {
            return;
        }
        if let Some(other) = self.row.take() {
            let mut shut = std::mem::take(&mut self.offset);
            if shut.value(now).abs() > 0.5 || shut.is_animating(now) {
                shut.release(0.0, motion, now);
                self.closing = Some((other, shut));
            }
        }
        if self.closing.as_ref().is_some_and(|(k, _)| *k == key) {
            // It was springing shut: carry on from where it is.
            self.offset = self.closing.take().map(|(_, a)| a).unwrap_or_default();
        } else {
            self.offset = Animated::new(0.0);
        }
        self.row = Some(key);
    }

    /// Shut the open row with `motion`.
    fn shut(&mut self, motion: Motion, now: Duration) {
        self.drag = None;
        self.wheel.row = None;
        self.offset.release(0.0, motion, now);
    }
}

/// What a list hands the swipeable row it's building: the padding and fill
/// it would give the row, which go on the row's sliding content instead, so
/// the actions behind span the whole row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RowChrome {
    pub(crate) padding: Insets,
    pub(crate) fill: Option<Color>,
}

/// A row's actions as menu entries: a label and what it runs.
pub(crate) type MenuEntries<S> = Vec<(String, Rc<dyn Fn(&mut S)>)>;

thread_local! {
    /// The chrome a list offers the row it's building.
    static CHROME: Cell<Option<RowChrome>> = const { Cell::new(None) };
    /// Whether a swipeable row took it.
    static TAKEN: Cell<bool> = const { Cell::new(false) };
    /// The last swipeable row's menu entries (a `MenuEntries<S>`), for a list
    /// to put on itself for its cursor row.
    static ENTRIES: RefCell<Option<Rc<dyn Any>>> = const { RefCell::new(None) };
}

/// Offer `chrome` to the row about to be built (see [`RowChrome`]).
pub(crate) fn offer_chrome(chrome: RowChrome) {
    CHROME.with(|c| c.set(Some(chrome)));
    TAKEN.with(|t| t.set(false));
    ENTRIES.with(|e| e.borrow_mut().take());
}

/// After building a row: whether a swipeable row took the chrome offered
/// (the list then leaves its own padding and fill off), and that row's menu
/// entries.
pub(crate) fn chrome_taken<S: 'static>() -> (bool, Option<MenuEntries<S>>) {
    CHROME.with(|c| c.set(None));
    let taken = TAKEN.with(|t| t.replace(false));
    let entries = ENTRIES
        .with(|e| e.borrow_mut().take())
        .and_then(|e| e.downcast_ref::<MenuEntries<S>>().cloned());
    (taken, entries)
}

/// The context menu for `entries`, with the refresh action of a
/// [`pull_to_refresh`](super::pull_to_refresh) around it last.
pub(crate) fn entries_menu<S: 'static>(entries: &MenuEntries<S>) -> Option<Menu<S>> {
    let refresh = super::refresh::scoped::<S>();
    if entries.is_empty() && refresh.is_none() {
        return None;
    }
    let mut menu = Menu::new("");
    for (label, run) in entries {
        let run = run.clone();
        menu = menu.item(MenuItem::new(menu_label(label), move |s: &mut S| run(s)));
    }
    if let Some((label, run)) = refresh {
        if !entries.is_empty() {
            menu = menu.separator();
        }
        menu = menu.item(MenuItem::new(menu_label(&label), move |s: &mut S| run(s)));
    }
    Some(menu)
}

/// A label as a menu item shows it: `&` doubled, so it isn't a mnemonic.
fn menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

/// A swipeable list row being built: see [`swipe_row`]. Its content comes in
/// [`child`](Self::child).
#[must_use = "a swipe row shows only once `.child(content)` builds it"]
pub struct SwipeRow {
    key: u64,
    /// The actions behind, laid out for the row's current offset.
    behind: Option<Behind>,
    /// The row's corner radius.
    radius: f64,
    /// How far the content is moved (logical pixels, positive right).
    dx: f64,
    pointer: crate::core::PointerId,
    /// The tap that shuts the row while it's open.
    shut: Option<crate::core::ActionId>,
    context: Option<crate::core::ContextId>,
    chrome: Option<RowChrome>,
    surface: Color,
    motion: Motion,
    /// The exit when the row is the one swiped (its action may delete it).
    exit: Option<Effect>,
}

impl std::fmt::Debug for SwipeRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwipeRow")
            .field("key", &self.key)
            .field("dx", &self.dx)
            .finish_non_exhaustive()
    }
}

/// A list row with `actions` behind it, swiped sideways to reveal them; its
/// content comes in [`child`](SwipeRow::child).
///
/// `state` is the list's [`SwipeState`] and `access` reaches it inside the
/// app state; `key` tells this row from the others across frames (an id
/// that stays with the item, not its index, so a deleted row's neighbors
/// keep theirs).
///
/// Dragged sideways (by a finger, or the mouse), the row follows; let go
/// past half its buttons it opens on them, and a tap on one runs it; short
/// of that it springs back on the theme's `swipe` motion. Dragged across
/// most of the row, the side's first action runs at once (unless
/// [`full_swipe`](SwipeActions::full_swipe) is off). A destructive action's
/// row leaves sliding and fading, and the rows below glide up. A tap on an
/// open row shuts it, and pressing another row shuts it too. A trackpad's
/// two-finger sideways scroll opens and shuts it. The actions are the row's
/// context menu too (a right-click, a long press, the Menu key), for the
/// keyboard and assistive technologies.
///
/// In a [`virtual_list`](super::virtual_list) the list's row padding and
/// selection tint go on the row's content, and the list's own context menu
/// (the Menu key while it has focus) holds its cursor row's actions.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{SwipeAction, SwipeActions, SwipeState, swipe_row};
///
/// struct App { notes: Vec<(u32, String)>, swipe: SwipeState }
///
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     let rows: Vec<Element> = state.notes.iter().map(|(id, text)| {
///         let id = *id;
///         let actions = SwipeActions::new()
///             .trailing(
///                 SwipeAction::new("Delete", move |s: &mut App| s.notes.retain(|n| n.0 != id))
///                     .destructive(),
///             );
///         swipe_row(cx, &state.swipe, |s| &mut s.swipe, id, actions)
///             .child(label(cx, text).padding(Insets::uniform(12.0)))
///     }).collect();
///     scroll(cx).children(rows)
/// }
/// # let theme = Theme::light();
/// # let app = App { notes: vec![(1, "Milk".into())], swipe: SwipeState::new() };
/// # let _ = view(&app, &mut Cx::new(&theme));
/// ```
pub fn swipe_row<S: 'static>(
    cx: &mut Cx<S>,
    state: &SwipeState,
    access: impl Fn(&mut S) -> &mut SwipeState + 'static,
    key: impl Hash,
    actions: SwipeActions<S>,
) -> SwipeRow {
    let theme = super::theme_of(cx);
    let p = theme.palette;
    let motion = theme.motion.swipe;
    let key = hash_key(key);
    let access: Access<S> = Rc::new(access);
    let chrome = CHROME.with(|c| c.take());
    if chrome.is_some() {
        TAKEN.with(|t| t.set(true));
    }
    let offset = if state.row == Some(key) {
        cx.animate(&state.offset)
    } else {
        match &state.closing {
            Some((k, a)) if *k == key => cx.animate(a),
            _ => 0.0,
        }
    };
    let armed = state.drag.is_some_and(|d| d.row == key && d.armed);
    let dir = if theme.is_rtl() { -1.0 } else { 1.0 };

    // The buttons of the side the offset reveals, as wide as it.
    let behind = (offset.abs() > 0.5).then(|| {
        let sign = offset.signum();
        let side = actions.side(sign);
        let shown = if armed { side.len().min(1) } else { side.len() };
        let mut buttons: Vec<Element> = side[..shown]
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let color = action_color(a, i == 0, &theme);
                let mut inner = Vec::with_capacity(2);
                if let Some(icon) = &a.icon {
                    inner.push(Element::image(icon.clone()).width(22.0).height(22.0));
                }
                if !(a.icon_only && a.icon.is_some()) {
                    inner.push(Element::text(
                        a.label.clone(),
                        theme.typography.caption,
                        Color::WHITE,
                    ));
                }
                let run = a.run.clone();
                let access = access.clone();
                let destructive = a.destructive;
                Element::stack(Axis::Vertical, inner)
                    .gap(theme.spacing.xs)
                    .align(Align::Center, Align::Center)
                    .fill(color)
                    .grow(1.0)
                    .clip()
                    .role(Role::Button)
                    .a11y_label(a.label.clone())
                    .on_tap(cx, move |s| {
                        let st = access(s);
                        if destructive {
                            st.close();
                        } else {
                            st.shut(motion, crate::anim::now());
                        }
                        run(s);
                    })
            })
            .collect();
        // The primary action sits at the outer edge.
        if sign < 0.0 {
            buttons.reverse();
        }
        Behind {
            buttons,
            width: offset.abs(),
            fill: side.first().map(|first| action_color(first, true, &theme)),
            leading: sign > 0.0,
        }
    });

    let swiper = Rc::new(Swiper {
        access: access.clone(),
        key,
        actions: actions.clone(),
        dir,
        motion,
    });
    let wheel = swiper.clone();
    let pointer = cx.register_pointer(None, move |ev, s| swiper.pointer(ev, s, crate::anim::now()));
    let pointer = cx.register_wheel(Some(pointer), move |ev, s| {
        wheel.wheel(ev, s, crate::anim::now())
    });
    cx.wheel_sideways(pointer);

    let open = state.row == Some(key) && offset.abs() > 0.5 && !state.is_swiping();
    let shut = open.then(|| {
        let access = access.clone();
        cx.register(move |s| access(s).shut(motion, crate::anim::now()))
    });

    let mut entries: MenuEntries<S> = Vec::new();
    for a in actions.leading.iter().chain(&actions.trailing) {
        let run = a.run.clone();
        let access = access.clone();
        entries.push((
            a.label.clone(),
            Rc::new(move |s: &mut S| {
                access(s).close();
                run(s);
            }),
        ));
    }
    let context = entries_menu(&entries).map(|menu| cx.register_context_menu(menu));
    if chrome.is_some() {
        ENTRIES.with(|e| *e.borrow_mut() = Some(Rc::new(entries) as Rc<dyn Any>));
    }

    let exit = (state.row == Some(key)).then(|| {
        let toward = if offset.abs() > 0.5 {
            offset.signum()
        } else {
            -1.0
        };
        Effect::fade()
            .and(Effect::slide(toward * dir * 120.0, 0.0))
            .transition(Transition(motion))
    });
    SwipeRow {
        key,
        behind,
        radius: 0.0,
        dx: offset * dir,
        pointer,
        shut,
        context,
        chrome,
        surface: p.surface,
        motion,
        exit,
    }
}

/// An action button's color: its own, else red for a destructive one, the
/// primary color for a side's first and a muted gray for the others.
fn action_color<S>(a: &SwipeAction<S>, first: bool, theme: &crate::style::Theme) -> Color {
    let p = theme.palette;
    a.color.unwrap_or(if a.destructive {
        p.danger
    } else if first {
        p.primary
    } else {
        p.text_muted
    })
}

/// The buttons a swiped row shows, on the side it was swiped from.
struct Behind {
    buttons: Vec<Element>,
    /// How wide they are together: how far the row moved.
    width: f64,
    /// The color under them (the first action's, as a full swipe stretches
    /// it).
    fill: Option<Color>,
    /// On the leading side (the row moved toward the trailing edge).
    leading: bool,
}

impl Behind {
    /// The buttons beside the moved row, their corners rounded by `radius`
    /// as the row's are.
    fn view(self, radius: f64) -> Element {
        let buttons = self
            .buttons
            .into_iter()
            .map(|b| if radius > 0.0 { b.radius(radius) } else { b })
            .collect();
        let mut area = Element::stack(Axis::Horizontal, buttons)
            .width(self.width)
            .align(Align::Start, Align::Stretch)
            .radius(radius);
        if let Some(fill) = self.fill {
            area = area.fill(fill);
        }
        let gap = super::spacer();
        let children = if self.leading {
            vec![area, gap]
        } else {
            vec![gap, area]
        };
        Element::stack(Axis::Horizontal, children).align(Align::Start, Align::Stretch)
    }
}

impl SwipeRow {
    /// Round the row's corners by `radius` (an inset card list): the content
    /// as it moves, and the action buttons beside it, follow them.
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius.max(0.0);
        self
    }

    /// Build the row around `content`.
    pub fn child(self, content: Element) -> Element {
        let chrome = self.chrome.unwrap_or_default();
        let radius = self.radius;
        let mut front = Element::stack(Axis::Horizontal, vec![content.grow(1.0)])
            .align(Align::Start, Align::Center)
            .padding(chrome.padding);
        if let Some(fill) = chrome.fill {
            front = front.fill(fill);
        }
        if self.dx.abs() > 0.5 {
            // Opaque while it's moved, so the actions show only beside it.
            front = Element::stack(Axis::Horizontal, vec![front.grow(1.0)])
                .align(Align::Start, Align::Stretch)
                .fill(self.surface)
                .radius(radius)
                .transform(Transform::IDENTITY.translate(self.dx, 0.0));
        }
        front.action = self.shut;
        let mut layers = Vec::with_capacity(2);
        layers.extend(self.behind.map(|b| b.view(radius)));
        layers.push(front);
        let mut el = Element::layers(layers)
            .grow(1.0)
            .align_self(Align::Stretch)
            .clip()
            .key(("stipple-swipe", self.key))
            .animate_position(Transition(self.motion));
        if let Some(exit) = self.exit {
            el = el.exit(exit);
        }
        el.pointer = Some(self.pointer);
        el.context = self.context;
        el
    }
}

type Access<S> = Rc<dyn Fn(&mut S) -> &mut SwipeState>;

/// A row's pointer and wheel handler: swipes it and settles it.
pub(super) struct Swiper<S> {
    pub(super) access: Access<S>,
    pub(super) key: u64,
    pub(super) actions: SwipeActions<S>,
    /// 1 in a left-to-right layout, -1 in a right-to-left one: which way a
    /// rightward move takes the offset.
    pub(super) dir: f64,
    pub(super) motion: Motion,
}

/// What a release does.
enum Settle<S> {
    Rest,
    Run(Rc<dyn Fn(&mut S)>),
}

impl<S> Swiper<S> {
    pub(super) fn pointer(&self, ev: &PointerEvent, s: &mut S, now: Duration) {
        match ev.kind {
            PointerEventKind::Down if ev.button == Some(PointerButton::Left) => {
                let st = (self.access)(s);
                st.take_row(self.key, self.motion, now);
                let start = st.offset.value(now);
                st.drag = Some(Drag {
                    row: self.key,
                    origin: ev.window_position,
                    start,
                    width: ev.size.width.max(1.0),
                    swiping: None,
                    armed: false,
                });
            }
            PointerEventKind::Move => {
                let st = (self.access)(s);
                let Some(d) = st.drag.as_mut().filter(|d| d.row == self.key) else {
                    return;
                };
                let at = ev.window_position;
                let (dx, dy) = (at.x - d.origin.x, at.y - d.origin.y);
                if d.swiping.is_none() {
                    if dx.abs() <= SLOP && dy.abs() <= SLOP {
                        return;
                    }
                    // Mostly sideways, and not a scroll the touch recognizer
                    // already took (it decides on the same move, first).
                    let sideways = dx.abs() > dy.abs()
                        && !crate::core::touch::scrolling()
                        && !crate::core::touch::pulling();
                    d.swiping = Some(sideways);
                    if !sideways {
                        return;
                    }
                    // From where it crossed the slop, so the row doesn't jump.
                    d.origin.x += SLOP.copysign(dx);
                    crate::core::claim_press();
                }
                if d.swiping != Some(true) {
                    return;
                }
                let raw = d.start + (at.x - d.origin.x) * self.dir;
                let to = self.actions.shape(raw, d.width);
                d.armed = self.actions.armed(to, d.width);
                st.offset.drag_to(to, now);
            }
            PointerEventKind::Up => {
                let st = (self.access)(s);
                let Some(d) = st.drag.take().filter(|d| d.row == self.key) else {
                    return;
                };
                if d.swiping != Some(true) {
                    return;
                }
                if let Settle::Run(run) = self.settle(st, d, now) {
                    run(s);
                }
            }
            _ => {}
        }
    }

    /// Where a released row goes: open on a side's buttons, shut, or (past
    /// the full-swipe line) shut or gone as its primary action runs.
    fn settle(&self, st: &mut SwipeState, d: Drag, now: Duration) -> Settle<S> {
        let at = st.offset.value(now);
        if d.armed && self.actions.armed(at, d.width) {
            let action = &self.actions.side(at)[0];
            if action.destructive {
                // Its row goes; if it stays, it's back at rest.
                st.close();
            } else {
                st.offset.release(0.0, self.motion, now);
            }
            return Settle::Run(action.run.clone());
        }
        let heading = at + st.offset.velocity(now) * PROJECT;
        let sign = at.signum();
        let reveal = self.actions.reveal(sign, d.width);
        let to = if at.abs() > 0.5
            && heading.signum() == sign
            && reveal > 0.0
            && heading.abs() >= reveal / 2.0
        {
            sign * reveal
        } else {
            0.0
        };
        st.offset.release(to, self.motion, now);
        Settle::Rest
    }

    pub(super) fn wheel(&self, ev: &WheelEvent, s: &mut S, now: Duration) {
        let st = (self.access)(s);
        let px = match ev.unit {
            ScrollUnit::Lines => ev.dx * LINE,
            _ => ev.dx,
        };
        // Scrolling right (positive) moves the content left: in a
        // left-to-right layout, toward the trailing actions.
        let delta = -px * self.dir;
        // A wheel event has no row width: the buttons' reach is measured as
        // on a wide row, and there's no full swipe.
        let width = f64::INFINITY;
        match ev.phase {
            ScrollPhase::Started | ScrollPhase::Moved => {
                if st.wheel.row != Some(self.key) {
                    if px == 0.0 {
                        return;
                    }
                    st.take_row(self.key, self.motion, now);
                    st.wheel.row = Some(self.key);
                    st.wheel.raw = st.offset.value(now);
                }
                st.wheel.raw += delta;
                let to = self.actions.shape_resisting(st.wheel.raw, width);
                st.offset.drag_to(to, now);
            }
            ScrollPhase::Ended => {
                if st.wheel.row != Some(self.key) {
                    return;
                }
                st.wheel.row = None;
                let drag = Drag {
                    row: self.key,
                    origin: Point::new(0.0, 0.0),
                    start: 0.0,
                    width,
                    swiping: Some(true),
                    armed: false,
                };
                let _ = self.settle(st, drag, now);
            }
            ScrollPhase::Momentum => {}
            _ => {
                // A wheel notch, or a scroll with no gesture phase: it steps,
                // once per flick.
                let t = now.as_secs_f64();
                if st.wheel.stepped > 0.0 && t - st.wheel.stepped < STEP_PAUSE {
                    return;
                }
                st.wheel.acc += delta;
                if st.wheel.acc.abs() < STEP {
                    return;
                }
                let sign = st.wheel.acc.signum();
                st.wheel.acc = 0.0;
                st.wheel.stepped = t;
                st.take_row(self.key, self.motion, now);
                let at = st.offset.target();
                let to = if at.abs() > 0.5 && at.signum() != sign {
                    0.0
                } else {
                    sign * self.actions.reveal(sign, width)
                };
                st.offset.release(to, self.motion, now);
            }
        }
    }
}
