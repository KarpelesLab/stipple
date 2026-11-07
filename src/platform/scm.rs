//! File-descriptor passing over a Unix socket (`SCM_RIGHTS`).
//!
//! The kernel can hand a real open file description from one process to another
//! through a Unix-domain socket's ancillary data — the receiver gets its own fd
//! referring to the *same* underlying object (pipe, dma-buf, memfd, …). This is
//! the transport primitive two parts of Stipple need:
//!
//! - **DRI3 / Present** (GPU compositor): send a rendered frame's `dma-buf` fd to
//!   the X server alongside a `PixmapFromBuffers` request, and receive the DRM
//!   device fd back from `DRI3Open` — all on the existing raw X11 socket.
//! - **Content-process IPC** (browser): send a page's `dma-buf` / shared-memory
//!   fd from the sandboxed content process to the UI process.
//!
//! No `nix`/`libc` crate — just `sendmsg`/`recvmsg` and a hand-built control
//! message, the same direct-to-the-OS approach as the rest of the platform layer.
//! Linux only for now (the ancillary-data layout is Linux's; macOS parity lands
//! with the macOS content backend).
#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

use core::ffi::c_void;

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
// Don't raise SIGPIPE if the peer has gone away — surface EPIPE instead.
const MSG_NOSIGNAL: i32 = 0x4000;

#[repr(C)]
struct IoVec {
    base: *mut c_void,
    len: usize,
}

// Linux `struct msghdr`. `#[repr(C)]` supplies the alignment padding (after
// `namelen`, and trailing after `flags`) so the layout matches the C struct.
#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    namelen: u32,
    iov: *mut IoVec,
    iovlen: usize,
    control: *mut c_void,
    controllen: usize,
    flags: i32,
}

unsafe extern "C" {
    fn sendmsg(fd: i32, msg: *const MsgHdr, flags: i32) -> isize;
    fn recvmsg(fd: i32, msg: *mut MsgHdr, flags: i32) -> isize;
}

// `cmsghdr` is `{ size_t cmsg_len; int cmsg_level; int cmsg_type; }` = 16 bytes,
// and the fd array starts at the (8-aligned) 16-byte boundary after it.
const CMSG_HDR: usize = 16;
fn align8(n: usize) -> usize {
    n.div_ceil(8) * 8
}
/// Bytes a control buffer needs to carry `n` fds (header + aligned fd array).
fn cmsg_space(n: usize) -> usize {
    CMSG_HDR + align8(n * 4)
}

/// Send `data` (a non-empty byte slice) over `stream`, attaching `fds` as
/// `SCM_RIGHTS` ancillary data. The receiver gets duplicated fds referring to
/// the same objects. Returns the number of data bytes written (control data is
/// all-or-nothing).
pub fn send_with_fds(stream: &UnixStream, data: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let mut iov = IoVec {
        base: data.as_ptr() as *mut c_void,
        len: data.len(),
    };
    let mut cbuf = vec![0u8; cmsg_space(fds.len()).max(1)];
    let mut msg: MsgHdr = unsafe { core::mem::zeroed() };
    msg.iov = &mut iov;
    msg.iovlen = 1;
    if !fds.is_empty() {
        // cmsghdr: cmsg_len = header + payload, level = SOL_SOCKET, type = RIGHTS.
        let cmsg_len = CMSG_HDR + fds.len() * 4;
        cbuf[0..8].copy_from_slice(&cmsg_len.to_ne_bytes());
        cbuf[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
        cbuf[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
        for (i, fd) in fds.iter().enumerate() {
            cbuf[CMSG_HDR + i * 4..CMSG_HDR + i * 4 + 4].copy_from_slice(&fd.to_ne_bytes());
        }
        msg.control = cbuf.as_mut_ptr() as *mut c_void;
        msg.controllen = cmsg_space(fds.len());
    }
    let n = unsafe { sendmsg(stream.as_raw_fd(), &msg, MSG_NOSIGNAL) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

/// Receive into `buf`, collecting any `SCM_RIGHTS` fds into `fds_out` (up to
/// `max_fds`). Returns the number of data bytes read. The returned fds are owned
/// by the caller, who must close them.
pub fn recv_with_fds(
    stream: &UnixStream,
    buf: &mut [u8],
    fds_out: &mut Vec<RawFd>,
    max_fds: usize,
) -> io::Result<usize> {
    let mut iov = IoVec {
        base: buf.as_mut_ptr() as *mut c_void,
        len: buf.len(),
    };
    let mut cbuf = vec![0u8; cmsg_space(max_fds).max(1)];
    let mut msg: MsgHdr = unsafe { core::mem::zeroed() };
    msg.iov = &mut iov;
    msg.iovlen = 1;
    msg.control = cbuf.as_mut_ptr() as *mut c_void;
    msg.controllen = cmsg_space(max_fds);

    let n = unsafe { recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // Parse a single SCM_RIGHTS control message, if present.
    if msg.controllen >= CMSG_HDR {
        let cmsg_len = usize::from_ne_bytes(cbuf[0..8].try_into().unwrap());
        let level = i32::from_ne_bytes(cbuf[8..12].try_into().unwrap());
        let typ = i32::from_ne_bytes(cbuf[12..16].try_into().unwrap());
        if level == SOL_SOCKET && typ == SCM_RIGHTS && cmsg_len >= CMSG_HDR {
            let count = ((cmsg_len - CMSG_HDR) / 4).min(max_fds);
            for i in 0..count {
                let off = CMSG_HDR + i * 4;
                fds_out.push(i32::from_ne_bytes(cbuf[off..off + 4].try_into().unwrap()));
            }
        }
    }
    Ok(n as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::FromRawFd;

    #[test]
    fn passes_a_real_fd_to_the_peer() {
        // Write a marker into a temp file, pass its fd across a socketpair, and
        // confirm the received fd reads the same file — proving it's the same
        // open description, not a copy of the bytes.
        let mut tmp = tempfile();
        tmp.write_all(b"stipple-scm").unwrap();
        tmp.flush().unwrap();

        let (a, b) = UnixStream::pair().unwrap();
        let sent = send_with_fds(&a, b"x", &[tmp.as_raw_fd()]).unwrap();
        assert_eq!(sent, 1);

        let mut buf = [0u8; 8];
        let mut fds = Vec::new();
        let n = recv_with_fds(&b, &mut buf, &mut fds, 4).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], b'x');
        assert_eq!(fds.len(), 1, "expected exactly one passed fd");

        // The received fd refers to the same file: rewind and read the marker.
        let mut got = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        got.seek(SeekFrom::Start(0)).unwrap();
        let mut s = String::new();
        got.read_to_string(&mut s).unwrap();
        assert_eq!(s, "stipple-scm");
    }

    #[test]
    fn no_fds_when_none_were_sent() {
        let (a, b) = UnixStream::pair().unwrap();
        send_with_fds(&a, b"hi", &[]).unwrap();
        let mut buf = [0u8; 8];
        let mut fds = Vec::new();
        let n = recv_with_fds(&b, &mut buf, &mut fds, 4).unwrap();
        assert_eq!(&buf[..n], b"hi");
        assert!(fds.is_empty());
    }

    fn tempfile() -> std::fs::File {
        let mut path = std::env::temp_dir();
        // A per-process unique-ish name without extra deps.
        path.push(format!("stipple-scm-test-{}", std::process::id()));
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let _ = std::fs::remove_file(&path); // unlink; the open fd keeps it alive
        f
    }
}
