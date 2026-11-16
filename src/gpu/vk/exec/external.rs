//! External images on Vulkan: a `dma-buf` imported as an image's memory
//! (`VK_EXT_external_memory_dma_buf` over `VK_KHR_external_memory_fd`),
//! laid out as its producer said (`VK_EXT_image_drm_format_modifier`: the
//! modifier and the plane's offset and pitch), and sampled. No pixel is
//! copied.
//!
//! Single-plane formats are imported; NV12, and any buffer the device
//! refuses, is drawn from its pixels read on the CPU (uploaded as an image).
//! Imports are cached by the image's identity; the images the last frame
//! drew are held until the next one is recorded (one frame is in flight).

// Off Linux and Android (MoltenVK) there are no `dma-buf`s to import.
#![cfg_attr(
    not(any(target_os = "linux", target_os = "android")),
    allow(dead_code, unused_imports)
)]

use super::*;
use crate::platform::external::{DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR, Handle as Ext};
use crate::render::{ExternalFormat, ExternalImage};

/// The instance extensions the device ones below need on a 1.0 instance.
pub(super) const INSTANCE_EXTS: &[&CStr] = &[
    c"VK_KHR_get_physical_device_properties2",
    c"VK_KHR_external_memory_capabilities",
];

/// The device extensions importing a `dma-buf` takes (each enabled where
/// the device has it): external memory and its fd and `dma-buf` handles,
/// dedicated allocations, and DRM format modifiers with what they build on.
pub(super) const DEVICE_EXTS: &[&CStr] = &[
    c"VK_KHR_external_memory",
    c"VK_KHR_external_memory_fd",
    c"VK_EXT_external_memory_dma_buf",
    c"VK_KHR_get_memory_requirements2",
    c"VK_KHR_dedicated_allocation",
    c"VK_KHR_maintenance1",
    c"VK_KHR_bind_memory2",
    c"VK_KHR_image_format_list",
    c"VK_KHR_sampler_ycbcr_conversion",
    c"VK_EXT_image_drm_format_modifier",
];

const ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO: i32 = 1_000_072_001;
const ST_IMPORT_MEMORY_FD_INFO: i32 = 1_000_074_000;
const ST_MEMORY_FD_PROPERTIES: i32 = 1_000_074_001;
const ST_MEMORY_DEDICATED_ALLOCATE_INFO: i32 = 1_000_127_001;
const ST_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO: i32 = 1_000_158_004;
const TILING_DRM_FORMAT_MODIFIER: u32 = 1_000_158_000;
const HANDLE_DMA_BUF: u32 = 0x200;
const LAYOUT_GENERAL: u32 = 1;
const ACCESS_SHADER_READ: u32 = 0x20;
/// `VK_QUEUE_FAMILY_EXTERNAL`: the image's memory comes from outside.
const QUEUE_FAMILY_EXTERNAL: u32 = !1;
const SWIZZLE_IDENTITY: u32 = 0;
const SWIZZLE_ONE: u32 = 6;

#[repr(C)]
struct ExternalMemoryImageCreateInfo {
    s_type: i32,
    p_next: *const c_void,
    handle_types: u32,
}

#[repr(C)]
struct SubresourceLayout {
    offset: u64,
    size: u64,
    row_pitch: u64,
    array_pitch: u64,
    depth_pitch: u64,
}

#[repr(C)]
struct DrmFormatModifierExplicitCreateInfo {
    s_type: i32,
    p_next: *const c_void,
    modifier: u64,
    plane_count: u32,
    planes: *const SubresourceLayout,
}

#[repr(C)]
struct ImportMemoryFdInfo {
    s_type: i32,
    p_next: *const c_void,
    handle_type: u32,
    fd: i32,
}

#[repr(C)]
struct MemoryDedicatedAllocateInfo {
    s_type: i32,
    p_next: *const c_void,
    image: VkImage,
    buffer: Handle,
}

#[repr(C)]
pub(super) struct MemoryFdProperties {
    s_type: i32,
    p_next: *mut c_void,
    memory_type_bits: u32,
}

/// `vkGetMemoryFdPropertiesKHR`.
pub(super) type GetMemoryFdProperties =
    unsafe extern "system" fn(VkDevice, u32, i32, *mut MemoryFdProperties) -> VkResult;

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
}
const F_DUPFD_CLOEXEC: i32 = 1030;

/// One external image's import.
pub(super) struct Import {
    image: Weak<ExternalImage>,
    /// The image, its memory and view; `None` if it couldn't be imported.
    made: Option<(VkImage, VkDeviceMemory, VkImageView)>,
    used: u64,
}

/// What a frame's external images are drawn from.
#[derive(Clone, Copy, Debug)]
pub(super) enum ExtView {
    Imported(VkImageView),
    /// Its pixels, uploaded as [`Recording::images`]`[i]`.
    Image(usize),
    /// Nothing (it can't be imported or read).
    Nothing,
}

/// The executor's imports.
#[derive(Default)]
pub(super) struct Imports {
    /// `VK_EXT_external_memory_dma_buf` and `VK_EXT_image_drm_format_modifier`
    /// are on, with `vkGetMemoryFdPropertiesKHR`.
    pub get_fd_properties: Option<GetMemoryFdProperties>,
    pub dedicated: bool,
    pub(super) map: HashMap<usize, Import>,
    pub(super) held: Vec<Arc<ExternalImage>>,
}

impl Imports {
    /// The formats imported without a copy.
    pub(super) fn formats(&self) -> &'static [ExternalFormat] {
        if self.get_fd_properties.is_some() {
            &[
                ExternalFormat::Rgba8,
                ExternalFormat::Bgra8,
                ExternalFormat::Rgbx8,
                ExternalFormat::Bgrx8,
            ]
        } else {
            &[]
        }
    }
}

impl VkExecutor {
    /// Import (or find imported) each of the frame's external images, before
    /// its textures are uploaded: the view each is drawn from, the pixels of
    /// those that can't be imported added to `rec.images`. Images gone or
    /// not drawn lately are freed; the last frame's are let go of.
    pub(super) unsafe fn import_externals(&mut self, rec: &mut Recording) -> Vec<ExtView> {
        unsafe {
            let count = self.count;
            self.externals.held.clear();
            let gone: Vec<usize> = self
                .externals
                .map
                .iter()
                .filter(|(_, i)| i.image.strong_count() == 0 || count.saturating_sub(i.used) >= 120)
                .map(|(k, _)| *k)
                .collect();
            for k in gone {
                let i = self.externals.map.remove(&k).unwrap();
                self.free_import(i);
            }
            crate::platform::external::note_native(self.externals.formats());
            let mut barriers = Vec::new();
            let mut views = Vec::with_capacity(rec.externals.len());
            for image in rec.externals.clone() {
                let key = Arc::as_ptr(&image) as usize;
                if self
                    .externals
                    .map
                    .get(&key)
                    .is_some_and(|i| !i.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, &image)))
                {
                    let i = self.externals.map.remove(&key).unwrap();
                    self.free_import(i);
                }
                if !self.externals.map.contains_key(&key) {
                    let made = self.import(&image);
                    if let Some((img, _, _)) = made {
                        barriers.push(VkImageMemoryBarrier {
                            sType: ST_IMAGE_MEMORY_BARRIER,
                            pNext: std::ptr::null(),
                            srcAccessMask: 0,
                            dstAccessMask: ACCESS_SHADER_READ,
                            oldLayout: LAYOUT_GENERAL,
                            newLayout: LAYOUT_SHADER_READ_ONLY,
                            srcQueueFamilyIndex: QUEUE_FAMILY_EXTERNAL,
                            dstQueueFamilyIndex: self.family,
                            image: img,
                            subresourceRange: subresource(ASPECT_COLOR),
                        });
                    }
                    self.externals.map.insert(
                        key,
                        Import {
                            image: Arc::downgrade(&image),
                            made,
                            used: count,
                        },
                    );
                }
                let i = self.externals.map.get_mut(&key).unwrap();
                i.used = count;
                self.externals.held.push(image.clone());
                views.push(match i.made {
                    Some((_, _, view)) => ExtView::Imported(view),
                    None => match image.pixmap() {
                        Some(p)
                            if p.size().width as i32 <= self.max_size
                                && p.size().height as i32 <= self.max_size =>
                        {
                            rec.images.push(p);
                            ExtView::Image(rec.images.len() - 1)
                        }
                        _ => ExtView::Nothing,
                    },
                });
            }
            if !barriers.is_empty() {
                self.barrier(&barriers);
            }
            views
        }
    }

    /// Import `image`'s `dma-buf` as a sampled image, or `None`.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    unsafe fn import(
        &mut self,
        _: &ExternalImage,
    ) -> Option<(VkImage, VkDeviceMemory, VkImageView)> {
        None
    }

    /// Import `image`'s `dma-buf` as a sampled image, or `None`.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe fn import(
        &mut self,
        image: &ExternalImage,
    ) -> Option<(VkImage, VkDeviceMemory, VkImageView)> {
        let get_props = self.externals.get_fd_properties?;
        let Ext::DmaBuf(_) = image.handle() else {
            return None;
        };
        let (format, opaque) = match image.format() {
            ExternalFormat::Rgba8 => (FORMAT_R8G8B8A8_UNORM, false),
            ExternalFormat::Rgbx8 => (FORMAT_R8G8B8A8_UNORM, true),
            ExternalFormat::Bgra8 => (FORMAT_B8G8R8A8_UNORM, false),
            ExternalFormat::Bgrx8 => (FORMAT_B8G8R8A8_UNORM, true),
            ExternalFormat::Nv12 => return None,
        };
        let [(fd, offset, stride)] = image.dma_buf_planes()?[..] else {
            return None;
        };
        image.wait_acquire();
        let size = image.size();
        let modifier = match image.modifier() {
            DRM_FORMAT_MOD_INVALID => DRM_FORMAT_MOD_LINEAR,
            m => m,
        };
        unsafe {
            let d = self.device;
            let plane = SubresourceLayout {
                offset,
                size: 0,
                row_pitch: stride as u64,
                array_pitch: 0,
                depth_pitch: 0,
            };
            let explicit = DrmFormatModifierExplicitCreateInfo {
                s_type: ST_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO,
                p_next: std::ptr::null(),
                modifier,
                plane_count: 1,
                planes: &plane,
            };
            let external = ExternalMemoryImageCreateInfo {
                s_type: ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
                p_next: (&explicit as *const DrmFormatModifierExplicitCreateInfo).cast(),
                handle_types: HANDLE_DMA_BUF,
            };
            let ci = VkImageCreateInfo {
                sType: ST_IMAGE_CREATE_INFO,
                pNext: (&external as *const ExternalMemoryImageCreateInfo).cast(),
                flags: 0,
                imageType: IMAGE_TYPE_2D,
                format,
                extent: VkExtent3D {
                    width: size.width,
                    height: size.height,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: SAMPLE_COUNT_1,
                tiling: TILING_DRM_FORMAT_MODIFIER,
                usage: USAGE_SAMPLED,
                sharingMode: SHARING_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: std::ptr::null(),
                initialLayout: LAYOUT_UNDEFINED,
            };
            let mut img = NULL;
            if (self.vk.vkCreateImage)(d, &ci, std::ptr::null(), &mut img) != VK_SUCCESS {
                return None;
            }
            let mut req = VkMemoryRequirements::default();
            (self.vk.vkGetImageMemoryRequirements)(d, img, &mut req);
            let mut props = MemoryFdProperties {
                s_type: ST_MEMORY_FD_PROPERTIES,
                p_next: std::ptr::null_mut(),
                memory_type_bits: 0,
            };
            let fail = |vk: &Fns| {
                (vk.vkDestroyImage)(d, img, std::ptr::null());
                None
            };
            if get_props(d, HANDLE_DMA_BUF, fd, &mut props) != VK_SUCCESS {
                return fail(&self.vk);
            }
            let Some(type_index) = self.memory_type(req.memoryTypeBits & props.memory_type_bits, 0)
            else {
                return fail(&self.vk);
            };
            // The allocation takes the descriptor it imports: a duplicate.
            let dup = fcntl(fd, F_DUPFD_CLOEXEC, 0);
            if dup < 0 {
                return fail(&self.vk);
            }
            let dedicated = MemoryDedicatedAllocateInfo {
                s_type: ST_MEMORY_DEDICATED_ALLOCATE_INFO,
                p_next: std::ptr::null(),
                image: img,
                buffer: NULL,
            };
            let import = ImportMemoryFdInfo {
                s_type: ST_IMPORT_MEMORY_FD_INFO,
                p_next: if self.externals.dedicated {
                    (&dedicated as *const MemoryDedicatedAllocateInfo).cast()
                } else {
                    std::ptr::null()
                },
                handle_type: HANDLE_DMA_BUF,
                fd: dup,
            };
            let ai = VkMemoryAllocateInfo {
                sType: ST_MEMORY_ALLOCATE_INFO,
                pNext: (&import as *const ImportMemoryFdInfo).cast(),
                allocationSize: req.size,
                memoryTypeIndex: type_index,
            };
            let mut mem = NULL;
            if (self.vk.vkAllocateMemory)(d, &ai, std::ptr::null(), &mut mem) != VK_SUCCESS {
                close(dup);
                return fail(&self.vk);
            }
            if (self.vk.vkBindImageMemory)(d, img, mem, 0) != VK_SUCCESS {
                (self.vk.vkFreeMemory)(d, mem, std::ptr::null());
                return fail(&self.vk);
            }
            let alpha = if opaque {
                SWIZZLE_ONE
            } else {
                SWIZZLE_IDENTITY
            };
            let vi = VkImageViewCreateInfo {
                sType: ST_IMAGE_VIEW_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                image: img,
                viewType: VIEW_TYPE_2D,
                format,
                components: VkComponentMapping {
                    r: SWIZZLE_IDENTITY,
                    g: SWIZZLE_IDENTITY,
                    b: SWIZZLE_IDENTITY,
                    a: alpha,
                },
                subresourceRange: subresource(ASPECT_COLOR),
            };
            let mut view = NULL;
            if (self.vk.vkCreateImageView)(d, &vi, std::ptr::null(), &mut view) != VK_SUCCESS {
                (self.vk.vkFreeMemory)(d, mem, std::ptr::null());
                return fail(&self.vk);
            }
            Some((img, mem, view))
        }
    }

    unsafe fn free_import(&mut self, i: Import) {
        if let Some((img, mem, view)) = i.made {
            unsafe {
                let d = self.device;
                (self.vk.vkDestroyImageView)(d, view, std::ptr::null());
                (self.vk.vkDestroyImage)(d, img, std::ptr::null());
                (self.vk.vkFreeMemory)(d, mem, std::ptr::null());
            }
        }
    }

    /// Free every import (the device is going away; it's idle).
    pub(super) unsafe fn free_imports(&mut self) {
        self.externals.held.clear();
        for (_, i) in std::mem::take(&mut self.externals.map) {
            unsafe { self.free_import(i) };
        }
    }
}
