//! Translucent windows ([`WindowAttributes::transparent`]): the activity's
//! window takes a translucent format (`Window.setFormat(PixelFormat.
//! TRANSLUCENT)`, where `NativeActivity` asks for an opaque one), so the
//! system composes it with its alpha, and shows the wallpaper behind it
//! (`FLAG_SHOW_WALLPAPER`; under an activity whose theme is translucent,
//! `android:windowIsTranslucent`, the app below shows instead). Its buffers
//! are `RGBA_8888`, holding the frames premultiplied, as the system takes
//! them; a GPU renderer's swapchain or EGL surface keeps the alpha too.
//! With blur, on Android 12 and later, what shows through is blurred
//! (`Window.setBackgroundBlurRadius`, where the device has cross-window
//! blur).
//!
//! [`WindowAttributes::transparent`]: crate::platform::WindowAttributes::transparent

use super::jni::{JObject, JValue, Jni};

/// `PixelFormat.TRANSLUCENT`.
const PIXEL_FORMAT_TRANSLUCENT: i32 = -3;
/// `WindowManager.LayoutParams.FLAG_SHOW_WALLPAPER`.
pub(super) const FLAG_SHOW_WALLPAPER: u32 = 0x0010_0000;
/// `WINDOW_FORMAT_RGBA_8888`: a translucent window's buffers.
pub(super) const WINDOW_FORMAT_RGBA_8888: i32 = 1;
/// How far the blur behind a window reaches, in pixels.
const BLUR_RADIUS: i32 = 48;

/// Make the activity's window translucent, blurring what shows through
/// with `blur` where the system can. On the main thread.
pub(super) fn make_translucent(jni: &Jni, activity: JObject, blur: bool, sdk: i32) -> Option<()> {
    let window = jni.call_object(activity, c"getWindow", c"()Landroid/view/Window;", &[])?;
    jni.call_void(
        window,
        c"setFormat",
        c"(I)V",
        &[JValue {
            i: PIXEL_FORMAT_TRANSLUCENT,
        }],
    )?;
    if blur && sdk >= 31 {
        jni.call_void(
            window,
            c"setBackgroundBlurRadius",
            c"(I)V",
            &[JValue { i: BLUR_RADIUS }],
        )?;
    }
    Some(())
}

/// The color to fill a window's margins (around the content, behind the
/// system bars) with, from a frame's corner pixel (premultiplied RGBA):
/// opaque in an opaque window, as it is in a translucent one.
pub(super) fn margin_color([r, g, b, a]: [u8; 4], translucent: bool) -> [u8; 4] {
    if translucent {
        [r, g, b, a]
    } else {
        [r, g, b, 255]
    }
}

/// A premultiplied color's straight (unpremultiplied) value; a fully
/// transparent one stays as it is (black).
pub(super) fn straight([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    if a == 0 || a == 255 {
        return [r, g, b, a];
    }
    let un = |c: u8| ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
    [un(r), un(g), un(b), a]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_translucent_window_keeps_its_margins_alpha() {
        assert_eq!(margin_color([10, 20, 30, 128], true), [10, 20, 30, 128]);
        assert_eq!(margin_color([10, 20, 30, 128], false), [10, 20, 30, 255]);
    }

    #[test]
    fn straight_colors_undo_the_premultiplication() {
        assert_eq!(straight([100, 50, 0, 128]), [199, 100, 0, 128]);
        assert_eq!(straight([10, 20, 30, 255]), [10, 20, 30, 255]);
        assert_eq!(straight([0, 0, 0, 0]), [0, 0, 0, 0]);
        assert_eq!(straight([200, 0, 0, 100]), [255, 0, 0, 100]);
    }
}
