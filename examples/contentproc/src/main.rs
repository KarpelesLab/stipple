//! Stipple-as-compositor **content process** path, CPU shm dual (Linux).
//!
//! The browser-compositor architecture end to end, with a CPU buffer instead of
//! a GPU texture (so it runs with no GPU and verifies headlessly): a UI process
//! spawns a separate **content process**; the content process renders pixels
//! into a `memfd` [`SharedBuffer`] and hands the UI process its fd over a Unix
//! socket (`SCM_RIGHTS`); the UI process maps the *same* memory and composites it
//! into a [`viewport`](stipple::widgets::viewport). Input the UI routes to the
//! viewport is forwarded over the socket, and the content process redraws into
//! the shared buffer — exactly the GPU `dma-buf` flow, minus the GPU.
//!
//! This is the dual of the `dmabuftest`/`dri3probe` GPU path: same separation and
//! same fd-over-socket transport, but CI-verifiable on any Linux box. The demo
//! self-checks the whole loop and prints `RESULT: PASS`; no window/display
//! needed (it composites with `App::render_once`).
//!
//! Run: `cargo run -p contentproc`

use std::process::ExitCode;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("child") {
        return imp::child();
    }
    match imp::parent() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("contentproc UI error: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    // memfd + fd inheritance are Linux-only; keep the workspace building elsewhere.
    println!("contentproc: Linux only");
    ExitCode::SUCCESS
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, ExitCode};
    use stipple::platform::shm::SharedBuffer;
    use stipple::platform::{sandbox, scm};
    use stipple::prelude::*;

    const W: u32 = 200;
    const H: u32 = 120;
    /// The inherited socket fd number in the spawned content process.
    const CHILD_SOCK_FD: RawFd = 3;
    const PAGE: ViewportId = ViewportId(1);

    const GREEN: [u8; 4] = [0x34, 0xd3, 0x99, 0xff];
    const ORANGE: [u8; 4] = [0xf5, 0x9e, 0x0b, 0xff];
    const MARKER: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

    unsafe extern "C" {
        fn dup2(oldfd: i32, newfd: i32) -> i32;
        fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
    }

    fn sample(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        buf[i..i + 4].try_into().unwrap()
    }

    /// The content process's base frame: a diagonal green/orange split — an
    /// obviously "rendered page", distinct from the viewport's dark placeholder.
    fn render_base(buf: &mut [u8], w: u32, h: u32) {
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let c = if x + y < (w + h) / 2 { GREEN } else { ORANGE };
                buf[i..i + 4].copy_from_slice(&c);
            }
        }
    }

    /// Draw a small white marker centered at `(cx, cy)` (a forwarded click).
    fn draw_marker(buf: &mut [u8], w: u32, h: u32, cx: i32, cy: i32) {
        for dy in -6..6 {
            for dx in -6..6 {
                let (px, py) = (cx + dx, cy + dy);
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let i = ((py as u32 * w + px as u32) * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&MARKER);
                }
            }
        }
    }

    /// The content process: render into shared memory, hand the UI process the
    /// buffer fd, then apply forwarded input by redrawing into the same memory.
    pub fn child() -> ExitCode {
        // The UI process dup2'd our socket onto fd 3 before exec.
        let sock = unsafe { UnixStream::from_raw_fd(CHILD_SOCK_FD) };

        // 1. Read the requested frame size.
        let mut dims = [0u8; 8];
        if (&sock).read_exact(&mut dims).is_err() {
            return ExitCode::from(1);
        }
        let w = u32::from_le_bytes(dims[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(dims[4..8].try_into().unwrap());

        // 2. Render the page into a shared (memfd) buffer.
        let mut buf = match SharedBuffer::create((w * h * 4) as usize) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("content: SharedBuffer::create failed: {e}");
                return ExitCode::from(1);
            }
        };
        render_base(buf.as_mut_slice(), w, h);

        // 3. Hand the buffer fd to the UI process (SCM_RIGHTS).
        if scm::send_with_fds(&sock, &dims, &[buf.fd()]).is_err() {
            return ExitCode::from(1);
        }
        eprintln!("content: sent {w}x{h} buffer to the UI process");

        // 3b. Now that we have our socket + buffer, drop privilege: a seccomp
        //     sandbox so a compromised content process can't open the network or
        //     exec. The existing IPC fd keeps working (read/write aren't blocked);
        //     creating a NEW socket must now fail — self-check it.
        match sandbox::restrict() {
            Ok(()) => {
                let s = unsafe {
                    socket(2 /*AF_INET*/, 1 /*SOCK_STREAM*/, 0)
                };
                if s < 0 {
                    eprintln!("content: sandbox active (new sockets blocked)");
                } else {
                    eprintln!("content: SANDBOX FAILED (socket() succeeded fd={s})");
                    return ExitCode::from(1);
                }
            }
            Err(e) => eprintln!("content: sandbox unavailable ({e})"),
        }

        // 4. Apply a forwarded pointer press: redraw a marker into the SAME shared
        //    memory the UI process is mapped to, then acknowledge.
        let mut ev = [0u8; 8];
        if (&sock).read_exact(&mut ev).is_ok() {
            let x = i32::from_le_bytes(ev[0..4].try_into().unwrap());
            let y = i32::from_le_bytes(ev[4..8].try_into().unwrap());
            draw_marker(buf.as_mut_slice(), w, h, x, y);
            eprintln!("content: drew marker at forwarded ({x},{y})");
            let _ = (&sock).write_all(b"OK");
        }
        ExitCode::SUCCESS
    }

    /// The UI process: spawn the content process, receive + map its buffer,
    /// composite it into a viewport, and forward input back to it.
    pub fn parent() -> std::io::Result<ExitCode> {
        let (ui, child_sock) = UnixStream::pair()?;
        let child_fd = child_sock.as_raw_fd();

        // Spawn ourselves in `child` mode with the content-side socket on fd 3.
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg("child");
        unsafe {
            cmd.pre_exec(move || {
                // dup2 clears CLOEXEC on the new fd, so fd 3 survives exec.
                if dup2(child_fd, CHILD_SOCK_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        drop(child_sock); // the content process holds its copy via fd 3

        // 1. Ask the content process for a W×H frame.
        let mut dims = [0u8; 8];
        dims[0..4].copy_from_slice(&W.to_le_bytes());
        dims[4..8].copy_from_slice(&H.to_le_bytes());
        (&ui).write_all(&dims)?;

        // 2. Receive its buffer fd and map the same memory.
        let mut got = [0u8; 8];
        let mut fds = Vec::new();
        scm::recv_with_fds(&ui, &mut got, &mut fds, 1)?;
        let Some(&shmfd) = fds.first() else {
            eprintln!("UI: no buffer fd received from the content process");
            return Ok(ExitCode::from(1));
        };
        let (w, h) = (
            u32::from_le_bytes(got[0..4].try_into().unwrap()),
            u32::from_le_bytes(got[4..8].try_into().unwrap()),
        );
        let buf = SharedBuffer::from_fd(shmfd, (w * h * 4) as usize)?;
        println!("UI: mapped {w}x{h} content buffer shared from the content process");

        // 3. The cross-process pixels are really there (top-left is in the green
        //    half of the content process's render).
        let px = sample(buf.as_slice(), w, 4, 4);
        if px != GREEN {
            eprintln!("UI: content mismatch at (4,4): {px:?}");
            return Ok(ExitCode::from(1));
        }

        // 4. Composite that shared buffer into a Stipple viewport and confirm
        //    render_once paints the content (not the placeholder) — no display.
        let content = Pixmap::from_rgba8(PhysicalSize::new(w, h), buf.as_slice().to_vec());
        let mut app = App::new((), |_s: &(), _cx: &mut Cx<()>| {
            column(vec![
                Element::viewport(PAGE).width(W as f64).height(H as f64),
            ])
        })
        .logical_size(Size::new(W as f64, H as f64))
        .viewport_content(PAGE, content);
        let frame = app.render_once();
        let center = frame.pixel(W / 2, H / 2).unwrap_or([0, 0, 0, 0]);
        if center != GREEN && center != ORANGE {
            eprintln!("UI: viewport not composited (center={center:?})");
            return Ok(ExitCode::from(1));
        }
        println!("UI: composited the shared content into a viewport (center={center:?})");

        // 5. Forward a pointer press (viewport-local) to the content process.
        let (mx, my) = (30i32, 30i32);
        let mut ev = [0u8; 8];
        ev[0..4].copy_from_slice(&mx.to_le_bytes());
        ev[4..8].copy_from_slice(&my.to_le_bytes());
        (&ui).write_all(&ev)?;
        let mut ack = [0u8; 2];
        (&ui).read_exact(&mut ack)?;

        // 6. The content process drew into the shared memory we hold — the input
        //    crossed the process boundary and changed the content.
        let marker = sample(buf.as_slice(), w, mx as u32, my as u32);
        if marker != MARKER {
            eprintln!("UI: forwarded input not applied across processes: {marker:?}");
            return Ok(ExitCode::from(1));
        }
        println!("UI: forwarded input -> content process drew a marker in shared memory");

        let _ = child.wait();
        println!("RESULT: PASS (content process + fd-passing + compositing + input forwarding)");
        Ok(ExitCode::SUCCESS)
    }
}
