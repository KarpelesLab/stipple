//! Where the accessibility bus is, looked up as `libatspi` does: the
//! `$AT_SPI_BUS_ADDRESS` variable, then the X root window's `AT_SPI_BUS`
//! property (which `at-spi-bus-launcher` sets on an X display, so apps on
//! that display find the bus even when they can't reach the session bus
//! that launched it: started over ssh with X forwarding, under `sudo`, or
//! in a sandbox), then `org.a11y.Bus.GetAddress` on the session bus. Each
//! is tried in turn until one connects (a stale property, left by a
//! launcher that has gone, falls through to the session bus).
//!
//! The X11 backend reads the property when it connects to the display and
//! hands it over with [`set_x11_address`]; under Wayland there is none.

use std::sync::Mutex;

use crate::platform::a11y::DBus;

/// The X root window's `AT_SPI_BUS`, as the X11 backend read it.
static X11_ADDRESS: Mutex<Option<String>> = Mutex::new(None);

/// Record the X root window's `AT_SPI_BUS` property (`None`: there was
/// none), for the bridge's next look for the bus.
pub(crate) fn set_x11_address(address: Option<String>) {
    let address = address
        .map(|a| a.trim_end_matches('\0').trim().to_string())
        .filter(|a| !a.is_empty());
    super::debug(format_args!("X root window's AT_SPI_BUS: {address:?}"));
    *X11_ADDRESS.lock().unwrap_or_else(|e| e.into_inner()) = address;
}

/// Where to look, in order: what the variable says, what the X property
/// says (`x11`), and the session bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Variable(String),
    X11(String),
    Session,
}

/// The places to look for the bus, in order (see the module docs).
pub(super) fn sources(variable: Option<String>, x11: Option<String>) -> Vec<Source> {
    let mut out = Vec::new();
    if let Some(v) = variable.filter(|v| !v.is_empty()) {
        out.push(Source::Variable(v));
    }
    if let Some(x) = x11.filter(|x| !x.is_empty()) {
        out.push(Source::X11(x));
    }
    out.push(Source::Session);
    out
}

/// The session bus's own address: the variable, else the systemd user bus.
fn session_address() -> Result<String, String> {
    match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(a) if !a.is_empty() => Ok(a),
        _ => {
            let dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "no session bus")?;
            let path = format!("{dir}/bus");
            if !std::path::Path::new(&path).exists() {
                return Err("no session bus".into());
            }
            Ok(format!("unix:path={path}"))
        }
    }
}

/// The accessibility bus's address, from the session bus's `org.a11y.Bus`.
fn from_session() -> Result<String, String> {
    let mut bus = DBus::connect_address(&session_address()?)?;
    bus.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    bus.a11y_bus_address()
}

/// Connect to the accessibility bus, trying each place in turn; the
/// connection and its address.
pub(super) fn connect() -> Result<(DBus, String), String> {
    let x11 = X11_ADDRESS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let mut errors = Vec::new();
    for source in sources(std::env::var("AT_SPI_BUS_ADDRESS").ok(), x11) {
        let (how, addr) = match source {
            Source::Variable(a) => ("$AT_SPI_BUS_ADDRESS", Ok(a)),
            Source::X11(a) => ("the X root window's AT_SPI_BUS", Ok(a)),
            Source::Session => ("org.a11y.Bus", from_session()),
        };
        match addr.and_then(|a| DBus::connect_address(&a).map(|bus| (bus, a))) {
            Ok((bus, addr)) => {
                super::debug(format_args!("accessibility bus from {how}: {addr}"));
                return Ok((bus, addr));
            }
            Err(e) => errors.push(format!("{how}: {e}")),
        }
    }
    Err(errors.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_then_the_x_property_then_the_session_bus() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            sources(s("unix:path=/a"), s("unix:path=/b")),
            vec![
                Source::Variable("unix:path=/a".into()),
                Source::X11("unix:path=/b".into()),
                Source::Session
            ]
        );
        assert_eq!(
            sources(s(""), s("unix:path=/b")),
            vec![Source::X11("unix:path=/b".into()), Source::Session]
        );
        assert_eq!(sources(None, None), vec![Source::Session]);
    }
}
