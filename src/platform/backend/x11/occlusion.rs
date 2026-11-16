//! Whether an X11 window can be seen ([`Event::Occluded`](crate::platform::Event::Occluded)).
//!
//! Three things hide a window: being unmapped (`UnmapNotify`: ICCCM window
//! managers unmap a window they iconify, and the app hides its last window
//! that way), the window manager's `_NET_WM_STATE_HIDDEN` (minimized, or on
//! another workspace, read into the window's geometry), and the server's
//! `VisibilityNotify` saying it's fully obscured. Under a compositing
//! manager every window is drawn offscreen and reported unobscured, so
//! there only the first two say anything.

/// `VisibilityChangeMask`.
pub(super) const EV_VISIBILITY_CHANGE: u32 = 0x0001_0000;
/// `VisibilityNotify`: window at 4, state at 8.
pub(super) const X_VISIBILITY_NOTIFY: u8 = 15;
/// `UnmapNotify`: event window at 4, window at 8.
pub(super) const X_UNMAP_NOTIFY: u8 = 18;
/// `MapNotify`: event window at 4, window at 8.
pub(super) const X_MAP_NOTIFY: u8 = 19;
/// `VisibilityNotify`'s state: `FullyObscured`.
const FULLY_OBSCURED: u8 = 2;

/// What is known of a window's visibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Occlusion {
    unmapped: bool,
    minimized: bool,
    obscured: bool,
    /// What the app was last told.
    reported: bool,
}

/// The window an occlusion event is about, if `buf` is one.
pub(super) fn event_window(buf: &[u8; 32]) -> Option<u32> {
    let at = match buf[0] & 0x7f {
        X_VISIBILITY_NOTIFY => 4,
        X_UNMAP_NOTIFY | X_MAP_NOTIFY => 8,
        _ => return None,
    };
    Some(u32::from_le_bytes([
        buf[at],
        buf[at + 1],
        buf[at + 2],
        buf[at + 3],
    ]))
}

impl Occlusion {
    /// Take in a `VisibilityNotify`, `UnmapNotify` or `MapNotify`.
    pub(super) fn event(&mut self, buf: &[u8; 32]) {
        match buf[0] & 0x7f {
            X_VISIBILITY_NOTIFY => self.obscured = buf[8] == FULLY_OBSCURED,
            X_UNMAP_NOTIFY => self.unmapped = true,
            X_MAP_NOTIFY => {
                self.unmapped = false;
                // Mapped again: the server sends a new `VisibilityNotify`.
                self.obscured = false;
            }
            _ => {}
        }
    }

    /// The window manager's `_NET_WM_STATE_HIDDEN`.
    pub(super) fn set_minimized(&mut self, minimized: bool) {
        self.minimized = minimized;
    }

    /// Whether the window is hidden, if that changed since the app was last
    /// told.
    pub(super) fn change(&mut self) -> Option<bool> {
        let hidden = self.unmapped || self.minimized || self.obscured;
        (hidden != self.reported).then(|| {
            self.reported = hidden;
            hidden
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: u8, window: u32, state: u8) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0] = code;
        let at = if code == X_VISIBILITY_NOTIFY { 4 } else { 8 };
        b[at..at + 4].copy_from_slice(&window.to_le_bytes());
        if code == X_VISIBILITY_NOTIFY {
            b[8] = state;
        }
        b
    }

    #[test]
    fn a_window_is_hidden_while_anything_hides_it() {
        let mut o = Occlusion::default();
        assert_eq!(o.change(), None, "shown, as the app assumes");
        o.event(&ev(X_VISIBILITY_NOTIFY, 7, 1));
        assert_eq!(o.change(), None, "partly covered still shows");
        o.event(&ev(X_VISIBILITY_NOTIFY, 7, FULLY_OBSCURED));
        assert_eq!(o.change(), Some(true));
        o.set_minimized(true);
        assert_eq!(o.change(), None, "still hidden");
        o.event(&ev(X_VISIBILITY_NOTIFY, 7, 0));
        assert_eq!(o.change(), None, "uncovered, but minimized");
        o.set_minimized(false);
        assert_eq!(o.change(), Some(false));
        // Iconified by an ICCCM window manager: unmapped, then mapped.
        o.event(&ev(X_UNMAP_NOTIFY, 7, 0));
        assert_eq!(o.change(), Some(true));
        o.event(&ev(X_MAP_NOTIFY, 7, 0));
        assert_eq!(o.change(), Some(false));
    }

    #[test]
    fn events_name_their_window() {
        assert_eq!(event_window(&ev(X_VISIBILITY_NOTIFY, 9, 2)), Some(9));
        assert_eq!(
            event_window(&ev(X_UNMAP_NOTIFY, 0x40_0001, 0)),
            Some(0x40_0001)
        );
        assert_eq!(event_window(&ev(X_MAP_NOTIFY, 3, 0)), Some(3));
        assert_eq!(event_window(&[12u8; 32]), None);
        // A sent event (high bit set) is the same event.
        let mut sent = ev(X_UNMAP_NOTIFY, 5, 0);
        sent[0] |= 0x80;
        assert_eq!(event_window(&sent), Some(5));
    }
}
