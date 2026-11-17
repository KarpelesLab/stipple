//! extensiondemo's app extension: a notification service extension
//! (`com.apple.usernotifications.service`) written in Rust, in the app's
//! `PlugIns/Service.appex`.
//!
//! An extension's `Info.plist` names a principal class
//! (`NSExtensionPrincipalClass`), which `NSExtensionMain` instantiates to
//! serve the host. Here that class, `ExtensionDemoService`, a subclass of
//! `UNNotificationServiceExtension`, is made at run time with the
//! Objective-C runtime's `objc_allocateClassPair`, before `main` hands the
//! process to `NSExtensionMain` through
//! [`platform::extension_main`](stipple::platform::extension_main). A
//! Network Extension provider (`NEPacketTunnelProvider`) is made the same
//! way, overriding `startTunnelWithOptions:completionHandler:` and
//! `stopTunnelWithReason:completionHandler:`.
//!
//! For each remote notification with `mutable-content`, the service adds
//! "(via the extension)" to the title and appends a line to
//! `from-extension.txt` in the app group's shared container, which the app
//! shows.
//!
//! **Self-test.** Run with `EXTENSIONDEMO_SELFTEST` set (`xcrun simctl spawn
//! booted .../Service.appex/extensiondemo_ext` with
//! `SIMCTL_CHILD_EXTENSIONDEMO_SELFTEST=1`), it doesn't start
//! `NSExtensionMain`: it prints what Stipple's non-UI parts say inside the
//! extension (its bundle, directories, languages, the shared container,
//! the file the app wrote there), registers its class, and exits. CI runs it.

#[cfg(target_os = "ios")]
fn main() {
    ios::main();
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!(
        "extensiondemo_ext is an iOS app extension: package it with \
         `cargo-stipple ios --example extensiondemo --config examples/extensiondemo/package.toml`"
    );
    // The parts that don't need iOS work here too.
    let _ = stipple::platform::shared_dir(GROUP);
}

/// The app group the app and its extension share.
const GROUP: &str = "group.com.karpeleslab.stipple.extdemo";

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod ios {
    use super::GROUP;
    use std::ffi::{CStr, CString, c_char, c_void};

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(cls: Id);
        fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }

    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    }

    /// The class `NSExtensionPrincipalClass` names in package.toml.
    const PRINCIPAL: &CStr = c"ExtensionDemoService";

    fn sel(name: &CStr) -> Sel {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    fn class(name: &CStr) -> Id {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    /// `[obj sel]` returning an object.
    unsafe fn send0(obj: Id, s: &CStr) -> Id {
        let f: unsafe extern "C" fn(Id, Sel) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
        unsafe { f(obj, sel(s)) }
    }

    /// `[obj sel:arg]` with an object or pointer argument.
    unsafe fn send1(obj: Id, s: &CStr, arg: *const c_void) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, *const c_void) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
        unsafe { f(obj, sel(s), arg) }
    }

    fn nsstring(s: &str) -> Id {
        let c = CString::new(s.replace('\0', "")).unwrap_or_default();
        unsafe {
            send1(
                class(c"NSString"),
                c"stringWithUTF8String:",
                c.as_ptr().cast(),
            )
        }
    }

    fn rust_string(ns: Id) -> String {
        if ns.is_null() {
            return String::new();
        }
        let p = unsafe { send0(ns, c"UTF8String") } as *const c_char;
        if p.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// The start of every block literal: what's needed to call one.
    #[repr(C)]
    struct BlockHeader {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: *const c_void,
    }

    /// Call `^(UNNotificationContent *content)` with `content`.
    unsafe fn call_content_handler(block: Id, content: Id) {
        if block.is_null() {
            return;
        }
        let invoke = unsafe { (*(block as *const BlockHeader)).invoke };
        let invoke: unsafe extern "C" fn(Id, Id) = unsafe { std::mem::transmute(invoke) };
        unsafe { invoke(block, content) };
    }

    fn append_shared(line: &str) {
        use std::io::Write;
        let Some(dir) = stipple::platform::shared_dir(GROUP) else {
            return;
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("from-extension.txt"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// `-didReceiveNotificationRequest:withContentHandler:`: the title, marked,
    /// handed back.
    unsafe extern "C" fn did_receive(_this: Id, _cmd: Sel, request: Id, handler: Id) {
        unsafe {
            let content = send0(send0(request, c"content"), c"mutableCopy");
            let title = rust_string(send0(content, c"title"));
            append_shared(&format!("notification {title:?}"));
            let marked = nsstring(&format!("{title} (via the extension)"));
            send1(content, c"setTitle:", marked);
            call_content_handler(handler, content);
            send0(content, c"release");
        }
    }

    /// `ExtensionDemoService : UNNotificationServiceExtension`, registered
    /// once. Null if UserNotifications isn't there.
    fn register() -> Id {
        let existing = class(PRINCIPAL);
        if !existing.is_null() {
            return existing;
        }
        unsafe {
            // Its classes are known once the framework is loaded.
            dlopen(
                c"/System/Library/Frameworks/UserNotifications.framework/UserNotifications"
                    .as_ptr(),
                1,
            );
            let base = class(c"UNNotificationServiceExtension");
            if base.is_null() {
                return std::ptr::null_mut();
            }
            let cls = objc_allocateClassPair(base, PRINCIPAL.as_ptr(), 0);
            if cls.is_null() {
                return std::ptr::null_mut();
            }
            let imp: unsafe extern "C" fn(Id, Sel, Id, Id) = did_receive;
            class_addMethod(
                cls,
                sel(c"didReceiveNotificationRequest:withContentHandler:"),
                imp as *const c_void,
                c"v@:@@?".as_ptr(),
            );
            objc_registerClassPair(cls);
            cls
        }
    }

    pub fn main() {
        if std::env::var_os("EXTENSIONDEMO_SELFTEST").is_some() {
            selftest();
            return;
        }
        if register().is_null() {
            eprintln!("extensiondemo_ext: no UNNotificationServiceExtension");
            std::process::exit(1);
        }
        let Err(e) = stipple::platform::extension_main();
        eprintln!("extensiondemo_ext: {e}");
        std::process::exit(1);
    }

    fn selftest() {
        let info = stipple::platform::app_info();
        println!(
            "extensiondemo_ext: id={} name={:?} version={:?} build={:?}",
            info.id, info.name, info.version, info.build
        );
        match stipple::platform::dirs() {
            Ok(d) => println!("extensiondemo_ext: dirs data={}", d.data.display()),
            Err(e) => println!("extensiondemo_ext: dirs error {e}"),
        }
        println!(
            "extensiondemo_ext: languages={:?} locale={}",
            stipple::platform::preferred_languages(),
            stipple::platform::locale().tag()
        );
        let shared = stipple::platform::shared_dir(GROUP);
        match &shared {
            Some(dir) => {
                println!("extensiondemo_ext: shared_dir={}", dir.display());
                let from_app = std::fs::read_to_string(dir.join("from-app.txt"));
                println!("extensiondemo_ext: from-app.txt {from_app:?}");
            }
            None => println!("extensiondemo_ext: shared_dir=None"),
        }
        append_shared("selftest");
        let cls = register();
        let base = class(c"UNNotificationServiceExtension");
        let is_sub = !cls.is_null() && {
            let f: unsafe extern "C" fn(Id, Sel, Id) -> bool =
                unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
            unsafe { f(cls, sel(c"isSubclassOfClass:"), base) }
        };
        println!("extensiondemo_ext: principal class registered={is_sub}");
    }
}
