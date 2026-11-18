//! How a Linux desktop launched the app, and who it is to the window
//! manager: the startup-notification id (`DESKTOP_STARTUP_ID`, X11) and the
//! activation token (`XDG_ACTIVATION_TOKEN`, Wayland) a launcher hands its
//! child, and the `WM_CLASS` docks and taskbars match windows to the app's
//! `.desktop` file by.
//!
//! A launcher shows a busy cursor until the app says it started (on X11, a
//! `remove` message about its id), and compositors guarding against focus
//! stealing only focus a new window that comes with the launcher's token.
//! Both variables are taken out of the environment as they are read, so the
//! processes the app starts (a terminal's shells) don't pass a stale one on.

use std::sync::Mutex;

/// The launcher's hand-off, read once.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Startup {
    /// `DESKTOP_STARTUP_ID`, for X11's startup notification.
    pub desktop_startup_id: Option<String>,
    /// `XDG_ACTIVATION_TOKEN`, for Wayland's `xdg_activation_v1`.
    pub activation_token: Option<String>,
}

static TAKEN: Mutex<Option<Startup>> = Mutex::new(None);

/// Read both variables and remove them from the environment, the first time;
/// later calls get what was read then. The backends call it as they start,
/// before the app has had a chance to start any process of its own.
#[allow(unsafe_code)]
pub(crate) fn take() -> Startup {
    let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    taken
        .get_or_insert_with(|| {
            let read = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
            let startup = Startup {
                desktop_startup_id: read("DESKTOP_STARTUP_ID"),
                activation_token: read("XDG_ACTIVATION_TOKEN"),
            };
            // SAFETY: the backend reads these on its UI thread as it starts;
            // nothing in Stipple reads the environment from another thread
            // meanwhile (the hazard `remove_var` warns of is a concurrent
            // `getenv`). GTK and Qt unset them the same way.
            unsafe {
                std::env::remove_var("DESKTOP_STARTUP_ID");
                std::env::remove_var("XDG_ACTIVATION_TOKEN");
            }
            startup
        })
        .clone()
}

/// What [`take`] read, if a backend took the variables already: for
/// [`Launch::current`](crate::platform::Launch::current), which reports the
/// launch as it was (a launch forwarded to a running instance carries its
/// token).
pub(crate) fn taken() -> Option<Startup> {
    TAKEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The X server timestamp a startup id carries (`…_TIME<n>`), which is the
/// time of the user's action that launched the app.
pub(crate) fn startup_time(id: &str) -> Option<u32> {
    let at = id.rfind("_TIME")?;
    id[at + 5..].parse().ok()
}

/// The startup-notification message that says the launch with `id` is over
/// (`remove: ID="…"`), in the spec's quoting.
pub(crate) fn remove_message(id: &str) -> String {
    let mut quoted = String::with_capacity(id.len() + 2);
    quoted.push('"');
    for c in id.chars() {
        if matches!(c, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    format!("remove: ID={quoted}")
}

/// `message` cut into the 20-byte pieces `_NET_STARTUP_INFO_BEGIN` and
/// `_NET_STARTUP_INFO` client messages carry, NUL-terminated and padded with
/// NULs.
pub(crate) fn message_chunks(message: &str) -> Vec<[u8; 20]> {
    let mut bytes = message.as_bytes().to_vec();
    bytes.push(0);
    bytes
        .chunks(20)
        .map(|chunk| {
            let mut out = [0u8; 20];
            out[..chunk.len()].copy_from_slice(chunk);
            out
        })
        .collect()
}

/// The `WM_CLASS` property: the instance name (`RESOURCE_NAME`, else the
/// program's file name), then the class: the id the app gave
/// ([`App::app_info`](crate::App::app_info)), which Wayland's `app_id`
/// carries too, or else the instance name capitalized, as Xlib and GTK do.
pub(crate) fn wm_class(
    resource_name: Option<&str>,
    program: Option<&str>,
    given_id: Option<&str>,
) -> Vec<u8> {
    let instance = resource_name
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| {
            program
                .and_then(|p| std::path::Path::new(p).file_name())
                .map(|n| n.to_string_lossy().into_owned())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "stipple".to_string());
    let class = match given_id {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            let mut chars = instance.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    };
    let mut out = Vec::with_capacity(instance.len() + class.len() + 2);
    out.extend_from_slice(instance.as_bytes());
    out.push(0);
    out.extend_from_slice(class.as_bytes());
    out.push(0);
    out
}

/// This process's `WM_CLASS`.
pub(crate) fn own_wm_class() -> Vec<u8> {
    let resource = std::env::var("RESOURCE_NAME").ok();
    let program = std::env::args_os()
        .next()
        .map(|a| a.to_string_lossy().into_owned());
    let given = crate::platform::bundle::given_id();
    wm_class(resource.as_deref(), program.as_deref(), given.as_deref())
}

/// The machine's host name, for `WM_CLIENT_MACHINE` (which tells a window
/// manager that `_NET_WM_PID` is a process on its own machine).
pub(crate) fn host_name() -> Option<String> {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_startup_id_carries_the_launch_time() {
        assert_eq!(
            startup_time("gnome-shell-1234-host-cterm-0_TIME5678"),
            Some(5678)
        );
        assert_eq!(startup_time("no-time-here"), None);
        assert_eq!(startup_time("x_TIMEnope"), None);
    }

    #[test]
    fn the_remove_message_quotes_the_id_and_splits_into_twenty_bytes() {
        let msg = remove_message(r#"a"b\c"#);
        assert_eq!(msg, r#"remove: ID="a\"b\\c""#);
        // Twenty bytes exactly: the NUL goes in a piece of its own.
        let chunks = message_chunks(&msg);
        assert_eq!(chunks.len(), 2);
        assert_eq!(&chunks[0], msg.as_bytes());
        assert_eq!(chunks[1], [0; 20]);
        let chunks = message_chunks("remove: ID=\"launch-42\"");
        assert_eq!(chunks.len(), 2);
        assert_eq!(&chunks[1][..3], b"2\"\0");
        assert!(chunks[1][3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn wm_class_is_the_instance_then_the_class() {
        assert_eq!(
            wm_class(None, Some("/usr/bin/cterm"), Some("com.cterm.terminal")),
            b"cterm\0com.cterm.terminal\0"
        );
        assert_eq!(
            wm_class(None, Some("/usr/bin/cterm"), None),
            b"cterm\0Cterm\0"
        );
        assert_eq!(
            wm_class(Some("term"), Some("/usr/bin/cterm"), None),
            b"term\0Term\0"
        );
        assert_eq!(wm_class(None, None, None), b"stipple\0Stipple\0");
    }
}
