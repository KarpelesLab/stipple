//! The single-instance transport on Windows: the owner holds the named mutex
//! `Local\stipple-instance-<id>` (one per logon session) for its life and
//! serves the named pipe `\\.\pipe\stipple-instance-<id>-<session>`, refusing
//! remote clients. A second launch opens the pipe as a file, and first lets
//! whoever owns the foreground hand it on (`AllowSetForegroundWindow`), so
//! that the window the owner shows for it may come to the front.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;

use super::{Launch, file_name, serve};

type Handle = *mut c_void;

const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_PIPE_CONNECTED: u32 = 535;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const PIPE_ACCESS_DUPLEX: u32 = 0x3;
const PIPE_TYPE_BYTE: u32 = 0x0;
const PIPE_READMODE_BYTE: u32 = 0x0;
const PIPE_WAIT: u32 = 0x0;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const ASFW_ANY: u32 = u32::MAX;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer: u32,
        in_buffer: u32,
        default_timeout: u32,
        attributes: *const c_void,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
    fn DisconnectNamedPipe(pipe: Handle) -> i32;
    fn FlushFileBuffers(file: Handle) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
}

/// The mutex the process that owns an id holds.
pub(super) struct Owner {
    /// The mutex's handle, as an address (so the struct crosses threads).
    mutex: usize,
    pipe: Vec<u16>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The pipe's name for `id`, in this logon session.
fn pipe_name(id: &str) -> String {
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    format!(r"\\.\pipe\stipple-instance-{}-{session}", file_name(id))
}

/// Take `id`: `Some` holding its mutex, `None` when another process holds it.
pub(super) fn own(id: &str) -> io::Result<Option<Owner>> {
    let name = wide(&format!(r"Local\stipple-instance-{}", file_name(id)));
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let err = unsafe { GetLastError() };
    if mutex.is_null() {
        return Err(io::Error::from_raw_os_error(err as i32));
    }
    if err == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(mutex) };
        return Ok(None);
    }
    Ok(Some(Owner {
        mutex: mutex as usize,
        pipe: wide(&pipe_name(id)),
    }))
}

/// Serve launches on a thread for the process's life, handing each to `sink`:
/// one pipe instance at a time, each made before the last one's client is
/// served, so a launch never finds no pipe to open.
pub(super) fn listen(owner: Owner, sink: Box<dyn Fn(Launch) + Send + Sync>) {
    let spawned = std::thread::Builder::new()
        .name("stipple-instance".into())
        .spawn(move || {
            let create = || unsafe {
                CreateNamedPipeW(
                    owner.pipe.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    4096,
                    4096,
                    0,
                    std::ptr::null(),
                )
            };
            let mut pipe = create();
            while pipe != INVALID_HANDLE_VALUE {
                let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                let next = create();
                if connected {
                    // Owns the handle: closing it ends the connection.
                    let file = unsafe { File::from_raw_handle(pipe) };
                    serve(&file, &*sink);
                    unsafe {
                        FlushFileBuffers(pipe);
                        DisconnectNamedPipe(pipe);
                    }
                    drop(file);
                } else {
                    unsafe { CloseHandle(pipe) };
                }
                pipe = next;
            }
            unsafe { CloseHandle(owner.mutex as Handle) };
        });
    if let Err(e) = spawned {
        crate::platform::window::debug_window(format_args!("single instance: {e}"));
    }
}

/// Open the pipe of the owner of `id`.
pub(super) fn connect(id: &str) -> io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_name(id))?;
    // The owner brings its window up for this launch: let it take the
    // foreground this process may have been given.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    Ok(file)
}
