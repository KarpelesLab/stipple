use crate::geometry::Size;

/// The ratio of physical device pixels to logical pixels for a surface
/// (e.g. `2.0` on a typical HiDPI "retina" display, `1.0` on a standard
/// display). Always strictly positive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleFactor(f64);

impl ScaleFactor {
    /// The identity scale (1 physical pixel per logical pixel).
    pub const IDENTITY: Self = Self(1.0);

    /// Creates a scale factor, clamping non-finite or non-positive input to
    /// [`ScaleFactor::IDENTITY`].
    #[inline]
    pub fn new(factor: f64) -> Self {
        if factor.is_finite() && factor > 0.0 {
            Self(factor)
        } else {
            Self::IDENTITY
        }
    }

    #[inline]
    pub fn get(self) -> f64 {
        self.0
    }

    /// Convert a logical size to physical pixels, rounding to whole pixels.
    #[inline]
    pub fn to_physical(self, logical: Size) -> PhysicalSize {
        PhysicalSize {
            width: (logical.width * self.0).round().max(0.0) as u32,
            height: (logical.height * self.0).round().max(0.0) as u32,
        }
    }

    /// Convert a physical size back to logical pixels.
    #[inline]
    pub fn to_logical(self, physical: PhysicalSize) -> Size {
        Size::new(
            physical.width as f64 / self.0,
            physical.height as f64 / self.0,
        )
    }
}

impl Default for ScaleFactor {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// An integer pixel extent in **physical** device pixels. This is the unit the
/// platform layer and the render surface allocate buffers in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    #[inline]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Number of pixels (`width × height`).
    #[inline]
    pub const fn pixel_count(self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidpi_roundtrip() {
        let s = ScaleFactor::new(2.0);
        assert_eq!(
            s.to_physical(Size::new(100.0, 50.0)),
            PhysicalSize::new(200, 100)
        );
        assert_eq!(
            s.to_logical(PhysicalSize::new(200, 100)),
            Size::new(100.0, 50.0)
        );
    }

    #[test]
    fn invalid_scale_falls_back_to_identity() {
        assert_eq!(ScaleFactor::new(0.0), ScaleFactor::IDENTITY);
        assert_eq!(ScaleFactor::new(f64::NAN), ScaleFactor::IDENTITY);
        assert_eq!(ScaleFactor::new(-1.0), ScaleFactor::IDENTITY);
    }
}
