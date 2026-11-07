use oxideav_core::Rgba;

/// A straight (non-premultiplied) 8-bit-per-channel sRGB color.
///
/// Matches `oxideav-core`'s `Rgba` model; [`Color::to_oxideav`] is a
/// zero-cost bridge used when building a scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    #[inline]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parse `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA` (leading `#`
    /// optional). Returns `None` on any malformed input.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#').unwrap_or(s);
        let h = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        let n = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|v| v * 17);
        match s.len() {
            3 => Some(Self::rgb(n(0)?, n(1)?, n(2)?)),
            4 => Some(Self::rgba(n(0)?, n(1)?, n(2)?, n(3)?)),
            6 => Some(Self::rgb(h(0)?, h(2)?, h(4)?)),
            8 => Some(Self::rgba(h(0)?, h(2)?, h(4)?, h(6)?)),
            _ => None,
        }
    }

    /// Returns a copy with the alpha channel replaced.
    #[inline]
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Linear blend toward `other` by `t` (0.0 = self, 1.0 = other), per
    /// channel including alpha. `t` is clamped to `[0, 1]`.
    pub fn mix(self, other: Color, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
        Color {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: lerp(self.a, other.a),
        }
    }

    /// Blend `amount` (0..=1) toward white — for hover/active tints.
    pub fn lighten(self, amount: f64) -> Color {
        self.mix(Color::rgb(255, 255, 255).with_alpha(self.a), amount)
    }

    /// Blend `amount` (0..=1) toward black.
    pub fn darken(self, amount: f64) -> Color {
        self.mix(Color::rgb(0, 0, 0).with_alpha(self.a), amount)
    }

    /// Perceptual luminance in `[0, 1]` (ITU-R BT.709 weights), for picking a
    /// readable on-color.
    pub fn luminance(self) -> f64 {
        (0.2126 * self.r as f64 + 0.7152 * self.g as f64 + 0.0722 * self.b as f64) / 255.0
    }

    /// Black or white, whichever contrasts better with `self`.
    pub fn on_color(self) -> Color {
        if self.luminance() > 0.5 {
            Color::BLACK
        } else {
            Color::WHITE
        }
    }

    /// Bridge to the `oxideav-core` color used by the scene graph.
    #[inline]
    pub const fn to_oxideav(self) -> Rgba {
        Rgba {
            r: self.r,
            g: self.g,
            b: self.b,
            a: self.a,
        }
    }
}

impl From<Color> for Rgba {
    #[inline]
    fn from(c: Color) -> Rgba {
        c.to_oxideav()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(Color::from_hex("#ff0000"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::from_hex("0f0"), Some(Color::rgb(0, 255, 0)));
        assert_eq!(
            Color::from_hex("#11223344"),
            Some(Color::rgba(0x11, 0x22, 0x33, 0x44))
        );
        assert_eq!(Color::from_hex("#abc"), Some(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(Color::from_hex("zzz"), None);
        assert_eq!(Color::from_hex("#12345"), None);
    }

    #[test]
    fn mix_lighten_darken() {
        let c = Color::rgb(100, 100, 100);
        assert_eq!(
            c.mix(Color::rgb(200, 200, 200), 0.5),
            Color::rgb(150, 150, 150)
        );
        assert_eq!(c.mix(Color::rgb(200, 200, 200), 0.0), c);
        assert_eq!(
            Color::rgb(100, 100, 100).lighten(0.5),
            Color::rgb(178, 178, 178)
        );
        assert_eq!(
            Color::rgb(100, 100, 100).darken(0.5),
            Color::rgb(50, 50, 50)
        );
    }

    #[test]
    fn on_color_contrasts() {
        assert_eq!(Color::WHITE.on_color(), Color::BLACK);
        assert_eq!(Color::rgb(20, 20, 20).on_color(), Color::WHITE);
    }
}
