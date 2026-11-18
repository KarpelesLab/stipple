//! Fractional scales (`wp_fractional_scale_v1`) through `wp_viewporter`.
//!
//! Without them a surface's scale is an integer (`wl_surface.set_buffer_scale`,
//! from the outputs it is on), and at 125% or 150% the compositor scales a 1x
//! or 2x buffer, blurring it. With both globals, each window asks for its
//! preferred scale (`preferred_scale`, in 120ths), draws its buffers at that
//! many pixels per logical one (rounded), and maps them back to its logical
//! size with a `wp_viewport` destination, at buffer scale 1: the buffer's
//! pixels are the screen's, and text is sharp.
//!
//! A window's scale is an `f64` either way: a whole number of the output's
//! when there is no fractional scale.

use super::{WL_SURFACE_SET_BUFFER_SCALE, WlConn, arg_u32};

/// The globals' interface names.
pub(super) const MANAGER: &str = "wp_fractional_scale_manager_v1";
pub(super) const VIEWPORTER: &str = "wp_viewporter";

const MANAGER_GET_FRACTIONAL_SCALE: u16 = 1;
const FRACTIONAL_DESTROY: u16 = 0;
/// `wp_fractional_scale_v1.preferred_scale`: the scale × 120.
const FRACTIONAL_PREFERRED_SCALE: u16 = 0;
const VIEWPORTER_GET_VIEWPORT: u16 = 1;
const VIEWPORT_DESTROY: u16 = 0;
const VIEWPORT_SET_DESTINATION: u16 = 2;

/// The globals, and the objects made for each surface.
#[derive(Default)]
pub(super) struct Fractional {
    manager: Option<u32>,
    viewporter: Option<u32>,
    /// `(wp_fractional_scale_v1, its window's surface)`.
    scales: Vec<(u32, u32)>,
    /// `(surface, its wp_viewport, the destination last set)`.
    viewports: Vec<(u32, u32, (u32, u32))>,
}

impl Fractional {
    /// Whether windows follow fractional scales: the compositor has both
    /// globals.
    pub(super) fn enabled(&self) -> bool {
        self.manager.is_some() && self.viewporter.is_some()
    }
}

/// Take the bound globals (either may be missing; fractional scales need
/// both).
pub(super) fn setup(conn: &mut WlConn, manager: Option<u32>, viewporter: Option<u32>) {
    conn.fractional.manager = manager;
    conn.fractional.viewporter = viewporter;
    super::dbg(format_args!(
        "fractional scale: {}",
        if conn.fractional.enabled() {
            "yes"
        } else {
            "no"
        }
    ));
}

/// Ask for window surface `surface`'s preferred scale, which comes as
/// [`preferred`] events.
pub(super) fn watch(conn: &mut WlConn, surface: u32) -> std::io::Result<()> {
    let Some(manager) = conn
        .fractional
        .manager
        .filter(|_| conn.fractional.enabled())
    else {
        return Ok(());
    };
    let id = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, id);
    arg_u32(&mut a, surface);
    conn.send(manager, MANAGER_GET_FRACTIONAL_SCALE, &a)?;
    conn.fractional.scales.push((id, surface));
    Ok(())
}

/// A `preferred_scale` event: the window surface it is for, and the scale.
pub(super) fn preferred(
    conn: &WlConn,
    object: u32,
    opcode: u16,
    args: &[u8],
) -> Option<(u32, f64)> {
    if opcode != FRACTIONAL_PREFERRED_SCALE {
        return None;
    }
    let &(_, surface) = conn
        .fractional
        .scales
        .iter()
        .find(|(id, _)| *id == object)?;
    let v = u32::from_le_bytes(args.get(..4)?.try_into().ok()?);
    (v > 0).then(|| (surface, v as f64 / 120.0))
}

/// The buffer size for `logical` surface units at `scale`: rounded half away
/// from zero, as the protocol asks.
pub(super) fn physical(logical: u32, scale: f64) -> u32 {
    (logical as f64 * scale).round().max(1.0) as u32
}

/// The surface units a buffer of `physical` pixels at `scale` covers.
pub(super) fn logical(physical: u32, scale: f64) -> u32 {
    (physical as f64 / scale.max(f64::MIN_POSITIVE))
        .round()
        .max(1.0) as u32
}

/// Say how `surface`'s next buffer maps to surface units: at `scale` pixels a
/// unit, over `units` (`width`, `height`). With fractional scales a viewport
/// (made the first time) stretches it over `units` at buffer scale 1; else
/// the buffer scale is the (whole) scale.
pub(super) fn set_scale(
    conn: &mut WlConn,
    surface: u32,
    scale: f64,
    units: (u32, u32),
) -> std::io::Result<()> {
    let Some(viewporter) = conn
        .fractional
        .viewporter
        .filter(|_| conn.fractional.enabled())
    else {
        let whole = scale.round().max(1.0) as u32;
        return conn.send(surface, WL_SURFACE_SET_BUFFER_SCALE, &whole.to_le_bytes());
    };
    let units = (units.0.max(1), units.1.max(1));
    let at = conn
        .fractional
        .viewports
        .iter()
        .position(|v| v.0 == surface);
    let at = match at {
        Some(at) => at,
        None => {
            let viewport = conn.new_id();
            let mut a = Vec::new();
            arg_u32(&mut a, viewport);
            arg_u32(&mut a, surface);
            conn.send(viewporter, VIEWPORTER_GET_VIEWPORT, &a)?;
            conn.send(surface, WL_SURFACE_SET_BUFFER_SCALE, &1u32.to_le_bytes())?;
            conn.fractional.viewports.push((surface, viewport, (0, 0)));
            conn.fractional.viewports.len() - 1
        }
    };
    let (_, viewport, last) = conn.fractional.viewports[at];
    if last != units {
        let mut a = Vec::new();
        arg_u32(&mut a, units.0);
        arg_u32(&mut a, units.1);
        conn.send(viewport, VIEWPORT_SET_DESTINATION, &a)?;
        conn.fractional.viewports[at].2 = units;
    }
    Ok(())
}

/// Destroy what was made for `surface` (before it goes).
pub(super) fn forget(conn: &mut WlConn, surface: u32) {
    let f = &mut conn.fractional;
    let viewports: Vec<u32> = f
        .viewports
        .iter()
        .filter(|v| v.0 == surface)
        .map(|v| v.1)
        .collect();
    let scales: Vec<u32> = f
        .scales
        .iter()
        .filter(|s| s.1 == surface)
        .map(|s| s.0)
        .collect();
    f.viewports.retain(|v| v.0 != surface);
    f.scales.retain(|s| s.1 != surface);
    for v in viewports {
        let _ = conn.send(v, VIEWPORT_DESTROY, &[]);
    }
    for s in scales {
        let _ = conn.send(s, FRACTIONAL_DESTROY, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_at_fractional_scales() {
        assert_eq!(physical(800, 1.5), 1200);
        assert_eq!(physical(801, 1.25), 1001); // 1001.25
        assert_eq!(physical(3, 1.5), 5); // 4.5 rounds up
        assert_eq!(logical(1200, 1.5), 800);
        assert_eq!(logical(1001, 1.25), 801);
        // Back and forth gives the logical size again at any scale ≥ 1.
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 3.0] {
            for l in [1, 7, 333, 800, 1921] {
                assert_eq!(logical(physical(l, scale), scale), l, "{l} at {scale}");
            }
        }
    }

    #[test]
    fn preferred_scale_events_name_their_window() {
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut conn = super::super::WlConn::over(ours);
        conn.fractional.scales.push((40, 7));
        let args = 180u32.to_le_bytes();
        assert_eq!(preferred(&conn, 40, 0, &args), Some((7, 1.5)));
        assert_eq!(preferred(&conn, 41, 0, &args), None, "not ours");
        assert_eq!(preferred(&conn, 40, 0, &0u32.to_le_bytes()), None);
    }
}
