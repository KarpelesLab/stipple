//! The single instance over the session bus, as GApplication does it: the
//! running instance owns the app's id as a well-known bus name and serves
//! `org.freedesktop.Application` (the Desktop Entry spec's D-Bus activation
//! interface) at the id's object path; a second launch calls it. With a
//! `.service` file naming the app (`cargo stipple linux` writes one), the
//! bus also starts the app on demand: a desktop that sees
//! `DBusActivatable=true` in the `.desktop` file calls `Activate`, `Open` or
//! `ActivateAction` instead of running `Exec`, and the bus runs the
//! service's `Exec` first if nobody owns the name.
//!
//! | Call | The [`Launch`] it gives |
//! |---|---|
//! | `Activate(a{sv} platform_data)` | no arguments |
//! | `Open(as uris, a{sv} platform_data)` | the URIs as arguments, `file:` ones as paths |
//! | `ActivateAction(s name, av parameter, a{sv} platform_data)` | [`Launch::action`], and a string parameter as [`Launch::action_target`] |
//!
//! `platform_data`'s `activation-token` (Wayland) and `desktop-startup-id`
//! (X11) become the launch's [`activation_token`](Launch::activation_token)
//! and its `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID`; `cwd` (a
//! bytestring, as GLib sends it) its directory. A Stipple app forwarding
//! its own launch adds `stipple-args` (`as`), its exact command line, which
//! the running instance takes over the URIs.

use std::path::PathBuf;

use super::{Claim, Launch};
use crate::platform::a11y::{DBus, Message, align_to, read_string, read_u32, skip_value};
use crate::platform::shell::linux::W;

/// The interface, as the Desktop Entry spec names it.
const IFACE: &str = "org.freedesktop.Application";
/// `RequestName`'s `DBUS_NAME_FLAG_DO_NOT_QUEUE`, and its replies.
const DO_NOT_QUEUE: u32 = 4;
const PRIMARY_OWNER: u32 = 1;
const ALREADY_OWNER: u32 = 4;
/// The `platform_data` key carrying a Stipple launch's exact arguments.
const ARGS_KEY: &str = "stipple-args";
/// GApplication's command-line switch for a process the bus starts, which
/// the `.service` files `cargo stipple` writes pass.
pub(crate) const SERVICE_SWITCH: &str = "--gapplication-service";

fn debug(args: std::fmt::Arguments<'_>) {
    crate::platform::window::debug_window(format_args!("single instance (D-Bus): {args}"));
}

/// The object path GApplication serves an id at: `/` and the id with `.`
/// as `/` and `-` as `_` (`org.example.My-App` → `/org/example/My_App`).
pub(crate) fn object_path(id: &str) -> String {
    let mut path = String::from("/");
    for c in id.chars() {
        path.push(match c {
            '.' => '/',
            c if c.is_ascii_alphanumeric() || c == '_' => c,
            _ => '_',
        });
    }
    path
}

/// Own `id` on the session bus and serve `org.freedesktop.Application`, or
/// hand `launch` to the process that owns it. `Unavailable` when there's no
/// session bus, `id` isn't a bus name, or the owner doesn't answer.
pub(super) fn claim(id: &str, launch: &Launch) -> Claim {
    let mut bus = match DBus::connect_session() {
        Ok(bus) => bus,
        Err(e) => {
            debug(format_args!("no session bus: {e}"));
            return Claim::Unavailable;
        }
    };
    bus.set_read_timeout(Some(super::PATIENCE));
    match bus.request_name_flags(id, DO_NOT_QUEUE) {
        Ok(PRIMARY_OWNER | ALREADY_OWNER) => {
            bus.set_read_timeout(None);
            let path = object_path(id);
            debug(format_args!("owns {id} at {path}"));
            serve(bus, path, started_by_bus());
            Claim::Primary
        }
        Ok(_) => {
            let path = object_path(id);
            let (member, signature, body) = call_for(launch);
            match bus.call_with_body(id, &path, IFACE, member, signature, &body) {
                Ok(_) => {
                    debug(format_args!("{member} on {id}"));
                    Claim::Forwarded
                }
                Err(e) => {
                    debug(format_args!("{member} on {id} failed: {e}"));
                    Claim::Unavailable
                }
            }
        }
        Err(e) => {
            debug(format_args!("RequestName {id}: {e}"));
            Claim::Unavailable
        }
    }
}

/// Whether the session bus started this process (for a call it holds until
/// the name is owned): `dbus-daemon` sets `DBUS_STARTER_BUS_TYPE`, and
/// GApplication's `.service` files pass [`SERVICE_SWITCH`].
fn started_by_bus() -> bool {
    std::env::var_os("DBUS_STARTER_BUS_TYPE").is_some_and(|t| t == "session")
        || std::env::args_os().any(|a| a == SERVICE_SWITCH)
}

/// Serve the calls on a thread for the process's life, each launch
/// delivered as a second launch's is.
fn serve(mut bus: DBus, path: String, started: bool) {
    let spawned = std::thread::Builder::new()
        .name("stipple-instance-dbus".into())
        .spawn(move || {
            let mut first = started;
            loop {
                let m = match bus.read_message() {
                    Ok(m) => m,
                    Err(e) => {
                        debug(format_args!("the bus went away: {e}"));
                        return;
                    }
                };
                if m.mtype != 1 {
                    continue;
                }
                match answer(&m, &path) {
                    Answer::Launch(mut launch) => {
                        launch.started_app = std::mem::take(&mut first);
                        debug(format_args!("{} from {}: {launch:?}", m.member, m.sender));
                        super::deliver(launch);
                        let _ = bus.send_return(&m, "", &[]);
                    }
                    Answer::Return(sig, body) => {
                        let _ = bus.send_return(&m, sig, &body);
                    }
                    Answer::Error(name) => {
                        let _ = bus.send_error(&m, name);
                    }
                }
            }
        });
    if let Err(e) = spawned {
        debug(format_args!("{e}"));
    }
}

/// What to do with a method call.
#[derive(Debug, PartialEq)]
enum Answer {
    Launch(Launch),
    Return(&'static str, Vec<u8>),
    Error(&'static str),
}

fn answer(m: &Message, path: &str) -> Answer {
    match (m.interface.as_str(), m.member.as_str()) {
        ("org.freedesktop.DBus.Peer", "Ping") => Answer::Return("", Vec::new()),
        ("org.freedesktop.DBus.Introspectable", "Introspect") => {
            let mut w = W::default();
            w.str(&introspection(path, &m.path));
            Answer::Return("s", w.buf)
        }
        _ if m.path != path => Answer::Error("org.freedesktop.DBus.Error.UnknownObject"),
        (IFACE, member) => match parse_call(member, &m.body) {
            Some(launch) => Answer::Launch(launch),
            None => Answer::Error("org.freedesktop.DBus.Error.InvalidArgs"),
        },
        _ => Answer::Error("org.freedesktop.DBus.Error.UnknownMethod"),
    }
}

fn introspection(path: &str, asked: &str) -> String {
    let doctype = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \
                   \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n";
    if asked == path {
        return format!(
            "{doctype}<node>\n <interface name=\"{IFACE}\">\n  \
             <method name=\"Activate\"><arg type=\"a{{sv}}\" name=\"platform_data\" direction=\"in\"/></method>\n  \
             <method name=\"Open\"><arg type=\"as\" name=\"uris\" direction=\"in\"/>\
             <arg type=\"a{{sv}}\" name=\"platform_data\" direction=\"in\"/></method>\n  \
             <method name=\"ActivateAction\"><arg type=\"s\" name=\"action_name\" direction=\"in\"/>\
             <arg type=\"av\" name=\"parameter\" direction=\"in\"/>\
             <arg type=\"a{{sv}}\" name=\"platform_data\" direction=\"in\"/></method>\n \
             </interface>\n</node>\n"
        );
    }
    // A parent of the object: its next path element.
    let prefix = if asked == "/" {
        "/".to_string()
    } else {
        format!("{asked}/")
    };
    let child = path
        .strip_prefix(&prefix)
        .and_then(|rest| rest.split('/').next())
        .map(|c| format!(" <node name=\"{c}\"/>\n"))
        .unwrap_or_default();
    format!("{doctype}<node>\n{child}</node>\n")
}

// ---- platform_data -----------------------------------------------------------

/// The `platform_data` entries a launch reads.
#[derive(Debug, Default, PartialEq)]
struct PlatformData {
    activation_token: Option<String>,
    startup_id: Option<String>,
    cwd: Option<PathBuf>,
    args: Option<Vec<String>>,
}

/// Read an `as` at `off`.
fn read_strings(buf: &[u8], off: &mut usize) -> Option<Vec<String>> {
    align_to(off, 4);
    let len = read_u32(buf, off).ok()? as usize;
    let end = off.checked_add(len).filter(|e| *e <= buf.len())?;
    let mut out = Vec::new();
    while *off < end {
        out.push(read_string(buf, off).ok()?);
    }
    Some(out)
}

/// Read an `ay` at `off`.
fn read_bytes(buf: &[u8], off: &mut usize) -> Option<Vec<u8>> {
    align_to(off, 4);
    let len = read_u32(buf, off).ok()? as usize;
    let bytes = buf.get(*off..off.checked_add(len)?)?.to_vec();
    *off += len;
    Some(bytes)
}

/// Read a variant's signature at `off`.
fn read_signature(buf: &[u8], off: &mut usize) -> Option<String> {
    let len = *buf.get(*off)? as usize;
    let sig = buf.get(*off + 1..*off + 1 + len)?;
    let sig = std::str::from_utf8(sig).ok()?.to_string();
    *off += len + 2;
    Some(sig)
}

/// Read an `a{sv}` at `off`, keeping the entries a launch uses.
fn read_platform_data(buf: &[u8], off: &mut usize) -> Option<PlatformData> {
    align_to(off, 4);
    let len = read_u32(buf, off).ok()? as usize;
    align_to(off, 8);
    let mut pd = PlatformData::default();
    if len == 0 {
        // Empty: the padding may be missing at the body's end.
        *off = (*off).min(buf.len());
        return Some(pd);
    }
    let end = off.checked_add(len).filter(|e| *e <= buf.len())?;
    while *off < end {
        align_to(off, 8);
        let key = read_string(buf, off).ok()?;
        let sig = read_signature(buf, off)?;
        match (key.as_str(), sig.as_str()) {
            ("activation-token", "s") => pd.activation_token = Some(read_string(buf, off).ok()?),
            ("desktop-startup-id", "s") => pd.startup_id = Some(read_string(buf, off).ok()?),
            ("cwd", "ay") => {
                let mut bytes = read_bytes(buf, off)?;
                if bytes.last() == Some(&0) {
                    bytes.pop();
                }
                use std::os::unix::ffi::OsStringExt;
                pd.cwd = Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)));
            }
            ("cwd", "s") => pd.cwd = Some(PathBuf::from(read_string(buf, off).ok()?)),
            (ARGS_KEY, "as") => pd.args = Some(read_strings(buf, off)?),
            _ => skip_value(buf, off, sig.as_bytes(), &mut 0).ok()?,
        }
    }
    *off = end;
    Some(pd)
}

/// A call's launch: its arguments and action, and what `platform_data` says.
fn parse_call(member: &str, body: &[u8]) -> Option<Launch> {
    let mut off = 0;
    let mut launch = Launch::default();
    let mut uris = Vec::new();
    match member {
        "Activate" => {}
        "Open" => uris = read_strings(body, &mut off)?,
        "ActivateAction" => {
            launch.action = Some(read_string(body, &mut off).ok()?);
            // `av`: a string parameter is the action's target.
            align_to(&mut off, 4);
            let len = read_u32(body, &mut off).ok()? as usize;
            let end = off.checked_add(len).filter(|e| *e <= body.len())?;
            while off < end {
                let sig = read_signature(body, &mut off)?;
                if sig == "s" && launch.action_target.is_none() {
                    launch.action_target = Some(read_string(body, &mut off).ok()?);
                } else {
                    skip_value(body, &mut off, sig.as_bytes(), &mut 0).ok()?;
                }
            }
            off = end;
        }
        _ => return None,
    }
    let pd = read_platform_data(body, &mut off)?;
    launch.args = pd
        .args
        .unwrap_or_else(|| uris.iter().map(|u| uri_arg(u)).collect());
    launch.cwd = pd.cwd;
    for (name, value) in [
        ("XDG_ACTIVATION_TOKEN", &pd.activation_token),
        ("DESKTOP_STARTUP_ID", &pd.startup_id),
    ] {
        if let Some(value) = value {
            launch.env.push((name.to_string(), value.clone()));
        }
    }
    launch.activation_token = pd
        .activation_token
        .filter(|t| !t.is_empty())
        .or(pd.startup_id.filter(|t| !t.is_empty()));
    Some(launch)
}

/// An `Open` URI as a command-line argument: a local `file:` URI as its
/// path, any other as it is (for [`on_open_url`](crate::App::on_open_url)).
fn uri_arg(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("file://") else {
        return uri.to_string();
    };
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return uri.to_string();
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| hex(*b)),
                bytes.get(i + 2).and_then(|b| hex(*b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A command-line argument as an `Open` URI: a URI as it is, a path as a
/// `file:` URI (made absolute against `cwd`).
fn arg_uri(arg: &str, cwd: Option<&std::path::Path>) -> String {
    let scheme = arg.split_once(':').is_some_and(|(s, _)| {
        s.len() > 1
            && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if scheme {
        return arg.to_string();
    }
    let path = match cwd {
        Some(cwd) if !arg.starts_with('/') => cwd.join(arg),
        _ => PathBuf::from(arg),
    };
    let mut uri = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The call that hands `launch` over: `Activate` without arguments, `Open`
/// when they're all files or URIs, else `Activate` (its arguments in
/// `stipple-args` either way). Returns the member, its signature and body.
fn call_for(launch: &Launch) -> (&'static str, &'static str, Vec<u8>) {
    let mut w = W::default();
    let open = !launch.args.is_empty() && !launch.args.iter().any(|a| a.starts_with('-'));
    if open {
        w.array(4, |w| {
            for arg in &launch.args {
                w.str(&arg_uri(arg, launch.cwd.as_deref()));
            }
        });
    }
    write_platform_data(&mut w, launch);
    if open {
        ("Open", "asa{sv}", w.buf)
    } else {
        ("Activate", "a{sv}", w.buf)
    }
}

/// `launch`'s `platform_data`: its tokens, directory and arguments.
fn write_platform_data(w: &mut W, launch: &Launch) {
    let token = launch.env_var("XDG_ACTIVATION_TOKEN").map(str::to_string);
    let startup = launch.env_var("DESKTOP_STARTUP_ID").map(str::to_string);
    // A token that came another way: Wayland's if it isn't X11's.
    let token = token.or_else(|| {
        launch
            .activation_token
            .clone()
            .filter(|t| startup.as_ref() != Some(t))
    });
    w.array(8, |w| {
        for (key, value) in [("activation-token", token), ("desktop-startup-id", startup)] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                w.strukt(|w| {
                    w.str(key);
                    w.variant("s", |w| w.str(&value));
                });
            }
        }
        if let Some(cwd) = &launch.cwd {
            use std::os::unix::ffi::OsStrExt;
            w.strukt(|w| {
                w.str("cwd");
                w.variant("ay", |w| {
                    w.array(1, |w| {
                        w.buf.extend_from_slice(cwd.as_os_str().as_bytes());
                        w.buf.push(0);
                    })
                });
            });
        }
        if !launch.args.is_empty() {
            w.strukt(|w| {
                w.str(ARGS_KEY);
                w.variant("as", |w| {
                    w.array(4, |w| {
                        for arg in &launch.args {
                            w.str(arg);
                        }
                    })
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// How long a test waits for the bus.
    const TEST_WAIT: Duration = Duration::from_secs(10);

    fn launch() -> Launch {
        Launch {
            args: vec!["notes.txt".into(), "https://example.com/a b".into()],
            cwd: Some("/home/u/my docs".into()),
            env: vec![
                ("XDG_ACTIVATION_TOKEN".into(), "wl-1".into()),
                ("DESKTOP_STARTUP_ID".into(), "x11_TIME7".into()),
            ],
            activation_token: Some("wl-1".into()),
            ..Launch::default()
        }
    }

    #[test]
    fn object_paths_follow_gapplication() {
        assert_eq!(object_path("org.example.App"), "/org/example/App");
        assert_eq!(object_path("org.example.My-App"), "/org/example/My_App");
    }

    #[test]
    fn files_and_uris_become_an_open_call_and_back() {
        let (member, sig, body) = call_for(&launch());
        assert_eq!((member, sig), ("Open", "asa{sv}"));
        let mut off = 0;
        let uris = read_strings(&body, &mut off).unwrap();
        assert_eq!(
            uris,
            [
                "file:///home/u/my%20docs/notes.txt",
                "https://example.com/a b"
            ]
        );
        let pd = read_platform_data(&body, &mut off).unwrap();
        assert_eq!(off, body.len());
        assert_eq!(pd.activation_token.as_deref(), Some("wl-1"));
        assert_eq!(pd.startup_id.as_deref(), Some("x11_TIME7"));
        assert_eq!(
            pd.cwd.as_deref(),
            Some(std::path::Path::new("/home/u/my docs"))
        );
        // The receiver takes the exact arguments, the tokens and directory.
        let back = parse_call(member, &body).unwrap();
        assert_eq!(back.args, launch().args);
        assert_eq!(back.cwd, launch().cwd);
        assert_eq!(back.activation_token.as_deref(), Some("wl-1"));
        assert_eq!(back.env_var("DESKTOP_STARTUP_ID"), Some("x11_TIME7"));
        assert_eq!(back.action, None);
    }

    #[test]
    fn options_or_nothing_become_activate() {
        let none = Launch::default();
        let (member, sig, body) = call_for(&none);
        assert_eq!((member, sig), ("Activate", "a{sv}"));
        assert_eq!(body, [0; 8], "an empty a{{sv}}, padded to its entries");
        assert_eq!(parse_call(member, &body).unwrap(), Launch::default());
        let opts = Launch {
            args: vec!["--new-window".into()],
            ..Launch::default()
        };
        let (member, _, body) = call_for(&opts);
        assert_eq!(member, "Activate");
        assert_eq!(parse_call(member, &body).unwrap().args, ["--new-window"]);
    }

    /// `platform_data` as GLib (`g_app_launch_context`) would send it.
    fn glib_platform_data(w: &mut W) {
        w.array(8, |w| {
            w.strukt(|w| {
                w.str("desktop-startup-id");
                w.variant("s", |w| w.str("gnome_TIME9"));
            });
            w.strukt(|w| {
                w.str("pid");
                w.variant("x", |w| {
                    w.pad(8);
                    w.buf.extend_from_slice(&42i64.to_le_bytes());
                });
            });
            w.strukt(|w| {
                w.str("activation-token");
                w.variant("s", |w| w.str("tok"));
            });
        });
    }

    #[test]
    fn an_open_from_the_desktop_gives_paths_and_the_token() {
        let mut w = W::default();
        w.array(4, |w| {
            w.str("file:///tmp/a%20b.txt");
            w.str("myapp://auth?x=1");
        });
        glib_platform_data(&mut w);
        let l = parse_call("Open", &w.buf).unwrap();
        assert_eq!(l.args, ["/tmp/a b.txt", "myapp://auth?x=1"]);
        assert_eq!(l.activation_token.as_deref(), Some("tok"));
        assert_eq!(l.env_var("XDG_ACTIVATION_TOKEN"), Some("tok"));
        assert_eq!(l.env_var("DESKTOP_STARTUP_ID"), Some("gnome_TIME9"));
        assert_eq!(l.cwd, None);
    }

    #[test]
    fn an_action_carries_its_name_and_string_target() {
        let mut w = W::default();
        w.str("open-tab");
        w.array(1, |w| w.variant("s", |w| w.str("ssh://host")));
        glib_platform_data(&mut w);
        let l = parse_call("ActivateAction", &w.buf).unwrap();
        assert_eq!(l.action.as_deref(), Some("open-tab"));
        assert_eq!(l.action_target.as_deref(), Some("ssh://host"));
        assert!(l.args.is_empty());
        assert_eq!(l.activation_token.as_deref(), Some("tok"));
        // No parameter at all.
        let mut w = W::default();
        w.str("new-window");
        w.array(1, |_| {});
        w.array(8, |_| {});
        let l = parse_call("ActivateAction", &w.buf).unwrap();
        assert_eq!(
            (l.action.as_deref(), l.action_target),
            (Some("new-window"), None)
        );
    }

    #[test]
    fn calls_are_answered_by_path_and_interface() {
        let path = object_path("org.example.App");
        let call = |path: &str, iface: &str, member: &str, body: Vec<u8>| Message {
            mtype: 1,
            path: path.into(),
            interface: iface.into(),
            member: member.into(),
            body,
            ..Message::default()
        };
        let activate = call(&path, IFACE, "Activate", vec![0, 0, 0, 0]);
        assert_eq!(answer(&activate, &path), Answer::Launch(Launch::default()));
        let truncated = call(&path, IFACE, "Open", vec![9, 0, 0, 0]);
        assert_eq!(
            answer(&truncated, &path),
            Answer::Error("org.freedesktop.DBus.Error.InvalidArgs")
        );
        let elsewhere = call("/nope", IFACE, "Activate", vec![0, 0, 0, 0]);
        assert_eq!(
            answer(&elsewhere, &path),
            Answer::Error("org.freedesktop.DBus.Error.UnknownObject")
        );
        let other = call(&path, "org.example.X", "Y", vec![]);
        assert_eq!(
            answer(&other, &path),
            Answer::Error("org.freedesktop.DBus.Error.UnknownMethod")
        );
        let Answer::Return("s", xml) = answer(
            &call(
                "/org",
                "org.freedesktop.DBus.Introspectable",
                "Introspect",
                vec![],
            ),
            &path,
        ) else {
            panic!("introspection");
        };
        assert!(String::from_utf8_lossy(&xml).contains("<node name=\"example\"/>"));
        let Answer::Return("s", xml) = answer(
            &call(
                &path,
                "org.freedesktop.DBus.Introspectable",
                "Introspect",
                vec![],
            ),
            &path,
        ) else {
            panic!("introspection");
        };
        assert!(String::from_utf8_lossy(&xml).contains("ActivateAction"));
    }

    /// A private `dbus-daemon`, where one is installed: the first claim owns
    /// the name and the second hands its launch over.
    #[test]
    fn a_second_launch_reaches_the_owner_over_a_private_bus() {
        let dir = std::env::temp_dir().join(format!("stipple-dbus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("bus");
        let config = dir.join("session.conf");
        std::fs::write(
            &config,
            format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \
                 \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
                 <busconfig><type>session</type><listen>unix:path={}</listen>\
                 <auth>EXTERNAL</auth><policy context=\"default\">\
                 <allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/>\
                 <allow own=\"*\"/></policy></busconfig>\n",
                socket.display()
            ),
        )
        .unwrap();
        let Ok(mut daemon) = std::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .spawn()
        else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let deadline = std::time::Instant::now() + TEST_WAIT;
        while !socket.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let address = format!("unix:path={}", socket.display());
        let id = "org.stipple.DBusInstanceTest";

        let mut owner = DBus::connect_address(&address).unwrap();
        assert_eq!(
            owner.request_name_flags(id, DO_NOT_QUEUE).unwrap(),
            PRIMARY_OWNER
        );
        let mut second = DBus::connect_address(&address).unwrap();
        second.set_read_timeout(Some(TEST_WAIT));
        assert_ne!(
            second.request_name_flags(id, DO_NOT_QUEUE).unwrap(),
            PRIMARY_OWNER
        );
        let path = object_path(id);
        let sender = std::thread::spawn(move || {
            let (member, signature, body) = call_for(&launch());
            second.call_with_body(id, &path, IFACE, member, signature, &body)
        });
        owner.set_read_timeout(Some(TEST_WAIT));
        let path = object_path(id);
        let heard = loop {
            let m = owner.read_message().unwrap();
            if m.mtype == 1 && m.interface == IFACE {
                let Answer::Launch(l) = answer(&m, &path) else {
                    panic!("not a launch: {m:?}");
                };
                owner.send_return(&m, "", &[]).unwrap();
                break l;
            }
        };
        assert!(sender.join().unwrap().is_ok(), "the call was answered");
        assert_eq!(heard.args, launch().args);
        assert_eq!(heard.activation_token.as_deref(), Some("wl-1"));
        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
