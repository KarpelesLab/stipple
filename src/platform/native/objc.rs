//! The small part of the Objective-C runtime the native renderers use (also
//! shared by the clipboard and file-dialog shims).

use std::ffi::{CString, c_void};

pub type Id = *mut c_void;
pub type Sel = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    pub fn objc_msgSend();
    fn objc_getClass(name: *const std::ffi::c_char) -> Id;
    fn sel_registerName(name: *const std::ffi::c_char) -> Sel;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

unsafe extern "C" {
    /// libSystem: nonzero on the process's main thread.
    fn pthread_main_np() -> std::ffi::c_int;
}

/// Whether this is the main thread, the only one AppKit and UIKit views may
/// be used from.
pub fn on_main_thread() -> bool {
    unsafe { pthread_main_np() != 0 }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct CgSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct CgRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn class(name: &str) -> Id {
    let c = CString::new(name).expect("class name");
    unsafe { objc_getClass(c.as_ptr()) }
}

pub fn sel(name: &str) -> Sel {
    let c = CString::new(name).expect("selector");
    unsafe { sel_registerName(c.as_ptr()) }
}

/// Send a message with a fixed signature: `msg!(obj, "setX:", (x: f64) -> ())`.
macro_rules! msg {
    ($obj:expr, $sel:expr, ($($arg:ident : $ty:ty),*) -> $ret:ty) => {{
        let f: unsafe extern "C" fn($crate::platform::native::objc::Id,
            $crate::platform::native::objc::Sel $(, $ty)*) -> $ret =
            std::mem::transmute($crate::platform::native::objc::objc_msgSend as *const std::ffi::c_void);
        f($obj, $crate::platform::native::objc::sel($sel) $(, $arg)*)
    }};
}
pub(crate) use msg;

/// `[NSString stringWithUTF8String:s]`. Interior NULs end the string.
pub fn nsstring(s: &str) -> Id {
    let s = s.split('\0').next().unwrap_or("");
    let c = CString::new(s).expect("string");
    let p = c.as_ptr();
    unsafe { msg!(class("NSString"), "stringWithUTF8String:", (p: *const std::ffi::c_char) -> Id) }
}

/// Copy an `NSString` into a Rust `String` (via `UTF8String`); `None` for nil.
/// The bytes are copied out before returning, so the result outlives any
/// enclosing autorelease pool.
pub fn rust_string(ns: Id) -> Option<String> {
    if ns.is_null() {
        return None;
    }
    unsafe {
        let p = msg!(ns, "UTF8String", () -> *const std::ffi::c_char);
        if p.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

/// The name of `obj`'s class (`UISwitch`); empty for nil.
pub fn class_name(obj: Id) -> String {
    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn object_getClassName(obj: Id) -> *const std::ffi::c_char;
    }
    if obj.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(object_getClassName(obj)) }
        .to_string_lossy()
        .into_owned()
}

/// Run `f` inside an autorelease pool, for calls made outside the event loop.
pub fn with_pool<R>(f: impl FnOnce() -> R) -> R {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let r = f();
        objc_autoreleasePoolPop(pool);
        r
    }
}

// ---- Blocks that call a Rust closure once -------------------------------------

unsafe extern "C" {
    static _NSConcreteGlobalBlock: c_void;
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A block literal taking one argument of type `A` and calling `f` with it,
/// once. Marked global, so `Block_copy` hands back the same pointer and never
/// frees it: each one is leaked (a few dozen bytes per completion handler).
#[repr(C)]
struct OnceBlock<A> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut OnceBlock<A>, A),
    descriptor: *const BlockDescriptor,
    f: std::sync::Mutex<Option<OnceFn<A>>>,
}

type OnceFn<A> = Box<dyn FnOnce(A) + Send>;

static ONCE_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    // The same for every `A`: it only shows in the function pointer's type.
    size: std::mem::size_of::<OnceBlock<usize>>(),
};

unsafe extern "C" fn once_invoke<A>(block: *mut OnceBlock<A>, arg: A) {
    let f = unsafe { &(*block).f }
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(f) = f {
        f(arg);
    }
}

/// A completion-handler block `^(A arg)` calling `f` the first time it's
/// invoked, from whatever queue the framework answers on. `A` is the
/// argument's C type: `u8` for a `BOOL`, `isize` for an `NSInteger`, [`Id`]
/// for an object.
pub fn once_block<A: 'static>(f: impl FnOnce(A) + Send + 'static) -> Id {
    const BLOCK_IS_GLOBAL: i32 = 1 << 28;
    let block = Box::leak(Box::new(OnceBlock::<A> {
        isa: &raw const _NSConcreteGlobalBlock,
        flags: BLOCK_IS_GLOBAL,
        reserved: 0,
        invoke: once_invoke::<A>,
        descriptor: &ONCE_DESCRIPTOR,
        f: std::sync::Mutex::new(Some(Box::new(f))),
    }));
    (block as *mut OnceBlock<A>).cast()
}

// ---- Frameworks loaded on demand ----------------------------------------------

/// A system framework [`load_framework`] loaded (or didn't find): loaded
/// for good, since the classes it registers outlive any handle.
#[derive(Clone, Copy, Debug)]
pub struct Framework(Option<&'static dlsym::Library>);

impl Framework {
    /// The framework isn't there.
    pub fn is_null(self) -> bool {
        self.0.is_none()
    }
}

/// Load the system framework `name` (`"AVFoundation"`), so its classes are
/// known to the runtime. Loading again is cheap: each framework is opened
/// once and kept.
pub fn load_framework(name: &str) -> Framework {
    static LOADED: std::sync::Mutex<Vec<(String, Framework)>> = std::sync::Mutex::new(Vec::new());
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, f)) = loaded.iter().find(|(n, _)| n == name) {
        return *f;
    }
    let path = format!("/System/Library/Frameworks/{name}.framework/{name}");
    // SAFETY: a system framework, trusted like the rest of the platform.
    let lib = unsafe { dlsym::os::unix::OpenOptions::new().lazy(true).open(path) };
    let f = Framework(lib.ok().map(dlsym::Library::leak));
    loaded.push((name.to_owned(), f));
    f
}

/// The address of symbol `name` of a framework [`load_framework`] loaded;
/// null if missing.
pub fn symbol(framework: Framework, name: &std::ffi::CStr) -> *mut c_void {
    let Some(lib) = framework.0 else {
        return std::ptr::null_mut();
    };
    // SAFETY: only the address is taken; the caller gives it its type. The
    // framework stays loaded, so the address stays valid.
    unsafe { lib.get::<*mut c_void>(name) }.map_or(std::ptr::null_mut(), |s| s.as_ptr())
}
