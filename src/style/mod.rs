//! Design tokens and themes for Stipple's self-drawn widgets.
//!
//! Because every control is drawn by Stipple (not the OS), a single [`Theme`]
//! value defines the entire look: a semantic [`Palette`] (roles + interaction
//! states + status colors + overlays), a [`Typography`] scale, a [`Spacing`]
//! scale, and corner [`radius`](Theme::radius). Widgets read tokens from the
//! theme rather than hard-coding values, so re-skinning is swapping the theme —
//! and the [builder methods](Theme::with_accent) make a custom theme a
//! one-liner.

#![forbid(unsafe_code)]

use crate::platform::native;
use crate::render::Color;

/// Semantic colors. Names describe *role*, not hue, so light/dark/custom themes
/// can supply different values for the same role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Window/background fill.
    pub background: Color,
    /// Raised surface (cards, panels).
    pub surface: Color,
    /// Primary accent (default button, selection).
    pub primary: Color,
    /// Primary under hover.
    pub primary_hover: Color,
    /// Primary while pressed/active.
    pub primary_active: Color,
    /// Content drawn on top of `primary`.
    pub on_primary: Color,
    /// Default text/icon color on `background`/`surface`.
    pub text: Color,
    /// De-emphasized text.
    pub text_muted: Color,
    /// Hairline borders and dividers.
    pub border: Color,
    /// Positive / success status.
    pub success: Color,
    /// Caution / warning status.
    pub warning: Color,
    /// Destructive / error status.
    pub danger: Color,
    /// Focus-ring color (the App draws it around the focused element).
    pub focus_ring: Color,
    /// Translucent overlay the App composites on a hovered element.
    pub hover_overlay: Color,
    /// Translucent highlight behind selected text.
    pub selection: Color,
}

/// A type scale, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Typography {
    pub caption: f64,
    pub body: f64,
    pub title: f64,
    pub heading: f64,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            caption: 12.0,
            body: 14.0,
            title: 18.0,
            heading: 24.0,
        }
    }
}

/// A spacing scale in logical pixels. A small fixed scale keeps layouts
/// rhythmically consistent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spacing {
    pub xs: f64,
    pub sm: f64,
    pub md: f64,
    pub lg: f64,
    pub xl: f64,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            xs: 4.0,
            sm: 8.0,
            md: 12.0,
            lg: 16.0,
            xl: 24.0,
        }
    }
}

/// A complete look: palette + typography + spacing + corner radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub palette: Palette,
    pub typography: Typography,
    pub spacing: Spacing,
    /// Default corner radius for rounded surfaces, in logical pixels.
    pub radius: f64,
    /// Base font size in logical pixels (mirrors `typography.body`).
    pub font_size: f64,
    /// Whether widgets ask the OS to draw their chrome. See [`Theme::native`].
    pub native: bool,
}

/// Derive the hover/active/ring tints from a `primary` accent.
fn states(primary: Color, dark: bool) -> (Color, Color) {
    if dark {
        (primary.lighten(0.12), primary.darken(0.12))
    } else {
        (primary.darken(0.08), primary.darken(0.18))
    }
}

impl Theme {
    /// The platform's own look. On Windows, macOS and iOS the OS's UI framework
    /// draws the controls; elsewhere (Linux has no native framework) they stay
    /// self-drawn. Everywhere, it follows the system's light/dark mode and accent
    /// color and uses the platform's metrics.
    ///
    /// Unlike the other themes, this one is deliberately not pixel-identical
    /// across platforms. Colors are read once, when this is called.
    pub fn native() -> Self {
        Self::native_for(native::system_look())
    }

    fn native_for(look: native::SystemLook) -> Self {
        let base = if look.dark {
            Self::dark()
        } else {
            Self::light()
        };
        let mut theme = match look.accent {
            Some(accent) => base.with_accent(accent),
            None => base,
        };
        // Window background, control surface, and separator of each platform.
        #[cfg(target_os = "windows")]
        let (metrics, light, dark) = (
            (4.0, 14.0),
            (0xf3f3f3, 0xfbfbfb, 0xd0d0d0),
            (0x202020, 0x2d2d2d, 0x434343),
        );
        #[cfg(target_os = "macos")]
        let (metrics, light, dark) = (
            (6.0, 13.0),
            (0xececec, 0xffffff, 0xd0d0d0),
            (0x1e1e1e, 0x2d2d2d, 0x3d3d3d),
        );
        #[cfg(target_os = "ios")]
        let (metrics, light, dark) = (
            (10.0, 17.0),
            (0xf2f2f7, 0xffffff, 0xc6c6c8),
            (0x000000, 0x1c1c1e, 0x38383a),
        );
        // GNOME's libadwaita colors, the closest thing Linux has to a default.
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios")))]
        let (metrics, light, dark) = (
            (6.0, 14.0),
            (0xfafafb, 0xffffff, 0xd9d9dc),
            (0x222226, 0x2e2e32, 0x3f3f43),
        );
        let (background, surface, border) = if look.dark { dark } else { light };
        let color = |hex: u32| Color::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        theme.palette.background = color(background);
        theme.palette.surface = color(surface);
        theme.palette.border = color(border);
        theme.radius = metrics.0;
        theme.native = native::available();
        theme.with_font_size(metrics.1)
    }

    /// Whether this theme's background is dark, so windows can match it.
    pub fn is_dark(&self) -> bool {
        self.palette.background.luminance() < 0.5
    }

    /// How the OS should draw `control` under this theme, or `None` if the widget
    /// should draw itself.
    pub fn native_deco(&self, control: native::NativeControl) -> Option<native::NativeDeco> {
        (self.native && native::supports(control)).then(|| native::NativeDeco {
            control,
            dark: self.is_dark(),
            state: native::NativeState::default(),
        })
    }

    /// The default light theme.
    pub fn light() -> Self {
        let primary = Color::rgb(0x3b, 0x82, 0xf6);
        let (primary_hover, primary_active) = states(primary, false);
        Self {
            palette: Palette {
                background: Color::rgb(0xf7, 0xf7, 0xf8),
                surface: Color::WHITE,
                primary,
                primary_hover,
                primary_active,
                on_primary: Color::WHITE,
                text: Color::rgb(0x1a, 0x1a, 0x1e),
                text_muted: Color::rgb(0x6b, 0x70, 0x80),
                border: Color::rgb(0xe2, 0xe4, 0xe9),
                success: Color::rgb(0x22, 0xa5, 0x5a),
                warning: Color::rgb(0xe0, 0x8a, 0x00),
                danger: Color::rgb(0xdc, 0x3a, 0x3a),
                focus_ring: primary,
                hover_overlay: Color::rgba(0, 0, 0, 18),
                selection: Color::rgba(0x3b, 0x82, 0xf6, 60),
            },
            typography: Typography::default(),
            spacing: Spacing::default(),
            radius: 8.0,
            font_size: 14.0,
            native: false,
        }
    }

    /// The default dark theme.
    pub fn dark() -> Self {
        let primary = Color::rgb(0x60, 0x9c, 0xff);
        let (primary_hover, primary_active) = states(primary, true);
        Self {
            palette: Palette {
                background: Color::rgb(0x14, 0x15, 0x18),
                surface: Color::rgb(0x1e, 0x20, 0x25),
                primary,
                primary_hover,
                primary_active,
                on_primary: Color::rgb(0x0a, 0x0b, 0x0d),
                text: Color::rgb(0xec, 0xee, 0xf2),
                text_muted: Color::rgb(0x9a, 0xa0, 0xae),
                border: Color::rgb(0x2c, 0x2f, 0x36),
                success: Color::rgb(0x35, 0xc7, 0x76),
                warning: Color::rgb(0xf5, 0x9e, 0x0b),
                danger: Color::rgb(0xf2, 0x55, 0x55),
                focus_ring: primary,
                hover_overlay: Color::rgba(255, 255, 255, 28),
                selection: Color::rgba(0x60, 0x9c, 0xff, 80),
            },
            typography: Typography::default(),
            spacing: Spacing::default(),
            radius: 8.0,
            font_size: 14.0,
            native: false,
        }
    }

    /// Recolor the theme around a new `accent`: sets `primary`, derives its
    /// hover/active tints + focus ring, and picks a readable `on_primary`.
    pub fn with_accent(mut self, accent: Color) -> Self {
        let dark = self.palette.background.luminance() < 0.5;
        let (hover, active) = states(accent, dark);
        self.palette.primary = accent;
        self.palette.primary_hover = hover;
        self.palette.primary_active = active;
        self.palette.on_primary = accent.on_color();
        self.palette.focus_ring = accent;
        // Tint the text selection with the accent, keeping the prior opacity.
        let a = self.palette.selection.a;
        self.palette.selection = Color::rgba(accent.r, accent.g, accent.b, a);
        self
    }

    /// Set the default corner radius.
    pub fn with_radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// Set the base font size (also updates `typography.body`).
    pub fn with_font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self.typography.body = size;
        self
    }

    /// A higher-contrast variant: pure black/white text on a near-pure
    /// background, square-ish corners, and bolder borders. Useful for
    /// accessibility-leaning skins.
    pub fn high_contrast(mut self) -> Self {
        let dark = self.palette.background.luminance() < 0.5;
        if dark {
            self.palette.background = Color::BLACK;
            self.palette.surface = Color::rgb(0x10, 0x10, 0x10);
            self.palette.text = Color::WHITE;
            self.palette.border = Color::rgb(0xc0, 0xc0, 0xc0);
        } else {
            self.palette.background = Color::WHITE;
            self.palette.surface = Color::WHITE;
            self.palette.text = Color::BLACK;
            self.palette.border = Color::BLACK;
        }
        self.palette.text_muted = self.palette.text;
        self.radius = 2.0;
        self
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::light()
    }
}

// ---- Material 3 (Material You) dynamic color --------------------------------
//
// Material 3 derives a whole color scheme from a single seed color via *tonal
// palettes*: for each key color (primary, neutral, neutral-variant, error) a
// ramp of tones 0..100 is generated, and each semantic role picks a fixed tone
// (e.g. light `primary` = tone 40, `onPrimary` = tone 100). A "tone" in M3 is
// defined as **CIELAB L\***, so we build the ramps with real Lab color math; the
// hue/chroma are M3's CAM16 quantities, which we approximate with Lab hue/chroma
// (close for UI seeds, and fully self-consistent). The result maps onto Stipple's
// existing [`Palette`] roles, so every widget themes correctly with no changes.

/// Material 3's default seed (`#6750A4`) — the baseline "Material You" purple.
const M3_SEED: Color = Color::rgb(0x67, 0x50, 0xa4);

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// D65 reference white.
const XN: f64 = 0.95047;
const ZN: f64 = 1.08883;

fn lab_f(t: f64) -> f64 {
    if t > 0.008856 {
        t.cbrt()
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

fn lab_f_inv(t: f64) -> f64 {
    let t3 = t * t * t;
    if t3 > 0.008856 {
        t3
    } else {
        (t - 16.0 / 116.0) / 7.787
    }
}

/// sRGB color → CIELAB `(L*, a*, b*)`.
fn color_to_lab(c: Color) -> (f64, f64, f64) {
    let r = srgb_to_linear(c.r as f64 / 255.0);
    let g = srgb_to_linear(c.g as f64 / 255.0);
    let b = srgb_to_linear(c.b as f64 / 255.0);
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / XN;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / ZN;
    let (fx, fy, fz) = (lab_f(x), lab_f(y), lab_f(z));
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

/// CIELAB `(L*, a*, b*)` → sRGB color, clipping out-of-gamut channels.
fn lab_to_color(l: f64, a: f64, b: f64) -> Color {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let x = XN * lab_f_inv(fx);
    let y = lab_f_inv(fy);
    let z = ZN * lab_f_inv(fz);
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let bl = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    let to8 = |v: f64| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Color::rgb(to8(r), to8(g), to8(bl))
}

/// A Material 3 tonal palette: a fixed hue + chroma, sampled at any tone (L\*).
#[derive(Clone, Copy)]
struct Tonal {
    hue: f64,
    chroma: f64,
}

impl Tonal {
    /// Take the hue from `seed`, with chroma at least `min_chroma` (M3 floors the
    /// key palettes' chroma so a desaturated seed still yields a usable accent).
    fn from_seed(seed: Color, min_chroma: f64) -> Self {
        let (_l, a, b) = color_to_lab(seed);
        Self {
            hue: b.atan2(a),
            chroma: (a * a + b * b).sqrt().max(min_chroma),
        }
    }

    /// `seed`'s hue at a fixed (usually low) chroma — for the neutral ramps.
    fn neutral_of(seed: Color, chroma: f64) -> Self {
        let (_l, a, b) = color_to_lab(seed);
        Self {
            hue: b.atan2(a),
            chroma,
        }
    }

    /// A fixed hue (degrees) + chroma — for the error/success/warning ramps that
    /// don't follow the seed.
    fn fixed(hue_deg: f64, chroma: f64) -> Self {
        Self {
            hue: hue_deg.to_radians(),
            chroma,
        }
    }

    /// The color at tone `t` (CIELAB L\*, 0..=100).
    fn tone(&self, t: f64) -> Color {
        lab_to_color(
            t,
            self.chroma * self.hue.cos(),
            self.chroma * self.hue.sin(),
        )
    }
}

impl Theme {
    /// A **Material 3** light theme from the default "Material You" seed.
    pub fn material3_light() -> Self {
        Self::material3_from_seed(M3_SEED, false)
    }

    /// A **Material 3** dark theme from the default "Material You" seed.
    pub fn material3_dark() -> Self {
        Self::material3_from_seed(M3_SEED, true)
    }

    /// Build a **Material 3 dynamic-color** theme from any `seed`, light or
    /// `dark`. Generates tonal palettes (primary, neutral, neutral-variant, plus
    /// fixed error/success/warning) and maps M3's role tones onto Stipple's
    /// [`Palette`] — so a single brand color reskins the whole UI, the way
    /// "Material You" recolors from the wallpaper. Tones are CIELAB L\* (M3's
    /// definition); hue/chroma approximate M3's CAM16 quantities.
    pub fn material3_from_seed(seed: Color, dark: bool) -> Self {
        let primary = Tonal::from_seed(seed, 48.0);
        let neutral = Tonal::neutral_of(seed, 4.0);
        let nv = Tonal::neutral_of(seed, 8.0);
        let error = Tonal::fixed(25.0, 84.0);
        let success = Tonal::fixed(145.0, 50.0);
        let warning = Tonal::fixed(75.0, 80.0);

        // Role tones per the M3 spec, light vs dark.
        let p = if dark {
            // (primary, on_primary, background, surface, text, muted, border,
            //  danger, success, warning)
            Palette {
                background: neutral.tone(6.0),
                surface: neutral.tone(17.0),
                primary: primary.tone(80.0),
                primary_hover: states(primary.tone(80.0), true).0,
                primary_active: states(primary.tone(80.0), true).1,
                on_primary: primary.tone(20.0),
                text: neutral.tone(90.0),
                text_muted: nv.tone(80.0),
                border: nv.tone(30.0),
                success: success.tone(70.0),
                warning: warning.tone(75.0),
                danger: error.tone(80.0),
                focus_ring: primary.tone(80.0),
                hover_overlay: with_alpha(neutral.tone(90.0), 22),
                selection: with_alpha(primary.tone(80.0), 80),
            }
        } else {
            Palette {
                background: neutral.tone(96.0),
                surface: neutral.tone(100.0),
                primary: primary.tone(40.0),
                primary_hover: states(primary.tone(40.0), false).0,
                primary_active: states(primary.tone(40.0), false).1,
                on_primary: primary.tone(100.0),
                text: neutral.tone(10.0),
                text_muted: nv.tone(30.0),
                border: nv.tone(80.0),
                success: success.tone(40.0),
                warning: warning.tone(45.0),
                danger: error.tone(40.0),
                focus_ring: primary.tone(40.0),
                hover_overlay: with_alpha(neutral.tone(10.0), 20),
                selection: with_alpha(primary.tone(40.0), 60),
            }
        };
        Self {
            palette: p,
            // M3 type scale (sp): body 14, title 16, headline 22, label 12.
            typography: Typography {
                caption: 12.0,
                body: 14.0,
                title: 16.0,
                heading: 22.0,
            },
            spacing: Spacing::default(),
            // M3 medium components (cards) use a 12dp corner radius.
            radius: 12.0,
            font_size: 14.0,
            native: false,
        }
    }
}

/// A copy of `c` with its alpha replaced (Material 3 state layers + selection).
fn with_alpha(c: Color, a: u8) -> Color {
    Color::rgba(c.r, c.g, c.b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_theme_follows_the_system_look() {
        let accent = Color::rgb(10, 20, 200);
        let dark = Theme::native_for(native::SystemLook {
            dark: true,
            accent: Some(accent),
        });
        assert!(dark.is_dark());
        assert_eq!(dark.palette.primary, accent);
        let light = Theme::native_for(native::SystemLook::default());
        assert!(!light.is_dark());
    }

    #[test]
    fn only_the_native_theme_asks_the_os_to_draw() {
        let control = native::NativeControl::TextField;
        assert_eq!(Theme::light().native_deco(control), None);
        assert_eq!(Theme::dark().native_deco(control), None);
        assert_eq!(
            Theme::native_for(native::SystemLook::default())
                .native_deco(control)
                .is_some(),
            native::available() && native::supports(control)
        );
    }

    #[test]
    fn themes_differ_in_palette_not_metrics() {
        let (l, d) = (Theme::light(), Theme::dark());
        assert_ne!(l.palette.background, d.palette.background);
        assert_eq!(l.spacing, d.spacing);
        assert_eq!(l.radius, d.radius);
    }

    #[test]
    fn with_accent_recolors_and_picks_on_color() {
        let t = Theme::light().with_accent(Color::rgb(0xff, 0xd0, 0x00)); // bright yellow
        assert_eq!(t.palette.primary, Color::rgb(0xff, 0xd0, 0x00));
        assert_eq!(t.palette.focus_ring, t.palette.primary);
        // On a bright accent, content should be dark.
        assert_eq!(t.palette.on_primary, Color::BLACK);
        // Hover/active are derived (different from the base).
        assert_ne!(t.palette.primary_hover, t.palette.primary);
    }

    #[test]
    fn high_contrast_maximizes_text_contrast() {
        let t = Theme::light().high_contrast();
        assert_eq!(t.palette.text, Color::BLACK);
        assert_eq!(t.palette.background, Color::WHITE);
    }

    #[test]
    fn lab_tone_ramp_spans_black_to_white() {
        // A tonal palette's extremes are near-black and near-white, and tones
        // increase monotonically in luminance.
        let t = Tonal::from_seed(M3_SEED, 48.0);
        assert!(t.tone(0.0).luminance() < 0.02, "tone 0 ~ black");
        assert!(t.tone(100.0).luminance() > 0.95, "tone 100 ~ white");
        assert!(t.tone(40.0).luminance() < t.tone(80.0).luminance());
        // A round-trip through Lab is near-identity (within rounding).
        let c = Color::rgb(0x67, 0x50, 0xa4);
        let (l, a, b) = color_to_lab(c);
        let back = lab_to_color(l, a, b);
        assert!((back.r as i32 - c.r as i32).abs() <= 1);
        assert!((back.g as i32 - c.g as i32).abs() <= 1);
        assert!((back.b as i32 - c.b as i32).abs() <= 1);
    }

    #[test]
    fn material3_light_and_dark_are_distinct_and_readable() {
        let l = Theme::material3_light();
        let d = Theme::material3_dark();
        // Light has a bright background, dark a deep one.
        assert!(l.palette.background.luminance() > 0.8);
        assert!(d.palette.background.luminance() < 0.1);
        // M3 shape + type scale.
        assert_eq!(l.radius, 12.0);
        assert_eq!(l.typography.heading, 22.0);
        // The primary is a recognizable Material You purple (blue-dominant, low
        // green) in both modes.
        for t in [l, d] {
            let p = t.palette.primary;
            assert!(p.b > p.g, "primary is purple-leaning (b>g)");
        }
        // on_primary must contrast strongly with primary (a filled button reads).
        let contrast = (l.palette.primary.luminance() - l.palette.on_primary.luminance()).abs();
        assert!(contrast > 0.4, "onPrimary contrasts with primary");
    }

    #[test]
    fn material3_from_seed_recolors_the_whole_scheme() {
        // A red seed yields a red-dominant primary; a green seed a green one.
        let red = Theme::material3_from_seed(Color::rgb(0xb0, 0x00, 0x20), false);
        assert!(red.palette.primary.r > red.palette.primary.b);
        assert!(red.palette.primary.r > red.palette.primary.g);
        let green = Theme::material3_from_seed(Color::rgb(0x00, 0x80, 0x40), false);
        assert!(green.palette.primary.g > green.palette.primary.r);
        // The neutral background even picks up a faint tint of the seed hue.
        assert_ne!(red.palette.background, green.palette.background);
    }
}
