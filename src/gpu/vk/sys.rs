//! Raw Vulkan bindings for the scene executor: the handful of types, enum
//! values and structs it uses, written out from the Vulkan headers (no
//! `ash`), and the entry points, loaded from `libvulkan` at run time with
//! `dlopen` so a build with the `vk` feature still starts (and falls back to
//! GLES or the CPU) on a system without a Vulkan loader.
//!
//! Every entry point we call passes floats by pointer only, so the plain C
//! calling convention is right even on 32-bit ARM Android, where Vulkan uses
//! the hard-float variant.

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::{CStr, c_char, c_int, c_void};

pub(crate) type VkInstance = *mut c_void;
pub(crate) type VkPhysicalDevice = *mut c_void;
pub(crate) type VkDevice = *mut c_void;
pub(crate) type VkQueue = *mut c_void;
pub(crate) type VkCommandBuffer = *mut c_void;
pub(crate) type VkResult = i32;
pub(crate) type Handle = u64;
pub(crate) type VkImage = Handle;
pub(crate) type VkImageView = Handle;
pub(crate) type VkDeviceMemory = Handle;
pub(crate) type VkBuffer = Handle;
pub(crate) type VkRenderPass = Handle;
pub(crate) type VkFramebuffer = Handle;
pub(crate) type VkCommandPool = Handle;
pub(crate) type VkFence = Handle;
pub(crate) type VkSemaphore = Handle;
pub(crate) type VkShaderModule = Handle;
pub(crate) type VkPipelineLayout = Handle;
pub(crate) type VkPipeline = Handle;
pub(crate) type VkDescriptorSetLayout = Handle;
pub(crate) type VkDescriptorPool = Handle;
pub(crate) type VkDescriptorSet = Handle;
pub(crate) type VkSampler = Handle;
pub(crate) type VkSurfaceKHR = Handle;
pub(crate) type VkSwapchainKHR = Handle;

pub(crate) const NULL: Handle = 0;

// VkResult
pub(crate) const VK_SUCCESS: VkResult = 0;
pub(crate) const VK_NOT_READY: VkResult = 1;
pub(crate) const VK_TIMEOUT: VkResult = 2;
pub(crate) const VK_SUBOPTIMAL_KHR: VkResult = 1_000_001_003;
pub(crate) const VK_ERROR_OUT_OF_DATE_KHR: VkResult = -1_000_001_004;
pub(crate) const VK_ERROR_SURFACE_LOST_KHR: VkResult = -1_000_000_000;

// VkStructureType
pub(crate) const ST_APPLICATION_INFO: i32 = 0;
pub(crate) const ST_INSTANCE_CREATE_INFO: i32 = 1;
pub(crate) const ST_DEVICE_QUEUE_CREATE_INFO: i32 = 2;
pub(crate) const ST_DEVICE_CREATE_INFO: i32 = 3;
pub(crate) const ST_SUBMIT_INFO: i32 = 4;
pub(crate) const ST_MEMORY_ALLOCATE_INFO: i32 = 5;
pub(crate) const ST_FENCE_CREATE_INFO: i32 = 8;
pub(crate) const ST_SEMAPHORE_CREATE_INFO: i32 = 9;
pub(crate) const ST_BUFFER_CREATE_INFO: i32 = 12;
pub(crate) const ST_IMAGE_CREATE_INFO: i32 = 14;
pub(crate) const ST_IMAGE_VIEW_CREATE_INFO: i32 = 15;
pub(crate) const ST_SHADER_MODULE_CREATE_INFO: i32 = 16;
pub(crate) const ST_PIPELINE_SHADER_STAGE_CREATE_INFO: i32 = 18;
pub(crate) const ST_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: i32 = 19;
pub(crate) const ST_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: i32 = 20;
pub(crate) const ST_PIPELINE_VIEWPORT_STATE_CREATE_INFO: i32 = 22;
pub(crate) const ST_PIPELINE_RASTERIZATION_STATE_CREATE_INFO: i32 = 23;
pub(crate) const ST_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: i32 = 24;
pub(crate) const ST_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO: i32 = 25;
pub(crate) const ST_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: i32 = 26;
pub(crate) const ST_PIPELINE_DYNAMIC_STATE_CREATE_INFO: i32 = 27;
pub(crate) const ST_GRAPHICS_PIPELINE_CREATE_INFO: i32 = 28;
pub(crate) const ST_PIPELINE_LAYOUT_CREATE_INFO: i32 = 30;
pub(crate) const ST_SAMPLER_CREATE_INFO: i32 = 31;
pub(crate) const ST_DESCRIPTOR_SET_LAYOUT_CREATE_INFO: i32 = 32;
pub(crate) const ST_DESCRIPTOR_POOL_CREATE_INFO: i32 = 33;
pub(crate) const ST_DESCRIPTOR_SET_ALLOCATE_INFO: i32 = 34;
pub(crate) const ST_WRITE_DESCRIPTOR_SET: i32 = 35;
pub(crate) const ST_FRAMEBUFFER_CREATE_INFO: i32 = 37;
pub(crate) const ST_RENDER_PASS_CREATE_INFO: i32 = 38;
pub(crate) const ST_COMMAND_POOL_CREATE_INFO: i32 = 39;
pub(crate) const ST_COMMAND_BUFFER_ALLOCATE_INFO: i32 = 40;
pub(crate) const ST_COMMAND_BUFFER_BEGIN_INFO: i32 = 42;
pub(crate) const ST_RENDER_PASS_BEGIN_INFO: i32 = 43;
pub(crate) const ST_IMAGE_MEMORY_BARRIER: i32 = 45;
pub(crate) const ST_MEMORY_BARRIER: i32 = 46;
pub(crate) const ST_SWAPCHAIN_CREATE_INFO_KHR: i32 = 1_000_001_000;
pub(crate) const ST_PRESENT_INFO_KHR: i32 = 1_000_001_001;
pub(crate) const ST_PRESENT_REGIONS_KHR: i32 = 1_000_084_000;
pub(crate) const ST_XCB_SURFACE_CREATE_INFO_KHR: i32 = 1_000_005_000;
pub(crate) const ST_ANDROID_SURFACE_CREATE_INFO_KHR: i32 = 1_000_008_000;
pub(crate) const ST_METAL_SURFACE_CREATE_INFO_EXT: i32 = 1_000_217_000;

pub(crate) const API_VERSION_1_0: u32 = 1 << 22;

/// `sizeof(VkPhysicalDeviceProperties)`, and where in it are `deviceType`
/// and `limits.maxImageDimension2D` (read from the bytes rather than
/// spelling out the whole struct).
pub(crate) const PROPS_SIZE: usize = 824;
pub(crate) const PROPS_DEVICE_TYPE: usize = 16;
pub(crate) const PROPS_DEVICE_NAME: usize = 20;
pub(crate) const PROPS_MAX_IMAGE_2D: usize = 300;
pub(crate) const DEVICE_TYPE_INTEGRATED: u32 = 1;
pub(crate) const DEVICE_TYPE_DISCRETE: u32 = 2;

// VkFormat
pub(crate) const FORMAT_UNDEFINED: u32 = 0;
pub(crate) const FORMAT_R8_UNORM: u32 = 9;
pub(crate) const FORMAT_R8G8_UNORM: u32 = 16;
pub(crate) const FORMAT_R8G8B8A8_UNORM: u32 = 37;
pub(crate) const FORMAT_B8G8R8A8_UNORM: u32 = 44;
pub(crate) const FORMAT_R32G32B32A32_SFLOAT: u32 = 109;
pub(crate) const FORMAT_S8_UINT: u32 = 127;
pub(crate) const FORMAT_D16_UNORM_S8_UINT: u32 = 128;
pub(crate) const FORMAT_D24_UNORM_S8_UINT: u32 = 129;
pub(crate) const FORMAT_D32_SFLOAT_S8_UINT: u32 = 130;

// VkFormatFeatureFlagBits
pub(crate) const FEATURE_DEPTH_STENCIL_ATTACHMENT: u32 = 0x200;
pub(crate) const FEATURE_BLIT_DST: u32 = 0x800;

// VkImageUsageFlagBits
pub(crate) const USAGE_TRANSFER_SRC: u32 = 0x1;
pub(crate) const USAGE_TRANSFER_DST: u32 = 0x2;
pub(crate) const USAGE_SAMPLED: u32 = 0x4;
pub(crate) const USAGE_COLOR_ATTACHMENT: u32 = 0x10;
pub(crate) const USAGE_DEPTH_STENCIL_ATTACHMENT: u32 = 0x20;

// VkBufferUsageFlagBits
pub(crate) const BUFFER_TRANSFER_SRC: u32 = 0x1;
pub(crate) const BUFFER_TRANSFER_DST: u32 = 0x2;
pub(crate) const BUFFER_VERTEX: u32 = 0x80;

// VkImageLayout
pub(crate) const LAYOUT_UNDEFINED: u32 = 0;
pub(crate) const LAYOUT_COLOR_ATTACHMENT: u32 = 2;
pub(crate) const LAYOUT_DEPTH_STENCIL_ATTACHMENT: u32 = 3;
pub(crate) const LAYOUT_SHADER_READ_ONLY: u32 = 5;
pub(crate) const LAYOUT_TRANSFER_SRC: u32 = 6;
pub(crate) const LAYOUT_TRANSFER_DST: u32 = 7;
pub(crate) const LAYOUT_PRESENT_SRC_KHR: u32 = 1_000_001_002;

// VkImageAspectFlagBits
pub(crate) const ASPECT_COLOR: u32 = 0x1;
pub(crate) const ASPECT_DEPTH: u32 = 0x2;
pub(crate) const ASPECT_STENCIL: u32 = 0x4;

// VkMemoryPropertyFlagBits
pub(crate) const MEMORY_DEVICE_LOCAL: u32 = 0x1;
pub(crate) const MEMORY_HOST_VISIBLE: u32 = 0x2;
pub(crate) const MEMORY_HOST_COHERENT: u32 = 0x4;

// VkPipelineStageFlagBits, VkAccessFlagBits
pub(crate) const STAGE_ALL_COMMANDS: u32 = 0x1_0000;
pub(crate) const STAGE_HOST: u32 = 0x4000;
pub(crate) const ACCESS_HOST_READ: u32 = 0x2000;
pub(crate) const ACCESS_MEMORY_READ: u32 = 0x8000;
pub(crate) const ACCESS_MEMORY_WRITE: u32 = 0x1_0000;

pub(crate) const QUEUE_GRAPHICS: u32 = 0x1;
pub(crate) const QUEUE_FAMILY_IGNORED: u32 = !0;

pub(crate) const LOAD_OP_LOAD: u32 = 0;
pub(crate) const STORE_OP_STORE: u32 = 0;
pub(crate) const SAMPLE_COUNT_1: u32 = 1;
pub(crate) const IMAGE_TYPE_2D: u32 = 1;
pub(crate) const TILING_OPTIMAL: u32 = 0;
pub(crate) const SHARING_EXCLUSIVE: u32 = 0;
pub(crate) const VIEW_TYPE_2D: u32 = 1;
pub(crate) const BIND_POINT_GRAPHICS: u32 = 0;
pub(crate) const SUBPASS_CONTENTS_INLINE: u32 = 0;
pub(crate) const COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
pub(crate) const COMMAND_BUFFER_ONE_TIME_SUBMIT: u32 = 0x1;
pub(crate) const COMMAND_POOL_RESET_COMMAND_BUFFER: u32 = 0x2;
pub(crate) const FENCE_SIGNALED: u32 = 0x1;

pub(crate) const SHADER_STAGE_VERTEX: u32 = 0x1;
pub(crate) const SHADER_STAGE_FRAGMENT: u32 = 0x10;
pub(crate) const TOPOLOGY_TRIANGLE_LIST: u32 = 3;
pub(crate) const POLYGON_FILL: u32 = 0;
pub(crate) const CULL_NONE: u32 = 0;
pub(crate) const FRONT_FACE_CCW: u32 = 0;
pub(crate) const VERTEX_INPUT_RATE_VERTEX: u32 = 0;
pub(crate) const COLOR_RGBA: u32 = 0xf;
pub(crate) const BLEND_ONE: u32 = 1;
pub(crate) const BLEND_ONE_MINUS_SRC_ALPHA: u32 = 7;
pub(crate) const BLEND_OP_ADD: u32 = 0;
pub(crate) const DYNAMIC_VIEWPORT: u32 = 0;
pub(crate) const DYNAMIC_SCISSOR: u32 = 1;
pub(crate) const DYNAMIC_STENCIL_REFERENCE: u32 = 8;

// VkCompareOp, VkStencilOp
pub(crate) const COMPARE_EQUAL: u32 = 2;
pub(crate) const COMPARE_NOT_EQUAL: u32 = 5;
pub(crate) const COMPARE_ALWAYS: u32 = 7;
pub(crate) const STENCIL_KEEP: u32 = 0;
pub(crate) const STENCIL_INCR_CLAMP: u32 = 3;
pub(crate) const STENCIL_DECR_CLAMP: u32 = 4;
pub(crate) const STENCIL_INVERT: u32 = 5;
pub(crate) const STENCIL_INCR_WRAP: u32 = 6;
pub(crate) const STENCIL_DECR_WRAP: u32 = 7;

pub(crate) const FILTER_NEAREST: u32 = 0;
pub(crate) const FILTER_LINEAR: u32 = 1;
pub(crate) const MIPMAP_NEAREST: u32 = 0;
pub(crate) const ADDRESS_CLAMP_TO_EDGE: u32 = 2;
pub(crate) const DESCRIPTOR_COMBINED_IMAGE_SAMPLER: u32 = 1;

// WSI
pub(crate) const PRESENT_MODE_FIFO: u32 = 2;
pub(crate) const COLOR_SPACE_SRGB_NONLINEAR: u32 = 0;
pub(crate) const COMPOSITE_OPAQUE: u32 = 0x1;
pub(crate) const COMPOSITE_PRE_MULTIPLIED: u32 = 0x2;
pub(crate) const COMPOSITE_POST_MULTIPLIED: u32 = 0x4;
pub(crate) const COMPOSITE_INHERIT: u32 = 0x8;
pub(crate) const TRANSFORM_IDENTITY: u32 = 0x1;

#[repr(C)]
pub(crate) struct VkApplicationInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub pApplicationName: *const c_char,
    pub applicationVersion: u32,
    pub pEngineName: *const c_char,
    pub engineVersion: u32,
    pub apiVersion: u32,
}

#[repr(C)]
pub(crate) struct VkInstanceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub pApplicationInfo: *const VkApplicationInfo,
    pub enabledLayerCount: u32,
    pub ppEnabledLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub ppEnabledExtensionNames: *const *const c_char,
}

#[repr(C)]
pub(crate) struct VkExtensionProperties {
    pub extensionName: [c_char; 256],
    pub specVersion: u32,
}

#[repr(C)]
pub(crate) struct VkDeviceQueueCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueFamilyIndex: u32,
    pub queueCount: u32,
    pub pQueuePriorities: *const f32,
}

#[repr(C)]
pub(crate) struct VkDeviceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueCreateInfoCount: u32,
    pub pQueueCreateInfos: *const VkDeviceQueueCreateInfo,
    pub enabledLayerCount: u32,
    pub ppEnabledLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub ppEnabledExtensionNames: *const *const c_char,
    pub pEnabledFeatures: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkQueueFamilyProperties {
    pub queueFlags: u32,
    pub queueCount: u32,
    pub timestampValidBits: u32,
    pub minImageTransferGranularity: VkExtent3D,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkMemoryType {
    pub propertyFlags: u32,
    pub heapIndex: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkMemoryHeap {
    pub size: u64,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct VkPhysicalDeviceMemoryProperties {
    pub memoryTypeCount: u32,
    pub memoryTypes: [VkMemoryType; 32],
    pub memoryHeapCount: u32,
    pub memoryHeaps: [VkMemoryHeap; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkFormatProperties {
    pub linearTilingFeatures: u32,
    pub optimalTilingFeatures: u32,
    pub bufferFeatures: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkMemoryRequirements {
    pub size: u64,
    pub alignment: u64,
    pub memoryTypeBits: u32,
}

#[repr(C)]
pub(crate) struct VkMemoryAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub allocationSize: u64,
    pub memoryTypeIndex: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct VkExtent2D {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkExtent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkOffset2D {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkOffset3D {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkRect2D {
    pub offset: VkOffset2D,
    pub extent: VkExtent2D,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub minDepth: f32,
    pub maxDepth: f32,
}

#[repr(C)]
pub(crate) struct VkImageCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub imageType: u32,
    pub format: u32,
    pub extent: VkExtent3D,
    pub mipLevels: u32,
    pub arrayLayers: u32,
    pub samples: u32,
    pub tiling: u32,
    pub usage: u32,
    pub sharingMode: u32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub initialLayout: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkComponentMapping {
    pub r: u32,
    pub g: u32,
    pub b: u32,
    pub a: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkImageSubresourceRange {
    pub aspectMask: u32,
    pub baseMipLevel: u32,
    pub levelCount: u32,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkImageSubresourceLayers {
    pub aspectMask: u32,
    pub mipLevel: u32,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
pub(crate) struct VkImageViewCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub image: VkImage,
    pub viewType: u32,
    pub format: u32,
    pub components: VkComponentMapping,
    pub subresourceRange: VkImageSubresourceRange,
}

#[repr(C)]
pub(crate) struct VkAttachmentDescription {
    pub flags: u32,
    pub format: u32,
    pub samples: u32,
    pub loadOp: u32,
    pub storeOp: u32,
    pub stencilLoadOp: u32,
    pub stencilStoreOp: u32,
    pub initialLayout: u32,
    pub finalLayout: u32,
}

#[repr(C)]
pub(crate) struct VkAttachmentReference {
    pub attachment: u32,
    pub layout: u32,
}

#[repr(C)]
pub(crate) struct VkSubpassDescription {
    pub flags: u32,
    pub pipelineBindPoint: u32,
    pub inputAttachmentCount: u32,
    pub pInputAttachments: *const VkAttachmentReference,
    pub colorAttachmentCount: u32,
    pub pColorAttachments: *const VkAttachmentReference,
    pub pResolveAttachments: *const VkAttachmentReference,
    pub pDepthStencilAttachment: *const VkAttachmentReference,
    pub preserveAttachmentCount: u32,
    pub pPreserveAttachments: *const u32,
}

#[repr(C)]
pub(crate) struct VkRenderPassCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub attachmentCount: u32,
    pub pAttachments: *const VkAttachmentDescription,
    pub subpassCount: u32,
    pub pSubpasses: *const VkSubpassDescription,
    pub dependencyCount: u32,
    pub pDependencies: *const c_void,
}

#[repr(C)]
pub(crate) struct VkFramebufferCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub renderPass: VkRenderPass,
    pub attachmentCount: u32,
    pub pAttachments: *const VkImageView,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

#[repr(C)]
pub(crate) struct VkCommandPoolCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueFamilyIndex: u32,
}

#[repr(C)]
pub(crate) struct VkCommandBufferAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub commandPool: VkCommandPool,
    pub level: u32,
    pub commandBufferCount: u32,
}

#[repr(C)]
pub(crate) struct VkCommandBufferBeginInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub pInheritanceInfo: *const c_void,
}

/// `VkClearValue`: a color (four floats) or a depth and stencil value, in
/// the same 16 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct VkClearValue {
    pub data: [u32; 4],
}

impl VkClearValue {
    pub(crate) fn color(c: [f32; 4]) -> Self {
        VkClearValue {
            data: c.map(f32::to_bits),
        }
    }

    pub(crate) fn stencil(s: u32) -> Self {
        // `VkClearDepthStencilValue { float depth; uint32_t stencil; }`.
        VkClearValue {
            data: [0.0f32.to_bits(), s, 0, 0],
        }
    }
}

#[repr(C)]
pub(crate) struct VkRenderPassBeginInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub renderPass: VkRenderPass,
    pub framebuffer: VkFramebuffer,
    pub renderArea: VkRect2D,
    pub clearValueCount: u32,
    pub pClearValues: *const VkClearValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct VkClearAttachment {
    pub aspectMask: u32,
    pub colorAttachment: u32,
    pub clearValue: VkClearValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct VkClearRect {
    pub rect: VkRect2D,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
pub(crate) struct VkSubmitInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const VkSemaphore,
    pub pWaitDstStageMask: *const u32,
    pub commandBufferCount: u32,
    pub pCommandBuffers: *const VkCommandBuffer,
    pub signalSemaphoreCount: u32,
    pub pSignalSemaphores: *const VkSemaphore,
}

#[repr(C)]
pub(crate) struct VkFenceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
}

#[repr(C)]
pub(crate) struct VkSemaphoreCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
}

#[repr(C)]
pub(crate) struct VkBufferCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub size: u64,
    pub usage: u32,
    pub sharingMode: u32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
}

#[repr(C)]
pub(crate) struct VkBufferImageCopy {
    pub bufferOffset: u64,
    pub bufferRowLength: u32,
    pub bufferImageHeight: u32,
    pub imageSubresource: VkImageSubresourceLayers,
    pub imageOffset: VkOffset3D,
    pub imageExtent: VkExtent3D,
}

#[repr(C)]
pub(crate) struct VkImageMemoryBarrier {
    pub sType: i32,
    pub pNext: *const c_void,
    pub srcAccessMask: u32,
    pub dstAccessMask: u32,
    pub oldLayout: u32,
    pub newLayout: u32,
    pub srcQueueFamilyIndex: u32,
    pub dstQueueFamilyIndex: u32,
    pub image: VkImage,
    pub subresourceRange: VkImageSubresourceRange,
}

#[repr(C)]
pub(crate) struct VkMemoryBarrier {
    pub sType: i32,
    pub pNext: *const c_void,
    pub srcAccessMask: u32,
    pub dstAccessMask: u32,
}

#[repr(C)]
pub(crate) struct VkImageBlit {
    pub srcSubresource: VkImageSubresourceLayers,
    pub srcOffsets: [VkOffset3D; 2],
    pub dstSubresource: VkImageSubresourceLayers,
    pub dstOffsets: [VkOffset3D; 2],
}

#[repr(C)]
pub(crate) struct VkImageCopy {
    pub srcSubresource: VkImageSubresourceLayers,
    pub srcOffset: VkOffset3D,
    pub dstSubresource: VkImageSubresourceLayers,
    pub dstOffset: VkOffset3D,
    pub extent: VkExtent3D,
}

#[repr(C)]
pub(crate) struct VkSamplerCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub magFilter: u32,
    pub minFilter: u32,
    pub mipmapMode: u32,
    pub addressModeU: u32,
    pub addressModeV: u32,
    pub addressModeW: u32,
    pub mipLodBias: f32,
    pub anisotropyEnable: u32,
    pub maxAnisotropy: f32,
    pub compareEnable: u32,
    pub compareOp: u32,
    pub minLod: f32,
    pub maxLod: f32,
    pub borderColor: u32,
    pub unnormalizedCoordinates: u32,
}

#[repr(C)]
pub(crate) struct VkDescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptorType: u32,
    pub descriptorCount: u32,
    pub stageFlags: u32,
    pub pImmutableSamplers: *const VkSampler,
}

#[repr(C)]
pub(crate) struct VkDescriptorSetLayoutCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub bindingCount: u32,
    pub pBindings: *const VkDescriptorSetLayoutBinding,
}

#[repr(C)]
pub(crate) struct VkDescriptorPoolSize {
    pub ty: u32,
    pub descriptorCount: u32,
}

#[repr(C)]
pub(crate) struct VkDescriptorPoolCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub maxSets: u32,
    pub poolSizeCount: u32,
    pub pPoolSizes: *const VkDescriptorPoolSize,
}

#[repr(C)]
pub(crate) struct VkDescriptorSetAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub descriptorPool: VkDescriptorPool,
    pub descriptorSetCount: u32,
    pub pSetLayouts: *const VkDescriptorSetLayout,
}

#[repr(C)]
pub(crate) struct VkDescriptorImageInfo {
    pub sampler: VkSampler,
    pub imageView: VkImageView,
    pub imageLayout: u32,
}

#[repr(C)]
pub(crate) struct VkWriteDescriptorSet {
    pub sType: i32,
    pub pNext: *const c_void,
    pub dstSet: VkDescriptorSet,
    pub dstBinding: u32,
    pub dstArrayElement: u32,
    pub descriptorCount: u32,
    pub descriptorType: u32,
    pub pImageInfo: *const VkDescriptorImageInfo,
    pub pBufferInfo: *const c_void,
    pub pTexelBufferView: *const c_void,
}

#[repr(C)]
pub(crate) struct VkPushConstantRange {
    pub stageFlags: u32,
    pub offset: u32,
    pub size: u32,
}

#[repr(C)]
pub(crate) struct VkPipelineLayoutCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub setLayoutCount: u32,
    pub pSetLayouts: *const VkDescriptorSetLayout,
    pub pushConstantRangeCount: u32,
    pub pPushConstantRanges: *const VkPushConstantRange,
}

#[repr(C)]
pub(crate) struct VkShaderModuleCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub codeSize: usize,
    pub pCode: *const u32,
}

#[repr(C)]
pub(crate) struct VkPipelineShaderStageCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub stage: u32,
    pub module: VkShaderModule,
    pub pName: *const c_char,
    pub pSpecializationInfo: *const c_void,
}

#[repr(C)]
pub(crate) struct VkVertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub inputRate: u32,
}

#[repr(C)]
pub(crate) struct VkVertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: u32,
    pub offset: u32,
}

#[repr(C)]
pub(crate) struct VkPipelineVertexInputStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub vertexBindingDescriptionCount: u32,
    pub pVertexBindingDescriptions: *const VkVertexInputBindingDescription,
    pub vertexAttributeDescriptionCount: u32,
    pub pVertexAttributeDescriptions: *const VkVertexInputAttributeDescription,
}

#[repr(C)]
pub(crate) struct VkPipelineInputAssemblyStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub topology: u32,
    pub primitiveRestartEnable: u32,
}

#[repr(C)]
pub(crate) struct VkPipelineViewportStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub viewportCount: u32,
    pub pViewports: *const VkViewport,
    pub scissorCount: u32,
    pub pScissors: *const VkRect2D,
}

#[repr(C)]
pub(crate) struct VkPipelineRasterizationStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub depthClampEnable: u32,
    pub rasterizerDiscardEnable: u32,
    pub polygonMode: u32,
    pub cullMode: u32,
    pub frontFace: u32,
    pub depthBiasEnable: u32,
    pub depthBiasConstantFactor: f32,
    pub depthBiasClamp: f32,
    pub depthBiasSlopeFactor: f32,
    pub lineWidth: f32,
}

#[repr(C)]
pub(crate) struct VkPipelineMultisampleStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub rasterizationSamples: u32,
    pub sampleShadingEnable: u32,
    pub minSampleShading: f32,
    pub pSampleMask: *const u32,
    pub alphaToCoverageEnable: u32,
    pub alphaToOneEnable: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkStencilOpState {
    pub failOp: u32,
    pub passOp: u32,
    pub depthFailOp: u32,
    pub compareOp: u32,
    pub compareMask: u32,
    pub writeMask: u32,
    pub reference: u32,
}

#[repr(C)]
pub(crate) struct VkPipelineDepthStencilStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub depthTestEnable: u32,
    pub depthWriteEnable: u32,
    pub depthCompareOp: u32,
    pub depthBoundsTestEnable: u32,
    pub stencilTestEnable: u32,
    pub front: VkStencilOpState,
    pub back: VkStencilOpState,
    pub minDepthBounds: f32,
    pub maxDepthBounds: f32,
}

#[repr(C)]
pub(crate) struct VkPipelineColorBlendAttachmentState {
    pub blendEnable: u32,
    pub srcColorBlendFactor: u32,
    pub dstColorBlendFactor: u32,
    pub colorBlendOp: u32,
    pub srcAlphaBlendFactor: u32,
    pub dstAlphaBlendFactor: u32,
    pub alphaBlendOp: u32,
    pub colorWriteMask: u32,
}

#[repr(C)]
pub(crate) struct VkPipelineColorBlendStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub logicOpEnable: u32,
    pub logicOp: u32,
    pub attachmentCount: u32,
    pub pAttachments: *const VkPipelineColorBlendAttachmentState,
    pub blendConstants: [f32; 4],
}

#[repr(C)]
pub(crate) struct VkPipelineDynamicStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub dynamicStateCount: u32,
    pub pDynamicStates: *const u32,
}

#[repr(C)]
pub(crate) struct VkGraphicsPipelineCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub stageCount: u32,
    pub pStages: *const VkPipelineShaderStageCreateInfo,
    pub pVertexInputState: *const VkPipelineVertexInputStateCreateInfo,
    pub pInputAssemblyState: *const VkPipelineInputAssemblyStateCreateInfo,
    pub pTessellationState: *const c_void,
    pub pViewportState: *const VkPipelineViewportStateCreateInfo,
    pub pRasterizationState: *const VkPipelineRasterizationStateCreateInfo,
    pub pMultisampleState: *const VkPipelineMultisampleStateCreateInfo,
    pub pDepthStencilState: *const VkPipelineDepthStencilStateCreateInfo,
    pub pColorBlendState: *const VkPipelineColorBlendStateCreateInfo,
    pub pDynamicState: *const VkPipelineDynamicStateCreateInfo,
    pub layout: VkPipelineLayout,
    pub renderPass: VkRenderPass,
    pub subpass: u32,
    pub basePipelineHandle: VkPipeline,
    pub basePipelineIndex: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkSurfaceCapabilitiesKHR {
    pub minImageCount: u32,
    pub maxImageCount: u32,
    pub currentExtent: VkExtent2D,
    pub minImageExtent: VkExtent2D,
    pub maxImageExtent: VkExtent2D,
    pub maxImageArrayLayers: u32,
    pub supportedTransforms: u32,
    pub currentTransform: u32,
    pub supportedCompositeAlpha: u32,
    pub supportedUsageFlags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkSurfaceFormatKHR {
    pub format: u32,
    pub colorSpace: u32,
}

#[repr(C)]
pub(crate) struct VkSwapchainCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub surface: VkSurfaceKHR,
    pub minImageCount: u32,
    pub imageFormat: u32,
    pub imageColorSpace: u32,
    pub imageExtent: VkExtent2D,
    pub imageArrayLayers: u32,
    pub imageUsage: u32,
    pub imageSharingMode: u32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub preTransform: u32,
    pub compositeAlpha: u32,
    pub presentMode: u32,
    pub clipped: u32,
    pub oldSwapchain: VkSwapchainKHR,
}

#[repr(C)]
pub(crate) struct VkPresentInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const VkSemaphore,
    pub swapchainCount: u32,
    pub pSwapchains: *const VkSwapchainKHR,
    pub pImageIndices: *const u32,
    pub pResults: *mut VkResult,
}

/// `VK_GOOGLE_display_timing`: how long the swapchain's display takes per
/// refresh, in nanoseconds.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct VkRefreshCycleDurationGOOGLE {
    pub refreshDuration: u64,
}

/// `VK_KHR_incremental_present`: the parts of each swapchain's image that
/// changed, chained to a [`VkPresentInfoKHR`].
#[repr(C)]
pub(crate) struct VkPresentRegionsKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub swapchainCount: u32,
    pub pRegions: *const VkPresentRegionKHR,
}

#[repr(C)]
pub(crate) struct VkPresentRegionKHR {
    pub rectangleCount: u32,
    pub pRectangles: *const VkRectLayerKHR,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct VkRectLayerKHR {
    pub offset: VkOffset2D,
    pub extent: VkExtent2D,
    pub layer: u32,
}

#[repr(C)]
pub(crate) struct VkXcbSurfaceCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub connection: *mut c_void,
    pub window: u32,
}

#[repr(C)]
pub(crate) struct VkMetalSurfaceCreateInfoEXT {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub pLayer: *mut c_void,
}

#[repr(C)]
pub(crate) struct VkAndroidSurfaceCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub window: *mut c_void,
}

pub(crate) type PFN_vkVoidFunction = *const c_void;
type GetInstanceProcAddr =
    unsafe extern "system" fn(VkInstance, *const c_char) -> PFN_vkVoidFunction;

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2;

/// `vkGetInstanceProcAddr` from the system's Vulkan loader, loaded once.
pub(crate) fn loader() -> Result<GetInstanceProcAddr, String> {
    static GIPA: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let p = *GIPA.get_or_init(|| unsafe {
        let names: &[&CStr] = if cfg!(target_os = "android") {
            &[c"libvulkan.so"]
        } else if cfg!(target_os = "macos") {
            // The Vulkan loader over MoltenVK (the SDK's, or Homebrew's).
            &[
                c"libvulkan.1.dylib",
                c"/usr/local/lib/libvulkan.1.dylib",
                c"/opt/homebrew/lib/libvulkan.1.dylib",
            ]
        } else {
            &[c"libvulkan.so.1", c"libvulkan.so"]
        };
        for name in names {
            let lib = dlopen(name.as_ptr(), RTLD_NOW);
            if !lib.is_null() {
                let f = dlsym(lib, c"vkGetInstanceProcAddr".as_ptr());
                if !f.is_null() {
                    return f as usize;
                }
            }
        }
        0
    });
    if p == 0 {
        return Err("no Vulkan loader (libvulkan)".into());
    }
    Ok(unsafe { std::mem::transmute::<usize, GetInstanceProcAddr>(p) })
}

/// The entry points, as `unsafe extern "system" fn` pointers: the required
/// ones resolved or an error, the WSI ones `None` when the loader doesn't
/// have them.
macro_rules! entry_points {
    (
        required { $($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)* }
        optional { $($oname:ident : fn($($oarg:ty),*) $(-> $oret:ty)?;)* }
    ) => {
        pub(crate) struct Fns {
            $(pub $name: unsafe extern "system" fn($($arg),*) $(-> $ret)?,)*
            $(pub $oname: Option<unsafe extern "system" fn($($oarg),*) $(-> $oret)?>,)*
        }

        impl Fns {
            /// Resolve every entry point through `gipa` on `instance`.
            pub(crate) unsafe fn load(gipa: GetInstanceProcAddr, instance: VkInstance) -> Result<Self, String> {
                unsafe {
                    Ok(Fns {
                        $($name: {
                            let p = gipa(instance, concat!(stringify!($name), "\0").as_ptr().cast());
                            if p.is_null() {
                                return Err(concat!("no ", stringify!($name)).into());
                            }
                            std::mem::transmute::<PFN_vkVoidFunction, unsafe extern "system" fn($($arg),*) $(-> $ret)?>(p)
                        },)*
                        $($oname: {
                            let p = gipa(instance, concat!(stringify!($oname), "\0").as_ptr().cast());
                            (!p.is_null()).then(|| std::mem::transmute::<PFN_vkVoidFunction, unsafe extern "system" fn($($oarg),*) $(-> $oret)?>(p))
                        },)*
                    })
                }
            }
        }
    };
}

type P = *const c_void;

entry_points! {
    required {
        vkDestroyInstance: fn(VkInstance, P);
        vkEnumeratePhysicalDevices: fn(VkInstance, *mut u32, *mut VkPhysicalDevice) -> VkResult;
        vkGetPhysicalDeviceProperties: fn(VkPhysicalDevice, *mut u8);
        vkGetPhysicalDeviceQueueFamilyProperties: fn(VkPhysicalDevice, *mut u32, *mut VkQueueFamilyProperties);
        vkGetPhysicalDeviceMemoryProperties: fn(VkPhysicalDevice, *mut VkPhysicalDeviceMemoryProperties);
        vkGetPhysicalDeviceFormatProperties: fn(VkPhysicalDevice, u32, *mut VkFormatProperties);
        vkEnumerateDeviceExtensionProperties: fn(VkPhysicalDevice, *const c_char, *mut u32, *mut VkExtensionProperties) -> VkResult;
        vkCreateDevice: fn(VkPhysicalDevice, *const VkDeviceCreateInfo, P, *mut VkDevice) -> VkResult;
        vkDestroyDevice: fn(VkDevice, P);
        vkGetDeviceQueue: fn(VkDevice, u32, u32, *mut VkQueue);
        vkDeviceWaitIdle: fn(VkDevice) -> VkResult;
        vkCreateImage: fn(VkDevice, *const VkImageCreateInfo, P, *mut VkImage) -> VkResult;
        vkDestroyImage: fn(VkDevice, VkImage, P);
        vkGetImageMemoryRequirements: fn(VkDevice, VkImage, *mut VkMemoryRequirements);
        vkBindImageMemory: fn(VkDevice, VkImage, VkDeviceMemory, u64) -> VkResult;
        vkCreateImageView: fn(VkDevice, *const VkImageViewCreateInfo, P, *mut VkImageView) -> VkResult;
        vkDestroyImageView: fn(VkDevice, VkImageView, P);
        vkCreateBuffer: fn(VkDevice, *const VkBufferCreateInfo, P, *mut VkBuffer) -> VkResult;
        vkDestroyBuffer: fn(VkDevice, VkBuffer, P);
        vkGetBufferMemoryRequirements: fn(VkDevice, VkBuffer, *mut VkMemoryRequirements);
        vkBindBufferMemory: fn(VkDevice, VkBuffer, VkDeviceMemory, u64) -> VkResult;
        vkAllocateMemory: fn(VkDevice, *const VkMemoryAllocateInfo, P, *mut VkDeviceMemory) -> VkResult;
        vkFreeMemory: fn(VkDevice, VkDeviceMemory, P);
        vkMapMemory: fn(VkDevice, VkDeviceMemory, u64, u64, u32, *mut *mut c_void) -> VkResult;
        vkUnmapMemory: fn(VkDevice, VkDeviceMemory);
        vkCreateRenderPass: fn(VkDevice, *const VkRenderPassCreateInfo, P, *mut VkRenderPass) -> VkResult;
        vkDestroyRenderPass: fn(VkDevice, VkRenderPass, P);
        vkCreateFramebuffer: fn(VkDevice, *const VkFramebufferCreateInfo, P, *mut VkFramebuffer) -> VkResult;
        vkDestroyFramebuffer: fn(VkDevice, VkFramebuffer, P);
        vkCreateShaderModule: fn(VkDevice, *const VkShaderModuleCreateInfo, P, *mut VkShaderModule) -> VkResult;
        vkDestroyShaderModule: fn(VkDevice, VkShaderModule, P);
        vkCreatePipelineLayout: fn(VkDevice, *const VkPipelineLayoutCreateInfo, P, *mut VkPipelineLayout) -> VkResult;
        vkDestroyPipelineLayout: fn(VkDevice, VkPipelineLayout, P);
        vkCreateGraphicsPipelines: fn(VkDevice, Handle, u32, *const VkGraphicsPipelineCreateInfo, P, *mut VkPipeline) -> VkResult;
        vkDestroyPipeline: fn(VkDevice, VkPipeline, P);
        vkCreateSampler: fn(VkDevice, *const VkSamplerCreateInfo, P, *mut VkSampler) -> VkResult;
        vkDestroySampler: fn(VkDevice, VkSampler, P);
        vkCreateDescriptorSetLayout: fn(VkDevice, *const VkDescriptorSetLayoutCreateInfo, P, *mut VkDescriptorSetLayout) -> VkResult;
        vkDestroyDescriptorSetLayout: fn(VkDevice, VkDescriptorSetLayout, P);
        vkCreateDescriptorPool: fn(VkDevice, *const VkDescriptorPoolCreateInfo, P, *mut VkDescriptorPool) -> VkResult;
        vkDestroyDescriptorPool: fn(VkDevice, VkDescriptorPool, P);
        vkResetDescriptorPool: fn(VkDevice, VkDescriptorPool, u32) -> VkResult;
        vkAllocateDescriptorSets: fn(VkDevice, *const VkDescriptorSetAllocateInfo, *mut VkDescriptorSet) -> VkResult;
        vkUpdateDescriptorSets: fn(VkDevice, u32, *const VkWriteDescriptorSet, u32, P);
        vkCreateCommandPool: fn(VkDevice, *const VkCommandPoolCreateInfo, P, *mut VkCommandPool) -> VkResult;
        vkDestroyCommandPool: fn(VkDevice, VkCommandPool, P);
        vkAllocateCommandBuffers: fn(VkDevice, *const VkCommandBufferAllocateInfo, *mut VkCommandBuffer) -> VkResult;
        vkResetCommandBuffer: fn(VkCommandBuffer, u32) -> VkResult;
        vkBeginCommandBuffer: fn(VkCommandBuffer, *const VkCommandBufferBeginInfo) -> VkResult;
        vkEndCommandBuffer: fn(VkCommandBuffer) -> VkResult;
        vkCreateFence: fn(VkDevice, *const VkFenceCreateInfo, P, *mut VkFence) -> VkResult;
        vkDestroyFence: fn(VkDevice, VkFence, P);
        vkWaitForFences: fn(VkDevice, u32, *const VkFence, u32, u64) -> VkResult;
        vkResetFences: fn(VkDevice, u32, *const VkFence) -> VkResult;
        vkCreateSemaphore: fn(VkDevice, *const VkSemaphoreCreateInfo, P, *mut VkSemaphore) -> VkResult;
        vkDestroySemaphore: fn(VkDevice, VkSemaphore, P);
        vkQueueSubmit: fn(VkQueue, u32, *const VkSubmitInfo, VkFence) -> VkResult;
        vkCmdPipelineBarrier: fn(VkCommandBuffer, u32, u32, u32, u32, P, u32, P, u32, *const VkImageMemoryBarrier);
        vkCmdBeginRenderPass: fn(VkCommandBuffer, *const VkRenderPassBeginInfo, u32);
        vkCmdEndRenderPass: fn(VkCommandBuffer);
        vkCmdClearAttachments: fn(VkCommandBuffer, u32, *const VkClearAttachment, u32, *const VkClearRect);
        vkCmdClearColorImage: fn(VkCommandBuffer, VkImage, u32, *const VkClearValue, u32, *const VkImageSubresourceRange);
        vkCmdBindPipeline: fn(VkCommandBuffer, u32, VkPipeline);
        vkCmdBindVertexBuffers: fn(VkCommandBuffer, u32, u32, *const VkBuffer, *const u64);
        vkCmdBindDescriptorSets: fn(VkCommandBuffer, u32, VkPipelineLayout, u32, u32, *const VkDescriptorSet, u32, *const u32);
        vkCmdPushConstants: fn(VkCommandBuffer, VkPipelineLayout, u32, u32, u32, P);
        vkCmdSetViewport: fn(VkCommandBuffer, u32, u32, *const VkViewport);
        vkCmdSetScissor: fn(VkCommandBuffer, u32, u32, *const VkRect2D);
        vkCmdSetStencilReference: fn(VkCommandBuffer, u32, u32);
        vkCmdDraw: fn(VkCommandBuffer, u32, u32, u32, u32);
        vkCmdCopyBufferToImage: fn(VkCommandBuffer, VkBuffer, VkImage, u32, u32, *const VkBufferImageCopy);
        vkCmdCopyImageToBuffer: fn(VkCommandBuffer, VkImage, u32, VkBuffer, u32, *const VkBufferImageCopy);
        vkCmdBlitImage: fn(VkCommandBuffer, VkImage, u32, VkImage, u32, u32, *const VkImageBlit, u32);
        vkCmdCopyImage: fn(VkCommandBuffer, VkImage, u32, VkImage, u32, u32, *const VkImageCopy);
    }
    optional {
        vkDestroySurfaceKHR: fn(VkInstance, VkSurfaceKHR, P);
        vkGetPhysicalDeviceSurfaceSupportKHR: fn(VkPhysicalDevice, u32, VkSurfaceKHR, *mut u32) -> VkResult;
        vkGetPhysicalDeviceSurfaceCapabilitiesKHR: fn(VkPhysicalDevice, VkSurfaceKHR, *mut VkSurfaceCapabilitiesKHR) -> VkResult;
        vkGetPhysicalDeviceSurfaceFormatsKHR: fn(VkPhysicalDevice, VkSurfaceKHR, *mut u32, *mut VkSurfaceFormatKHR) -> VkResult;
        vkCreateSwapchainKHR: fn(VkDevice, *const VkSwapchainCreateInfoKHR, P, *mut VkSwapchainKHR) -> VkResult;
        vkDestroySwapchainKHR: fn(VkDevice, VkSwapchainKHR, P);
        vkGetSwapchainImagesKHR: fn(VkDevice, VkSwapchainKHR, *mut u32, *mut VkImage) -> VkResult;
        vkAcquireNextImageKHR: fn(VkDevice, VkSwapchainKHR, u64, VkSemaphore, VkFence, *mut u32) -> VkResult;
        vkQueuePresentKHR: fn(VkQueue, *const VkPresentInfoKHR) -> VkResult;
        vkGetRefreshCycleDurationGOOGLE: fn(VkDevice, VkSwapchainKHR, *mut VkRefreshCycleDurationGOOGLE) -> VkResult;
        vkCreateXcbSurfaceKHR: fn(VkInstance, *const VkXcbSurfaceCreateInfoKHR, P, *mut VkSurfaceKHR) -> VkResult;
        vkCreateAndroidSurfaceKHR: fn(VkInstance, *const VkAndroidSurfaceCreateInfoKHR, P, *mut VkSurfaceKHR) -> VkResult;
        vkGetMemoryFdPropertiesKHR: fn(VkDevice, u32, i32, *mut c_void) -> VkResult;
        vkGetPhysicalDeviceFeatures2KHR: fn(VkPhysicalDevice, *mut c_void);
        vkGetDeviceProcAddr: fn(VkDevice, *const c_char) -> PFN_vkVoidFunction;
        vkCreateMetalSurfaceEXT: fn(VkInstance, *const VkMetalSurfaceCreateInfoEXT, P, *mut VkSurfaceKHR) -> VkResult;
    }
}

/// The global entry points (before there is an instance).
pub(crate) struct Globals {
    pub vkCreateInstance:
        unsafe extern "system" fn(*const VkInstanceCreateInfo, P, *mut VkInstance) -> VkResult,
    pub vkEnumerateInstanceExtensionProperties:
        unsafe extern "system" fn(*const c_char, *mut u32, *mut VkExtensionProperties) -> VkResult,
}

impl Globals {
    pub(crate) unsafe fn load(gipa: GetInstanceProcAddr) -> Result<Self, String> {
        unsafe {
            let create = gipa(std::ptr::null_mut(), c"vkCreateInstance".as_ptr());
            let exts = gipa(
                std::ptr::null_mut(),
                c"vkEnumerateInstanceExtensionProperties".as_ptr(),
            );
            if create.is_null() || exts.is_null() {
                return Err("the Vulkan loader has no vkCreateInstance".into());
            }
            type Create = unsafe extern "system" fn(
                *const VkInstanceCreateInfo,
                P,
                *mut VkInstance,
            ) -> VkResult;
            type Exts = unsafe extern "system" fn(
                *const c_char,
                *mut u32,
                *mut VkExtensionProperties,
            ) -> VkResult;
            Ok(Globals {
                vkCreateInstance: std::mem::transmute::<PFN_vkVoidFunction, Create>(create),
                vkEnumerateInstanceExtensionProperties: std::mem::transmute::<
                    PFN_vkVoidFunction,
                    Exts,
                >(exts),
            })
        }
    }
}

/// An extension's name as a `&CStr`.
pub(crate) fn ext_name(p: &VkExtensionProperties) -> &CStr {
    // The name is NUL-terminated within its 256 bytes.
    unsafe { CStr::from_ptr(p.extensionName.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_c_headers() {
        // Sizes from the Vulkan headers (`sizeof` on a 64-bit target).
        if std::mem::size_of::<usize>() != 8 {
            return;
        }
        let sizes = [
            (std::mem::size_of::<VkImageCreateInfo>(), 88),
            (std::mem::size_of::<VkImageMemoryBarrier>(), 72),
            (std::mem::size_of::<VkGraphicsPipelineCreateInfo>(), 144),
            (std::mem::size_of::<VkSwapchainCreateInfoKHR>(), 104),
            (std::mem::size_of::<VkSamplerCreateInfo>(), 80),
            (std::mem::size_of::<VkWriteDescriptorSet>(), 64),
            (std::mem::size_of::<VkBufferImageCopy>(), 56),
            (std::mem::size_of::<VkPhysicalDeviceMemoryProperties>(), 520),
            (std::mem::size_of::<VkClearValue>(), 16),
            (std::mem::size_of::<VkStencilOpState>(), 28),
            (std::mem::size_of::<VkSurfaceCapabilitiesKHR>(), 52),
            (std::mem::size_of::<VkQueueFamilyProperties>(), 24),
            (std::mem::size_of::<VkExtensionProperties>(), 260),
            (
                std::mem::size_of::<VkPipelineDepthStencilStateCreateInfo>(),
                104,
            ),
            (
                std::mem::size_of::<VkPipelineRasterizationStateCreateInfo>(),
                64,
            ),
            (
                std::mem::size_of::<VkPipelineMultisampleStateCreateInfo>(),
                48,
            ),
            (
                std::mem::size_of::<VkPipelineColorBlendStateCreateInfo>(),
                56,
            ),
            (std::mem::size_of::<VkRenderPassBeginInfo>(), 64),
            (std::mem::size_of::<VkSubmitInfo>(), 72),
            (std::mem::size_of::<VkPresentInfoKHR>(), 64),
            (std::mem::size_of::<VkPresentRegionsKHR>(), 32),
            (std::mem::size_of::<VkPresentRegionKHR>(), 16),
            (std::mem::size_of::<VkRectLayerKHR>(), 20),
            (std::mem::size_of::<VkImageBlit>(), 80),
            (std::mem::size_of::<VkImageCopy>(), 68),
            (std::mem::size_of::<VkClearAttachment>(), 24),
            (std::mem::size_of::<VkClearRect>(), 24),
            (std::mem::size_of::<VkAttachmentDescription>(), 36),
            (std::mem::size_of::<VkSubpassDescription>(), 72),
            (std::mem::size_of::<VkRenderPassCreateInfo>(), 64),
            (std::mem::size_of::<VkFramebufferCreateInfo>(), 64),
            (std::mem::size_of::<VkImageViewCreateInfo>(), 80),
            (std::mem::size_of::<VkDescriptorPoolCreateInfo>(), 40),
            (std::mem::size_of::<VkDescriptorSetAllocateInfo>(), 40),
            (std::mem::size_of::<VkDescriptorSetLayoutBinding>(), 24),
            (std::mem::size_of::<VkPipelineLayoutCreateInfo>(), 48),
            (std::mem::size_of::<VkPipelineShaderStageCreateInfo>(), 48),
            (std::mem::size_of::<VkShaderModuleCreateInfo>(), 40),
            (
                std::mem::size_of::<VkPipelineVertexInputStateCreateInfo>(),
                48,
            ),
            (std::mem::size_of::<VkPipelineViewportStateCreateInfo>(), 48),
            (std::mem::size_of::<VkBufferCreateInfo>(), 56),
            (std::mem::size_of::<VkMemoryAllocateInfo>(), 32),
            (std::mem::size_of::<VkMemoryRequirements>(), 24),
            (std::mem::size_of::<VkDeviceCreateInfo>(), 72),
            (std::mem::size_of::<VkInstanceCreateInfo>(), 64),
            (std::mem::size_of::<VkApplicationInfo>(), 48),
            (std::mem::size_of::<VkDeviceQueueCreateInfo>(), 40),
            (std::mem::size_of::<VkCommandBufferBeginInfo>(), 32),
            (std::mem::size_of::<VkCommandBufferAllocateInfo>(), 32),
            (std::mem::size_of::<VkCommandPoolCreateInfo>(), 24),
            (std::mem::size_of::<VkDescriptorImageInfo>(), 24),
            (
                std::mem::size_of::<VkPipelineColorBlendAttachmentState>(),
                32,
            ),
            (std::mem::size_of::<VkPipelineDynamicStateCreateInfo>(), 32),
            (
                std::mem::size_of::<VkPipelineInputAssemblyStateCreateInfo>(),
                32,
            ),
            (std::mem::size_of::<VkFormatProperties>(), 12),
            (std::mem::size_of::<VkViewport>(), 24),
            (std::mem::size_of::<VkFenceCreateInfo>(), 24),
            (std::mem::size_of::<VkSemaphoreCreateInfo>(), 24),
            (std::mem::size_of::<VkDescriptorSetLayoutCreateInfo>(), 32),
            (std::mem::size_of::<VkPushConstantRange>(), 12),
            (std::mem::size_of::<VkVertexInputBindingDescription>(), 12),
            (std::mem::size_of::<VkVertexInputAttributeDescription>(), 16),
            (std::mem::size_of::<VkDescriptorPoolSize>(), 8),
            (std::mem::size_of::<VkAttachmentReference>(), 8),
            (std::mem::size_of::<VkMemoryBarrier>(), 24),
        ];
        for (i, (rust, c)) in sizes.iter().enumerate() {
            assert_eq!(rust, c, "struct #{i}");
        }
    }
}
