//! A document tab bar, as in a browser, an editor or a terminal: closable,
//! reorderable tabs with a color and an indicator each, a context menu, a
//! new-tab button, and scroll arrows plus a list of every tab when they don't
//! all fit.

use std::cell::Cell;
use std::rc::Rc;

use crate::anim::{Animated, SpringParams};
use crate::core::{
    Align, Anchor, AnimProp, Axis, BoxStyle, Cx, Element, KeyInput, Menu, OverlaySpec, Role,
    Transition,
};
use crate::core::{PointerEvent, PointerEventKind};
use crate::geometry::{Insets, Transform};
use crate::platform::{DragOffer, DropData, PointerButton};
use crate::render::Color;
use crate::style::Theme;

/// A tab never gets narrower than this (logical pixels); past that, the bar
/// scrolls.
pub const TAB_MIN_WIDTH: f64 = 110.0;
/// Nor wider than this.
pub const TAB_MAX_WIDTH: f64 = 220.0;
/// The bar's height.
const HEIGHT: f64 = 34.0;
/// The new-tab, arrow and list buttons' width.
const BUTTON_W: f64 = 28.0;

/// A mark on a tab that something happened in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabIndicator {
    /// A dot (activity, unsaved changes), in the tab's color or the accent.
    Dot,
    /// A bell (a terminal's bell rang).
    Bell,
    /// A count (unread messages).
    Count(u32),
}

/// One tab of a [`TabBar`].
pub struct Tab<S> {
    id: u64,
    label: String,
    color: Option<Color>,
    indicator: Option<TabIndicator>,
    closable: bool,
    menu: Option<Menu<S>>,
}

impl<S> Tab<S> {
    /// A closable tab labeled `label`. `id` identifies it across frames
    /// (while it moves, its hover and its animation follow it): give each
    /// tab its own, kept for its lifetime.
    pub fn new(id: u64, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            color: None,
            indicator: None,
            closable: true,
            menu: None,
        }
    }

    /// Color the tab: a strip along its top and a tint.
    pub fn color(mut self, color: Option<Color>) -> Self {
        self.color = color;
        self
    }

    /// Show `indicator` before the label.
    pub fn indicator(mut self, indicator: Option<TabIndicator>) -> Self {
        self.indicator = indicator;
        self
    }

    /// Whether the tab has a close button (the default).
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    /// The context menu a right-click on the tab (or the Menu key over it)
    /// opens: Rename, Set Color, Close Others…
    pub fn context_menu(mut self, menu: Menu<S>) -> Self {
        self.menu = Some(menu);
        self
    }
}

impl<S> std::fmt::Debug for Tab<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tab")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("color", &self.color)
            .field("indicator", &self.indicator)
            .field("closable", &self.closable)
            .finish_non_exhaustive()
    }
}

type IndexFn<S> = Rc<dyn Fn(&mut S, usize)>;
type ReorderFn<S> = Rc<dyn Fn(&mut S, usize, usize)>;
type NewFn<S> = Rc<dyn Fn(&mut S)>;

/// A tab bar to show with [`tab_bar`]: the tabs, which is selected, and what
/// to do when the user selects, closes, moves or adds one. The app keeps the
/// tabs; the bar only reports what the user did.
pub struct TabBar<S> {
    id: String,
    tabs: Vec<Tab<S>>,
    selected: usize,
    width: Option<f64>,
    on_select: Option<IndexFn<S>>,
    on_close: Option<IndexFn<S>>,
    on_reorder: Option<ReorderFn<S>>,
    on_new: Option<NewFn<S>>,
    on_rename: Option<IndexFn<S>>,
}

impl<S> std::fmt::Debug for TabBar<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabBar")
            .field("id", &self.id)
            .field("tabs", &self.tabs)
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

impl<S> TabBar<S> {
    /// A bar of `tabs` with the first selected. `id` tells this bar's tab
    /// drags from another's (and names its elements); use one per bar.
    pub fn new(id: impl Into<String>, tabs: Vec<Tab<S>>) -> Self {
        Self {
            id: id.into(),
            tabs,
            selected: 0,
            width: None,
            on_select: None,
            on_close: None,
            on_reorder: None,
            on_new: None,
            on_rename: None,
        }
    }

    /// Which tab is selected.
    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    /// The width the bar has, in logical pixels, to work out how many tabs
    /// fit. By default the window's width ([`Cx::window_size`]).
    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width);
        self
    }

    /// Run `f` with a tab's index when it is selected (clicked, picked from
    /// the list, or reached with the arrow keys).
    pub fn on_select(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Run `f` with a tab's index when its close button is clicked (or Delete
    /// is pressed with the bar focused). The app removes it, or asks first.
    pub fn on_close(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }

    /// Run `f(from, to)` when a tab is dragged onto another: the app moves
    /// tab `from` to index `to`. Without it, tabs don't drag.
    pub fn on_reorder(mut self, f: impl Fn(&mut S, usize, usize) + 'static) -> Self {
        self.on_reorder = Some(Rc::new(f));
        self
    }

    /// Run `f` with a tab's index when it is double-clicked: the app starts
    /// renaming it (a message box with a text field, say).
    pub fn on_rename(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_rename = Some(Rc::new(f));
        self
    }

    /// Show a "+" button at the end that runs `f`.
    pub fn on_new(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_new = Some(Rc::new(f));
        self
    }
}

/// What a [`tab_bar`] remembers between frames: how far it is scrolled when
/// the tabs don't fit, whether its list of every tab is open, and the slide
/// of tabs that just moved. Keep one in your app state per bar.
#[derive(Clone, Debug, Default)]
pub struct TabBarState {
    /// The first tab shown; the view moves it to bring a newly selected tab
    /// into view (hence a `Cell`: the view only reads the state otherwise).
    first: Cell<usize>,
    /// The selection the bar last showed, to tell when it changes.
    seen_selected: Cell<Option<usize>>,
    list_open: bool,
    /// Tabs sliding into place after a reorder: by tab id, the offset from
    /// their slot, springing to zero.
    slides: Vec<(u64, Animated<f64>)>,
}

impl TabBarState {
    /// A bar scrolled to its start.
    pub fn new() -> Self {
        Self::default()
    }

    /// The first tab shown when they don't all fit. The arrows scroll it;
    /// a newly selected tab is scrolled into view.
    pub fn first(&self) -> usize {
        self.first.get()
    }

    /// How many tabs are sliding into place after a reorder.
    pub fn slide_count(&self) -> usize {
        self.slides.len()
    }

    /// Whether the list of every tab is open.
    pub fn is_list_open(&self) -> bool {
        self.list_open
    }

    /// Slide the tabs that a move from `from` to `to` shifted (given the ids
    /// in their order before it, and the tabs' width) from where they were.
    fn start_slides(&mut self, ids: &[u64], from: usize, to: usize, width: f64) {
        if from == to || from >= ids.len() || to >= ids.len() || width <= 0.0 {
            return;
        }
        let now = crate::anim::now();
        let mut order: Vec<u64> = ids.to_vec();
        let moved = order.remove(from);
        order.insert(to, moved);
        for (new, id) in order.iter().enumerate() {
            let old = ids.iter().position(|i| i == id).unwrap_or(new);
            if old == new {
                continue;
            }
            let mut a = Animated::new((old as f64 - new as f64) * width);
            a.spring_to(0.0, SpringParams::SNAPPY, now);
            self.slides.retain(|(i, _)| i != id);
            self.slides.push((*id, a));
        }
    }
}

/// How the bar lays out: which tabs show, how wide, and whether it scrolls.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    first: usize,
    count: usize,
    /// Each tab's width, or `None` when the bar's width is unknown (they
    /// share whatever there is).
    tab_width: Option<f64>,
    overflow: bool,
}

/// Work out which of `n` tabs show in `width` (0 for unknown), from `first`
/// (scrolled to show `selected` when `reveal`), with `extra` taken by the
/// new-tab button.
fn fit(n: usize, selected: usize, first: usize, width: f64, extra: f64, reveal: bool) -> Fit {
    if width <= 0.0 || n == 0 {
        return Fit {
            first: 0,
            count: n,
            tab_width: None,
            overflow: false,
        };
    }
    let room = (width - extra).max(0.0);
    if n as f64 * TAB_MIN_WIDTH <= room {
        return Fit {
            first: 0,
            count: n,
            tab_width: Some((room / n as f64).min(TAB_MAX_WIDTH)),
            overflow: false,
        };
    }
    // Two arrows and the list button, then as many whole tabs as fit.
    let room = (room - 3.0 * BUTTON_W).max(0.0);
    let count = ((room / TAB_MIN_WIDTH).floor() as usize).clamp(1, n);
    let selected = selected.min(n - 1);
    let mut first = first.min(n - count);
    if reveal && selected < first {
        first = selected;
    } else if reveal && selected >= first + count {
        first = selected + 1 - count;
    }
    Fit {
        first,
        count,
        tab_width: Some(room / count as f64),
        overflow: true,
    }
}

/// The payload of a tab drag from bar `bar`: `stipple-tab:<bar>:<index>`.
fn drag_payload(bar: &str, index: usize) -> String {
    format!("stipple-tab:{bar}:{index}")
}

fn parse_payload(bar: &str, data: &DropData) -> Option<usize> {
    let DropData::Internal(s) = data else {
        return None;
    };
    s.strip_prefix("stipple-tab:")?
        .strip_prefix(bar)?
        .strip_prefix(':')?
        .parse()
        .ok()
}

/// A small square button with `glyph`, for the bar's ends.
fn bar_button<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    glyph: &str,
    enabled: bool,
    name: &str,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    let p = &theme.palette;
    let ink = if enabled {
        p.text
    } else {
        p.text_muted.with_alpha(110)
    };
    let hover = p.hover_overlay;
    let el = Element::stack(
        Axis::Horizontal,
        vec![Element::text(glyph, theme.typography.body, ink)],
    )
    .width(BUTTON_W)
    .height(HEIGHT - 8.0)
    .radius(theme.radius)
    .align(Align::Center, Align::Center)
    .a11y_label(name);
    if !enabled {
        return el.disabled(true);
    }
    el.hovered(move |e| e.fill(hover))
        .pressed(move |e| e.fill(hover.with_alpha(hover.a.saturating_mul(2))))
        .transition_for(
            AnimProp::Fill,
            Transition::ease(std::time::Duration::from_millis(100)),
        )
        .on_tap(cx, on_tap)
}

/// An indicator's mark, in `accent`.
fn indicator_mark(theme: &Theme, indicator: TabIndicator, accent: Color) -> Element {
    match indicator {
        TabIndicator::Dot => Element::boxed(BoxStyle {
            fill: Some(accent),
            radius: 4.0,
            border: None,
        })
        .width(8.0)
        .height(8.0),
        TabIndicator::Bell => bell(accent)
            .map(|svg| Element::image(svg).width(14.0).height(14.0))
            .unwrap_or_else(|| Element::text("!", theme.typography.caption, accent)),
        TabIndicator::Count(n) => Element::stack(
            Axis::Horizontal,
            vec![Element::text(
                if n > 99 {
                    "99+".to_string()
                } else {
                    n.to_string()
                },
                theme.typography.caption,
                accent.on_color(),
            )],
        )
        .fill(accent)
        .radius(8.0)
        .padding(Insets::symmetric(5.0, 0.0))
        .min_width(16.0)
        .height(16.0)
        .align(Align::Center, Align::Center),
    }
}

thread_local! {
    static BELLS: std::cell::RefCell<Vec<(Color, std::sync::Arc<crate::render::Svg>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A bell icon in `color`.
fn bell(color: Color) -> Option<std::sync::Arc<crate::render::Svg>> {
    BELLS.with(|bells| {
        if let Some((_, svg)) = bells.borrow().iter().find(|(c, _)| *c == color) {
            return Some(svg.clone());
        }
        let fill = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24"><path d="M12 2a1.5 1.5 0 0 1 1.5 1.5v.7A7 7 0 0 1 19 11v4.5l2 2.5v1H3v-1l2-2.5V11a7 7 0 0 1 5.5-6.8v-.7A1.5 1.5 0 0 1 12 2zm-2.5 18h5a2.5 2.5 0 0 1-5 0z" fill="{fill}"/></svg>"#
        );
        let svg = std::sync::Arc::new(crate::render::Svg::parse(doc.as_bytes()).ok()?);
        bells.borrow_mut().push((color, svg.clone()));
        Some(svg)
    })
}

/// A document tab bar for `bar`, scrolled and animated by `state` (see
/// [`TabBarState`]; `field` is an accessor returning it inside the app
/// state).
///
/// - Clicking a tab selects it; its × closes it (`on_close`).
/// - With `on_reorder`, dragging a tab onto another moves it there; the tabs
///   it passes slide into their new places on a spring.
/// - A tab's [`color`](Tab::color) shows as a strip along its top and a tint,
///   and its [`indicator`](Tab::indicator) before its label.
/// - A right-click on a tab opens its [`context_menu`](Tab::context_menu).
/// - With `on_new`, a "+" button ends the bar.
/// - When the tabs don't fit at [`TAB_MIN_WIDTH`], the bar shows as many as
///   do, keeping the selected one in view, with ‹ › arrows to scroll and a ▾
///   button listing every tab.
/// - Focused (Tab key), ←/→ select the previous/next tab, Home/End the
///   first/last, and Delete closes the selected one.
/// - A middle click on a closable tab closes it, and a double click runs
///   `on_rename`.
/// - Tabs, the close buttons and the bar's buttons fade their hover and press
///   looks in and out.
///
/// Assistive technologies see a [`Role::TabList`] of [`Role::Tab`]s, the
/// selected one selected.
///
/// Dragging a tab out into a window of its own isn't done.
pub fn tab_bar<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    bar: TabBar<S>,
    state: &TabBarState,
    field: impl Fn(&mut S) -> &mut TabBarState + Copy + 'static,
) -> Element {
    let p = theme.palette;
    let n = bar.tabs.len();
    let selected = bar.selected.min(n.saturating_sub(1));
    let width = bar.width.unwrap_or(cx.window_size().width);
    let extra = if bar.on_new.is_some() {
        BUTTON_W + 4.0
    } else {
        0.0
    };
    let reveal = state.seen_selected.get() != Some(selected);
    let fit = fit(n, selected, state.first.get(), width, extra, reveal);
    state.first.set(fit.first);
    state.seen_selected.set(Some(selected));
    let ids: Rc<[u64]> = bar.tabs.iter().map(|t| t.id).collect();
    let entries: Vec<(String, Option<Color>)> = bar
        .tabs
        .iter()
        .map(|t| (t.label.clone(), t.color))
        .collect();
    let rtl = theme.layout_direction.is_rtl();
    let bar_bg = p.background.mix(p.border, 0.35);
    let quick = Transition::ease(std::time::Duration::from_millis(120));

    let mut row = Vec::new();
    if fit.overflow {
        let first = fit.first;
        row.push(bar_button(
            cx,
            theme,
            "‹",
            first > 0,
            "Scroll tabs left",
            move |s: &mut S| {
                field(s).first.set(first.saturating_sub(1));
            },
        ));
    }

    let mut strip = Vec::with_capacity(fit.count);
    let bar_id: Rc<str> = bar.id.clone().into();
    for (i, tab) in bar
        .tabs
        .into_iter()
        .enumerate()
        .skip(fit.first)
        .take(fit.count)
    {
        let is_sel = i == selected;
        let accent = tab.color.unwrap_or(p.primary);
        let base = match (is_sel, tab.color) {
            (true, Some(c)) => p.surface.mix(c, 0.12),
            (true, None) => p.surface,
            (false, Some(c)) => bar_bg.mix(c, 0.18),
            (false, None) => bar_bg,
        };
        let ink = if is_sel { p.text } else { p.text_muted };

        let mut content = Vec::new();
        if let Some(ind) = tab.indicator {
            content.push(indicator_mark(theme, ind, accent));
        }
        content.push(
            Element::text(tab.label.clone(), theme.typography.body, ink)
                .grow(1.0)
                .shrink(1.0)
                .clip(),
        );
        if tab.closable {
            let on_close = bar.on_close.clone();
            let danger = p.danger;
            let close = Element::stack(
                Axis::Horizontal,
                vec![Element::text("×", theme.typography.body, p.text_muted)],
            )
            .width(18.0)
            .height(18.0)
            .radius(9.0)
            .align(Align::Center, Align::Center)
            .key(("stipple-tab-close", &*bar_id, tab.id))
            .hovered(move |e| e.fill(danger.with_alpha(40)))
            .pressed(move |e| e.fill(danger.with_alpha(90)))
            .transition_for(AnimProp::Fill, quick)
            .on_tap(cx, move |s: &mut S| {
                if let Some(f) = &on_close {
                    f(s, i);
                }
            })
            .a11y_label(format!("Close {}", tab.label));
            content.push(close);
        }
        let body = Element::stack(Axis::Horizontal, content)
            .gap(theme.spacing.xs)
            .padding(Insets::new(0.0, theme.spacing.sm, 0.0, theme.spacing.md))
            .align(Align::Start, Align::Center)
            .grow(1.0);
        // The color strip along the top (transparent without a color).
        let strip_color = match tab.color {
            Some(c) if is_sel => c,
            Some(c) => c.with_alpha(150),
            None if is_sel => p.primary,
            None => Color::TRANSPARENT,
        };
        let top = Element::boxed(BoxStyle {
            fill: Some(strip_color),
            radius: 0.0,
            border: None,
        })
        .height(3.0);

        let on_select = bar.on_select.clone();
        let hover = p.hover_overlay;
        let hovered_fill = base.mix(hover.with_alpha(255), f64::from(hover.a) / 255.0);
        let mut el = Element::stack(Axis::Vertical, vec![top, body])
            .fill(base)
            .radius(theme.radius)
            .align(Align::Start, Align::Stretch)
            .height(HEIGHT - 4.0)
            .clip()
            .key(("stipple-tab", &*bar_id, tab.id));
        if is_sel {
            el = el.border(p.border, 1.0);
        } else {
            el = el.hovered(move |e| e.fill(hovered_fill));
        }
        el = el
            .dragged(|e| e.opacity(0.55))
            .transition_for(AnimProp::Fill, quick)
            .transition_for(AnimProp::Opacity, quick)
            .transition_for(AnimProp::Border, quick)
            .on_tap(cx, move |s: &mut S| {
                if let Some(f) = &on_select {
                    f(s, i);
                }
            });
        el = match fit.tab_width {
            Some(w) => el.width(w),
            None => el.grow(1.0).shrink(1.0).max_width(TAB_MAX_WIDTH),
        };
        if let Some(on_reorder) = bar.on_reorder.clone() {
            let focus = p.focus_ring;
            let payload = drag_payload(&bar_id, i);
            let bar_name = bar_id.clone();
            let ids = ids.clone();
            let w = fit.tab_width.unwrap_or(0.0);
            el = el
                .drag_source(cx, move |_| Some(DropData::Internal(payload.clone())))
                .on_drop_of(cx, DragOffer::INTERNAL, move |s: &mut S, data, _| {
                    let Some(from) = parse_payload(&bar_name, data) else {
                        return;
                    };
                    if from != i && from < ids.len() {
                        field(s).start_slides(&ids, from, i, w);
                        on_reorder(s, from, i);
                    }
                })
                .drag_over(move |e| e.border(focus, 2.0));
        }
        if let Some((_, slide)) = state.slides.iter().find(|(id, _)| *id == tab.id) {
            let dx = cx.animate(slide);
            if dx.abs() > 0.25 {
                el = el.transform(Transform::IDENTITY.translate(if rtl { -dx } else { dx }, 0.0));
            }
        }
        let (on_close, on_rename) = (bar.on_close.clone(), bar.on_rename.clone());
        let closable = tab.closable;
        if (closable && on_close.is_some()) || on_rename.is_some() {
            el = el.on_pointer(cx, move |ev: &PointerEvent, s: &mut S| {
                if ev.kind != PointerEventKind::Down {
                    return;
                }
                match ev.button {
                    Some(PointerButton::Middle) if closable => {
                        if let Some(f) = &on_close {
                            f(s, i);
                        }
                    }
                    Some(PointerButton::Left) if ev.click_count == 2 => {
                        if let Some(f) = &on_rename {
                            f(s, i);
                        }
                    }
                    _ => {}
                }
            });
        }
        if let Some(menu) = tab.menu {
            el = el.context_menu(cx, menu);
        }
        strip.push(el.role(Role::Tab).selected(is_sel).a11y_label(tab.label));
    }
    row.push(
        Element::stack(Axis::Horizontal, strip)
            .gap(2.0)
            .align(Align::Start, Align::End)
            .grow(1.0)
            .shrink(1.0)
            .clip(),
    );

    if fit.overflow {
        let first = fit.first;
        let more = first + fit.count < n;
        row.push(bar_button(
            cx,
            theme,
            "›",
            more,
            "Scroll tabs right",
            move |s: &mut S| {
                field(s).first.set(first + 1);
            },
        ));
        let list = bar_button(cx, theme, "▾", true, "All tabs", move |s: &mut S| {
            let st = field(s);
            st.list_open = !st.list_open;
        })
        .expanded(state.list_open);
        if state.list_open
            && let Some(tap) = list.action
        {
            let on_select = bar.on_select.clone();
            list_overlay(cx, theme, tap, &entries, selected, on_select, field);
        }
        row.push(list);
    }
    if let Some(on_new) = bar.on_new.clone() {
        row.push(bar_button(
            cx,
            theme,
            "+",
            true,
            "New tab",
            move |s: &mut S| on_new(s),
        ));
    }

    let on_select = bar.on_select.clone();
    let on_close = bar.on_close.clone();
    Element::stack(Axis::Horizontal, row)
        .gap(4.0)
        .padding(Insets::new(4.0, 4.0, 0.0, 4.0))
        .fill(bar_bg)
        .align(Align::Start, Align::End)
        .height(HEIGHT)
        .on_key(cx, move |s: &mut S, k| {
            let to = match k {
                KeyInput::Left if !rtl => selected.checked_sub(1),
                KeyInput::Right if rtl => selected.checked_sub(1),
                KeyInput::Right | KeyInput::Left => (selected + 1 < n).then_some(selected + 1),
                KeyInput::Home => (n > 0).then_some(0),
                KeyInput::End => n.checked_sub(1),
                KeyInput::Delete => {
                    if let (Some(f), true) = (&on_close, n > 0) {
                        f(s, selected);
                    }
                    None
                }
                _ => None,
            };
            if let (Some(to), Some(f)) = (to, &on_select) {
                f(s, to);
            }
        })
        .role(Role::TabList)
}

/// The list of every tab, dropped below the ▾ button (`tap`).
fn list_overlay<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    tap: crate::core::ActionId,
    entries: &[(String, Option<Color>)],
    selected: usize,
    on_select: Option<IndexFn<S>>,
    field: impl Fn(&mut S) -> &mut TabBarState + Copy + 'static,
) {
    let p = theme.palette;
    let items = entries
        .iter()
        .enumerate()
        .map(|(i, (label, color))| {
            let dot = Element::boxed(BoxStyle {
                fill: Some(color.unwrap_or(Color::TRANSPARENT)),
                radius: 4.0,
                border: None,
            })
            .width(8.0)
            .height(8.0);
            let on_select = on_select.clone();
            let hover = p.hover_overlay;
            let weight_ink = if i == selected { p.primary } else { p.text };
            Element::stack(
                Axis::Horizontal,
                vec![
                    dot,
                    Element::text(label.clone(), theme.typography.body, weight_ink),
                ],
            )
            .gap(theme.spacing.sm)
            .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
            .radius(theme.radius / 2.0)
            .align(Align::Start, Align::Center)
            .hovered(move |e| e.fill(hover))
            .on_tap(cx, move |s: &mut S| {
                field(s).list_open = false;
                if let Some(f) = &on_select {
                    f(s, i);
                }
            })
            .role(Role::MenuItem)
            .selected(i == selected)
            .a11y_label(label.clone())
        })
        .collect();
    let content = Element::stack(Axis::Vertical, items)
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .gap(2.0)
        .align(Align::Start, Align::Stretch)
        .min_width(180.0)
        .role(Role::Menu);
    let dismiss = cx.register(move |s: &mut S| field(s).list_open = false);
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::Below(tap),
        modal: false,
        dismiss: Some(dismiss),
    });
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn fit_shares_the_width_then_scrolls() {
        // Unknown width: everything, sized by layout.
        assert_eq!(fit(5, 0, 0, 0.0, 0.0, true).count, 5);
        // Room for all: equal shares, capped.
        let f = fit(3, 0, 0, 900.0, 0.0, true);
        assert_eq!(
            (f.count, f.overflow, f.tab_width),
            (3, false, Some(TAB_MAX_WIDTH))
        );
        assert_eq!(fit(4, 0, 0, 480.0, 40.0, true).tab_width, Some(110.0));
        // Ten tabs in 500 px: the arrows and the list take 84, so 3 show.
        let f = fit(10, 0, 0, 500.0, 0.0, true);
        assert!(f.overflow);
        assert_eq!((f.first, f.count), (0, 3));
        // A newly selected tab is brought into view from either side...
        assert_eq!(fit(10, 8, 0, 500.0, 0.0, true).first, 6);
        assert_eq!(fit(10, 2, 5, 500.0, 0.0, true).first, 2);
        // ...but the arrows may scroll away from the selection.
        assert_eq!(fit(10, 2, 5, 500.0, 0.0, false).first, 5);
        // Scrolled past the end: clamped.
        assert_eq!(fit(10, 9, 50, 500.0, 0.0, false).first, 7);
    }

    #[test]
    fn payloads_name_their_bar() {
        let d = DropData::Internal(drag_payload("docs", 4));
        assert_eq!(parse_payload("docs", &d), Some(4));
        assert_eq!(parse_payload("doc", &d), None);
        assert_eq!(parse_payload("docs", &DropData::Text("x".into())), None);
    }
}
