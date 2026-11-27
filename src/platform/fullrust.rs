//! The C library, as far as the Linux backends use it, for static,
//! libc-free builds (`x86_64-unknown-linux-fullrust`).
//!
//! The Linux code declares the handful of libc functions it calls
//! (`poll`, `mmap`, `sendmsg`, System V shared memory...) in `extern "C"`
//! blocks, as everywhere else in the crate. A fullrust binary has no libc to
//! link them against, so this module defines them, under their C names, as
//! the system calls they wrap (through `std::os::fullrust::syscall`), with a
//! per-thread `errno` behind `__errno_location`. Nothing else changes: the
//! X11 and Wayland backends, AT-SPI, the portals and the rest speak their
//! protocols over sockets as on any Linux.
//!
//! `localtime_r` isn't a system call: it's the system libc's, loaded through
//! `dlsym` (which boots the system's `ld.so` on fullrust), and fails (UTC)
//! where there is none.
//!
//! Variadic C functions (`ioctl`, `fcntl`, `open`) are defined with their
//! third argument as a plain register-sized value, which is how x86-64's C
//! calling convention passes it either way (fullrust is x86-64 only).

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::io;
use std::os::fullrust::syscall::{self, nr};

thread_local! {
    static ERRNO: Cell<c_int> = const { Cell::new(0) };
}

/// `-1` (or the error's pointer form) with `errno` set, or the value.
fn cvt(r: io::Result<usize>) -> isize {
    match r {
        Ok(v) => v as isize,
        Err(e) => {
            const EIO: c_int = 5;
            ERRNO.with(|errno| errno.set(e.raw_os_error().unwrap_or(EIO)));
            -1
        }
    }
}

fn cvt_int(r: io::Result<usize>) -> c_int {
    cvt(r) as c_int
}

fn cvt_ptr(r: io::Result<usize>) -> *mut c_void {
    cvt(r) as *mut c_void
}

#[unsafe(no_mangle)]
extern "C" fn __errno_location() -> *mut c_int {
    ERRNO.with(Cell::as_ptr)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize {
    cvt(unsafe { syscall::syscall3(nr::READ, fd as usize, buf as usize, n) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn write(fd: c_int, buf: *const c_void, n: usize) -> isize {
    cvt(unsafe { syscall::syscall3(nr::WRITE, fd as usize, buf as usize, n) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn pwrite(fd: c_int, buf: *const c_void, n: usize, off: i64) -> isize {
    cvt(unsafe { syscall::syscall4(nr::PWRITE64, fd as usize, buf as usize, n, off as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::OPEN, path as usize, flags as usize, mode as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn close(fd: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall1(nr::CLOSE, fd as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn dup(fd: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall1(nr::DUP, fd as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn lseek(fd: c_int, off: i64, whence: c_int) -> i64 {
    cvt(unsafe { syscall::syscall3(nr::LSEEK, fd as usize, off as usize, whence as usize) }) as i64
}

#[unsafe(no_mangle)]
unsafe extern "C" fn ftruncate(fd: c_int, len: i64) -> c_int {
    cvt_int(unsafe { syscall::syscall2(nr::FTRUNCATE, fd as usize, len as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn fcntl(fd: c_int, cmd: c_int, arg: usize) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::FCNTL, fd as usize, cmd as usize, arg) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn ioctl(fd: c_int, request: c_ulong, arg: usize) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::IOCTL, fd as usize, request as usize, arg) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn flock(fd: c_int, op: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall2(nr::FLOCK, fd as usize, op as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn pipe2(fds: *mut c_int, flags: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall2(nr::PIPE2, fds as usize, flags as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poll(fds: *mut c_void, n: c_ulong, timeout: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::POLL, fds as usize, n as usize, timeout as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memfd_create(name: *const c_char, flags: c_uint) -> c_int {
    cvt_int(unsafe { syscall::syscall2(nr::MEMFD_CREATE, name as usize, flags as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mmap(
    addr: *mut c_void,
    len: usize,
    prot: c_int,
    flags: c_int,
    fd: c_int,
    off: i64,
) -> *mut c_void {
    cvt_ptr(unsafe {
        syscall::syscall6(
            nr::MMAP,
            addr as usize,
            len,
            prot as usize,
            flags as usize,
            fd as usize,
            off as usize,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn munmap(addr: *mut c_void, len: usize) -> c_int {
    cvt_int(unsafe { syscall::syscall2(nr::MUNMAP, addr as usize, len) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn shmget(key: c_int, size: usize, flags: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::SHMGET, key as usize, size, flags as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn shmat(id: c_int, addr: *const c_void, flags: c_int) -> *mut c_void {
    cvt_ptr(unsafe { syscall::syscall3(nr::SHMAT, id as usize, addr as usize, flags as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn shmdt(addr: *const c_void) -> c_int {
    cvt_int(unsafe { syscall::syscall1(nr::SHMDT, addr as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn shmctl(id: c_int, cmd: c_int, buf: *mut c_void) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::SHMCTL, id as usize, cmd as usize, buf as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int {
    cvt_int(unsafe {
        syscall::syscall3(
            nr::SOCKET,
            domain as usize,
            kind as usize,
            protocol as usize,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn bind(fd: c_int, addr: *const c_void, len: u32) -> c_int {
    cvt_int(unsafe { syscall::syscall3(nr::BIND, fd as usize, addr as usize, len as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn send(fd: c_int, buf: *const c_void, len: usize, flags: c_int) -> isize {
    cvt(unsafe {
        syscall::syscall6(
            nr::SENDTO,
            fd as usize,
            buf as usize,
            len,
            flags as usize,
            0,
            0,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize {
    cvt(unsafe {
        syscall::syscall6(
            nr::RECVFROM,
            fd as usize,
            buf as usize,
            len,
            flags as usize,
            0,
            0,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sendmsg(fd: c_int, msg: *const c_void, flags: c_int) -> isize {
    cvt(unsafe { syscall::syscall3(nr::SENDMSG, fd as usize, msg as usize, flags as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn recvmsg(fd: c_int, msg: *mut c_void, flags: c_int) -> isize {
    cvt(unsafe { syscall::syscall3(nr::RECVMSG, fd as usize, msg as usize, flags as usize) })
}

#[unsafe(no_mangle)]
extern "C" fn getuid() -> c_uint {
    // SAFETY: no arguments, no side effects.
    cvt(unsafe { syscall::syscall0(nr::GETUID) }) as c_uint
}

#[unsafe(no_mangle)]
extern "C" fn geteuid() -> c_uint {
    // SAFETY: no arguments, no side effects.
    cvt(unsafe { syscall::syscall0(nr::GETEUID) }) as c_uint
}

#[unsafe(no_mangle)]
unsafe extern "C" fn prctl(
    option: c_int,
    a2: c_ulong,
    a3: c_ulong,
    a4: c_ulong,
    a5: c_ulong,
) -> c_int {
    cvt_int(unsafe {
        syscall::syscall5(
            nr::PRCTL,
            option as usize,
            a2 as usize,
            a3 as usize,
            a4 as usize,
            a5 as usize,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn inotify_init1(flags: c_int) -> c_int {
    cvt_int(unsafe { syscall::syscall1(nr::INOTIFY_INIT1, flags as usize) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int {
    cvt_int(unsafe {
        syscall::syscall3(
            nr::INOTIFY_ADD_WATCH,
            fd as usize,
            path as usize,
            mask as usize,
        )
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn clock_gettime(clock: c_int, ts: *mut c_void) -> c_int {
    cvt_int(unsafe { syscall::syscall2(nr::CLOCK_GETTIME, clock as usize, ts as usize) })
}

dlsym::library! {
    /// The system libc's time zone conversion.
    struct Time {
        unsafe extern "C" {
            #[link_name = "localtime_r"]
            fn localtime(t: *const c_long, out: *mut c_void) -> *mut c_void;
        }
    }
}

/// The system libc's `localtime_r` (its `struct tm` is the caller's), or a
/// failure (null) where there's no system loader.
#[unsafe(no_mangle)]
unsafe extern "C" fn localtime_r(t: *const c_long, out: *mut c_void) -> *mut c_void {
    static TIME: std::sync::OnceLock<Option<Time>> = std::sync::OnceLock::new();
    let time = TIME.get_or_init(|| {
        let libc = dlsym::Library::this().ok()?;
        <Time as dlsym::Bindings>::load(libc).ok()
    });
    match time {
        // SAFETY: the caller's pointers, passed on.
        Some(time) => unsafe { time.localtime(t, out) },
        None => std::ptr::null_mut(),
    }
}
