//! Menus and screens: [`menu_button`], [`menu_list`], [`title_screen`],
//! [`pause_menu`], [`game_over_screen`] and [`controls_hint`].

use crate::anim::SpringParams;
use crate::core::{
    Align, Anchor, AnimProp, Axis, Cx, Effect, Element, OverlaySpec, Role, Transition,
};
use crate::geometry::{Insets, Transform};
use crate::render::Color;
use crate::style::Theme;

/// How much a focused or hovered menu button grows.
pub const MENU_FOCUS_SCALE: f64 = 1.06;

/// A game menu's button: a wide panel with the label in the title size,
/// which lights up in the accent, grows a little and shows a marker when
/// focused (by the arrows or a gamepad) or hovered, and presses on a tap,
/// Enter, Space or a gamepad's South button.
///
/// It reads its focus and hover through [`Cx::interaction`] under the key
/// `("menu_button", label)`, so give the buttons on one screen different
/// labels. Put them in a [`menu_list`] for the arrows and the pad to move
/// between them.
pub fn menu_button<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    label: impl Into<String>,
    on_press: impl FnMut(&mut S) + 'static,
) -> Element {
    let label = label.into();
    let key = ("menu_button", label.clone());
    let states = cx.interaction(&key);
    let lit = states.focus_visible() || states.hovered();
    let p = &theme.palette;
    let state = Transition(theme.motion.state);
    let (fill, ink, border) = if lit {
        (p.primary, p.on_primary, p.focus_ring)
    } else {
        (p.surface.mix(p.background, 0.15), p.text, p.border)
    };
    // A marker either side, shown while lit (kept in the layout so the
    // label doesn't move).
    let marker = |glyph: &str| {
        Element::text(glyph, theme.typography.title, ink)
            .bold()
            .opacity(if lit { 1.0 } else { 0.0 })
            .transition(state)
    };
    let scale = if states.pressed() {
        0.97
    } else if lit {
        MENU_FOCUS_SCALE
    } else {
        1.0
    };
    Element::stack(
        Axis::Horizontal,
        vec![
            marker("\u{203A}"),
            crate::widgets::spacer(),
            Element::text(label.clone(), theme.typography.title, ink)
                .bold()
                .transition(state),
            crate::widgets::spacer(),
            marker("\u{2039}"),
        ],
    )
    .fill(fill)
    .border(border, 2.0)
    .radius(theme.radius)
    .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.md))
    .gap(theme.spacing.sm)
    .align(Align::Start, Align::Center)
    .min_width(280.0)
    .transform(Transform::IDENTITY.scale(scale))
    .transition_for(AnimProp::Fill, state)
    .transition_for(AnimProp::Border, state)
    .transition_for(
        AnimProp::Transform,
        Transition::spring(SpringParams::SNAPPY),
    )
    .key(key)
    .on_tap(cx, on_press)
    .role(Role::Button)
    .a11y_label(label)
}

/// A column of [`menu_button`]s (or any controls) the arrow keys and a
/// gamepad move through by where they are
/// ([`spatial_nav`](Element::spatial_nav)); its first control takes focus
/// when nothing has it.
pub fn menu_list(theme: &Theme, items: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, items)
        .gap(theme.spacing.sm)
        .align(Align::Start, Align::Stretch)
        .spatial_nav()
}

/// A row of control hints, each a key cap (or a gamepad button's name) and
/// what it does: `[("↑↓", "Move"), ("Enter / A", "Select")]`.
pub fn controls_hint(theme: &Theme, hints: &[(&str, &str)]) -> Element {
    let p = &theme.palette;
    let items = hints
        .iter()
        .map(|(key, what)| {
            Element::stack(
                Axis::Horizontal,
                vec![
                    key_cap(theme, key),
                    Element::text(*what, theme.typography.caption, p.text_muted),
                ],
            )
            .gap(theme.spacing.xs)
            .align(Align::Start, Align::Center)
        })
        .collect();
    Element::stack(Axis::Horizontal, items)
        .gap(theme.spacing.lg)
        .align(Align::Center, Align::Center)
}

/// A key cap: `text` (a key's or a gamepad button's name) in a small framed
/// box, as the controls screen and the hints show bindings.
pub fn key_cap(theme: &Theme, text: &str) -> Element {
    let p = &theme.palette;
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(text, theme.typography.caption, p.text).bold()],
    )
    .fill(p.background.mix(p.surface, 0.5))
    .border(p.border, 1.5)
    .radius((theme.radius / 2.0).max(3.0))
    .padding(Insets::symmetric(theme.spacing.xs + 2.0, 2.0))
    .min_width(26.0)
    .align(Align::Center, Align::Center)
}

/// A title screen: the game's `title` large in the accent, a `subtitle`
/// under it, the `menu` (a [`menu_list`]) below, and `footer` (say a
/// [`controls_hint`]) at the bottom, on the theme's background.
pub fn title_screen(
    theme: &Theme,
    title: impl Into<String>,
    subtitle: impl Into<String>,
    menu: Element,
    footer: Element,
) -> Element {
    let p = &theme.palette;
    let heading = theme.typography.heading;
    let head = Element::stack(
        Axis::Vertical,
        vec![
            Element::text(title, heading * 1.6, p.primary).bold(),
            Element::text(subtitle, theme.typography.title, p.text_muted),
        ],
    )
    .gap(theme.spacing.xs)
    .align(Align::Start, Align::Center)
    .appear(Effect::fade().and(Effect::slide(0.0, -16.0)));
    screen(
        theme,
        vec![
            crate::widgets::spacer(),
            head,
            gap(theme.spacing.xl),
            menu,
            crate::widgets::spacer(),
            footer,
        ],
    )
    .fill(p.background)
}

/// A game-over screen: `title` large (in the danger color), `lines` of
/// results (a label and a value each: score, time, best), and the `menu`
/// (retry, quit) under them.
pub fn game_over_screen(
    theme: &Theme,
    title: impl Into<String>,
    lines: &[(&str, String)],
    menu: Element,
) -> Element {
    let p = &theme.palette;
    let rows = lines
        .iter()
        .map(|(what, value)| {
            Element::stack(
                Axis::Horizontal,
                vec![
                    Element::text(*what, theme.typography.body, p.text_muted),
                    crate::widgets::spacer(),
                    Element::text(value.clone(), theme.typography.title, p.text).bold(),
                ],
            )
            .align(Align::Start, Align::Center)
        })
        .collect();
    let results = Element::stack(Axis::Vertical, rows)
        .gap(theme.spacing.sm)
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.lg))
        .align(Align::Start, Align::Stretch)
        .width(320.0);
    screen(
        theme,
        vec![
            crate::widgets::spacer(),
            Element::text(title, theme.typography.heading * 1.4, p.danger)
                .bold()
                .appear(
                    Effect::fade()
                        .and(Effect::scale(1.4))
                        .transition(Transition::spring(SpringParams::BOUNCY)),
                ),
            gap(theme.spacing.lg),
            results,
            gap(theme.spacing.lg),
            menu,
            crate::widgets::spacer(),
        ],
    )
    .fill(p.background.mix(Color::BLACK, 0.2))
}

/// Declare the pause menu: a modal panel with `title` and `buttons` (a
/// [`menu_list`] is made of them) over the game, which shows frozen and
/// dimmed behind it. Call it from the view while the game is paused
/// (pause the loop with [`GameLoop::paused_when`](crate::game::GameLoop::paused_when));
/// a press on the dimmed game runs `on_resume`. It fades and grows in, and
/// out when the view stops declaring it. Escape and a gamepad's East button
/// go back through the app's [`on_back`](crate::App::on_back): resume there.
pub fn pause_menu<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    title: impl Into<String>,
    buttons: Vec<Element>,
    on_resume: impl FnMut(&mut S) + 'static,
) {
    let p = &theme.palette;
    let dismiss = cx.register(on_resume);
    let content = Element::stack(
        Axis::Vertical,
        vec![
            Element::stack(
                Axis::Horizontal,
                vec![Element::text(title, theme.typography.heading, p.text).bold()],
            )
            .align(Align::Center, Align::Center),
            menu_list(theme, buttons),
        ],
    )
    .gap(theme.spacing.lg)
    .fill(p.surface)
    .border(p.primary, 2.0)
    .radius(theme.radius * 2.0)
    .padding(Insets::uniform(theme.spacing.xl))
    .align(Align::Center, Align::Stretch)
    .role(Role::Dialog)
    .enter(Effect::fade().and(Effect::scale(0.9)))
    .exit(Effect::fade().and(Effect::scale(0.95)));
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::Center,
        modal: true,
        dismiss: Some(dismiss),
    });
}

/// A screen: `children` in a column filling the window, centered across.
fn screen(theme: &Theme, children: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, children)
        .padding(Insets::uniform(theme.spacing.xl))
        .align(Align::Start, Align::Center)
        .grow(1.0)
}

/// A fixed gap in a column.
fn gap(height: f64) -> Element {
    Element::stack(Axis::Vertical, Vec::new()).height(height)
}
