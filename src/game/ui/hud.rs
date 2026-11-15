//! A HUD over the game: [`hud`], [`stat_bar`] / [`health_bar`] with a
//! damage ghost, [`counter`] with a pop, [`minimap_slot`],
//! [`FloatingTexts`], and [`DialogueBox`] with [`Typewriter`] text.

use std::cell::Cell;
use std::time::Duration;

use crate::anim::{Easing, Motion, SpringParams};
use crate::core::{Align, AnimProp, Axis, BoxStyle, Cx, Effect, Element, Role, Transition};
use crate::geometry::{Insets, Point};
use crate::render::Color;

/// The game (a canvas, say) with `overlay` laid over it: the overlay lets
/// pointer input through to the game except where one of its controls sits
/// (see [`Element::hit_through`]), so a click on the HUD's empty space, a
/// health bar or a score reaches the canvas's
/// [`on_pointer`](Element::on_pointer) handler and a click on a pause
/// button presses the button.
///
/// A game that reads clicks from its
/// [`InputState`](crate::game::InputState) instead sees every click,
/// those on HUD buttons too: take the game's clicks with `on_pointer` on
/// the canvas to leave the HUD's to the HUD.
pub fn hud(game: Element, overlay: Element) -> Element {
    Element::layers(vec![game, overlay.hit_through()])
}

/// How the trailing part of a bar that just went down moves to the new
/// value: it lingers, then drains (a 700 ms ease that starts slow).
pub const GHOST_MOTION: Motion = Motion::Tween {
    duration: 0.7,
    easing: Easing::cubic_bezier(0.8, 0.0, 0.3, 1.0),
};

/// How the bar itself moves to a new value: quickly (120 ms).
pub const FILL_MOTION: Motion = Motion::Tween {
    duration: 0.12,
    easing: Easing::EaseOut,
};

/// A HUD bar: health, energy, a boss's life, a charge.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// How full it is, 0 to `max`.
    pub value: f32,
    pub max: f32,
    /// The fill's color.
    pub color: Color,
    /// Cut into this many cells (hearts, pips); 0 or 1 for a smooth bar.
    pub segments: u32,
    /// The color of what was just lost, trailing behind the fill as it
    /// goes down; `None` for no ghost.
    pub ghost: Option<Color>,
    /// Its size, in logical pixels.
    pub width: f64,
    pub height: f64,
}

impl Bar {
    /// A smooth bar of `value` out of `max` in `color`, with a pale ghost of
    /// the same color, 200 × 14.
    pub fn new(value: f32, max: f32, color: Color) -> Self {
        Self {
            value,
            max,
            color,
            segments: 0,
            ghost: Some(color.mix(Color::WHITE, 0.55)),
            width: 200.0,
            height: 14.0,
        }
    }

    /// Cut it into `n` cells.
    pub fn segments(mut self, n: u32) -> Self {
        self.segments = n;
        self
    }

    /// Size it.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Trail losses in `color`, or not at all (`None`).
    pub fn ghost(mut self, color: Option<Color>) -> Self {
        self.ghost = color;
        self
    }

    /// How full it is, 0 to 1.
    pub fn fraction(&self) -> f64 {
        if self.max > 0.0 {
            f64::from((self.value / self.max).clamp(0.0, 1.0))
        } else {
            0.0
        }
    }
}

/// A HUD bar (see [`Bar`]): a dark track, the fill, and on a drop the part
/// just lost in the ghost color, lingering and then draining to the new
/// value ([`GHOST_MOTION`]) while the fill moves at once
/// ([`FILL_MOTION`]). A segmented bar shows its cells with gaps.
///
/// The fill and the ghost are separate elements whose widths animate, so
/// keep the bar in the same place in the view from frame to frame (or
/// [`key`](Element::key) it).
pub fn stat_bar<S>(cx: &Cx<S>, bar: &Bar) -> Element {
    let theme = cx.theme();
    let p = &theme.palette;
    let skin = theme.skin.and_then(|s| s.bar.as_ref());
    let pad = match skin {
        Some(skin) => skin.track.content_padding(),
        None => Insets::uniform(2.0),
    };
    let inner = (bar.width - pad.horizontal()).max(0.0);
    let h = (bar.height - pad.vertical()).max(0.0);
    let filled = (inner * bar.fraction()).round();
    let radius = (theme.radius / 2.0).min(h / 2.0);
    let strip = |fill: Element, motion: Motion| {
        Element::stack(
            Axis::Horizontal,
            vec![fill.transition_for(AnimProp::Width, Transition(motion))],
        )
        .align(Align::Start, Align::Start)
    };
    let colored = |color: Color| {
        Element::boxed(BoxStyle {
            fill: Some(color),
            radius,
            border: None,
        })
        .width(filled)
        .height(h)
    };
    let mut layers = Vec::with_capacity(3);
    if let Some(ghost) = bar.ghost {
        layers.push(strip(colored(ghost), GHOST_MOTION));
    }
    let fill = match skin {
        Some(skin) => skin.fill_element((inner, h), filled, bar.color),
        None => colored(bar.color),
    };
    layers.push(strip(fill, FILL_MOTION));
    if bar.segments > 1 {
        // Gaps between the cells, in the track's color.
        let n = bar.segments as usize;
        let mut cells = Vec::with_capacity(2 * n - 1);
        for i in 0..n {
            if i > 0 {
                cells.push(
                    Element::boxed(BoxStyle {
                        fill: Some(p.background),
                        radius: 0.0,
                        border: None,
                    })
                    .width(3.0)
                    .height(h),
                );
            }
            cells.push(crate::widgets::spacer());
        }
        layers.push(Element::stack(Axis::Horizontal, cells));
    }
    let track = Element::layers(layers).width(bar.width).height(bar.height);
    match skin {
        Some(skin) => track.background_image(&skin.track).clip(),
        None => track
            .padding(pad)
            .fill(p.background.mix(Color::BLACK, 0.3))
            .border(p.border, 1.0)
            .radius(radius + 2.0)
            .clip(),
    }
}

/// A health bar: `value` of `max` in the theme's danger color, with a ghost,
/// and the numbers beside it.
pub fn health_bar<S>(cx: &Cx<S>, value: f32, max: f32) -> Element {
    let theme = cx.theme();
    let p = &theme.palette;
    let bar = stat_bar(cx, &Bar::new(value, max, p.danger));
    Element::stack(
        Axis::Horizontal,
        vec![
            Element::text("\u{2665}", theme.typography.title, p.danger),
            bar,
            Element::text(
                format!("{}/{}", value.max(0.0).round(), max.round()),
                theme.typography.caption,
                p.text,
            )
            .bold(),
        ],
    )
    .gap(theme.spacing.xs)
}

/// A HUD counter (score, coins, ammo): `label` and `value`, the number
/// popping (growing and settling back, on a bouncy spring) each time it
/// changes. `id` tells counters apart.
pub fn counter<S>(cx: &Cx<S>, id: &str, label: &str, value: i64) -> Element {
    let theme = cx.theme();
    let p = &theme.palette;
    let number = Element::text(value.to_string(), theme.typography.title, p.text)
        .bold()
        .key(("counter", id, value))
        .enter(Effect::scale(1.6).transition(Transition::spring(SpringParams::BOUNCY)));
    Element::stack(
        Axis::Horizontal,
        vec![
            Element::text(label, theme.typography.caption, p.text_muted),
            number,
        ],
    )
    .gap(theme.spacing.xs)
    .padding(Insets::symmetric(theme.spacing.sm, theme.spacing.xs))
    .fill(Color {
        a: 200,
        ..p.background
    })
    .radius(theme.radius)
    .key(("counter-box", id))
}

/// A framed slot for a minimap, `width` × `height`: the frame, a dark
/// backing, and `content` (a canvas the game draws the map into, or
/// nothing yet) clipped inside.
pub fn minimap_slot<S>(cx: &Cx<S>, width: f64, height: f64, content: Option<Element>) -> Element {
    let theme = cx.theme();
    let p = &theme.palette;
    let inside = content.unwrap_or_else(|| {
        Element::stack(
            Axis::Vertical,
            vec![Element::text("MAP", theme.typography.caption, p.text_muted)],
        )
        .align(Align::Center, Align::Center)
    });
    Element::stack(Axis::Vertical, vec![inside.grow(1.0)])
        .width(width)
        .height(height)
        .fill(Color {
            a: 210,
            ..p.background
        })
        .border(p.primary, 2.0)
        .radius(theme.radius)
        .padding(Insets::uniform(2.0))
        .clip()
}

/// How long a floating text stays before it rises off and fades.
pub const FLOAT_LIFE: Duration = Duration::from_millis(600);

/// One floating text.
#[derive(Clone, Debug, PartialEq)]
struct Floater {
    id: u64,
    text: String,
    color: Color,
    at: Point,
    born: Duration,
}

/// Texts that pop up where something happened ("-12", "+50", "Critical!"),
/// rise and fade: [`spawn`](Self::spawn) one at a point of the HUD,
/// [`update`](Self::update) each frame, and show them with
/// [`floating_texts`] in the HUD.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FloatingTexts {
    items: Vec<Floater>,
    next: u64,
}

impl FloatingTexts {
    /// No texts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Show `text` in `color` at `at` (in the HUD's coordinates), from
    /// frame time `now` (see [`Frame::time`](crate::game::Frame::time)).
    pub fn spawn(&mut self, text: impl Into<String>, color: Color, at: Point, now: Duration) {
        self.items.push(Floater {
            id: self.next,
            text: text.into(),
            color,
            at,
            born: now,
        });
        self.next += 1;
    }

    /// Drop the texts older than [`FLOAT_LIFE`] at `now` (they rise and
    /// fade out as they go). Returns whether any went, so the view needs
    /// a rebuild.
    pub fn update(&mut self, now: Duration) -> bool {
        let before = self.items.len();
        self.items
            .retain(|f| now.saturating_sub(f.born) < FLOAT_LIFE);
        self.items.len() != before
    }

    /// How many are showing.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether none are.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The [`FloatingTexts`], each where it was spawned: popping in as it comes
/// (fading in, rising into place), and rising off as it fades when it goes.
/// Lay it over the game in a [`hud`].
pub fn floating_texts<S>(cx: &Cx<S>, texts: &FloatingTexts) -> Element {
    let theme = cx.theme();
    let size = theme.typography.title;
    let layers = texts
        .items
        .iter()
        .map(|f| {
            let text = Element::text(f.text.clone(), size, f.color).bold();
            Element::stack(Axis::Vertical, vec![text])
                .padding(Insets {
                    top: f.at.y.max(0.0),
                    left: f.at.x.max(0.0),
                    bottom: 0.0,
                    right: 0.0,
                })
                .align(Align::Start, Align::Start)
                .key(("floating-text", f.id))
                .enter(
                    Effect::fade()
                        .and(Effect::slide(0.0, 14.0))
                        .transition(Transition::spring(SpringParams::SNAPPY)),
                )
                .exit(
                    Effect::fade()
                        .and(Effect::slide(0.0, -36.0))
                        .transition(Transition::ease(Duration::from_millis(600))),
                )
        })
        .collect();
    Element::layers(layers)
}

/// Characters per second a [`Typewriter`] reveals by default.
pub const TYPEWRITER_SPEED: f32 = 40.0;

/// Text revealed a character at a time, as a dialogue box types it out.
///
/// It reads the time from the frames that show it: the first build that
/// shows it starts the clock ([`Cx::frame_time`]), and each build shows
/// what `speed` characters a second have reached
/// ([`shown_at`](Self::shown_at)). [`advance`](Self::advance) is what
/// the player's "next" does: reveal the rest at once, or, when all of it
/// shows, say so (show the next line).
#[derive(Clone, Debug, PartialEq)]
pub struct Typewriter {
    text: String,
    speed: f32,
    started: Cell<Option<Duration>>,
    shown: Cell<usize>,
    skipped: bool,
}

impl Typewriter {
    /// Type `text` out at [`TYPEWRITER_SPEED`].
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            speed: TYPEWRITER_SPEED,
            started: Cell::new(None),
            shown: Cell::new(0),
            skipped: false,
        }
    }

    /// Type at `chars_per_second` instead.
    pub fn speed(mut self, chars_per_second: f32) -> Self {
        self.speed = chars_per_second.max(0.0);
        self
    }

    /// The whole text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How many characters it has.
    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// How many characters show at frame time `now`. The first call starts
    /// the clock. A speed of 0, a skip, or a target without a clock
    /// (`now` stays zero) shows them all.
    pub fn shown_at(&self, now: Duration) -> usize {
        let total = self.len();
        let shown = if self.skipped || self.speed <= 0.0 || now.is_zero() {
            total
        } else {
            let start = self.started.get().unwrap_or_else(|| {
                self.started.set(Some(now));
                now
            });
            let secs = now.saturating_sub(start).as_secs_f64();
            ((secs * f64::from(self.speed)).floor() as usize).min(total)
        };
        self.shown.set(self.shown.get().max(shown));
        shown
    }

    /// The text showing at `now` (see [`shown_at`](Self::shown_at)).
    pub fn visible_at(&self, now: Duration) -> &str {
        let n = self.shown_at(now);
        match self.text.char_indices().nth(n) {
            Some((i, _)) => &self.text[..i],
            None => &self.text,
        }
    }

    /// Whether the whole text showed in the last frame built.
    pub fn is_done(&self) -> bool {
        self.skipped || self.shown.get() >= self.len()
    }

    /// Reveal the rest at once.
    pub fn skip(&mut self) {
        self.skipped = true;
    }

    /// The player's "next": while it types, reveal the rest and return
    /// `false`; once all of it shows, return `true` (move on).
    pub fn advance(&mut self) -> bool {
        if self.is_done() {
            return true;
        }
        self.skip();
        false
    }
}

/// What a [`DialogueBox`] runs when the player moves on.
type OnActivate<S> = Box<dyn FnMut(&mut S)>;

/// A dialogue box along the bottom of the screen: the speaker's name on a
/// tab ([`speaker`](Self::speaker)), an optional
/// [`portrait`](Self::portrait) (an [`image`](crate::widgets::image), shown
/// 72 × 72), and the [`Typewriter`]'s text as far as it has typed, with a ▼
/// when it is all there. It asks for frames while it types.
///
/// The box is a button that takes focus when it shows (it is a
/// [`spatial_nav`](Element::spatial_nav) group of one): a tap, Enter,
/// Space or a gamepad's South button runs `on_activate`, which calls
/// [`Typewriter::advance`] and shows the next line when that says so.
///
/// ```
/// use stipple::game::ui::{DialogueBox, Typewriter};
/// use stipple::prelude::*;
///
/// struct Game { line: Typewriter }
///
/// fn dialogue(g: &Game, cx: &mut Cx<Game>, theme: &Theme) -> Element {
///     DialogueBox::new(&g.line, |g: &mut Game| {
///         if g.line.advance() {
///             g.line = Typewriter::new("Next line.");
///         }
///     })
///     .speaker("Dispatch")
///     .view(cx)
/// }
/// # let _ = dialogue;
/// ```
pub struct DialogueBox<'a, S> {
    text: &'a Typewriter,
    speaker: Option<String>,
    portrait: Option<Element>,
    on_activate: OnActivate<S>,
}

impl<S> std::fmt::Debug for DialogueBox<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogueBox")
            .field("text", &self.text)
            .field("speaker", &self.speaker)
            .field("portrait", &self.portrait)
            .finish_non_exhaustive()
    }
}

impl<'a, S: 'static> DialogueBox<'a, S> {
    /// A box typing out `text`, running `on_activate` when the player
    /// moves on.
    pub fn new(text: &'a Typewriter, on_activate: impl FnMut(&mut S) + 'static) -> Self {
        Self {
            text,
            speaker: None,
            portrait: None,
            on_activate: Box::new(on_activate),
        }
    }

    /// With `name` on a tab above the box.
    pub fn speaker(mut self, name: impl Into<String>) -> Self {
        self.speaker = Some(name.into());
        self
    }

    /// With `portrait` (an image, say) at the left, 72 × 72.
    pub fn portrait(mut self, portrait: Element) -> Self {
        self.portrait = Some(portrait);
        self
    }

    /// The box's element.
    pub fn view(self, cx: &mut Cx<S>) -> Element {
        let theme = &crate::widgets::theme_of(cx);
        let Self {
            text,
            speaker,
            portrait,
            on_activate,
        } = self;
        let p = &theme.palette;
        let now = cx.frame_time();
        let shown = text.visible_at(now);
        let done = text.is_done();
        if !done {
            cx.request_frame();
        }
        let body = theme.typography.title;
        // The whole text, unseen, holds the box's size while it types.
        let words = Element::layers(vec![
            Element::text(text.text(), body, Color::TRANSPARENT).wrap(),
            Element::text(shown, body, p.text).wrap(),
        ])
        .grow(1.0)
        .shrink(1.0);
        let more = Element::text(
            if done { "\u{25BC}" } else { " " },
            theme.typography.caption,
            p.primary,
        );
        let mut row = Vec::with_capacity(3);
        if let Some(face) = portrait {
            row.push(
                face.width(72.0)
                    .height(72.0)
                    .border(p.primary, 2.0)
                    .radius(theme.radius),
            );
        }
        row.push(words);
        row.push(
            Element::stack(Axis::Vertical, vec![crate::widgets::spacer(), more])
                .align(Align::Start, Align::End),
        );
        let panel = Element::stack(Axis::Horizontal, row)
            .gap(theme.spacing.md)
            .padding(Insets::uniform(theme.spacing.lg));
        let frame = theme
            .skin
            .and_then(|s| s.dialogue.as_ref().or(s.panel.as_ref()));
        let panel = match frame {
            Some(frame) => panel.background_image(frame),
            None => panel
                .fill(Color {
                    a: 235,
                    ..p.surface
                })
                .border(p.primary, 2.0)
                .radius(theme.radius),
        };
        let panel = panel
            .align(Align::Start, Align::Stretch)
            .min_height(96.0)
            .on_tap(cx, on_activate)
            .role(Role::Button)
            .a11y_label(text.text().to_string());
        let mut parts = Vec::with_capacity(2);
        if let Some(name) = speaker {
            let tab = Element::stack(
                Axis::Horizontal,
                vec![Element::text(name, theme.typography.body, p.on_primary).bold()],
            )
            .fill(p.primary)
            .radius(theme.radius)
            .padding(Insets::symmetric(theme.spacing.md, theme.spacing.xs));
            parts.push(
                Element::stack(Axis::Horizontal, vec![tab])
                    .padding(Insets::symmetric(theme.spacing.md, 0.0))
                    .align(Align::Start, Align::Start),
            );
        }
        parts.push(panel);
        Element::stack(Axis::Vertical, parts)
            .shrink(1.0)
            .spatial_nav()
    }
}
