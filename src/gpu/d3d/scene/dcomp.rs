//! Transparent windows on Direct3D 11: a flip-model swap chain on a window
//! (`CreateSwapChainForHwnd`) is always opaque, so a transparent window's
//! swap chain is made for composition (`CreateSwapChainForComposition`,
//! `DXGI_ALPHA_MODE_PREMULTIPLIED`) and shown through DirectComposition: a
//! target on the window, topmost (over the redirection surface the window's
//! GDI painting would go to), whose root visual's content is the swap chain.
//! The desktop compositor then blends the premultiplied frames over what is
//! behind the window, as it does the window's premultiplied bitmap.
//!
//! `dcomp.dll` is loaded at run time (Windows 8 and later); without it a
//! transparent window can't be presented to and the frame goes to the CPU.

use super::*;

/// `IDXGIFactory2::CreateSwapChainForComposition` (after
/// `UnregisterOcclusionStatus`, 23).
pub(super) const VT_FACTORY2_CREATE_SWAP_CHAIN_FOR_COMPOSITION: usize = 24;
/// `IDCompositionDevice` (`IUnknown` 0-2): Commit 3, WaitForCommitCompletion
/// 4, GetFrameStatistics 5, CreateTargetForHwnd 6, CreateVisual 7.
const VT_DCOMP_COMMIT: usize = 3;
const VT_DCOMP_CREATE_TARGET_FOR_HWND: usize = 6;
const VT_DCOMP_CREATE_VISUAL: usize = 7;
/// `IDCompositionTarget::SetRoot`.
const VT_TARGET_SET_ROOT: usize = 3;
/// `IDCompositionVisual::SetContent`: after SetOffsetX (two overloads),
/// SetOffsetY (two), SetTransform (two), SetTransformParent, SetEffect,
/// SetBitmapInterpolationMode, SetBorderMode and SetClip (two).
const VT_VISUAL_SET_CONTENT: usize = 15;
/// `DXGI_ALPHA_MODE_PREMULTIPLIED`.
pub(super) const DXGI_ALPHA_MODE_PREMULTIPLIED: u32 = 1;

/// {c37ea93a-e7aa-450d-b16f-9746cb0407f3}
const IID_IDCOMPOSITION_DEVICE: Guid = Guid {
    data1: 0xc37e_a93a,
    data2: 0xe7aa,
    data3: 0x450d,
    data4: [0xb1, 0x6f, 0x97, 0x46, 0xcb, 0x04, 0x07, 0xf3],
};

/// `DCompositionCreateDevice`.
type CreateDeviceFn =
    unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

/// `DCompositionCreateDevice` from `dcomp.dll`, loaded once.
fn create_device_fn() -> Option<CreateDeviceFn> {
    static ENTRY: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let entry = *ENTRY.get_or_init(|| unsafe {
        let name: Vec<u16> = "dcomp.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(name.as_ptr());
        if module.is_null() {
            return 0;
        }
        GetProcAddress(module, c"DCompositionCreateDevice".as_ptr().cast::<u8>()) as usize
    });
    (entry != 0).then(|| unsafe { core::mem::transmute::<usize, CreateDeviceFn>(entry) })
}

/// What shows a composition swap chain in a window: released (the window
/// shows its redirection surface again) when dropped.
pub(super) struct Composition {
    // Released in this order: the visual, the target, the device.
    _visual: Com,
    _target: Com,
    _device: Com,
}

/// Make the composition swap chain description: as for a window, but
/// premultiplied (`STRETCH` scaling, which composition swap chains need).
pub(super) fn composition_desc(w: u32, h: u32, effect: u32, flags: u32) -> SwapChainDesc1 {
    SwapChainDesc1 {
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Scaling: DXGI_SCALING_STRETCH,
        ..swap_chain_desc(w, h, effect, flags)
    }
}

/// Show `swap` (a composition swap chain made on the device whose
/// `IDXGIDevice` is `dxgi`) over all of `hwnd`'s client area.
///
/// # Safety
/// `dxgi` and `swap` are live; `hwnd` is a window of this thread.
pub(super) unsafe fn compose(
    dxgi: *mut c_void,
    hwnd: *mut c_void,
    swap: *mut c_void,
) -> Result<Composition, GpuError> {
    unsafe {
        let create = create_device_fn()
            .ok_or_else(|| GpuError::Unavailable("no DirectComposition (dcomp.dll)".into()))?;
        let mut device = std::ptr::null_mut();
        check(
            create(dxgi, &IID_IDCOMPOSITION_DEVICE, &mut device),
            "DCompositionCreateDevice",
        )?;
        let device = Com(device);
        let mut target = std::ptr::null_mut();
        // Topmost: over the window's redirection surface.
        check(
            com!(device.ptr(), VT_DCOMP_CREATE_TARGET_FOR_HWND,
                (*mut c_void, i32, *mut *mut c_void) -> Hresult, hwnd, 1, &mut target),
            "IDCompositionDevice::CreateTargetForHwnd",
        )?;
        let target = Com(target);
        let mut visual = std::ptr::null_mut();
        check(
            com!(device.ptr(), VT_DCOMP_CREATE_VISUAL,
                (*mut *mut c_void) -> Hresult, &mut visual),
            "IDCompositionDevice::CreateVisual",
        )?;
        let visual = Com(visual);
        check(
            com!(visual.ptr(), VT_VISUAL_SET_CONTENT, (*mut c_void) -> Hresult, swap),
            "IDCompositionVisual::SetContent",
        )?;
        check(
            com!(target.ptr(), VT_TARGET_SET_ROOT, (*mut c_void) -> Hresult, visual.ptr()),
            "IDCompositionTarget::SetRoot",
        )?;
        check(
            com!(device.ptr(), VT_DCOMP_COMMIT, () -> Hresult),
            "IDCompositionDevice::Commit",
        )?;
        Ok(Composition {
            _visual: visual,
            _target: target,
            _device: device,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_swap_chains_are_premultiplied() {
        let d = composition_desc(64, 32, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 0);
        assert_eq!((d.Width, d.Height), (64, 32));
        assert_eq!(d.AlphaMode, DXGI_ALPHA_MODE_PREMULTIPLIED);
        assert_eq!(d.Scaling, DXGI_SCALING_STRETCH);
        assert_eq!(d.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(d.SwapEffect, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL);
    }
}
