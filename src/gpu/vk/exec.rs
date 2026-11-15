//! The Vulkan executor: draws a lowered [`Plan`] with Vulkan, offscreen (and
//! read back) or into a window's swapchain (and presented).
//!
//! The plan is first recorded (`record.rs`: passes and draws, no Vulkan),
//! then turned into one command buffer: the images the frame draws uploaded
//! (each texture cached by the image's identity, in shared memory blocks),
//! the vertices of the whole frame in one host-visible buffer, push
//! constants for every uniform, and one descriptor set per texture. Every
//! render pass loads and stores; a pass that starts cleared clears inside it.
//! Between passes one pipeline barrier makes the previous pass's writes
//! visible and moves the images that change roles (a layer drawn into, then
//! sampled) to their new layouts.
//!
//! Targets are `R8G8B8A8_UNORM` (the frame and layers) or `R8_UNORM` (the
//! path scratch), each with a stencil image of the first stencil format the
//! device can attach. Colors are blended on the encoded values, as the
//! software renderer blends them. When presenting, the frame is drawn into
//! its offscreen target as for a read-back and then copied into the acquired
//! swapchain image (`vkCmdBlitImage`, or a copy when the formats match) on the
//! GPU: one pass more than drawing into the swapchain image, but one set of
//! pipelines and one code path for both.
//!
//! One frame is in flight: a frame waits for the one before it, so every
//! buffer and target can be reused.

#![allow(unsafe_code)]

use super::alloc::Ranges;
use super::record::{self, Cmd, Draw, Pipe, Recording, Target, Texture};
use super::shaders;
use super::sys::*;
use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::gpu::ops::Plan;
use crate::gpu::renderer::{Api, Executor, GpuError};
use crate::platform::GpuLayer;
use crate::render::Pixmap;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Weak};

/// Textures of images not drawn for this many frames are freed.
const TEXTURE_KEEP_FRAMES: u64 = 120;
/// Texture memory comes in blocks of this many bytes.
const BLOCK: u64 = 16 << 20;
/// A frame the GPU hasn't finished after this long is a lost device.
const FENCE_TIMEOUT: u64 = 10_000_000_000;
/// How long to wait for a swapchain image before skipping the frame.
const ACQUIRE_TIMEOUT: u64 = 1_000_000_000;

const VK_KHR_SURFACE: &CStr = c"VK_KHR_surface";
const VK_KHR_XCB_SURFACE: &CStr = c"VK_KHR_xcb_surface";
const VK_KHR_ANDROID_SURFACE: &CStr = c"VK_KHR_android_surface";
const VK_EXT_METAL_SURFACE: &CStr = c"VK_EXT_metal_surface";
const VK_KHR_SWAPCHAIN: &CStr = c"VK_KHR_swapchain";
// Drivers that implement Vulkan over another API (MoltenVK) are listed only
// to an instance that asks for them, and need their subset enabled.
const VK_KHR_PORTABILITY_ENUMERATION: &CStr = c"VK_KHR_portability_enumeration";
const VK_KHR_PORTABILITY_SUBSET: &CStr = c"VK_KHR_portability_subset";
const INSTANCE_ENUMERATE_PORTABILITY: u32 = 0x1;
const VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES2: &CStr = c"VK_KHR_get_physical_device_properties2";

fn lost(what: &str, r: VkResult) -> GpuError {
    GpuError::Lost(format!("{what} failed ({r})"))
}

fn check(r: VkResult, what: &str) -> Result<(), GpuError> {
    if r == VK_SUCCESS {
        Ok(())
    } else {
        Err(lost(what, r))
    }
}

fn subresource(aspect: u32) -> VkImageSubresourceRange {
    VkImageSubresourceRange {
        aspectMask: aspect,
        baseMipLevel: 0,
        levelCount: 1,
        baseArrayLayer: 0,
        layerCount: 1,
    }
}

fn layers(aspect: u32) -> VkImageSubresourceLayers {
    VkImageSubresourceLayers {
        aspectMask: aspect,
        mipLevel: 0,
        baseArrayLayer: 0,
        layerCount: 1,
    }
}

/// Move `img` to `layout` (with a barrier added to `barriers`), if it isn't
/// there.
fn transition(barriers: &mut Vec<VkImageMemoryBarrier>, img: &mut Image, layout: u32) {
    if img.layout == layout {
        return;
    }
    barriers.push(VkImageMemoryBarrier {
        sType: ST_IMAGE_MEMORY_BARRIER,
        pNext: std::ptr::null(),
        srcAccessMask: ACCESS_MEMORY_WRITE,
        dstAccessMask: ACCESS_MEMORY_READ | ACCESS_MEMORY_WRITE,
        oldLayout: img.layout,
        newLayout: layout,
        srcQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
        dstQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
        image: img.image,
        subresourceRange: subresource(img.aspect),
    });
    img.layout = layout;
}

/// Where an image's memory is.
enum Memory {
    Own(VkDeviceMemory),
    Block {
        block: usize,
        offset: u64,
        size: u64,
    },
}

/// An image, its view, and the layout it is in.
struct Image {
    image: VkImage,
    view: VkImageView,
    memory: Memory,
    aspect: u32,
    layout: u32,
}

/// A color image and a stencil image, and the framebuffer over both.
struct Surface2 {
    color: Image,
    stencil: Image,
    framebuffer: VkFramebuffer,
    w: i32,
    h: i32,
}

/// A cached image texture.
struct Tex {
    image: Weak<Pixmap>,
    img: Image,
    used: u64,
}

/// A host-visible buffer, mapped for its life.
struct HostBuffer {
    buffer: VkBuffer,
    memory: VkDeviceMemory,
    ptr: *mut u8,
    size: u64,
}

/// A block of device memory textures share.
struct Block {
    memory: VkDeviceMemory,
    type_index: u32,
    ranges: Ranges,
}

/// A window's surface and swapchain.
struct Swap {
    surface: VkSurfaceKHR,
    swapchain: VkSwapchainKHR,
    images: Vec<VkImage>,
    /// Signaled by each image's frame, waited by its present.
    done: Vec<VkSemaphore>,
    acquire: VkSemaphore,
    extent: VkExtent2D,
    format: u32,
    /// Re-create it before the next frame (resized, out of date).
    stale: bool,
}

/// The Vulkan executor: instance, device, pipelines, targets and caches.
pub(crate) struct VkExecutor {
    vk: Fns,
    instance: VkInstance,
    phys: VkPhysicalDevice,
    device: VkDevice,
    queue: VkQueue,
    family: u32,
    memory: VkPhysicalDeviceMemoryProperties,
    max_size: i32,
    stencil_format: u32,
    stencil_aspect: u32,
    /// The platform's window surface extension (xcb, Android) is on.
    has_surface: bool,
    has_swapchain: bool,
    color_pass: VkRenderPass,
    scratch_pass: VkRenderPass,
    set_layout: VkDescriptorSetLayout,
    layout: VkPipelineLayout,
    linear: VkSampler,
    nearest: VkSampler,
    pipes: HashMap<Pipe, VkPipeline>,
    pool: VkCommandPool,
    cmd: VkCommandBuffer,
    fence: VkFence,
    descriptors: VkDescriptorPool,
    descriptor_capacity: u32,
    frame: Option<Surface2>,
    layers: Vec<Surface2>,
    scratch: Option<Surface2>,
    textures: HashMap<usize, Tex>,
    blocks: Vec<Block>,
    vertices: Option<HostBuffer>,
    staging: Option<HostBuffer>,
    readback: Option<HostBuffer>,
    swaps: HashMap<usize, Swap>,
    count: u64,
    uploads: usize,
}

impl VkExecutor {
    /// Set up on the system's Vulkan device: an instance (with the window
    /// surface extensions this platform has), a device with a graphics
    /// queue, the render passes and pipelines.
    pub(crate) fn new() -> Result<Self, GpuError> {
        unsafe { Self::create() }.map_err(GpuError::Unavailable)
    }

    unsafe fn create() -> Result<Self, String> {
        unsafe {
            let gipa = loader()?;
            let globals = Globals::load(gipa)?;

            // Instance extensions: the surfaces this platform presents to.
            let available = {
                let mut n = 0u32;
                (globals.vkEnumerateInstanceExtensionProperties)(
                    std::ptr::null(),
                    &mut n,
                    std::ptr::null_mut(),
                );
                let mut v: Vec<VkExtensionProperties> = (0..n)
                    .map(|_| VkExtensionProperties {
                        extensionName: [0; 256],
                        specVersion: 0,
                    })
                    .collect();
                (globals.vkEnumerateInstanceExtensionProperties)(
                    std::ptr::null(),
                    &mut n,
                    v.as_mut_ptr(),
                );
                v.truncate(n as usize);
                v
            };
            let has = |name: &CStr| available.iter().any(|e| ext_name(e) == name);
            let surface_ext = if cfg!(target_os = "android") {
                VK_KHR_ANDROID_SURFACE
            } else if cfg!(target_os = "macos") {
                VK_EXT_METAL_SURFACE
            } else {
                VK_KHR_XCB_SURFACE
            };
            let surfaces = has(VK_KHR_SURFACE) && has(surface_ext);
            let mut exts: Vec<*const c_char> = Vec::new();
            if surfaces {
                exts.push(VK_KHR_SURFACE.as_ptr());
                exts.push(surface_ext.as_ptr());
            }
            let portability = has(VK_KHR_PORTABILITY_ENUMERATION);
            if portability {
                exts.push(VK_KHR_PORTABILITY_ENUMERATION.as_ptr());
                // What the portability subset needs on a 1.0 instance.
                if has(VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES2) {
                    exts.push(VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES2.as_ptr());
                }
            }
            let app = VkApplicationInfo {
                sType: ST_APPLICATION_INFO,
                pNext: std::ptr::null(),
                pApplicationName: c"stipple".as_ptr(),
                applicationVersion: 0,
                pEngineName: c"stipple".as_ptr(),
                engineVersion: 0,
                apiVersion: API_VERSION_1_0,
            };
            let ici = VkInstanceCreateInfo {
                sType: ST_INSTANCE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: if portability {
                    INSTANCE_ENUMERATE_PORTABILITY
                } else {
                    0
                },
                pApplicationInfo: &app,
                enabledLayerCount: 0,
                ppEnabledLayerNames: std::ptr::null(),
                enabledExtensionCount: exts.len() as u32,
                ppEnabledExtensionNames: exts.as_ptr(),
            };
            let mut instance: VkInstance = std::ptr::null_mut();
            let r = (globals.vkCreateInstance)(&ici, std::ptr::null(), &mut instance);
            if r != VK_SUCCESS {
                return Err(format!("vkCreateInstance failed ({r})"));
            }
            // Without its entry points there's nothing to destroy the
            // instance with: a one-off leak on a broken loader.
            let vk = Fns::load(gipa, instance)?;
            Self::on_instance(vk, instance, surfaces)
        }
    }

    /// The rest of setting up, on `instance` (destroyed again on failure).
    unsafe fn on_instance(vk: Fns, instance: VkInstance, surfaces: bool) -> Result<Self, String> {
        unsafe {
            let picked = match pick_device(&vk, instance) {
                Ok(p) => p,
                Err(e) => {
                    (vk.vkDestroyInstance)(instance, std::ptr::null());
                    return Err(e);
                }
            };
            let (phys, family) = picked;
            let mut props = vec![0u8; PROPS_SIZE];
            (vk.vkGetPhysicalDeviceProperties)(phys, props.as_mut_ptr());
            let max = u32::from_ne_bytes(
                props[PROPS_MAX_IMAGE_2D..PROPS_MAX_IMAGE_2D + 4]
                    .try_into()
                    .unwrap(),
            );
            let max_size = (max as i32).clamp(4096, 16384);

            // Device extensions: the swapchain, when there are surfaces, and
            // the portability subset, where the driver has one.
            let dev_exts = {
                let mut n = 0u32;
                (vk.vkEnumerateDeviceExtensionProperties)(
                    phys,
                    std::ptr::null(),
                    &mut n,
                    std::ptr::null_mut(),
                );
                let mut v: Vec<VkExtensionProperties> = (0..n)
                    .map(|_| VkExtensionProperties {
                        extensionName: [0; 256],
                        specVersion: 0,
                    })
                    .collect();
                (vk.vkEnumerateDeviceExtensionProperties)(
                    phys,
                    std::ptr::null(),
                    &mut n,
                    v.as_mut_ptr(),
                );
                v.truncate(n as usize);
                v
            };
            let dev_has = |name: &CStr| dev_exts.iter().any(|e| ext_name(e) == name);
            let has_swapchain = surfaces && dev_has(VK_KHR_SWAPCHAIN);
            let mut dexts: Vec<*const c_char> = Vec::new();
            if has_swapchain {
                dexts.push(VK_KHR_SWAPCHAIN.as_ptr());
            }
            if dev_has(VK_KHR_PORTABILITY_SUBSET) {
                dexts.push(VK_KHR_PORTABILITY_SUBSET.as_ptr());
            }
            let priority = 1.0f32;
            let qci = VkDeviceQueueCreateInfo {
                sType: ST_DEVICE_QUEUE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                queueFamilyIndex: family,
                queueCount: 1,
                pQueuePriorities: &priority,
            };
            let dci = VkDeviceCreateInfo {
                sType: ST_DEVICE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                queueCreateInfoCount: 1,
                pQueueCreateInfos: &qci,
                enabledLayerCount: 0,
                ppEnabledLayerNames: std::ptr::null(),
                enabledExtensionCount: dexts.len() as u32,
                ppEnabledExtensionNames: dexts.as_ptr(),
                pEnabledFeatures: std::ptr::null(),
            };
            let mut device: VkDevice = std::ptr::null_mut();
            let r = (vk.vkCreateDevice)(phys, &dci, std::ptr::null(), &mut device);
            if r != VK_SUCCESS {
                (vk.vkDestroyInstance)(instance, std::ptr::null());
                return Err(format!("vkCreateDevice failed ({r})"));
            }
            let mut queue: VkQueue = std::ptr::null_mut();
            (vk.vkGetDeviceQueue)(device, family, 0, &mut queue);
            let mut memory: VkPhysicalDeviceMemoryProperties = std::mem::zeroed();
            (vk.vkGetPhysicalDeviceMemoryProperties)(phys, &mut memory);
            let stencil = [
                FORMAT_S8_UINT,
                FORMAT_D24_UNORM_S8_UINT,
                FORMAT_D32_SFLOAT_S8_UINT,
                FORMAT_D16_UNORM_S8_UINT,
            ]
            .into_iter()
            .find(|&f| {
                let mut p = VkFormatProperties::default();
                (vk.vkGetPhysicalDeviceFormatProperties)(phys, f, &mut p);
                p.optimalTilingFeatures & FEATURE_DEPTH_STENCIL_ATTACHMENT != 0
            });
            let mut me = VkExecutor {
                vk,
                instance,
                phys,
                device,
                queue,
                family,
                memory,
                max_size,
                stencil_format: stencil.unwrap_or(FORMAT_UNDEFINED),
                stencil_aspect: if stencil == Some(FORMAT_S8_UINT) {
                    ASPECT_STENCIL
                } else {
                    ASPECT_DEPTH | ASPECT_STENCIL
                },
                has_surface: surfaces,
                has_swapchain,
                color_pass: NULL,
                scratch_pass: NULL,
                set_layout: NULL,
                layout: NULL,
                linear: NULL,
                nearest: NULL,
                pipes: HashMap::new(),
                pool: NULL,
                cmd: std::ptr::null_mut(),
                fence: NULL,
                descriptors: NULL,
                descriptor_capacity: 0,
                frame: None,
                layers: Vec::new(),
                scratch: None,
                textures: HashMap::new(),
                blocks: Vec::new(),
                vertices: None,
                staging: None,
                readback: None,
                swaps: HashMap::new(),
                count: 0,
                uploads: 0,
            };
            // From here `me`'s Drop frees whatever was made.
            if stencil.is_none() {
                return Err("no stencil format to attach".into());
            }
            // On failure, dropping `me` destroys the device and the instance.
            me.build().map_err(|e| e.to_string())?;
            Ok(me)
        }
    }

    /// Render passes, the pipeline layout, samplers, pipelines, the command
    /// buffer and its fence.
    unsafe fn build(&mut self) -> Result<(), GpuError> {
        unsafe {
            let vk = &self.vk;
            let d = self.device;
            self.color_pass = self.render_pass(FORMAT_R8G8B8A8_UNORM)?;
            self.scratch_pass = self.render_pass(FORMAT_R8_UNORM)?;

            let binding = VkDescriptorSetLayoutBinding {
                binding: 0,
                descriptorType: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                descriptorCount: 1,
                stageFlags: SHADER_STAGE_FRAGMENT,
                pImmutableSamplers: std::ptr::null(),
            };
            let dsl = VkDescriptorSetLayoutCreateInfo {
                sType: ST_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                bindingCount: 1,
                pBindings: &binding,
            };
            check(
                (vk.vkCreateDescriptorSetLayout)(d, &dsl, std::ptr::null(), &mut self.set_layout),
                "vkCreateDescriptorSetLayout",
            )?;
            let range = VkPushConstantRange {
                stageFlags: SHADER_STAGE_VERTEX | SHADER_STAGE_FRAGMENT,
                offset: 0,
                size: record::PUSH_BYTES,
            };
            let pl = VkPipelineLayoutCreateInfo {
                sType: ST_PIPELINE_LAYOUT_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                setLayoutCount: 1,
                pSetLayouts: &self.set_layout,
                pushConstantRangeCount: 1,
                pPushConstantRanges: &range,
            };
            check(
                (vk.vkCreatePipelineLayout)(d, &pl, std::ptr::null(), &mut self.layout),
                "vkCreatePipelineLayout",
            )?;
            for (filter, slot) in [
                (FILTER_LINEAR, &mut self.linear),
                (FILTER_NEAREST, &mut self.nearest),
            ] {
                let s = VkSamplerCreateInfo {
                    sType: ST_SAMPLER_CREATE_INFO,
                    pNext: std::ptr::null(),
                    flags: 0,
                    magFilter: filter,
                    minFilter: filter,
                    mipmapMode: MIPMAP_NEAREST,
                    addressModeU: ADDRESS_CLAMP_TO_EDGE,
                    addressModeV: ADDRESS_CLAMP_TO_EDGE,
                    addressModeW: ADDRESS_CLAMP_TO_EDGE,
                    mipLodBias: 0.0,
                    anisotropyEnable: 0,
                    maxAnisotropy: 1.0,
                    compareEnable: 0,
                    compareOp: 0,
                    minLod: 0.0,
                    maxLod: 0.0,
                    borderColor: 0,
                    unnormalizedCoordinates: 0,
                };
                check(
                    (vk.vkCreateSampler)(d, &s, std::ptr::null(), slot),
                    "vkCreateSampler",
                )?;
            }

            let modules = [
                shaders::SCENE_VERT,
                shaders::BOX_FRAG,
                shaders::TEX_FRAG,
                shaders::FLAT_FRAG,
                shaders::RESOLVE_FRAG,
            ]
            .map(|spv| self.shader(spv));
            let built = match modules.iter().cloned().collect::<Result<Vec<_>, _>>() {
                Ok(m) => self.pipelines([m[0], m[1], m[2], m[3], m[4]]),
                Err(e) => Err(e),
            };
            for m in modules.into_iter().flatten() {
                (self.vk.vkDestroyShaderModule)(d, m, std::ptr::null());
            }
            self.pipes = built?;

            let vk = &self.vk;
            let cp = VkCommandPoolCreateInfo {
                sType: ST_COMMAND_POOL_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: COMMAND_POOL_RESET_COMMAND_BUFFER,
                queueFamilyIndex: self.family,
            };
            check(
                (vk.vkCreateCommandPool)(d, &cp, std::ptr::null(), &mut self.pool),
                "vkCreateCommandPool",
            )?;
            let ca = VkCommandBufferAllocateInfo {
                sType: ST_COMMAND_BUFFER_ALLOCATE_INFO,
                pNext: std::ptr::null(),
                commandPool: self.pool,
                level: COMMAND_BUFFER_LEVEL_PRIMARY,
                commandBufferCount: 1,
            };
            check(
                (vk.vkAllocateCommandBuffers)(d, &ca, &mut self.cmd),
                "vkAllocateCommandBuffers",
            )?;
            let fc = VkFenceCreateInfo {
                sType: ST_FENCE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: FENCE_SIGNALED,
            };
            check(
                (vk.vkCreateFence)(d, &fc, std::ptr::null(), &mut self.fence),
                "vkCreateFence",
            )?;
            Ok(())
        }
    }

    /// A render pass with one `format` color attachment and the stencil,
    /// both loaded and stored, staying in their attachment layouts.
    unsafe fn render_pass(&self, format: u32) -> Result<VkRenderPass, GpuError> {
        unsafe {
            let atts = [
                VkAttachmentDescription {
                    flags: 0,
                    format,
                    samples: SAMPLE_COUNT_1,
                    loadOp: LOAD_OP_LOAD,
                    storeOp: STORE_OP_STORE,
                    stencilLoadOp: LOAD_OP_LOAD,
                    stencilStoreOp: STORE_OP_STORE,
                    initialLayout: LAYOUT_COLOR_ATTACHMENT,
                    finalLayout: LAYOUT_COLOR_ATTACHMENT,
                },
                VkAttachmentDescription {
                    flags: 0,
                    format: self.stencil_format,
                    samples: SAMPLE_COUNT_1,
                    loadOp: LOAD_OP_LOAD,
                    storeOp: STORE_OP_STORE,
                    stencilLoadOp: LOAD_OP_LOAD,
                    stencilStoreOp: STORE_OP_STORE,
                    initialLayout: LAYOUT_DEPTH_STENCIL_ATTACHMENT,
                    finalLayout: LAYOUT_DEPTH_STENCIL_ATTACHMENT,
                },
            ];
            let color = VkAttachmentReference {
                attachment: 0,
                layout: LAYOUT_COLOR_ATTACHMENT,
            };
            let stencil = VkAttachmentReference {
                attachment: 1,
                layout: LAYOUT_DEPTH_STENCIL_ATTACHMENT,
            };
            let sub = VkSubpassDescription {
                flags: 0,
                pipelineBindPoint: BIND_POINT_GRAPHICS,
                inputAttachmentCount: 0,
                pInputAttachments: std::ptr::null(),
                colorAttachmentCount: 1,
                pColorAttachments: &color,
                pResolveAttachments: std::ptr::null(),
                pDepthStencilAttachment: &stencil,
                preserveAttachmentCount: 0,
                pPreserveAttachments: std::ptr::null(),
            };
            let rp = VkRenderPassCreateInfo {
                sType: ST_RENDER_PASS_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                attachmentCount: 2,
                pAttachments: atts.as_ptr(),
                subpassCount: 1,
                pSubpasses: &sub,
                dependencyCount: 0,
                pDependencies: std::ptr::null(),
            };
            let mut pass = NULL;
            check(
                (self.vk.vkCreateRenderPass)(self.device, &rp, std::ptr::null(), &mut pass),
                "vkCreateRenderPass",
            )?;
            Ok(pass)
        }
    }

    unsafe fn shader(&self, spv: &[u8]) -> Result<VkShaderModule, GpuError> {
        unsafe {
            let words = shaders::words(spv);
            let ci = VkShaderModuleCreateInfo {
                sType: ST_SHADER_MODULE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                codeSize: words.len() * 4,
                pCode: words.as_ptr(),
            };
            let mut m = NULL;
            check(
                (self.vk.vkCreateShaderModule)(self.device, &ci, std::ptr::null(), &mut m),
                "vkCreateShaderModule",
            )?;
            Ok(m)
        }
    }

    /// Every pipeline, from the vertex shader and the four fragment shaders.
    unsafe fn pipelines(
        &self,
        m: [VkShaderModule; 5],
    ) -> Result<HashMap<Pipe, VkPipeline>, GpuError> {
        unsafe {
            let [vs, boxes, tex, flat, resolve] = m;
            let mut pipes = HashMap::new();
            for pipe in Pipe::ALL {
                let fs = match pipe {
                    Pipe::Box => boxes,
                    Pipe::Tex => tex,
                    Pipe::Resolve => resolve,
                    _ => flat,
                };
                match self.pipeline(pipe, vs, fs) {
                    Ok(p) => {
                        pipes.insert(pipe, p);
                    }
                    Err(e) => {
                        for (_, p) in pipes {
                            (self.vk.vkDestroyPipeline)(self.device, p, std::ptr::null());
                        }
                        return Err(e);
                    }
                }
            }
            Ok(pipes)
        }
    }

    /// The pipeline for `pipe`: its blend, color writes and stencil state,
    /// with the viewport, scissor and stencil reference set per draw.
    unsafe fn pipeline(
        &self,
        pipe: Pipe,
        vs: VkShaderModule,
        fs: VkShaderModule,
    ) -> Result<VkPipeline, GpuError> {
        unsafe {
            let face = |compare, pass, compare_mask, write_mask| VkStencilOpState {
                failOp: STENCIL_KEEP,
                passOp: pass,
                depthFailOp: STENCIL_KEEP,
                compareOp: compare,
                compareMask: compare_mask,
                writeMask: write_mask,
                reference: 0,
            };
            // (front, back, blend, writes color)
            let test = face(COMPARE_EQUAL, STENCIL_KEEP, 0xff, 0);
            let (front, back, blend, write) = match pipe {
                Pipe::Box | Pipe::Tex | Pipe::Resolve => (test, test, true, true),
                Pipe::ClipInc => {
                    let f = face(COMPARE_EQUAL, STENCIL_INCR_CLAMP, 0xff, 0xff);
                    (f, f, false, false)
                }
                Pipe::ClipDec => {
                    let f = face(COMPARE_EQUAL, STENCIL_DECR_CLAMP, 0xff, 0xff);
                    (f, f, false, false)
                }
                Pipe::WindNonZero => (
                    face(COMPARE_ALWAYS, STENCIL_INCR_WRAP, 0xff, 0xff),
                    face(COMPARE_ALWAYS, STENCIL_DECR_WRAP, 0xff, 0xff),
                    false,
                    false,
                ),
                Pipe::WindEvenOdd => {
                    let f = face(COMPARE_ALWAYS, STENCIL_INVERT, 0xff, 0xff);
                    (f, f, false, false)
                }
                Pipe::CoverNonZero => {
                    let f = face(COMPARE_NOT_EQUAL, STENCIL_KEEP, 0xff, 0);
                    (f, f, false, true)
                }
                Pipe::CoverEvenOdd => {
                    let f = face(COMPARE_NOT_EQUAL, STENCIL_KEEP, 1, 0);
                    (f, f, false, true)
                }
            };
            let stages = [
                VkPipelineShaderStageCreateInfo {
                    sType: ST_PIPELINE_SHADER_STAGE_CREATE_INFO,
                    pNext: std::ptr::null(),
                    flags: 0,
                    stage: SHADER_STAGE_VERTEX,
                    module: vs,
                    pName: c"main".as_ptr(),
                    pSpecializationInfo: std::ptr::null(),
                },
                VkPipelineShaderStageCreateInfo {
                    sType: ST_PIPELINE_SHADER_STAGE_CREATE_INFO,
                    pNext: std::ptr::null(),
                    flags: 0,
                    stage: SHADER_STAGE_FRAGMENT,
                    module: fs,
                    pName: c"main".as_ptr(),
                    pSpecializationInfo: std::ptr::null(),
                },
            ];
            let binding = VkVertexInputBindingDescription {
                binding: 0,
                stride: 16,
                inputRate: VERTEX_INPUT_RATE_VERTEX,
            };
            let attr = VkVertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: FORMAT_R32G32B32A32_SFLOAT,
                offset: 0,
            };
            let vi = VkPipelineVertexInputStateCreateInfo {
                sType: ST_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                vertexBindingDescriptionCount: 1,
                pVertexBindingDescriptions: &binding,
                vertexAttributeDescriptionCount: 1,
                pVertexAttributeDescriptions: &attr,
            };
            let ia = VkPipelineInputAssemblyStateCreateInfo {
                sType: ST_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                topology: TOPOLOGY_TRIANGLE_LIST,
                primitiveRestartEnable: 0,
            };
            let vp = VkPipelineViewportStateCreateInfo {
                sType: ST_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                viewportCount: 1,
                pViewports: std::ptr::null(),
                scissorCount: 1,
                pScissors: std::ptr::null(),
            };
            let rs = VkPipelineRasterizationStateCreateInfo {
                sType: ST_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                depthClampEnable: 0,
                rasterizerDiscardEnable: 0,
                polygonMode: POLYGON_FILL,
                cullMode: CULL_NONE,
                frontFace: FRONT_FACE_CCW,
                depthBiasEnable: 0,
                depthBiasConstantFactor: 0.0,
                depthBiasClamp: 0.0,
                depthBiasSlopeFactor: 0.0,
                lineWidth: 1.0,
            };
            let ms = VkPipelineMultisampleStateCreateInfo {
                sType: ST_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                rasterizationSamples: SAMPLE_COUNT_1,
                sampleShadingEnable: 0,
                minSampleShading: 0.0,
                pSampleMask: std::ptr::null(),
                alphaToCoverageEnable: 0,
                alphaToOneEnable: 0,
            };
            let ds = VkPipelineDepthStencilStateCreateInfo {
                sType: ST_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                depthTestEnable: 0,
                depthWriteEnable: 0,
                depthCompareOp: COMPARE_ALWAYS,
                depthBoundsTestEnable: 0,
                stencilTestEnable: 1,
                front,
                back,
                minDepthBounds: 0.0,
                maxDepthBounds: 1.0,
            };
            let att = VkPipelineColorBlendAttachmentState {
                blendEnable: blend as u32,
                srcColorBlendFactor: BLEND_ONE,
                dstColorBlendFactor: BLEND_ONE_MINUS_SRC_ALPHA,
                colorBlendOp: BLEND_OP_ADD,
                srcAlphaBlendFactor: BLEND_ONE,
                dstAlphaBlendFactor: BLEND_ONE_MINUS_SRC_ALPHA,
                alphaBlendOp: BLEND_OP_ADD,
                colorWriteMask: if write { COLOR_RGBA } else { 0 },
            };
            let cb = VkPipelineColorBlendStateCreateInfo {
                sType: ST_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                logicOpEnable: 0,
                logicOp: 0,
                attachmentCount: 1,
                pAttachments: &att,
                blendConstants: [0.0; 4],
            };
            let dynamic = [DYNAMIC_VIEWPORT, DYNAMIC_SCISSOR, DYNAMIC_STENCIL_REFERENCE];
            let dy = VkPipelineDynamicStateCreateInfo {
                sType: ST_PIPELINE_DYNAMIC_STATE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                dynamicStateCount: dynamic.len() as u32,
                pDynamicStates: dynamic.as_ptr(),
            };
            let gp = VkGraphicsPipelineCreateInfo {
                sType: ST_GRAPHICS_PIPELINE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                stageCount: 2,
                pStages: stages.as_ptr(),
                pVertexInputState: &vi,
                pInputAssemblyState: &ia,
                pTessellationState: std::ptr::null(),
                pViewportState: &vp,
                pRasterizationState: &rs,
                pMultisampleState: &ms,
                pDepthStencilState: &ds,
                pColorBlendState: &cb,
                pDynamicState: &dy,
                layout: self.layout,
                renderPass: if pipe.on_scratch() {
                    self.scratch_pass
                } else {
                    self.color_pass
                },
                subpass: 0,
                basePipelineHandle: NULL,
                basePipelineIndex: -1,
            };
            let mut p = NULL;
            check(
                (self.vk.vkCreateGraphicsPipelines)(
                    self.device,
                    NULL,
                    1,
                    &gp,
                    std::ptr::null(),
                    &mut p,
                ),
                "vkCreateGraphicsPipelines",
            )?;
            Ok(p)
        }
    }

    /// A memory type for `bits` with `want` properties.
    fn memory_type(&self, bits: u32, want: u32) -> Option<u32> {
        (0..self.memory.memoryTypeCount).find(|&i| {
            bits & (1 << i) != 0 && self.memory.memoryTypes[i as usize].propertyFlags & want == want
        })
    }

    unsafe fn allocate(&self, size: u64, type_index: u32) -> Result<VkDeviceMemory, GpuError> {
        unsafe {
            let ai = VkMemoryAllocateInfo {
                sType: ST_MEMORY_ALLOCATE_INFO,
                pNext: std::ptr::null(),
                allocationSize: size,
                memoryTypeIndex: type_index,
            };
            let mut m = NULL;
            check(
                (self.vk.vkAllocateMemory)(self.device, &ai, std::ptr::null(), &mut m),
                "vkAllocateMemory",
            )?;
            Ok(m)
        }
    }

    /// A `w`×`h` image of `format` for `usage`, with its view; `shared` puts
    /// it in a texture block rather than an allocation of its own.
    unsafe fn image(
        &mut self,
        format: u32,
        w: i32,
        h: i32,
        usage: u32,
        aspect: u32,
        shared: bool,
    ) -> Result<Image, GpuError> {
        unsafe {
            let d = self.device;
            let ci = VkImageCreateInfo {
                sType: ST_IMAGE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                imageType: IMAGE_TYPE_2D,
                format,
                extent: VkExtent3D {
                    width: w as u32,
                    height: h as u32,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: SAMPLE_COUNT_1,
                tiling: TILING_OPTIMAL,
                usage,
                sharingMode: SHARING_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: std::ptr::null(),
                initialLayout: LAYOUT_UNDEFINED,
            };
            let mut image = NULL;
            check(
                (self.vk.vkCreateImage)(d, &ci, std::ptr::null(), &mut image),
                "vkCreateImage",
            )?;
            let mut req = VkMemoryRequirements::default();
            (self.vk.vkGetImageMemoryRequirements)(d, image, &mut req);
            let type_index = self
                .memory_type(req.memoryTypeBits, MEMORY_DEVICE_LOCAL)
                .or_else(|| self.memory_type(req.memoryTypeBits, 0));
            let Some(type_index) = type_index else {
                (self.vk.vkDestroyImage)(d, image, std::ptr::null());
                return Err(GpuError::Lost("no memory type for an image".into()));
            };
            let placed = if shared && req.size <= BLOCK / 2 {
                self.suballocate(req, type_index)
            } else {
                None
            };
            let (memory, mem, offset) = match placed {
                Some((block, offset)) => (
                    Memory::Block {
                        block,
                        offset,
                        size: req.size,
                    },
                    self.blocks[block].memory,
                    offset,
                ),
                None => match self.allocate(req.size, type_index) {
                    Ok(m) => (Memory::Own(m), m, 0),
                    Err(e) => {
                        (self.vk.vkDestroyImage)(d, image, std::ptr::null());
                        return Err(e);
                    }
                },
            };
            let mut img = Image {
                image,
                view: NULL,
                memory,
                aspect,
                layout: LAYOUT_UNDEFINED,
            };
            if let Err(e) = check(
                (self.vk.vkBindImageMemory)(d, image, mem, offset),
                "vkBindImageMemory",
            ) {
                self.free_image(img);
                return Err(e);
            }
            let vi = VkImageViewCreateInfo {
                sType: ST_IMAGE_VIEW_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                image,
                viewType: VIEW_TYPE_2D,
                format,
                components: VkComponentMapping::default(),
                subresourceRange: subresource(aspect),
            };
            if let Err(e) = check(
                (self.vk.vkCreateImageView)(d, &vi, std::ptr::null(), &mut img.view),
                "vkCreateImageView",
            ) {
                self.free_image(img);
                return Err(e);
            }
            Ok(img)
        }
    }

    /// Room for `req` in a texture block of memory type `type_index`,
    /// adding a block when the others are full.
    unsafe fn suballocate(
        &mut self,
        req: VkMemoryRequirements,
        type_index: u32,
    ) -> Option<(usize, u64)> {
        unsafe {
            for (i, b) in self.blocks.iter_mut().enumerate() {
                if b.type_index == type_index
                    && let Some(off) = b.ranges.alloc(req.size, req.alignment)
                {
                    return Some((i, off));
                }
            }
            let memory = self.allocate(BLOCK, type_index).ok()?;
            let mut ranges = Ranges::new(BLOCK);
            let off = ranges.alloc(req.size, req.alignment)?;
            self.blocks.push(Block {
                memory,
                type_index,
                ranges,
            });
            Some((self.blocks.len() - 1, off))
        }
    }

    unsafe fn free_image(&mut self, img: Image) {
        unsafe {
            let d = self.device;
            if img.view != NULL {
                (self.vk.vkDestroyImageView)(d, img.view, std::ptr::null());
            }
            (self.vk.vkDestroyImage)(d, img.image, std::ptr::null());
            match img.memory {
                Memory::Own(m) => (self.vk.vkFreeMemory)(d, m, std::ptr::null()),
                Memory::Block {
                    block,
                    offset,
                    size,
                } => self.blocks[block].ranges.free(offset, size),
            }
        }
    }

    /// A color target of `format` with its stencil and framebuffer.
    unsafe fn surface(&mut self, format: u32, w: i32, h: i32) -> Result<Surface2, GpuError> {
        unsafe {
            let usage = USAGE_COLOR_ATTACHMENT | USAGE_SAMPLED | USAGE_TRANSFER_SRC;
            let color = self.image(format, w, h, usage, ASPECT_COLOR, false)?;
            let stencil = match self.image(
                self.stencil_format,
                w,
                h,
                USAGE_DEPTH_STENCIL_ATTACHMENT,
                self.stencil_aspect,
                false,
            ) {
                Ok(s) => s,
                Err(e) => {
                    self.free_image(color);
                    return Err(e);
                }
            };
            let views = [color.view, stencil.view];
            let fi = VkFramebufferCreateInfo {
                sType: ST_FRAMEBUFFER_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                renderPass: if format == FORMAT_R8_UNORM {
                    self.scratch_pass
                } else {
                    self.color_pass
                },
                attachmentCount: 2,
                pAttachments: views.as_ptr(),
                width: w as u32,
                height: h as u32,
                layers: 1,
            };
            let mut framebuffer = NULL;
            if let Err(e) = check(
                (self.vk.vkCreateFramebuffer)(self.device, &fi, std::ptr::null(), &mut framebuffer),
                "vkCreateFramebuffer",
            ) {
                self.free_image(color);
                self.free_image(stencil);
                return Err(e);
            }
            Ok(Surface2 {
                color,
                stencil,
                framebuffer,
                w,
                h,
            })
        }
    }

    unsafe fn free_surface(&mut self, s: Surface2) {
        unsafe {
            (self.vk.vkDestroyFramebuffer)(self.device, s.framebuffer, std::ptr::null());
            self.free_image(s.color);
            self.free_image(s.stencil);
        }
    }

    /// A host-visible, coherent buffer of `size` bytes for `usage`, mapped.
    unsafe fn host_buffer(
        &self,
        size: u64,
        usage: u32,
        cached: bool,
    ) -> Result<HostBuffer, GpuError> {
        unsafe {
            let d = self.device;
            let bi = VkBufferCreateInfo {
                sType: ST_BUFFER_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                size,
                usage,
                sharingMode: SHARING_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: std::ptr::null(),
            };
            let mut buffer = NULL;
            check(
                (self.vk.vkCreateBuffer)(d, &bi, std::ptr::null(), &mut buffer),
                "vkCreateBuffer",
            )?;
            let mut req = VkMemoryRequirements::default();
            (self.vk.vkGetBufferMemoryRequirements)(d, buffer, &mut req);
            let want = MEMORY_HOST_VISIBLE | MEMORY_HOST_COHERENT;
            // Host-cached memory reads back faster, where there is some.
            let ty = cached
                .then(|| self.memory_type(req.memoryTypeBits, want | 0x8))
                .flatten()
                .or_else(|| self.memory_type(req.memoryTypeBits, want));
            let Some(ty) = ty else {
                (self.vk.vkDestroyBuffer)(d, buffer, std::ptr::null());
                return Err(GpuError::Lost("no host-visible memory".into()));
            };
            let memory = match self.allocate(req.size, ty) {
                Ok(m) => m,
                Err(e) => {
                    (self.vk.vkDestroyBuffer)(d, buffer, std::ptr::null());
                    return Err(e);
                }
            };
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let bound = check(
                (self.vk.vkBindBufferMemory)(d, buffer, memory, 0),
                "vkBindBufferMemory",
            )
            .and_then(|()| {
                check(
                    (self.vk.vkMapMemory)(d, memory, 0, req.size, 0, &mut ptr),
                    "vkMapMemory",
                )
            });
            if let Err(e) = bound {
                (self.vk.vkDestroyBuffer)(d, buffer, std::ptr::null());
                (self.vk.vkFreeMemory)(d, memory, std::ptr::null());
                return Err(e);
            }
            Ok(HostBuffer {
                buffer,
                memory,
                ptr: ptr as *mut u8,
                size,
            })
        }
    }

    unsafe fn free_buffer(&self, b: HostBuffer) {
        unsafe {
            (self.vk.vkDestroyBuffer)(self.device, b.buffer, std::ptr::null());
            (self.vk.vkFreeMemory)(self.device, b.memory, std::ptr::null());
        }
    }

    /// `slot`'s buffer if it holds `size` bytes, else a new one.
    unsafe fn ensure_buffer(
        &mut self,
        which: fn(&mut Self) -> &mut Option<HostBuffer>,
        size: u64,
        usage: u32,
        cached: bool,
    ) -> Result<(), GpuError> {
        unsafe {
            if which(self).as_ref().is_some_and(|b| b.size >= size) {
                return Ok(());
            }
            if let Some(old) = which(self).take() {
                self.free_buffer(old);
            }
            let size = size.next_power_of_two().max(64 * 1024);
            let b = self.host_buffer(size, usage, cached)?;
            *which(self) = Some(b);
            Ok(())
        }
    }

    /// Record `barriers` (and a global memory barrier: everything written
    /// so far is visible to what follows).
    unsafe fn barrier(&self, barriers: &[VkImageMemoryBarrier]) {
        unsafe {
            let global = VkMemoryBarrier {
                sType: ST_MEMORY_BARRIER,
                pNext: std::ptr::null(),
                srcAccessMask: ACCESS_MEMORY_WRITE,
                dstAccessMask: ACCESS_MEMORY_READ | ACCESS_MEMORY_WRITE,
            };
            (self.vk.vkCmdPipelineBarrier)(
                self.cmd,
                STAGE_ALL_COMMANDS,
                STAGE_ALL_COMMANDS,
                0,
                1,
                (&global as *const VkMemoryBarrier).cast(),
                0,
                std::ptr::null(),
                barriers.len() as u32,
                barriers.as_ptr(),
            );
        }
    }

    /// Make the transfers so far visible to the host (the read-back).
    unsafe fn host_barrier(&self) {
        unsafe {
            let b = VkMemoryBarrier {
                sType: ST_MEMORY_BARRIER,
                pNext: std::ptr::null(),
                srcAccessMask: ACCESS_MEMORY_WRITE,
                dstAccessMask: ACCESS_HOST_READ,
            };
            (self.vk.vkCmdPipelineBarrier)(
                self.cmd,
                STAGE_ALL_COMMANDS,
                STAGE_HOST,
                0,
                1,
                (&b as *const VkMemoryBarrier).cast(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            );
        }
    }

    fn surface_of(&mut self, t: Target) -> &mut Surface2 {
        match t {
            Target::Frame => self.frame.as_mut().unwrap(),
            Target::Layer(i) => &mut self.layers[i],
            Target::Scratch => self.scratch.as_mut().unwrap(),
        }
    }

    /// Wait for the frame before (one frame in flight).
    unsafe fn wait(&self) -> Result<(), GpuError> {
        unsafe {
            let r = (self.vk.vkWaitForFences)(self.device, 1, &self.fence, 1, FENCE_TIMEOUT);
            check(r, "waiting for the last frame")
        }
    }

    fn check_size(&self, plan: &Plan) -> Result<(i32, i32), GpuError> {
        let (w, h) = (plan.size.width as i32, plan.size.height as i32);
        if w <= 0 || h <= 0 || w > self.max_size || h > self.max_size {
            return Err(GpuError::Lost(format!(
                "{w}x{h} is outside the device's image sizes"
            )));
        }
        Ok((w, h))
    }

    /// Record `plan` into the command buffer, begun: textures uploaded, every
    /// pass drawn, the frame target left readable by transfers.
    unsafe fn encode(&mut self, plan: &Plan) -> Result<(), GpuError> {
        unsafe {
            let (w, h) = self.check_size(plan)?;
            if self.frame.as_ref().is_none_or(|t| (t.w, t.h) != (w, h)) {
                if let Some(old) = self.frame.take() {
                    self.free_surface(old);
                }
                for old in std::mem::take(&mut self.layers) {
                    self.free_surface(old);
                }
                self.frame = Some(self.surface(FORMAT_R8G8B8A8_UNORM, w, h)?);
            }
            // Record; on a scratch too small, grow it and record again.
            let have = self.scratch.as_ref().map_or((0, 0), |s| (s.w, s.h));
            let mut rec = record::record(plan, have, self.max_size);
            if !rec.fits(have) {
                let size = record::grow(rec.scratch, have, self.max_size);
                if let Some(old) = self.scratch.take() {
                    self.free_surface(old);
                }
                self.scratch = Some(self.surface(FORMAT_R8_UNORM, size.0, size.1)?);
                rec = record::record(plan, size, self.max_size);
            }
            while self.layers.len() < rec.layers {
                let l = self.surface(FORMAT_R8G8B8A8_UNORM, w, h)?;
                self.layers.push(l);
            }

            let begin = VkCommandBufferBeginInfo {
                sType: ST_COMMAND_BUFFER_BEGIN_INFO,
                pNext: std::ptr::null(),
                flags: COMMAND_BUFFER_ONE_TIME_SUBMIT,
                pInheritanceInfo: std::ptr::null(),
            };
            check(
                (self.vk.vkResetCommandBuffer)(self.cmd, 0),
                "vkResetCommandBuffer",
            )?;
            check(
                (self.vk.vkBeginCommandBuffer)(self.cmd, &begin),
                "vkBeginCommandBuffer",
            )?;
            self.uploads = 0;
            let images = self.upload(&rec)?;
            let sets = self.descriptor_sets(&rec, &images)?;
            if !rec.vertices.is_empty() {
                let bytes = std::mem::size_of_val(rec.vertices.as_slice()) as u64;
                self.ensure_buffer(|s| &mut s.vertices, bytes, BUFFER_VERTEX, false)?;
                let vb = self.vertices.as_ref().unwrap();
                std::ptr::copy_nonoverlapping(
                    rec.vertices.as_ptr() as *const u8,
                    vb.ptr,
                    bytes as usize,
                );
                let offset = 0u64;
                (self.vk.vkCmdBindVertexBuffers)(self.cmd, 0, 1, &vb.buffer, &offset);
            }
            self.passes(&rec, &sets);

            // The frame, ready to be copied out.
            let mut barriers = Vec::new();
            let frame = self.frame.as_mut().unwrap();
            transition(&mut barriers, &mut frame.color, LAYOUT_TRANSFER_SRC);
            self.barrier(&barriers);
            Ok(())
        }
    }

    /// Upload the images `rec` draws that aren't cached; returns each one's
    /// view, in `rec.images`' order.
    unsafe fn upload(&mut self, rec: &Recording) -> Result<Vec<VkImageView>, GpuError> {
        unsafe {
            let count = self.count;
            let mut views = Vec::with_capacity(rec.images.len());
            let mut fresh: Vec<(usize, usize)> = Vec::new(); // (image index, staging offset)
            let mut staged = 0usize;
            for (i, image) in rec.images.iter().enumerate() {
                let key = Arc::as_ptr(image) as usize;
                if let Some(t) = self.textures.get_mut(&key) {
                    if t.image.upgrade().is_some_and(|a| Arc::ptr_eq(&a, image)) {
                        t.used = count;
                        views.push(t.img.view);
                        continue;
                    }
                    // The address was reused by another image: upload anew.
                    let old = self.textures.remove(&key).unwrap();
                    self.free_image(old.img);
                }
                let size = image.size();
                let img = self.image(
                    FORMAT_R8G8B8A8_UNORM,
                    size.width as i32,
                    size.height as i32,
                    USAGE_SAMPLED | USAGE_TRANSFER_DST,
                    ASPECT_COLOR,
                    true,
                )?;
                views.push(img.view);
                self.textures.insert(
                    key,
                    Tex {
                        image: Arc::downgrade(image),
                        img,
                        used: count,
                    },
                );
                fresh.push((i, staged));
                staged += image.as_bytes().len().next_multiple_of(16);
            }
            if fresh.is_empty() {
                return Ok(views);
            }
            self.ensure_buffer(
                |s| &mut s.staging,
                staged as u64,
                BUFFER_TRANSFER_SRC,
                false,
            )?;
            let staging = self.staging.as_ref().unwrap();
            let (buffer, ptr) = (staging.buffer, staging.ptr);
            let mut barriers = Vec::new();
            for &(i, _) in &fresh {
                let key = Arc::as_ptr(&rec.images[i]) as usize;
                let t = self.textures.get_mut(&key).unwrap();
                transition(&mut barriers, &mut t.img, LAYOUT_TRANSFER_DST);
            }
            self.barrier(&barriers);
            for &(i, offset) in &fresh {
                let image = &rec.images[i];
                let bytes = image.as_bytes();
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.add(offset), bytes.len());
                let size = image.size();
                let region = VkBufferImageCopy {
                    bufferOffset: offset as u64,
                    bufferRowLength: 0,
                    bufferImageHeight: 0,
                    imageSubresource: layers(ASPECT_COLOR),
                    imageOffset: VkOffset3D::default(),
                    imageExtent: VkExtent3D {
                        width: size.width,
                        height: size.height,
                        depth: 1,
                    },
                };
                let key = Arc::as_ptr(image) as usize;
                let dst = self.textures[&key].img.image;
                (self.vk.vkCmdCopyBufferToImage)(
                    self.cmd,
                    buffer,
                    dst,
                    LAYOUT_TRANSFER_DST,
                    1,
                    &region,
                );
                self.uploads += 1;
            }
            let mut barriers = Vec::new();
            for &(i, _) in &fresh {
                let key = Arc::as_ptr(&rec.images[i]) as usize;
                let t = self.textures.get_mut(&key).unwrap();
                transition(&mut barriers, &mut t.img, LAYOUT_SHADER_READ_ONLY);
            }
            self.barrier(&barriers);
            Ok(views)
        }
    }

    /// One descriptor set per texture `rec` samples.
    unsafe fn descriptor_sets(
        &mut self,
        rec: &Recording,
        images: &[VkImageView],
    ) -> Result<HashMap<Texture, VkDescriptorSet>, GpuError> {
        unsafe {
            let textures = rec.textures();
            let mut sets = HashMap::new();
            if textures.is_empty() {
                return Ok(sets);
            }
            let need = textures.len() as u32;
            if self.descriptor_capacity < need {
                if self.descriptors != NULL {
                    (self.vk.vkDestroyDescriptorPool)(
                        self.device,
                        self.descriptors,
                        std::ptr::null(),
                    );
                    self.descriptors = NULL;
                    self.descriptor_capacity = 0;
                }
                let cap = need.next_power_of_two().max(64);
                let size = VkDescriptorPoolSize {
                    ty: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                    descriptorCount: cap,
                };
                let pi = VkDescriptorPoolCreateInfo {
                    sType: ST_DESCRIPTOR_POOL_CREATE_INFO,
                    pNext: std::ptr::null(),
                    flags: 0,
                    maxSets: cap,
                    poolSizeCount: 1,
                    pPoolSizes: &size,
                };
                check(
                    (self.vk.vkCreateDescriptorPool)(
                        self.device,
                        &pi,
                        std::ptr::null(),
                        &mut self.descriptors,
                    ),
                    "vkCreateDescriptorPool",
                )?;
                self.descriptor_capacity = cap;
            } else {
                check(
                    (self.vk.vkResetDescriptorPool)(self.device, self.descriptors, 0),
                    "vkResetDescriptorPool",
                )?;
            }
            let layouts = vec![self.set_layout; textures.len()];
            let ai = VkDescriptorSetAllocateInfo {
                sType: ST_DESCRIPTOR_SET_ALLOCATE_INFO,
                pNext: std::ptr::null(),
                descriptorPool: self.descriptors,
                descriptorSetCount: need,
                pSetLayouts: layouts.as_ptr(),
            };
            let mut handles = vec![NULL; textures.len()];
            check(
                (self.vk.vkAllocateDescriptorSets)(self.device, &ai, handles.as_mut_ptr()),
                "vkAllocateDescriptorSets",
            )?;
            let infos: Vec<VkDescriptorImageInfo> = textures
                .iter()
                .map(|t| {
                    let (view, sampler) = match *t {
                        Texture::Image { index, nearest } => (
                            images[index],
                            if nearest { self.nearest } else { self.linear },
                        ),
                        Texture::Layer(i) => (self.layers[i].color.view, self.linear),
                        Texture::Scratch => {
                            (self.scratch.as_ref().unwrap().color.view, self.linear)
                        }
                    };
                    VkDescriptorImageInfo {
                        sampler,
                        imageView: view,
                        imageLayout: LAYOUT_SHADER_READ_ONLY,
                    }
                })
                .collect();
            let writes: Vec<VkWriteDescriptorSet> = handles
                .iter()
                .zip(&infos)
                .map(|(&set, info)| VkWriteDescriptorSet {
                    sType: ST_WRITE_DESCRIPTOR_SET,
                    pNext: std::ptr::null(),
                    dstSet: set,
                    dstBinding: 0,
                    dstArrayElement: 0,
                    descriptorCount: 1,
                    descriptorType: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                    pImageInfo: info,
                    pBufferInfo: std::ptr::null(),
                    pTexelBufferView: std::ptr::null(),
                })
                .collect();
            (self.vk.vkUpdateDescriptorSets)(
                self.device,
                writes.len() as u32,
                writes.as_ptr(),
                0,
                std::ptr::null(),
            );
            for (t, set) in textures.into_iter().zip(handles) {
                sets.insert(t, set);
            }
            Ok(sets)
        }
    }

    /// Turn the recorded passes and draws into Vulkan commands.
    unsafe fn passes(&mut self, rec: &Recording, sets: &HashMap<Texture, VkDescriptorSet>) {
        unsafe {
            let cmd = self.cmd;
            let mut open = false;
            let mut bound: Option<Pipe> = None;
            for (i, c) in rec.cmds.iter().enumerate() {
                match c {
                    Cmd::Pass { target, clear } => {
                        if open {
                            (self.vk.vkCmdEndRenderPass)(cmd);
                        }
                        // What this pass samples moves to shader reads; its
                        // target to attachment writes.
                        let mut barriers = Vec::new();
                        for d in rec.cmds[i + 1..].iter().map_while(|c| match c {
                            Cmd::Draw(d) => Some(d),
                            Cmd::Pass { .. } => None,
                        }) {
                            let t = match d.texture {
                                Some(Texture::Layer(l)) => Target::Layer(l),
                                Some(Texture::Scratch) => Target::Scratch,
                                _ => continue,
                            };
                            let color = &mut self.surface_of(t).color;
                            transition(&mut barriers, color, LAYOUT_SHADER_READ_ONLY);
                        }
                        let surface = self.surface_of(*target);
                        transition(&mut barriers, &mut surface.color, LAYOUT_COLOR_ATTACHMENT);
                        transition(
                            &mut barriers,
                            &mut surface.stencil,
                            LAYOUT_DEPTH_STENCIL_ATTACHMENT,
                        );
                        let (fb, w, h) = (surface.framebuffer, surface.w, surface.h);
                        self.barrier(&barriers);

                        let area = VkRect2D {
                            offset: VkOffset2D::default(),
                            extent: VkExtent2D {
                                width: w as u32,
                                height: h as u32,
                            },
                        };
                        let bi = VkRenderPassBeginInfo {
                            sType: ST_RENDER_PASS_BEGIN_INFO,
                            pNext: std::ptr::null(),
                            renderPass: if *target == Target::Scratch {
                                self.scratch_pass
                            } else {
                                self.color_pass
                            },
                            framebuffer: fb,
                            renderArea: area,
                            clearValueCount: 0,
                            pClearValues: std::ptr::null(),
                        };
                        (self.vk.vkCmdBeginRenderPass)(cmd, &bi, SUBPASS_CONTENTS_INLINE);
                        open = true;
                        bound = None;
                        let viewport = VkViewport {
                            x: 0.0,
                            y: 0.0,
                            width: w as f32,
                            height: h as f32,
                            minDepth: 0.0,
                            maxDepth: 1.0,
                        };
                        (self.vk.vkCmdSetViewport)(cmd, 0, 1, &viewport);
                        if let Some(c) = clear {
                            let atts = [
                                VkClearAttachment {
                                    aspectMask: ASPECT_COLOR,
                                    colorAttachment: 0,
                                    clearValue: VkClearValue::color(*c),
                                },
                                VkClearAttachment {
                                    aspectMask: ASPECT_STENCIL,
                                    colorAttachment: 0,
                                    clearValue: VkClearValue::stencil(0),
                                },
                            ];
                            let rect = VkClearRect {
                                rect: area,
                                baseArrayLayer: 0,
                                layerCount: 1,
                            };
                            (self.vk.vkCmdClearAttachments)(cmd, 2, atts.as_ptr(), 1, &rect);
                        }
                    }
                    Cmd::Draw(d) => {
                        if !open {
                            continue;
                        }
                        self.draw(d, sets, &mut bound);
                    }
                }
            }
            if open {
                (self.vk.vkCmdEndRenderPass)(cmd);
            }
        }
    }

    unsafe fn draw(
        &self,
        d: &Draw,
        sets: &HashMap<Texture, VkDescriptorSet>,
        bound: &mut Option<Pipe>,
    ) {
        unsafe {
            let cmd = self.cmd;
            if *bound != Some(d.pipe) {
                (self.vk.vkCmdBindPipeline)(cmd, BIND_POINT_GRAPHICS, self.pipes[&d.pipe]);
                *bound = Some(d.pipe);
            }
            let s = d.scissor;
            let scissor = VkRect2D {
                offset: VkOffset2D { x: s[0], y: s[1] },
                extent: VkExtent2D {
                    width: (s[2] - s[0]) as u32,
                    height: (s[3] - s[1]) as u32,
                },
            };
            (self.vk.vkCmdSetScissor)(cmd, 0, 1, &scissor);
            // Front and back faces.
            (self.vk.vkCmdSetStencilReference)(cmd, 3, d.reference);
            (self.vk.vkCmdPushConstants)(
                cmd,
                self.layout,
                SHADER_STAGE_VERTEX | SHADER_STAGE_FRAGMENT,
                0,
                record::PUSH_BYTES,
                d.push.as_ptr().cast(),
            );
            if let Some(t) = d.texture
                && let Some(set) = sets.get(&t)
            {
                (self.vk.vkCmdBindDescriptorSets)(
                    cmd,
                    BIND_POINT_GRAPHICS,
                    self.layout,
                    0,
                    1,
                    set,
                    0,
                    std::ptr::null(),
                );
            }
            (self.vk.vkCmdDraw)(cmd, d.count, 1, d.first, 0);
        }
    }

    /// Submit the command buffer (ended here), waiting on `wait` and
    /// signaling `signal`, and fence it.
    unsafe fn submit(
        &self,
        wait: Option<VkSemaphore>,
        signal: Option<VkSemaphore>,
    ) -> Result<(), GpuError> {
        unsafe {
            check((self.vk.vkEndCommandBuffer)(self.cmd), "vkEndCommandBuffer")?;
            check(
                (self.vk.vkResetFences)(self.device, 1, &self.fence),
                "vkResetFences",
            )?;
            let stage = STAGE_ALL_COMMANDS;
            let si = VkSubmitInfo {
                sType: ST_SUBMIT_INFO,
                pNext: std::ptr::null(),
                waitSemaphoreCount: wait.is_some() as u32,
                pWaitSemaphores: wait.as_ref().map_or(std::ptr::null(), |s| s),
                pWaitDstStageMask: &stage,
                commandBufferCount: 1,
                pCommandBuffers: &self.cmd,
                signalSemaphoreCount: signal.is_some() as u32,
                pSignalSemaphores: signal.as_ref().map_or(std::ptr::null(), |s| s),
            };
            check(
                (self.vk.vkQueueSubmit)(self.queue, 1, &si, self.fence),
                "vkQueueSubmit",
            )
        }
    }

    /// End a frame's bookkeeping: free textures whose image is gone or that
    /// weren't drawn lately (none of them is in the frame just submitted).
    unsafe fn end_frame(&mut self) {
        unsafe {
            let count = self.count;
            let stale: Vec<usize> = self
                .textures
                .iter()
                .filter(|(_, t)| {
                    t.image.strong_count() == 0
                        || count.saturating_sub(t.used) >= TEXTURE_KEEP_FRAMES
                })
                .map(|(k, _)| *k)
                .collect();
            for k in stale {
                let t = self.textures.remove(&k).unwrap();
                self.free_image(t.img);
            }
            // Give back empty blocks, past the first.
            while self.blocks.len() > 1 && self.blocks.last().unwrap().ranges.is_empty() {
                let b = self.blocks.pop().unwrap();
                (self.vk.vkFreeMemory)(self.device, b.memory, std::ptr::null());
            }
            self.count += 1;
        }
    }

    /// Draw `plan` offscreen and wait for it; with `read`, return its
    /// premultiplied RGBA bytes.
    unsafe fn offscreen(&mut self, plan: &Plan, read: bool) -> Result<Vec<u8>, GpuError> {
        unsafe {
            self.wait()?;
            let encoded = self.encode(plan);
            if encoded.is_err() {
                self.end_frame();
                encoded?;
            }
            let (w, h) = (plan.size.width, plan.size.height);
            let len = w as u64 * h as u64 * 4;
            if read {
                self.ensure_buffer(|s| &mut s.readback, len, BUFFER_TRANSFER_DST, true)?;
                let region = VkBufferImageCopy {
                    bufferOffset: 0,
                    bufferRowLength: 0,
                    bufferImageHeight: 0,
                    imageSubresource: layers(ASPECT_COLOR),
                    imageOffset: VkOffset3D::default(),
                    imageExtent: VkExtent3D {
                        width: w,
                        height: h,
                        depth: 1,
                    },
                };
                let frame = self.frame.as_ref().unwrap().color.image;
                let rb = self.readback.as_ref().unwrap().buffer;
                (self.vk.vkCmdCopyImageToBuffer)(
                    self.cmd,
                    frame,
                    LAYOUT_TRANSFER_SRC,
                    rb,
                    1,
                    &region,
                );
                self.host_barrier();
            }
            let submitted = self.submit(None, None);
            self.end_frame();
            submitted?;
            self.wait()?;
            let mut out = Vec::new();
            if read {
                let rb = self.readback.as_ref().unwrap();
                out.extend_from_slice(std::slice::from_raw_parts(rb.ptr, len as usize));
            }
            Ok(out)
        }
    }
}

/// The physical device to use and its graphics queue family: a discrete
/// GPU, else an integrated one, else whatever there is (lavapipe).
unsafe fn pick_device(vk: &Fns, instance: VkInstance) -> Result<(VkPhysicalDevice, u32), String> {
    unsafe {
        let mut n = 0u32;
        (vk.vkEnumeratePhysicalDevices)(instance, &mut n, std::ptr::null_mut());
        let mut devices = vec![std::ptr::null_mut(); n as usize];
        if n > 0 {
            (vk.vkEnumeratePhysicalDevices)(instance, &mut n, devices.as_mut_ptr());
        }
        devices.truncate(n as usize);
        let mut best: Option<(u32, VkPhysicalDevice, u32)> = None;
        for dev in devices {
            let mut qn = 0u32;
            (vk.vkGetPhysicalDeviceQueueFamilyProperties)(dev, &mut qn, std::ptr::null_mut());
            let mut qs = vec![VkQueueFamilyProperties::default(); qn as usize];
            (vk.vkGetPhysicalDeviceQueueFamilyProperties)(dev, &mut qn, qs.as_mut_ptr());
            let Some(family) = qs
                .iter()
                .take(qn as usize)
                .position(|q| q.queueFlags & QUEUE_GRAPHICS != 0)
            else {
                continue;
            };
            let mut props = vec![0u8; PROPS_SIZE];
            (vk.vkGetPhysicalDeviceProperties)(dev, props.as_mut_ptr());
            let ty = u32::from_ne_bytes(
                props[PROPS_DEVICE_TYPE..PROPS_DEVICE_TYPE + 4]
                    .try_into()
                    .unwrap(),
            );
            let rank = match ty {
                DEVICE_TYPE_DISCRETE => 3,
                DEVICE_TYPE_INTEGRATED => 2,
                _ => 1,
            };
            if best.is_none_or(|(r, _, _)| rank > r) {
                best = Some((rank, dev, family as u32));
            }
        }
        best.map(|(_, d, f)| (d, f))
            .ok_or_else(|| "no Vulkan device with a graphics queue".into())
    }
}

/// The key of a window's swapchain.
fn layer_key(layer: &GpuLayer) -> Option<usize> {
    match *layer {
        GpuLayer::X11 { window } => Some(window as usize),
        GpuLayer::Android { window, .. } => Some(window as usize),
        GpuLayer::Metal(layer) => Some(layer as usize),
        _ => None,
    }
}

impl VkExecutor {
    /// The surface for `layer`.
    unsafe fn create_surface(&self, layer: &GpuLayer) -> Result<VkSurfaceKHR, GpuError> {
        unsafe {
            let mut surface = NULL;
            let r = match *layer {
                #[cfg(target_os = "linux")]
                GpuLayer::X11 { window } => {
                    let (conn, _) = crate::gpu::xcb::connection().map_err(GpuError::Unavailable)?;
                    let create = self
                        .vk
                        .vkCreateXcbSurfaceKHR
                        .ok_or_else(|| GpuError::Unavailable("no VK_KHR_xcb_surface".into()))?;
                    let ci = VkXcbSurfaceCreateInfoKHR {
                        sType: ST_XCB_SURFACE_CREATE_INFO_KHR,
                        pNext: std::ptr::null(),
                        flags: 0,
                        connection: conn,
                        window,
                    };
                    create(self.instance, &ci, std::ptr::null(), &mut surface)
                }
                #[cfg(target_os = "android")]
                GpuLayer::Android { window, .. } => {
                    let create = self
                        .vk
                        .vkCreateAndroidSurfaceKHR
                        .ok_or_else(|| GpuError::Unavailable("no VK_KHR_android_surface".into()))?;
                    let ci = VkAndroidSurfaceCreateInfoKHR {
                        sType: ST_ANDROID_SURFACE_CREATE_INFO_KHR,
                        pNext: std::ptr::null(),
                        flags: 0,
                        window,
                    };
                    create(self.instance, &ci, std::ptr::null(), &mut surface)
                }
                // MoltenVK, on the window's `CAMetalLayer` (macOS; for
                // working on this executor there).
                #[cfg(target_os = "macos")]
                GpuLayer::Metal(layer) => {
                    let create = self
                        .vk
                        .vkCreateMetalSurfaceEXT
                        .ok_or_else(|| GpuError::Unavailable("no VK_EXT_metal_surface".into()))?;
                    let ci = VkMetalSurfaceCreateInfoEXT {
                        sType: ST_METAL_SURFACE_CREATE_INFO_EXT,
                        pNext: std::ptr::null(),
                        flags: 0,
                        pLayer: layer,
                    };
                    create(self.instance, &ci, std::ptr::null(), &mut surface)
                }
                #[allow(unreachable_patterns)]
                _ => return Err(GpuError::Unavailable("not a Vulkan surface".into())),
            };
            if r != VK_SUCCESS {
                return Err(GpuError::Unavailable(format!(
                    "creating the window surface failed ({r})"
                )));
            }
            let mut supported = 0u32;
            let support = self.vk.vkGetPhysicalDeviceSurfaceSupportKHR.unwrap();
            support(self.phys, self.family, surface, &mut supported);
            if supported == 0 {
                (self.vk.vkDestroySurfaceKHR.unwrap())(self.instance, surface, std::ptr::null());
                return Err(GpuError::Unavailable(
                    "the device's queue can't present to this window".into(),
                ));
            }
            Ok(surface)
        }
    }

    /// (Re)build `swap`'s swapchain at the surface's size (or `want` where
    /// the surface lets the swapchain choose).
    unsafe fn build_swapchain(&self, swap: &mut Swap, want: (i32, i32)) -> Result<(), GpuError> {
        unsafe {
            let vk = &self.vk;
            let mut caps = VkSurfaceCapabilitiesKHR::default();
            check(
                (vk.vkGetPhysicalDeviceSurfaceCapabilitiesKHR.unwrap())(
                    self.phys,
                    swap.surface,
                    &mut caps,
                ),
                "vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
            )?;
            if caps.supportedUsageFlags & USAGE_TRANSFER_DST == 0 {
                return Err(GpuError::Unavailable(
                    "the swapchain's images can't be copied into".into(),
                ));
            }
            let extent = if caps.currentExtent.width == u32::MAX {
                VkExtent2D {
                    width: (want.0.max(1) as u32)
                        .clamp(caps.minImageExtent.width, caps.maxImageExtent.width.max(1)),
                    height: (want.1.max(1) as u32).clamp(
                        caps.minImageExtent.height,
                        caps.maxImageExtent.height.max(1),
                    ),
                }
            } else {
                caps.currentExtent
            };
            if extent.width == 0 || extent.height == 0 {
                // Minimized: nothing to show.
                return Err(GpuError::Skipped);
            }
            let mut n = 0u32;
            let formats_of = vk.vkGetPhysicalDeviceSurfaceFormatsKHR.unwrap();
            formats_of(self.phys, swap.surface, &mut n, std::ptr::null_mut());
            let mut formats = vec![VkSurfaceFormatKHR::default(); n as usize];
            formats_of(self.phys, swap.surface, &mut n, formats.as_mut_ptr());
            formats.truncate(n as usize);
            // A UNORM format: the frame's encoded values copied as they are
            // (an sRGB one would encode them again).
            let format = [FORMAT_B8G8R8A8_UNORM, FORMAT_R8G8B8A8_UNORM]
                .into_iter()
                .find(|f| {
                    formats
                        .iter()
                        .any(|s| s.format == *f && s.colorSpace == COLOR_SPACE_SRGB_NONLINEAR)
                })
                .ok_or_else(|| GpuError::Unavailable("no 8-bit UNORM swapchain format".into()))?;
            if format != FORMAT_R8G8B8A8_UNORM {
                let mut p = VkFormatProperties::default();
                (vk.vkGetPhysicalDeviceFormatProperties)(self.phys, format, &mut p);
                if p.optimalTilingFeatures & FEATURE_BLIT_DST == 0 {
                    return Err(GpuError::Unavailable(
                        "the swapchain format can't be blitted into".into(),
                    ));
                }
            }
            let mut images = caps.minImageCount.max(2) + 1;
            if caps.maxImageCount > 0 {
                images = images.min(caps.maxImageCount);
            }
            let alpha = [
                COMPOSITE_OPAQUE,
                COMPOSITE_INHERIT,
                COMPOSITE_PRE_MULTIPLIED,
                COMPOSITE_POST_MULTIPLIED,
            ]
            .into_iter()
            .find(|a| caps.supportedCompositeAlpha & a != 0)
            .unwrap_or(COMPOSITE_OPAQUE);
            let transform = if caps.supportedTransforms & TRANSFORM_IDENTITY != 0 {
                TRANSFORM_IDENTITY
            } else {
                caps.currentTransform
            };
            let ci = VkSwapchainCreateInfoKHR {
                sType: ST_SWAPCHAIN_CREATE_INFO_KHR,
                pNext: std::ptr::null(),
                flags: 0,
                surface: swap.surface,
                minImageCount: images,
                imageFormat: format,
                imageColorSpace: COLOR_SPACE_SRGB_NONLINEAR,
                imageExtent: extent,
                imageArrayLayers: 1,
                imageUsage: USAGE_TRANSFER_DST,
                imageSharingMode: SHARING_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: std::ptr::null(),
                preTransform: transform,
                compositeAlpha: alpha,
                presentMode: PRESENT_MODE_FIFO,
                clipped: 1,
                oldSwapchain: swap.swapchain,
            };
            let mut sc = NULL;
            check(
                (vk.vkCreateSwapchainKHR.unwrap())(self.device, &ci, std::ptr::null(), &mut sc),
                "vkCreateSwapchainKHR",
            )?;
            // The old one (retired by the new one) and its semaphores go once
            // the device is idle.
            (vk.vkDeviceWaitIdle)(self.device);
            self.drop_swapchain(swap);
            swap.swapchain = sc;
            let mut n = 0u32;
            let get = vk.vkGetSwapchainImagesKHR.unwrap();
            get(self.device, sc, &mut n, std::ptr::null_mut());
            let mut imgs = vec![NULL; n as usize];
            get(self.device, sc, &mut n, imgs.as_mut_ptr());
            imgs.truncate(n as usize);
            let sem = VkSemaphoreCreateInfo {
                sType: ST_SEMAPHORE_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
            };
            for _ in 0..=imgs.len() {
                let mut s = NULL;
                check(
                    (vk.vkCreateSemaphore)(self.device, &sem, std::ptr::null(), &mut s),
                    "vkCreateSemaphore",
                )?;
                if swap.acquire == NULL {
                    swap.acquire = s;
                } else {
                    swap.done.push(s);
                }
            }
            swap.images = imgs;
            swap.extent = extent;
            swap.format = format;
            swap.stale = false;
            Ok(())
        }
    }

    /// Destroy `swap`'s swapchain and semaphores (not its surface).
    unsafe fn drop_swapchain(&self, swap: &mut Swap) {
        unsafe {
            let vk = &self.vk;
            for s in swap.done.drain(..) {
                (vk.vkDestroySemaphore)(self.device, s, std::ptr::null());
            }
            if swap.acquire != NULL {
                (vk.vkDestroySemaphore)(self.device, swap.acquire, std::ptr::null());
                swap.acquire = NULL;
            }
            if swap.swapchain != NULL {
                (vk.vkDestroySwapchainKHR.unwrap())(self.device, swap.swapchain, std::ptr::null());
                swap.swapchain = NULL;
            }
            swap.images.clear();
        }
    }

    unsafe fn drop_swap(&self, mut swap: Swap) {
        unsafe {
            (self.vk.vkDeviceWaitIdle)(self.device);
            self.drop_swapchain(&mut swap);
            (self.vk.vkDestroySurfaceKHR.unwrap())(self.instance, swap.surface, std::ptr::null());
        }
    }

    unsafe fn present_to(&mut self, plan: &Plan, layer: &GpuLayer) -> Result<(), GpuError> {
        unsafe {
            let key = layer_key(layer).ok_or_else(|| GpuError::Unavailable("no window".into()))?;
            self.wait()?;
            let (w, h) = self.check_size(plan)?;
            let (want, offset) = match *layer {
                GpuLayer::Android { size, content, .. } => (size, (content[0], content[1])),
                _ => ((w, h), (0, 0)),
            };
            if !self.swaps.contains_key(&key) {
                let surface = self.create_surface(layer)?;
                self.swaps.insert(
                    key,
                    Swap {
                        surface,
                        swapchain: NULL,
                        images: Vec::new(),
                        done: Vec::new(),
                        acquire: NULL,
                        extent: VkExtent2D::default(),
                        format: 0,
                        stale: true,
                    },
                );
            }
            let mut swap = self.swaps.remove(&key).unwrap();
            let r = self.present_swap(plan, &mut swap, want, offset);
            match &r {
                Err(GpuError::Lost(_)) | Err(GpuError::Unavailable(_)) => self.drop_swap(swap),
                _ => {
                    self.swaps.insert(key, swap);
                }
            }
            r
        }
    }

    unsafe fn present_swap(
        &mut self,
        plan: &Plan,
        swap: &mut Swap,
        want: (i32, i32),
        offset: (i32, i32),
    ) -> Result<(), GpuError> {
        unsafe {
            if !swap.stale {
                // The window may have been resized since.
                let mut caps = VkSurfaceCapabilitiesKHR::default();
                (self.vk.vkGetPhysicalDeviceSurfaceCapabilitiesKHR.unwrap())(
                    self.phys,
                    swap.surface,
                    &mut caps,
                );
                if caps.currentExtent.width != u32::MAX && caps.currentExtent != swap.extent {
                    swap.stale = true;
                }
                if caps.currentExtent.width == u32::MAX
                    && (swap.extent.width, swap.extent.height) != (want.0 as u32, want.1 as u32)
                {
                    swap.stale = true;
                }
            }
            let acquire = self.vk.vkAcquireNextImageKHR.unwrap();
            let mut index = 0u32;
            let mut tries = 0;
            loop {
                if swap.stale {
                    self.build_swapchain(swap, want)?;
                }
                let r = acquire(
                    self.device,
                    swap.swapchain,
                    ACQUIRE_TIMEOUT,
                    swap.acquire,
                    NULL,
                    &mut index,
                );
                match r {
                    VK_SUCCESS => break,
                    VK_SUBOPTIMAL_KHR => {
                        // Show this one; re-create before the next.
                        swap.stale = true;
                        break;
                    }
                    VK_ERROR_OUT_OF_DATE_KHR if tries == 0 => {
                        swap.stale = true;
                        tries += 1;
                    }
                    VK_TIMEOUT | VK_NOT_READY | VK_ERROR_OUT_OF_DATE_KHR => {
                        return Err(GpuError::Skipped);
                    }
                    VK_ERROR_SURFACE_LOST_KHR => return Err(lost("the window surface", r)),
                    _ => return Err(lost("vkAcquireNextImageKHR", r)),
                }
            }
            let target = swap.images[index as usize];
            let done = swap.done[index as usize];

            let encoded = self.encode(plan);
            if encoded.is_err() {
                self.end_frame();
                encoded?;
            }
            // Into the swapchain image: the background where the frame
            // doesn't reach, then the frame.
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
            let (ew, eh) = (swap.extent.width as i32, swap.extent.height as i32);
            let (pw, ph) = (plan.size.width as i32, plan.size.height as i32);
            let (ox, oy) = (offset.0.clamp(0, ew), offset.1.clamp(0, eh));
            let (cw, ch) = ((ew - ox).min(pw), (eh - oy).min(ph));
            if (ox, oy) != (0, 0) || (cw, ch) != (ew, eh) {
                let bg = plan.background;
                let a = bg.a as f32 / 255.0;
                let clear = VkClearValue::color([
                    bg.r as f32 / 255.0 * a,
                    bg.g as f32 / 255.0 * a,
                    bg.b as f32 / 255.0 * a,
                    a,
                ]);
                let range = subresource(ASPECT_COLOR);
                (self.vk.vkCmdClearColorImage)(
                    self.cmd,
                    target,
                    LAYOUT_TRANSFER_DST,
                    &clear,
                    1,
                    &range,
                );
                self.barrier(&[]);
            }
            if cw > 0 && ch > 0 {
                let frame = self.frame.as_ref().unwrap().color.image;
                if swap.format == FORMAT_R8G8B8A8_UNORM {
                    let region = VkImageCopy {
                        srcSubresource: layers(ASPECT_COLOR),
                        srcOffset: VkOffset3D::default(),
                        dstSubresource: layers(ASPECT_COLOR),
                        dstOffset: VkOffset3D { x: ox, y: oy, z: 0 },
                        extent: VkExtent3D {
                            width: cw as u32,
                            height: ch as u32,
                            depth: 1,
                        },
                    };
                    (self.vk.vkCmdCopyImage)(
                        self.cmd,
                        frame,
                        LAYOUT_TRANSFER_SRC,
                        target,
                        LAYOUT_TRANSFER_DST,
                        1,
                        &region,
                    );
                } else {
                    let region = VkImageBlit {
                        srcSubresource: layers(ASPECT_COLOR),
                        srcOffsets: [VkOffset3D::default(), VkOffset3D { x: cw, y: ch, z: 1 }],
                        dstSubresource: layers(ASPECT_COLOR),
                        dstOffsets: [
                            VkOffset3D { x: ox, y: oy, z: 0 },
                            VkOffset3D {
                                x: ox + cw,
                                y: oy + ch,
                                z: 1,
                            },
                        ],
                    };
                    (self.vk.vkCmdBlitImage)(
                        self.cmd,
                        frame,
                        LAYOUT_TRANSFER_SRC,
                        target,
                        LAYOUT_TRANSFER_DST,
                        1,
                        &region,
                        FILTER_NEAREST,
                    );
                }
            }
            barrier.srcAccessMask = ACCESS_MEMORY_WRITE;
            barrier.oldLayout = LAYOUT_TRANSFER_DST;
            barrier.newLayout = LAYOUT_PRESENT_SRC_KHR;
            self.barrier(std::slice::from_ref(&barrier));

            let submitted = self.submit(Some(swap.acquire), Some(done));
            self.end_frame();
            submitted?;
            let pi = VkPresentInfoKHR {
                sType: ST_PRESENT_INFO_KHR,
                pNext: std::ptr::null(),
                waitSemaphoreCount: 1,
                pWaitSemaphores: &done,
                swapchainCount: 1,
                pSwapchains: &swap.swapchain,
                pImageIndices: &index,
                pResults: std::ptr::null_mut(),
            };
            let r = (self.vk.vkQueuePresentKHR.unwrap())(self.queue, &pi);
            match r {
                VK_SUCCESS => Ok(()),
                VK_SUBOPTIMAL_KHR | VK_ERROR_OUT_OF_DATE_KHR => {
                    swap.stale = true;
                    Ok(())
                }
                _ => Err(lost("vkQueuePresentKHR", r)),
            }
        }
    }
}

impl Executor for VkExecutor {
    fn api(&self) -> Api {
        Api::Vulkan
    }

    fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError> {
        let mut data = unsafe { self.offscreen(plan, true) }?;
        // Premultiplied RGBA to straight.
        for px in data.as_chunks_mut::<4>().0 {
            let a = px[3];
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                }
            }
        }
        Ok(Pixmap::from_rgba8(
            PhysicalSize::new(plan.size.width, plan.size.height),
            data,
        ))
    }

    fn finish(&mut self, plan: &Plan) -> Result<(), GpuError> {
        unsafe { self.offscreen(plan, false) }.map(drop)
    }

    fn can_present(&self, layer: &GpuLayer) -> bool {
        self.has_swapchain
            && match *layer {
                #[cfg(target_os = "linux")]
                GpuLayer::X11 { .. } => self.has_surface && crate::gpu::xcb::connection().is_ok(),
                GpuLayer::Android { window, .. } => self.has_surface && !window.is_null(),
                #[cfg(target_os = "macos")]
                GpuLayer::Metal(layer) => self.has_surface && !layer.is_null(),
                _ => false,
            }
    }

    fn present(
        &mut self,
        plan: &Plan,
        layer: &GpuLayer,
        _scale: ScaleFactor,
    ) -> Result<(), GpuError> {
        unsafe { self.present_to(plan, layer) }
    }

    fn release(&mut self, layer: &GpuLayer) {
        if let Some(swap) = layer_key(layer).and_then(|k| self.swaps.remove(&k)) {
            unsafe { self.drop_swap(swap) };
        }
    }

    fn uploads(&self) -> usize {
        self.uploads
    }
}

impl Drop for VkExecutor {
    fn drop(&mut self) {
        unsafe {
            let d = self.device;
            if d.is_null() {
                (self.vk.vkDestroyInstance)(self.instance, std::ptr::null());
                return;
            }
            (self.vk.vkDeviceWaitIdle)(d);
            for (_, swap) in std::mem::take(&mut self.swaps) {
                self.drop_swap(swap);
            }
            for (_, t) in std::mem::take(&mut self.textures) {
                self.free_image(t.img);
            }
            let surfaces: Vec<Surface2> = self
                .frame
                .take()
                .into_iter()
                .chain(self.scratch.take())
                .chain(std::mem::take(&mut self.layers))
                .collect();
            for s in surfaces {
                self.free_surface(s);
            }
            for b in [
                self.vertices.take(),
                self.staging.take(),
                self.readback.take(),
            ]
            .into_iter()
            .flatten()
            {
                self.free_buffer(b);
            }
            for b in std::mem::take(&mut self.blocks) {
                (self.vk.vkFreeMemory)(d, b.memory, std::ptr::null());
            }
            for (_, p) in std::mem::take(&mut self.pipes) {
                (self.vk.vkDestroyPipeline)(d, p, std::ptr::null());
            }
            let n = std::ptr::null();
            if self.descriptors != NULL {
                (self.vk.vkDestroyDescriptorPool)(d, self.descriptors, n);
            }
            if self.fence != NULL {
                (self.vk.vkDestroyFence)(d, self.fence, n);
            }
            if self.pool != NULL {
                (self.vk.vkDestroyCommandPool)(d, self.pool, n);
            }
            for s in [self.linear, self.nearest] {
                if s != NULL {
                    (self.vk.vkDestroySampler)(d, s, n);
                }
            }
            if self.layout != NULL {
                (self.vk.vkDestroyPipelineLayout)(d, self.layout, n);
            }
            if self.set_layout != NULL {
                (self.vk.vkDestroyDescriptorSetLayout)(d, self.set_layout, n);
            }
            for p in [self.color_pass, self.scratch_pass] {
                if p != NULL {
                    (self.vk.vkDestroyRenderPass)(d, p, n);
                }
            }
            (self.vk.vkDestroyDevice)(d, n);
            (self.vk.vkDestroyInstance)(self.instance, n);
        }
    }
}
