//! Raw Metal FFI — no `metal`/`objc` crate, just the Metal framework's
//! `MTLCreateSystemDefaultDevice` and a couple of `objc_msgSend` calls by hand
//! (mirroring the Cocoa backend's approach), matching the "close to the OS"
//! policy. macOS-only.
//!
//! This is the entry point for a GPU-native Metal render backend: it creates the
//! system default `MTLDevice` and reads its name, the same way the Vulkan path
//! starts from an instance + device. The offscreen pipeline (a `MTLTexture`
//! render target, a render pipeline from a `.metal` source string, a command
//! buffer that draws, and `getBytes` readback) builds on this.

#![allow(unsafe_code, non_snake_case, non_upper_case_globals)]

use core::ffi::{c_char, c_void};
use std::ffi::{CStr, CString};

type Id = *mut c_void;
type Sel = *mut c_void;
type Class = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(ctx: *mut c_void);
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

// ---- IOSurface export: macOS shared GPU surface (dma-buf analog) -------------
//
// IOSurface is the macOS cross-process shared-image primitive — the analog of a
// Linux dma-buf for the compositor's content path. A content process creates an
// IOSurface (CPU+GPU accessible, bindable as an `MTLTexture`) and hands the UI
// process its global `IOSurfaceID`, which the UI re-opens with `IOSurfaceLookup`.
// Built via the CoreFoundation C API (no `core-foundation` crate).

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFNumberRef = *const c_void;
type CFDictionaryRef = *const c_void;
type IOSurfaceRef = *const c_void;

const KCF_NUMBER_SINT32: isize = 3; // kCFNumberSInt32Type
const IOSURFACE_PIXEL_FORMAT_BGRA: i32 = 0x4247_5241; // 'BGRA'

/// Opaque stand-in for the `CFDictionary{Key,Value}CallBacks` structs — we only
/// take the address of the real CoreFoundation globals, never their layout.
#[repr(C)]
struct CFCallbacks {
    _opaque: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFNumberCreate(allocator: CFTypeRef, the_type: isize, value: *const c_void) -> CFNumberRef;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const CFCallbacks,
        value_callbacks: *const CFCallbacks,
    ) -> CFDictionaryRef;
    fn CFRelease(cf: CFTypeRef);
    static kCFTypeDictionaryKeyCallBacks: CFCallbacks;
    static kCFTypeDictionaryValueCallBacks: CFCallbacks;
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceCreate(properties: CFDictionaryRef) -> IOSurfaceRef;
    fn IOSurfaceGetID(buffer: IOSurfaceRef) -> u32;
    static kIOSurfaceWidth: CFStringRef;
    static kIOSurfaceHeight: CFStringRef;
    static kIOSurfaceBytesPerElement: CFStringRef;
    static kIOSurfacePixelFormat: CFStringRef;
}

/// Create a shareable `width`×`height` BGRA8 **IOSurface** and return its global
/// `IOSurfaceID` — the macOS analog of exporting a `dma-buf` for the compositor's
/// content path (the content process creates the surface; the UI process re-opens
/// the id with `IOSurfaceLookup` and binds it as an `MTLTexture`). Errors if the
/// CoreFoundation property dictionary or the surface can't be created.
pub fn export_iosurface(width: u32, height: u32) -> Result<String, String> {
    unsafe {
        let (w, h, bpe, fmt) = (
            width as i32,
            height as i32,
            4i32,
            IOSURFACE_PIXEL_FORMAT_BGRA,
        );
        let num = |v: &i32| {
            CFNumberCreate(
                core::ptr::null(),
                KCF_NUMBER_SINT32,
                v as *const i32 as *const c_void,
            )
        };
        let (n_w, n_h, n_bpe, n_fmt) = (num(&w), num(&h), num(&bpe), num(&fmt));

        let keys: [*const c_void; 4] = [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
        ];
        let values: [*const c_void; 4] = [n_w, n_h, n_bpe, n_fmt];
        let dict = CFDictionaryCreate(
            core::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            4,
            core::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
            core::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
        );
        CFRelease(n_w);
        CFRelease(n_h);
        CFRelease(n_bpe);
        CFRelease(n_fmt);
        if dict.is_null() {
            return Err("CFDictionaryCreate failed".into());
        }

        let surface = IOSurfaceCreate(dict);
        CFRelease(dict);
        if surface.is_null() {
            return Err("IOSurfaceCreate failed".into());
        }
        let id = IOSurfaceGetID(surface);
        CFRelease(surface);
        Ok(format!("IOSurface {width}x{height} BGRA, id {id}"))
    }
}

// Metal enum values we use.
const MTL_PIXEL_FORMAT_RGBA8UNORM: u64 = 70;
const MTL_TEXTURE_USAGE_RENDER_TARGET: u64 = 0x0004;
const MTL_STORAGE_MODE_SHARED: u64 = 0;
const MTL_LOAD_ACTION_CLEAR: u64 = 2;
const MTL_STORE_ACTION_STORE: u64 = 1;
const MTL_PRIMITIVE_TYPE_TRIANGLE: u64 = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlClearColor {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlOrigin {
    x: u64,
    y: u64,
    z: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlSize {
    width: u64,
    height: u64,
    depth: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlRegion {
    origin: MtlOrigin,
    size: MtlSize,
}

fn class(name: &str) -> Class {
    let c = CString::new(name).unwrap();
    unsafe { objc_getClass(c.as_ptr()) }
}

fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}

/// `+[NSString stringWithUTF8String:]` — `stringWithUTF8String:` copies the
/// bytes, so the temporary `CString` need not outlive the call.
fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap();
    unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), c.as_ptr()) }
}

/// `objc_msgSend` typed as `(id, SEL) -> id` — the documented way to call it.
unsafe fn msg_id(obj: Id, s: Sel) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s)
    }
}

/// `objc_msgSend` typed as `(id, SEL) -> *const c_char` (for `-UTF8String`).
unsafe fn msg_cstr(obj: Id, s: Sel) -> *const c_char {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s)
    }
}

/// `objc_msgSend` typed as `(id, SEL)` returning nothing (for `-release`).
unsafe fn msg_void(obj: Id, s: Sel) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) = std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s);
    }
}

unsafe fn msg_id_id(obj: Id, s: Sel, a: Id) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, a)
    }
}

unsafe fn msg_id_u64(obj: Id, s: Sel, a: u64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, u64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, a)
    }
}

unsafe fn msg_void_id(obj: Id, s: Sel, a: Id) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, a);
    }
}

unsafe fn msg_void_u64(obj: Id, s: Sel, a: u64) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, u64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, a);
    }
}

/// `+[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:width:height:mipmapped:]`.
unsafe fn msg_texdesc(cls: Class, s: Sel, fmt: u64, w: u64, h: u64, mip: bool) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Class, Sel, u64, u64, u64, bool) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(cls, s, fmt, w, h, mip)
    }
}

/// `-[…setClearColor:]` — `MTLClearColor` is 4 doubles (an HFA, passed in SIMD
/// registers on arm64, which `extern "C"` matches).
unsafe fn msg_void_clearcolor(obj: Id, s: Sel, c: MtlClearColor) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, MtlClearColor) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, c);
    }
}

/// `-[MTLTexture getBytes:bytesPerRow:fromRegion:mipmapLevel:]`. `MTLRegion` is
/// 48 bytes, passed indirectly per AAPCS64 — `extern "C"` matches.
unsafe fn msg_getbytes(obj: Id, s: Sel, ptr: *mut c_void, bpr: u64, region: MtlRegion, level: u64) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, *mut c_void, u64, MtlRegion, u64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, ptr, bpr, region, level);
    }
}

unsafe fn msg_id_cstr(obj: Id, s: Sel, a: *const c_char) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, a)
    }
}

/// `-[MTLDevice newLibraryWithSource:options:error:]`.
unsafe fn msg_newobj_err(obj: Id, s: Sel, a: Id, b: Id, err: *mut Id) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id, *mut Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, a, b, err)
    }
}

/// `-[MTLDevice newRenderPipelineStateWithDescriptor:error:]`.
unsafe fn msg_newpipeline(obj: Id, s: Sel, desc: Id, err: *mut Id) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id, *mut Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, desc, err)
    }
}

/// `-[MTLRenderCommandEncoder drawPrimitives:vertexStart:vertexCount:]`.
unsafe fn msg_draw(obj: Id, s: Sel, prim: u64, start: u64, count: u64) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, u64, u64, u64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, s, prim, start, count);
    }
}

/// Create the system default `MTLDevice` and return its name (e.g.
/// `"Apple M1"`) — the foundation a GPU-native Metal backend builds on. Errors
/// if no Metal device is available.
pub fn device() -> Result<String, String> {
    unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev.is_null() {
            return Err("MTLCreateSystemDefaultDevice returned nil".into());
        }
        // `-[MTLDevice name]` → NSString → `-UTF8String` → C string we copy out.
        let ns_name = msg_id(dev, sel("name"));
        let name = if ns_name.is_null() {
            "<unnamed>".to_string()
        } else {
            let cstr = msg_cstr(ns_name, sel("UTF8String"));
            if cstr.is_null() {
                "<unnamed>".to_string()
            } else {
                CStr::from_ptr(cstr).to_string_lossy().into_owned()
            }
        };
        // We own the device returned by MTLCreateSystemDefaultDevice; release it.
        msg_void(dev, sel("release"));
        Ok(name)
    }
}

/// Render a `width`×`height` frame on the GPU through Metal and read it back: a
/// `MTLTexture` render target cleared to stipple blue via a render command
/// encoder, then `getBytes` to the CPU — an actual GPU-rendered frame, the Metal
/// analog of the Vulkan clear+readback. Returns the RGBA pixels. Errors if no
/// Metal device is available or any object fails to create.
pub fn render_clear(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let blue = MtlClearColor {
        red: 0x60 as f64 / 255.0,
        green: 0x9c as f64 / 255.0,
        blue: 1.0,
        alpha: 1.0,
    };
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let result = render_inner(width, height, blue, |_dev, _enc| Ok(Vec::new()));
        objc_autoreleasePoolPop(pool);
        result
    }
}

/// Drive one render pass: make a CPU-readable RGBA8 texture, clear it to `clear`,
/// run `record` (which may build a pipeline on the device and encode draws on the
/// encoder, returning owned objects to release once the GPU is done), then read
/// the texture back to the CPU. The shared path behind both the plain clear and
/// the triangle draw.
unsafe fn render_inner(
    width: u32,
    height: u32,
    clear: MtlClearColor,
    record: impl FnOnce(Id, Id) -> Result<Vec<Id>, String>,
) -> Result<Vec<u8>, String> {
    unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev.is_null() {
            return Err("MTLCreateSystemDefaultDevice returned nil".into());
        }
        let queue = msg_id(dev, sel("newCommandQueue"));
        if queue.is_null() {
            msg_void(dev, sel("release"));
            return Err("newCommandQueue returned nil".into());
        }

        // A CPU-readable (Shared) RGBA8 render-target texture.
        let desc = msg_texdesc(
            class("MTLTextureDescriptor"),
            sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
            MTL_PIXEL_FORMAT_RGBA8UNORM,
            width as u64,
            height as u64,
            false,
        );
        msg_void_u64(desc, sel("setUsage:"), MTL_TEXTURE_USAGE_RENDER_TARGET);
        msg_void_u64(desc, sel("setStorageMode:"), MTL_STORAGE_MODE_SHARED);
        let tex = msg_id_id(dev, sel("newTextureWithDescriptor:"), desc);
        if tex.is_null() {
            msg_void(queue, sel("release"));
            msg_void(dev, sel("release"));
            return Err("newTextureWithDescriptor returned nil".into());
        }

        // A render pass that clears the texture to `clear` and stores it.
        let rpd = msg_id(
            class("MTLRenderPassDescriptor"),
            sel("renderPassDescriptor"),
        );
        let attachments = msg_id(rpd, sel("colorAttachments"));
        let att0 = msg_id_u64(attachments, sel("objectAtIndexedSubscript:"), 0);
        msg_void_id(att0, sel("setTexture:"), tex);
        msg_void_u64(att0, sel("setLoadAction:"), MTL_LOAD_ACTION_CLEAR);
        msg_void_u64(att0, sel("setStoreAction:"), MTL_STORE_ACTION_STORE);
        msg_void_clearcolor(att0, sel("setClearColor:"), clear);

        let cmdbuf = msg_id(queue, sel("commandBuffer"));
        let enc = msg_id_id(cmdbuf, sel("renderCommandEncoderWithDescriptor:"), rpd);
        if enc.is_null() {
            msg_void(tex, sel("release"));
            msg_void(queue, sel("release"));
            msg_void(dev, sel("release"));
            return Err("renderCommandEncoderWithDescriptor returned nil".into());
        }
        // Let the caller add draw work (pipeline + drawPrimitives), if any.
        let owned = match record(dev, enc) {
            Ok(v) => v,
            Err(e) => {
                msg_void(enc, sel("endEncoding"));
                msg_void(tex, sel("release"));
                msg_void(queue, sel("release"));
                msg_void(dev, sel("release"));
                return Err(e);
            }
        };
        msg_void(enc, sel("endEncoding"));
        msg_void(cmdbuf, sel("commit"));
        msg_void(cmdbuf, sel("waitUntilCompleted"));

        // Read the texture back to the CPU.
        let size = width as usize * height as usize * 4;
        let mut pixels = vec![0u8; size];
        let region = MtlRegion {
            origin: MtlOrigin { x: 0, y: 0, z: 0 },
            size: MtlSize {
                width: width as u64,
                height: height as u64,
                depth: 1,
            },
        };
        msg_getbytes(
            tex,
            sel("getBytes:bytesPerRow:fromRegion:mipmapLevel:"),
            pixels.as_mut_ptr() as *mut c_void,
            width as u64 * 4,
            region,
            0,
        );

        // Release caller-owned objects (e.g. the pipeline state) now the GPU is
        // done, then our own.
        for obj in owned {
            msg_void(obj, sel("release"));
        }
        msg_void(tex, sel("release"));
        msg_void(queue, sel("release"));
        msg_void(dev, sel("release"));
        Ok(pixels)
    }
}

// A Metal shader pair: the vertex stage emits a centered triangle from its
// vertex id (no vertex buffers), the fragment stage paints it stipple green.
// Compiled at runtime via newLibraryWithSource (no offline toolchain).
const TRIANGLE_MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;

vertex float4 vertex_main(uint vid [[vertex_id]]) {
    float2 p[3] = { float2(0.0, 0.6), float2(0.6, -0.6), float2(-0.6, -0.6) };
    return float4(p[vid], 0.0, 1.0);
}

fragment float4 fragment_main() {
    return float4(52.0/255.0, 211.0/255.0, 153.0/255.0, 1.0);
}
"#;

/// The full Metal render pipeline: compile a `.metal` source into a library,
/// build a render pipeline from its vertex+fragment functions, and **draw** a
/// triangle over a dark-cleared `width`×`height` target, then read it back. The
/// center pixel comes out stipple green — the Metal analog of the Vulkan triangle.
/// Returns the RGBA pixels. Errors if compilation or any step fails.
pub fn render_triangle(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let dark = MtlClearColor {
        red: 0x14 as f64 / 255.0,
        green: 0x15 as f64 / 255.0,
        blue: 0x18 as f64 / 255.0,
        alpha: 1.0,
    };
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let result = render_inner(width, height, dark, |dev, enc| {
            // Compile the shader source into a library.
            let src = nsstring(TRIANGLE_MSL);
            let mut err: Id = core::ptr::null_mut();
            let lib = msg_newobj_err(
                dev,
                sel("newLibraryWithSource:options:error:"),
                src,
                core::ptr::null_mut(),
                &mut err,
            );
            if lib.is_null() {
                return Err("newLibraryWithSource failed (shader compile error)".into());
            }
            let vfn = msg_id_id(lib, sel("newFunctionWithName:"), nsstring("vertex_main"));
            let ffn = msg_id_id(lib, sel("newFunctionWithName:"), nsstring("fragment_main"));
            if vfn.is_null() || ffn.is_null() {
                msg_void(lib, sel("release"));
                return Err("newFunctionWithName returned nil".into());
            }

            // A render pipeline from the two functions, matching the target format.
            let pdesc = msg_id(
                msg_id(class("MTLRenderPipelineDescriptor"), sel("alloc")),
                sel("init"),
            );
            msg_void_id(pdesc, sel("setVertexFunction:"), vfn);
            msg_void_id(pdesc, sel("setFragmentFunction:"), ffn);
            let pca = msg_id(pdesc, sel("colorAttachments"));
            let pca0 = msg_id_u64(pca, sel("objectAtIndexedSubscript:"), 0);
            msg_void_u64(pca0, sel("setPixelFormat:"), MTL_PIXEL_FORMAT_RGBA8UNORM);

            let mut perr: Id = core::ptr::null_mut();
            let pipeline = msg_newpipeline(
                dev,
                sel("newRenderPipelineStateWithDescriptor:error:"),
                pdesc,
                &mut perr,
            );
            // The library, functions, and descriptor are no longer needed.
            msg_void(pdesc, sel("release"));
            msg_void(ffn, sel("release"));
            msg_void(vfn, sel("release"));
            msg_void(lib, sel("release"));
            if pipeline.is_null() {
                return Err("newRenderPipelineStateWithDescriptor failed".into());
            }

            // Bind it and draw the triangle (3 vertices).
            msg_void_id(enc, sel("setRenderPipelineState:"), pipeline);
            msg_draw(
                enc,
                sel("drawPrimitives:vertexStart:vertexCount:"),
                MTL_PRIMITIVE_TYPE_TRIANGLE,
                0,
                3,
            );
            // The pipeline must outlive GPU execution — hand it back to release.
            Ok(vec![pipeline])
        });
        objc_autoreleasePoolPop(pool);
        result
    }
}
