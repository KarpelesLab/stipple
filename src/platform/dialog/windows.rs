//! Windows file dialogs: the shell's `IFileOpenDialog` / `IFileSaveDialog`,
//! driven through hand-written COM vtables (the same approach as
//! [`crate::platform::uia`]).

use super::{Op, PathBuf};
use core::ffi::c_void;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

type Hresult = i32;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// GUIDs as declared in shobjidl.h.
const CLSID_FILE_OPEN_DIALOG: Guid = Guid {
    data1: 0xdc1c5a9c,
    data2: 0xe88a,
    data3: 0x4dde,
    data4: [0xa5, 0xa1, 0x60, 0xf8, 0x2a, 0x20, 0xae, 0xf7],
};
const CLSID_FILE_SAVE_DIALOG: Guid = Guid {
    data1: 0xc0b4e2f3,
    data2: 0xba21,
    data3: 0x4773,
    data4: [0x8d, 0xba, 0x33, 0x5e, 0xc9, 0x46, 0xeb, 0x8b],
};
const IID_IFILE_OPEN_DIALOG: Guid = Guid {
    data1: 0xd57c7288,
    data2: 0xd4ad,
    data3: 0x4768,
    data4: [0xbe, 0x02, 0x9d, 0x96, 0x95, 0x32, 0xd9, 0x60],
};
const IID_IFILE_SAVE_DIALOG: Guid = Guid {
    data1: 0x84bccd23,
    data2: 0x5fde,
    data3: 0x4cdb,
    data4: [0xae, 0xa4, 0xaf, 0x64, 0xb8, 0x3d, 0x78, 0xab],
};

const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
/// `CoInitializeEx` on a thread already initialised as multi-threaded. COM is
/// still usable; we just mustn't balance it with `CoUninitialize`.
const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106_u32 as i32;

// FILEOPENDIALOGOPTIONS (DWORD).
const FOS_OVERWRITEPROMPT: u32 = 0x2;
const FOS_PICKFOLDERS: u32 = 0x20;
const FOS_FORCEFILESYSTEM: u32 = 0x40;
const FOS_PATHMUSTEXIST: u32 = 0x800;
const FOS_FILEMUSTEXIST: u32 = 0x1000;

/// `SIGDN_FILESYSPATH`: the item's plain file-system path.
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
    fn CoUninitialize();
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetActiveWindow() -> *mut c_void;
}

/// The leading slots of `IFileDialog`, which both `IFileOpenDialog` and
/// `IFileSaveDialog` start with. Slot order, from shobjidl.h:
///
/// - `IUnknown`: 0 QueryInterface, 1 AddRef, 2 Release
/// - `IModalWindow`: 3 Show
/// - `IFileDialog`: 4 SetFileTypes, 5 SetFileTypeIndex, 6 GetFileTypeIndex,
///   7 Advise, 8 Unadvise, 9 SetOptions, 10 GetOptions, 11 SetDefaultFolder,
///   12 SetFolder, 13 GetFolder, 14 GetCurrentSelection, 15 SetFileName,
///   16 GetFileName, 17 SetTitle, 18 SetOkButtonLabel, 19 SetFileNameLabel,
///   20 GetResult (then AddPlace, SetDefaultExtension, Close, SetClientGuid,
///   ClearClientData, SetFilter, and the open/save-specific methods, unused).
///
/// Unused slots are `usize`, so only the methods we call carry signatures. The
/// struct is a prefix of the real vtable, which is all we ever read.
#[repr(C)]
#[allow(dead_code)]
struct FileDialogVtbl {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    show: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult,
    set_file_types: usize,
    set_file_type_index: usize,
    get_file_type_index: usize,
    advise: usize,
    unadvise: usize,
    set_options: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
    get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    set_default_folder: usize,
    set_folder: usize,
    get_folder: usize,
    get_current_selection: usize,
    set_file_name: usize,
    get_file_name: usize,
    set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> Hresult,
    set_ok_button_label: usize,
    set_file_name_label: usize,
    get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

/// `IShellItem`'s leading slots: 0 QueryInterface, 1 AddRef, 2 Release,
/// 3 BindToHandler, 4 GetParent, 5 GetDisplayName (then GetAttributes, Compare).
#[repr(C)]
#[allow(dead_code)]
struct ShellItemVtbl {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    bind_to_handler: usize,
    get_parent: usize,
    get_display_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> Hresult,
}

/// The vtable of a COM object: its first field.
///
/// # Safety
/// `obj` must be a live interface pointer whose vtable starts with `V`'s layout.
unsafe fn vtbl<'a, V>(obj: *mut c_void) -> &'a V {
    unsafe { &**(obj as *mut *const V) }
}

/// Releases a COM interface pointer on drop. Any interface works: `Release` is
/// slot 2 of `IUnknown`, which every vtable starts with.
struct Com(*mut c_void);

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (vtbl::<ShellItemVtbl>(self.0).release)(self.0) };
        }
    }
}

/// Balances a successful `CoInitializeEx` on drop.
struct ComInit(bool);

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .filter(|&u| u != 0)
        .chain(std::iter::once(0))
        .collect()
}

pub(super) fn file_chooser(op: Op, title: &str) -> Option<PathBuf> {
    // S_OK / S_FALSE (already initialised on this thread, as an STA) must be
    // balanced; RPC_E_CHANGED_MODE means an MTA, where the dialog still works.
    let hr = unsafe {
        CoInitializeEx(
            std::ptr::null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        )
    };
    if hr < 0 && hr != RPC_E_CHANGED_MODE {
        return None;
    }
    let _init = ComInit(hr >= 0);

    let (clsid, iid) = match op {
        Op::Save => (&CLSID_FILE_SAVE_DIALOG, &IID_IFILE_SAVE_DIALOG),
        Op::Open | Op::Folder => (&CLSID_FILE_OPEN_DIALOG, &IID_IFILE_OPEN_DIALOG),
    };
    let mut raw: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        CoCreateInstance(
            clsid,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            iid,
            &mut raw,
        )
    };
    if hr < 0 || raw.is_null() {
        return None;
    }
    let dialog = Com(raw);
    let vt: &FileDialogVtbl = unsafe { vtbl(dialog.0) };

    unsafe {
        let mut options = 0u32;
        if (vt.get_options)(dialog.0, &mut options) < 0 {
            return None;
        }
        options |= FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        options |= match op {
            Op::Open => FOS_FILEMUSTEXIST,
            Op::Save => FOS_OVERWRITEPROMPT,
            Op::Folder => FOS_PICKFOLDERS,
        };
        (vt.set_options)(dialog.0, options);
        let title = wide(title);
        (vt.set_title)(dialog.0, title.as_ptr());

        // Modal to our window when it's active. Cancelling returns
        // HRESULT_FROM_WIN32(ERROR_CANCELLED), a failure like any other here.
        if (vt.show)(dialog.0, GetActiveWindow()) < 0 {
            return None;
        }

        let mut item: *mut c_void = std::ptr::null_mut();
        if (vt.get_result)(dialog.0, &mut item) < 0 || item.is_null() {
            return None;
        }
        let item = Com(item);
        let mut name: *mut u16 = std::ptr::null_mut();
        let hr =
            (vtbl::<ShellItemVtbl>(item.0).get_display_name)(item.0, SIGDN_FILESYSPATH, &mut name);
        if hr < 0 || name.is_null() {
            return None;
        }
        let mut len = 0;
        while *name.add(len) != 0 {
            len += 1;
        }
        let path = OsString::from_wide(std::slice::from_raw_parts(name, len));
        CoTaskMemFree(name as *mut c_void);
        Some(PathBuf::from(path))
    }
}
