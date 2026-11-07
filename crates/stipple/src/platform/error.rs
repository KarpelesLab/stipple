use core::fmt;

/// Errors raised while creating or driving a platform event loop or window.
#[derive(Debug)]
#[non_exhaustive]
pub enum PlatformError {
    /// No windowing backend is available for the current target (e.g. no
    /// Wayland/X11 display, or the target's backend is not yet implemented).
    NoBackend(&'static str),
    /// The OS refused or failed a windowing request.
    Os(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::NoBackend(s) => write!(f, "no windowing backend available: {s}"),
            PlatformError::Os(s) => write!(f, "platform error: {s}"),
        }
    }
}

impl std::error::Error for PlatformError {}
