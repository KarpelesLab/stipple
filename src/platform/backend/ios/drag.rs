//! Dragging out of the app (to another app beside it on iPad, between apps
//! on iPhone since iOS 15, and onto the app itself): a `UIDragInteraction`
//! on the view, enabled on iPhone too.
//!
//! UIKit recognizes the lift itself (a long press), then asks the delegate
//! for the items under the finger (`dragInteraction:itemsForBeginningSession:`).
//! The app answers inside that call: the delegate sends
//! [`Event::DragLift`], the App finds the drag source there and hands its
//! payload back through [`Window::lift_drag`](crate::platform::Window::lift_drag)
//! ([`lift`] here), and the delegate turns it into `UIDragItem`s:
//!
//! - a text is an `NSItemProvider` of an `NSString` (`initWithObject:`);
//! - each file is a provider with a file representation of the file's type
//!   (`registerFileRepresentationForTypeIdentifier:fileOptions:visibility:loadHandler:`,
//!   the type from `UTType` by the file's extension, `public.data` without
//!   one), whose load handler hands the file's URL over (the receiver gets
//!   a copy), named after the file (`suggestedName`).
//!
//! Nothing there (or a payload that can't leave the app) is no items, and
//! UIKit lets the lift go. The lifted element's picture (the App's render of
//! it) is the drag preview (`dragInteraction:previewForLiftingItem:session:`),
//! where the element is. Once the drag begins UIKit cancels the view's
//! touches; dropped on the app's own view it comes back through the drop
//! interaction ([`drop`](super::drop)) like any other app's drag.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use super::{
    CgPoint, CgRect, CgSize, Class, Id, Sel, call_handler, class, class_addMethod,
    class_addProtocol, current_window, msg_id, msg_id_usize, msg_usize, msg_void_bool, msg_void_id,
    nsstring, objc_allocateClassPair, objc_getClass, objc_getProtocol, objc_msgSend,
    objc_registerClassPair, sel, string_of,
};
use crate::geometry::Point;
use crate::platform::event::{DropData, Event};
use crate::platform::window::DragLift;

thread_local! {
    /// The interaction, once on the view.
    static INTERACTION: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
    /// The delegate, kept for the process (the interaction holds it weakly).
    static DELEGATE: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
    /// UIKit is asking for the items (the app's answer goes to [`LIFTED`]).
    static ASKING: Cell<bool> = const { Cell::new(false) };
    /// What the app lifted while UIKit asked.
    static LIFTED: RefCell<Option<DragLift>> = const { RefCell::new(None) };
    /// The lift being dragged, for its preview.
    static PREVIEW: RefCell<Option<DragLift>> = const { RefCell::new(None) };
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGDataProviderCreateWithCFData(data: Id) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
}

/// `kCGImageAlphaLast`: straight R, G, B, A, as a [`Pixmap`](crate::render::Pixmap) is.
const CG_RGBA_STRAIGHT: u32 = 3;

/// `Window::lift_drag`: while UIKit asks for the items, keep the app's
/// payload for them. Outside that (the App's own long press), the lift is
/// UIKit's to recognize, so it's taken all the same: the App then leaves
/// the finger alone, and UIKit asks when it lifts.
pub(super) fn lift(lift: &DragLift) -> bool {
    if INTERACTION.with(Cell::get).is_null() {
        return false;
    }
    if ASKING.with(Cell::get) {
        LIFTED.with(|l| *l.borrow_mut() = Some(lift.clone()));
    }
    true
}

/// Give `view` a `UIDragInteraction` (iOS 11 and later), enabled on iPhone
/// as well (where it's off by default).
///
/// # Safety
/// `view` must be the live `StippleUIView`.
pub(super) unsafe fn install(view: Id) {
    unsafe {
        let interaction_class = class("UIDragInteraction");
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
        if interaction.is_null() {
            return;
        }
        msg_void_bool(interaction, sel("setEnabled:"), true);
        msg_void_id(view, sel("addInteraction:"), interaction);
        INTERACTION.with(|i| i.set(interaction));
    }
}

fn register_delegate() -> Class {
    let name = c"StippleDragDelegate";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("dragInteraction:itemsForBeginningSession:"),
            items_for_beginning as *const c_void,
            c"@@:@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dragInteraction:previewForLiftingItem:session:"),
            preview_for_lifting as *const c_void,
            c"@@:@@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("dragInteraction:session:didEndWithOperation:"),
            did_end as *const c_void,
            c"v@:@@Q".as_ptr(),
        );
        let protocol = objc_getProtocol(c"UIDragInteractionDelegate".as_ptr());
        if !protocol.is_null() {
            class_addProtocol(cls, protocol);
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// The session's position in the app's view, in points.
unsafe fn location(session: Id) -> Point {
    let view = super::app_view();
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = f(session, sel("locationInView:"), view);
        Point::new(p.x, p.y)
    }
}

extern "C" fn items_for_beginning(_this: Id, _cmd: Sel, _interaction: Id, session: Id) -> Id {
    let position = unsafe { location(session) };
    LIFTED.with(|l| l.borrow_mut().take());
    ASKING.with(|a| a.set(true));
    call_handler(Event::DragLift { position }, &current_window());
    ASKING.with(|a| a.set(false));
    let lifted = LIFTED.with(|l| l.borrow_mut().take());
    let items = lifted
        .as_ref()
        .map(|l| unsafe { drag_items(&l.data) })
        .unwrap_or_default();
    crate::platform::window::debug_window(format_args!(
        "drag: lift at {position:?}: {} item(s)",
        items.len()
    ));
    if !items.is_empty() {
        PREVIEW.with(|p| *p.borrow_mut() = lifted);
    }
    unsafe { ns_array(&items) }
}

/// An `NSArray` of `items`.
unsafe fn ns_array(items: &[Id]) -> Id {
    unsafe {
        let array = msg_id(class("NSMutableArray"), sel("array"));
        for &item in items {
            msg_void_id(array, sel("addObject:"), item);
        }
        array
    }
}

/// The `UIDragItem`s carrying `data`: one for a text, one per file.
unsafe fn drag_items(data: &DropData) -> Vec<Id> {
    unsafe {
        let providers: Vec<Id> = match data {
            DropData::Text(text) => vec![text_provider(text)],
            DropData::Files(paths) => paths.iter().map(|p| file_provider(p)).collect(),
            DropData::Internal(_) => Vec::new(),
        };
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        providers
            .into_iter()
            .filter(|p| !p.is_null())
            .map(|p| {
                let item = init(
                    msg_id(class("UIDragItem"), sel("alloc")),
                    sel("initWithItemProvider:"),
                    p,
                );
                msg_id(item, sel("autorelease"))
            })
            .filter(|i| !i.is_null())
            .collect()
    }
}

/// An item provider of `text`, as an `NSString`.
unsafe fn text_provider(text: &str) -> Id {
    unsafe {
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let provider = init(
            msg_id(class("NSItemProvider"), sel("alloc")),
            sel("initWithObject:"),
            nsstring(text),
        );
        msg_id(provider, sel("autorelease"))
    }
}

/// The uniform type identifier of a file named `name`: `UTType`'s for its
/// extension (iOS 14 and later), else `public.data`.
unsafe fn file_type(name: &std::path::Path) -> String {
    let ext = name.extension().and_then(|e| e.to_str()).unwrap_or("");
    let ut = class("UTType");
    if ext.is_empty() || ut.is_null() {
        return "public.data".into();
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let ty = f(ut, sel("typeWithFilenameExtension:"), nsstring(ext));
        if ty.is_null() {
            return "public.data".into();
        }
        let id = string_of(msg_id(ty, sel("identifier")));
        if id.is_empty() {
            "public.data".into()
        } else {
            id
        }
    }
}

/// An item provider of the file at `path`: a file representation of its
/// type, which hands the file over, named after it.
unsafe fn file_provider(path: &std::path::Path) -> Id {
    unsafe {
        let make_url: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let url = make_url(
            class("NSURL"),
            sel("fileURLWithPath:"),
            nsstring(&path.to_string_lossy()),
        );
        if url.is_null() {
            return std::ptr::null_mut();
        }
        let provider = msg_id(msg_id(class("NSItemProvider"), sel("alloc")), sel("init"));
        if provider.is_null() {
            return provider;
        }
        // Kept by the block for the process (a few dozen bytes per file).
        let block = Box::leak(Box::new(FileBlock {
            isa: &raw const _NSConcreteGlobalBlock,
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke: hand_file_over,
            descriptor: &FILE_DESCRIPTOR,
            url: msg_id(url, sel("retain")),
        }));
        let register: unsafe extern "C" fn(Id, Sel, Id, isize, isize, *mut FileBlock) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let ty = file_type(path);
        register(
            provider,
            sel("registerFileRepresentationForTypeIdentifier:fileOptions:visibility:loadHandler:"),
            nsstring(&ty),
            0, // a copy, not opened in place
            0, // NSItemProviderRepresentationVisibilityAll
            block,
        );
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.is_empty() {
            msg_void_id(provider, sel("setSuggestedName:"), nsstring(&name));
        }
        crate::platform::window::debug_window(format_args!(
            "drag: file {} as {ty}",
            path.display()
        ));
        msg_id(provider, sel("autorelease"))
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

/// A load handler, `NSProgress *(^)(void (^)(NSURL *, BOOL, NSError *))`,
/// with the file's URL after the block literal's fields.
#[repr(C)]
struct FileBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut FileBlock, *mut CompletionBlock) -> Id,
    descriptor: *const BlockDescriptor,
    url: Id,
}

static FILE_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<FileBlock>(),
};

/// The completion block UIKit hands the load handler.
#[repr(C)]
struct CompletionBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut CompletionBlock, Id, i8, Id),
}

/// The load handler: hand the file over at once (not coordinated, so the
/// receiver gets a copy), with no progress to report.
unsafe extern "C" fn hand_file_over(block: *mut FileBlock, completion: *mut CompletionBlock) -> Id {
    unsafe {
        if !completion.is_null() {
            ((*completion).invoke)(completion, (*block).url, 0, std::ptr::null_mut());
        }
    }
    std::ptr::null_mut()
}

extern "C" fn preview_for_lifting(
    _this: Id,
    _cmd: Sel,
    _interaction: Id,
    _item: Id,
    _session: Id,
) -> Id {
    PREVIEW.with(|p| {
        p.borrow()
            .as_ref()
            .map_or(std::ptr::null_mut(), |l| unsafe { preview(l) })
    })
}

/// The lifted element's picture where it is in the view, as a
/// `UITargetedDragPreview` (`nil`, UIKit's default, without one).
unsafe fn preview(lift: &DragLift) -> Id {
    let Some(image) = lift.image.as_deref() else {
        return std::ptr::null_mut();
    };
    let size = image.size();
    let (w, h) = (size.width as usize, size.height as usize);
    if w == 0 || h == 0 || lift.source.width() <= 0.0 {
        return std::ptr::null_mut();
    }
    unsafe {
        let bytes = image.as_bytes();
        let with_bytes: unsafe extern "C" fn(Id, Sel, *const c_void, usize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let data = with_bytes(
            class("NSData"),
            sel("dataWithBytes:length:"),
            bytes.as_ptr() as *const c_void,
            bytes.len(),
        );
        let space = CGColorSpaceCreateDeviceRGB();
        let provider = CGDataProviderCreateWithCFData(data);
        let cg = CGImageCreate(
            w,
            h,
            8,
            32,
            w * 4,
            space,
            CG_RGBA_STRAIGHT,
            provider,
            std::ptr::null(),
            true,
            0,
        );
        CGDataProviderRelease(provider);
        CGColorSpaceRelease(space);
        if cg.is_null() {
            return std::ptr::null_mut();
        }
        let scale = w as f64 / lift.source.width();
        let make_image: unsafe extern "C" fn(Id, Sel, *mut c_void, f64, isize) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let ui_image = make_image(
            class("UIImage"),
            sel("imageWithCGImage:scale:orientation:"),
            cg,
            scale,
            0,
        );
        CGImageRelease(cg);
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let image_view = init(
            msg_id(class("UIImageView"), sel("alloc")),
            sel("initWithImage:"),
            ui_image,
        );
        let set_frame: unsafe extern "C" fn(Id, Sel, CgRect) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set_frame(
            image_view,
            sel("setFrame:"),
            CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize {
                    width: lift.source.width(),
                    height: lift.source.height(),
                },
            },
        );
        let parameters = msg_id(
            msg_id(class("UIDragPreviewParameters"), sel("alloc")),
            sel("init"),
        );
        let center = lift.source.center();
        let make_target: unsafe extern "C" fn(Id, Sel, Id, CgPoint) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let target = make_target(
            msg_id(class("UIDragPreviewTarget"), sel("alloc")),
            sel("initWithContainer:center:"),
            super::app_view(),
            CgPoint {
                x: center.x,
                y: center.y,
            },
        );
        let make_preview: unsafe extern "C" fn(Id, Sel, Id, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let preview = make_preview(
            msg_id(class("UITargetedDragPreview"), sel("alloc")),
            sel("initWithView:parameters:target:"),
            image_view,
            parameters,
            target,
        );
        for o in [image_view, parameters, target] {
            if !o.is_null() {
                super::msg_void(o, sel("release"));
            }
        }
        if preview.is_null() {
            preview
        } else {
            msg_id(preview, sel("autorelease"))
        }
    }
}

extern "C" fn did_end(_this: Id, _cmd: Sel, _interaction: Id, _session: Id, operation: usize) {
    crate::platform::window::debug_window(format_args!(
        "drag: session ended (operation {operation})"
    ));
    PREVIEW.with(|p| p.borrow_mut().take());
}

// ---- Self-tests ------------------------------------------------------------------

thread_local! {
    static TEST_AT: Cell<CgPoint> = const { Cell::new(CgPoint { x: 0.0, y: 0.0 }) };
}

/// A stand-in `UIDragSession` for self-tests: only its location.
fn test_session_class() -> Class {
    let name = c"StippleTestDragSession";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        extern "C" fn location_in_view(_this: Id, _cmd: Sel, _view: Id) -> CgPoint {
            TEST_AT.with(Cell::get)
        }
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

/// For self-tests: lift at `at` (points) as UIKit does once its long press
/// is recognized, asking the delegate for the items with a stand-in session
/// there. Returns the items (an `NSArray` of `UIDragItem`s, autoreleased)
/// and a description of each provider: its registered types, and its
/// suggested name.
///
/// # Safety
/// On the main thread, inside an autorelease pool, with the view installed.
pub(super) unsafe fn simulate_lift(at: Point) -> (Id, String) {
    unsafe {
        let delegate = DELEGATE.with(Cell::get);
        if delegate.is_null() {
            return (std::ptr::null_mut(), "no drag interaction".into());
        }
        TEST_AT.with(|t| t.set(CgPoint { x: at.x, y: at.y }));
        let session = msg_id(msg_id(test_session_class(), sel("alloc")), sel("init"));
        let ask: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let items = ask(
            delegate,
            sel("dragInteraction:itemsForBeginningSession:"),
            INTERACTION.with(Cell::get),
            session,
        );
        let mut described = Vec::new();
        let count = if items.is_null() {
            0
        } else {
            msg_usize(items, sel("count"))
        };
        for i in 0..count {
            let provider = msg_id(
                msg_id_usize(items, sel("objectAtIndex:"), i),
                sel("itemProvider"),
            );
            let types = msg_id(provider, sel("registeredTypeIdentifiers"));
            let types: Vec<String> = (0..msg_usize(types, sel("count")))
                .map(|j| string_of(msg_id_usize(types, sel("objectAtIndex:"), j)))
                .collect();
            let name = msg_id(provider, sel("suggestedName"));
            let name = (!name.is_null()).then(|| string_of(name));
            described.push(format!("{types:?} name={name:?}"));
        }
        let preview = PREVIEW.with(|p| {
            p.borrow()
                .as_ref()
                .map(|l| !preview(l).is_null())
                .unwrap_or(false)
        });
        PREVIEW.with(|p| p.borrow_mut().take());
        (
            items,
            format!("{count} item(s) {} preview={preview}", described.join(", ")),
        )
    }
}

/// For self-tests: whether the interaction is on the view and enabled.
pub(super) fn status() -> String {
    let interaction = INTERACTION.with(Cell::get);
    if interaction.is_null() {
        return "drag_interaction=false".into();
    }
    unsafe {
        let enabled = super::msg_bool(interaction, sel("isEnabled"));
        let view = msg_id(interaction, sel("view"));
        format!(
            "drag_interaction={} enabled={enabled}",
            view == super::app_view()
        )
    }
}
