//! Windows message boxes: `TaskDialogIndirect`, with common controls 6
//! activated for the call (an executable without a manifest gets version 5,
//! which has no task dialog), else `MessageBoxW` when the buttons are one of
//! its standard sets. Either runs on a thread of its own, owned by the window
//! active when it was asked for, so the app's event loop keeps running (the
//! owner is disabled while the dialog is up, as for any modal dialog).

use super::{MessageBox, MessageDone, MessageKind, MessageResponse};
use core::ffi::c_void;
use std::sync::Mutex;

type Hwnd = *mut c_void;
type Hresult = i32;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetActiveWindow() -> Hwnd;
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, kind: u32) -> i32;
    fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateActCtxW(ctx: *const ActCtxW) -> *mut c_void;
    fn ActivateActCtx(ctx: *mut c_void, cookie: *mut usize) -> i32;
    fn DeactivateActCtx(flags: u32, cookie: usize) -> i32;
    fn ReleaseActCtx(ctx: *mut c_void);
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

/// `ACTCTXW`.
#[repr(C)]
struct ActCtxW {
    size: u32,
    flags: u32,
    source: *const u16,
    processor_architecture: u16,
    lang_id: u16,
    assembly_directory: *const u16,
    resource_name: *const u16,
    application_name: *const u16,
    module: *mut c_void,
}

const ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID: u32 = 0x4;
const ACTCTX_FLAG_RESOURCE_NAME_VALID: u32 = 0x8;
/// shell32.dll's manifest resource that asks for common controls 6 (what
/// `EnableVisualStyles`-style code activates for a manifest-less module).
const SHELL32_COMCTL6_MANIFEST: usize = 124;

/// `TASKDIALOG_BUTTON` (commctrl.h packs the task dialog structures).
#[repr(C, packed(1))]
struct TaskDialogButton {
    id: i32,
    text: *const u16,
}

/// `TASKDIALOGCONFIG`.
#[repr(C, packed(1))]
struct TaskDialogConfig {
    size: u32,
    parent: Hwnd,
    instance: *mut c_void,
    flags: i32,
    common_buttons: i32,
    window_title: *const u16,
    main_icon: *const u16,
    main_instruction: *const u16,
    content: *const u16,
    button_count: u32,
    buttons: *const TaskDialogButton,
    default_button: i32,
    radio_count: u32,
    radios: *const TaskDialogButton,
    default_radio: i32,
    verification_text: *const u16,
    expanded_information: *const u16,
    expanded_control_text: *const u16,
    collapsed_control_text: *const u16,
    footer_icon: *const u16,
    footer: *const u16,
    callback: Option<TaskDialogCallback>,
    callback_data: isize,
    width: u32,
}

type TaskDialogCallback = unsafe extern "system" fn(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    data: isize,
) -> Hresult;
type TaskDialogIndirectFn = unsafe extern "system" fn(
    config: *const TaskDialogConfig,
    button: *mut i32,
    radio: *mut i32,
    verified: *mut i32,
) -> Hresult;

const TDF_ALLOW_DIALOG_CANCELLATION: i32 = 0x0008;
const TDF_POSITION_RELATIVE_TO_WINDOW: i32 = 0x1000;
const TDN_CREATED: u32 = 0;
const TDN_DESTROYED: u32 = 5;
const TDM_CLICK_BUTTON: u32 = 0x0400 + 102;
const WM_COMMAND: u32 = 0x0111;
const IDCANCEL: i32 = 2;
/// Our buttons' ids: `BUTTON_ID + index`.
const BUTTON_ID: i32 = 100;

// Task dialog icons (`MAKEINTRESOURCEW(-1)`…).
const TD_WARNING_ICON: usize = 0xFFFF;
const TD_ERROR_ICON: usize = 0xFFFE;
const TD_INFORMATION_ICON: usize = 0xFFFD;

// MessageBoxW types and results.
const MB_ICONERROR: u32 = 0x10;
const MB_ICONQUESTION: u32 = 0x20;
const MB_ICONWARNING: u32 = 0x30;
const MB_ICONINFORMATION: u32 = 0x40;
const MB_DEFBUTTON2: u32 = 0x100;
const MB_DEFBUTTON3: u32 = 0x200;
const MB_SETFOREGROUND: u32 = 0x10000;

/// The dialog up now: the task dialog's window, or how to find a message
/// box's, and its buttons' command ids.
enum Current {
    Task(usize),
    Box { caption: Vec<u16>, ids: Vec<i32> },
}

static CURRENT: Mutex<Option<Current>> = Mutex::new(None);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A null-terminated copy of `s`, or null when it's empty.
fn wide_or_null(s: &str, keep: &mut Vec<Vec<u16>>) -> *const u16 {
    if s.is_empty() {
        return std::ptr::null();
    }
    keep.push(wide(s));
    keep.last().map_or(std::ptr::null(), |w| w.as_ptr())
}

/// The window title: the one given, else the executable's name.
fn caption(mb: &MessageBox) -> String {
    if !mb.title.is_empty() {
        return mb.title.clone();
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Message".into())
}

/// The standard `MessageBoxW` set for `labels`, as its type and the command
/// id of each button.
fn standard_set(labels: &[String]) -> Option<(u32, Vec<i32>)> {
    let plain: Vec<String> = labels
        .iter()
        .map(|l| l.replace('&', "").to_lowercase())
        .collect();
    let plain: Vec<&str> = plain.iter().map(String::as_str).collect();
    Some(match plain.as_slice() {
        ["ok"] => (0, vec![1]),
        ["ok", "cancel"] => (1, vec![1, 2]),
        ["abort", "retry", "ignore"] => (2, vec![3, 4, 5]),
        ["yes", "no", "cancel"] => (3, vec![6, 7, 2]),
        ["yes", "no"] => (4, vec![6, 7]),
        ["retry", "cancel"] => (5, vec![4, 2]),
        _ => return None,
    })
}

pub(super) fn show(mb: &MessageBox, done: MessageDone) -> Result<(), MessageDone> {
    if mb.input.is_some() {
        return Err(done);
    }
    let labels = mb.button_labels();
    let task = task_dialog_available();
    if !task && standard_set(&labels).is_none() {
        return Err(done);
    }
    let owner = unsafe { GetActiveWindow() } as usize;
    let mb = mb.clone();
    let spawned = std::thread::Builder::new()
        .name("stipple-message".into())
        .spawn(move || {
            let button = if task {
                run_task_dialog(&mb, owner as Hwnd)
            } else {
                run_message_box(&mb, owner as Hwnd)
            };
            done(MessageResponse {
                button,
                input: None,
            });
        });
    // A thread that couldn't start drops `done`; the box then never answers,
    // which beats blocking the event loop.
    if let Err(e) = spawned {
        super::debug(format_args!("message box thread: {e}"));
    }
    Ok(())
}

/// Activate common controls 6 on this thread (through shell32's manifest)
/// and find `TaskDialogIndirect`. The context stays active (it's per
/// thread, and this thread only shows the dialog).
fn task_dialog() -> Option<TaskDialogIndirectFn> {
    unsafe {
        let mut dir = vec![0u16; 260];
        let n = GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) as usize;
        if n == 0 || n >= dir.len() {
            return None;
        }
        dir.truncate(n);
        let mut source = dir.clone();
        source.extend("\\shell32.dll".encode_utf16());
        source.push(0);
        dir.push(0);
        let ctx = ActCtxW {
            size: std::mem::size_of::<ActCtxW>() as u32,
            flags: ACTCTX_FLAG_RESOURCE_NAME_VALID | ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID,
            source: source.as_ptr(),
            processor_architecture: 0,
            lang_id: 0,
            assembly_directory: dir.as_ptr(),
            resource_name: SHELL32_COMCTL6_MANIFEST as *const u16,
            application_name: std::ptr::null(),
            module: std::ptr::null_mut(),
        };
        let handle = CreateActCtxW(&ctx);
        let mut cookie = 0usize;
        let activated =
            !handle.is_null() && handle as isize != -1 && ActivateActCtx(handle, &mut cookie) != 0;
        let comctl = LoadLibraryW(wide("comctl32.dll").as_ptr());
        let f = if comctl.is_null() {
            std::ptr::null()
        } else {
            GetProcAddress(comctl, c"TaskDialogIndirect".as_ptr().cast())
        };
        if f.is_null() {
            if activated {
                DeactivateActCtx(0, cookie);
            }
            if !handle.is_null() && handle as isize != -1 {
                ReleaseActCtx(handle);
            }
            return None;
        }
        Some(std::mem::transmute::<*const c_void, TaskDialogIndirectFn>(
            f,
        ))
    }
}

/// Whether this process can show a task dialog (checked once, on a
/// throwaway thread, so the UI thread's activation context is left alone).
fn task_dialog_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::thread::spawn(|| task_dialog().is_some())
            .join()
            .unwrap_or(false)
    })
}

unsafe extern "system" fn task_callback(
    hwnd: Hwnd,
    msg: u32,
    _wparam: usize,
    _lparam: isize,
    _data: isize,
) -> Hresult {
    match msg {
        TDN_CREATED => {
            if let Ok(mut c) = CURRENT.lock() {
                *c = Some(Current::Task(hwnd as usize));
            }
        }
        TDN_DESTROYED => {
            if let Ok(mut c) = CURRENT.lock() {
                *c = None;
            }
        }
        _ => {}
    }
    0
}

fn run_task_dialog(mb: &MessageBox, owner: Hwnd) -> Option<usize> {
    let f = task_dialog()?;
    let labels = mb.button_labels();
    let mut keep = Vec::new();
    let texts: Vec<Vec<u16>> = labels.iter().map(|l| wide(l)).collect();
    let buttons: Vec<TaskDialogButton> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| TaskDialogButton {
            id: BUTTON_ID + i as i32,
            text: t.as_ptr(),
        })
        .collect();
    let icon = match mb.kind {
        MessageKind::Warning => TD_WARNING_ICON,
        MessageKind::Error => TD_ERROR_ICON,
        MessageKind::Info | MessageKind::Question => TD_INFORMATION_ICON,
    };
    let cancel = mb.cancel_index();
    let mut flags = TDF_POSITION_RELATIVE_TO_WINDOW;
    if cancel.is_some() {
        flags |= TDF_ALLOW_DIALOG_CANCELLATION;
    }
    let title = caption(mb);
    let config = TaskDialogConfig {
        size: std::mem::size_of::<TaskDialogConfig>() as u32,
        parent: owner,
        instance: std::ptr::null_mut(),
        flags,
        common_buttons: 0,
        window_title: wide_or_null(&title, &mut keep),
        main_icon: icon as *const u16,
        main_instruction: wide_or_null(&mb.text, &mut keep),
        content: wide_or_null(&mb.detail, &mut keep),
        button_count: buttons.len() as u32,
        buttons: buttons.as_ptr(),
        default_button: BUTTON_ID + mb.default_index() as i32,
        radio_count: 0,
        radios: std::ptr::null(),
        default_radio: 0,
        verification_text: std::ptr::null(),
        expanded_information: std::ptr::null(),
        expanded_control_text: std::ptr::null(),
        collapsed_control_text: std::ptr::null(),
        footer_icon: std::ptr::null(),
        footer: std::ptr::null(),
        callback: Some(task_callback),
        callback_data: 0,
        width: 0,
    };
    let mut pressed = 0i32;
    let hr = unsafe {
        f(
            &config,
            &mut pressed,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if let Ok(mut c) = CURRENT.lock() {
        *c = None;
    }
    super::debug(format_args!(
        "message box: TaskDialogIndirect hr={hr:#x} button={pressed}"
    ));
    if hr < 0 {
        return None;
    }
    if pressed == IDCANCEL {
        return cancel;
    }
    let i = pressed - BUTTON_ID;
    (0..labels.len() as i32).contains(&i).then_some(i as usize)
}

fn run_message_box(mb: &MessageBox, owner: Hwnd) -> Option<usize> {
    let labels = mb.button_labels();
    let (set, ids) = standard_set(&labels)?;
    let icon = match mb.kind {
        MessageKind::Info => MB_ICONINFORMATION,
        MessageKind::Warning => MB_ICONWARNING,
        MessageKind::Error => MB_ICONERROR,
        MessageKind::Question => MB_ICONQUESTION,
    };
    let default = match mb.default_index() {
        0 => 0,
        1 => MB_DEFBUTTON2,
        _ => MB_DEFBUTTON3,
    };
    let mut text = mb.text.clone();
    if !mb.detail.is_empty() {
        text.push_str("\n\n");
        text.push_str(&mb.detail);
    }
    let title = wide(&caption(mb));
    if let Ok(mut c) = CURRENT.lock() {
        *c = Some(Current::Box {
            caption: title.clone(),
            ids: ids.clone(),
        });
    }
    let r = unsafe {
        MessageBoxW(
            owner,
            wide(&text).as_ptr(),
            title.as_ptr(),
            set | icon | default | MB_SETFOREGROUND,
        )
    };
    if let Ok(mut c) = CURRENT.lock() {
        *c = None;
    }
    super::debug(format_args!("message box: MessageBoxW -> {r}"));
    ids.iter().position(|&id| id == r)
}

pub(super) fn press(index: usize) -> bool {
    let Ok(current) = CURRENT.lock() else {
        return false;
    };
    unsafe {
        match &*current {
            Some(Current::Task(hwnd)) => {
                PostMessageW(
                    *hwnd as Hwnd,
                    TDM_CLICK_BUTTON,
                    (BUTTON_ID as usize) + index,
                    0,
                ) != 0
            }
            Some(Current::Box { caption, ids }) => {
                let Some(&id) = ids.get(index) else {
                    return false;
                };
                let hwnd = FindWindowW(wide("#32770").as_ptr(), caption.as_ptr());
                !hwnd.is_null() && PostMessageW(hwnd, WM_COMMAND, id as usize, 0) != 0
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_commctrl() {
        // The packed x64 sizes commctrl.h gives.
        assert_eq!(std::mem::size_of::<TaskDialogButton>(), 12);
        assert_eq!(std::mem::size_of::<TaskDialogConfig>(), 160);
    }

    #[test]
    fn standard_sets() {
        let s = |l: &[&str]| standard_set(&l.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&["OK"]), Some((0, vec![1])));
        assert_eq!(s(&["&Yes", "No"]), Some((4, vec![6, 7])));
        assert_eq!(s(&["Save", "Don't Save"]), None);
    }
}
