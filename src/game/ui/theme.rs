//! A game's own look: [`GameStyle`] → [`Theme::game`], and the font to go
//! with it ([`App::game_style`]).

use crate::App;
use crate::render::{Color, Font, FontError};
use crate::style::{FocusLook, MotionTokens, Palette, Spacing, Theme, Typography};

/// A game's look, in the few choices a game makes: its font, its colors, its
/// corners and its focus ring. [`theme`](Self::theme) (or [`Theme::game`])
/// makes the [`Theme`] every widget reads, with type sized for a screen
/// read from a sofa; [`App::game_style`] sets that theme and the font at
/// once.
///
/// Start from a preset ([`GameStyle::neon`], [`GameStyle::ember`]) and
/// change what differs:
///
/// ```
/// use stipple::game::ui::GameStyle;
/// use stipple::prelude::*;
/// let style = GameStyle::neon()
///     .with_accent(Color::rgb(0x2e, 0xc4, 0xb6))
///     .with_radius(4.0);
/// let theme = style.theme();
/// assert_eq!(theme.palette.primary, Color::rgb(0x2e, 0xc4, 0xb6));
/// ```
///
/// The font comes from the game's own files (`include_bytes!`, or read at
/// run time), with [`with_font_bytes`](Self::with_font_bytes) or
/// [`with_font`](Self::with_font) for a [`Font`] built with its other faces.
#[derive(Clone, Debug, PartialEq)]
pub struct GameStyle {
    /// The game's font, drawn by every widget (through
    /// [`App::game_style`], or [`App::font`]). `None` keeps the app's.
    pub font: Option<Font>,
    /// Behind everything (and behind a menu shown without the game).
    pub background: Color,
    /// Panels, buttons at rest, the track of a bar.
    pub surface: Color,
    /// The highlight: a focused or hovered button, a slider's fill, the
    /// focus ring.
    pub accent: Color,
    /// Text, and what is drawn on the surface.
    pub text: Color,
    /// Labels and hints that step back.
    pub text_muted: Color,
    /// Health, warnings and failures.
    pub danger: Color,
    /// Healing, success, a full bar.
    pub success: Color,
    /// Gold, coins, a caution.
    pub warning: Color,
    /// Corner radius of panels and buttons, in logical pixels.
    pub radius: f64,
    /// Body text size; captions, titles and headings scale from it.
    pub font_size: f64,
    /// The focus ring: bolder than a desktop app's.
    pub focus: FocusLook,
}

impl GameStyle {
    /// Night-sky blues with a hot pink accent and a glowing focus ring:
    /// an arcade's look.
    pub fn neon() -> Self {
        Self {
            font: None,
            background: Color::rgb(0x0d, 0x0b, 0x1e),
            surface: Color::rgb(0x1c, 0x18, 0x3a),
            accent: Color::rgb(0xff, 0x3d, 0x8b),
            text: Color::rgb(0xf4, 0xf1, 0xff),
            text_muted: Color::rgb(0x9d, 0x96, 0xc8),
            danger: Color::rgb(0xff, 0x4d, 0x5e),
            success: Color::rgb(0x3d, 0xf5, 0xa7),
            warning: Color::rgb(0xff, 0xc8, 0x3d),
            radius: 6.0,
            font_size: 17.0,
            focus: FocusLook::BOLD,
        }
    }

    /// Charcoal and parchment with an amber accent, square-ish corners and
    /// a solid ring without glow: a fantasy or adventure game's look.
    pub fn ember() -> Self {
        Self {
            font: None,
            background: Color::rgb(0x17, 0x13, 0x10),
            surface: Color::rgb(0x2a, 0x22, 0x1b),
            accent: Color::rgb(0xe8, 0x9a, 0x2c),
            text: Color::rgb(0xf3, 0xe6, 0xcc),
            text_muted: Color::rgb(0xa8, 0x97, 0x7c),
            danger: Color::rgb(0xd9, 0x48, 0x3b),
            success: Color::rgb(0x7f, 0xbf, 0x5a),
            warning: Color::rgb(0xf2, 0xc1, 0x4e),
            radius: 2.0,
            font_size: 17.0,
            focus: FocusLook {
                width: 3.0,
                offset: 3.0,
                glow: 0.0,
            },
        }
    }

    /// Use `font` for every widget.
    pub fn with_font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Use the font in `bytes` (a TrueType `.ttf`, from the game's files)
    /// for every widget.
    pub fn with_font_bytes(self, bytes: Vec<u8>) -> Result<Self, FontError> {
        Ok(self.with_font(Font::from_bytes(bytes)?))
    }

    /// Set the accent (the highlight and the focus ring).
    pub fn with_accent(mut self, accent: Color) -> Self {
        self.accent = accent;
        self
    }

    /// Set the corner radius.
    pub fn with_radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// Set the body text size (the rest of the type scale follows).
    pub fn with_font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self
    }

    /// Set the focus ring's look.
    pub fn with_focus(mut self, focus: FocusLook) -> Self {
        self.focus = focus;
        self
    }

    /// The [`Theme`] this style makes (see [`Theme::game`]).
    pub fn theme(&self) -> Theme {
        let dark = self.background.luminance() < 0.5;
        let (hover, active) = if dark {
            (self.accent.lighten(0.12), self.accent.darken(0.12))
        } else {
            (self.accent.darken(0.08), self.accent.darken(0.18))
        };
        let a = self.accent;
        let size = self.font_size;
        Theme {
            palette: Palette {
                background: self.background,
                surface: self.surface,
                primary: a,
                primary_hover: hover,
                primary_active: active,
                on_primary: a.on_color(),
                text: self.text,
                text_muted: self.text_muted,
                border: self.surface.mix(self.text, 0.22),
                success: self.success,
                warning: self.warning,
                danger: self.danger,
                focus_ring: a.lighten(0.15),
                hover_overlay: if dark {
                    Color::rgba(255, 255, 255, 26)
                } else {
                    Color::rgba(0, 0, 0, 20)
                },
                selection: Color::rgba(a.r, a.g, a.b, 90),
            },
            typography: Typography {
                caption: (size * 0.8).round(),
                body: size,
                title: (size * 1.3).round(),
                heading: (size * 2.0).round(),
            },
            spacing: Spacing {
                xs: 6.0,
                sm: 10.0,
                md: 14.0,
                lg: 20.0,
                xl: 32.0,
            },
            radius: self.radius,
            font_size: size,
            native: false,
            follows_system: false,
            layout_direction: crate::layout::LayoutDirection::Ltr,
            motion: MotionTokens::standard(),
            focus: self.focus,
        }
    }
}

impl Default for GameStyle {
    /// [`GameStyle::neon`].
    fn default() -> Self {
        Self::neon()
    }
}

impl Theme {
    /// A game's theme, from its [`GameStyle`]: its colors, corners, focus
    /// ring and a type scale for the sofa. The style's font goes to
    /// [`App::font`] (or use [`App::game_style`] for both).
    pub fn game(style: &GameStyle) -> Theme {
        style.theme()
    }
}

impl<S: 'static> App<S> {
    /// Dress the app in a game's [`GameStyle`]: its [`theme`](GameStyle::theme)
    /// and, if it has one, its font.
    pub fn game_style(self, style: GameStyle) -> Self {
        let app = self.theme(style.theme());
        match style.font {
            Some(font) => app.font(font),
            None => app,
        }
    }
}
