//! macOS and iOS: Network.framework's path monitor (`nw_path_monitor`),
//! through its C API.
//!
//! The framework is loaded when the network is first watched (`dlopen`), so
//! apps that never ask don't link it. The monitor runs on a serial dispatch
//! queue of its own and calls its update handler (a block built by hand,
//! global, so the framework's copy of it is a no-op) with each new path:
//! the first one right after it starts, then one per change. Each path is
//! read into a [`Reading`]: online when it's `satisfied`, the transport by
//! `nw_path_uses_interface_type`, `nw_path_is_expensive` for metered (a
//! cellular network, a phone's hotspot) and `nw_path_is_constrained` for Low
//! Data Mode (macOS 10.15, iOS 13). The network's identity is the index and
//! type of the path's first interface (the one traffic takes), so a new
//! default network is told from the old one even when both are, say, Wi-Fi.

#![allow(unsafe_code)]

use super::{Reading, Transport};
use crate::platform::native::objc::{load_framework, symbol};
use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

type Obj = *mut c_void;

// `nw_path_status_t`.
const STATUS_SATISFIED: i32 = 1;
// `nw_interface_type_t`.
const INTERFACE_WIFI: i32 = 1;
const INTERFACE_CELLULAR: i32 = 2;
const INTERFACE_WIRED: i32 = 3;

/// Network.framework's functions, looked up once.
struct Lib {
    monitor_create: unsafe extern "C" fn() -> Obj,
    monitor_set_queue: unsafe extern "C" fn(Obj, Obj),
    monitor_set_update_handler: unsafe extern "C" fn(Obj, *const c_void),
    monitor_start: unsafe extern "C" fn(Obj),
    path_get_status: unsafe extern "C" fn(Obj) -> i32,
    path_uses_interface_type: unsafe extern "C" fn(Obj, i32) -> bool,
    path_is_expensive: unsafe extern "C" fn(Obj) -> bool,
    /// macOS 10.15, iOS 13.
    path_is_constrained: Option<unsafe extern "C" fn(Obj) -> bool>,
    path_enumerate_interfaces: unsafe extern "C" fn(Obj, *const c_void),
    interface_get_index: unsafe extern "C" fn(Obj) -> u32,
    interface_get_type: unsafe extern "C" fn(Obj) -> i32,
}

// Each symbol's type is its field's.
#[allow(clippy::missing_transmute_annotations)]
fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let nw = load_framework("Network");
        if nw.is_null() {
            return None;
        }
        macro_rules! f {
            ($name:literal) => {{
                let p = symbol(nw, $name);
                if p.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, _>(p)
            }};
        }
        let constrained = symbol(nw, c"nw_path_is_constrained");
        Some(Lib {
            monitor_create: f!(c"nw_path_monitor_create"),
            monitor_set_queue: f!(c"nw_path_monitor_set_queue"),
            monitor_set_update_handler: f!(c"nw_path_monitor_set_update_handler"),
            monitor_start: f!(c"nw_path_monitor_start"),
            path_get_status: f!(c"nw_path_get_status"),
            path_uses_interface_type: f!(c"nw_path_uses_interface_type"),
            path_is_expensive: f!(c"nw_path_is_expensive"),
            path_is_constrained: (!constrained.is_null()).then(|| std::mem::transmute(constrained)),
            path_enumerate_interfaces: f!(c"nw_path_enumerate_interfaces"),
            interface_get_index: f!(c"nw_interface_get_index"),
            interface_get_type: f!(c"nw_interface_get_type"),
        })
    })
    .as_ref()
}

unsafe extern "C" {
    /// The isa of a block that lives forever (libSystem's libclosure).
    static _NSConcreteGlobalBlock: c_void;
    // libdispatch, in libSystem.
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> Obj;
}

/// `BLOCK_IS_GLOBAL`: copies and releases of the block are no-ops.
const BLOCK_IS_GLOBAL: i32 = 1 << 28;

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A block literal with no captures, calling `invoke`.
#[repr(C)]
struct Block<F> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: F,
    descriptor: *const BlockDescriptor,
}

// SAFETY: the blocks are immutable statics with no captures.
unsafe impl<F> Sync for Block<F> {}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<Block<usize>>(),
};

type UpdateFn = unsafe extern "C" fn(*const c_void, Obj);
type InterfaceFn = unsafe extern "C" fn(*const c_void, Obj) -> bool;

/// `^(nw_path_t path)`: the monitor's update handler.
static UPDATE: Block<UpdateFn> = Block {
    isa: &raw const _NSConcreteGlobalBlock,
    flags: BLOCK_IS_GLOBAL,
    reserved: 0,
    invoke: updated,
    descriptor: &DESCRIPTOR,
};

/// `^bool(nw_interface_t interface)`: takes the path's first interface.
static FIRST_INTERFACE: Block<InterfaceFn> = Block {
    isa: &raw const _NSConcreteGlobalBlock,
    flags: BLOCK_IS_GLOBAL,
    reserved: 0,
    invoke: first_interface,
    descriptor: &DESCRIPTOR,
};

thread_local! {
    /// The first interface's index and type, while enumerating (on the
    /// monitor's queue).
    static FIRST: std::cell::Cell<Option<(u32, i32)>> = const { std::cell::Cell::new(None) };
}

unsafe extern "C" fn first_interface(_block: *const c_void, interface: Obj) -> bool {
    if let Some(lib) = lib() {
        let index = unsafe { (lib.interface_get_index)(interface) };
        let kind = unsafe { (lib.interface_get_type)(interface) };
        FIRST.set(Some((index, kind)));
    }
    // Stop: the first is the one.
    false
}

/// A new path: read it and report it.
unsafe extern "C" fn updated(_block: *const c_void, path: Obj) {
    let Some(lib) = lib() else {
        return;
    };
    if path.is_null() {
        return;
    }
    super::report(unsafe { read(lib, path) });
}

unsafe fn read(lib: &Lib, path: Obj) -> Reading {
    unsafe {
        if (lib.path_get_status)(path) != STATUS_SATISFIED {
            return Reading::OFFLINE;
        }
        let uses = |kind| (lib.path_uses_interface_type)(path, kind);
        let transport = if uses(INTERFACE_WIFI) {
            Transport::Wifi
        } else if uses(INTERFACE_CELLULAR) {
            Transport::Cellular
        } else if uses(INTERFACE_WIRED) {
            Transport::Ethernet
        } else {
            Transport::Other
        };
        FIRST.set(None);
        (lib.path_enumerate_interfaces)(path, (&raw const FIRST_INTERFACE).cast());
        let (index, kind) = FIRST.take().unwrap_or((0, 0));
        Reading {
            online: true,
            transport,
            metered: (lib.path_is_expensive)(path),
            constrained: lib.path_is_constrained.is_some_and(|f| f(path)),
            network: ((index as u64) << 8 | (kind as u8 as u64)) | 1 << 40,
        }
    }
}

/// Start the monitor, once.
pub(super) fn start() {
    let Some(lib) = lib() else {
        eprintln!("stipple: Network.framework isn't available: can't watch the network");
        return;
    };
    unsafe {
        let queue = dispatch_queue_create(c"org.stipple.network".as_ptr(), std::ptr::null());
        let monitor = (lib.monitor_create)();
        if queue.is_null() || monitor.is_null() {
            eprintln!("stipple: can't create the network path monitor");
            return;
        }
        // Both live as long as the process.
        (lib.monitor_set_queue)(monitor, queue);
        (lib.monitor_set_update_handler)(monitor, (&raw const UPDATE).cast());
        (lib.monitor_start)(monitor);
    }
}
