//! The screen the app fills: the system bars' insets, drawing edge to
//! edge under them, the requested orientation and keeping the screen on.
//!
//! The insets come from the decor view's `getRootWindowInsets` (Android 6
//! and later): on Android 11 and later `getInsets(systemBars() |
//! displayCutout())` and `getInsets(ime())`; before, the system window
//! insets (the bars shown) and the display cutout's safe insets (Android 9),
//! the keyboard being what the bottom one adds to the stable inset there. They are
//! read on the main thread whenever the host view is handed new insets
//! (`onApplyWindowInsets`, a native of the run-time `HostView`) and after
//! the window's size or content rectangle changes, and reach the UI thread
//! as a [`Command::Insets`](super::Command).
//!
//! Edge to edge, the window lays out under the bars
//! (`setDecorFitsSystemWindows(false)` on Android 11 and later, the
//! `SYSTEM_UI_FLAG_LAYOUT_*` flags before), into the display cutout
//! (`LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS`, `SHORT_EDGES` before Android 11),
//! and the bars go transparent without the scrim Android 10 adds.

use super::insets::{ARect, WindowInsets};
use super::jni::{self, Env, JObject, JValue, Jni};
use super::{Command, post_command, with_activity};

fn decor(jni: &Jni, activity: JObject) -> Option<JObject> {
    let window = jni.call_object(activity, c"getWindow", c"()Landroid/view/Window;", &[])?;
    jni.call_object(window, c"getDecorView", c"()Landroid/view/View;", &[])
}

/// Read the window's insets. On the main thread.
pub(super) fn read(jni: &Jni, activity: JObject, sdk: i32) -> Option<WindowInsets> {
    if sdk < 23 {
        return None;
    }
    let insets = jni.call_object(
        decor(jni, activity)?,
        c"getRootWindowInsets",
        c"()Landroid/view/WindowInsets;",
        &[],
    )?;
    if sdk >= 30 {
        let ty = |name: &std::ffi::CStr| {
            jni.call_static_int(c"android/view/WindowInsets$Type", name, c"()I", &[])
        };
        let of = |types: i32| -> Option<ARect> {
            let i = jni.call_object(
                insets,
                c"getInsets",
                c"(I)Landroid/graphics/Insets;",
                &[JValue { i: types }],
            )?;
            Some(ARect {
                left: jni.int_field(i, c"left")?,
                top: jni.int_field(i, c"top")?,
                right: jni.int_field(i, c"right")?,
                bottom: jni.int_field(i, c"bottom")?,
            })
        };
        let bars = of(ty(c"systemBars")? | ty(c"displayCutout")?)?;
        let ime = of(ty(c"ime")?).map_or(0, |r| r.bottom);
        return Some(WindowInsets { bars, ime });
    }
    // The bars shown (none of the status bar's in fullscreen); at the
    // bottom, the keyboard counts too, so the bar is the stable inset there.
    let side = |name: &std::ffi::CStr| jni.call_int(insets, name, c"()I", &[]);
    let shown_bottom = side(c"getSystemWindowInsetBottom")?;
    let mut bars = ARect {
        left: side(c"getSystemWindowInsetLeft")?,
        top: side(c"getSystemWindowInsetTop")?,
        right: side(c"getSystemWindowInsetRight")?,
        bottom: side(c"getStableInsetBottom")?.min(shown_bottom),
    };
    if sdk >= 28
        && let Some(cutout) = jni.call_object(
            insets,
            c"getDisplayCutout",
            c"()Landroid/view/DisplayCutout;",
            &[],
        )
    {
        let safe = |name: &std::ffi::CStr| jni.call_int(cutout, name, c"()I", &[]).unwrap_or(0);
        bars.left = bars.left.max(safe(c"getSafeInsetLeft"));
        bars.top = bars.top.max(safe(c"getSafeInsetTop"));
        bars.right = bars.right.max(safe(c"getSafeInsetRight"));
        bars.bottom = bars.bottom.max(safe(c"getSafeInsetBottom"));
    }
    // What the system window insets add at the bottom is the keyboard.
    let ime = (shown_bottom - bars.bottom).max(0);
    Some(WindowInsets { bars, ime })
}

/// Read the insets and hand them to the UI thread. On the main thread.
pub(super) fn report(jni: &Jni, activity: JObject, sdk: i32) {
    if let Some(insets) = read(jni, activity, sdk) {
        post_command(Command::Insets(insets));
    }
}

/// `HostView.onApplyWindowInsets`: the window has new insets.
pub(super) unsafe extern "C" fn on_apply_window_insets(
    env: Env,
    _this: JObject,
    insets: JObject,
) -> JObject {
    let activity = super::classes::host().activity;
    let sdk = with_activity(|a| Some(a.sdk_version)).unwrap_or(0);
    if !activity.get().is_null() {
        unsafe {
            jni::in_native(env, |jni| {
                report(jni, activity.get(), sdk);
                Some(())
            })
        };
    }
    // Not consumed: the rest of the window's views see them too.
    insets
}

/// Lay the window out under the system bars and the display cutout, with
/// no scrim behind the bars. On the main thread.
pub(super) fn draw_edge_to_edge(jni: &Jni, activity: JObject, sdk: i32) -> Option<()> {
    // View.SYSTEM_UI_FLAG_LAYOUT_STABLE, _LAYOUT_HIDE_NAVIGATION,
    // _LAYOUT_FULLSCREEN.
    const LAYOUT_FLAGS: i32 = 0x100 | 0x200 | 0x400;
    let window = jni.call_object(activity, c"getWindow", c"()Landroid/view/Window;", &[])?;
    if sdk >= 30 {
        jni.call_void(
            window,
            c"setDecorFitsSystemWindows",
            c"(Z)V",
            &[JValue { z: 0 }],
        )?;
    } else {
        let decor = decor(jni, activity)?;
        let flags = jni.call_int(decor, c"getSystemUiVisibility", c"()I", &[])?;
        jni.call_void(
            decor,
            c"setSystemUiVisibility",
            c"(I)V",
            &[JValue {
                i: flags | LAYOUT_FLAGS,
            }],
        )?;
    }
    if sdk >= 28 {
        // LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS (Android 11), _SHORT_EDGES.
        let mode = if sdk >= 30 { 3 } else { 1 };
        let attrs = jni.call_object(
            window,
            c"getAttributes",
            c"()Landroid/view/WindowManager$LayoutParams;",
            &[],
        )?;
        jni.set_int_field(attrs, c"layoutInDisplayCutoutMode", mode)?;
        jni.call_void(
            window,
            c"setAttributes",
            c"(Landroid/view/WindowManager$LayoutParams;)V",
            &[JValue { l: attrs }],
        )?;
    }
    if sdk >= 29 {
        let no = [JValue { z: 0 }];
        jni.call_void(window, c"setStatusBarContrastEnforced", c"(Z)V", &no)?;
        jni.call_void(window, c"setNavigationBarContrastEnforced", c"(Z)V", &no)?;
    }
    Some(())
}

/// `Activity.setRequestedOrientation`. On the main thread.
pub(super) fn request_orientation(jni: &Jni, activity: JObject, value: i32) {
    jni.call_void(
        activity,
        c"setRequestedOrientation",
        c"(I)V",
        &[JValue { i: value }],
    );
}
