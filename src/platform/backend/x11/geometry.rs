//! Titles, geometry and window state on X11.
//!
//! - **Titles** go in `_NET_WM_NAME` (`UTF8_STRING`, what EWMH window managers
//!   and taskbars read) and in the ICCCM `WM_NAME`, as Latin-1 `STRING` where
//!   the title fits and `UTF8_STRING` (which every current window manager
//!   reads too) where it doesn't.
//! - **Position and size** come from `ConfigureNotify`: before a window
//!   manager reparents the window into its frame, the event's coordinates are
//!   on the root; after, only the window manager's synthetic events are
//!   (ICCCM §4.1.5). `_NET_FRAME_EXTENTS` gives the frame around it, so the
//!   outer corner and size can be worked out. Moves and resizes go through
//!   `ConfigureWindow`, which a window manager reads with the default
//!   north-west gravity as the frame's corner.
//! - **State** (maximized, fullscreen, minimized) is read from `_NET_WM_STATE`
//!   when it changes, and the work area from the root's `_NET_WORKAREA` (the
//!   whole desktop's: X11 has no per-monitor work area).
//! - **Resize increments** are `PResizeInc` in `WM_NORMAL_HINTS`.

use std::collections::VecDeque;
use std::io::{self, Read};

use super::{Conn, finish, intern_atom, rd_u32};
use crate::geometry::{PhysicalSize, Point, Rect, ScaleFactor, Size};
use crate::platform::window::WindowInfo;

const OP_CONFIGURE_WINDOW: u8 = 12;
const OP_UNMAP_WINDOW: u8 = 10;
const OP_MAP_WINDOW: u8 = 8;
const OP_GET_PROPERTY: u8 = 20;
const OP_SEND_EVENT: u8 = 25;
const ATOM_STRING: u32 = 31;
const ATOM_WM_NAME: u32 = 39;
const ATOM_CARDINAL: u32 = 6;
/// The `ReparentNotify` event code.
pub(super) const X_REPARENT_NOTIFY: u8 = 21;
/// `WM_SIZE_HINTS` flags: a position the user (here: the app) asked for, and
/// resize increments.
pub(super) const US_POSITION: u32 = 1;
pub(super) const P_RESIZE_INC: u32 = 64;
/// ICCCM `IconicState`, for `WM_CHANGE_STATE`.
const ICONIC_STATE: u32 = 3;

/// The atoms this module uses, interned once.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Atoms {
    pub net_wm_name: u32,
    pub frame_extents: u32,
    pub workarea: u32,
    pub hidden: u32,
    pub active_window: u32,
    pub change_state: u32,
    pub user_time: u32,
    pub utf8: u32,
}

impl Atoms {
    pub fn intern(conn: &mut Conn) -> io::Result<Self> {
        Ok(Self {
            net_wm_name: intern_atom(conn, b"_NET_WM_NAME")?,
            frame_extents: intern_atom(conn, b"_NET_FRAME_EXTENTS")?,
            workarea: intern_atom(conn, b"_NET_WORKAREA")?,
            hidden: intern_atom(conn, b"_NET_WM_STATE_HIDDEN")?,
            active_window: intern_atom(conn, b"_NET_ACTIVE_WINDOW")?,
            change_state: intern_atom(conn, b"WM_CHANGE_STATE")?,
            user_time: intern_atom(conn, b"_NET_WM_USER_TIME")?,
            utf8: intern_atom(conn, b"UTF8_STRING")?,
        })
    }
}

/// What the system last told us of one window, in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Geometry {
    /// The content's top-left on the root, and its size.
    pub origin: (i32, i32),
    pub size: PhysicalSize,
    /// Whether a window manager put the window in a frame of its own.
    pub reparented: bool,
    /// The frame around the content: left, right, top, bottom.
    pub frame: [u32; 4],
    pub maximized: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    /// The desktop's work area: x, y, width, height.
    pub work_area: Option<[u32; 4]>,
}

impl Geometry {
    pub fn new(origin: (i32, i32), size: PhysicalSize) -> Self {
        Self {
            origin,
            size,
            ..Default::default()
        }
    }

    /// Take in a `ConfigureNotify`. Returns whether the content's position
    /// changed (its size is reported as a resize).
    pub fn configure(&mut self, ev: &[u8; 32]) -> bool {
        let synthetic = ev[0] & 0x80 != 0;
        let x = i16::from_le_bytes([ev[16], ev[17]]) as i32;
        let y = i16::from_le_bytes([ev[18], ev[19]]) as i32;
        let w = u16::from_le_bytes([ev[20], ev[21]]) as u32;
        let h = u16::from_le_bytes([ev[22], ev[23]]) as u32;
        self.size = PhysicalSize::new(w, h);
        // Inside a frame, a real event's position is relative to the frame.
        if synthetic || !self.reparented {
            let moved = self.origin != (x, y);
            self.origin = (x, y);
            return moved;
        }
        false
    }

    /// Take in a `ReparentNotify` naming `root` as the root window.
    pub fn reparent(&mut self, ev: &[u8; 32], root: u32) {
        let parent = rd_u32(ev, 12).unwrap_or(0);
        self.reparented = parent != root;
        if !self.reparented {
            let x = i16::from_le_bytes([ev[16], ev[17]]) as i32;
            let y = i16::from_le_bytes([ev[18], ev[19]]) as i32;
            self.origin = (x, y);
        }
    }

    /// The platform-neutral view of it, in logical pixels.
    pub fn info(&self, scale: ScaleFactor) -> WindowInfo {
        let s = scale.get();
        let [left, right, top, bottom] = self.frame.map(|v| v as f64);
        let inner = scale.to_logical(self.size);
        WindowInfo {
            position: Some(Point::new(
                (self.origin.0 as f64 - left) / s,
                (self.origin.1 as f64 - top) / s,
            )),
            inner_size: inner,
            outer_size: Size::new(
                inner.width + (left + right) / s,
                inner.height + (top + bottom) / s,
            ),
            scale,
            maximized: self.maximized,
            minimized: self.minimized,
            fullscreen: self.fullscreen,
            work_area: self.work_area.map(|[x, y, w, h]| {
                Rect::from_xywh(x as f64 / s, y as f64 / s, w as f64 / s, h as f64 / s)
            }),
        }
    }
}

/// `WM_NAME`'s type and bytes for `title`: Latin-1 `STRING` where it fits,
/// else UTF-8.
pub(super) fn wm_name(title: &str, utf8: u32) -> (u32, Vec<u8>) {
    if title.chars().all(|c| (c as u32) < 0x100) {
        (ATOM_STRING, title.chars().map(|c| c as u8).collect())
    } else {
        (utf8, title.as_bytes().to_vec())
    }
}

/// Set the window's title.
pub(super) fn set_title(conn: &mut Conn, window: u32, title: &str) -> io::Result<()> {
    let utf8 = conn.geo.utf8;
    let (ty, bytes) = wm_name(title, utf8);
    super::set_property_str(conn, window, ATOM_WM_NAME, ty, &bytes)?;
    let net = conn.geo.net_wm_name;
    if net != 0 && utf8 != 0 {
        super::set_property_str(conn, window, net, utf8, title.as_bytes())?;
    }
    Ok(())
}

/// A `ConfigureWindow` request moving (`at`) and/or resizing (`size`) a
/// window, in physical pixels.
pub(super) fn configure_request(
    window: u32,
    at: Option<(i32, i32)>,
    size: Option<PhysicalSize>,
) -> Vec<u8> {
    let mut mask = 0u16;
    let mut values = Vec::new();
    if let Some((x, y)) = at {
        mask |= 0x1 | 0x2;
        values.extend([x as u32, y as u32]);
    }
    if let Some(size) = size {
        mask |= 0x4 | 0x8;
        values.extend([size.width.max(1), size.height.max(1)]);
    }
    let mut req = vec![OP_CONFIGURE_WINDOW, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&mask.to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes());
    for v in values {
        req.extend_from_slice(&v.to_le_bytes());
    }
    finish(req)
}

/// Map (show) or unmap (hide) a window.
pub(super) fn map_request(window: u32, visible: bool) -> Vec<u8> {
    let op = if visible {
        OP_MAP_WINDOW
    } else {
        OP_UNMAP_WINDOW
    };
    let mut req = vec![op, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    finish(req)
}

/// A client message about `window` to the root window, as the window
/// manager wants them (EWMH / ICCCM): `SendEvent` with the substructure
/// masks.
pub(super) fn root_message(root: u32, window: u32, ty: u32, data: [u32; 5]) -> Vec<u8> {
    const SUBSTRUCTURE: u32 = 0x0018_0000;
    const CLIENT_MESSAGE: u8 = 33;
    let mut req = vec![OP_SEND_EVENT, 0, 0, 0];
    req.extend_from_slice(&root.to_le_bytes());
    req.extend_from_slice(&SUBSTRUCTURE.to_le_bytes());
    req.extend_from_slice(&[CLIENT_MESSAGE, 32, 0, 0]);
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&ty.to_le_bytes());
    for word in data {
        req.extend_from_slice(&word.to_le_bytes());
    }
    finish(req)
}

/// Minimize a window (ICCCM `WM_CHANGE_STATE` to `IconicState`), or bring it
/// back (map it and ask for it to be activated).
pub(super) fn set_minimized(conn: &mut Conn, window: u32, minimized: bool) -> io::Result<()> {
    let root = conn.setup.root;
    let geo = conn.geo;
    if minimized {
        conn.send(&root_message(
            root,
            window,
            geo.change_state,
            [ICONIC_STATE, 0, 0, 0, 0],
        ))
    } else {
        conn.send(&map_request(window, true))?;
        conn.send(&root_message(
            root,
            window,
            geo.active_window,
            [1, 0, 0, 0, 0],
        ))
    }
}

/// Read a format-32 property while the event loop runs: events that arrive
/// before the reply wait in `backlog`. `None` if the property is missing or
/// the request failed.
pub(super) fn get_property32(
    conn: &mut Conn,
    backlog: &mut VecDeque<[u8; 32]>,
    window: u32,
    prop: u32,
) -> io::Result<Option<Vec<u32>>> {
    let mut req = vec![OP_GET_PROPERTY, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&prop.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // any type
    req.extend_from_slice(&0u32.to_le_bytes()); // offset
    req.extend_from_slice(&1024u32.to_le_bytes()); // length, in words
    conn.send(&finish(req))?;
    loop {
        let mut packet = [0u8; 32];
        conn.stream.read_exact(&mut packet)?;
        match packet[0] {
            1 => {
                let extra = rd_u32(&packet, 4).unwrap_or(0) as usize * 4;
                let mut body = vec![0u8; extra];
                conn.stream.read_exact(&mut body)?;
                let format = packet[1];
                let count = rd_u32(&packet, 16).unwrap_or(0) as usize;
                if format != 32 {
                    return Ok(None);
                }
                return Ok(Some(
                    (0..count.min(extra / 4))
                        .filter_map(|i| rd_u32(&body, i * 4))
                        .collect(),
                ));
            }
            // An error: ours if it names GetProperty; an earlier request's
            // otherwise, which the loop logs.
            0 if packet[10] == OP_GET_PROPERTY => return Ok(None),
            _ => backlog.push_back(packet),
        }
    }
}

/// Re-read the window's state, frame and the work area into `geo`. Returns
/// whether anything changed.
pub(super) fn refresh(
    conn: &mut Conn,
    backlog: &mut VecDeque<[u8; 32]>,
    window: u32,
    geo: &mut Geometry,
) -> io::Result<bool> {
    let before = *geo;
    let atoms = conn.geo;
    let (state, max_v, max_h, full) = (
        conn.atom_net_wm_state,
        conn.atom_max_vert,
        conn.atom_max_horz,
        conn.atom_fullscreen,
    );
    if state != 0 {
        let states = get_property32(conn, backlog, window, state)?.unwrap_or_default();
        geo.maximized = states.contains(&max_v) && states.contains(&max_h);
        geo.fullscreen = states.contains(&full);
        geo.minimized = atoms.hidden != 0 && states.contains(&atoms.hidden);
    }
    if atoms.frame_extents != 0 {
        let extents = get_property32(conn, backlog, window, atoms.frame_extents)?;
        geo.frame = match extents.as_deref() {
            Some([l, r, t, b, ..]) => [*l, *r, *t, *b],
            _ => [0; 4],
        };
    }
    if atoms.workarea != 0 {
        let root = conn.setup.root;
        let area = get_property32(conn, backlog, root, atoms.workarea)?;
        geo.work_area = match area.as_deref() {
            Some([x, y, w, h, ..]) => Some([*x, *y, *w, *h]),
            _ => None,
        };
    }
    Ok(*geo != before)
}

/// Whether a property change on a window is one [`refresh`] reads.
pub(super) fn watches(conn: &Conn, atom: u32) -> bool {
    atom != 0 && (atom == conn.atom_net_wm_state || atom == conn.geo.frame_extents)
}

/// Tell the window manager not to focus a window when it maps
/// (`_NET_WM_USER_TIME` 0, EWMH).
pub(super) fn no_focus_on_map(conn: &mut Conn, window: u32) -> io::Result<()> {
    let prop = conn.geo.user_time;
    if prop == 0 {
        return Ok(());
    }
    super::set_property32(conn, window, prop, ATOM_CARDINAL, &[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configure(x: i16, y: i16, w: u16, h: u16, synthetic: bool) -> [u8; 32] {
        let mut ev = [0u8; 32];
        ev[0] = 22 | if synthetic { 0x80 } else { 0 };
        ev[16..18].copy_from_slice(&x.to_le_bytes());
        ev[18..20].copy_from_slice(&y.to_le_bytes());
        ev[20..22].copy_from_slice(&w.to_le_bytes());
        ev[22..24].copy_from_slice(&h.to_le_bytes());
        ev
    }

    #[test]
    fn positions_come_from_the_root_or_the_window_manager() {
        let mut geo = Geometry::new((0, 0), PhysicalSize::new(10, 10));
        // No window manager: the real event is on the root.
        assert!(geo.configure(&configure(30, 40, 200, 100, false)));
        assert_eq!(geo.origin, (30, 40));
        assert_eq!(geo.size, PhysicalSize::new(200, 100));
        // In a frame: real events are frame-relative and ignored...
        let mut reparent = [0u8; 32];
        reparent[12..16].copy_from_slice(&7u32.to_le_bytes());
        geo.reparent(&reparent, 1);
        assert!(geo.reparented);
        assert!(!geo.configure(&configure(4, 20, 210, 100, false)));
        assert_eq!(geo.origin, (30, 40));
        assert_eq!(geo.size, PhysicalSize::new(210, 100));
        // ...and the window manager's own say where it is.
        assert!(geo.configure(&configure(104, 120, 210, 100, true)));
        assert_eq!(geo.origin, (104, 120));
    }

    #[test]
    fn info_takes_the_frame_out_and_scales_to_logical() {
        let mut geo = Geometry::new((104, 120), PhysicalSize::new(400, 200));
        geo.frame = [4, 4, 20, 4];
        geo.maximized = true;
        geo.work_area = Some([0, 32, 1920, 1048]);
        let info = geo.info(ScaleFactor::new(2.0));
        assert_eq!(info.position, Some(Point::new(50.0, 50.0)));
        assert_eq!(info.inner_size, Size::new(200.0, 100.0));
        assert_eq!(info.outer_size, Size::new(204.0, 112.0));
        assert!(info.maximized && !info.fullscreen);
        assert_eq!(
            info.work_area,
            Some(Rect::from_xywh(0.0, 16.0, 960.0, 524.0))
        );
    }

    #[test]
    fn wm_name_is_latin1_where_it_fits() {
        assert_eq!(
            wm_name("café", 99),
            (ATOM_STRING, vec![b'c', b'a', b'f', 0xe9])
        );
        assert_eq!(wm_name("日本", 99), (99, "日本".as_bytes().to_vec()));
    }

    #[test]
    fn configure_window_requests_are_well_formed() {
        let req = configure_request(0x42, Some((-5, 7)), Some(PhysicalSize::new(300, 0)));
        assert_eq!(req[0], OP_CONFIGURE_WINDOW);
        assert_eq!(u16::from_le_bytes([req[2], req[3]]) as usize * 4, req.len());
        assert_eq!(rd_u32(&req, 4), Some(0x42));
        assert_eq!(u16::from_le_bytes([req[8], req[9]]), 0xf);
        assert_eq!(rd_u32(&req, 12), Some(-5i32 as u32));
        assert_eq!(rd_u32(&req, 16), Some(7));
        assert_eq!(rd_u32(&req, 20), Some(300));
        assert_eq!(rd_u32(&req, 24), Some(1), "at least a pixel");
        let moved = configure_request(1, Some((1, 2)), None);
        assert_eq!(moved.len(), 20);
        assert_eq!(u16::from_le_bytes([moved[8], moved[9]]), 0x3);
    }

    #[test]
    fn root_messages_are_client_messages_to_the_window_manager() {
        let req = root_message(1, 2, 3, [4, 5, 6, 7, 8]);
        assert_eq!(req.len(), 44);
        assert_eq!(req[0], OP_SEND_EVENT);
        assert_eq!(rd_u32(&req, 4), Some(1)); // to the root
        assert_eq!(req[12], 33); // ClientMessage
        assert_eq!(req[13], 32); // format
        assert_eq!(rd_u32(&req, 16), Some(2)); // about the window
        assert_eq!(rd_u32(&req, 20), Some(3)); // its type
        assert_eq!(rd_u32(&req, 24), Some(4));
        assert_eq!(map_request(9, false)[0], OP_UNMAP_WINDOW);
    }
}
