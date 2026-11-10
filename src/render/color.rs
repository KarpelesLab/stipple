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
        // Hex digits are ASCII; anything else would also split a character.
        if !s.is_ascii() {
            return None;
        }
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

    /// `#rrggbb`, or `#rrggbbaa` when not opaque: what
    /// [`from_hex`](Self::from_hex) reads back.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// A color from hue (degrees, wrapped into 0..360), saturation and value
    /// (each 0..=1), and alpha.
    pub fn from_hsv(hue: f64, saturation: f64, value: f64, alpha: u8) -> Self {
        let h = hue.rem_euclid(360.0) / 60.0;
        let s = saturation.clamp(0.0, 1.0);
        let v = value.clamp(0.0, 1.0);
        let c = v * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        let to = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Self::rgba(to(r), to(g), to(b), alpha)
    }

    /// Hue (degrees, 0..360), saturation and value (each 0..=1). A gray has
    /// hue 0, and black saturation 0.
    pub fn to_hsv(self) -> (f64, f64, f64) {
        let (r, g, b) = (
            self.r as f64 / 255.0,
            self.g as f64 / 255.0,
            self.b as f64 / 255.0,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        let s = if max == 0.0 { 0.0 } else { d / max };
        (h, s, max)
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
        // Non-ASCII is refused rather than split mid-character.
        assert_eq!(Color::from_hex("#ééé"), None);
        assert_eq!(Color::rgb(1, 0x22, 0xff).to_hex(), "#0122ff");
        let c = Color::rgba(0x12, 0x34, 0x56, 0x78);
        assert_eq!(Color::from_hex(&c.to_hex()), Some(c));
    }

    #[test]
    fn hsv_round_trips() {
        assert_eq!(Color::from_hsv(0.0, 1.0, 1.0, 255), Color::rgb(255, 0, 0));
        assert_eq!(Color::from_hsv(120.0, 1.0, 1.0, 255), Color::rgb(0, 255, 0));
        assert_eq!(
            Color::from_hsv(600.0, 1.0, 1.0, 9),
            Color::rgba(0, 0, 255, 9)
        );
        assert_eq!(
            Color::from_hsv(42.0, 0.0, 0.5, 255),
            Color::rgb(128, 128, 128)
        );
        for c in [
            Color::rgb(0x33, 0x99, 0xcc),
            Color::rgb(250, 10, 128),
            Color::rgb(0, 0, 0),
            Color::rgb(255, 255, 255),
            Color::rgb(12, 200, 13),
        ] {
            let (h, s, v) = c.to_hsv();
            assert_eq!(Color::from_hsv(h, s, v, 255), c);
        }
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
