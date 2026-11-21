//! Transparent windows on OpenGL ES: an X11 window with the ARGB visual, or
//! a translucent Android window, takes its frames' premultiplied alpha from
//! an EGL window surface whose config has an alpha channel (on X11 the
//! config of the window's 32-bit visual; on Android, `RGBA_8888` rather than
//! `RGBX_8888`). The frame is copied into the surface unblended, so the
//! surface holds the premultiplied frame, background alpha and all.
//!
//! The context is made with the config of the first window presented to; a
//! window of the other kind gets a surface of its own kind's config, which
//! the context draws into where EGL lets it (Mesa and Android do). Where no
//! config with alpha makes a surface on the window, or the context can't
//! draw into it, the window's frames go through its `Surface` (which keeps
//! their alpha) instead.

use super::*;

/// `EGL_ALPHA_SIZE`.
const EGL_ALPHA_SIZE: EGLint = 0x3021;

thread_local! {
    /// Transparent windows (by [`layer_key`]) no EGL surface keeps the alpha
    /// of: [`GlExecutor::can_present`] turns them down.
    static NO_ALPHA: RefCell<std::collections::HashSet<usize>> = RefCell::default();
}

/// Whether `layer`'s window is transparent.
pub(super) fn layer_alpha(layer: &GpuLayer) -> bool {
    match *layer {
        GpuLayer::X11 { alpha, .. } | GpuLayer::Android { alpha, .. } => alpha,
        _ => false,
    }
}

/// Whether `layer` is a transparent window no EGL surface keeps the alpha
/// of.
pub(super) fn refused(layer: &GpuLayer) -> bool {
    layer_alpha(layer)
        && layer_key(layer).is_some_and(|k| NO_ALPHA.with(|n| n.borrow().contains(&k)))
}

/// Give up presenting to transparent `layer` straight: its frames go
/// through its `Surface` from the next one on.
pub(super) fn refuse(layer: &GpuLayer, why: &str) -> GpuError {
    if let Some(k) = layer_key(layer) {
        NO_ALPHA.with(|n| n.borrow_mut().insert(k));
    }
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        eprintln!(
            "stipple: OpenGL ES can't keep a transparent window's alpha ({why}); \
             its frames are shown through the window's bitmap"
        )
    });
    GpuError::Skipped
}

/// Forget `layer`'s window (it is going away).
pub(super) fn forget(layer: &GpuLayer) {
    if let Some(k) = layer_key(layer) {
        NO_ALPHA.with(|n| n.borrow_mut().remove(&k));
    }
}

/// The attributes choosing the configs of window surfaces of `kind`
/// (`EGL_SURFACE_TYPE` bits), 8 bits a channel, with an alpha channel when
/// `alpha`.
pub(super) fn config_attribs(kind: EGLint, alpha: bool) -> [EGLint; 13] {
    [
        EGL_SURFACE_TYPE,
        kind,
        EGL_RENDERABLE_TYPE,
        EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE,
        8,
        EGL_GREEN_SIZE,
        8,
        EGL_BLUE_SIZE,
        8,
        EGL_ALPHA_SIZE,
        if alpha { 8 } else { 0 },
        EGL_NONE,
    ]
}

/// The configs of window surfaces of `kind` on `dpy`, in EGL's order (with
/// `alpha`, only those with an alpha channel).
pub(super) unsafe fn window_configs(
    dpy: EGLDisplay,
    kind: EGLint,
    alpha: bool,
) -> Result<Vec<EGLConfig>, String> {
    let attribs = config_attribs(kind, alpha);
    let mut configs = [core::ptr::null_mut(); 64];
    let mut n: EGLint = 0;
    if unsafe { eglChooseConfig(dpy, attribs.as_ptr(), configs.as_mut_ptr(), 64, &mut n) } == 0 {
        return Err(format!("eglChooseConfig failed: {:#x}", unsafe {
            eglGetError()
        }));
    }
    Ok(configs[..n.max(0) as usize].to_vec())
}

/// The first of `configs` that makes a window surface on `layer`'s window,
/// and that surface.
pub(super) unsafe fn first_surface(
    dpy: EGLDisplay,
    configs: &[EGLConfig],
    layer: &GpuLayer,
) -> Option<(EGLConfig, EGLSurface)> {
    configs.iter().find_map(|&c| {
        let s = unsafe { create_window_surface(dpy, c, layer) };
        (!s.is_null()).then_some((c, s))
    })
}

impl State {
    /// A new window surface on `layer`'s window, in the config of its kind
    /// (opaque, or with alpha), found the first time; null if none makes
    /// one.
    pub(super) unsafe fn window_surface(&mut self, layer: &GpuLayer) -> EGLSurface {
        unsafe {
            let alpha = layer_alpha(layer);
            if let Some(c) = self.window_configs[alpha as usize] {
                return create_window_surface(self.dpy, c, layer);
            }
            let Ok(configs) = window_configs(self.dpy, EGL_WINDOW_BIT, alpha) else {
                return core::ptr::null_mut();
            };
            // The context's own config first, if it is of the kind.
            let ours = configs.contains(&self.config).then_some(self.config);
            let order: Vec<EGLConfig> = ours
                .into_iter()
                .chain(configs.into_iter().filter(|&c| Some(c) != ours))
                .collect();
            match first_surface(self.dpy, &order, layer) {
                Some((c, s)) => {
                    self.window_configs[alpha as usize] = Some(c);
                    s
                }
                None => core::ptr::null_mut(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_windows_ask_for_an_alpha_channel() {
        let a = config_attribs(EGL_WINDOW_BIT, true);
        assert_eq!(&a[10..], &[EGL_ALPHA_SIZE, 8, EGL_NONE]);
        let o = config_attribs(EGL_WINDOW_BIT, false);
        assert_eq!(&o[10..], &[EGL_ALPHA_SIZE, 0, EGL_NONE]);
        assert_eq!(&a[..2], &[EGL_SURFACE_TYPE, EGL_WINDOW_BIT]);
    }

    #[test]
    fn refused_windows_are_transparent_ones() {
        let x = GpuLayer::X11 {
            window: 0x1234,
            alpha: true,
        };
        let opaque = GpuLayer::X11 {
            window: 0x1234,
            alpha: false,
        };
        assert!(!refused(&x));
        assert!(matches!(refuse(&x, "test"), GpuError::Skipped));
        assert!(refused(&x));
        // An opaque window by the same id is never turned down.
        assert!(!refused(&opaque));
        forget(&x);
        assert!(!refused(&x));
    }
}
