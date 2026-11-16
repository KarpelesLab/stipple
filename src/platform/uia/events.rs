//! The UIA events a tree swap raises, beyond the focus change: property
//! changes (a label's Name, a field's Value, a checkbox's ToggleState, an
//! item's expanded and selected state, a container's scroll percent),
//! `ChildrenInvalidated` structure changes where children came or went,
//! `SelectionItem_ElementSelected`, and a notification for each alert (a
//! toast) that appeared, which Narrator and NVDA read out. What changed comes
//! from `uia_logic::events`; this raises it on the new tree's providers.

use super::patterns::bstr;
use super::*;
use crate::platform::uia_logic::{UiaEvent, Val};

const UIA_SelectionItem_ElementSelectedEventId: i32 = 20012;
/// `StructureChangeType_ChildrenInvalidated`.
const ChildrenInvalidated: i32 = 2;
/// `NotificationKind_Other`: a toast isn't the outcome of a call.
const NotificationKind_Other: i32 = 4;
/// `NotificationProcessing_ImportantAll`: read every one, even while
/// something else is being read.
const NotificationProcessing_ImportantAll: i32 = 0;

#[link(name = "uiautomationcore")]
unsafe extern "system" {
    fn UiaRaiseAutomationPropertyChangedEvent(
        provider: *mut c_void,
        id: i32,
        old: Variant,
        new: Variant,
    ) -> Hresult;
    fn UiaRaiseStructureChangedEvent(
        provider: *mut c_void,
        kind: i32,
        runtime_id: *mut i32,
        len: i32,
    ) -> Hresult;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

/// `UiaRaiseNotificationEvent` (Windows 10 1709 on): looked up at run time,
/// so the program still starts on older systems, which just don't announce.
type RaiseNotification =
    unsafe extern "system" fn(*mut c_void, i32, i32, *mut u16, *mut u16) -> Hresult;

fn raise_notification_fn() -> Option<RaiseNotification> {
    static FN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let f = *FN.get_or_init(|| unsafe {
        let name: Vec<u16> = "uiautomationcore.dll".encode_utf16().chain([0]).collect();
        let mut module = GetModuleHandleW(name.as_ptr());
        if module.is_null() {
            module = LoadLibraryW(name.as_ptr());
        }
        if module.is_null() {
            return 0;
        }
        GetProcAddress(module, c"UiaRaiseNotificationEvent".as_ptr().cast::<u8>()) as usize
    });
    (f != 0).then(|| unsafe { core::mem::transmute::<usize, RaiseNotification>(f) })
}

/// The events of a tree swap, with the providers (of the new tree) they're
/// raised on.
pub(crate) struct PlannedEvents(Vec<(*mut Provider, UiaEvent)>);

/// Plan the events going from `old` to `new`, on `tree` (built from `new`).
/// Nothing for the first tree, which a client reads whole.
pub(crate) fn plan_events(tree: &UiaTree, old: Option<&A11yNode>, new: &A11yNode) -> PlannedEvents {
    let Some(old) = old else {
        return PlannedEvents(Vec::new());
    };
    let planned = logic::events(old, new)
        .into_iter()
        .filter_map(|e| {
            let path = match &e {
                UiaEvent::Property { path, .. }
                | UiaEvent::ChildrenInvalidated { path }
                | UiaEvent::ElementSelected { path }
                | UiaEvent::Notification { path, .. } => path,
            };
            let p = tree.provider_at(path) as *mut Provider;
            (!p.is_null()).then_some((p, e))
        })
        .collect();
    PlannedEvents(planned)
}

/// A VARIANT holding `v` (a BSTR the caller frees with [`free`]).
fn variant(v: &Val) -> Variant {
    let mut out = Variant::empty();
    match v {
        Val::Bool(b) => {
            out.vt = VT_BOOL;
            out.val = if *b { VARIANT_TRUE } else { 0 };
        }
        Val::I4(n) => {
            out.vt = VT_I4;
            out.val = *n as u32 as u64;
        }
        Val::R8(x) => {
            out.vt = VT_R8;
            out.val = x.to_bits();
        }
        Val::Str(s) => {
            out.vt = VT_BSTR;
            out.val = bstr(s) as u64;
        }
    }
    out
}

fn free(v: &Variant) {
    if v.vt == VT_BSTR {
        unsafe { SysFreeString(v.val as *mut u16) };
    }
}

/// Raise `planned`, if a client listens. Call it once the tree they're on
/// is the window's current one, on the window's thread, outside any borrow
/// of the window's state (UIA may call the providers back from inside).
///
/// # Safety
///
/// The tree the events were planned on is alive.
pub(crate) unsafe fn raise_planned(planned: PlannedEvents) {
    if planned.0.is_empty() || unsafe { UiaClientsAreListening() } == 0 {
        return;
    }
    for (p, e) in planned.0 {
        let simple = p as *mut c_void;
        unsafe {
            match e {
                UiaEvent::Property {
                    property, old, new, ..
                } => {
                    // UIA copies the values; the strings are ours to free.
                    let (a, b) = (variant(&old), variant(&new));
                    UiaRaiseAutomationPropertyChangedEvent(simple, property, a, b);
                    free(&a);
                    free(&b);
                }
                UiaEvent::ChildrenInvalidated { .. } => {
                    let mut id = [UiaAppendRuntimeId, (*p).runtime_id];
                    UiaRaiseStructureChangedEvent(simple, ChildrenInvalidated, id.as_mut_ptr(), 2);
                }
                UiaEvent::ElementSelected { .. } => {
                    UiaRaiseAutomationEvent(simple, UIA_SelectionItem_ElementSelectedEventId);
                }
                UiaEvent::Notification { text, .. } => {
                    if let Some(raise) = raise_notification_fn() {
                        let (display, activity) = (bstr(&text), bstr("stipple-toast"));
                        raise(
                            simple,
                            NotificationKind_Other,
                            NotificationProcessing_ImportantAll,
                            display,
                            activity,
                        );
                        SysFreeString(display);
                        SysFreeString(activity);
                    }
                }
            }
        }
    }
}
