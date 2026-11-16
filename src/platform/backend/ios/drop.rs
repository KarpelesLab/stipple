//! Drops from other apps (iPad split view and Slide Over, and between apps
//! on iPhone since iOS 15): a `UIDropInteraction` on the view, whose
//! delegate turns the session into [`Event::DragEnter`] / `DragOver` /
//! `DragLeave` / `Drop`.
//!
//! What the drag offers comes from each item's `NSItemProvider`
//! ([`mobile_drop::ios_read`]): an image or a file is copied, other items
//! with a type too, text is text and a web link is its URL. UIKit asks
//! `dropInteraction:sessionDidUpdate:` for a `UIDropProposal` on every move;
//! the app's handler runs inside it (UIKit calls on the main thread, outside
//! the app's handler), so the answer is the app's verdict on that very move
//! (copy, or forbidden).
//!
//! The data loads after the drop, off the main thread: text and links
//! through `loadObjectOfClass:completionHandler:` (`NSString`, `NSURL`),
//! files through `loadFileRepresentationForTypeIdentifier:completionHandler:`,
//! whose file only lasts while its block runs, so the block copies it into a
//! directory of the drop's own under the app's temporary directory, under
//! the item's `suggestedName`. Once every item has answered, the drop is
//! delivered on the main queue. The completion blocks are hand-built block
//! literals, global so UIKit's `Block_copy` keeps them as they are; each is
//! leaked (a few dozen bytes per dropped item).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    CgPoint, Class, Id, Sel, call_handler, class, class_addMethod, class_addProtocol,
    current_window, dispatch_async_f, main_queue, msg_id, msg_id_usize, msg_usize, nsstring,
    objc_allocateClassPair, objc_getClass, objc_getProtocol, objc_msgSend, objc_registerClassPair,
    sel, string_of,
};
use crate::geometry::Point;
use crate::platform::backend::mobile_drop::{self, Item, Read};
use crate::platform::event::Event;

/// `UIDropOperation`.
const DROP_FORBIDDEN: usize = 1;
const DROP_COPY: usize = 2;

thread_local! {
    /// The app's verdict on the drag where it last was.
    static ACCEPTED: Cell<bool> = const { Cell::new(false) };
    /// The app was told a drag entered (and not yet that it left or
    /// dropped), and where it last was.
    static OVER: RefCell<Option<Point>> = const { RefCell::new(None) };
    /// The delegate, kept for the process (the interaction holds it weakly).
    static DELEGATE: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
}

/// `Window::set_drop_accepted`.
pub(super) fn set_accepted(accepted: bool) {
    ACCEPTED.with(|a| a.set(accepted));
}

/// Give `view` a `UIDropInteraction` (iOS 11 and later).
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn install(view: Id) {
    unsafe {
        let interaction_class = class("UIDropInteraction");
        if interaction_class.is_null() {
            return;
        }
        let delegate = msg_id(msg_id(register_delegate(), sel("alloc")), sel("init"));
        DELEGATE.with(|d| d.set(delegate));
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let interaction = init(
            msg_id(interaction_class, sel("alloc")),
            sel("initWithDelegate:"),
            delegate,
        );
        let add: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        add(view, sel("addInteraction:"), interaction);
    }
}

/// The delegate, for self-tests.
pub(super) fn delegate() -> Id {
    DELEGATE.with(|d| d.get())
}

fn register_delegate() -> Class {
    let name = c"StippleDropDelegate";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        let void = c"v@:@@";
        class_addMethod(
            cls,
            sel("dropInteraction:canHandleSession:"),
            can_handle as *const c_void,
            c"B@:@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dropInteraction:sessionDidEnter:"),
            did_enter as *const c_void,
            void.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dropInteraction:sessionDidUpdate:"),
            did_update as *const c_void,
            c"@@:@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dropInteraction:sessionDidExit:"),
            did_exit as *const c_void,
            void.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dropInteraction:performDrop:"),
            perform_drop as *const c_void,
            void.as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dropInteraction:sessionDidEnd:"),
            did_end as *const c_void,
            void.as_ptr(),
        );
        let protocol = objc_getProtocol(c"UIDropInteractionDelegate".as_ptr());
        if !protocol.is_null() {
            class_addProtocol(cls, protocol);
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// The `NSItemProvider` of each of the session's items.
unsafe fn providers(session: Id) -> Vec<Id> {
    unsafe {
        let items = msg_id(session, sel("items"));
        if items.is_null() {
            return Vec::new();
        }
        (0..msg_usize(items, sel("count")))
            .map(|i| {
                msg_id(
                    msg_id_usize(items, sel("objectAtIndex:"), i),
                    sel("itemProvider"),
                )
            })
            .filter(|p| !p.is_null())
            .collect()
    }
}

/// Whether `provider` has an item conforming to UTI `ty`.
unsafe fn conforms(provider: Id, ty: &str) -> bool {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(
            provider,
            sel("hasItemConformingToTypeIdentifier:"),
            nsstring(ty),
        ) != 0
    }
}

/// `provider`'s registered type identifiers, best first.
unsafe fn types(provider: Id) -> Vec<String> {
    unsafe {
        let types = msg_id(provider, sel("registeredTypeIdentifiers"));
        if types.is_null() {
            return Vec::new();
        }
        (0..msg_usize(types, sel("count")))
            .map(|i| string_of(msg_id_usize(types, sel("objectAtIndex:"), i)))
            .collect()
    }
}

/// How to read `provider`.
unsafe fn read_of(provider: Id) -> Option<Read> {
    unsafe {
        mobile_drop::ios_read(mobile_drop::IosItem {
            image: conforms(provider, "public.image"),
            text: conforms(provider, "public.plain-text"),
            url: conforms(provider, "public.url"),
            file_url: conforms(provider, "public.file-url"),
            named: !msg_id(provider, sel("suggestedName")).is_null(),
            typed: !types(provider).is_empty(),
        })
    }
}

/// The session's position in `view` (the app's view), in points.
unsafe fn location(session: Id) -> Point {
    let view = super::app_view();
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = f(session, sel("locationInView:"), view);
        Point::new(p.x, p.y)
    }
}

extern "C" fn can_handle(_this: Id, _cmd: Sel, _interaction: Id, session: Id) -> bool {
    unsafe { providers(session).into_iter().any(|p| read_of(p).is_some()) }
}

extern "C" fn did_enter(_this: Id, _cmd: Sel, _interaction: Id, session: Id) {
    let offer = unsafe {
        mobile_drop::offer_from_reads(providers(session).into_iter().filter_map(|p| read_of(p)))
    };
    let position = unsafe { location(session) };
    // Until the app answers, take anything that can be read.
    set_accepted(!offer.is_empty());
    OVER.with(|o| *o.borrow_mut() = Some(position));
    call_handler(Event::DragEnter { position, offer }, &current_window());
}

extern "C" fn did_update(_this: Id, _cmd: Sel, _interaction: Id, session: Id) -> Id {
    let position = unsafe { location(session) };
    let moved = OVER.with(|o| {
        let mut o = o.borrow_mut();
        let moved = *o != Some(position);
        *o = Some(position);
        moved
    });
    if moved {
        call_handler(Event::DragOver { position }, &current_window());
    }
    let operation = if ACCEPTED.with(|a| a.get()) {
        DROP_COPY
    } else {
        DROP_FORBIDDEN
    };
    unsafe {
        let init: unsafe extern "C" fn(Id, Sel, usize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let proposal = init(
            msg_id(class("UIDropProposal"), sel("alloc")),
            sel("initWithDropOperation:"),
            operation,
        );
        msg_id(proposal, sel("autorelease"))
    }
}

extern "C" fn did_exit(_this: Id, _cmd: Sel, _interaction: Id, _session: Id) {
    if OVER.with(|o| o.borrow_mut().take()).is_some() {
        call_handler(Event::DragLeave, &current_window());
    }
}

extern "C" fn did_end(_this: Id, _cmd: Sel, _interaction: Id, _session: Id) {
    // Ended over the view without a drop (cancelled).
    did_exit(_this, _cmd, _interaction, _session);
}

// ---- Reading the drop ------------------------------------------------------------

/// A drop whose items are loading.
struct Pending {
    id: u64,
    position: Point,
    /// Each item, once it answered (`Some(None)`: it couldn't be read).
    items: Vec<Option<Option<Item>>>,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static DROPS: AtomicU64 = AtomicU64::new(1);

extern "C" fn perform_drop(_this: Id, _cmd: Sel, _interaction: Id, session: Id) {
    let position = unsafe { location(session) };
    OVER.with(|o| *o.borrow_mut() = None);
    let reads: Vec<(Id, Read)> = unsafe {
        providers(session)
            .into_iter()
            .filter_map(|p| read_of(p).map(|r| (p, r)))
            .collect()
    };
    if reads.is_empty() {
        call_handler(Event::DragLeave, &current_window());
        return;
    }
    let id = DROPS.fetch_add(1, Ordering::Relaxed);
    // The drop's own directory, made only if a file comes.
    let dir = std::env::temp_dir();
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Pending {
            id,
            position,
            items: vec![None; reads.len()],
        });
    for (index, (provider, read)) in reads.into_iter().enumerate() {
        let (ty, name) = unsafe {
            let types = types(provider);
            let ty = types
                .iter()
                .find(|t| {
                    !matches!(
                        t.as_str(),
                        "public.url"
                            | "public.file-url"
                            | "public.plain-text"
                            | "public.utf8-plain-text"
                            | "public.text"
                    )
                })
                .or(types.first())
                .cloned()
                .unwrap_or_default();
            let name = msg_id(provider, sel("suggestedName"));
            let name = (!name.is_null()).then(|| string_of(name));
            crate::platform::window::debug_window(format_args!(
                "drop: item {index} read as {read:?}, types {types:?}, loading {ty:?}, suggested name {name:?}"
            ));
            (ty, name)
        };
        let block = Box::leak(Box::new(LoadBlock {
            isa: &raw const _NSConcreteGlobalBlock,
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke: loaded,
            descriptor: &DESCRIPTOR,
            drop_id: id,
            index,
            read,
            ty: ty.clone(),
            name,
            dir: dir.clone(),
        }));
        unsafe {
            let load: unsafe extern "C" fn(Id, Sel, Id, *mut LoadBlock) -> Id =
                std::mem::transmute(objc_msgSend as *const c_void);
            match read {
                Read::Text => load(
                    provider,
                    sel("loadObjectOfClass:completionHandler:"),
                    class("NSString"),
                    block,
                ),
                Read::Url => load(
                    provider,
                    sel("loadObjectOfClass:completionHandler:"),
                    class("NSURL"),
                    block,
                ),
                Read::File => load(
                    provider,
                    sel("loadFileRepresentationForTypeIdentifier:completionHandler:"),
                    nsstring(&ty),
                    block,
                ),
            };
        }
    }
}

unsafe extern "C" {
    static _NSConcreteGlobalBlock: c_void;
}

const BLOCK_IS_GLOBAL: i32 = 1 << 28;

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A completion block taking `(id object, NSError *error)`, with what it
/// needs to know after the block literal's fields.
#[repr(C)]
struct LoadBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut LoadBlock, Id, Id),
    descriptor: *const BlockDescriptor,
    drop_id: u64,
    index: usize,
    read: Read,
    ty: String,
    name: Option<String>,
    dir: std::path::PathBuf,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<LoadBlock>(),
};

/// A provider answered, on a queue of UIKit's: read what it gave (copying a
/// file while it lasts), and finish the drop on the main queue once every
/// item has.
unsafe extern "C" fn loaded(block: *mut LoadBlock, object: Id, _error: Id) {
    let b = unsafe { &*block };
    let item = if object.is_null() {
        None
    } else {
        match b.read {
            Read::Text => Some(Item::Text(unsafe { string_of(object) })),
            Read::Url => Some(Item::Text(unsafe {
                string_of(msg_id(object, sel("absoluteString")))
            })),
            Read::File => unsafe { copy_file(b, object) },
        }
    };
    let done = {
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(p) = pending.iter_mut().find(|p| p.id == b.drop_id) else {
            return;
        };
        if let Some(slot) = p.items.get_mut(b.index) {
            *slot = Some(item);
        }
        p.items.iter().all(Option::is_some)
    };
    if done {
        unsafe {
            dispatch_async_f(main_queue(), b.drop_id as usize as *mut c_void, finish);
        }
    }
}

/// Copy the file a provider handed over (an `NSURL`, valid while its block
/// runs) into the drop's directory.
unsafe fn copy_file(b: &LoadBlock, url: Id) -> Option<Item> {
    let from = std::path::PathBuf::from(unsafe { string_of(msg_id(url, sel("path"))) });
    let fallback = from
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = mobile_drop::file_name(b.name.as_deref(), &fallback, Some(&b.ty));
    let dir = mobile_drop::drop_dir(&b.dir, b.drop_id).ok()?;
    let to = mobile_drop::unique_path(&dir, &name);
    crate::platform::window::debug_window(format_args!(
        "drop: copying {} to {}",
        from.display(),
        to.display()
    ));
    match std::fs::copy(&from, &to) {
        Ok(_) => Some(Item::File(to)),
        Err(e) => {
            crate::platform::window::debug_window(format_args!(
                "drop: can't copy {}: {e}",
                from.display()
            ));
            None
        }
    }
}

/// On the main queue: deliver drop `id` (its items all read).
extern "C" fn finish(context: *mut c_void) {
    let id = context as usize as u64;
    let pending = {
        let mut all = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(i) = all.iter().position(|p| p.id == id) else {
            return;
        };
        all.remove(i)
    };
    let items = pending.items.into_iter().flatten().flatten().collect();
    let event = match mobile_drop::merge(items) {
        Some(data) => Event::Drop {
            position: pending.position,
            data,
        },
        None => Event::DragLeave,
    };
    call_handler(event, &current_window());
}

// ---- Self-tests ------------------------------------------------------------------

thread_local! {
    /// The stand-in session's items and position.
    static TEST_SESSION: RefCell<(Id, CgPoint)> =
        const { RefCell::new((std::ptr::null_mut(), CgPoint { x: 0.0, y: 0.0 })) };
}

/// A stand-in `UIDropSession` for self-tests: real `UIDragItem`s with real
/// `NSItemProvider`s, at a position set by hand.
fn test_session_class() -> Class {
    let name = c"StippleTestDropSession";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        extern "C" fn items(_this: Id, _cmd: Sel) -> Id {
            TEST_SESSION.with(|s| s.borrow().0)
        }
        extern "C" fn location_in_view(_this: Id, _cmd: Sel, _view: Id) -> CgPoint {
            TEST_SESSION.with(|s| s.borrow().1)
        }
        class_addMethod(cls, sel("items"), items as *const c_void, c"@@:".as_ptr());
        class_addMethod(
            cls,
            sel("locationInView:"),
            location_in_view as *const c_void,
            c"{CGPoint=dd}@:@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

/// For self-tests: drag `text` or the file at `path` over the view as
/// another app would, entering at `from` and dropping at `to` (points):
/// the delegate's methods called in UIKit's order with a stand-in session
/// whose item providers are real (`initWithObject:` an `NSString`,
/// `initWithContentsOfURL:` a file URL), so the data loads as a real drop's
/// does. `accept` is what the proposal for the last move said (copy or not),
/// read back from the delegate's answer.
///
/// # Safety
/// On the main thread, with the view installed.
pub(super) unsafe fn simulate(
    text: Option<&str>,
    path: Option<&str>,
    from: Point,
    to: Point,
) -> bool {
    unsafe {
        let delegate = delegate();
        if delegate.is_null() {
            return false;
        }
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let provider = match (text, path) {
            (Some(text), _) => init(
                msg_id(class("NSItemProvider"), sel("alloc")),
                sel("initWithObject:"),
                nsstring(text),
            ),
            (None, Some(path)) => {
                let url = init(class("NSURL"), sel("fileURLWithPath:"), nsstring(path));
                let provider = init(
                    msg_id(class("NSItemProvider"), sel("alloc")),
                    sel("initWithContentsOfURL:"),
                    url,
                );
                // Named after the file, as the Files app's drags are (a
                // provider made from a URL has no name of its own on some
                // iOS releases, and the file it loads is then "text.txt").
                if !provider.is_null() {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    super::msg_void_id(provider, sel("setSuggestedName:"), nsstring(&name));
                }
                provider
            }
            (None, None) => return false,
        };
        if provider.is_null() {
            return false;
        }
        let item = init(
            msg_id(class("UIDragItem"), sel("alloc")),
            sel("initWithItemProvider:"),
            provider,
        );
        let items = init(class("NSArray"), sel("arrayWithObject:"), item);
        TEST_SESSION.with(|s| {
            *s.borrow_mut() = (
                items,
                CgPoint {
                    x: from.x,
                    y: from.y,
                },
            )
        });
        let session = msg_id(msg_id(test_session_class(), sel("alloc")), sel("init"));
        let interaction = std::ptr::null_mut();
        let two: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let ask: unsafe extern "C" fn(Id, Sel, Id, Id) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        if ask(
            delegate,
            sel("dropInteraction:canHandleSession:"),
            interaction,
            session,
        ) == 0
        {
            return false;
        }
        two(
            delegate,
            sel("dropInteraction:sessionDidEnter:"),
            interaction,
            session,
        );
        let mut proposal = std::ptr::null_mut();
        for step in 1..=4 {
            let t = f64::from(step) / 4.0;
            TEST_SESSION.with(|s| {
                s.borrow_mut().1 = CgPoint {
                    x: from.x + (to.x - from.x) * t,
                    y: from.y + (to.y - from.y) * t,
                }
            });
            proposal = two(
                delegate,
                sel("dropInteraction:sessionDidUpdate:"),
                interaction,
                session,
            );
        }
        let copy = !proposal.is_null() && msg_usize(proposal, sel("operation")) == DROP_COPY;
        two(
            delegate,
            sel("dropInteraction:performDrop:"),
            interaction,
            session,
        );
        two(
            delegate,
            sel("dropInteraction:sessionDidEnd:"),
            interaction,
            session,
        );
        copy
    }
}
