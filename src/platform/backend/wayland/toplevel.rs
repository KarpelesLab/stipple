//! Several `xdg_toplevel` windows on one connection: creating and destroying
//! them, and reading their state from `xdg_toplevel.configure`.
//!
//! Every window is a `wl_surface` with an `xdg_surface` and an
//! `xdg_toplevel`; events name the object they're for, so the loop finds the
//! window by it. Windows opened or closed from inside the handler wait in a
//! [`Reg`] until it returns.
//!
//! Each window has its own decorations (`decor::Decor`); the connection holds
//! the one in use, and [`use_decor`] swaps in a window's before anything
//! touches them.
//!
//! Wayland keeps window positions from clients: there is no way to read or
//! set where a window is, and no resize increments. A window can be
//! minimized (`set_minimized`) but not restored by the client, and whether a
//! new window takes focus is up to the compositor.

use std::io::{self, Write};

use super::{
    WL_SURFACE_COMMIT, WlConn, XDG_TOPLEVEL_SET_FULLSCREEN, XDG_TOPLEVEL_SET_MAX_SIZE,
    XDG_TOPLEVEL_SET_MAXIMIZED, XDG_TOPLEVEL_SET_MIN_SIZE, XDG_TOPLEVEL_SET_TITLE, arg_str,
    arg_u32, set_toplevel_icon, size_args,
};
use crate::geometry::{PhysicalSize, Size};
use crate::platform::window::WindowAttributes;

const WL_COMPOSITOR_CREATE_SURFACE: u16 = 0;
const WL_SURFACE_DESTROY: u16 = 0;
const WL_SURFACE_ATTACH: u16 = 1;
const XDG_WM_BASE_GET_XDG_SURFACE: u16 = 2;
const XDG_SURFACE_DESTROY: u16 = 0;
const XDG_SURFACE_GET_TOPLEVEL: u16 = 1;
const XDG_TOPLEVEL_DESTROY: u16 = 0;
pub(super) const XDG_TOPLEVEL_SET_MINIMIZED: u16 = 13;
/// `xdg_toplevel.state` values.
const STATE_MAXIMIZED: u32 = 1;
const STATE_FULLSCREEN: u32 = 2;

/// The globals a new window is made from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Globals {
    pub compositor: u32,
    pub wm_base: u32,
    pub shm: u32,
    pub icon_manager: Option<u32>,
    /// The `zxdg_decoration_manager_v1`, and the seat window moves cite.
    pub decoration_manager: Option<u32>,
    pub seat: Option<u32>,
}

/// The protocol objects of one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Objects {
    pub surface: u32,
    pub xdg_surface: u32,
    pub toplevel: u32,
}

/// Windows opened, closed or resized from inside the handler, for the loop
/// to take up once it returns.
#[derive(Default)]
pub(super) struct Reg {
    pub opened: Vec<super::WaylandWindow>,
    /// Surfaces of windows to destroy.
    pub closed: Vec<u32>,
    /// Surfaces of windows the app resized, and the size (logical).
    pub resized: Vec<(u32, Size)>,
}

/// Create a window's surface, `xdg_surface` and `xdg_toplevel` with `attrs`,
/// and make the initial commit (the compositor answers with a configure).
/// Returns the objects and the icon's buffer, which must outlive its use.
pub(super) fn create(
    conn: &mut WlConn,
    globals: Globals,
    attrs: &WindowAttributes,
) -> io::Result<(Objects, Option<super::ShmBuffer>)> {
    let surface = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, surface);
    conn.send(globals.compositor, WL_COMPOSITOR_CREATE_SURFACE, &a)?;

    let xdg_surface = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, xdg_surface);
    arg_u32(&mut a, surface);
    conn.send(globals.wm_base, XDG_WM_BASE_GET_XDG_SURFACE, &a)?;

    let toplevel = conn.new_id();
    let mut a = Vec::new();
    arg_u32(&mut a, toplevel);
    conn.send(xdg_surface, XDG_SURFACE_GET_TOPLEVEL, &a)?;

    let mut a = Vec::new();
    arg_str(&mut a, &attrs.title);
    conn.send(toplevel, XDG_TOPLEVEL_SET_TITLE, &a)?;
    // Server-side decorations where offered, else our own; the other
    // windows' wait aside.
    if conn.decor.toplevel() != 0 {
        let current = std::mem::take(&mut conn.decor);
        conn.decors.push(current);
    }
    super::decor::setup(
        conn,
        globals.decoration_manager,
        xdg_surface,
        toplevel,
        globals.seat,
        attrs,
    );

    // Size limits and initial state, applied by the initial commit below.
    // They're the window's: the title bar we draw counts.
    let (min, max) = if attrs.resizable {
        (attrs.min_size, attrs.max_size)
    } else {
        (Some(attrs.logical_size), Some(attrs.logical_size))
    };
    let (min, max) = conn.decor.set_limits(Some(min), Some(max));
    if min.is_some() {
        conn.send(toplevel, XDG_TOPLEVEL_SET_MIN_SIZE, &size_args(min))?;
    }
    if max.is_some() {
        conn.send(toplevel, XDG_TOPLEVEL_SET_MAX_SIZE, &size_args(max))?;
    }
    if attrs.maximized {
        conn.send(toplevel, XDG_TOPLEVEL_SET_MAXIMIZED, &[])?;
    }
    if attrs.fullscreen {
        conn.send(toplevel, XDG_TOPLEVEL_SET_FULLSCREEN, &0u32.to_le_bytes())?;
    }
    let icon = match (globals.icon_manager, attrs.icon.as_ref()) {
        (Some(manager), Some(icon)) => {
            set_toplevel_icon(conn, globals.shm, manager, toplevel, Some(icon))
        }
        _ => None,
    };

    // Initial commit: the compositor responds with an xdg_surface.configure.
    conn.send(surface, WL_SURFACE_COMMIT, &[])?;
    conn.stream.flush()?;
    crate::platform::window::debug_window(format_args!("opened {surface:#x} {:?}", attrs.title));
    Ok((
        Objects {
            surface,
            xdg_surface,
            toplevel,
        },
        icon,
    ))
}

/// Put window `toplevel`'s decorations in use on the connection (where
/// `conn.decor` and the `decor` functions find them), setting the others
/// aside.
pub(super) fn use_decor(conn: &mut WlConn, toplevel: u32) {
    if conn.decor.toplevel() == toplevel {
        return;
    }
    if let Some(i) = conn.decors.iter().position(|d| d.toplevel() == toplevel) {
        std::mem::swap(&mut conn.decor, &mut conn.decors[i]);
    }
}

/// Put the decorations whose `zxdg_toplevel_decoration_v1` is `object` in
/// use, if any are. Returns whether they were found.
pub(super) fn use_decor_of(conn: &mut WlConn, object: u32) -> bool {
    if conn.decor.object() == Some(object) {
        return true;
    }
    match conn.decors.iter().position(|d| d.object() == Some(object)) {
        Some(i) => {
            std::mem::swap(&mut conn.decor, &mut conn.decors[i]);
            true
        }
        None => false,
    }
}

/// Destroy a window's objects: its decorations, then the toplevel (the
/// order xdg-decoration and xdg-shell ask for).
pub(super) fn destroy(conn: &mut WlConn, objects: Objects) {
    use_decor(conn, objects.toplevel);
    if conn.decor.toplevel() == objects.toplevel {
        super::decor::destroy(conn);
        conn.decor = conn.decors.pop().unwrap_or_default();
    }
    let _ = conn.send(objects.toplevel, XDG_TOPLEVEL_DESTROY, &[]);
    let _ = conn.send(objects.xdg_surface, XDG_SURFACE_DESTROY, &[]);
    let _ = conn.send(objects.surface, WL_SURFACE_DESTROY, &[]);
    let _ = conn.stream.flush();
    crate::platform::window::debug_window(format_args!("closed {:#x}", objects.surface));
}

/// Show (commit, and the next configure draws it) or hide (no buffer) a
/// window's surface.
pub(super) fn set_visible(conn: &mut WlConn, surface: u32, visible: bool) {
    if !visible {
        let mut a = Vec::new();
        arg_u32(&mut a, 0); // no buffer: unmapped
        arg_u32(&mut a, 0);
        arg_u32(&mut a, 0);
        let _ = conn.send(surface, WL_SURFACE_ATTACH, &a);
    }
    let _ = conn.send(surface, WL_SURFACE_COMMIT, &[]);
    let _ = conn.stream.flush();
}

/// What an `xdg_toplevel.configure` says: the suggested size (`None`: the
/// client picks) and whether the window is maximized and fullscreen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Configure {
    pub size: Option<PhysicalSize>,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// Parse `xdg_toplevel.configure`: width, height, then the states array
/// (its byte length, then 32-bit states).
pub(super) fn parse_configure(args: &[u8]) -> Configure {
    let word = |i: usize| -> u32 {
        args.get(i..i + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (w, h) = (word(0) as i32, word(4) as i32);
    let len = word(8) as usize;
    let states: Vec<u32> = (0..len / 4).map(|i| word(12 + i * 4)).collect();
    Configure {
        size: (w > 0 && h > 0).then(|| PhysicalSize::new(w as u32, h as u32)),
        maximized: states.contains(&STATE_MAXIMIZED),
        fullscreen: states.contains(&STATE_FULLSCREEN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_carries_size_and_states() {
        let mut args = Vec::new();
        for w in [800u32, 600, 12, 4 /* activated */, 1, 2] {
            args.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(
            parse_configure(&args),
            Configure {
                size: Some(PhysicalSize::new(800, 600)),
                maximized: true,
                fullscreen: true,
            }
        );
        // 0x0: the client picks; no states.
        let args = [0u8; 12];
        assert_eq!(parse_configure(&args), Configure::default());
    }
}
