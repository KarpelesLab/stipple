//! Presenting to a Wayland window without reading back: each frame is
//! copied from the frame target into an image whose memory is a `dma-buf`
//! (`VK_KHR_external_memory_fd` with `VK_EXT_external_memory_dma_buf`), laid
//! out with a modifier the compositor takes (`VK_EXT_image_drm_format_modifier`,
//! else a linear image), and handed to the window's sink, which attaches it
//! through `zwp_linux_dmabuf_v1` (see [`crate::gpu::dmabuf_present`]).
//!
//! The extensions need Vulkan 1.1 (external memory, dedicated allocations,
//! `vkGetPhysicalDeviceFormatProperties2` are core there): the instance asks
//! for 1.1 where the loader has it. The frame waits for its fence before the
//! commit, so the compositor reads a finished buffer whether or not the
//! driver synchronizes the `dma-buf` implicitly; the image is handed over
//! with a release to `VK_QUEUE_FAMILY_EXTERNAL`.

#![allow(non_snake_case)]

use super::super::sys::*;
use super::{VkExecutor, layers, subresource};
use crate::gpu::dmabuf_present::{self as dp, Layout, Ring, Slot};
use crate::gpu::renderer::GpuError;
use crate::platform::dmabuf::{Buffer, MOD_LINEAR, Plane, Present};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};

const VK_KHR_EXTERNAL_MEMORY_FD: &CStr = c"VK_KHR_external_memory_fd";
const VK_EXT_EXTERNAL_MEMORY_DMA_BUF: &CStr = c"VK_EXT_external_memory_dma_buf";
const VK_EXT_IMAGE_DRM_FORMAT_MODIFIER: &CStr = c"VK_EXT_image_drm_format_modifier";
const VK_KHR_IMAGE_FORMAT_LIST: &CStr = c"VK_KHR_image_format_list";

pub(super) const API_VERSION_1_1: u32 = (1 << 22) | (1 << 12);

const ST_FORMAT_PROPERTIES_2: i32 = 1_000_059_002;
const ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO: i32 = 1_000_072_001;
const ST_EXPORT_MEMORY_ALLOCATE_INFO: i32 = 1_000_072_002;
const ST_MEMORY_GET_FD_INFO_KHR: i32 = 1_000_074_002;
const ST_MEMORY_DEDICATED_ALLOCATE_INFO: i32 = 1_000_127_001;
const ST_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT: i32 = 1_000_158_000;
const ST_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT: i32 = 1_000_158_003;
const ST_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT: i32 = 1_000_158_005;

const HANDLE_TYPE_DMA_BUF: u32 = 0x200;
const TILING_LINEAR: u32 = 1;
const TILING_DRM_FORMAT_MODIFIER: u32 = 1_000_158_000;
/// `VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT_EXT`; plane `i` is this `<< i`.
const ASPECT_MEMORY_PLANE_0: u32 = 0x80;
/// `VK_FORMAT_FEATURE_TRANSFER_DST_BIT` (core in 1.1).
const FEATURE_TRANSFER_DST: u32 = 0x4000;
const LAYOUT_GENERAL: u32 = 1;
const QUEUE_FAMILY_EXTERNAL: u32 = !1;

#[repr(C)]
struct VkExternalMemoryImageCreateInfo {
    sType: i32,
    pNext: *const c_void,
    handleTypes: u32,
}

#[repr(C)]
struct VkExportMemoryAllocateInfo {
    sType: i32,
    pNext: *const c_void,
    handleTypes: u32,
}

#[repr(C)]
struct VkMemoryDedicatedAllocateInfo {
    sType: i32,
    pNext: *const c_void,
    image: VkImage,
    buffer: VkBuffer,
}

#[repr(C)]
struct VkMemoryGetFdInfoKHR {
    sType: i32,
    pNext: *const c_void,
    memory: VkDeviceMemory,
    handleType: u32,
}

#[repr(C)]
struct VkFormatProperties2 {
    sType: i32,
    pNext: *mut c_void,
    formatProperties: VkFormatProperties,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkDrmFormatModifierPropertiesEXT {
    drmFormatModifier: u64,
    drmFormatModifierPlaneCount: u32,
    drmFormatModifierTilingFeatures: u32,
}

#[repr(C)]
struct VkDrmFormatModifierPropertiesListEXT {
    sType: i32,
    pNext: *mut c_void,
    drmFormatModifierCount: u32,
    pDrmFormatModifierProperties: *mut VkDrmFormatModifierPropertiesEXT,
}

#[repr(C)]
struct VkImageDrmFormatModifierListCreateInfoEXT {
    sType: i32,
    pNext: *const c_void,
    drmFormatModifierCount: u32,
    pDrmFormatModifiers: *const u64,
}

#[repr(C)]
struct VkImageDrmFormatModifierPropertiesEXT {
    sType: i32,
    pNext: *mut c_void,
    drmFormatModifier: u64,
}

#[repr(C)]
struct VkImageSubresource {
    aspectMask: u32,
    mipLevel: u32,
    arrayLayer: u32,
}

#[repr(C)]
#[derive(Default)]
struct VkSubresourceLayout {
    offset: u64,
    size: u64,
    rowPitch: u64,
    arrayPitch: u64,
    depthPitch: u64,
}

type P = *const c_void;
type GetFormatProperties2 =
    unsafe extern "system" fn(VkPhysicalDevice, u32, *mut VkFormatProperties2);
type GetMemoryFd =
    unsafe extern "system" fn(VkDevice, *const VkMemoryGetFdInfoKHR, *mut i32) -> VkResult;
type GetSubresourceLayout = unsafe extern "system" fn(
    VkDevice,
    VkImage,
    *const VkImageSubresource,
    *mut VkSubresourceLayout,
);
type GetModifierProperties = unsafe extern "system" fn(
    VkDevice,
    VkImage,
    *mut VkImageDrmFormatModifierPropertiesEXT,
) -> VkResult;
type GetInstanceProcAddr = unsafe extern "system" fn(VkInstance, *const c_char) -> P;
type EnumerateInstanceVersion = unsafe extern "system" fn(*mut u32) -> VkResult;

unsafe extern "C" {
    fn close(fd: i32) -> i32;
}

/// The API version to ask the instance for: 1.1 where the loader has it
/// (the export needs it), else 1.0.
pub(super) unsafe fn instance_version(gipa: GetInstanceProcAddr) -> u32 {
    unsafe {
        let p = gipa(std::ptr::null_mut(), c"vkEnumerateInstanceVersion".as_ptr());
        if p.is_null() {
            return API_VERSION_1_0;
        }
        let f = std::mem::transmute::<P, EnumerateInstanceVersion>(p);
        let mut v = 0;
        if f(&mut v) == VK_SUCCESS && v >= API_VERSION_1_1 {
            API_VERSION_1_1
        } else {
            API_VERSION_1_0
        }
    }
}

/// What the device can do, from its extensions (it has Vulkan 1.1).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Support {
    export: bool,
    modifiers: bool,
}

/// The device extensions to turn on for the export, if the instance and
/// the device (`device_api`) have 1.1 and `has` names them all.
pub(super) fn device_extensions(
    instance_api: u32,
    device_api: u32,
    has: impl Fn(&CStr) -> bool,
) -> (Vec<*const c_char>, Support) {
    let mut exts = Vec::new();
    if instance_api < API_VERSION_1_1
        || device_api < API_VERSION_1_1
        || !has(VK_KHR_EXTERNAL_MEMORY_FD)
        || !has(VK_EXT_EXTERNAL_MEMORY_DMA_BUF)
    {
        return (exts, Support::default());
    }
    exts.push(VK_KHR_EXTERNAL_MEMORY_FD.as_ptr());
    exts.push(VK_EXT_EXTERNAL_MEMORY_DMA_BUF.as_ptr());
    let modifiers = has(VK_EXT_IMAGE_DRM_FORMAT_MODIFIER) && has(VK_KHR_IMAGE_FORMAT_LIST);
    if modifiers {
        exts.push(VK_EXT_IMAGE_DRM_FORMAT_MODIFIER.as_ptr());
        exts.push(VK_KHR_IMAGE_FORMAT_LIST.as_ptr());
    }
    (
        exts,
        Support {
            export: true,
            modifiers,
        },
    )
}

/// The export's entry points.
struct Fns {
    format_properties2: GetFormatProperties2,
    memory_fd: GetMemoryFd,
    subresource_layout: GetSubresourceLayout,
    modifier_properties: Option<GetModifierProperties>,
}

/// The format a window's buffers are made in.
#[derive(Clone, Debug)]
struct Choice {
    fourcc: u32,
    layout: Layout,
    format: u32,
    /// The modifiers both the compositor and the driver take, the driver's
    /// best first (LINEAR alone without the modifier extension), and their
    /// plane counts.
    modifiers: Vec<(u64, u32)>,
}

/// An exported image: the image, its memory, and the `dma-buf` (ours).
pub(super) struct Exported {
    image: VkImage,
    memory: VkDeviceMemory,
    fd: i32,
}

/// The export state: entry points, the format chosen, the windows' rings.
#[derive(Default)]
pub(super) struct Wayland {
    fns: Option<Fns>,
    support: Support,
    /// The format the compositor and the driver agree on (`None` inside:
    /// none, read back), once worked out.
    choice: RefCell<Option<Option<Choice>>>,
    rings: HashMap<u32, Ring<Exported>>,
    /// The frame swizzled to `[B, G, R, A]`, for buffers in that order, and
    /// its size.
    swizzle: Option<(super::Image, (u32, u32))>,
}

impl Wayland {
    /// Look the entry points up, if the device has the export.
    pub(super) unsafe fn load(
        gipa: GetInstanceProcAddr,
        instance: VkInstance,
        support: Support,
    ) -> Wayland {
        unsafe {
            let get = |name: &CStr| gipa(instance, name.as_ptr());
            let (fp2, fd, layout) = (
                get(c"vkGetPhysicalDeviceFormatProperties2"),
                get(c"vkGetMemoryFdKHR"),
                get(c"vkGetImageSubresourceLayout"),
            );
            let modifier = get(c"vkGetImageDrmFormatModifierPropertiesEXT");
            let fns = (support.export && !fp2.is_null() && !fd.is_null() && !layout.is_null())
                .then(|| Fns {
                    format_properties2: std::mem::transmute::<P, GetFormatProperties2>(fp2),
                    memory_fd: std::mem::transmute::<P, GetMemoryFd>(fd),
                    subresource_layout: std::mem::transmute::<P, GetSubresourceLayout>(layout),
                    modifier_properties: (support.modifiers && !modifier.is_null())
                        .then(|| std::mem::transmute::<P, GetModifierProperties>(modifier)),
                });
            Wayland {
                support: Support {
                    modifiers: support.modifiers
                        && fns
                            .as_ref()
                            .is_some_and(|f| f.modifier_properties.is_some()),
                    ..support
                },
                fns,
                choice: RefCell::new(None),
                rings: HashMap::new(),
                swizzle: None,
            }
        }
    }
}

impl Exported {
    unsafe fn free(self, ex: &VkExecutor) {
        unsafe {
            (ex.vk.vkDestroyImage)(ex.device, self.image, std::ptr::null());
            (ex.vk.vkFreeMemory)(ex.device, self.memory, std::ptr::null());
            close(self.fd);
        }
    }
}

impl VkExecutor {
    /// The modifiers the driver can copy (`Rgba`) or blit (`Bgra`) frames
    /// into for `format`, with their plane counts, best first.
    unsafe fn drawable(&self, format: u32, layout: Layout) -> Vec<(u64, u32)> {
        unsafe {
            let Some(fns) = self.wayland.fns.as_ref() else {
                return Vec::new();
            };
            let need = FEATURE_TRANSFER_DST;
            let mut p = VkFormatProperties::default();
            (self.vk.vkGetPhysicalDeviceFormatProperties)(self.phys, format, &mut p);
            if layout == Layout::Bgra && p.optimalTilingFeatures & FEATURE_BLIT_DST == 0 {
                return Vec::new();
            }
            if !self.wayland.support.modifiers {
                return if p.linearTilingFeatures & need == need {
                    vec![(MOD_LINEAR, 1)]
                } else {
                    Vec::new()
                };
            }
            let mut list = VkDrmFormatModifierPropertiesListEXT {
                sType: ST_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT,
                pNext: std::ptr::null_mut(),
                drmFormatModifierCount: 0,
                pDrmFormatModifierProperties: std::ptr::null_mut(),
            };
            let mut p2 = VkFormatProperties2 {
                sType: ST_FORMAT_PROPERTIES_2,
                pNext: (&mut list as *mut VkDrmFormatModifierPropertiesListEXT).cast(),
                formatProperties: VkFormatProperties::default(),
            };
            (fns.format_properties2)(self.phys, format, &mut p2);
            let mut props = vec![
                VkDrmFormatModifierPropertiesEXT::default();
                list.drmFormatModifierCount as usize
            ];
            list.pDrmFormatModifierProperties = props.as_mut_ptr();
            p2.pNext = (&mut list as *mut VkDrmFormatModifierPropertiesListEXT).cast();
            (fns.format_properties2)(self.phys, format, &mut p2);
            props.truncate(list.drmFormatModifierCount as usize);
            props
                .iter()
                .filter(|m| m.drmFormatModifierTilingFeatures & need == need)
                .map(|m| (m.drmFormatModifier, m.drmFormatModifierPlaneCount.max(1)))
                .collect()
        }
    }

    /// The format to make `sink`'s buffers in, worked out once.
    unsafe fn choice(&self, sink: &dyn Present) -> Option<Choice> {
        unsafe {
            if let Some(c) = &*self.wayland.choice.borrow() {
                return c.clone();
            }
            let choice = if self.wayland.fns.is_none() {
                dp::say_reading_back(
                    "Vulkan",
                    "the device can't export dma-bufs (VK_EXT_external_memory_dma_buf, Vulkan 1.1)",
                );
                None
            } else {
                let offered = sink.formats();
                let mut planes = HashMap::new();
                let picked = dp::choose(&offered, |fourcc, layout| {
                    let format = vk_format(layout);
                    let mods = self.drawable(format, layout);
                    for &(m, n) in &mods {
                        planes.insert((fourcc, m), n);
                    }
                    mods.into_iter().map(|(m, _)| m).collect()
                });
                if picked.is_none() {
                    dp::say_reading_back(
                        "Vulkan",
                        &format!(
                            "no format the compositor takes ({} offered) can be exported",
                            offered.len()
                        ),
                    );
                }
                picked
                    .map(|(fourcc, layout, mods)| Choice {
                        fourcc,
                        layout,
                        format: vk_format(layout),
                        modifiers: mods
                            .into_iter()
                            .map(|m| (m, planes.get(&(fourcc, m)).copied().unwrap_or(1)))
                            .collect(),
                    })
                    .filter(|choice| self.probe(choice, sink))
            };
            *self.wayland.choice.borrow_mut() = Some(choice.clone());
            choice
        }
    }

    /// Whether the driver exports a small image of `choice` and the
    /// compositor imports it.
    unsafe fn probe(&self, choice: &Choice, sink: &dyn Present) -> bool {
        unsafe {
            let imported = self
                .export_image(choice, 64, 64)
                .and_then(|(image, buffer)| {
                    let id = sink.import(&buffer);
                    image.free(self);
                    id
                });
            match imported {
                Ok(id) => {
                    sink.destroy(id);
                    true
                }
                Err(e) => {
                    dp::say_reading_back("Vulkan", &e);
                    false
                }
            }
        }
    }

    /// Whether frames can go to the Wayland window `sink` is for.
    pub(super) unsafe fn can_present_dmabuf(&self, sink: &dyn Present) -> bool {
        unsafe { self.choice(sink).is_some() }
    }

    /// A `w`×`h` image of `choice` in exported memory, and the buffer the
    /// compositor is to see.
    unsafe fn export_image(
        &self,
        choice: &Choice,
        w: u32,
        h: u32,
    ) -> Result<(Exported, Buffer), String> {
        unsafe {
            let fns = self.wayland.fns.as_ref().ok_or("no export")?;
            let d = self.device;
            let external = VkExternalMemoryImageCreateInfo {
                sType: ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
                pNext: std::ptr::null(),
                handleTypes: HANDLE_TYPE_DMA_BUF,
            };
            let mods: Vec<u64> = choice.modifiers.iter().map(|m| m.0).collect();
            let list = VkImageDrmFormatModifierListCreateInfoEXT {
                sType: ST_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT,
                pNext: (&external as *const VkExternalMemoryImageCreateInfo).cast(),
                drmFormatModifierCount: mods.len() as u32,
                pDrmFormatModifiers: mods.as_ptr(),
            };
            let modifiers = self.wayland.support.modifiers;
            let ci = VkImageCreateInfo {
                sType: ST_IMAGE_CREATE_INFO,
                pNext: if modifiers {
                    (&list as *const VkImageDrmFormatModifierListCreateInfoEXT).cast()
                } else {
                    (&external as *const VkExternalMemoryImageCreateInfo).cast()
                },
                flags: 0,
                imageType: IMAGE_TYPE_2D,
                format: choice.format,
                extent: VkExtent3D {
                    width: w,
                    height: h,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: SAMPLE_COUNT_1,
                tiling: if modifiers {
                    TILING_DRM_FORMAT_MODIFIER
                } else {
                    TILING_LINEAR
                },
                usage: USAGE_TRANSFER_DST,
                sharingMode: SHARING_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: std::ptr::null(),
                initialLayout: LAYOUT_UNDEFINED,
            };
            let mut image = NULL;
            let r = (self.vk.vkCreateImage)(d, &ci, std::ptr::null(), &mut image);
            if r != VK_SUCCESS {
                return Err(format!("vkCreateImage (exportable) failed ({r})"));
            }
            let mut req = VkMemoryRequirements::default();
            (self.vk.vkGetImageMemoryRequirements)(d, image, &mut req);
            let Some(type_index) = self
                .memory_type(req.memoryTypeBits, MEMORY_DEVICE_LOCAL)
                .or_else(|| self.memory_type(req.memoryTypeBits, 0))
            else {
                (self.vk.vkDestroyImage)(d, image, std::ptr::null());
                return Err("no memory type for an exportable image".into());
            };
            let dedicated = VkMemoryDedicatedAllocateInfo {
                sType: ST_MEMORY_DEDICATED_ALLOCATE_INFO,
                pNext: std::ptr::null(),
                image,
                buffer: NULL,
            };
            let export = VkExportMemoryAllocateInfo {
                sType: ST_EXPORT_MEMORY_ALLOCATE_INFO,
                pNext: (&dedicated as *const VkMemoryDedicatedAllocateInfo).cast(),
                handleTypes: HANDLE_TYPE_DMA_BUF,
            };
            let ai = VkMemoryAllocateInfo {
                sType: ST_MEMORY_ALLOCATE_INFO,
                pNext: (&export as *const VkExportMemoryAllocateInfo).cast(),
                allocationSize: req.size,
                memoryTypeIndex: type_index,
            };
            let mut memory = NULL;
            let r = (self.vk.vkAllocateMemory)(d, &ai, std::ptr::null(), &mut memory);
            if r != VK_SUCCESS {
                (self.vk.vkDestroyImage)(d, image, std::ptr::null());
                return Err(format!("vkAllocateMemory (exportable) failed ({r})"));
            }
            let fail = |what: String| {
                (self.vk.vkDestroyImage)(d, image, std::ptr::null());
                (self.vk.vkFreeMemory)(d, memory, std::ptr::null());
                what
            };
            let r = (self.vk.vkBindImageMemory)(d, image, memory, 0);
            if r != VK_SUCCESS {
                return Err(fail(format!("vkBindImageMemory failed ({r})")));
            }
            let gi = VkMemoryGetFdInfoKHR {
                sType: ST_MEMORY_GET_FD_INFO_KHR,
                pNext: std::ptr::null(),
                memory,
                handleType: HANDLE_TYPE_DMA_BUF,
            };
            let mut fd = -1;
            let r = (fns.memory_fd)(d, &gi, &mut fd);
            if r != VK_SUCCESS || fd < 0 {
                return Err(fail(format!("vkGetMemoryFdKHR failed ({r})")));
            }
            let exported = Exported { image, memory, fd };
            let (modifier, planes) = match fns.modifier_properties.filter(|_| modifiers) {
                Some(get) => {
                    let mut p = VkImageDrmFormatModifierPropertiesEXT {
                        sType: ST_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT,
                        pNext: std::ptr::null_mut(),
                        drmFormatModifier: 0,
                    };
                    let r = get(d, image, &mut p);
                    if r != VK_SUCCESS {
                        exported.free(self);
                        return Err(format!(
                            "vkGetImageDrmFormatModifierPropertiesEXT failed ({r})"
                        ));
                    }
                    let n = choice
                        .modifiers
                        .iter()
                        .find(|m| m.0 == p.drmFormatModifier)
                        .map_or(1, |m| m.1);
                    (p.drmFormatModifier, n)
                }
                None => (MOD_LINEAR, 1),
            };
            let planes = (0..planes.min(4))
                .map(|i| {
                    let sub = VkImageSubresource {
                        aspectMask: if modifiers {
                            ASPECT_MEMORY_PLANE_0 << i
                        } else {
                            ASPECT_COLOR
                        },
                        mipLevel: 0,
                        arrayLayer: 0,
                    };
                    let mut l = VkSubresourceLayout::default();
                    (fns.subresource_layout)(d, image, &sub, &mut l);
                    Plane {
                        fd,
                        offset: l.offset as u32,
                        stride: l.rowPitch as u32,
                    }
                })
                .collect();
            Ok((
                exported,
                Buffer {
                    width: w,
                    height: h,
                    fourcc: choice.fourcc,
                    modifier,
                    planes,
                },
            ))
        }
    }

    /// Draw `plan`, copy the frame into a free buffer of the window's ring,
    /// wait for it and commit it.
    pub(super) unsafe fn present_dmabuf(
        &mut self,
        plan: &mut crate::gpu::ops::Plan,
        surface: u32,
        sink: &dyn Present,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        unsafe {
            if !sink.ready() {
                return Err(GpuError::Skipped);
            }
            let choice = self.choice(sink).ok_or(GpuError::Skipped)?;
            self.wait()?;
            let (w, h) = self.check_size(plan)?;
            let size = (w as u32, h as u32);
            if self
                .wayland
                .rings
                .get(&surface)
                .is_some_and(|r| r.size != size)
            {
                let mut old = self.wayland.rings.remove(&surface).unwrap();
                old.clear(Some(sink), |e| e.free(self));
            }
            let mut ring = self
                .wayland
                .rings
                .remove(&surface)
                .unwrap_or_else(|| Ring::new(size));
            let r = self.frame_into(&mut ring, &choice, plan, surface, sink, dirty);
            if r.is_err() {
                self.kept.forget();
            }
            self.wayland.rings.insert(surface, ring);
            r
        }
    }

    unsafe fn frame_into(
        &mut self,
        ring: &mut Ring<Exported>,
        choice: &Choice,
        plan: &mut crate::gpu::ops::Plan,
        surface: u32,
        sink: &dyn Present,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        unsafe {
            let (w, h) = ring.size;
            let i = ring.acquire(sink, || {
                let made = self.export_image(choice, w, h);
                let (image, buffer) = match made {
                    Ok(m) => m,
                    Err(e) => {
                        // Read back from the next frame on.
                        dp::say_reading_back("Vulkan", &e);
                        *self.wayland.choice.borrow_mut() = Some(None);
                        return Err(GpuError::Skipped);
                    }
                };
                match sink.import(&buffer) {
                    Ok(id) => {
                        dp::say_presenting(
                            "Vulkan",
                            buffer.fourcc,
                            buffer.modifier,
                            buffer.planes.len(),
                        );
                        Ok(Slot { image, id })
                    }
                    Err(e) => {
                        image.free(self);
                        dp::say_reading_back("Vulkan", &e);
                        *self.wayland.choice.borrow_mut() = Some(None);
                        Err(GpuError::Skipped)
                    }
                }
            })?;
            let target = ring.slots[i].image.image;
            // Only what changed, over the window's last frame, when the
            // frame target holds it (it's all copied into the buffer below).
            let key = surface as usize;
            if self
                .frame
                .as_ref()
                .is_some_and(|t| (t.w as u32, t.h as u32) == (w, h))
            {
                self.kept.limit(key, plan, dirty);
            }
            let encoded = self.encode(plan);
            if encoded.is_err() {
                self.end_frame();
                encoded?;
            }
            // The buffer's old contents go: all of the frame is copied in.
            let mut barrier = VkImageMemoryBarrier {
                sType: ST_IMAGE_MEMORY_BARRIER,
                pNext: std::ptr::null(),
                srcAccessMask: 0,
                dstAccessMask: ACCESS_MEMORY_READ | ACCESS_MEMORY_WRITE,
                oldLayout: LAYOUT_UNDEFINED,
                newLayout: LAYOUT_TRANSFER_DST,
                srcQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
                dstQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
                image: target,
                subresourceRange: subresource(ASPECT_COLOR),
            };
            self.barrier(std::slice::from_ref(&barrier));
            let frame = self.frame.as_ref().unwrap().color.image;
            // A swizzle goes through an image of the buffer's format (some
            // drivers can't blit into a linear image), then the same copy.
            let source = match choice.layout {
                Layout::Rgba => frame,
                Layout::Bgra => self.swizzle(frame, choice.format, w, h)?,
            };
            let region = VkImageCopy {
                srcSubresource: layers(ASPECT_COLOR),
                srcOffset: VkOffset3D::default(),
                dstSubresource: layers(ASPECT_COLOR),
                dstOffset: VkOffset3D::default(),
                extent: VkExtent3D {
                    width: w,
                    height: h,
                    depth: 1,
                },
            };
            (self.vk.vkCmdCopyImage)(
                self.cmd,
                source,
                LAYOUT_TRANSFER_SRC,
                target,
                LAYOUT_TRANSFER_DST,
                1,
                &region,
            );
            // Handed over to the compositor.
            barrier.srcAccessMask = ACCESS_MEMORY_WRITE;
            barrier.dstAccessMask = 0;
            barrier.oldLayout = LAYOUT_TRANSFER_DST;
            barrier.newLayout = LAYOUT_GENERAL;
            barrier.srcQueueFamilyIndex = self.family;
            barrier.dstQueueFamilyIndex = QUEUE_FAMILY_EXTERNAL;
            self.barrier(std::slice::from_ref(&barrier));
            let submitted = self.submit(None, None);
            self.end_frame();
            submitted?;
            // The compositor reads it once the GPU is done with it.
            self.wait()?;
            self.kept.presented(key, (w, h), dirty);
            let damage = ring.damage(dirty);
            sink.commit(ring.slots[i].id, (w, h), damage)
                .map_err(|e| GpuError::Lost(format!("committing the frame: {e}")))
        }
    }

    /// Blit the frame (`frame`, readable by transfers) into the swizzle
    /// image of `format`, made or remade at `w`×`h`, and leave that readable.
    unsafe fn swizzle(
        &mut self,
        frame: VkImage,
        format: u32,
        w: u32,
        h: u32,
    ) -> Result<VkImage, GpuError> {
        unsafe {
            if self.wayland.swizzle.as_ref().is_some_and(|s| s.1 != (w, h)) {
                let (old, _) = self.wayland.swizzle.take().unwrap();
                self.free_image(old);
            }
            if self.wayland.swizzle.is_none() {
                let img = self.image(
                    format,
                    w as i32,
                    h as i32,
                    USAGE_TRANSFER_SRC | USAGE_TRANSFER_DST,
                    ASPECT_COLOR,
                    false,
                )?;
                self.wayland.swizzle = Some((img, (w, h)));
            }
            let image = self.wayland.swizzle.as_ref().unwrap().0.image;
            let mut barrier = VkImageMemoryBarrier {
                sType: ST_IMAGE_MEMORY_BARRIER,
                pNext: std::ptr::null(),
                srcAccessMask: 0,
                dstAccessMask: ACCESS_MEMORY_READ | ACCESS_MEMORY_WRITE,
                oldLayout: LAYOUT_UNDEFINED,
                newLayout: LAYOUT_TRANSFER_DST,
                srcQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
                dstQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
                image,
                subresourceRange: subresource(ASPECT_COLOR),
            };
            self.barrier(std::slice::from_ref(&barrier));
            let end = VkOffset3D {
                x: w as i32,
                y: h as i32,
                z: 1,
            };
            let region = VkImageBlit {
                srcSubresource: layers(ASPECT_COLOR),
                srcOffsets: [VkOffset3D::default(), end],
                dstSubresource: layers(ASPECT_COLOR),
                dstOffsets: [VkOffset3D::default(), end],
            };
            (self.vk.vkCmdBlitImage)(
                self.cmd,
                frame,
                LAYOUT_TRANSFER_SRC,
                image,
                LAYOUT_TRANSFER_DST,
                1,
                &region,
                FILTER_NEAREST,
            );
            barrier.srcAccessMask = ACCESS_MEMORY_WRITE;
            barrier.oldLayout = LAYOUT_TRANSFER_DST;
            barrier.newLayout = LAYOUT_TRANSFER_SRC;
            self.barrier(std::slice::from_ref(&barrier));
            Ok(image)
        }
    }

    /// Let go of the Wayland window `surface`'s ring.
    pub(super) unsafe fn release_dmabuf(&mut self, surface: u32, sink: &dyn Present) {
        unsafe {
            if let Some(mut ring) = self.wayland.rings.remove(&surface) {
                (self.vk.vkDeviceWaitIdle)(self.device);
                ring.clear(Some(sink), |e| e.free(self));
            }
        }
    }

    /// Free every ring's images (the device is going away; the windows'
    /// buffers go with their windows).
    pub(super) unsafe fn drop_dmabufs(&mut self) {
        unsafe {
            for (_, mut ring) in std::mem::take(&mut self.wayland.rings) {
                ring.clear(None, |e| e.free(self));
            }
            if let Some((img, _)) = self.wayland.swizzle.take() {
                self.free_image(img);
            }
        }
    }
}

/// The Vulkan format a layout is drawn in.
fn vk_format(layout: Layout) -> u32 {
    match layout {
        Layout::Rgba => FORMAT_R8G8B8A8_UNORM,
        Layout::Bgra => FORMAT_B8G8R8A8_UNORM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_c_headers() {
        if std::mem::size_of::<usize>() != 8 {
            return;
        }
        assert_eq!(std::mem::size_of::<VkExternalMemoryImageCreateInfo>(), 24);
        assert_eq!(std::mem::size_of::<VkExportMemoryAllocateInfo>(), 24);
        assert_eq!(std::mem::size_of::<VkMemoryDedicatedAllocateInfo>(), 32);
        assert_eq!(std::mem::size_of::<VkMemoryGetFdInfoKHR>(), 32);
        assert_eq!(std::mem::size_of::<VkFormatProperties2>(), 32);
        assert_eq!(std::mem::size_of::<VkDrmFormatModifierPropertiesEXT>(), 16);
        assert_eq!(
            std::mem::size_of::<VkDrmFormatModifierPropertiesListEXT>(),
            32
        );
        assert_eq!(
            std::mem::size_of::<VkImageDrmFormatModifierListCreateInfoEXT>(),
            32
        );
        assert_eq!(
            std::mem::size_of::<VkImageDrmFormatModifierPropertiesEXT>(),
            24
        );
        assert_eq!(std::mem::size_of::<VkImageSubresource>(), 12);
        assert_eq!(std::mem::size_of::<VkSubresourceLayout>(), 40);
    }

    #[test]
    fn the_export_needs_1_1_and_both_extensions() {
        let all = |_: &CStr| true;
        let (exts, s) = device_extensions(API_VERSION_1_1, API_VERSION_1_1, all);
        assert!(s.export && s.modifiers);
        assert_eq!(exts.len(), 4);
        let (exts, s) = device_extensions(API_VERSION_1_0, API_VERSION_1_1, all);
        assert!(!s.export && exts.is_empty());
        let no_mods = |n: &CStr| n != VK_EXT_IMAGE_DRM_FORMAT_MODIFIER;
        let (exts, s) = device_extensions(API_VERSION_1_1, API_VERSION_1_1, no_mods);
        assert!(s.export && !s.modifiers);
        assert_eq!(exts.len(), 2);
        let no_dma = |n: &CStr| n != VK_EXT_EXTERNAL_MEMORY_DMA_BUF;
        assert!(
            !device_extensions(API_VERSION_1_1, API_VERSION_1_1, no_dma)
                .1
                .export
        );
    }
}
