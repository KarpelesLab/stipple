//! Transparent windows (`WindowAttributes::transparent`) and the blur behind
//! them (`WindowAttributes::blur`).
//!
//! A window keeps its frames' alpha when it is created with a 32-bit
//! TrueColor visual (ARGB: alpha in the top byte) and a colormap of that
//! visual: a compositing manager then blends its premultiplied pixels over
//! what is behind. Without a compositing manager (no owner of
//! `_NET_WM_CM_S0`) nothing would blend them, so the window is made opaque as
//! usual. Blur is KDE's `_KDE_NET_WM_BLUR_BEHIND_REGION` (an empty region:
//! the whole window), which KWin, and picom with its blur on, honor.

use std::io;

use super::refresh::round_trip;
use super::{Conn, intern_atom, rd_u16, rd_u32};

/// `GetSelectionOwner`.
const OP_GET_SELECTION_OWNER: u8 = 23;
/// `CreateColormap`.
const OP_CREATE_COLORMAP: u8 = 78;
/// `CWBorderPixel`: an ARGB window needs a border pixel of its depth (the
/// root's would be a mismatch).
pub(super) const CW_BORDER_PIXEL: u32 = 0x0000_0008;
/// `CWColormap`.
pub(super) const CW_COLORMAP: u32 = 0x0000_2000;
/// The predefined `CARDINAL` atom.
const ATOM_CARDINAL: u32 = 6;

/// What transparent windows need, found once at startup.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Backdrop {
    /// The screen's 32-bit TrueColor visual, if it has one.
    pub(super) visual: Option<u32>,
    /// A compositing manager runs.
    compositing: bool,
    /// The colormap of `visual`, created on first use.
    colormap: u32,
    /// `_KDE_NET_WM_BLUR_BEHIND_REGION`.
    blur: u32,
}

/// The first 32-bit TrueColor visual with alpha in the top byte among a
/// screen's allowed depths, which follow the 40-byte `SCREEN` at `screen` in
/// the setup reply's data.
pub(super) fn argb_visual(data: &[u8], screen: usize) -> Option<u32> {
    let depths = *data.get(screen + 39)? as usize;
    let mut at = screen + 40;
    for _ in 0..depths {
        let depth = *data.get(at)?;
        let visuals = rd_u16(data, at + 2)? as usize;
        at += 8;
        for i in 0..visuals {
            let v = at + i * 24;
            let (id, class) = (rd_u32(data, v)?, *data.get(v + 4)?);
            let masks = (
                rd_u32(data, v + 8)?,
                rd_u32(data, v + 12)?,
                rd_u32(data, v + 16)?,
            );
            // TrueColor (4), with red, green and blue in the low 24 bits.
            if depth == 32 && class == 4 && masks == (0xff_0000, 0xff00, 0xff) {
                return Some(id);
            }
        }
        at += visuals * 24;
    }
    None
}

impl Backdrop {
    /// Find whether a compositing manager runs, and intern the blur atom.
    /// Call before any window exists, as replies are read directly.
    pub(super) fn probe(conn: &mut Conn, visual: Option<u32>) -> io::Result<Self> {
        let mut b = Self {
            visual,
            ..Self::default()
        };
        if visual.is_none() {
            return Ok(b);
        }
        let selection = intern_atom(conn, b"_NET_WM_CM_S0")?;
        let mut req = vec![OP_GET_SELECTION_OWNER, 0, 0, 0];
        req.extend_from_slice(&selection.to_le_bytes());
        b.compositing = round_trip(conn, req)?.is_some_and(|r| rd_u32(&r, 8).unwrap_or(0) != 0);
        b.blur = intern_atom(conn, b"_KDE_NET_WM_BLUR_BEHIND_REGION")?;
        super::dbg(format_args!(
            "ARGB visual {visual:#x?}, compositing manager: {}",
            b.compositing
        ));
        Ok(b)
    }

    /// The visual and colormap for a transparent window, or `None` where it
    /// would not show as one (no ARGB visual, or no compositing manager).
    pub(super) fn argb(conn: &mut Conn) -> Option<(u32, u32)> {
        let b = conn.backdrop;
        let visual = b.visual.filter(|_| b.compositing)?;
        if b.colormap == 0 {
            let colormap = conn.new_id();
            let mut req = vec![OP_CREATE_COLORMAP, 0 /* alloc None */, 0, 0];
            req.extend_from_slice(&colormap.to_le_bytes());
            req.extend_from_slice(&conn.setup.root.to_le_bytes());
            req.extend_from_slice(&visual.to_le_bytes());
            conn.send(&super::finish(req)).ok()?;
            conn.backdrop.colormap = colormap;
        }
        Some((visual, conn.backdrop.colormap))
    }

    /// Ask the compositing manager to blur what is behind all of `window`.
    pub(super) fn blur(conn: &mut Conn, window: u32) -> io::Result<()> {
        let atom = conn.backdrop.blur;
        if atom == 0 {
            return Ok(());
        }
        super::set_property32(conn, window, atom, ATOM_CARDINAL, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_argb_visual_among_the_depths() {
        // A SCREEN (40 bytes) with two depths: 24 (one visual) and 32 (two,
        // the second TrueColor with the RGB masks).
        let mut data = vec![0u8; 40];
        data[39] = 2;
        let visual = |id: u32, class: u8, masks: [u32; 3]| {
            let mut v = id.to_le_bytes().to_vec();
            v.extend_from_slice(&[class, 8, 0, 1]);
            for m in masks {
                v.extend_from_slice(&m.to_le_bytes());
            }
            v.extend_from_slice(&[0; 4]);
            v
        };
        data.extend_from_slice(&[24, 0, 1, 0, 0, 0, 0, 0]);
        data.extend(visual(0x21, 4, [0xff_0000, 0xff00, 0xff]));
        data.extend_from_slice(&[32, 0, 2, 0, 0, 0, 0, 0]);
        data.extend(visual(0x40, 5, [0xff_0000, 0xff00, 0xff]));
        data.extend(visual(0x41, 4, [0xff_0000, 0xff00, 0xff]));
        assert_eq!(argb_visual(&data, 0), Some(0x41));
        // No depth 32: none.
        data[39] = 1;
        assert_eq!(argb_visual(&data, 0), None);
    }
}
