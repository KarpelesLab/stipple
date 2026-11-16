//! Raw Direct3D 11 FFI — no `windows`/`winapi` crate, just `d3d11.dll` and the
//! handful of COM methods we call by hand, matching the "close to the OS"
//! policy. Windows-only.
//!
//! This is the entry point for a GPU-native Direct3D render backend: it creates
//! a D3D11 device on **WARP** (the software rasterizer shipped with Windows, so
//! it runs on any CI runner without a GPU), the same way the Vulkan path starts
//! from an instance + device. The offscreen pipeline (render-target texture,
//! HLSL shaders, draw, staging-texture readback) builds on this.
//!
//! COM objects are accessed by hand: an interface pointer points at a vtable
//! pointer, and `IUnknown::Release` is always vtable slot 2 — enough to clean up
//! what we create without binding the full COM machinery.

#![allow(unsafe_code, non_snake_case)]

mod scene;
pub(crate) use scene::D3dExecutor;

use super::d3d_abi::*;
use core::ffi::c_void;

// The spike's names for the shared descriptors.
type D3d11Texture2dDesc = Texture2dDesc;
type D3d11MappedSubresource = MappedSubresource;
type D3d11Viewport = Viewport;

#[link(name = "d3d11")]
unsafe extern "system" {
    fn D3D11CreateDevice(
        pAdapter: *mut c_void,
        DriverType: u32,
        Software: *mut c_void,
        Flags: u32,
        pFeatureLevels: *const u32,
        FeatureLevels: u32,
        SDKVersion: u32,
        ppDevice: *mut *mut c_void,
        pFeatureLevel: *mut u32,
        ppImmediateContext: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

/// `D3DCompile`'s signature.
type D3dCompileFn = unsafe extern "system" fn(
    pSrcData: *const c_void,
    SrcDataSize: usize,
    pSourceName: *const i8,
    pDefines: *const c_void,
    pInclude: *mut c_void,
    pEntrypoint: *const i8,
    pTarget: *const i8,
    Flags1: u32,
    Flags2: u32,
    ppCode: *mut *mut c_void,
    ppErrorMsgs: *mut *mut c_void,
) -> Hresult;

/// `D3DCompile` from `d3dcompiler_47.dll`, loaded the first time it's
/// needed. The DLL ships with Windows (8.1 and later, in System32); loading
/// it at run time rather than linking it means a system without it gets an
/// error, and the CPU renderer, instead of an app that can't start.
fn d3d_compile() -> Result<D3dCompileFn, String> {
    static ENTRY: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let entry = *ENTRY.get_or_init(|| unsafe {
        let name: Vec<u16> = "d3dcompiler_47.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(name.as_ptr());
        if module.is_null() {
            return 0;
        }
        GetProcAddress(module, c"D3DCompile".as_ptr().cast::<u8>()) as usize
    });
    if entry == 0 {
        return Err("d3dcompiler_47.dll (D3DCompile) isn't available".into());
    }
    Ok(unsafe { core::mem::transmute::<usize, D3dCompileFn>(entry) })
}

/// Release a COM object via `IUnknown::Release` (vtable slot 2). No-op on null.
unsafe fn release(obj: *mut c_void) {
    unsafe {
        if obj.is_null() {
            return;
        }
        // obj -> *vtable -> [QueryInterface, AddRef, Release, ...]
        let vtbl = *(obj as *mut *mut usize);
        let release_fn: extern "system" fn(*mut c_void) -> u32 =
            core::mem::transmute(*vtbl.add(VT_RELEASE));
        release_fn(obj);
    }
}

/// Fetch the `index`-th function pointer from a COM object's vtable, ready to
/// transmute to the method's signature. `obj -> *vtable -> [fn; ...]`.
unsafe fn vmethod(obj: *mut c_void, index: usize) -> *const c_void {
    unsafe {
        let vtbl = *(obj as *const *const *const c_void);
        *vtbl.add(index)
    }
}

/// Copy a render-target texture `rt` back to the CPU via a staging texture
/// (`CopyResource` → `Map` → row copy honoring `RowPitch` → `Unmap`). Releases
/// the staging texture it creates; the caller still owns `device`/`ctx`/`rt`.
unsafe fn readback(
    device: *mut c_void,
    ctx: *mut c_void,
    rt: *mut c_void,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    unsafe {
        let create_tex2d: unsafe extern "system" fn(
            *mut c_void,
            *const D3d11Texture2dDesc,
            *const c_void,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(device, VT_DEVICE_CREATE_TEXTURE2D));
        let staging_desc = D3d11Texture2dDesc {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DxgiSampleDesc {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ,
            MiscFlags: 0,
        };
        let mut staging: *mut c_void = core::ptr::null_mut();
        if create_tex2d(device, &staging_desc, core::ptr::null(), &mut staging) != S_OK {
            return Err("CreateTexture2D (staging) failed".into());
        }
        let copy_resource: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
            core::mem::transmute(vmethod(ctx, VT_CTX_COPY_RESOURCE));
        copy_resource(ctx, staging, rt);

        let map: unsafe extern "system" fn(
            *mut c_void,
            *mut c_void,
            u32,
            u32,
            u32,
            *mut D3d11MappedSubresource,
        ) -> Hresult = core::mem::transmute(vmethod(ctx, VT_CTX_MAP));
        let mut mapped = D3d11MappedSubresource {
            pData: core::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        if map(ctx, staging, 0, D3D11_MAP_READ, 0, &mut mapped) != S_OK || mapped.pData.is_null() {
            release(staging);
            return Err("Map (staging) failed".into());
        }
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let row_bytes = width as usize * 4;
        for y in 0..height as usize {
            let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
            let dst = pixels.as_mut_ptr().add(y * row_bytes);
            core::ptr::copy_nonoverlapping(src, dst, row_bytes);
        }
        let unmap: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) =
            core::mem::transmute(vmethod(ctx, VT_CTX_UNMAP));
        unmap(ctx, staging, 0);
        release(staging);
        Ok(pixels)
    }
}

/// Create a WARP device + immediate context. Returns `(device, context)`; the
/// caller releases both.
unsafe fn create_warp() -> Result<(*mut c_void, *mut c_void), String> {
    unsafe {
        let mut device: *mut c_void = core::ptr::null_mut();
        let mut ctx: *mut c_void = core::ptr::null_mut();
        let mut feature_level: u32 = 0;
        if D3D11CreateDevice(
            core::ptr::null_mut(),
            D3D_DRIVER_TYPE_WARP,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            0,
            D3D11_SDK_VERSION,
            &mut device,
            &mut feature_level,
            &mut ctx,
        ) != S_OK
        {
            return Err("D3D11CreateDevice (WARP) failed".into());
        }
        Ok((device, ctx))
    }
}

/// Create an RGBA8 render-target texture and a view over it. Returns
/// `(texture, rtv)`; the caller releases both.
unsafe fn create_render_target(
    device: *mut c_void,
    width: u32,
    height: u32,
) -> Result<(*mut c_void, *mut c_void), String> {
    unsafe {
        let create_tex2d: unsafe extern "system" fn(
            *mut c_void,
            *const D3d11Texture2dDesc,
            *const c_void,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(device, VT_DEVICE_CREATE_TEXTURE2D));
        let rt_desc = D3d11Texture2dDesc {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DxgiSampleDesc {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut rt: *mut c_void = core::ptr::null_mut();
        if create_tex2d(device, &rt_desc, core::ptr::null(), &mut rt) != S_OK {
            return Err("CreateTexture2D (render target) failed".into());
        }
        let create_rtv: unsafe extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(device, VT_DEVICE_CREATE_RTV));
        let mut rtv: *mut c_void = core::ptr::null_mut();
        if create_rtv(device, rt, core::ptr::null(), &mut rtv) != S_OK {
            release(rt);
            return Err("CreateRenderTargetView failed".into());
        }
        Ok((rt, rtv))
    }
}

/// Create a Direct3D 11 device on WARP and return its feature level (e.g.
/// `"0xb100"` for 11_1) — the foundation a GPU-native D3D backend builds on.
/// Errors if `d3d11.dll` can't create a WARP device.
pub fn device() -> Result<String, String> {
    unsafe {
        let mut device: *mut c_void = core::ptr::null_mut();
        let mut context: *mut c_void = core::ptr::null_mut();
        let mut feature_level: u32 = 0;
        let hr = D3D11CreateDevice(
            core::ptr::null_mut(),
            D3D_DRIVER_TYPE_WARP,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            0,
            D3D11_SDK_VERSION,
            &mut device,
            &mut feature_level,
            &mut context,
        );
        if hr != S_OK {
            return Err(format!(
                "D3D11CreateDevice (WARP) failed (0x{:08x})",
                hr as u32
            ));
        }
        release(context);
        release(device);
        Ok(format!("WARP device, feature level 0x{feature_level:04x}"))
    }
}

/// Create a **shareable** RGBA8 texture on WARP and return its cross-process
/// shared `HANDLE` — the Windows analog of exporting a `dma-buf` fd for the
/// compositor's content path: the content process creates the shared texture and
/// hands the UI process this handle (which `OpenSharedResource` re-opens). Built
/// with `D3D11_RESOURCE_MISC_SHARED`, then `QueryInterface(IDXGIResource)` →
/// `GetSharedHandle`. Errors if the device, the shared texture, or the handle
/// can't be created (software WARP may decline shared resources, in which case
/// this reports the failing call — a real GPU is the intended target).
pub fn export_shared_handle(width: u32, height: u32) -> Result<String, String> {
    unsafe {
        let (device, ctx) = create_warp()?;

        let create_tex2d: unsafe extern "system" fn(
            *mut c_void,
            *const D3d11Texture2dDesc,
            *const c_void,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(device, VT_DEVICE_CREATE_TEXTURE2D));
        let desc = D3d11Texture2dDesc {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DxgiSampleDesc {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED,
        };
        let mut tex: *mut c_void = core::ptr::null_mut();
        if create_tex2d(device, &desc, core::ptr::null(), &mut tex) != S_OK {
            release(ctx);
            release(device);
            return Err("CreateTexture2D (shared) failed".into());
        }

        // QueryInterface for IDXGIResource, then GetSharedHandle.
        let query: unsafe extern "system" fn(
            *mut c_void,
            *const Guid,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(tex, 0));
        let mut res: *mut c_void = core::ptr::null_mut();
        if query(tex, &IID_IDXGI_RESOURCE, &mut res) != S_OK {
            release(tex);
            release(ctx);
            release(device);
            return Err("QueryInterface(IDXGIResource) failed".into());
        }
        let get_handle: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
            core::mem::transmute(vmethod(res, VT_DXGI_RESOURCE_GET_SHARED_HANDLE));
        let mut handle: *mut c_void = core::ptr::null_mut();
        let hr = get_handle(res, &mut handle);
        release(res);
        release(tex);
        release(ctx);
        release(device);
        if hr != S_OK {
            return Err(format!("GetSharedHandle failed (0x{:08x})", hr as u32));
        }
        Ok(format!(
            "shared D3D11 texture {width}x{height}, handle {:#x}",
            handle as usize
        ))
    }
}

/// Render a `width`×`height` frame through Direct3D 11 on WARP and read it back:
/// create a render-target texture, clear it to stipple blue, copy it into a
/// CPU-readable staging texture, map it, and return the RGBA pixels — an actual
/// GPU-rendered frame, the D3D analog of the Vulkan/Metal clear+readback. Errors
/// if any COM call fails.
pub fn render_clear(width: u32, height: u32) -> Result<Vec<u8>, String> {
    unsafe {
        let (device, ctx) = create_warp()?;
        let (rt, rtv) = match create_render_target(device, width, height) {
            Ok(t) => t,
            Err(e) => {
                release(ctx);
                release(device);
                return Err(e);
            }
        };

        // ClearRenderTargetView to stipple blue (R,G,B,A floats).
        let clear_rtv: unsafe extern "system" fn(*mut c_void, *mut c_void, *const [f32; 4]) =
            core::mem::transmute(vmethod(ctx, VT_CTX_CLEAR_RTV));
        let color: [f32; 4] = [0x60 as f32 / 255.0, 0x9c as f32 / 255.0, 1.0, 1.0];
        clear_rtv(ctx, rtv, &color);

        let result = readback(device, ctx, rt, width, height);
        release(rtv);
        release(rt);
        release(ctx);
        release(device);
        result
    }
}

// HLSL for a self-contained triangle: the vertex stage builds positions from
// SV_VertexID (no vertex/input buffers), the pixel stage paints stipple green.
// Compiled at runtime with D3DCompile (d3dcompiler_47), so no offline toolchain.
const TRIANGLE_HLSL: &[u8] = br#"
float4 VSMain(uint vid : SV_VertexID) : SV_Position {
    float2 p[3] = { float2(0.0, 0.6), float2(0.6, -0.6), float2(-0.6, -0.6) };
    return float4(p[vid], 0.0, 1.0);
}
float4 PSMain() : SV_Target {
    return float4(52.0/255.0, 211.0/255.0, 153.0/255.0, 1.0);
}
"#;

/// Compile the HLSL entry point `entry` of `src` for `target` (`vs_4_0`,
/// `ps_4_0`) to a bytecode blob, which the caller releases. The error names
/// the entry point and carries the compiler's messages.
unsafe fn compile_source(
    src: &[u8],
    entry: &core::ffi::CStr,
    target: &core::ffi::CStr,
    flags: u32,
) -> Result<*mut c_void, String> {
    unsafe {
        let d3d_compile = d3d_compile()?;
        let mut blob: *mut c_void = core::ptr::null_mut();
        let mut errors: *mut c_void = core::ptr::null_mut();
        let hr = d3d_compile(
            src.as_ptr() as *const c_void,
            src.len(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null_mut(),
            entry.as_ptr(),
            target.as_ptr(),
            flags,
            0,
            &mut blob,
            &mut errors,
        );
        let mut messages = String::new();
        if !errors.is_null() {
            let (ptr, len) = blob_bytes(errors);
            if !ptr.is_null() {
                let bytes = core::slice::from_raw_parts(ptr as *const u8, len);
                messages = String::from_utf8_lossy(bytes)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string();
            }
            release(errors);
        }
        if hr != S_OK || blob.is_null() {
            release(blob);
            return Err(format!(
                "D3DCompile failed for {} (0x{:08x}): {messages}",
                entry.to_string_lossy(),
                hr as u32
            ));
        }
        Ok(blob)
    }
}

/// Compile one entry point of the triangle spike.
unsafe fn compile(
    entry: &core::ffi::CStr,
    target: &core::ffi::CStr,
) -> Result<*mut c_void, String> {
    unsafe { compile_source(TRIANGLE_HLSL, entry, target, 0) }
}

/// Pointer + length of a shader bytecode blob (`ID3DBlob` slots 3 and 4).
unsafe fn blob_bytes(blob: *mut c_void) -> (*const c_void, usize) {
    unsafe {
        let get_ptr: unsafe extern "system" fn(*mut c_void) -> *const c_void =
            core::mem::transmute(vmethod(blob, VT_BLOB_GET_BUFFER_POINTER));
        let get_size: unsafe extern "system" fn(*mut c_void) -> usize =
            core::mem::transmute(vmethod(blob, VT_BLOB_GET_BUFFER_SIZE));
        (get_ptr(blob), get_size(blob))
    }
}

/// Render a triangle through a full Direct3D 11 pipeline on WARP and read it
/// back: compile HLSL vertex+pixel shaders with `D3DCompile`, create the shader
/// objects, bind them, set the viewport and topology, `Draw` 3 vertices over a
/// dark-cleared target, then copy the result back through a staging texture. The
/// center pixel comes out stipple green — the D3D analog of the Vulkan/Metal
/// triangle. Errors if compilation or any COM call fails.
pub fn render_triangle(width: u32, height: u32) -> Result<Vec<u8>, String> {
    unsafe {
        let (device, ctx) = create_warp()?;

        // Compile + create the two shaders.
        let vs_blob = match compile(c"VSMain", c"vs_4_0") {
            Ok(b) => b,
            Err(e) => {
                release(ctx);
                release(device);
                return Err(e);
            }
        };
        let ps_blob = match compile(c"PSMain", c"ps_4_0") {
            Ok(b) => b,
            Err(e) => {
                release(vs_blob);
                release(ctx);
                release(device);
                return Err(e);
            }
        };
        let (vs_ptr, vs_len) = blob_bytes(vs_blob);
        let (ps_ptr, ps_len) = blob_bytes(ps_blob);

        let create_vs: unsafe extern "system" fn(
            *mut c_void,
            *const c_void,
            usize,
            *mut c_void,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(device, VT_DEVICE_CREATE_VERTEX_SHADER));
        let create_ps: unsafe extern "system" fn(
            *mut c_void,
            *const c_void,
            usize,
            *mut c_void,
            *mut *mut c_void,
        ) -> Hresult = core::mem::transmute(vmethod(device, VT_DEVICE_CREATE_PIXEL_SHADER));
        let mut vs: *mut c_void = core::ptr::null_mut();
        let mut ps: *mut c_void = core::ptr::null_mut();
        let vok = create_vs(device, vs_ptr, vs_len, core::ptr::null_mut(), &mut vs) == S_OK;
        let pok = create_ps(device, ps_ptr, ps_len, core::ptr::null_mut(), &mut ps) == S_OK;
        release(ps_blob);
        release(vs_blob);
        if !vok || !pok {
            release(ps);
            release(vs);
            release(ctx);
            release(device);
            return Err("Create{Vertex,Pixel}Shader failed".into());
        }

        let (rt, rtv) = match create_render_target(device, width, height) {
            Ok(t) => t,
            Err(e) => {
                release(ps);
                release(vs);
                release(ctx);
                release(device);
                return Err(e);
            }
        };

        // Bind the target + viewport, clear to dark, bind shaders, draw.
        let om_set_rt: unsafe extern "system" fn(
            *mut c_void,
            u32,
            *const *mut c_void,
            *mut c_void,
        ) = core::mem::transmute(vmethod(ctx, VT_CTX_OM_SET_RENDER_TARGETS));
        om_set_rt(ctx, 1, &rtv, core::ptr::null_mut());

        let rs_set_vp: unsafe extern "system" fn(*mut c_void, u32, *const D3d11Viewport) =
            core::mem::transmute(vmethod(ctx, VT_CTX_RS_SET_VIEWPORTS));
        let vp = D3d11Viewport {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width as f32,
            Height: height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        rs_set_vp(ctx, 1, &vp);

        let clear_rtv: unsafe extern "system" fn(*mut c_void, *mut c_void, *const [f32; 4]) =
            core::mem::transmute(vmethod(ctx, VT_CTX_CLEAR_RTV));
        let dark: [f32; 4] = [
            0x14 as f32 / 255.0,
            0x15 as f32 / 255.0,
            0x18 as f32 / 255.0,
            1.0,
        ];
        clear_rtv(ctx, rtv, &dark);

        let vs_set: unsafe extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, u32) =
            core::mem::transmute(vmethod(ctx, VT_CTX_VS_SET_SHADER));
        let ps_set: unsafe extern "system" fn(*mut c_void, *mut c_void, *const *mut c_void, u32) =
            core::mem::transmute(vmethod(ctx, VT_CTX_PS_SET_SHADER));
        vs_set(ctx, vs, core::ptr::null(), 0);
        ps_set(ctx, ps, core::ptr::null(), 0);

        let ia_topo: unsafe extern "system" fn(*mut c_void, u32) =
            core::mem::transmute(vmethod(ctx, VT_CTX_IA_SET_PRIMITIVE_TOPOLOGY));
        ia_topo(ctx, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

        let draw: unsafe extern "system" fn(*mut c_void, u32, u32) =
            core::mem::transmute(vmethod(ctx, VT_CTX_DRAW));
        draw(ctx, 3, 0);

        let result = readback(device, ctx, rt, width, height);

        release(rtv);
        release(rt);
        release(ps);
        release(vs);
        release(ctx);
        release(device);
        result
    }
}
