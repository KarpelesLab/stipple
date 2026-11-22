//! Image skins: bitmaps that dress the built-in widgets in place of the
//! theme's fills and borders. See [`Skin`].

use crate::core::{Element, Interaction};
use crate::render::NineSlice;

/// Images that dress the built-in widgets instead of the theme's colors: the
/// painted frames of a game's buttons, panels, dialogue box, bars and
/// toggles. Put one in a [`Theme`](crate::style::Theme) with
/// [`Theme::skin`](crate::style::Theme::skin) (or
/// [`GameStyle::skin`](crate::game::ui::GameStyle::skin)), and the widgets
/// that have an image here draw it; the others keep the theme's look.
///
/// | Field | Dresses |
/// |---|---|
/// | [`button`](Self::button) | [`button`](crate::widgets::button) / [`Button`](crate::widgets::Button) (all variants), [`menu_button`](crate::game::ui::menu_button) |
/// | [`panel`](Self::panel) | [`panel`](crate::widgets::panel), [`dialog`](crate::widgets::dialog)'s card, [`pause_menu`](crate::game::ui::pause_menu), [`GameOverScreen`](crate::game::ui::GameOverScreen)'s results |
/// | [`dialogue`](Self::dialogue) | [`DialogueBox`](crate::game::ui::DialogueBox) (the panel's when unset) |
/// | [`bar`](Self::bar) | [`stat_bar`](crate::game::ui::stat_bar) / [`health_bar`](crate::game::ui::health_bar), [`progress_bar`](crate::widgets::progress_bar) |
/// | [`checkbox`](Self::checkbox) | [`checkbox`](crate::widgets::checkbox)'s box |
/// | [`switch`](Self::switch) | [`switch`](crate::widgets::switch)'s track and knob |
/// | [`scroll_bar`](Self::scroll_bar) | every scroll container's bar: its track, thumb, arrow buttons and corner |
///
/// A theme is a plain `Copy` value, so it holds a skin by `&'static`
/// reference: build the skin once, when the game starts, and
/// [`leak`](Self::leak) it (or keep it in a `static`). Don't build one per
/// frame.
///
/// ```
/// use std::sync::Arc;
/// use stipple::prelude::*;
/// use stipple::render::{NineSlice, Pixmap};
/// use stipple::style::{Skin, StateSkin};
///
/// let atlas = Arc::new(Pixmap::new(PhysicalSize::new(64, 32)));
/// let part = |x: f64| NineSlice::new(atlas.clone(), Insets::uniform(5.0))
///     .region(Rect::from_xywh(x, 0.0, 16.0, 16.0));
/// let skin = Skin::new()
///     .button(StateSkin::new(part(0.0)).hovered(part(16.0)).pressed(part(32.0)))
///     .panel(part(48.0))
///     .leak();
/// let theme = Theme::dark().skin(skin);
/// assert!(theme.skin.is_some_and(|s| s.button.is_some()));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Skin {
    /// Buttons, per state.
    pub button: Option<StateSkin>,
    /// Panels and cards.
    pub panel: Option<NineSlice>,
    /// A game's dialogue box; `None` uses [`panel`](Self::panel).
    pub dialogue: Option<NineSlice>,
    /// Bars: a track and its fill.
    pub bar: Option<BarSkin>,
    /// A checkbox's box, unchecked and checked (the image shows the mark).
    pub checkbox: Option<ToggleSkin>,
    /// A switch's track, off and on, and its knob.
    pub switch: Option<ToggleSkin>,
    /// Scroll bars: their track, thumb, arrow buttons and the corner
    /// between two bars.
    pub scroll_bar: Option<ScrollBarSkin>,
}

impl Skin {
    /// A skin dressing nothing yet: every widget keeps the theme's look.
    pub fn new() -> Self {
        Self::default()
    }

    /// Dress buttons in `skin`.
    pub fn button(mut self, skin: StateSkin) -> Self {
        self.button = Some(skin);
        self
    }

    /// Dress panels in `frame`.
    pub fn panel(mut self, frame: NineSlice) -> Self {
        self.panel = Some(frame);
        self
    }

    /// Dress a game's dialogue box in `frame`.
    pub fn dialogue(mut self, frame: NineSlice) -> Self {
        self.dialogue = Some(frame);
        self
    }

    /// Dress bars in `skin`.
    pub fn bar(mut self, skin: BarSkin) -> Self {
        self.bar = Some(skin);
        self
    }

    /// Dress checkboxes in `skin`.
    pub fn checkbox(mut self, skin: ToggleSkin) -> Self {
        self.checkbox = Some(skin);
        self
    }

    /// Dress switches in `skin`.
    pub fn switch(mut self, skin: ToggleSkin) -> Self {
        self.switch = Some(skin);
        self
    }

    /// Dress scroll bars in `skin`.
    pub fn scroll_bar(mut self, skin: ScrollBarSkin) -> Self {
        self.scroll_bar = Some(skin);
        self
    }

    /// This skin, kept for the rest of the program, for a
    /// [`Theme`](crate::style::Theme) to hold. Call it once (it is never
    /// freed).
    pub fn leak(self) -> &'static Skin {
        Box::leak(Box::new(self))
    }
}

/// A control's frame per interaction state: the [`normal`](Self::normal)
/// one, and those shown instead while it is hovered, focused from the
/// keyboard or a gamepad, pressed or disabled (each `None` keeps the one
/// before it). Put it on any element with [`apply`](Self::apply).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct StateSkin {
    /// At rest.
    pub normal: NineSlice,
    /// Under the pointer.
    pub hovered: Option<NineSlice>,
    /// Focused from the keyboard or a gamepad (a menu's current item).
    pub focused: Option<NineSlice>,
    /// Held down.
    pub pressed: Option<NineSlice>,
    /// [Disabled](Element::disabled).
    pub disabled: Option<NineSlice>,
}

impl StateSkin {
    /// `normal` in every state, until others are given.
    pub fn new(normal: NineSlice) -> Self {
        Self {
            normal,
            hovered: None,
            focused: None,
            pressed: None,
            disabled: None,
        }
    }

    /// Show `frame` while hovered.
    pub fn hovered(mut self, frame: NineSlice) -> Self {
        self.hovered = Some(frame);
        self
    }

    /// Show `frame` while focused from the keyboard or a gamepad.
    pub fn focused(mut self, frame: NineSlice) -> Self {
        self.focused = Some(frame);
        self
    }

    /// Show `frame` while pressed.
    pub fn pressed(mut self, frame: NineSlice) -> Self {
        self.pressed = Some(frame);
        self
    }

    /// Show `frame` while disabled.
    pub fn disabled(mut self, frame: NineSlice) -> Self {
        self.disabled = Some(frame);
        self
    }

    /// The frame shown in `states` (what [`apply`](Self::apply) swaps in), for
    /// a widget that reads its states itself
    /// ([`Cx::interaction`](crate::core::Cx::interaction)).
    pub fn frame(&self, states: Interaction) -> &NineSlice {
        [
            (states.hovered(), &self.hovered),
            (states.focus_visible(), &self.focused),
            (states.pressed(), &self.pressed),
            (states.disabled(), &self.disabled),
        ]
        .into_iter()
        .filter_map(|(on, f)| f.as_ref().filter(|_| on))
        .next_back()
        .unwrap_or(&self.normal)
    }

    /// `el` with the normal frame as its
    /// [background](Element::background_image) and per-state
    /// overrides swapping in the others (hovered, then focused, then
    /// pressed, then disabled: the later wins). Its fill and border go: the
    /// frame is the whole look. (An element with a background image never
    /// gets the theme's hover highlight: its frames show the hover.)
    pub fn apply(&self, el: Element) -> Element {
        let mut el = el.background_image(&self.normal);
        el.decoration.fill = None;
        el.decoration.border = None;
        for (state, frame) in [
            (Interaction::HOVERED, &self.hovered),
            (Interaction::FOCUS_VISIBLE, &self.focused),
            (Interaction::PRESSED, &self.pressed),
            (Interaction::DISABLED, &self.disabled),
        ] {
            if let Some(frame) = frame.clone() {
                el = el.when(state, move |e| e.background_image(frame));
            }
        }
        el
    }
}

impl From<NineSlice> for StateSkin {
    fn from(normal: NineSlice) -> Self {
        StateSkin::new(normal)
    }
}

/// A bar's images: the [`track`](Self::track) frame, and the
/// [`fill`](Self::fill) drawn inside the track's content padding, as long as
/// the value. The fill is revealed (cut off at the value, so its ends keep
/// their shape until the very end), or [stretched](Self::stretch) to the
/// value's length.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BarSkin {
    /// The frame behind the fill.
    pub track: NineSlice,
    /// The full part.
    pub fill: NineSlice,
    /// Stretch the fill to the value's length instead of revealing as much
    /// of a full-length one.
    pub stretch: bool,
    /// Multiply the fill by the bar's color (a [`Bar`](crate::game::ui::Bar)'s
    /// [`color`](crate::game::ui::Bar::color), the theme's primary for a
    /// progress bar), so one fill drawn in grays makes the health bar red
    /// and the mana bar blue. On by default; off draws the fill as it is.
    pub tint: bool,
}

impl BarSkin {
    /// `fill` revealed over `track`.
    pub fn new(track: NineSlice, fill: NineSlice) -> Self {
        Self {
            track,
            fill,
            stretch: false,
            tint: true,
        }
    }

    /// Multiply the fill by the bar's color (`true`, the default) or draw it
    /// as it is.
    pub fn tint(mut self, tint: bool) -> Self {
        self.tint = tint;
        self
    }

    /// Stretch the fill to the value's length (`true`) or reveal it
    /// (`false`, the default).
    pub fn stretch(mut self, stretch: bool) -> Self {
        self.stretch = stretch;
        self
    }

    /// The fill of a bar whose inside is `inner` (width, height), filled
    /// `filled` logical pixels across, for a bar in `color`.
    pub(crate) fn fill_element(
        &self,
        inner: (f64, f64),
        filled: f64,
        color: crate::render::Color,
    ) -> Element {
        let (w, h) = inner;
        let fill = if self.tint {
            self.fill.clone().tint(color)
        } else {
            self.fill.clone()
        };
        if self.stretch {
            // Stretched: the frame drawn at the value's length.
            return Element::stack(crate::core::Axis::Horizontal, Vec::new())
                .background_image(&fill)
                .padding(crate::geometry::Insets::ZERO)
                .width(filled)
                .height(h);
        }
        // Revealed: a full-length fill, clipped to the value's length.
        let full = Element::stack(crate::core::Axis::Horizontal, Vec::new())
            .background_image(&fill)
            .padding(crate::geometry::Insets::ZERO)
            .width(w)
            .height(h)
            .shrink(0.0);
        Element::stack(crate::core::Axis::Horizontal, vec![full])
            .width(filled)
            .height(h)
            .clip()
    }
}

/// A two-state control's images: its frame [`off`](Self::off) and
/// [`on`](Self::on), and for a switch a [`knob`](Self::knob) sliding over
/// it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ToggleSkin {
    /// Unchecked, or off.
    pub off: NineSlice,
    /// Checked, or on.
    pub on: NineSlice,
    /// A switch's knob; `None` keeps the plain one.
    pub knob: Option<NineSlice>,
}

impl ToggleSkin {
    /// `off` and `on`.
    pub fn new(off: NineSlice, on: NineSlice) -> Self {
        Self {
            off,
            on,
            knob: None,
        }
    }

    /// A switch's knob.
    pub fn knob(mut self, knob: NineSlice) -> Self {
        self.knob = Some(knob);
        self
    }

    /// The frame for `on`.
    pub fn frame(&self, on: bool) -> &NineSlice {
        if on { &self.on } else { &self.off }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::core::{Cx, Interaction, layout};
    use crate::geometry::{Insets, Point, Rect, ScaleFactor, Size};
    use crate::render::nine_slice::test_frame;
    use crate::render::{BackgroundImage, Color, ImageFit, Pixmap, Scene, SoftwareRenderer};
    use crate::style::Theme;
    use std::sync::Arc;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    /// A 9×9 frame: a 3-pixel ring in `ring` round a `middle`.
    fn frame(ring: [u8; 4], middle: [u8; 4]) -> NineSlice {
        NineSlice::new(test_frame(9, 3, [ring, ring, middle]), Insets::uniform(3.0)).nearest()
    }

    fn skin() -> &'static Skin {
        Skin::new()
            .button(
                StateSkin::new(frame(RED, WHITE))
                    .hovered(frame(GREEN, WHITE))
                    .pressed(frame(BLUE, WHITE)),
            )
            .panel(frame(BLUE, WHITE))
            .bar(BarSkin::new(frame(BLUE, BLUE), frame(WHITE, WHITE)))
            .checkbox(ToggleSkin::new(frame(RED, RED), frame(GREEN, GREEN)))
            .leak()
    }

    fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        p.pixel(x, y).unwrap()
    }

    fn paint(el: &Element, size: Size) -> Pixmap {
        let mut scene = Scene::new(size);
        let tree = layout(el, Rect::from_xywh(0.0, 0.0, size.width, size.height), None);
        crate::core::paint(&tree, &mut scene, None);
        SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY)
    }

    #[test]
    fn a_state_skin_picks_the_latest_frame_given() {
        let s = StateSkin::new(frame(RED, WHITE)).pressed(frame(BLUE, WHITE));
        assert_eq!(s.frame(Interaction::NONE), &s.normal);
        assert_eq!(s.frame(Interaction::HOVERED), &s.normal, "no hovered frame");
        let held = Interaction::HOVERED | Interaction::PRESSED;
        assert_eq!(Some(s.frame(held)), s.pressed.as_ref());
    }

    #[test]
    fn a_skinned_button_swaps_its_frame_on_hover() {
        let theme = Theme::dark().skin(skin());
        let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
            crate::widgets::Button::new("Go")
                .view(cx)
                .width(60.0)
                .height(30.0)
                .on_tap(cx, |_| {})
        })
        .no_font()
        .theme(theme)
        .logical_size(Size::new(60.0, 30.0));
        let out = app.render_once();
        assert_eq!(rgba(&out, 1, 1), RED, "the frame at rest");
        assert_eq!(rgba(&out, 30, 15), WHITE, "its middle");
        app.hover_at(Point::new(30.0, 15.0));
        let out = app.render_once();
        // The hovered frame, without the theme's hover highlight over it.
        assert_eq!(rgba(&out, 1, 1), GREEN, "hovered");
        assert_eq!(rgba(&out, 30, 15), WHITE);
    }

    #[test]
    fn panels_and_checkboxes_wear_the_skin() {
        let theme = Theme::light().skin(skin());
        let mut cx = Cx::new(&theme);
        let panel = crate::widgets::panel(&cx);
        assert!(panel.background.is_some());
        assert_eq!(panel.decoration.fill, None);
        assert!(panel.layout.padding.left >= 3.0, "content off the frame");
        let off = crate::widgets::Toggle::checkbox("A", false).view(&cx);
        let on = crate::widgets::Toggle::checkbox("A", true).view(&cx);
        let out = paint(&off, Size::new(22.0, 22.0));
        assert_eq!(rgba(&out, 11, 11), RED);
        let out = paint(&on, Size::new(22.0, 22.0));
        assert_eq!(rgba(&out, 11, 11), GREEN);
        let _ = crate::widgets::checkbox(&mut cx, "A", true, |_: &mut ()| {});
        // Unskinned, they keep the palette.
        let plain = Theme::light();
        let panel = crate::widgets::panel(&Cx::<()>::new(&plain));
        assert!(panel.background.is_none());
        assert!(panel.decoration.fill.is_some());
    }

    #[test]
    fn a_skinned_bar_reveals_its_fill() {
        let theme = Theme::light().skin(skin());
        let cx = Cx::<()>::new(&theme);
        let bar = crate::game::ui::Bar::new(5.0, 10.0, Color::rgb(255, 0, 0))
            .ghost(None)
            .size(106.0, 26.0);
        let el = crate::game::ui::stat_bar(&cx, &bar);
        let out = paint(&el, Size::new(120.0, 30.0));
        // The track's frame, then the fill (white, tinted red) over the
        // first half of the 100 inside, then the track's blue middle.
        assert_eq!(rgba(&out, 1, 1), BLUE);
        assert_eq!(rgba(&out, 10, 13), RED);
        assert_eq!(rgba(&out, 52, 13), RED);
        assert_eq!(rgba(&out, 56, 13), BLUE, "past the value");
    }

    #[test]
    fn a_plain_image_background_covers_the_box() {
        let px = Arc::new(Pixmap::from_rgba8(
            crate::geometry::PhysicalSize::new(1, 1),
            GREEN.to_vec(),
        ));
        let el = Element::stack(crate::layout::Axis::Vertical, Vec::new())
            .background_image(BackgroundImage::image(&px, ImageFit::Fill))
            .width(10.0)
            .height(10.0);
        assert_eq!(el.layout.padding, Insets::ZERO);
        let out = paint(&el, Size::new(10.0, 10.0));
        assert_eq!(rgba(&out, 5, 5), GREEN);
    }

    #[test]
    fn swapping_a_frame_damages_the_element() {
        let el = |f: NineSlice| {
            Element::stack(crate::layout::Axis::Vertical, Vec::new())
                .background_image(f)
                .width(20.0)
                .height(20.0)
        };
        let bounds = Rect::from_xywh(0.0, 0.0, 40.0, 40.0);
        let (red, green) = (frame(RED, WHITE), frame(GREEN, WHITE));
        let a = layout(&el(red.clone()), bounds, None);
        let same = layout(&el(red), bounds, None);
        let b = layout(&el(green), bounds, None);
        use crate::core::Damage;
        assert_eq!(crate::core::diff::diff_trees(&a, &same), Damage::None);
        assert_ne!(crate::core::diff::diff_trees(&a, &b), Damage::None);
    }

    #[test]
    fn a_game_style_carries_its_skin_into_the_theme() {
        let s = skin();
        let style = crate::game::ui::GameStyle::ember().skin(s);
        assert!(style.theme().skin.is_some_and(|t| std::ptr::eq(t, s)));
        assert!(crate::game::ui::GameStyle::neon().theme().skin.is_none());
    }
}

/// A scroll bar's images, each part's drawn in place of the theme's colors
/// (a part left `None` keeps them): the [`track`](Self::track) along the
/// whole strip, the [`thumb`](Self::thumb) (hovered and pressed frames in
/// its [`StateSkin`]), the four [arrow buttons](ScrollBarArrows) where the
/// [`ScrollBarLook`](crate::style::ScrollBarLook) shows them, and the
/// [`corner`](Self::corner) square where a vertical and a horizontal bar
/// meet. The frames stretch as nine-slices, so one image serves both
/// orientations of the track and the thumb.
///
/// ```
/// use std::sync::Arc;
/// use stipple::prelude::*;
/// use stipple::render::{NineSlice, Pixmap};
/// use stipple::style::{ScrollBarSkin, Skin, StateSkin};
///
/// let atlas = Arc::new(Pixmap::new(PhysicalSize::new(48, 16)));
/// let part = |x: f64| NineSlice::new(atlas.clone(), Insets::uniform(4.0))
///     .region(Rect::from_xywh(x, 0.0, 16.0, 16.0));
/// let skin = Skin::new()
///     .scroll_bar(ScrollBarSkin::new().track(part(0.0)).thumb(
///         StateSkin::new(part(16.0)).pressed(part(32.0)),
///     ))
///     .leak();
/// let theme = Theme::light().skin(skin);
/// assert!(theme.skin.is_some_and(|s| s.scroll_bar.is_some()));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ScrollBarSkin {
    /// The strip behind the thumb.
    pub track: Option<NineSlice>,
    /// The thumb, per state (hovered under the pointer, pressed while
    /// dragged).
    pub thumb: Option<StateSkin>,
    /// The arrow buttons.
    pub arrows: Option<ScrollBarArrows>,
    /// The square between a vertical and a horizontal bar.
    pub corner: Option<NineSlice>,
}

impl ScrollBarSkin {
    /// No images yet: every part keeps the theme's look.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw the track as `frame`.
    pub fn track(mut self, frame: NineSlice) -> Self {
        self.track = Some(frame);
        self
    }

    /// Draw the thumb in `skin`.
    pub fn thumb(mut self, skin: StateSkin) -> Self {
        self.thumb = Some(skin);
        self
    }

    /// Draw the arrow buttons as `arrows`.
    pub fn arrows(mut self, arrows: ScrollBarArrows) -> Self {
        self.arrows = Some(arrows);
        self
    }

    /// Draw the corner between two bars as `frame`.
    pub fn corner(mut self, frame: NineSlice) -> Self {
        self.corner = Some(frame);
        self
    }
}

/// A scroll bar's four arrow buttons, each per state (see [`ScrollBarSkin`]);
/// the images show the arrows. Right to left, a horizontal bar's start is on
/// the right, and the button there is still [`right`](Self::right).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScrollBarArrows {
    /// The top button of a vertical bar.
    pub up: StateSkin,
    /// The bottom button of a vertical bar.
    pub down: StateSkin,
    /// The left button of a horizontal bar.
    pub left: StateSkin,
    /// The right button of a horizontal bar.
    pub right: StateSkin,
}

impl ScrollBarArrows {
    /// The four buttons.
    pub fn new(up: StateSkin, down: StateSkin, left: StateSkin, right: StateSkin) -> Self {
        Self {
            up,
            down,
            left,
            right,
        }
    }
}

/// The frame of `skin` for a part `hovered` or `pressed`.
pub(crate) fn part_frame(skin: &StateSkin, hovered: bool, pressed: bool) -> &NineSlice {
    if pressed && let Some(f) = &skin.pressed {
        return f;
    }
    if (hovered || pressed)
        && let Some(f) = &skin.hovered
    {
        return f;
    }
    &skin.normal
}
