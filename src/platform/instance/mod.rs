//! One running instance per app: a second launch hands its command line,
//! working directory, environment and activation token to the process
//! already running, and exits. See
//! [`App::single_instance`](crate::App::single_instance) and
//! `docs/os-integration.md`.
//!
//! | | Who owns the app's id | How a launch travels |
//! |---|---|---|
//! | Linux | an `flock` on `$XDG_RUNTIME_DIR/<id>.instance.lock` | a Unix socket beside it (`<id>.instance`) |
//! | macOS | the same, in the per-user `$TMPDIR` | the same; a bundled app's `open` also reaches the running one as a reopen (Launch Services) |
//! | Windows | a named mutex, `Local\stipple-instance-<id>` | a named pipe of the session |
//! | iOS, Android, Web | — (the system keeps one instance) | — |
//!
//! Both ends speak the same short message (see [`encode`]): the launch's
//! fields, each a tagged, length-prefixed record, then an end record; the
//! running instance answers `ok` once it has queued the launch, so the second
//! process exits knowing it was heard.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(unsafe_code)]
mod unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use unix as os;

/// The session bus's `org.freedesktop.Application`, for an app that is
/// [D-Bus activatable](crate::App::dbus_activatable).
#[cfg(target_os = "linux")]
mod dbus;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;
#[cfg(target_os = "windows")]
use windows as os;

/// How a process of the app was started: what
/// [`App::single_instance`](crate::App::single_instance)'s hook hears when a
/// second launch hands itself to the running instance. [`Launch::current`]
/// is this process's own.
///
/// ```
/// use stipple::platform::Launch;
/// let me = Launch::current();
/// assert_eq!(me.args, std::env::args().skip(1).collect::<Vec<_>>());
/// assert_eq!(me.env_var("PATH"), std::env::var("PATH").ok().as_deref());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Launch {
    /// The command-line arguments, past the program's name.
    pub args: Vec<String>,
    /// The directory it was started in, against which relative paths in
    /// [`args`](Self::args) resolve.
    pub cwd: Option<PathBuf>,
    /// Its environment, name and value.
    pub env: Vec<(String, String)>,
    /// The token a launcher hands the app so that its window may take the
    /// focus (Wayland's `XDG_ACTIVATION_TOKEN`, X11's `DESKTOP_STARTUP_ID`):
    /// pass it on with
    /// [`WindowRequest::Activate`](crate::platform::WindowRequest::Activate)
    /// to the window that shows the launch.
    pub activation_token: Option<String>,
    /// The desktop action asked for, when the launch came as D-Bus
    /// activation's `ActivateAction` (a `.desktop` file's `[Desktop Action
    /// new-window]`, picked from the dock or the app grid): its name. See
    /// [`App::dbus_activatable`](crate::App::dbus_activatable).
    pub action: Option<String>,
    /// `ActivateAction`'s parameter, when it is a string.
    pub action_target: Option<String>,
    /// Whether this is the launch the session bus started the running
    /// process for (D-Bus activation): the app's first window is already
    /// up, so show the launch there (raise it with the token, open the
    /// files in it) rather than in a new one.
    pub started_app: bool,
}

impl Launch {
    /// This process's launch: its arguments, working directory and
    /// environment, and the activation token in the environment.
    pub fn current() -> Self {
        let lossy = |s: std::ffi::OsString| s.to_string_lossy().into_owned();
        #[allow(unused_mut)]
        let mut env: Vec<(String, String)> = std::env::vars_os()
            .map(|(k, v)| (lossy(k), lossy(v)))
            .collect();
        // The backend takes the launcher's startup id and activation token
        // out of the environment as it starts (so the app's children don't
        // inherit them); the launch had them.
        #[cfg(target_os = "linux")]
        if let Some(startup) = crate::platform::backend::startup::taken() {
            for (name, value) in [
                ("XDG_ACTIVATION_TOKEN", startup.activation_token),
                ("DESKTOP_STARTUP_ID", startup.desktop_startup_id),
            ] {
                if let Some(value) = value
                    && !env.iter().any(|(k, _)| k == name)
                {
                    env.push((name.to_string(), value));
                }
            }
        }
        let token = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
            .iter()
            .find_map(|name| {
                env.iter()
                    .find(|(k, v)| k == name && !v.is_empty())
                    .map(|(_, v)| v.clone())
            });
        #[allow(unused_mut)]
        let mut args: Vec<String> = std::env::args_os().skip(1).map(lossy).collect();
        // The switch a D-Bus `.service` file starts the app with.
        #[cfg(target_os = "linux")]
        args.retain(|a| a != dbus::SERVICE_SWITCH);
        Self {
            args,
            cwd: std::env::current_dir().ok(),
            env,
            activation_token: token,
            ..Self::default()
        }
    }

    /// The value of the environment variable `name` the launch had.
    pub fn env_var(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Hand this process's [`Launch`] to the app's running instance, if one
/// owns `id` (as [`App::single_instance`](crate::App::single_instance) makes
/// it), and return whether it took it: the caller then exits. For an app
/// that must know before it builds its state (one that would otherwise
/// reconnect to sessions the running instance already has); `single_instance`
/// asks again as the app starts, and listens if nobody answered. Always
/// `false` on iOS, Android and the web.
///
/// ```no_run
/// if stipple::platform::forward_launch("org.example.Term") {
///     return; // The running terminal opens the window.
/// }
/// ```
pub fn forward_launch(id: &str) -> bool {
    forward(id, &Launch::current())
}

/// What became of claiming the app's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
pub(crate) enum Claim {
    /// This process owns it, and listens for later launches.
    Primary,
    /// Another process owns it and took this launch: exit.
    Forwarded,
    /// Neither: no transport here, or the owner doesn't answer. Run alone.
    Unavailable,
}

/// How long a second launch keeps trying to reach the owner, which may be
/// between taking the id and listening.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
const PATIENCE: Duration = Duration::from_secs(3);

/// Own `id`, listening for launches (each [`deliver`]ed), or hand this
/// process's launch to the process that owns it. With `dbus` (Linux), on
/// the session bus first, as `org.freedesktop.Application`; the Unix socket
/// still listens beside it, and takes over without a bus.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn claim(id: &str, dbus: bool) -> Claim {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let launch = Launch::current();
        #[cfg(target_os = "linux")]
        if dbus {
            match dbus::claim(id, &launch) {
                Claim::Primary => {
                    // `forward_launch` (and an app without a bus) reach it
                    // through the socket.
                    if let Ok(Some(owner)) = os::own(id) {
                        os::listen(owner, Box::new(deliver));
                    }
                    return Claim::Primary;
                }
                Claim::Forwarded => return Claim::Forwarded,
                Claim::Unavailable => {}
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = dbus;
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            match os::own(id) {
                Ok(Some(owner)) => {
                    os::listen(owner, Box::new(deliver));
                    return Claim::Primary;
                }
                Ok(None) => {}
                Err(e) => {
                    crate::platform::window::debug_window(format_args!(
                        "single instance {id:?}: {e}"
                    ));
                    return Claim::Unavailable;
                }
            }
            if forward(id, &launch) {
                return Claim::Forwarded;
            }
            if std::time::Instant::now() > deadline {
                return Claim::Unavailable;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (id, dbus);
        Claim::Unavailable
    }
}

/// Send `launch` to the owner of `id` and wait (a few seconds at most) for
/// its `ok`.
fn forward(id: &str, launch: &Launch) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let Ok(stream) = os::connect(id) else {
            return false;
        };
        exchange(stream, launch)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (id, launch);
        false
    }
}

/// Write `launch` to `stream` and read the answer, on a thread, so that an
/// owner that hangs doesn't hang the second launch too.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
fn exchange(mut stream: impl Read + Write + Send + 'static, launch: &Launch) -> bool {
    let message = encode(launch);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let sent = (|| {
            stream.write_all(&message)?;
            stream.flush()?;
            let mut ack = [0u8; 2];
            stream.read_exact(&mut ack)?;
            Ok::<bool, io::Error>(&ack == b"ok")
        })();
        let _ = tx.send(sent.unwrap_or(false));
    });
    rx.recv_timeout(PATIENCE).unwrap_or(false)
}

/// Serve one connection: read a launch, hand it to `sink`, answer `ok`.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
fn serve(mut stream: impl Read + Write, sink: &(dyn Fn(Launch) + Send + Sync)) {
    match decode(&mut stream) {
        Ok(launch) => {
            sink(launch);
            let _ = stream.write_all(b"ok");
            let _ = stream.flush();
        }
        Err(e) => {
            crate::platform::window::debug_window(format_args!("single instance: {e}"));
        }
    }
}

/// A launch the running instance heard: its URLs to
/// [`App::on_open_url`](crate::App::on_open_url) (a protocol handler starts
/// the app again with one, an OAuth redirect say), then the launch itself.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
fn deliver(launch: Launch) {
    let urls = crate::platform::incoming_url::launch_urls(launch.args.clone());
    crate::platform::shell::post(crate::platform::Event::Launch(launch));
    for url in urls {
        crate::platform::incoming_url::deliver(url);
    }
}

/// The message's first bytes.
const MAGIC: &[u8] = b"stipple-launch 1\n";
// Record tags.
const ARG: u8 = b'a';
const CWD: u8 = b'c';
const ENV: u8 = b'e';
const TOKEN: u8 = b't';
const END: u8 = b'.';
/// The most a record, and a whole message, may hold.
const MAX_RECORD: usize = 1 << 20;
const MAX_MESSAGE: usize = 8 << 20;

/// `launch` as the message a second launch sends: [`MAGIC`], then for each
/// field a tag byte, a little-endian `u32` length and the UTF-8 bytes (an
/// environment variable as `NAME=value`), then the end tag.
pub(crate) fn encode(launch: &Launch) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    let mut record = |tag: u8, bytes: &[u8]| {
        let bytes = &bytes[..bytes.len().min(MAX_RECORD)];
        out.push(tag);
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    };
    for arg in &launch.args {
        record(ARG, arg.as_bytes());
    }
    if let Some(cwd) = &launch.cwd {
        record(CWD, cwd.to_string_lossy().as_bytes());
    }
    for (name, value) in &launch.env {
        record(ENV, format!("{name}={value}").as_bytes());
    }
    if let Some(token) = &launch.activation_token {
        record(TOKEN, token.as_bytes());
    }
    out.push(END);
    out
}

/// Read one [`encode`]d launch from `r`, up to its end tag.
pub(crate) fn decode(r: &mut impl Read) -> io::Result<Launch> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let mut magic = [0u8; MAGIC.len()];
    r.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(bad("not a launch"));
    }
    let mut launch = Launch::default();
    let mut total = 0usize;
    loop {
        let mut tag = [0u8; 1];
        r.read_exact(&mut tag)?;
        if tag[0] == END {
            return Ok(launch);
        }
        let mut len = [0u8; 4];
        r.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        total += len;
        if len > MAX_RECORD || total > MAX_MESSAGE {
            return Err(bad("launch too long"));
        }
        let mut bytes = vec![0u8; len];
        r.read_exact(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        match tag[0] {
            ARG => launch.args.push(text),
            CWD => launch.cwd = Some(PathBuf::from(text)),
            ENV => {
                let (name, value) = text.split_once('=').unwrap_or((&text, ""));
                launch.env.push((name.to_string(), value.to_string()));
            }
            TOKEN => launch.activation_token = Some(text),
            _ => {} // A later version's field.
        }
    }
}

/// `id` as a file or pipe name: letters, digits, `.`, `-` and `_` kept, the
/// rest `_`; a long id shortened to its start and a hash of the whole (a
/// Unix socket's path is short: 104 bytes on macOS).
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows", test)),
    allow(dead_code)
)]
pub(crate) fn file_name(id: &str) -> String {
    let clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = if clean.is_empty() || clean.starts_with('.') {
        format!("app{clean}")
    } else {
        clean
    };
    if clean.len() <= 40 {
        return clean;
    }
    // FNV-1a, 64-bit.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{}-{hash:016x}", &clean[..23])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Launch {
        Launch {
            args: vec!["--new-tab".into(), "ssh://host".into(), "é \n".into()],
            cwd: Some(PathBuf::from("/home/u/src")),
            env: vec![
                ("TERM".into(), "xterm=256".into()),
                ("EMPTY".into(), "".into()),
            ],
            activation_token: Some("tok-1".into()),
            ..Launch::default()
        }
    }

    #[test]
    fn a_launch_survives_the_wire() {
        let bytes = encode(&sample());
        assert!(bytes.starts_with(MAGIC));
        assert_eq!(*bytes.last().unwrap(), END);
        let back = decode(&mut bytes.as_slice()).unwrap();
        assert_eq!(back, sample());
        assert_eq!(back.env_var("TERM"), Some("xterm=256"));
        assert_eq!(back.env_var("NOPE"), None);
        // Bytes after the end record are left for whoever reads next.
        let mut more = bytes.clone();
        more.extend_from_slice(b"ok");
        let mut r = more.as_slice();
        decode(&mut r).unwrap();
        assert_eq!(r, b"ok");
    }

    #[test]
    fn bad_messages_are_refused() {
        assert!(decode(&mut &b"hello"[..]).is_err());
        let mut truncated = encode(&sample());
        truncated.pop();
        assert!(decode(&mut truncated.as_slice()).is_err());
        let mut huge = MAGIC.to_vec();
        huge.push(ARG);
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&mut huge.as_slice()).is_err());
        // An unknown record is skipped.
        let mut later = MAGIC.to_vec();
        later.extend_from_slice(&[b'z', 1, 0, 0, 0, b'x', ARG, 1, 0, 0, 0, b'y', END]);
        assert_eq!(decode(&mut later.as_slice()).unwrap().args, ["y"]);
    }

    #[test]
    fn ids_become_short_safe_names() {
        assert_eq!(file_name("org.example.Term"), "org.example.Term");
        assert_eq!(file_name("a/b c"), "a_b_c");
        assert_eq!(file_name(""), "app");
        assert_eq!(file_name("../x"), "app.._x");
        let long = file_name(&"x".repeat(200));
        assert_eq!(long.len(), 40);
        assert_ne!(long, file_name(&"x".repeat(201)));
    }

    #[test]
    fn the_current_launch_reads_the_process() {
        let me = Launch::current();
        assert_eq!(me.cwd, std::env::current_dir().ok());
        assert!(!me.env.is_empty());
    }
}
