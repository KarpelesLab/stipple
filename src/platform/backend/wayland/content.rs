//! GPU frames under decorations Stipple draws: the frame in a subsurface.
//!
//! Where Stipple draws the title bar (compositors without xdg-decoration:
//! GNOME's Mutter, Weston), a frame read back is framed on the CPU, title bar
//! and content in one `wl_shm` buffer. A GPU frame can't be: it is a
//! `dma-buf` the compositor composites as it is. So the window's surface
//! keeps the decorations alone, in a `wl_shm` buffer with a transparent hole
//! where the content goes, and the frame is shown in a `wl_subsurface` above
//! it, placed at the content's corner.
//!
//! The subsurface is synchronized (the default): its commits apply with the
//! window surface's next one, so a resize shows the new frame and the new
//! decorations together. Its input region is empty, so the pointer over the
//! content is over the window surface underneath, in the same coordinates
//! as with a framed `wl_shm` buffer: hit-testing, moving, resizing and the
//! app's pointer events don't change. The decorations are drawn again only
//! when they look different (hover, focus, title, size), from a frame or
//! from an event that changes them (see [`Content::refresh`]).
//!
//! Without a title bar (server-side decorations, or fullscreen) frames go to
//! the window surface itself, and the subsurface is emptied.

use super::{
    ShmBuffer, WL_SHM_FORMAT_ARGB8888, WL_SURFACE_ATTACH, WL_SURFACE_COMMIT, WL_SURFACE_DAMAGE,
    WL_SURFACE_SET_BUFFER_SCALE, WlConn, arg_u32, dbg, decor, make_shm, syncobj,
};
use crate::geometry::PhysicalSize;

/// The global's interface name.
pub(super) const SUBCOMPOSITOR: &str = "wl_subcompositor";
const SUBCOMPOSITOR_GET_SUBSURFACE: u16 = 1;
const SUBSURFACE_DESTROY: u16 = 0;
const SUBSURFACE_SET_POSITION: u16 = 1;
const WL_COMPOSITOR_CREATE_SURFACE: u16 = 0;
const WL_COMPOSITOR_CREATE_REGION: u16 = 1;
const WL_REGION_DESTROY: u16 = 0;
const WL_SURFACE_DESTROY: u16 = 0;
const WL_SURFACE_SET_INPUT_REGION: u16 = 5;
const WL_BUFFER_DESTROY: u16 = 0;

/// What the decorations were last drawn for: their look, the content size
/// (logical) and the scale.
type Key = (decor::Look, PhysicalSize, u32);

/// A window's GPU content subsurface and the decorations around it.
#[derive(Default)]
pub(super) struct Content {
    compositor: u32,
    subcompositor: Option<u32>,
    shm: u32,
    /// The window's surface.
    main: u32,
    /// The subsurface's `wl_surface` and `wl_subsurface`, once made.
    sub: Option<(u32, u32)>,
    /// Whether the subsurface shows a frame (else it's empty).
    active: bool,
    position: (i32, i32),
    deco: Option<ShmBuffer>,
    key: Option<Key>,
    /// The content's size (logical) and scale at the last frame.
    content: PhysicalSize,
    scale: u32,
}

impl Content {
    /// For window surface `main`, made from these globals.
    pub(super) fn new(compositor: u32, subcompositor: Option<u32>, shm: u32, main: u32) -> Self {
        Content {
            compositor,
            subcompositor,
            shm,
            main,
            ..Content::default()
        }
    }

    /// The window's surface.
    pub(super) fn main(&self) -> u32 {
        self.main
    }

    /// Whether frames can go under decorations Stipple draws.
    pub(super) fn available(&self) -> bool {
        self.subcompositor.is_some()
    }

    /// The surface the next GPU frame goes to (the window's decorations in
    /// use on `conn`): the subsurface under a title bar, else the window's
    /// surface (the subsurface emptied).
    pub(super) fn target(&mut self, conn: &mut WlConn) -> std::io::Result<u32> {
        let Some(subcompositor) = self.subcompositor else {
            return Ok(self.main);
        };
        if conn.decor.title_height() <= 0.0 {
            self.leave(conn);
            return Ok(self.main);
        }
        if let Some((surface, _)) = self.sub {
            return Ok(surface);
        }
        let surface = conn.new_id();
        conn.send(
            self.compositor,
            WL_COMPOSITOR_CREATE_SURFACE,
            &surface.to_le_bytes(),
        )?;
        let subsurface = conn.new_id();
        let mut a = Vec::new();
        for v in [subsurface, surface, self.main] {
            arg_u32(&mut a, v);
        }
        conn.send(subcompositor, SUBCOMPOSITOR_GET_SUBSURFACE, &a)?;
        // An empty input region: the pointer goes through to the window.
        let region = conn.new_id();
        conn.send(
            self.compositor,
            WL_COMPOSITOR_CREATE_REGION,
            &region.to_le_bytes(),
        )?;
        conn.send(surface, WL_SURFACE_SET_INPUT_REGION, &region.to_le_bytes())?;
        conn.send(region, WL_REGION_DESTROY, &[])?;
        self.sub = Some((surface, subsurface));
        self.position = (0, 0);
        dbg(format_args!(
            "GPU frames in subsurface {surface} under the decorations of {}",
            self.main
        ));
        say_subsurface();
        Ok(surface)
    }

    /// After a frame was committed to the subsurface: place it, draw the
    /// decorations if they changed, give the window geometry and commit the
    /// window surface (which applies both). `content` in logical pixels.
    pub(super) fn finish(
        &mut self,
        conn: &mut WlConn,
        content: PhysicalSize,
        scale: u32,
    ) -> std::io::Result<()> {
        self.active = true;
        self.content = content;
        self.scale = scale.max(1);
        self.update(conn)?;
        conn.send(self.main, WL_SURFACE_COMMIT, &[])
    }

    /// Draw the decorations again where they look different (the window's
    /// decorations in use on `conn`), committing them; whether there was a
    /// frame under them to do it for.
    pub(super) fn refresh(&mut self, conn: &mut WlConn) -> bool {
        if !self.active || conn.decor.title_height() <= 0.0 {
            return false;
        }
        let key = (conn.decor.look(), self.content, self.scale);
        if self.key.as_ref() != Some(&key) && self.update(conn).is_ok() {
            let _ = conn.send(self.main, WL_SURFACE_COMMIT, &[]);
        }
        true
    }

    /// Position, decorations and geometry, for the next window commit.
    fn update(&mut self, conn: &mut WlConn) -> std::io::Result<()> {
        let s = self.scale;
        let (l, t, _, _) = conn.decor.insets();
        let position = (l as i32, t as i32);
        if let Some((_, subsurface)) = self.sub
            && position != self.position
        {
            let mut a = Vec::new();
            arg_u32(&mut a, position.0 as u32);
            arg_u32(&mut a, position.1 as u32);
            conn.send(subsurface, SUBSURFACE_SET_POSITION, &a)?;
            self.position = position;
        }
        let key = (conn.decor.look(), self.content, s);
        if self.key.as_ref() != Some(&key) {
            let physical = PhysicalSize::new(self.content.width * s, self.content.height * s);
            if let Some((size, argb)) = conn.decor.around(physical, s) {
                if self.deco.as_ref().map(|b| b.size) != Some(size) {
                    if let Some(old) = self.deco.take() {
                        conn.send(old.buffer, WL_BUFFER_DESTROY, &[])?;
                    }
                    self.deco = make_shm(conn, self.shm, size, WL_SHM_FORMAT_ARGB8888).ok();
                }
                if let Some(buf) = self.deco.as_mut() {
                    buf.bytes()[..argb.len()].copy_from_slice(&argb);
                    let buffer = buf.buffer;
                    // No sync object on a surface showing a wl_shm buffer.
                    syncobj::forget(conn, self.main);
                    conn.send(self.main, WL_SURFACE_SET_BUFFER_SCALE, &s.to_le_bytes())?;
                    let mut a = Vec::new();
                    for v in [buffer, 0, 0] {
                        arg_u32(&mut a, v);
                    }
                    conn.send(self.main, WL_SURFACE_ATTACH, &a)?;
                    let mut a = Vec::new();
                    for v in [0, 0, size.width / s, size.height / s] {
                        arg_u32(&mut a, v);
                    }
                    conn.send(self.main, WL_SURFACE_DAMAGE, &a)?;
                }
            }
            self.key = Some(key);
        }
        decor::send_geometry(conn, self.content);
        Ok(())
    }

    /// Empty the subsurface, if it shows a frame (applied by the window
    /// surface's next commit): frames go to the window surface again.
    pub(super) fn leave(&mut self, conn: &mut WlConn) {
        if !std::mem::take(&mut self.active) {
            return;
        }
        self.key = None;
        if let Some((surface, _)) = self.sub {
            let mut a = Vec::new();
            for v in [0u32, 0, 0] {
                arg_u32(&mut a, v);
            }
            let _ = conn.send(surface, WL_SURFACE_ATTACH, &a);
            let _ = conn.send(surface, WL_SURFACE_COMMIT, &[]);
        }
    }

    /// Destroy the subsurface and the decorations' buffer (before the
    /// window surface goes).
    pub(super) fn destroy(&mut self, conn: &mut WlConn) {
        if let Some((surface, subsurface)) = self.sub.take() {
            syncobj::forget(conn, surface);
            let _ = conn.send(subsurface, SUBSURFACE_DESTROY, &[]);
            let _ = conn.send(surface, WL_SURFACE_DESTROY, &[]);
        }
        if let Some(buf) = self.deco.take() {
            let _ = conn.send(buf.buffer, WL_BUFFER_DESTROY, &[]);
        }
        self.active = false;
        self.key = None;
    }
}

/// Say once that GPU frames go under Stipple's decorations (CI looks for it).
fn say_subsurface() {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        eprintln!("stipple: GPU frames go in a subsurface under the title bar Stipple draws")
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(args: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(args[i * 4..i * 4 + 4].try_into().unwrap())
    }

    /// Every message the client sent, as `(object, opcode, args)`.
    fn sent(peer: &mut std::os::unix::net::UnixStream) -> Vec<(u32, u16, Vec<u8>)> {
        use std::io::Read;
        peer.set_nonblocking(true).unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0u8; 65536];
        while let Ok(n) = peer.read(&mut buf) {
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
        }
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= bytes.len() {
            let object = word(&bytes[at..], 0);
            let w = word(&bytes[at..], 1);
            let size = (w >> 16) as usize;
            out.push((
                object,
                (w & 0xffff) as u16,
                bytes[at + 8..at + size].to_vec(),
            ));
            at += size;
        }
        out
    }

    #[test]
    fn frames_go_under_a_title_bar_and_to_the_window_without_one() {
        use std::io::Write;
        let (mut conn, mut peer) = super::super::clipboard::tests::conn_pair();
        let attrs = crate::platform::window::WindowAttributes::default();
        // No decoration manager: Stipple draws them.
        decor::setup(&mut conn, None, 50, 51, None, &attrs);
        const COMPOSITOR: u32 = 1000;
        const SUBCOMPOSITOR: u32 = 1001;
        const MAIN: u32 = 1002;
        let mut content = Content::new(COMPOSITOR, Some(SUBCOMPOSITOR), 5, MAIN);
        let target = content.target(&mut conn).unwrap();
        assert_ne!(target, MAIN);
        content
            .finish(&mut conn, PhysicalSize::new(200, 100), 1)
            .unwrap();
        conn.stream.flush().unwrap();
        let msgs = sent(&mut peer);
        // The subsurface: made, under the window surface, no input region.
        let get = msgs
            .iter()
            .find(|m| m.0 == SUBCOMPOSITOR && m.1 == SUBCOMPOSITOR_GET_SUBSURFACE)
            .unwrap();
        assert_eq!((word(&get.2, 1), word(&get.2, 2)), (target, MAIN));
        assert!(
            msgs.iter()
                .any(|m| m.0 == target && m.1 == WL_SURFACE_SET_INPUT_REGION)
        );
        // Placed at the content's corner: past the margin and the title bar.
        let (l, t, _, _) = conn.decor.insets();
        let subsurface = word(&get.2, 0);
        let pos = msgs
            .iter()
            .find(|m| m.0 == subsurface && m.1 == SUBSURFACE_SET_POSITION)
            .unwrap();
        assert_eq!((word(&pos.2, 0), word(&pos.2, 1)), (l as u32, t as u32));
        // The decorations' buffer attached to the window, which commits.
        assert!(msgs.iter().any(|m| m.0 == MAIN && m.1 == WL_SURFACE_ATTACH));
        assert_eq!(
            msgs.last().map(|m| (m.0, m.1)),
            Some((MAIN, WL_SURFACE_COMMIT))
        );
        // Nothing changed: no new decorations; a hover does.
        assert!(content.refresh(&mut conn));
        conn.stream.flush().unwrap();
        assert!(sent(&mut peer).is_empty());
        conn.decor.motion(
            crate::geometry::Point::new(l + 199.0, l + 5.0),
            PhysicalSize::new(200, 100),
        );
        assert!(content.refresh(&mut conn));
        conn.stream.flush().unwrap();
        let msgs = sent(&mut peer);
        assert!(msgs.iter().any(|m| m.0 == MAIN && m.1 == WL_SURFACE_ATTACH));
        // Fullscreen: no title bar, frames to the window, the subsurface
        // emptied.
        conn.decor.set_states(&[2]);
        assert_eq!(content.target(&mut conn).unwrap(), MAIN);
        conn.stream.flush().unwrap();
        let msgs = sent(&mut peer);
        let attach = msgs
            .iter()
            .find(|m| m.0 == target && m.1 == WL_SURFACE_ATTACH)
            .unwrap();
        assert_eq!(word(&attach.2, 0), 0);
        assert!(!content.refresh(&mut conn));
        content.destroy(&mut conn);
    }
}
