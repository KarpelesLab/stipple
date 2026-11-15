//! No camera support (Windows, Linux, and anything else): no lenses, and
//! opening says so.

use super::{CameraError, CameraOptions, Caps, Lens, Session, Sink};
use std::sync::Arc;

/// Whether Stipple captures from cameras here.
pub(super) const SUPPORTED: bool = false;

pub(super) fn lenses() -> Vec<Lens> {
    Vec::new()
}

pub(super) fn open(
    _sink: Arc<Sink>,
    _options: &CameraOptions,
) -> Result<(Box<dyn Session>, Caps), CameraError> {
    Err(CameraError::Unsupported)
}

pub(super) fn display_rotation() -> u32 {
    0
}
