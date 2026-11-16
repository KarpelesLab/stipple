//! How a YUV image's values map to RGB: the matrix (BT.601 or BT.709) and
//! the range (full, or the video "limited" range), as one 3×4 matrix every
//! converter uses, the shaders and the CPU alike.

/// The color space of a YUV [`ExternalImage`](super::ExternalImage)
/// ([`Nv12`](super::ExternalFormat::Nv12)): which matrix turns its Y, Cb, Cr
/// into R, G, B, and whether they use the full 0–255 range or the video
/// range (luma 16–235, chroma 16–240). Cameras deliver full range BT.601
/// (the default); video decoders mostly limited range BT.709.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum YuvColorSpace {
    /// BT.601, full range (JPEG's, the camera's): the default.
    #[default]
    Bt601Full,
    /// BT.601, limited (video) range: standard-definition video, Core
    /// Video's `420v`.
    Bt601Limited,
    /// BT.709, full range.
    Bt709Full,
    /// BT.709, limited (video) range: HD video.
    Bt709Limited,
}

impl YuvColorSpace {
    /// The rows of the matrix taking `[y, cb, cr, 1]` (each `0.0..=1.0`, as a
    /// shader samples them) to `r`, `g` and `b`.
    pub(crate) fn matrix(self) -> [[f32; 4]; 3] {
        let (kr, kb, limited) = match self {
            YuvColorSpace::Bt601Full => (0.299, 0.114, false),
            YuvColorSpace::Bt601Limited => (0.299, 0.114, true),
            YuvColorSpace::Bt709Full => (0.2126, 0.0722, false),
            YuvColorSpace::Bt709Limited => (0.2126, 0.0722, true),
        };
        let kg = 1.0 - kr - kb;
        // Chroma's coefficients, for values centered on zero.
        let r_cr = 2.0 * (1.0 - kr);
        let b_cb = 2.0 * (1.0 - kb);
        let g_cb = -2.0 * kb * (1.0 - kb) / kg;
        let g_cr = -2.0 * kr * (1.0 - kr) / kg;
        // y' = (y - y0) * ys, c' = (c - c0) * cs.
        let (y0, ys, cs) = if limited {
            (16.0 / 255.0, 255.0 / 219.0, 255.0 / 224.0)
        } else {
            (0.0, 1.0, 1.0)
        };
        let c0 = 128.0 / 255.0;
        let row = |a_cb: f64, a_cr: f64| {
            let (a_cb, a_cr) = (a_cb * cs, a_cr * cs);
            [
                ys as f32,
                a_cb as f32,
                a_cr as f32,
                (-y0 * ys - c0 * (a_cb + a_cr)) as f32,
            ]
        };
        [row(0.0, r_cr), row(g_cb, g_cr), row(b_cb, 0.0)]
    }

    /// One pixel's 8-bit Y, Cb, Cr as RGB.
    pub(crate) fn to_rgb(self, y: u8, cb: u8, cr: u8) -> [u8; 3] {
        if self == YuvColorSpace::Bt601Full {
            // Exactly as the camera converts its frames.
            return crate::camera::yuv_to_rgb(y, cb, cr);
        }
        let v = [y as f32 / 255.0, cb as f32 / 255.0, cr as f32 / 255.0];
        self.matrix().map(|m| {
            let c = m[0] * v[0] + m[1] * v[1] + m[2] * v[2] + m[3];
            (c * 255.0).round().clamp(0.0, 255.0) as u8
        })
    }

    /// The matrix as 12 floats, row after row (a shader's three `vec4`s).
    #[allow(dead_code)] // for the GPU executors built in
    pub(crate) fn rows(self) -> [f32; 12] {
        let m = self.matrix();
        let mut out = [0.0; 12];
        for (i, r) in m.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(r);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrices_agree_with_the_camera_and_the_standards() {
        // Full range BT.601 matches the camera's integer conversion within a
        // step.
        let full = YuvColorSpace::Bt601Full.matrix();
        for (y, cb, cr) in [(128u8, 128u8, 128u8), (76, 85, 255), (200, 30, 90)] {
            let want = crate::camera::yuv_to_rgb(y, cb, cr);
            let v = [y as f32 / 255.0, cb as f32 / 255.0, cr as f32 / 255.0];
            for (m, w) in full.iter().zip(want) {
                let c = (m[0] * v[0] + m[1] * v[1] + m[2] * v[2] + m[3]) * 255.0;
                let c = c.round().clamp(0.0, 255.0);
                assert!((c - w as f32).abs() <= 1.0, "{y} {cb} {cr}: {c} vs {w}");
            }
        }
        // Limited range: 16 is black, 235 white, neutral chroma gray.
        for cs in [YuvColorSpace::Bt601Limited, YuvColorSpace::Bt709Limited] {
            assert_eq!(cs.to_rgb(16, 128, 128), [0, 0, 0]);
            assert_eq!(cs.to_rgb(235, 128, 128), [255, 255, 255]);
            assert_eq!(cs.to_rgb(126, 128, 128), [128, 128, 128]);
        }
        // BT.709's pure red (limited): Y 63, Cb 102, Cr 240.
        let [r, g, b] = YuvColorSpace::Bt709Limited.to_rgb(63, 102, 240);
        assert!(r >= 253 && g <= 2 && b <= 2, "{r} {g} {b}");
        // The rows are the matrix's.
        assert_eq!(
            YuvColorSpace::Bt709Full.rows()[4..8],
            YuvColorSpace::Bt709Full.matrix()[1]
        );
    }
}
