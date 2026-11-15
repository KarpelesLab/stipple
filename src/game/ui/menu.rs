//! Menus and screens: [`menu_button`], [`menu_list`], [`TitleScreen`],
//! [`pause_menu`], [`GameOverScreen`] and [`controls_hint`].

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
    label: impl Into<String>,
    on_press: impl FnMut(&mut S) + 'static,
) -> Element {
    let theme = &crate::widgets::theme_of(cx);
    let label = label.into();
    let key = ("menu_button", label.clone());
    let states = cx.interaction(&key);
    let lit = states.focus_visible() || states.hovered();
    let p = &theme.palette;
    let state = Transition(theme.motion.state);
    let skin = theme.skin.and_then(|s| s.button.as_ref());
    let (fill, ink, border) = if lit && skin.is_none() {
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
    let el = Element::stack(
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
    .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.md))
    .gap(theme.spacing.sm)
    .min_width(280.0);
    let el = match skin {
        // The skin's frame for the state: focused (or hovered) and pressed
        // fall back on each other, then on the frame at rest.
        Some(b) => {
            let pressed = b.pressed.as_ref().filter(|_| states.pressed());
            let lit_frame = (b.focused.as_ref().or(b.hovered.as_ref())).filter(|_| lit);
            el.background_image(pressed.or(lit_frame).unwrap_or(&b.normal))
        }
        None => el.fill(fill).border(border, 2.0).radius(theme.radius),
    };
    el.transform(Transform::IDENTITY.scale(scale))
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

/// `el` as a panel: the skin's panel frame, or the surface with a
/// `border`-colored outline `width` wide and `radius` corners.
fn framed(theme: &Theme, el: Element, border: Color, width: f64, radius: f64) -> Element {
    match theme.skin.and_then(|s| s.panel.as_ref()) {
        Some(frame) => el.background_image(frame),
        None => el
            .fill(theme.palette.surface)
            .border(border, width)
            .radius(radius),
    }
}

/// A column for [`menu_button`]s (or any controls; add them with
/// [`children`](Element::children)) the arrow keys and a gamepad move
/// through by where they are ([`spatial_nav`](Element::spatial_nav)); its
/// first control takes focus when nothing has it.
pub fn menu_list<S>(cx: &Cx<S>) -> Element {
    let theme = cx.theme();
    Element::stack(Axis::Vertical, Vec::new())
        .gap(theme.spacing.sm)
        .spatial_nav()
}

/// A row of control hints, each a key cap (or a gamepad button's name) and
/// what it does: `[("↑↓", "Move"), ("Enter / A", "Select")]`.
pub fn controls_hint<S>(cx: &Cx<S>, hints: &[(&str, &str)]) -> Element {
    let theme = cx.theme();
    let p = &theme.palette;
    let items = hints
        .iter()
        .map(|(key, what)| {
            Element::stack(
                Axis::Horizontal,
                vec![
                    key_cap(cx, key),
                    Element::text(*what, theme.typography.caption, p.text_muted),
                ],
            )
            .gap(theme.spacing.xs)
        })
        .collect();
    Element::stack(Axis::Horizontal, items)
        .gap(theme.spacing.lg)
        .align(Align::Center, Align::Center)
}

/// A key cap: `text` (a key's or a gamepad button's name) in a small framed
/// box, as the controls screen and the hints show bindings.
pub fn key_cap<S>(cx: &Cx<S>, text: &str) -> Element {
    let theme = cx.theme();
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

/// A title screen: the game's title large in the accent, a subtitle under
/// it, the menu (a [`menu_list`]) below, and a footer (say a
/// [`controls_hint`]) at the bottom, on the theme's background, or on a
/// [`backdrop`](Self::backdrop) of your choosing, or none, to show the game
/// behind it.
///
/// ```
/// use stipple::game::ui::{self as gui, TitleScreen};
/// use stipple::prelude::*;
///
/// fn title(cx: &mut Cx<()>, theme: &Theme) -> Element {
///     let menu = gui::menu_list(cx).child(gui::menu_button(cx, "Play", |_| {}));
///     TitleScreen::new("STARLIT COURIER", menu)
///         .subtitle("deliveries across the comet belt")
///         .footer(gui::controls_hint(cx, &[("Enter / A", "Select")]))
///         .view(cx)
/// }
/// # let _ = title;
/// ```
#[derive(Debug)]
pub struct TitleScreen {
    title: String,
    subtitle: Option<String>,
    menu: Element,
    footer: Option<Element>,
    /// `None` for the theme's background.
    backdrop: Option<Option<Color>>,
}

impl TitleScreen {
    /// A title screen showing `title` over `menu`.
    pub fn new(title: impl Into<String>, menu: Element) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            menu,
            footer: None,
            backdrop: None,
        }
    }

    /// With `subtitle` under the title.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// With `footer` at the bottom.
    pub fn footer(mut self, footer: Element) -> Self {
        self.footer = Some(footer);
        self
    }

    /// Fill the screen with `color` instead of the theme's background, or
    /// with nothing (`None`): the game, or whatever is under the screen,
    /// shows through.
    pub fn backdrop(mut self, color: Option<Color>) -> Self {
        self.backdrop = Some(color);
        self
    }

    /// The screen's element.
    pub fn view<S>(self, cx: &Cx<S>) -> Element {
        let theme = cx.theme();
        let p = &theme.palette;
        let heading = theme.typography.heading;
        let mut head = vec![Element::text(self.title, heading * 1.6, p.primary).bold()];
        if let Some(subtitle) = self.subtitle {
            head.push(Element::text(
                subtitle,
                theme.typography.title,
                p.text_muted,
            ));
        }
        let head = Element::stack(Axis::Vertical, head)
            .gap(theme.spacing.xs)
            .align(Align::Start, Align::Center)
            .appear(Effect::fade().and(Effect::slide(0.0, -16.0)));
        let mut children = vec![
            crate::widgets::spacer(),
            head,
            gap(theme.spacing.xl),
            self.menu,
            crate::widgets::spacer(),
        ];
        children.extend(self.footer);
        backdrop(
            screen(theme, children),
            self.backdrop.unwrap_or(Some(p.background)),
        )
    }
}

/// A game-over screen: the title large (in the danger color, popping in),
/// lines of results (a label and a value each: score, time, best) in a
/// panel, and the menu (retry, quit) under them, on the theme's background
/// darkened, or on a [`backdrop`](Self::backdrop) of your choosing.
///
/// ```
/// use stipple::game::ui::{self as gui, GameOverScreen};
/// use stipple::prelude::*;
///
/// fn game_over(cx: &mut Cx<()>, theme: &Theme, score: u32) -> Element {
///     let menu = gui::menu_list(cx).child(gui::menu_button(cx, "Try again", |_| {}));
///     GameOverScreen::new("HULL BREACHED", menu)
///         .line("Score", score)
///         .line("Time", "2:05")
///         .view(cx)
/// }
/// # let _ = game_over;
/// ```
#[derive(Debug)]
pub struct GameOverScreen {
    title: String,
    lines: Vec<(String, String)>,
    menu: Element,
    /// `None` for the theme's background, darkened.
    backdrop: Option<Option<Color>>,
}

impl GameOverScreen {
    /// A game-over screen showing `title` over `menu`.
    pub fn new(title: impl Into<String>, menu: Element) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            menu,
            backdrop: None,
        }
    }

    /// With a line of results: `label` and its `value`.
    pub fn line(mut self, label: impl Into<String>, value: impl ToString) -> Self {
        self.lines.push((label.into(), value.to_string()));
        self
    }

    /// Fill the screen with `color` instead of the theme's darkened
    /// background, or with nothing (`None`): the game shows through.
    pub fn backdrop(mut self, color: Option<Color>) -> Self {
        self.backdrop = Some(color);
        self
    }

    /// The screen's element.
    pub fn view<S>(self, cx: &Cx<S>) -> Element {
        let theme = cx.theme();
        let p = &theme.palette;
        let rows = self
            .lines
            .into_iter()
            .map(|(what, value)| {
                Element::stack(
                    Axis::Horizontal,
                    vec![
                        Element::text(what, theme.typography.body, p.text_muted),
                        crate::widgets::spacer(),
                        Element::text(value, theme.typography.title, p.text).bold(),
                    ],
                )
            })
            .collect();
        let results = Element::stack(Axis::Vertical, rows)
            .gap(theme.spacing.sm)
            .padding(Insets::uniform(theme.spacing.lg))
            .width(320.0);
        let results = framed(theme, results, p.border, 1.0, theme.radius);
        let page = screen(
            theme,
            vec![
                crate::widgets::spacer(),
                Element::text(self.title, theme.typography.heading * 1.4, p.danger)
                    .bold()
                    .appear(
                        Effect::fade()
                            .and(Effect::scale(1.4))
                            .transition(Transition::spring(SpringParams::BOUNCY)),
                    ),
                gap(theme.spacing.lg),
                results,
                gap(theme.spacing.lg),
                self.menu,
                crate::widgets::spacer(),
            ],
        );
        backdrop(
            page,
            self.backdrop
                .unwrap_or(Some(p.background.mix(Color::BLACK, 0.2))),
        )
    }
}

/// The pause menu: a modal panel with `title` and the [`menu_button`]s you
/// add with [`children`](PauseMenu::children) (a [`menu_list`] is made of
/// them) over the game, which shows frozen and dimmed behind it. Declare it
/// with [`open`](PauseMenu::open) on each build while the game is paused
/// (pause the loop with
/// [`GameLoop::paused_when`](crate::game::GameLoop::paused_when)); a press
/// on the dimmed game runs `on_resume`. It fades and grows in, and out when
/// the view stops declaring it. Escape and a gamepad's East button go back
/// through the app's [`on_back`](crate::App::on_back): resume there.
///
/// ```
/// use stipple::game::ui as gui;
/// use stipple::prelude::*;
/// struct Game { paused: bool }
/// fn view(game: &Game, cx: &mut Cx<Game>) -> Element {
///     if game.paused {
///         gui::pause_menu(cx, "Paused", |g| g.paused = false)
///             .children(vec![
///                 gui::menu_button(cx, "Resume", |g| g.paused = false),
///                 gui::menu_button(cx, "Quit", |_| {}),
///             ])
///             .open(cx);
///     }
///     spacer()
/// }
/// # let theme = Theme::dark();
/// # let _ = view(&Game { paused: true }, &mut Cx::new(&theme));
/// ```
pub fn pause_menu<S>(
    cx: &mut Cx<S>,
    title: impl Into<String>,
    on_resume: impl FnMut(&mut S) + 'static,
) -> PauseMenu {
    PauseMenu {
        title: title.into(),
        dismiss: cx.register(on_resume),
        buttons: Vec::new(),
    }
}

/// A pause menu being built: see [`pause_menu`]. It shows once
/// [`open`](Self::open) declares it.
#[derive(Debug)]
#[must_use = "a pause menu shows only once `.open(cx)` declares it"]
pub struct PauseMenu {
    title: String,
    dismiss: crate::core::ActionId,
    buttons: Vec<Element>,
}

impl PauseMenu {
    /// Add `child` (a [`menu_button`]) to the menu.
    pub fn child(mut self, child: Element) -> Self {
        self.buttons.push(child);
        self
    }

    /// Add `children` ([`menu_button`]s) to the menu.
    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.buttons.extend(children);
        self
    }

    /// Declare the pause menu as an overlay on this build.
    pub fn open<S>(self, cx: &mut Cx<S>) {
        let theme = &crate::widgets::theme_of(cx);
        let p = &theme.palette;
        let content = Element::stack(
            Axis::Vertical,
            vec![
                Element::stack(
                    Axis::Horizontal,
                    vec![Element::text(self.title, theme.typography.heading, p.text).bold()],
                )
                .align(Align::Center, Align::Center),
                menu_list(cx).children(self.buttons),
            ],
        )
        .gap(theme.spacing.lg)
        .padding(Insets::uniform(theme.spacing.xl))
        .align(Align::Center, Align::Stretch);
        let content = framed(theme, content, p.primary, 2.0, theme.radius * 2.0)
            .role(Role::Dialog)
            .enter(Effect::fade().and(Effect::scale(0.9)))
            .exit(Effect::fade().and(Effect::scale(0.95)));
        cx.overlay(OverlaySpec {
            content,
            anchor: Anchor::Center,
            modal: true,
            dismiss: Some(self.dismiss),
        });
    }
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

/// `page` on `color`, if any.
fn backdrop(page: Element, color: Option<Color>) -> Element {
    match color {
        Some(color) => page.fill(color),
        None => page,
    }
}
