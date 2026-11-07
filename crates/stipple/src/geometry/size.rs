use crate::geometry::Insets;

/// A 2D extent (width × height) in logical pixels.
///
/// Components are expected to be non-negative; helpers that could produce a
/// negative extent clamp to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    #[inline]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    #[inline]
    pub fn area(self) -> f64 {
        self.width * self.height
    }

    /// Shrinks the size by `insets` on all sides, clamping to zero.
    #[inline]
    pub fn deflate(self, insets: Insets) -> Size {
        Size::new(
            (self.width - insets.left - insets.right).max(0.0),
            (self.height - insets.top - insets.bottom).max(0.0),
        )
    }

    /// Grows the size by `insets` on all sides.
    #[inline]
    pub fn inflate(self, insets: Insets) -> Size {
        Size::new(
            self.width + insets.left + insets.right,
            self.height + insets.top + insets.bottom,
        )
    }

    /// Component-wise clamp into `[min, max]`.
    #[inline]
    pub fn clamp(self, min: Size, max: Size) -> Size {
        Size::new(
            self.width.clamp(min.width, max.width),
            self.height.clamp(min.height, max.height),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflate_clamps_to_zero() {
        let s = Size::new(10.0, 4.0);
        assert_eq!(s.deflate(Insets::uniform(3.0)), Size::new(4.0, 0.0));
    }
}
