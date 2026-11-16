//! External images on Metal: an `IOSurface` bound as a texture per plane
//! (`newTextureWithDescriptor:iosurface:plane:`), no pixel copied. BGRA
//! and RGBA surfaces are one texture; NV12 (the camera's `420f` buffers)
//! is a luma (`R8Unorm`) and a chroma (`RG8Unorm`) texture, converted by
//! `quad_yuv_fs`.
//!
//! Imports are cached by the image's identity, like the image textures, and
//! the images of the last few frames are held, so a buffer isn't handed back
//! to its producer (the camera's pool) while a frame in flight reads it.

use super::*;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalFormat, ExternalImage};
use std::collections::VecDeque;

const PIXEL_RG8: u64 = 30;
/// Frames whose images are held (Metal keeps up to three in flight).
const HELD_FRAMES: usize = 3;

struct Import {
    image: Weak<ExternalImage>,
    textures: Vec<Id>,
    used: u64,
}

/// The executor's imports.
#[derive(Default)]
pub(super) struct Imports {
    map: HashMap<usize, Import>,
    held: VecDeque<Vec<Arc<ExternalImage>>>,
}

impl Imports {
    /// A frame begins: let go of the images of frames long done, and free
    /// the imports of images gone or not drawn lately.
    pub(super) fn begin(&mut self, count: u64, yuv: bool) {
        self.held.push_back(Vec::new());
        while self.held.len() > HELD_FRAMES {
            self.held.pop_front();
        }
        self.map.retain(|_, i| {
            let keep =
                i.image.strong_count() > 0 && count.saturating_sub(i.used) < TEXTURE_KEEP_FRAMES;
            if !keep {
                for &t in &i.textures {
                    unsafe { release(t) };
                }
            }
            keep
        });
        let all = [
            ExternalFormat::Bgra8,
            ExternalFormat::Rgba8,
            ExternalFormat::Nv12,
        ];
        crate::platform::external::note_native(if yuv { &all } else { &all[..2] });
    }

    /// The texture, and for NV12 the chroma texture and color space too,
    /// for `image`; `None` if it isn't an `IOSurface` or can't be bound.
    pub(super) unsafe fn get(
        &mut self,
        device: Id,
        image: &Arc<ExternalImage>,
        count: u64,
        yuv: bool,
    ) -> Option<(Id, Option<(Id, YuvColorSpace)>)> {
        if image.format() == ExternalFormat::Nv12 && !yuv {
            return None;
        }
        let key = Arc::as_ptr(image) as usize;
        if let Some(i) = self.map.get(&key)
            && !i.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image))
        {
            for t in self.map.remove(&key).unwrap().textures {
                unsafe { release(t) };
            }
        }
        let i = self.map.entry(key).or_insert_with(|| Import {
            image: Arc::downgrade(image),
            textures: unsafe { import(device, image) },
            used: count,
        });
        i.used = count;
        if let Some(frame) = self.held.back_mut()
            && !frame.iter().any(|h| Arc::ptr_eq(h, image))
        {
            frame.push(image.clone());
        }
        match i.textures[..] {
            [t] => Some((t, None)),
            [y, uv] => Some((y, Some((uv, image.yuv_color_space())))),
            _ => None,
        }
    }

    /// Release every texture (the executor is going away).
    pub(super) fn clear(&mut self) {
        for (_, i) in self.map.drain() {
            for t in i.textures {
                unsafe { release(t) };
            }
        }
        self.held.clear();
    }
}

/// Bind each plane of `image`'s `IOSurface` as a texture (none if it isn't
/// one, or a plane fails).
unsafe fn import(device: Id, image: &ExternalImage) -> Vec<Id> {
    use crate::platform::external::Handle;
    let Handle::IoSurface(surface) = image.handle() else {
        return Vec::new();
    };
    let formats: &[u64] = match image.format() {
        ExternalFormat::Bgra8 | ExternalFormat::Bgrx8 => &[PIXEL_BGRA8],
        ExternalFormat::Rgba8 | ExternalFormat::Rgbx8 => &[PIXEL_RGBA8],
        ExternalFormat::Nv12 => &[PIXEL_R8, PIXEL_RG8],
        _ => return Vec::new(),
    };
    let size = image.size();
    let mut out = Vec::new();
    unsafe {
        for (plane, &format) in formats.iter().enumerate() {
            let (w, h, _) = image.format().plane_extent(plane, size);
            let desc = send!(Id; class(c"MTLTextureDescriptor"),
                c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
                format; u64, w as u64; u64, h as u64; u64, false; bool);
            send!((); desc, c"setUsage:", USAGE_SHADER_READ; u64);
            let tex = send!(Id; device, c"newTextureWithDescriptor:iosurface:plane:",
                desc; Id, *surface; *mut c_void, plane; usize);
            if tex.is_null() {
                for t in out.drain(..) {
                    release(t);
                }
                return out;
            }
            out.push(tex);
        }
    }
    out
}

/// The YUV fragment shader's uniforms: `[nearest, 0, 0, 0]`, then the
/// color space's matrix rows (`YuvColorSpace::rows`).
pub(super) fn yuv_uniforms(nearest: bool, color: YuvColorSpace) -> [f32; 16] {
    let mut u = [0.0; 16];
    u[0] = if nearest { 1.0 } else { 0.0 };
    u[4..].copy_from_slice(&color.rows());
    u
}

/// NV12 from a luma (`.r`) and a chroma (`.rg`: Cb, Cr) texture, turned
/// into RGB by the image's color space's matrix as the CPU converts it
/// (`gl::external::QUAD_YUV_MAIN` in MSL).
pub(super) const MSL_YUV: &str = r#"
struct YuvU { float4 flags; float4 m0; float4 m1; float4 m2; };
fragment float4 quad_yuv_fs(QOut in [[stage_in]], texture2d<float> ty [[texture(0)]],
                            texture2d<float> tc [[texture(1)]],
                            constant YuvU& u [[buffer(0)]]) {
    if (in.mode < 0.5) { return quad_box(in); }
    constexpr sampler lin(filter::linear, address::clamp_to_edge);
    constexpr sampler near(filter::nearest, address::clamp_to_edge);
    float2 uv = clamp(in.p, in.a2.xy, in.a2.zw);
    bool n = u.flags.x > 0.5;
    float y = n ? ty.sample(near, uv).r : ty.sample(lin, uv).r;
    float2 c = n ? tc.sample(near, uv).rg : tc.sample(lin, uv).rg;
    float4 yuv = float4(y, c, 1.0);
    float4 col = float4(clamp(float3(dot(u.m0, yuv), dot(u.m1, yuv), dot(u.m2, yuv)),
                              0.0, 1.0), 1.0);
    col *= in.a3;
    float cov = clamp(min(in.a1.x, in.a1.z) + 0.5, 0.0, 1.0)
              * clamp(min(in.a1.y, in.a1.w) + 0.5, 0.0, 1.0);
    return col * cov;
}
"#;
