//! A minimal syscall sandbox for the content process (Linux, seccomp-bpf).
//!
//! In the Stipple-as-compositor model the page is rendered by a *separate* content
//! process that should run with as little authority as possible: once it has its
//! IPC socket and shared buffer, it never needs to open new network connections
//! or execute programs. [`restrict`] installs a seccomp-BPF filter that makes
//! those syscalls fail — so a compromised content process can't phone home or
//! spawn a shell, while the syscalls it does need (read/write on the existing
//! socket fd, `mmap`, etc.) keep working.
//!
//! This is the hardening layer of the content-process path (`contentproc`), the
//! analog of a browser renderer sandbox. The filter is a blocklist (deny a few
//! clearly-dangerous syscalls, allow the rest) — conservative so it can't break
//! the render loop — and is x86-64 specific; on other arches [`restrict`] is a
//! no-op-with-error so callers can decide whether to proceed.
//!
//! `prctl` + the BPF filter are raw FFI — the reason for the module-level
//! `allow(unsafe_code)`.
#![allow(unsafe_code)]

use std::io;

// prctl options.
const PR_SET_NO_NEW_PRIVS: i32 = 38;
const PR_SET_SECCOMP: i32 = 22;
const SECCOMP_MODE_FILTER: u64 = 2;

unsafe extern "C" {
    fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
}

/// A classic BPF instruction (`struct sock_filter`).
#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

/// A BPF program (`struct sock_fprog`).
#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

/// Install the content-process seccomp sandbox on the current thread/process:
/// set `NO_NEW_PRIVS` and load a seccomp-BPF filter that makes a few dangerous
/// syscalls fail with `EPERM` (creating sockets, connecting, `execve`/`execveat`,
/// `ptrace`) while allowing everything else — so the content process keeps using
/// its existing IPC fd and shared memory but can't open the network or exec.
///
/// Irreversible and inherited across `fork`. Call it in the content process
/// *after* it has acquired the resources it needs (its socket + buffer). Errors
/// if seccomp isn't available or on a non-x86-64 arch (where this filter's
/// syscall numbers don't apply).
pub fn restrict() -> io::Result<()> {
    #[cfg(target_arch = "x86_64")]
    {
        // BPF opcodes.
        const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
        const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
        const RET_K: u16 = 0x06; // BPF_RET | BPF_K
        // seccomp_data field offsets.
        const OFF_NR: u32 = 0;
        const OFF_ARCH: u32 = 4;
        const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
        // Filter return actions.
        const RET_ALLOW: u32 = 0x7FFF_0000;
        const RET_ERRNO_EPERM: u32 = 0x0005_0000 | 1; // SECCOMP_RET_ERRNO | EPERM
        const RET_KILL_PROCESS: u32 = 0x8000_0000;
        // x86-64 syscall numbers to deny.
        const NR_SOCKET: u32 = 41;
        const NR_CONNECT: u32 = 42;
        const NR_EXECVE: u32 = 59;
        const NR_PTRACE: u32 = 101;
        const NR_EXECVEAT: u32 = 322;

        // `if syscall == nr { fall through to deny } else { skip the deny }`.
        let jeq = |nr: u32| SockFilter {
            code: JEQ_K,
            jt: 0,
            jf: 1,
            k: nr,
        };
        let deny = SockFilter {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: RET_ERRNO_EPERM,
        };
        let prog = [
            // Reject any non-x86-64 syscall ABI outright (the nr table below is
            // arch-specific, so a foreign ABI must not slip through as "allow").
            SockFilter {
                code: LD_W_ABS,
                jt: 0,
                jf: 0,
                k: OFF_ARCH,
            },
            SockFilter {
                code: JEQ_K,
                jt: 1,
                jf: 0,
                k: AUDIT_ARCH_X86_64,
            },
            SockFilter {
                code: RET_K,
                jt: 0,
                jf: 0,
                k: RET_KILL_PROCESS,
            },
            // Load the syscall number, then deny each blocked syscall.
            SockFilter {
                code: LD_W_ABS,
                jt: 0,
                jf: 0,
                k: OFF_NR,
            },
            jeq(NR_SOCKET),
            deny,
            jeq(NR_CONNECT),
            deny,
            jeq(NR_EXECVE),
            deny,
            jeq(NR_EXECVEAT),
            deny,
            jeq(NR_PTRACE),
            deny,
            // Default: allow.
            SockFilter {
                code: RET_K,
                jt: 0,
                jf: 0,
                k: RET_ALLOW,
            },
        ];

        unsafe {
            if prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            let fprog = SockFprog {
                len: prog.len() as u16,
                filter: prog.as_ptr(),
            };
            if prctl(
                PR_SET_SECCOMP,
                SECCOMP_MODE_FILTER,
                &fprog as *const SockFprog as u64,
                0,
                0,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "seccomp content sandbox is implemented for x86-64 only",
        ))
    }
}
