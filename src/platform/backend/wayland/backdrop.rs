//! Transparent windows (`WindowAttributes::transparent`) and the blur behind
//! them (`WindowAttributes::blur`).
//!
//! A transparent window's frames go up as `ARGB8888` (premultiplied) instead
//! of `XRGB8888`, with no opaque region, so the compositor blends them over
//! what is behind. Blur is the staging `ext-background-effect-v1` where the
//! compositor has it and says it can blur, else KWin's
//! `org_kde_kwin_blur_manager`: either takes a region of the surface to blur,
//! which follows the window's geometry (without the resize margin of
//! decorations Stipple draws).

use super::{WlConn, arg_u32};

/// The globals' interface names.
pub(super) const BACKGROUND_EFFECT: &str = "ext_background_effect_manager_v1";
pub(super) const KDE_BLUR: &str = "org_kde_kwin_blur_manager";

const EFFECT_MANAGER_GET_BACKGROUND_EFFECT: u16 = 1;
/// `ext_background_effect_manager_v1.capabilities`, and its blur bit.
const EFFECT_MANAGER_CAPABILITIES: u16 = 0;
const EFFECT_CAPABILITY_BLUR: u32 = 1;
const EFFECT_DESTROY: u16 = 0;
const EFFECT_SET_BLUR_REGION: u16 = 1;
const KDE_MANAGER_CREATE: u16 = 0;
const KDE_BLUR_COMMIT: u16 = 0;
const KDE_BLUR_SET_REGION: u16 = 1;
const KDE_BLUR_RELEASE: u16 = 2;
const WL_COMPOSITOR_CREATE_REGION: u16 = 1;
const WL_REGION_DESTROY: u16 = 0;
const WL_REGION_ADD: u16 = 1;

/// How a surface's blur is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Effect,
    Kde,
}

/// The blur globals, and the transparent windows.
#[derive(Default)]
pub(super) struct Backdrop {
    compositor: u32,
    effect_manager: Option<u32>,
    /// Whether the background-effect manager said it can blur.
    effect_blurs: bool,
    kde_manager: Option<u32>,
    /// Window surfaces whose frames keep their alpha.
    transparent: Vec<u32>,
    /// `(surface, the blur object, how, the region last set)`.
    blurred: Vec<(u32, u32, Kind, [i32; 4])>,
    /// Surfaces that asked for blur before it could be set up.
    wanted: Vec<u32>,
}

/// Take the bound globals.
pub(super) fn setup(conn: &mut WlConn, compositor: u32, effect: Option<u32>, kde: Option<u32>) {
    let b = &mut conn.backdrop;
    b.compositor = compositor;
    b.effect_manager = effect;
    b.kde_manager = kde;
}

/// Note a new window's surface: transparent, and blurred behind.
pub(super) fn add(conn: &mut WlConn, surface: u32, transparent: bool, blur: bool) {
    if transparent {
        conn.backdrop.transparent.push(surface);
    }
    if transparent && blur {
        conn.backdrop.wanted.push(surface);
    }
}

/// Whether window surface `surface`'s frames keep their alpha.
pub(super) fn is_transparent(conn: &WlConn, surface: u32) -> bool {
    conn.backdrop.transparent.contains(&surface)
}

/// The manager's capabilities: whether it can blur. Returns whether the
/// event was one of ours.
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8]) -> bool {
    if Some(object) != conn.backdrop.effect_manager || opcode != EFFECT_MANAGER_CAPABILITIES {
        return false;
    }
    let flags = args
        .get(..4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    conn.backdrop.effect_blurs = flags & EFFECT_CAPABILITY_BLUR != 0;
    super::dbg(format_args!("background effect capabilities {flags:#x}"));
    true
}

/// Before `surface`'s next commit: blur behind `rect` (`x, y, width,
/// height`, in surface units) if the window asked for blur, when it changed.
pub(super) fn update(conn: &mut WlConn, surface: u32, rect: [i32; 4]) -> std::io::Result<()> {
    if let Some(i) = conn.backdrop.wanted.iter().position(|&s| s == surface) {
        let b = &conn.backdrop;
        let kind = if b.effect_manager.is_some() && b.effect_blurs {
            Kind::Effect
        } else if b.kde_manager.is_some() {
            Kind::Kde
        } else {
            // Nothing blurs here (yet: the effect manager's capabilities may
            // still come).
            return Ok(());
        };
        conn.backdrop.wanted.remove(i);
        let object = conn.new_id();
        let mut a = Vec::new();
        arg_u32(&mut a, object);
        arg_u32(&mut a, surface);
        let (manager, opcode) = match kind {
            Kind::Effect => (
                conn.backdrop.effect_manager,
                EFFECT_MANAGER_GET_BACKGROUND_EFFECT,
            ),
            Kind::Kde => (conn.backdrop.kde_manager, KDE_MANAGER_CREATE),
        };
        let Some(manager) = manager else {
            return Ok(());
        };
        // org_kde_kwin_blur_manager.create takes (id, surface) too.
        conn.send(manager, opcode, &a)?;
        conn.backdrop.blurred.push((surface, object, kind, [0; 4]));
    }
    let Some(i) = conn.backdrop.blurred.iter().position(|b| b.0 == surface) else {
        return Ok(());
    };
    let (_, object, kind, last) = conn.backdrop.blurred[i];
    if last == rect {
        return Ok(());
    }
    let region = conn.new_id();
    let compositor = conn.backdrop.compositor;
    conn.send(
        compositor,
        WL_COMPOSITOR_CREATE_REGION,
        &region.to_le_bytes(),
    )?;
    let mut a = Vec::new();
    for v in rect {
        arg_u32(&mut a, v as u32);
    }
    conn.send(region, WL_REGION_ADD, &a)?;
    match kind {
        Kind::Effect => conn.send(object, EFFECT_SET_BLUR_REGION, &region.to_le_bytes())?,
        Kind::Kde => {
            conn.send(object, KDE_BLUR_SET_REGION, &region.to_le_bytes())?;
            conn.send(object, KDE_BLUR_COMMIT, &[])?;
        }
    }
    conn.send(region, WL_REGION_DESTROY, &[])?;
    conn.backdrop.blurred[i].3 = rect;
    Ok(())
}

/// Forget `surface` (before it goes), destroying its blur object.
pub(super) fn forget(conn: &mut WlConn, surface: u32) {
    let b = &mut conn.backdrop;
    b.transparent.retain(|&s| s != surface);
    b.wanted.retain(|&s| s != surface);
    let gone: Vec<(u32, Kind)> = b
        .blurred
        .iter()
        .filter(|e| e.0 == surface)
        .map(|e| (e.1, e.2))
        .collect();
    b.blurred.retain(|e| e.0 != surface);
    for (object, kind) in gone {
        let opcode = match kind {
            Kind::Effect => EFFECT_DESTROY,
            Kind::Kde => KDE_BLUR_RELEASE,
        };
        let _ = conn.send(object, opcode, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection, and the compositor's end (kept open while it's used).
    fn conn() -> (WlConn, std::os::unix::net::UnixStream) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        (WlConn::over(ours), theirs)
    }

    #[test]
    fn transparency_and_blur_follow_the_window() {
        let (mut c, _peer) = conn();
        setup(&mut c, 3, Some(10), None);
        add(&mut c, 20, true, true);
        add(&mut c, 21, false, false);
        assert!(is_transparent(&c, 20) && !is_transparent(&c, 21));
        // No capabilities yet: blur waits.
        update(&mut c, 20, [0, 0, 100, 50]).unwrap();
        assert!(c.backdrop.blurred.is_empty());
        assert!(handle(&mut c, 10, 0, &1u32.to_le_bytes()));
        assert!(!handle(&mut c, 11, 0, &1u32.to_le_bytes()), "not ours");
        update(&mut c, 20, [0, 0, 100, 50]).unwrap();
        assert_eq!(c.backdrop.blurred.len(), 1);
        assert_eq!(c.backdrop.blurred[0].3, [0, 0, 100, 50]);
        forget(&mut c, 20);
        assert!(!is_transparent(&c, 20) && c.backdrop.blurred.is_empty());
    }

    #[test]
    fn kwin_blur_where_there_is_no_background_effect() {
        let (mut c, _peer) = conn();
        setup(&mut c, 3, None, Some(12));
        add(&mut c, 20, true, true);
        update(&mut c, 20, [8, 8, 300, 232]).unwrap();
        assert_eq!(c.backdrop.blurred[0].2, Kind::Kde);
    }
}
