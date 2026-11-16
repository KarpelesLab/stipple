//! External images on Direct3D 11: a DXGI shared handle opened as a
//! texture on the executor's device (`ID3D11Device1::OpenSharedResource1`
//! for an NT handle, `ID3D11Device::OpenSharedResource` for a legacy one)
//! and sampled through a shader resource view in the texture's own format.
//! An NV12 texture (`DXGI_FORMAT_NV12`) is sampled through two views, its
//! luma plane as `R8_UNORM` and its chroma plane as `R8G8_UNORM`, which
//! `quad_yuv_ps` converts with the image's color space. No pixel is copied.
//!
//! Imports are cached by the image's identity, and the images the last
//! frame drew are held until the next one begins.

use super::*;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalFormat, ExternalImage};

/// `ID3D11Device::OpenSharedResource`.
const VT_DEVICE_OPEN_SHARED_RESOURCE: usize = 28;
/// `ID3D11Device1::OpenSharedResource1`.
const VT_DEVICE1_OPEN_SHARED_RESOURCE1: usize = 48;
/// `ID3D11Device1`.
const IID_ID3D11_DEVICE1: Guid = Guid {
    data1: 0xa04b_fb29,
    data2: 0x08ef,
    data3: 0x43d6,
    data4: [0xa4, 0x9c, 0xa9, 0xbd, 0xbd, 0xcb, 0xe6, 0x86],
};
const DXGI_FORMAT_R8G8_UNORM: u32 = 49;
#[cfg(test)]
const DXGI_FORMAT_NV12: u32 = 103;
const D3D11_SRV_DIMENSION_TEXTURE2D: u32 = 4;

/// `D3D11_SHADER_RESOURCE_VIEW_DESC` for a 2D texture (the union's other
/// members are at most 16 bytes, as `Texture2D` and the padding).
#[repr(C)]
struct SrvDesc {
    format: u32,
    dimension: u32,
    most_detailed_mip: u32,
    mip_levels: u32,
    _rest: [u32; 2],
}

/// An external image's texture and its views.
struct Opened {
    _tex: Com,
    srv: Com,
    /// NV12's chroma view.
    chroma: Option<Com>,
}

struct Import {
    image: Weak<ExternalImage>,
    /// `None` if it couldn't be opened.
    opened: Option<Opened>,
    used: u64,
}

/// The views an external image is drawn from: one, or NV12's luma and
/// chroma and the color space they're converted in.
#[derive(Clone, Copy, Debug)]
pub(super) struct Views {
    pub srv: *mut c_void,
    pub chroma: Option<(*mut c_void, YuvColorSpace)>,
}

/// The executor's imports.
#[derive(Default)]
pub(super) struct Imports {
    map: HashMap<usize, Import>,
    held: Vec<Arc<ExternalImage>>,
}

impl Imports {
    /// A frame begins: let go of the last frame's images, and free the
    /// imports of images gone or not drawn lately.
    pub(super) fn begin(&mut self, count: u64) {
        self.held.clear();
        self.map.retain(|_, i| {
            i.image.strong_count() > 0 && count.saturating_sub(i.used) < TEXTURE_KEEP_FRAMES
        });
        crate::platform::external::note_native(&[
            ExternalFormat::Rgba8,
            ExternalFormat::Bgra8,
            ExternalFormat::Rgbx8,
            ExternalFormat::Bgrx8,
            ExternalFormat::Nv12,
        ]);
    }

    /// The shader resource views of `image`'s texture, opened on `device`
    /// unless it's cached; `None` if it isn't a shared handle or can't be
    /// opened here.
    pub(super) unsafe fn get(
        &mut self,
        device: *mut c_void,
        image: &Arc<ExternalImage>,
        count: u64,
    ) -> Option<Views> {
        let key = Arc::as_ptr(image) as usize;
        if self
            .map
            .get(&key)
            .is_some_and(|i| !i.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image)))
        {
            self.map.remove(&key);
        }
        let i = self.map.entry(key).or_insert_with(|| Import {
            image: Arc::downgrade(image),
            opened: unsafe { open(device, image) },
            used: count,
        });
        i.used = count;
        if !self.held.iter().any(|h| Arc::ptr_eq(h, image)) {
            self.held.push(image.clone());
        }
        i.opened.as_ref().map(|o| Views {
            srv: o.srv.ptr(),
            chroma: o
                .chroma
                .as_ref()
                .map(|c| (c.ptr(), image.yuv_color_space())),
        })
    }
}

/// A view of the 2D texture `tex` in `format` (an NV12 plane).
unsafe fn plane_view(device: *mut c_void, tex: *mut c_void, format: u32) -> Option<Com> {
    let desc = SrvDesc {
        format,
        dimension: D3D11_SRV_DIMENSION_TEXTURE2D,
        most_detailed_mip: 0,
        mip_levels: 1,
        _rest: [0; 2],
    };
    let mut v = std::ptr::null_mut();
    let hr = unsafe {
        com!(device, VT_DEVICE_CREATE_SRV,
            (*mut c_void, *const SrvDesc, *mut *mut c_void) -> Hresult,
            tex, &desc, &mut v)
    };
    (hr >= 0 && !v.is_null()).then(|| Com(v))
}

/// Open `image`'s shared handle on `device` and view it.
unsafe fn open(device: *mut c_void, image: &ExternalImage) -> Option<Opened> {
    use crate::platform::external::Handle;
    let Handle::Dxgi { handle, nt } = image.handle() else {
        return None;
    };
    unsafe {
        let mut tex = std::ptr::null_mut();
        let hr = if *nt {
            let mut dev1 = std::ptr::null_mut();
            let hr = com!(device, VT_QUERY_INTERFACE, (*const Guid, *mut *mut c_void) -> Hresult,
                &IID_ID3D11_DEVICE1, &mut dev1);
            if hr < 0 || dev1.is_null() {
                return None;
            }
            let dev1 = Com(dev1);
            com!(dev1.ptr(), VT_DEVICE1_OPEN_SHARED_RESOURCE1,
                (*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
                *handle, &IID_ID3D11_TEXTURE2D, &mut tex)
        } else {
            com!(device, VT_DEVICE_OPEN_SHARED_RESOURCE,
                (*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
                *handle, &IID_ID3D11_TEXTURE2D, &mut tex)
        };
        if hr < 0 || tex.is_null() {
            return None;
        }
        let tex = Com(tex);
        if image.format() == ExternalFormat::Nv12 {
            let srv = plane_view(device, tex.ptr(), DXGI_FORMAT_R8_UNORM)?;
            let chroma = plane_view(device, tex.ptr(), DXGI_FORMAT_R8G8_UNORM)?;
            return Some(Opened {
                _tex: tex,
                srv,
                chroma: Some(chroma),
            });
        }
        let srv = view(device, VT_DEVICE_CREATE_SRV, tex.ptr()).ok()?;
        Some(Opened {
            _tex: tex,
            srv,
            chroma: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Rect, Size};
    use crate::gpu::renderer::Executor;
    use crate::render::{Color, ImageOptions, Scene, SoftwareRenderer};

    /// A shared NV12 texture (luma rising left to right, chroma red on the
    /// left half and blue on the right), made on a device of its own as a
    /// video decoder would, and its legacy shared handle; with the bytes.
    unsafe fn shared_nv12(w: u32, h: u32) -> Result<(Com, Com, *mut c_void, Vec<u8>), String> {
        unsafe {
            let (device, ctx, _) = create_device().map_err(|e| e.to_string())?;
            let mut bytes = vec![0u8; (w * h * 3 / 2) as usize];
            for y in 0..h {
                for x in 0..w {
                    bytes[(y * w + x) as usize] = 60 + (x * 160 / w) as u8;
                }
            }
            for y in 0..h / 2 {
                for x in 0..w / 2 {
                    let (cb, cr) = if x < w / 4 { (90, 220) } else { (220, 100) };
                    let at = (w * h + y * w + x * 2) as usize;
                    bytes[at] = cb;
                    bytes[at + 1] = cr;
                }
            }
            let mut desc = Texture2dDesc::new(
                w,
                h,
                DXGI_FORMAT_NV12,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_SHADER_RESOURCE,
                0,
            );
            desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED;
            let data = SubresourceData {
                pSysMem: bytes.as_ptr().cast(),
                SysMemPitch: w,
                SysMemSlicePitch: 0,
            };
            let mut tex = std::ptr::null_mut();
            let hr = com!(device.ptr(), VT_DEVICE_CREATE_TEXTURE2D,
                (*const Texture2dDesc, *const SubresourceData, *mut *mut c_void) -> Hresult,
                &desc, &data, &mut tex);
            if hr < 0 || tex.is_null() {
                return Err(format!(
                    "CreateTexture2D (shared NV12) failed ({:#x})",
                    hr as u32
                ));
            }
            let tex = Com(tex);
            let mut res = std::ptr::null_mut();
            let hr = com!(tex.ptr(), VT_QUERY_INTERFACE, (*const Guid, *mut *mut c_void) -> Hresult,
                &IID_IDXGI_RESOURCE, &mut res);
            if hr < 0 || res.is_null() {
                return Err("QueryInterface(IDXGIResource) failed".into());
            }
            let res = Com(res);
            let mut handle = std::ptr::null_mut();
            let hr = com!(res.ptr(), VT_DXGI_RESOURCE_GET_SHARED_HANDLE,
                (*mut *mut c_void) -> Hresult, &mut handle);
            if hr < 0 || handle.is_null() {
                return Err(format!("GetSharedHandle failed ({:#x})", hr as u32));
            }
            com!(ctx.ptr(), VT_CTX_FLUSH, () -> ());
            Ok((device, tex, handle, bytes))
        }
    }

    /// An NV12 texture shared by another device is opened, sampled through
    /// its two plane views, converted in the shader, and drawn as the CPU
    /// converts the same bytes; nothing is uploaded.
    #[test]
    fn a_shared_nv12_texture_is_drawn_without_an_upload() {
        let (w, h) = (32u32, 16u32);
        let (_device, _tex, handle, bytes) = match unsafe { shared_nv12(w, h) } {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skipping: no shared NV12 texture: {e}");
                return;
            }
        };
        let size = PhysicalSize::new(w, h);
        let ws = w as usize;
        let planes = [
            (&bytes[..ws * h as usize], ws),
            (&bytes[ws * h as usize..], ws),
        ];
        let pixels = crate::platform::external::to_rgba(
            ExternalFormat::Nv12,
            size,
            &planes,
            YuvColorSpace::default(),
        )
        .unwrap();
        let image =
            unsafe { ExternalImage::dxgi_shared_handle(handle, false, size, ExternalFormat::Nv12) }
                .unwrap()
                .cpu_pixels(Arc::new(pixels));
        let image = Arc::new(image);
        let mut scene = Scene::new(Size::new(48.0, 32.0));
        let opts = ImageOptions::new().nearest();
        let src = Rect::from_xywh(0.0, 0.0, w as f64, h as f64);
        scene.draw_external_region(src, Rect::from_xywh(8.0, 8.0, 32.0, 16.0), &image, &opts);
        let bg = Color::rgb(20, 30, 40);
        let mut ex = match D3dExecutor::new() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("skipping: no Direct3D 11 executor: {e}");
                return;
            }
        };
        let mut rasters = crate::gpu::ops::RasterCache::default();
        let plan = crate::gpu::ops::plan(&scene, bg, PhysicalSize::new(48, 32), &mut rasters);
        let gpu = ex.read_back(&plan).expect("drawn");
        let uploads = ex.uploads();
        let cpu = SoftwareRenderer::new()
            .background(bg)
            .render(scene.clone(), ScaleFactor::IDENTITY);
        let (mut sum, mut worst) = (0u64, 0u8);
        for (p, q) in gpu.as_bytes().iter().zip(cpu.as_bytes()) {
            let d = p.abs_diff(*q);
            sum += d as u64;
            worst = worst.max(d);
        }
        let mean = sum as f64 / gpu.as_bytes().len() as f64;
        eprintln!(
            "shared NV12 on {}: mean {mean:.3}, worst {worst}, {uploads} uploads",
            ex.device()
        );
        assert!(mean < 1.5, "mean {mean}, worst {worst}");
        assert_eq!(uploads, 0, "imported, not uploaded");
    }
}
