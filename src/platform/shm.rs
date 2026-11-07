//! `memfd`-backed shared-memory buffers for cross-process content (Linux).
//!
//! The **CPU side of the Stipple-as-compositor content path**: a content process
//! renders pixels into a [`SharedBuffer`] (a `memfd` mapped `MAP_SHARED`) and
//! passes its fd to the UI process over a Unix socket with [`crate::platform::scm`]; the UI
//! process maps the same fd ([`SharedBuffer::from_fd`]) and composites the pixels
//! into a viewport. This is the dual of the GPU `dma-buf` path — the same
//! architecture (a separate, sandboxable process; the buffer fd handed over a
//! socket) with a CPU buffer instead of a GPU texture, so it works with no GPU
//! (and is the path used when GPU sharing is unavailable).
//!
//! `memfd_create`/`mmap`/`munmap` are raw libc FFI — the reason for the
//! module-level `allow(unsafe_code)` (the rest of the crate stays safe).
#![allow(unsafe_code)]

use std::ffi::{c_char, c_void};
use std::io;
use std::os::fd::RawFd;

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: u32) -> i32;
    fn ftruncate(fd: i32, length: i64) -> i32;
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
    -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn close(fd: i32) -> i32;
}

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

/// A `memfd`-backed byte region mapped `MAP_SHARED` into this process. Owns the
/// fd and the mapping; both are released on drop. Share it across a process
/// boundary by passing [`fd`](SharedBuffer::fd) over a socket (see
/// [`crate::platform::scm::send_with_fds`]) and re-mapping it in the peer with
/// [`from_fd`](SharedBuffer::from_fd) — writes through either mapping are visible
/// to the other.
pub struct SharedBuffer {
    fd: RawFd,
    ptr: *mut u8,
    len: usize,
}

impl SharedBuffer {
    /// Create a new `memfd` of `len` bytes, mapped read/write.
    pub fn create(len: usize) -> io::Result<Self> {
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zero-length shared buffer",
            ));
        }
        let name = c"stipple-shm";
        let fd = unsafe { memfd_create(name.as_ptr(), 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { ftruncate(fd, len as i64) } < 0 {
            let e = io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(e);
        }
        Self::map(fd, len)
    }

    /// Map an existing `memfd` `fd` of `len` bytes — e.g. a buffer fd received
    /// from a content process over a socket. Takes ownership of `fd` (closed on
    /// drop). The caller must ensure `fd` really refers to a region of at least
    /// `len` bytes.
    pub fn from_fd(fd: RawFd, len: usize) -> io::Result<Self> {
        Self::map(fd, len)
    }

    fn map(fd: RawFd, len: usize) -> io::Result<Self> {
        let ptr = unsafe {
            mmap(
                core::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == MAP_FAILED {
            let e = io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(e);
        }
        Ok(Self {
            fd,
            ptr: ptr as *mut u8,
            len,
        })
    }

    /// The backing `memfd` (for passing to a peer via `SCM_RIGHTS`).
    #[inline]
    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl core::fmt::Debug for SharedBuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedBuffer")
            .field("fd", &self.fd)
            .field("len", &self.len)
            .finish()
    }
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        unsafe {
            munmap(self.ptr as *mut c_void, self.len);
            close(self.fd);
        }
    }
}

// SAFETY: the handle owns a plain shared byte region; moving it between threads
// is sound (concurrent access is &/&mut-guarded like any slice).
unsafe impl Send for SharedBuffer {}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        fn dup(oldfd: i32) -> i32;
    }

    #[test]
    fn second_mapping_sees_writes_through_the_shared_memfd() {
        let mut a = SharedBuffer::create(64).expect("create");
        a.as_mut_slice()[..4].copy_from_slice(&[1, 2, 3, 4]);

        // A second mapping of the same memfd (as a peer process would make from a
        // received fd) sees the producer's writes — proving MAP_SHARED sharing.
        let fd2 = unsafe { dup(a.fd()) };
        assert!(fd2 >= 0, "dup failed");
        let mut b = SharedBuffer::from_fd(fd2, 64).expect("from_fd");
        assert_eq!(&b.as_slice()[..4], &[1, 2, 3, 4]);

        // ...and writes through the second mapping are visible through the first.
        b.as_mut_slice()[8..12].copy_from_slice(&[9, 9, 9, 9]);
        assert_eq!(&a.as_slice()[8..12], &[9, 9, 9, 9]);
    }
}
