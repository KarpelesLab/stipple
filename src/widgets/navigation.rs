//! A phone app's top-level navigation: the bar along the bottom of the
//! window that switches between its sections (an icon over a label per
//! destination, the current one highlighted), the same items as a rail down
//! the side of a wide window, and a stack of pages that keeps every
//! destination alive while one shows.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::core::{
    Align, AnimProp, Axis, BoxStyle, Cx, Effect, Element, KeyInput, Role, Transition,
};
use crate::geometry::{Insets, Transform, Vec2};
use crate::render::{Color, FontWeight, ImageSource, Pixmap, Svg};
use crate::style::Theme;

/// A mark on a destination that something waits there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NavBadge {
    /// A small dot (something new).
    Dot,
    /// A count (unread messages); above 99 it shows "99+".
    Count(u32),
}

/// A destination's icon.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum NavIcon {
    /// A bitmap or an SVG, drawn in its own colors.
    Image(ImageSource),
    /// The source of an SVG drawn in `currentColor`, which the bar replaces
    /// with the item's ink (the accent when selected under
    /// [`NavLook::Ios`], muted otherwise): one icon serves both states and
    /// follows the theme. Each tinted copy is parsed once and kept.
    Template(&'static str),
}

impl NavIcon {
    /// An icon from SVG source using `currentColor` (see
    /// [`NavIcon::Template`]).
    pub fn template(svg: &'static str) -> Self {
        Self::Template(svg)
    }
}

impl From<ImageSource> for NavIcon {
    fn from(source: ImageSource) -> Self {
        Self::Image(source)
    }
}

impl From<Arc<Svg>> for NavIcon {
    fn from(svg: Arc<Svg>) -> Self {
        Self::Image(ImageSource::Svg(svg))
    }
}

impl From<&Arc<Svg>> for NavIcon {
    fn from(svg: &Arc<Svg>) -> Self {
        Self::Image(ImageSource::Svg(svg.clone()))
    }
}

impl From<Arc<Pixmap>> for NavIcon {
    fn from(pixmap: Arc<Pixmap>) -> Self {
        Self::Image(ImageSource::Bitmap(pixmap))
    }
}

/// One destination of a [`navigation_bar`] or [`navigation_rail`].
#[derive(Clone, Debug)]
pub struct NavItem {
    label: String,
    icon: NavIcon,
    selected_icon: Option<NavIcon>,
    badge: Option<NavBadge>,
}

impl NavItem {
    /// A destination labeled `label` with `icon`.
    pub fn new(label: impl Into<String>, icon: impl Into<NavIcon>) -> Self {
        Self {
            label: label.into(),
            icon: icon.into(),
            selected_icon: None,
            badge: None,
        }
    }

    /// The icon to show while this destination is selected (Material's filled
    /// variant of an outlined icon, say). Without one, the same icon shows.
    pub fn selected_icon(mut self, icon: impl Into<NavIcon>) -> Self {
        self.selected_icon = Some(icon.into());
        self
    }

    /// A badge over the icon, or none.
    pub fn badge(mut self, badge: Option<NavBadge>) -> Self {
        self.badge = badge;
        self
    }

    /// What assistive technologies read: the label, then the badge.
    fn spoken(&self) -> String {
        match self.badge {
            None => self.label.clone(),
            Some(NavBadge::Dot) => format!("{}, new", self.label),
            Some(NavBadge::Count(n)) => format!("{}, {n} new", self.label),
        }
    }
}

/// How a [`navigation_bar`] looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NavLook {
    /// Material 3's navigation bar: 80 px tall, a pill behind the selected
    /// icon, the label below. The self-drawn default everywhere.
    Material,
    /// `UITabBar`'s proportions: 49 px tall over a hairline, no pill, the
    /// selected icon and its small label tinted with the accent.
    Ios,
}

impl NavLook {
    /// The look for `theme`: [`NavLook::Ios`] under [`Theme::native`] on iOS,
    /// [`NavLook::Material`] otherwise.
    pub fn of(theme: &Theme) -> Self {
        if theme.native && cfg!(target_os = "ios") {
            Self::Ios
        } else {
            Self::Material
        }
    }
}

/// What picking an item runs, with its index.
type OnSelect<S> = Rc<dyn Fn(&mut S, usize)>;

/// A [`navigation_bar`] with its options: its look, the safe area under it,
/// and an id when a window shows more than one.
pub struct NavigationBar<S> {
    id: String,
    items: Vec<NavItem>,
    selected: usize,
    on_select: OnSelect<S>,
    look: Option<NavLook>,
    bottom_inset: f64,
}

impl<S> std::fmt::Debug for NavigationBar<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavigationBar")
            .field("id", &self.id)
            .field("items", &self.items)
            .field("selected", &self.selected)
            .field("look", &self.look)
            .field("bottom_inset", &self.bottom_inset)
            .finish_non_exhaustive()
    }
}

impl<S: 'static> NavigationBar<S> {
    /// A bar of `items` with `selected` highlighted, running `on_select` with
    /// an item's index when it's picked.
    pub fn new(
        items: Vec<NavItem>,
        selected: usize,
        on_select: impl Fn(&mut S, usize) + 'static,
    ) -> Self {
        Self {
            id: String::new(),
            items,
            selected,
            on_select: Rc::new(on_select),
            look: None,
            bottom_inset: 0.0,
        }
    }

    /// Tells this bar's animations from another's in the same window.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Draw it as `look` instead of the one [`NavLook::of`] picks.
    pub fn look(mut self, look: NavLook) -> Self {
        self.look = Some(look);
        self
    }

    /// Extend the bar's background `inset` logical pixels below its items:
    /// the part of the screen under the home indicator (or gesture bar),
    /// where the items must not go. Window safe-area insets aren't reported
    /// yet ([#38](https://github.com/KarpelesLab/stipple/issues/38)); this is
    /// where they will go.
    pub fn bottom_inset(mut self, inset: f64) -> Self {
        self.bottom_inset = inset.max(0.0);
        self
    }

    /// The bar's element.
    pub fn view(self, cx: &mut Cx<S>, theme: &Theme) -> Element {
        let look = self.look.unwrap_or_else(|| NavLook::of(theme));
        let n = self.items.len();
        let selected = self.selected.min(n.saturating_sub(1));
        let rtl = theme.is_rtl();
        let id: Rc<str> = self.id.into();
        let items: Vec<Element> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let on = self.on_select.clone();
                let el = match look {
                    NavLook::Material => {
                        material_item(theme, item, i == selected, (&id, i), PILL_W).height(HEIGHT)
                    }
                    NavLook::Ios => ios_item(theme, item, i == selected),
                };
                el.width(0.0)
                    .grow(1.0)
                    .on_tap(cx, move |s: &mut S| on(s, i))
                    .role(Role::Tab)
                    .selected(i == selected)
                    .a11y_label(item.spoken())
            })
            .collect();
        let p = &theme.palette;
        let row = Element::stack(Axis::Horizontal, items).align(Align::Start, Align::Stretch);
        let (fill, top) = match look {
            NavLook::Material => (container(theme), None),
            NavLook::Ios => (
                p.surface.mix(p.background, 0.5),
                Some(
                    Element::boxed(BoxStyle {
                        fill: Some(p.border),
                        ..BoxStyle::default()
                    })
                    .height(0.5),
                ),
            ),
        };
        let inset = (self.bottom_inset > 0.0).then(|| super::spacer().height(self.bottom_inset));
        let on_select = self.on_select;
        Element::stack(
            Axis::Vertical,
            top.into_iter().chain([row]).chain(inset).collect(),
        )
        .fill(fill)
        .on_key(cx, move |s: &mut S, k| {
            if let Some(to) = step(k, selected, n, rtl, false) {
                on_select(s, to);
            }
        })
        .role(Role::TabList)
    }
}

/// A navigation bar along the bottom of the window: one destination per
/// item, an icon over a label, `selected` highlighted. Tapping an item runs
/// `on_select` with its index; with the bar focused, ← → move the selection
/// (mirrored right to left) and Home/End go to the first/last.
///
/// It draws as [`NavLook::of`] the theme picks: Material 3's bar (a pill
/// behind the selected icon), or `UITabBar`'s proportions under
/// [`Theme::native`] on iOS. For another look, an id, or the safe area
/// under it (see [`NavigationBar::bottom_inset`]) build a [`NavigationBar`].
///
/// Assistive technologies see a [`Role::TabList`] of [`Role::Tab`]s, the
/// selected one selected, each named by its label and badge.
///
/// Show the destinations' pages with [`page_stack`], which keeps the hidden
/// ones alive; on a wide window, show the same items as a
/// [`navigation_rail`].
pub fn navigation_bar<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    items: Vec<NavItem>,
    selected: usize,
    on_select: impl Fn(&mut S, usize) + 'static,
) -> Element {
    NavigationBar::new(items, selected, on_select).view(cx, theme)
}

/// The items of a [`navigation_bar`] down the side of a wide window, as
/// Material 3's navigation rail: 80 px wide, the items from the top, the
/// selected icon on a pill. ↑ ↓ move the selection; otherwise it works as
/// the bar does. Pick between the two by width, say:
///
/// ```ignore
/// if cx.window_size().width >= 600.0 { /* a row: rail, pages */ } else { /* a column: pages, bar */ }
/// ```
pub fn navigation_rail<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    items: Vec<NavItem>,
    selected: usize,
    on_select: impl Fn(&mut S, usize) + 'static,
) -> Element {
    let n = items.len();
    let selected = selected.min(n.saturating_sub(1));
    let on_select: OnSelect<S> = Rc::new(on_select);
    let id: Rc<str> = "rail".into();
    let cells: Vec<Element> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let on = on_select.clone();
            material_item(theme, item, i == selected, (&id, i), RAIL_PILL_W)
                .padding(Insets::symmetric(0.0, 4.0))
                .on_tap(cx, move |s: &mut S| on(s, i))
                .role(Role::Tab)
                .selected(i == selected)
                .a11y_label(item.spoken())
        })
        .collect();
    Element::stack(Axis::Vertical, cells)
        .gap(12.0)
        .padding(Insets::new(44.0, 0.0, 12.0, 0.0))
        .width(80.0)
        .fill(container(theme))
        .on_key(cx, move |s: &mut S, k| {
            if let Some(to) = step(k, selected, n, false, true) {
                on_select(s, to);
            }
        })
        .role(Role::TabList)
}

/// The destinations' pages, all built (in order, every frame) but only
/// `shown` on stage: the others are laid out where it is, keeping their
/// scroll offsets and their place among the window's ids, but not drawn,
/// hit, focused or read by assistive technologies (see
/// [`Element::offstage`]). So a page left for another comes back as it was:
/// scrolled where it was, the text being typed still there.
///
/// `page` builds page `i` and is told whether it's the one showing, so a
/// page can pause what only matters on screen (a camera preview, a timer).
/// Each page fills the stack.
pub fn page_stack<S>(
    cx: &mut Cx<S>,
    count: usize,
    shown: usize,
    mut page: impl FnMut(&mut Cx<S>, usize, bool) -> Element,
) -> Element {
    let pages = (0..count)
        .map(|i| {
            let visible = i == shown;
            page(cx, i, visible).offstage(!visible)
        })
        .collect();
    Element::layers(pages).grow(1.0).shrink(1.0)
}

/// Material 3's indicator pill: 64 × 32 in the bar, 56 × 32 in the rail.
const PILL_W: f64 = 64.0;
const RAIL_PILL_W: f64 = 56.0;
const PILL_H: f64 = 32.0;
const ICON: f64 = 24.0;
const IOS_ICON: f64 = 25.0;
/// `UITabBar`'s height, without the safe area under it.
const IOS_HEIGHT: f64 = 49.0;

/// Material's surface container, behind the bar and the rail.
fn container(theme: &Theme) -> Color {
    let p = &theme.palette;
    p.background.mix(p.primary, 0.05)
}

/// The next selection for key `k` from `selected` of `n`, along the bar
/// (← →, mirrored when `rtl`) or the rail (↑ ↓).
fn step(k: &KeyInput, selected: usize, n: usize, rtl: bool, vertical: bool) -> Option<usize> {
    let next = (selected + 1 < n).then_some(selected + 1);
    let prev = selected.checked_sub(1);
    match k {
        KeyInput::Up if vertical => prev,
        KeyInput::Down if vertical => next,
        KeyInput::Left if !vertical => {
            if rtl {
                next
            } else {
                prev
            }
        }
        KeyInput::Right if !vertical => {
            if rtl {
                prev
            } else {
                next
            }
        }
        KeyInput::Home => (n > 0).then_some(0),
        KeyInput::End => n.checked_sub(1),
        _ => None,
    }
    .filter(|&to| to != selected)
}

/// One Material destination: the icon on its pill (when `selected`), the
/// label below. `key` is the bar's id and the item's index.
fn material_item(
    theme: &Theme,
    item: &NavItem,
    selected: bool,
    key: (&str, usize),
    pill_w: f64,
) -> Element {
    let p = &theme.palette;
    let state = Transition(theme.motion.state);
    let indicator = Transition(theme.motion.indicator);
    let ink = if selected { p.text } else { p.text_muted };
    let mut area = Vec::with_capacity(3);
    if selected {
        // Grows out of its middle as it's selected, fades as it's left.
        let grow = Effect::custom(
            0.0,
            Transform {
                scale: Vec2::new(0.4, 1.0),
                ..Transform::IDENTITY
            },
        )
        .transition(indicator);
        area.push(
            Element::boxed(BoxStyle {
                fill: Some(p.surface.mix(p.primary, 0.22)),
                radius: PILL_H / 2.0,
                border: None,
            })
            .width(pill_w)
            .height(PILL_H)
            .key(("stipple-nav-pill", key.0, key.1))
            .enter(grow)
            .exit(Effect::fade().transition(state)),
        );
    }
    area.push(icon(item, selected, ink, ICON));
    if let Some(badge) = item.badge {
        // The badge at the icon's top end (the icon spans the middle 24 px).
        let left = pill_w / 2.0 + ICON / 2.0;
        let (pad, mark) = match badge {
            NavBadge::Dot => (Insets::new(4.0, 0.0, 0.0, left - 4.0), dot(p.danger, 6.0)),
            NavBadge::Count(n) => (
                Insets::new(1.0, 0.0, 0.0, left - 10.0),
                count(p.danger, n, 11.0, 16.0),
            ),
        };
        area.push(
            Element::stack(Axis::Horizontal, vec![mark])
                .padding(pad)
                .align(Align::Start, Align::Start),
        );
    }
    let area = Element::layers(area)
        .width(pill_w)
        .height(PILL_H)
        .align(Align::Center, Align::Center)
        // The pill's parent keeps an identity, for its fade as it's left.
        .key(("stipple-nav-area", key.0, key.1));
    let label = Element::text(item.label.clone(), 12.0, ink)
        .weight(if selected {
            FontWeight::BOLD
        } else {
            FontWeight::MEDIUM
        })
        .transition_for(AnimProp::TextColor, state);
    Element::stack(Axis::Vertical, vec![area, label])
        .gap(4.0)
        .padding(Insets::new(12.0, 0.0, 16.0, 0.0))
        .align(Align::Start, Align::Center)
}

/// Material's bar is 80 tall, whatever its labels measure.
const HEIGHT: f64 = 80.0;

/// One `UITabBar` destination: the icon over a small label, both tinted
/// with the accent when `selected`.
fn ios_item(theme: &Theme, item: &NavItem, selected: bool) -> Element {
    let p = &theme.palette;
    let ink = if selected { p.primary } else { p.text_muted };
    let w = 64.0;
    let mut area = vec![icon(item, selected, ink, IOS_ICON)];
    if let Some(badge) = item.badge {
        let left = w / 2.0 + 6.0;
        let (pad, mark) = match badge {
            NavBadge::Dot => (Insets::new(1.0, 0.0, 0.0, left + 2.0), dot(p.danger, 10.0)),
            NavBadge::Count(n) => (
                Insets::new(0.0, 0.0, 0.0, left),
                count(p.danger, n, 13.0, 18.0),
            ),
        };
        area.push(
            Element::stack(Axis::Horizontal, vec![mark])
                .padding(pad)
                .align(Align::Start, Align::Start),
        );
    }
    let area = Element::layers(area)
        .width(w)
        .height(30.0)
        .align(Align::Center, Align::Center);
    let label = Element::text(item.label.clone(), 10.0, ink).weight(FontWeight::MEDIUM);
    Element::stack(Axis::Vertical, vec![area, label])
        .padding(Insets::new(4.0, 0.0, 0.0, 0.0))
        .height(IOS_HEIGHT)
        .align(Align::Start, Align::Center)
}

/// The item's icon (its selected variant when `selected`), `size` square, a
/// template tinted `ink`.
fn icon(item: &NavItem, selected: bool, ink: Color, size: f64) -> Element {
    let which = match (&item.selected_icon, selected) {
        (Some(icon), true) => icon,
        _ => &item.icon,
    };
    let source = match which {
        NavIcon::Image(source) => Some(source.clone()),
        NavIcon::Template(svg) => tinted(svg, ink).map(ImageSource::Svg),
    };
    match source {
        Some(source) => Element::image(source).width(size).height(size),
        None => super::spacer().width(size).height(size),
    }
}

fn dot(color: Color, size: f64) -> Element {
    Element::boxed(BoxStyle {
        fill: Some(color),
        radius: size / 2.0,
        border: None,
    })
    .width(size)
    .height(size)
}

fn count(color: Color, n: u32, text: f64, height: f64) -> Element {
    let shown = if n > 99 {
        "99+".to_string()
    } else {
        n.to_string()
    };
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(shown, text, color.on_color()).weight(FontWeight::MEDIUM)],
    )
    .fill(color)
    .radius(height / 2.0)
    .padding(Insets::symmetric(4.0, 0.0))
    .min_width(height)
    .height(height)
    .align(Align::Center, Align::Center)
}

/// The tinted copies of template icons made so far, by source and color.
type Tinted = Vec<((usize, usize), Color, Arc<Svg>)>;

thread_local! {
    static TINTED: RefCell<Tinted> = const { RefCell::new(Vec::new()) };
}

/// Template `svg` with `currentColor` made `color`, parsed once.
fn tinted(svg: &'static str, color: Color) -> Option<Arc<Svg>> {
    let source = (svg.as_ptr() as usize, svg.len());
    TINTED.with(|cache| {
        if let Some((_, _, doc)) = cache
            .borrow()
            .iter()
            .find(|(s, c, _)| *s == source && *c == color)
        {
            return Some(doc.clone());
        }
        let hex = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
        let doc = Arc::new(Svg::parse(svg.replace("currentColor", &hex).as_bytes()).ok()?);
        let mut cache = cache.borrow_mut();
        // A theme switch or two makes new colors; don't keep them all.
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.push((source, color, doc.clone()));
        Some(doc)
    })
}
