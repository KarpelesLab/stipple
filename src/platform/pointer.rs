//! The modifier keys held at the last pointer event, and the platform's
//! double-click settings.
//!
//! Pointer events carry no modifiers, so each backend records them here just
//! before delivering a button, move or wheel event, and the app and widgets
//! read them back (through `stipple::core::pointer`, and in each
//! [`PointerEvent`](crate::core::PointerEvent)) to tell a Shift+click or
//! Ctrl+click from a plain one. Per thread: events are delivered on the UI
//! thread.

use crate::platform::Modifiers;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

thread_local! {
    static MODIFIERS: Cell<Modifiers> = const {
        Cell::new(Modifiers {
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        })
    };
}

/// The modifier keys held at the last pointer button event on this thread.
pub fn pointer_modifiers() -> Modifiers {
    MODIFIERS.with(Cell::get)
}

/// Record the modifier keys held at a pointer button event. Backends call this
/// before delivering the event; tests call it to simulate Shift/Ctrl+click.
pub fn set_pointer_modifiers(modifiers: Modifiers) {
    MODIFIERS.with(|m| m.set(modifiers));
}

/// How close in time and space two presses must be to make a double (or
/// triple) click.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoubleClick {
    /// The longest gap between presses.
    pub time: Duration,
    /// How far (logical pixels, on each axis) the pointer may drift between
    /// presses.
    pub distance: f64,
}

impl Default for DoubleClick {
    /// 500 ms and 4 px: the platforms' usual defaults.
    fn default() -> Self {
        Self {
            time: Duration::from_millis(500),
            distance: 4.0,
        }
    }
}

/// The double-click time (ms) and distance (hundredths of a pixel), packed;
/// 0 for the default.
static DOUBLE_CLICK: AtomicU64 = AtomicU64::new(0);

/// The platform's double-click time and distance, where the backend could
/// read them (Windows' `GetDoubleClickTime`, macOS's
/// `NSEvent.doubleClickInterval`, the X settings daemon's
/// `Net/DoubleClickTime`), else [`DoubleClick::default`].
pub fn double_click() -> DoubleClick {
    let packed = DOUBLE_CLICK.load(Ordering::Relaxed);
    let default = DoubleClick::default();
    let (ms, dist) = (packed >> 32, packed & 0xffff_ffff);
    DoubleClick {
        time: if ms == 0 {
            default.time
        } else {
            Duration::from_millis(ms)
        },
        distance: if dist == 0 {
            default.distance
        } else {
            dist as f64 / 100.0
        },
    }
}

/// Record the platform's double-click settings, `None` keeping the default
/// for that one. Backends call this as they start; tests may too.
pub fn set_double_click(time: Option<Duration>, distance: Option<f64>) {
    let ms = time.map_or(0, |t| (t.as_millis() as u64).clamp(1, u32::MAX as u64));
    let dist = distance.map_or(0, |d| {
        ((d * 100.0).round() as u64).clamp(1, u32::MAX as u64)
    });
    DOUBLE_CLICK.store(ms << 32 | dist, Ordering::Relaxed);
}
