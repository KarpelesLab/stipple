//! Raw Vulkan FFI foundation — no `ash`/`vulkano`, just `libvulkan` and the
//! Vulkan C structs, matching the "close to the OS" policy.
//!
//! This is the entry point for a future GPU-native Vulkan render backend: it
//! creates a `VkInstance` and enumerates the physical devices, returning their
//! names. The full offscreen pipeline (device + queues, image + memory, render
//! pass, SPIR-V pipeline, command buffers, readback) builds on this. It runs
//! headlessly under Mesa's **lavapipe** software Vulkan ICD in CI.

#![allow(unsafe_code, non_snake_case, non_upper_case_globals)]

use core::ffi::{c_char, c_void};

type VkInstance = *mut c_void;
type VkPhysicalDevice = *mut c_void;
type VkResult = i32;

type VkDevice = *mut c_void;
type VkQueue = *mut c_void;

const VK_SUCCESS: VkResult = 0;
const VK_STRUCTURE_TYPE_APPLICATION_INFO: i32 = 0;
const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: i32 = 1;
const VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO: i32 = 2;
const VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO: i32 = 3;
const VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO: i32 = 5;
const VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO: i32 = 14;
const VK_QUEUE_GRAPHICS_BIT: u32 = 0x1;
// VkQueueFamilyProperties is 24 bytes; queueFlags is its first u32.
const QUEUE_FAMILY_PROPS_SIZE: usize = 24;
const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
const VK_IMAGE_TYPE_2D: u32 = 1;
const VK_IMAGE_TILING_OPTIMAL: u32 = 0;
const VK_IMAGE_USAGE_TRANSFER_SRC_BIT: u32 = 0x1;
const VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT: u32 = 0x10;
const VK_SHARING_MODE_EXCLUSIVE: u32 = 0;
const VK_IMAGE_LAYOUT_UNDEFINED: u32 = 0;
const VK_SAMPLE_COUNT_1_BIT: u32 = 1;
const VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT: u32 = 0x1;
// VkPhysicalDeviceMemoryProperties: memoryTypeCount (u32) then memoryTypes[32]
// of VkMemoryType { propertyFlags: u32, heapIndex: u32 } (8 bytes each).
const MEM_TYPES_OFFSET: usize = 4;
const MEM_TYPE_STRIDE: usize = 8;

const VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO: i32 = 15;
const VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO: i32 = 37;
const VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO: i32 = 38;
const VK_IMAGE_VIEW_TYPE_2D: u32 = 1;
const VK_IMAGE_ASPECT_COLOR_BIT: u32 = 0x1;
const VK_ATTACHMENT_LOAD_OP_CLEAR: u32 = 1;
const VK_ATTACHMENT_STORE_OP_STORE: u32 = 0;
const VK_ATTACHMENT_LOAD_OP_DONT_CARE: u32 = 2;
const VK_ATTACHMENT_STORE_OP_DONT_CARE: u32 = 1;
const VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL: u32 = 2;
const VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL: u32 = 6;
const VK_PIPELINE_BIND_POINT_GRAPHICS: u32 = 0;
const VK_COMPONENT_SWIZZLE_IDENTITY: u32 = 0;

const VK_STRUCTURE_TYPE_SUBMIT_INFO: i32 = 4;
const VK_STRUCTURE_TYPE_FENCE_CREATE_INFO: i32 = 8;
const VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO: i32 = 39;
const VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO: i32 = 40;
const VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO: i32 = 42;
const VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO: i32 = 43;
const VK_COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
const VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: u32 = 0x1;
const VK_SUBPASS_CONTENTS_INLINE: u32 = 0;
const VK_TRUE: u32 = 1;

const VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO: i32 = 12;
const VK_BUFFER_USAGE_TRANSFER_DST_BIT: u32 = 0x2;
const VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT: u32 = 0x2;
const VK_MEMORY_PROPERTY_HOST_COHERENT_BIT: u32 = 0x4;

const VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO: i32 = 16;
const VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO: i32 = 18;
const VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: i32 = 19;
const VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: i32 = 20;
const VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO: i32 = 22;
const VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO: i32 = 23;
const VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: i32 = 24;
const VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: i32 = 26;
const VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO: i32 = 28;
const VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO: i32 = 30;
const VK_SHADER_STAGE_VERTEX_BIT: u32 = 0x1;
const VK_SHADER_STAGE_FRAGMENT_BIT: u32 = 0x10;
const VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: u32 = 3;
const VK_POLYGON_MODE_FILL: u32 = 0;
const VK_CULL_MODE_NONE: u32 = 0;
const VK_FRONT_FACE_COUNTER_CLOCKWISE: u32 = 0;
const VK_COLOR_COMPONENT_RGBA: u32 = 0x1 | 0x2 | 0x4 | 0x8;
// VK_API_VERSION_1_0 = VK_MAKE_API_VERSION(0, 1, 0, 0) = 1 << 22.
const VK_API_VERSION_1_0: u32 = 1 << 22;
// VkPhysicalDeviceProperties.deviceName follows five u32 fields (apiVersion,
// driverVersion, vendorID, deviceID, deviceType) → byte offset 20, length 256.
const DEVICE_NAME_OFFSET: usize = 20;
const VK_MAX_PHYSICAL_DEVICE_NAME_SIZE: usize = 256;

#[repr(C)]
struct VkApplicationInfo {
    sType: i32,
    pNext: *const c_void,
    pApplicationName: *const c_char,
    applicationVersion: u32,
    pEngineName: *const c_char,
    engineVersion: u32,
    apiVersion: u32,
}

#[repr(C)]
struct VkInstanceCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    pApplicationInfo: *const VkApplicationInfo,
    enabledLayerCount: u32,
    ppEnabledLayerNames: *const *const c_char,
    enabledExtensionCount: u32,
    ppEnabledExtensionNames: *const *const c_char,
}

#[repr(C)]
struct VkDeviceQueueCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    queueFamilyIndex: u32,
    queueCount: u32,
    pQueuePriorities: *const f32,
}

#[repr(C)]
struct VkDeviceCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    queueCreateInfoCount: u32,
    pQueueCreateInfos: *const VkDeviceQueueCreateInfo,
    enabledLayerCount: u32,
    ppEnabledLayerNames: *const *const c_char,
    enabledExtensionCount: u32,
    ppEnabledExtensionNames: *const *const c_char,
    pEnabledFeatures: *const c_void,
}

type VkImage = u64;
type VkDeviceMemory = u64;
type VkImageView = u64;
type VkRenderPass = u64;
type VkFramebuffer = u64;
type VkCommandPool = u64;
type VkFence = u64;
type VkBuffer = u64;
type VkShaderModule = u64;
type VkPipelineLayout = u64;
type VkPipeline = u64;
// VkCommandBuffer is a *dispatchable* handle (pointer-sized), not a u64 like the
// non-dispatchable handles above.
type VkCommandBuffer = *mut c_void;

#[repr(C)]
struct VkExtent3D {
    width: u32,
    height: u32,
    depth: u32,
}

#[repr(C)]
struct VkImageCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    imageType: u32,
    format: u32,
    extent: VkExtent3D,
    mipLevels: u32,
    arrayLayers: u32,
    samples: u32,
    tiling: u32,
    usage: u32,
    sharingMode: u32,
    queueFamilyIndexCount: u32,
    pQueueFamilyIndices: *const u32,
    initialLayout: u32,
}

#[repr(C)]
struct VkMemoryRequirements {
    size: u64,
    alignment: u64,
    memoryTypeBits: u32,
}

#[repr(C)]
struct VkMemoryAllocateInfo {
    sType: i32,
    pNext: *const c_void,
    allocationSize: u64,
    memoryTypeIndex: u32,
}

#[repr(C)]
struct VkComponentMapping {
    r: u32,
    g: u32,
    b: u32,
    a: u32,
}

#[repr(C)]
struct VkImageSubresourceRange {
    aspectMask: u32,
    baseMipLevel: u32,
    levelCount: u32,
    baseArrayLayer: u32,
    layerCount: u32,
}

#[repr(C)]
struct VkImageViewCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    image: VkImage,
    viewType: u32,
    format: u32,
    components: VkComponentMapping,
    subresourceRange: VkImageSubresourceRange,
}

#[repr(C)]
struct VkAttachmentDescription {
    flags: u32,
    format: u32,
    samples: u32,
    loadOp: u32,
    storeOp: u32,
    stencilLoadOp: u32,
    stencilStoreOp: u32,
    initialLayout: u32,
    finalLayout: u32,
}

#[repr(C)]
struct VkAttachmentReference {
    attachment: u32,
    layout: u32,
}

#[repr(C)]
struct VkSubpassDescription {
    flags: u32,
    pipelineBindPoint: u32,
    inputAttachmentCount: u32,
    pInputAttachments: *const VkAttachmentReference,
    colorAttachmentCount: u32,
    pColorAttachments: *const VkAttachmentReference,
    pResolveAttachments: *const VkAttachmentReference,
    pDepthStencilAttachment: *const VkAttachmentReference,
    preserveAttachmentCount: u32,
    pPreserveAttachments: *const u32,
}

#[repr(C)]
struct VkRenderPassCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    attachmentCount: u32,
    pAttachments: *const VkAttachmentDescription,
    subpassCount: u32,
    pSubpasses: *const VkSubpassDescription,
    dependencyCount: u32,
    pDependencies: *const c_void,
}

#[repr(C)]
struct VkFramebufferCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    renderPass: VkRenderPass,
    attachmentCount: u32,
    pAttachments: *const VkImageView,
    width: u32,
    height: u32,
    layers: u32,
}

#[repr(C)]
struct VkCommandPoolCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    queueFamilyIndex: u32,
}

#[repr(C)]
struct VkCommandBufferAllocateInfo {
    sType: i32,
    pNext: *const c_void,
    commandPool: VkCommandPool,
    level: u32,
    commandBufferCount: u32,
}

#[repr(C)]
struct VkCommandBufferBeginInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    pInheritanceInfo: *const c_void,
}

#[repr(C)]
struct VkOffset2D {
    x: i32,
    y: i32,
}

#[repr(C)]
struct VkExtent2D {
    width: u32,
    height: u32,
}

#[repr(C)]
struct VkRect2D {
    offset: VkOffset2D,
    extent: VkExtent2D,
}

/// `VkClearValue` is a union; for a color attachment its largest relevant member
/// is `VkClearColorValue { float32: [f32; 4] }` (16 bytes), which is what we use.
#[repr(C)]
struct VkClearValue {
    float32: [f32; 4],
}

#[repr(C)]
struct VkRenderPassBeginInfo {
    sType: i32,
    pNext: *const c_void,
    renderPass: VkRenderPass,
    framebuffer: VkFramebuffer,
    renderArea: VkRect2D,
    clearValueCount: u32,
    pClearValues: *const VkClearValue,
}

#[repr(C)]
struct VkSubmitInfo {
    sType: i32,
    pNext: *const c_void,
    waitSemaphoreCount: u32,
    pWaitSemaphores: *const u64,
    pWaitDstStageMask: *const u32,
    commandBufferCount: u32,
    pCommandBuffers: *const VkCommandBuffer,
    signalSemaphoreCount: u32,
    pSignalSemaphores: *const u64,
}

#[repr(C)]
struct VkFenceCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
}

#[repr(C)]
struct VkBufferCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    size: u64,
    usage: u32,
    sharingMode: u32,
    queueFamilyIndexCount: u32,
    pQueueFamilyIndices: *const u32,
}

#[repr(C)]
struct VkImageSubresourceLayers {
    aspectMask: u32,
    mipLevel: u32,
    baseArrayLayer: u32,
    layerCount: u32,
}

#[repr(C)]
struct VkOffset3D {
    x: i32,
    y: i32,
    z: i32,
}

#[repr(C)]
struct VkBufferImageCopy {
    bufferOffset: u64,
    bufferRowLength: u32,
    bufferImageHeight: u32,
    imageSubresource: VkImageSubresourceLayers,
    imageOffset: VkOffset3D,
    imageExtent: VkExtent3D,
}

#[repr(C)]
struct VkShaderModuleCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    codeSize: usize,
    pCode: *const u32,
}

#[repr(C)]
struct VkPipelineShaderStageCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    stage: u32,
    module: VkShaderModule,
    pName: *const c_char,
    pSpecializationInfo: *const c_void,
}

#[repr(C)]
struct VkPipelineVertexInputStateCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    vertexBindingDescriptionCount: u32,
    pVertexBindingDescriptions: *const c_void,
    vertexAttributeDescriptionCount: u32,
    pVertexAttributeDescriptions: *const c_void,
}

#[repr(C)]
struct VkPipelineInputAssemblyStateCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    topology: u32,
    primitiveRestartEnable: u32,
}

#[repr(C)]
struct VkViewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    minDepth: f32,
    maxDepth: f32,
}

#[repr(C)]
struct VkPipelineViewportStateCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    viewportCount: u32,
    pViewports: *const VkViewport,
    scissorCount: u32,
    pScissors: *const VkRect2D,
}

#[repr(C)]
struct VkPipelineRasterizationStateCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    depthClampEnable: u32,
    rasterizerDiscardEnable: u32,
    polygonMode: u32,
    cullMode: u32,
    frontFace: u32,
    depthBiasEnable: u32,
    depthBiasConstantFactor: f32,
    depthBiasClamp: f32,
    depthBiasSlopeFactor: f32,
    lineWidth: f32,
}

#[repr(C)]
struct VkPipelineMultisampleStateCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    rasterizationSamples: u32,
    sampleShadingEnable: u32,
    minSampleShading: f32,
    pSampleMask: *const u32,
    alphaToCoverageEnable: u32,
    alphaToOneEnable: u32,
}

#[repr(C)]
struct VkPipelineColorBlendAttachmentState {
    blendEnable: u32,
    srcColorBlendFactor: u32,
    dstColorBlendFactor: u32,
    colorBlendOp: u32,
    srcAlphaBlendFactor: u32,
    dstAlphaBlendFactor: u32,
    alphaBlendOp: u32,
    colorWriteMask: u32,
}

#[repr(C)]
struct VkPipelineColorBlendStateCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    logicOpEnable: u32,
    logicOp: u32,
    attachmentCount: u32,
    pAttachments: *const VkPipelineColorBlendAttachmentState,
    blendConstants: [f32; 4],
}

#[repr(C)]
struct VkPipelineLayoutCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    setLayoutCount: u32,
    pSetLayouts: *const u64,
    pushConstantRangeCount: u32,
    pPushConstantRanges: *const c_void,
}

#[repr(C)]
struct VkGraphicsPipelineCreateInfo {
    sType: i32,
    pNext: *const c_void,
    flags: u32,
    stageCount: u32,
    pStages: *const VkPipelineShaderStageCreateInfo,
    pVertexInputState: *const VkPipelineVertexInputStateCreateInfo,
    pInputAssemblyState: *const VkPipelineInputAssemblyStateCreateInfo,
    pTessellationState: *const c_void,
    pViewportState: *const VkPipelineViewportStateCreateInfo,
    pRasterizationState: *const VkPipelineRasterizationStateCreateInfo,
    pMultisampleState: *const VkPipelineMultisampleStateCreateInfo,
    pDepthStencilState: *const c_void,
    pColorBlendState: *const VkPipelineColorBlendStateCreateInfo,
    pDynamicState: *const c_void,
    layout: VkPipelineLayout,
    renderPass: VkRenderPass,
    subpass: u32,
    basePipelineHandle: VkPipeline,
    basePipelineIndex: i32,
}

#[link(name = "vulkan")]
unsafe extern "C" {
    fn vkCreateInstance(
        pCreateInfo: *const VkInstanceCreateInfo,
        pAllocator: *const c_void,
        pInstance: *mut VkInstance,
    ) -> VkResult;
    fn vkDestroyInstance(instance: VkInstance, pAllocator: *const c_void);
    fn vkEnumeratePhysicalDevices(
        instance: VkInstance,
        pPhysicalDeviceCount: *mut u32,
        pPhysicalDevices: *mut VkPhysicalDevice,
    ) -> VkResult;
    // Writes a VkPhysicalDeviceProperties (~824 bytes); we hand it a generous
    // byte buffer and read only `deviceName` out of it.
    fn vkGetPhysicalDeviceProperties(physicalDevice: VkPhysicalDevice, pProperties: *mut c_void);
    fn vkGetPhysicalDeviceQueueFamilyProperties(
        physicalDevice: VkPhysicalDevice,
        pQueueFamilyPropertyCount: *mut u32,
        pQueueFamilyProperties: *mut c_void,
    );
    fn vkCreateDevice(
        physicalDevice: VkPhysicalDevice,
        pCreateInfo: *const VkDeviceCreateInfo,
        pAllocator: *const c_void,
        pDevice: *mut VkDevice,
    ) -> VkResult;
    fn vkDestroyDevice(device: VkDevice, pAllocator: *const c_void);
    fn vkGetDeviceQueue(
        device: VkDevice,
        queueFamilyIndex: u32,
        queueIndex: u32,
        pQueue: *mut VkQueue,
    );
    fn vkCreateImage(
        device: VkDevice,
        pCreateInfo: *const VkImageCreateInfo,
        pAllocator: *const c_void,
        pImage: *mut VkImage,
    ) -> VkResult;
    fn vkDestroyImage(device: VkDevice, image: VkImage, pAllocator: *const c_void);
    fn vkGetImageMemoryRequirements(
        device: VkDevice,
        image: VkImage,
        pMemoryRequirements: *mut VkMemoryRequirements,
    );
    // Writes a VkPhysicalDeviceMemoryProperties (~520 bytes); we hand it a
    // generous byte buffer and read the memoryType property flags out of it.
    fn vkGetPhysicalDeviceMemoryProperties(
        physicalDevice: VkPhysicalDevice,
        pMemoryProperties: *mut c_void,
    );
    fn vkAllocateMemory(
        device: VkDevice,
        pAllocateInfo: *const VkMemoryAllocateInfo,
        pAllocator: *const c_void,
        pMemory: *mut VkDeviceMemory,
    ) -> VkResult;
    fn vkFreeMemory(device: VkDevice, memory: VkDeviceMemory, pAllocator: *const c_void);
    fn vkBindImageMemory(
        device: VkDevice,
        image: VkImage,
        memory: VkDeviceMemory,
        memoryOffset: u64,
    ) -> VkResult;
    fn vkCreateImageView(
        device: VkDevice,
        pCreateInfo: *const VkImageViewCreateInfo,
        pAllocator: *const c_void,
        pView: *mut VkImageView,
    ) -> VkResult;
    fn vkDestroyImageView(device: VkDevice, imageView: VkImageView, pAllocator: *const c_void);
    fn vkCreateRenderPass(
        device: VkDevice,
        pCreateInfo: *const VkRenderPassCreateInfo,
        pAllocator: *const c_void,
        pRenderPass: *mut VkRenderPass,
    ) -> VkResult;
    fn vkDestroyRenderPass(device: VkDevice, renderPass: VkRenderPass, pAllocator: *const c_void);
    fn vkCreateFramebuffer(
        device: VkDevice,
        pCreateInfo: *const VkFramebufferCreateInfo,
        pAllocator: *const c_void,
        pFramebuffer: *mut VkFramebuffer,
    ) -> VkResult;
    fn vkDestroyFramebuffer(
        device: VkDevice,
        framebuffer: VkFramebuffer,
        pAllocator: *const c_void,
    );
    fn vkCreateCommandPool(
        device: VkDevice,
        pCreateInfo: *const VkCommandPoolCreateInfo,
        pAllocator: *const c_void,
        pCommandPool: *mut VkCommandPool,
    ) -> VkResult;
    fn vkDestroyCommandPool(
        device: VkDevice,
        commandPool: VkCommandPool,
        pAllocator: *const c_void,
    );
    fn vkAllocateCommandBuffers(
        device: VkDevice,
        pAllocateInfo: *const VkCommandBufferAllocateInfo,
        pCommandBuffers: *mut VkCommandBuffer,
    ) -> VkResult;
    fn vkBeginCommandBuffer(
        commandBuffer: VkCommandBuffer,
        pBeginInfo: *const VkCommandBufferBeginInfo,
    ) -> VkResult;
    fn vkEndCommandBuffer(commandBuffer: VkCommandBuffer) -> VkResult;
    fn vkCmdBeginRenderPass(
        commandBuffer: VkCommandBuffer,
        pRenderPassBegin: *const VkRenderPassBeginInfo,
        contents: u32,
    );
    fn vkCmdEndRenderPass(commandBuffer: VkCommandBuffer);
    fn vkQueueSubmit(
        queue: VkQueue,
        submitCount: u32,
        pSubmits: *const VkSubmitInfo,
        fence: VkFence,
    ) -> VkResult;
    fn vkCreateFence(
        device: VkDevice,
        pCreateInfo: *const VkFenceCreateInfo,
        pAllocator: *const c_void,
        pFence: *mut VkFence,
    ) -> VkResult;
    fn vkDestroyFence(device: VkDevice, fence: VkFence, pAllocator: *const c_void);
    fn vkWaitForFences(
        device: VkDevice,
        fenceCount: u32,
        pFences: *const VkFence,
        waitAll: u32,
        timeout: u64,
    ) -> VkResult;
    fn vkCreateBuffer(
        device: VkDevice,
        pCreateInfo: *const VkBufferCreateInfo,
        pAllocator: *const c_void,
        pBuffer: *mut VkBuffer,
    ) -> VkResult;
    fn vkDestroyBuffer(device: VkDevice, buffer: VkBuffer, pAllocator: *const c_void);
    fn vkGetBufferMemoryRequirements(
        device: VkDevice,
        buffer: VkBuffer,
        pMemoryRequirements: *mut VkMemoryRequirements,
    );
    fn vkBindBufferMemory(
        device: VkDevice,
        buffer: VkBuffer,
        memory: VkDeviceMemory,
        memoryOffset: u64,
    ) -> VkResult;
    fn vkCmdCopyImageToBuffer(
        commandBuffer: VkCommandBuffer,
        srcImage: VkImage,
        srcImageLayout: u32,
        dstBuffer: VkBuffer,
        regionCount: u32,
        pRegions: *const VkBufferImageCopy,
    );
    fn vkMapMemory(
        device: VkDevice,
        memory: VkDeviceMemory,
        offset: u64,
        size: u64,
        flags: u32,
        ppData: *mut *mut c_void,
    ) -> VkResult;
    fn vkUnmapMemory(device: VkDevice, memory: VkDeviceMemory);
    fn vkCreateShaderModule(
        device: VkDevice,
        pCreateInfo: *const VkShaderModuleCreateInfo,
        pAllocator: *const c_void,
        pShaderModule: *mut VkShaderModule,
    ) -> VkResult;
    fn vkDestroyShaderModule(
        device: VkDevice,
        shaderModule: VkShaderModule,
        pAllocator: *const c_void,
    );
    fn vkCreatePipelineLayout(
        device: VkDevice,
        pCreateInfo: *const VkPipelineLayoutCreateInfo,
        pAllocator: *const c_void,
        pPipelineLayout: *mut VkPipelineLayout,
    ) -> VkResult;
    fn vkDestroyPipelineLayout(
        device: VkDevice,
        pipelineLayout: VkPipelineLayout,
        pAllocator: *const c_void,
    );
    fn vkCreateGraphicsPipelines(
        device: VkDevice,
        pipelineCache: u64,
        createInfoCount: u32,
        pCreateInfos: *const VkGraphicsPipelineCreateInfo,
        pAllocator: *const c_void,
        pPipelines: *mut VkPipeline,
    ) -> VkResult;
    fn vkDestroyPipeline(device: VkDevice, pipeline: VkPipeline, pAllocator: *const c_void);
    fn vkCmdBindPipeline(
        commandBuffer: VkCommandBuffer,
        pipelineBindPoint: u32,
        pipeline: VkPipeline,
    );
    fn vkCmdDraw(
        commandBuffer: VkCommandBuffer,
        vertexCount: u32,
        instanceCount: u32,
        firstVertex: u32,
        firstInstance: u32,
    );
}

/// Create a Vulkan instance and return the name of each physical device the
/// loader exposes (e.g. `"llvmpipe (LLVM ...)"` under lavapipe). Errors if no
/// Vulkan loader/ICD is reachable.
pub fn devices() -> Result<Vec<String>, String> {
    unsafe {
        let app = VkApplicationInfo {
            sType: VK_STRUCTURE_TYPE_APPLICATION_INFO,
            pNext: core::ptr::null(),
            pApplicationName: c"stipple".as_ptr(),
            applicationVersion: 0,
            pEngineName: c"stipple".as_ptr(),
            engineVersion: 0,
            apiVersion: VK_API_VERSION_1_0,
        };
        let create = VkInstanceCreateInfo {
            sType: VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            pApplicationInfo: &app,
            enabledLayerCount: 0,
            ppEnabledLayerNames: core::ptr::null(),
            enabledExtensionCount: 0,
            ppEnabledExtensionNames: core::ptr::null(),
        };
        let mut instance: VkInstance = core::ptr::null_mut();
        let r = vkCreateInstance(&create, core::ptr::null(), &mut instance);
        if r != VK_SUCCESS {
            return Err(format!("vkCreateInstance failed ({r})"));
        }

        let mut count: u32 = 0;
        vkEnumeratePhysicalDevices(instance, &mut count, core::ptr::null_mut());
        let mut handles: Vec<VkPhysicalDevice> = vec![core::ptr::null_mut(); count as usize];
        if count > 0 {
            vkEnumeratePhysicalDevices(instance, &mut count, handles.as_mut_ptr());
        }

        let mut names = Vec::with_capacity(handles.len());
        for dev in handles {
            // A 2 KiB buffer comfortably exceeds sizeof(VkPhysicalDeviceProperties).
            let mut props = vec![0u8; 2048];
            vkGetPhysicalDeviceProperties(dev, props.as_mut_ptr() as *mut c_void);
            let name =
                &props[DEVICE_NAME_OFFSET..DEVICE_NAME_OFFSET + VK_MAX_PHYSICAL_DEVICE_NAME_SIZE];
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            names.push(String::from_utf8_lossy(&name[..end]).into_owned());
        }

        vkDestroyInstance(instance, core::ptr::null());
        Ok(names)
    }
}

/// A live Vulkan instance + logical device on the first physical device, with a
/// graphics queue family selected. Owns the raw handles; callers destroy them
/// via [`Gpu::destroy`] once done (we keep this manual rather than wiring `Drop`,
/// to keep the FFI surface explicit).
struct Gpu {
    instance: VkInstance,
    phys: VkPhysicalDevice,
    device: VkDevice,
    family: u32,
}

impl Gpu {
    /// Create an instance, pick the first physical device, find a graphics queue
    /// family, and create a logical device with one queue from it.
    unsafe fn create() -> Result<Gpu, String> {
        unsafe {
            let app = VkApplicationInfo {
                sType: VK_STRUCTURE_TYPE_APPLICATION_INFO,
                pNext: core::ptr::null(),
                pApplicationName: c"stipple".as_ptr(),
                applicationVersion: 0,
                pEngineName: c"stipple".as_ptr(),
                engineVersion: 0,
                apiVersion: VK_API_VERSION_1_0,
            };
            let create = VkInstanceCreateInfo {
                sType: VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                pApplicationInfo: &app,
                enabledLayerCount: 0,
                ppEnabledLayerNames: core::ptr::null(),
                enabledExtensionCount: 0,
                ppEnabledExtensionNames: core::ptr::null(),
            };
            let mut instance: VkInstance = core::ptr::null_mut();
            if vkCreateInstance(&create, core::ptr::null(), &mut instance) != VK_SUCCESS {
                return Err("vkCreateInstance failed".into());
            }

            let mut count: u32 = 0;
            vkEnumeratePhysicalDevices(instance, &mut count, core::ptr::null_mut());
            if count == 0 {
                vkDestroyInstance(instance, core::ptr::null());
                return Err("no Vulkan physical devices".into());
            }
            let mut handles: Vec<VkPhysicalDevice> = vec![core::ptr::null_mut(); count as usize];
            vkEnumeratePhysicalDevices(instance, &mut count, handles.as_mut_ptr());
            let phys = handles[0];

            // Find a queue family that supports graphics.
            let mut qcount: u32 = 0;
            vkGetPhysicalDeviceQueueFamilyProperties(phys, &mut qcount, core::ptr::null_mut());
            let mut qbuf = vec![0u8; qcount as usize * QUEUE_FAMILY_PROPS_SIZE];
            vkGetPhysicalDeviceQueueFamilyProperties(
                phys,
                &mut qcount,
                qbuf.as_mut_ptr() as *mut c_void,
            );
            let mut family: Option<u32> = None;
            for i in 0..qcount as usize {
                let off = i * QUEUE_FAMILY_PROPS_SIZE;
                let flags =
                    u32::from_ne_bytes([qbuf[off], qbuf[off + 1], qbuf[off + 2], qbuf[off + 3]]);
                if flags & VK_QUEUE_GRAPHICS_BIT != 0 {
                    family = Some(i as u32);
                    break;
                }
            }
            let Some(family) = family else {
                vkDestroyInstance(instance, core::ptr::null());
                return Err("no graphics queue family".into());
            };

            // Create a logical device with one queue from that family.
            let priority: f32 = 1.0;
            let qci = VkDeviceQueueCreateInfo {
                sType: VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                queueFamilyIndex: family,
                queueCount: 1,
                pQueuePriorities: &priority,
            };
            let dci = VkDeviceCreateInfo {
                sType: VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                queueCreateInfoCount: 1,
                pQueueCreateInfos: &qci,
                enabledLayerCount: 0,
                ppEnabledLayerNames: core::ptr::null(),
                enabledExtensionCount: 0,
                ppEnabledExtensionNames: core::ptr::null(),
                pEnabledFeatures: core::ptr::null(),
            };
            let mut device: VkDevice = core::ptr::null_mut();
            if vkCreateDevice(phys, &dci, core::ptr::null(), &mut device) != VK_SUCCESS {
                vkDestroyInstance(instance, core::ptr::null());
                return Err("vkCreateDevice failed".into());
            }
            Ok(Gpu {
                instance,
                phys,
                device,
                family,
            })
        }
    }

    /// Create a `width`×`height` `R8G8B8A8_UNORM` color-attachment image and back
    /// it with a `DEVICE_LOCAL` memory allocation. Returns `(image, memory,
    /// size)`; the caller frees both with `vkDestroyImage` + `vkFreeMemory`.
    unsafe fn create_color_image(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(VkImage, VkDeviceMemory, u64), String> {
        unsafe {
            let ici = VkImageCreateInfo {
                sType: VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                imageType: VK_IMAGE_TYPE_2D,
                format: VK_FORMAT_R8G8B8A8_UNORM,
                extent: VkExtent3D {
                    width,
                    height,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: VK_SAMPLE_COUNT_1_BIT,
                tiling: VK_IMAGE_TILING_OPTIMAL,
                usage: VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT,
                sharingMode: VK_SHARING_MODE_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: core::ptr::null(),
                initialLayout: VK_IMAGE_LAYOUT_UNDEFINED,
            };
            let mut image: VkImage = 0;
            if vkCreateImage(self.device, &ici, core::ptr::null(), &mut image) != VK_SUCCESS {
                return Err("vkCreateImage failed".into());
            }

            let mut req = VkMemoryRequirements {
                size: 0,
                alignment: 0,
                memoryTypeBits: 0,
            };
            vkGetImageMemoryRequirements(self.device, image, &mut req);

            // Read the device's memory types and pick the first DEVICE_LOCAL one
            // permitted by the image's memoryTypeBits mask.
            let mut memprops = vec![0u8; 1024];
            vkGetPhysicalDeviceMemoryProperties(self.phys, memprops.as_mut_ptr() as *mut c_void);
            let type_count =
                u32::from_ne_bytes([memprops[0], memprops[1], memprops[2], memprops[3]]);
            let mut chosen: Option<u32> = None;
            for i in 0..type_count as usize {
                if req.memoryTypeBits & (1 << i) == 0 {
                    continue;
                }
                let off = MEM_TYPES_OFFSET + i * MEM_TYPE_STRIDE;
                let flags = u32::from_ne_bytes([
                    memprops[off],
                    memprops[off + 1],
                    memprops[off + 2],
                    memprops[off + 3],
                ]);
                if flags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT != 0 {
                    chosen = Some(i as u32);
                    break;
                }
            }
            let Some(mem_type) = chosen else {
                vkDestroyImage(self.device, image, core::ptr::null());
                return Err("no DEVICE_LOCAL memory type for the image".into());
            };

            let mai = VkMemoryAllocateInfo {
                sType: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                pNext: core::ptr::null(),
                allocationSize: req.size,
                memoryTypeIndex: mem_type,
            };
            let mut memory: VkDeviceMemory = 0;
            if vkAllocateMemory(self.device, &mai, core::ptr::null(), &mut memory) != VK_SUCCESS {
                vkDestroyImage(self.device, image, core::ptr::null());
                return Err("vkAllocateMemory failed".into());
            }
            if vkBindImageMemory(self.device, image, memory, 0) != VK_SUCCESS {
                vkFreeMemory(self.device, memory, core::ptr::null());
                vkDestroyImage(self.device, image, core::ptr::null());
                return Err("vkBindImageMemory failed".into());
            }
            Ok((image, memory, req.size))
        }
    }

    /// Build a complete offscreen render target: a color image + memory, an
    /// image view, a single-attachment render pass (clear→store, ending
    /// transfer-readable), and a framebuffer binding them. Frees what it already
    /// created on any failure.
    unsafe fn create_target(&self, width: u32, height: u32) -> Result<Target, String> {
        unsafe {
            let (image, memory, _bytes) = self.create_color_image(width, height)?;

            let view_ci = VkImageViewCreateInfo {
                sType: VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                image,
                viewType: VK_IMAGE_VIEW_TYPE_2D,
                format: VK_FORMAT_R8G8B8A8_UNORM,
                components: VkComponentMapping {
                    r: VK_COMPONENT_SWIZZLE_IDENTITY,
                    g: VK_COMPONENT_SWIZZLE_IDENTITY,
                    b: VK_COMPONENT_SWIZZLE_IDENTITY,
                    a: VK_COMPONENT_SWIZZLE_IDENTITY,
                },
                subresourceRange: VkImageSubresourceRange {
                    aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                    baseMipLevel: 0,
                    levelCount: 1,
                    baseArrayLayer: 0,
                    layerCount: 1,
                },
            };
            let mut view: VkImageView = 0;
            if vkCreateImageView(self.device, &view_ci, core::ptr::null(), &mut view) != VK_SUCCESS
            {
                vkFreeMemory(self.device, memory, core::ptr::null());
                vkDestroyImage(self.device, image, core::ptr::null());
                return Err("vkCreateImageView failed".into());
            }

            let attachment = VkAttachmentDescription {
                flags: 0,
                format: VK_FORMAT_R8G8B8A8_UNORM,
                samples: VK_SAMPLE_COUNT_1_BIT,
                loadOp: VK_ATTACHMENT_LOAD_OP_CLEAR,
                storeOp: VK_ATTACHMENT_STORE_OP_STORE,
                stencilLoadOp: VK_ATTACHMENT_LOAD_OP_DONT_CARE,
                stencilStoreOp: VK_ATTACHMENT_STORE_OP_DONT_CARE,
                initialLayout: VK_IMAGE_LAYOUT_UNDEFINED,
                finalLayout: VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
            };
            let color_ref = VkAttachmentReference {
                attachment: 0,
                layout: VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
            };
            let subpass = VkSubpassDescription {
                flags: 0,
                pipelineBindPoint: VK_PIPELINE_BIND_POINT_GRAPHICS,
                inputAttachmentCount: 0,
                pInputAttachments: core::ptr::null(),
                colorAttachmentCount: 1,
                pColorAttachments: &color_ref,
                pResolveAttachments: core::ptr::null(),
                pDepthStencilAttachment: core::ptr::null(),
                preserveAttachmentCount: 0,
                pPreserveAttachments: core::ptr::null(),
            };
            let rp_ci = VkRenderPassCreateInfo {
                sType: VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                attachmentCount: 1,
                pAttachments: &attachment,
                subpassCount: 1,
                pSubpasses: &subpass,
                dependencyCount: 0,
                pDependencies: core::ptr::null(),
            };
            let mut render_pass: VkRenderPass = 0;
            if vkCreateRenderPass(self.device, &rp_ci, core::ptr::null(), &mut render_pass)
                != VK_SUCCESS
            {
                vkDestroyImageView(self.device, view, core::ptr::null());
                vkFreeMemory(self.device, memory, core::ptr::null());
                vkDestroyImage(self.device, image, core::ptr::null());
                return Err("vkCreateRenderPass failed".into());
            }

            let fb_ci = VkFramebufferCreateInfo {
                sType: VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                renderPass: render_pass,
                attachmentCount: 1,
                pAttachments: &view,
                width,
                height,
                layers: 1,
            };
            let mut framebuffer: VkFramebuffer = 0;
            if vkCreateFramebuffer(self.device, &fb_ci, core::ptr::null(), &mut framebuffer)
                != VK_SUCCESS
            {
                vkDestroyRenderPass(self.device, render_pass, core::ptr::null());
                vkDestroyImageView(self.device, view, core::ptr::null());
                vkFreeMemory(self.device, memory, core::ptr::null());
                vkDestroyImage(self.device, image, core::ptr::null());
                return Err("vkCreateFramebuffer failed".into());
            }

            Ok(Target {
                image,
                memory,
                view,
                render_pass,
                framebuffer,
                width,
                height,
            })
        }
    }

    /// Create a `size`-byte buffer usable as a transfer destination, backed by
    /// `HOST_VISIBLE | HOST_COHERENT` memory so the CPU can map and read it.
    /// Returns `(buffer, memory)`; the caller frees both.
    unsafe fn create_host_buffer(&self, size: u64) -> Result<(VkBuffer, VkDeviceMemory), String> {
        unsafe {
            let bci = VkBufferCreateInfo {
                sType: VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                size,
                usage: VK_BUFFER_USAGE_TRANSFER_DST_BIT,
                sharingMode: VK_SHARING_MODE_EXCLUSIVE,
                queueFamilyIndexCount: 0,
                pQueueFamilyIndices: core::ptr::null(),
            };
            let mut buffer: VkBuffer = 0;
            if vkCreateBuffer(self.device, &bci, core::ptr::null(), &mut buffer) != VK_SUCCESS {
                return Err("vkCreateBuffer failed".into());
            }

            let mut req = VkMemoryRequirements {
                size: 0,
                alignment: 0,
                memoryTypeBits: 0,
            };
            vkGetBufferMemoryRequirements(self.device, buffer, &mut req);

            let want = VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
            let mut memprops = vec![0u8; 1024];
            vkGetPhysicalDeviceMemoryProperties(self.phys, memprops.as_mut_ptr() as *mut c_void);
            let type_count =
                u32::from_ne_bytes([memprops[0], memprops[1], memprops[2], memprops[3]]);
            let mut chosen: Option<u32> = None;
            for i in 0..type_count as usize {
                if req.memoryTypeBits & (1 << i) == 0 {
                    continue;
                }
                let off = MEM_TYPES_OFFSET + i * MEM_TYPE_STRIDE;
                let flags = u32::from_ne_bytes([
                    memprops[off],
                    memprops[off + 1],
                    memprops[off + 2],
                    memprops[off + 3],
                ]);
                if flags & want == want {
                    chosen = Some(i as u32);
                    break;
                }
            }
            let Some(mem_type) = chosen else {
                vkDestroyBuffer(self.device, buffer, core::ptr::null());
                return Err("no HOST_VISIBLE|COHERENT memory type for the buffer".into());
            };

            let mai = VkMemoryAllocateInfo {
                sType: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                pNext: core::ptr::null(),
                allocationSize: req.size,
                memoryTypeIndex: mem_type,
            };
            let mut memory: VkDeviceMemory = 0;
            if vkAllocateMemory(self.device, &mai, core::ptr::null(), &mut memory) != VK_SUCCESS {
                vkDestroyBuffer(self.device, buffer, core::ptr::null());
                return Err("vkAllocateMemory (host buffer) failed".into());
            }
            if vkBindBufferMemory(self.device, buffer, memory, 0) != VK_SUCCESS {
                vkFreeMemory(self.device, memory, core::ptr::null());
                vkDestroyBuffer(self.device, buffer, core::ptr::null());
                return Err("vkBindBufferMemory failed".into());
            }
            Ok((buffer, memory))
        }
    }

    /// Create a shader module from a SPIR-V byte blob. SPIR-V is a stream of
    /// 32-bit words; `include_bytes!` yields byte alignment 1, so we realign into
    /// a `Vec<u32>` before handing Vulkan `pCode`.
    unsafe fn shader_module(&self, spv: &[u8]) -> Result<VkShaderModule, String> {
        unsafe {
            if !spv.len().is_multiple_of(4) {
                return Err("SPIR-V length is not a multiple of 4".into());
            }
            let words: Vec<u32> = spv
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            let ci = VkShaderModuleCreateInfo {
                sType: VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                codeSize: spv.len(),
                pCode: words.as_ptr(),
            };
            let mut module: VkShaderModule = 0;
            if vkCreateShaderModule(self.device, &ci, core::ptr::null(), &mut module) != VK_SUCCESS
            {
                return Err("vkCreateShaderModule failed".into());
            }
            Ok(module)
        }
    }

    /// Run one render pass on `target` — clearing to `clear`, then executing
    /// `record` (which adds draw commands inside the pass) — copy the result to a
    /// host buffer, and return the read-back RGBA pixels. The shared
    /// record→submit→copy→map path behind both the plain clear and the triangle
    /// draw. Cleans up everything it allocates (buffer, pool, fence); the caller
    /// still owns `target` and `self`.
    unsafe fn draw_to_pixels(
        &self,
        target: &Target,
        clear: [f32; 4],
        record: impl FnOnce(VkCommandBuffer),
    ) -> Result<Vec<u8>, String> {
        unsafe {
            let size = target.width as u64 * target.height as u64 * 4;
            let (buffer, buf_mem) = self.create_host_buffer(size)?;

            // Tears down everything this function allocates.
            let cleanup = |dev: VkDevice,
                           buffer: VkBuffer,
                           buf_mem: VkDeviceMemory,
                           pool: VkCommandPool,
                           fence: VkFence| {
                if fence != 0 {
                    vkDestroyFence(dev, fence, core::ptr::null());
                }
                if pool != 0 {
                    vkDestroyCommandPool(dev, pool, core::ptr::null());
                }
                vkFreeMemory(dev, buf_mem, core::ptr::null());
                vkDestroyBuffer(dev, buffer, core::ptr::null());
            };

            let pool_ci = VkCommandPoolCreateInfo {
                sType: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                queueFamilyIndex: self.family,
            };
            let mut pool: VkCommandPool = 0;
            if vkCreateCommandPool(self.device, &pool_ci, core::ptr::null(), &mut pool)
                != VK_SUCCESS
            {
                cleanup(self.device, buffer, buf_mem, 0, 0);
                return Err("vkCreateCommandPool failed".into());
            }
            let alloc = VkCommandBufferAllocateInfo {
                sType: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                pNext: core::ptr::null(),
                commandPool: pool,
                level: VK_COMMAND_BUFFER_LEVEL_PRIMARY,
                commandBufferCount: 1,
            };
            let mut cmd: VkCommandBuffer = core::ptr::null_mut();
            if vkAllocateCommandBuffers(self.device, &alloc, &mut cmd) != VK_SUCCESS {
                cleanup(self.device, buffer, buf_mem, pool, 0);
                return Err("vkAllocateCommandBuffers failed".into());
            }

            let begin = VkCommandBufferBeginInfo {
                sType: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                pNext: core::ptr::null(),
                flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
                pInheritanceInfo: core::ptr::null(),
            };
            if vkBeginCommandBuffer(cmd, &begin) != VK_SUCCESS {
                cleanup(self.device, buffer, buf_mem, pool, 0);
                return Err("vkBeginCommandBuffer failed".into());
            }
            let clear_value = VkClearValue { float32: clear };
            let rp_begin = VkRenderPassBeginInfo {
                sType: VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
                pNext: core::ptr::null(),
                renderPass: target.render_pass,
                framebuffer: target.framebuffer,
                renderArea: VkRect2D {
                    offset: VkOffset2D { x: 0, y: 0 },
                    extent: VkExtent2D {
                        width: target.width,
                        height: target.height,
                    },
                },
                clearValueCount: 1,
                pClearValues: &clear_value,
            };
            vkCmdBeginRenderPass(cmd, &rp_begin, VK_SUBPASS_CONTENTS_INLINE);
            record(cmd);
            vkCmdEndRenderPass(cmd);

            // The render pass left the image in TRANSFER_SRC_OPTIMAL — copy it,
            // tightly packed, into the host buffer.
            let region = VkBufferImageCopy {
                bufferOffset: 0,
                bufferRowLength: 0,
                bufferImageHeight: 0,
                imageSubresource: VkImageSubresourceLayers {
                    aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                    mipLevel: 0,
                    baseArrayLayer: 0,
                    layerCount: 1,
                },
                imageOffset: VkOffset3D { x: 0, y: 0, z: 0 },
                imageExtent: VkExtent3D {
                    width: target.width,
                    height: target.height,
                    depth: 1,
                },
            };
            vkCmdCopyImageToBuffer(
                cmd,
                target.image,
                VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                buffer,
                1,
                &region,
            );
            if vkEndCommandBuffer(cmd) != VK_SUCCESS {
                cleanup(self.device, buffer, buf_mem, pool, 0);
                return Err("vkEndCommandBuffer failed".into());
            }

            let mut queue: VkQueue = core::ptr::null_mut();
            vkGetDeviceQueue(self.device, self.family, 0, &mut queue);
            let fence_ci = VkFenceCreateInfo {
                sType: VK_STRUCTURE_TYPE_FENCE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
            };
            let mut fence: VkFence = 0;
            if vkCreateFence(self.device, &fence_ci, core::ptr::null(), &mut fence) != VK_SUCCESS {
                cleanup(self.device, buffer, buf_mem, pool, 0);
                return Err("vkCreateFence failed".into());
            }
            let submit = VkSubmitInfo {
                sType: VK_STRUCTURE_TYPE_SUBMIT_INFO,
                pNext: core::ptr::null(),
                waitSemaphoreCount: 0,
                pWaitSemaphores: core::ptr::null(),
                pWaitDstStageMask: core::ptr::null(),
                commandBufferCount: 1,
                pCommandBuffers: &cmd,
                signalSemaphoreCount: 0,
                pSignalSemaphores: core::ptr::null(),
            };
            if vkQueueSubmit(queue, 1, &submit, fence) != VK_SUCCESS {
                cleanup(self.device, buffer, buf_mem, pool, fence);
                return Err("vkQueueSubmit failed".into());
            }
            if vkWaitForFences(self.device, 1, &fence, VK_TRUE, u64::MAX) != VK_SUCCESS {
                cleanup(self.device, buffer, buf_mem, pool, fence);
                return Err("vkWaitForFences failed".into());
            }

            let mut ptr: *mut c_void = core::ptr::null_mut();
            if vkMapMemory(self.device, buf_mem, 0, size, 0, &mut ptr) != VK_SUCCESS
                || ptr.is_null()
            {
                cleanup(self.device, buffer, buf_mem, pool, fence);
                return Err("vkMapMemory failed".into());
            }
            let mut pixels = vec![0u8; size as usize];
            core::ptr::copy_nonoverlapping(ptr as *const u8, pixels.as_mut_ptr(), size as usize);
            vkUnmapMemory(self.device, buf_mem);

            cleanup(self.device, buffer, buf_mem, pool, fence);
            Ok(pixels)
        }
    }

    unsafe fn destroy(self) {
        unsafe {
            vkDestroyDevice(self.device, core::ptr::null());
            vkDestroyInstance(self.instance, core::ptr::null());
        }
    }
}

/// A complete offscreen render target owned alongside a [`Gpu`]: the color image
/// and its backing memory, a view, a render pass, and a framebuffer. Destroyed
/// explicitly via [`Target::destroy`] (passing the owning device).
struct Target {
    image: VkImage,
    memory: VkDeviceMemory,
    view: VkImageView,
    render_pass: VkRenderPass,
    framebuffer: VkFramebuffer,
    width: u32,
    height: u32,
}

impl Target {
    unsafe fn destroy(self, device: VkDevice) {
        unsafe {
            vkDestroyFramebuffer(device, self.framebuffer, core::ptr::null());
            vkDestroyRenderPass(device, self.render_pass, core::ptr::null());
            vkDestroyImageView(device, self.view, core::ptr::null());
            vkFreeMemory(device, self.memory, core::ptr::null());
            vkDestroyImage(device, self.image, core::ptr::null());
        }
    }
}

/// Create an instance, pick the first physical device, select a graphics queue
/// family, and create a logical device with that queue — the core a Vulkan
/// render backend builds on. Returns a one-line summary. Errors if any step
/// fails or no graphics-capable device exists.
pub fn init_device() -> Result<String, String> {
    unsafe {
        let gpu = Gpu::create()?;
        let mut queue: VkQueue = core::ptr::null_mut();
        vkGetDeviceQueue(gpu.device, gpu.family, 0, &mut queue);
        let ok = !queue.is_null();
        let family = gpu.family;
        gpu.destroy();
        if ok {
            Ok(format!(
                "logical device + graphics queue (family {family}) created"
            ))
        } else {
            Err("vkGetDeviceQueue returned null".into())
        }
    }
}

/// Allocate a GPU-resident render target: create a `width`×`height`
/// `R8G8B8A8_UNORM` color-attachment [`VkImage`], query its memory
/// requirements, choose a `DEVICE_LOCAL` memory type, and back it with a
/// [`VkDeviceMemory`] allocation — the offscreen target a Vulkan render pass
/// draws into. Returns a one-line summary (size + chosen memory type). Errors if
/// any step fails.
pub fn init_image(width: u32, height: u32) -> Result<String, String> {
    unsafe {
        let gpu = Gpu::create()?;
        let r = gpu.create_color_image(width, height);
        match r {
            Ok((image, memory, bytes)) => {
                vkFreeMemory(gpu.device, memory, core::ptr::null());
                vkDestroyImage(gpu.device, image, core::ptr::null());
                gpu.destroy();
                Ok(format!(
                    "{width}x{height} color image bound to {bytes}B of DEVICE_LOCAL memory"
                ))
            }
            Err(e) => {
                gpu.destroy();
                Err(e)
            }
        }
    }
}

/// Build the render-target objects a Vulkan render pass needs: a [`VkImageView`]
/// over a freshly-allocated color image, a single-attachment [`VkRenderPass`]
/// (clear on load, store on done, ending in `TRANSFER_SRC_OPTIMAL` ready for
/// readback), and a [`VkFramebuffer`] binding the view to the pass. Returns a
/// one-line summary. Errors if any step fails.
pub fn init_framebuffer(width: u32, height: u32) -> Result<String, String> {
    unsafe {
        let gpu = Gpu::create()?;
        match gpu.create_target(width, height) {
            Ok(target) => {
                target.destroy(gpu.device);
                gpu.destroy();
                Ok(format!(
                    "{width}x{height} framebuffer + render pass + image view created"
                ))
            }
            Err(e) => {
                gpu.destroy();
                Err(e)
            }
        }
    }
}

/// Execute real GPU work: record a primary command buffer that runs the
/// single-attachment render pass — whose load-op **clears** the color image to a
/// fixed color — submit it to the graphics queue, and block on a fence until the
/// GPU signals completion. This is the first command-buffer round-trip the
/// eventual draw pipeline (pipeline + draw calls) slots into. Returns a one-line
/// summary. Errors if any step fails.
pub fn init_clear(width: u32, height: u32) -> Result<String, String> {
    unsafe {
        let gpu = Gpu::create()?;
        let target = match gpu.create_target(width, height) {
            Ok(t) => t,
            Err(e) => {
                gpu.destroy();
                return Err(e);
            }
        };

        // Tear down everything created so far, on failure or success.
        let finish = |gpu: Gpu, target: Target, pool: VkCommandPool, fence: VkFence| {
            if fence != 0 {
                vkDestroyFence(gpu.device, fence, core::ptr::null());
            }
            if pool != 0 {
                // Frees the command buffers allocated from it too.
                vkDestroyCommandPool(gpu.device, pool, core::ptr::null());
            }
            target.destroy(gpu.device);
            gpu.destroy();
        };

        // A command pool on the graphics family, and one primary command buffer.
        let pool_ci = VkCommandPoolCreateInfo {
            sType: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            queueFamilyIndex: gpu.family,
        };
        let mut pool: VkCommandPool = 0;
        if vkCreateCommandPool(gpu.device, &pool_ci, core::ptr::null(), &mut pool) != VK_SUCCESS {
            finish(gpu, target, 0, 0);
            return Err("vkCreateCommandPool failed".into());
        }
        let alloc = VkCommandBufferAllocateInfo {
            sType: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
            pNext: core::ptr::null(),
            commandPool: pool,
            level: VK_COMMAND_BUFFER_LEVEL_PRIMARY,
            commandBufferCount: 1,
        };
        let mut cmd: VkCommandBuffer = core::ptr::null_mut();
        if vkAllocateCommandBuffers(gpu.device, &alloc, &mut cmd) != VK_SUCCESS {
            finish(gpu, target, pool, 0);
            return Err("vkAllocateCommandBuffers failed".into());
        }

        // Record: begin → run the clearing render pass → end.
        let begin = VkCommandBufferBeginInfo {
            sType: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
            pNext: core::ptr::null(),
            flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
            pInheritanceInfo: core::ptr::null(),
        };
        if vkBeginCommandBuffer(cmd, &begin) != VK_SUCCESS {
            finish(gpu, target, pool, 0);
            return Err("vkBeginCommandBuffer failed".into());
        }
        // A recognizable stipple blue (0x60, 0x9c, 0xff) so a later readback can
        // confirm the clear actually ran.
        let clear = VkClearValue {
            float32: [0x60 as f32 / 255.0, 0x9c as f32 / 255.0, 1.0, 1.0],
        };
        let rp_begin = VkRenderPassBeginInfo {
            sType: VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
            pNext: core::ptr::null(),
            renderPass: target.render_pass,
            framebuffer: target.framebuffer,
            renderArea: VkRect2D {
                offset: VkOffset2D { x: 0, y: 0 },
                extent: VkExtent2D {
                    width: target.width,
                    height: target.height,
                },
            },
            clearValueCount: 1,
            pClearValues: &clear,
        };
        vkCmdBeginRenderPass(cmd, &rp_begin, VK_SUBPASS_CONTENTS_INLINE);
        vkCmdEndRenderPass(cmd);
        if vkEndCommandBuffer(cmd) != VK_SUCCESS {
            finish(gpu, target, pool, 0);
            return Err("vkEndCommandBuffer failed".into());
        }

        // Submit to the graphics queue, fenced, and wait for the GPU.
        let mut queue: VkQueue = core::ptr::null_mut();
        vkGetDeviceQueue(gpu.device, gpu.family, 0, &mut queue);

        let fence_ci = VkFenceCreateInfo {
            sType: VK_STRUCTURE_TYPE_FENCE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
        };
        let mut fence: VkFence = 0;
        if vkCreateFence(gpu.device, &fence_ci, core::ptr::null(), &mut fence) != VK_SUCCESS {
            finish(gpu, target, pool, 0);
            return Err("vkCreateFence failed".into());
        }
        let submit = VkSubmitInfo {
            sType: VK_STRUCTURE_TYPE_SUBMIT_INFO,
            pNext: core::ptr::null(),
            waitSemaphoreCount: 0,
            pWaitSemaphores: core::ptr::null(),
            pWaitDstStageMask: core::ptr::null(),
            commandBufferCount: 1,
            pCommandBuffers: &cmd,
            signalSemaphoreCount: 0,
            pSignalSemaphores: core::ptr::null(),
        };
        if vkQueueSubmit(queue, 1, &submit, fence) != VK_SUCCESS {
            finish(gpu, target, pool, fence);
            return Err("vkQueueSubmit failed".into());
        }
        let waited = vkWaitForFences(gpu.device, 1, &fence, VK_TRUE, u64::MAX) == VK_SUCCESS;

        finish(gpu, target, pool, fence);
        if waited {
            Ok(format!(
                "{width}x{height} render pass submitted and cleared (fence signaled)"
            ))
        } else {
            Err("vkWaitForFences failed".into())
        }
    }
}

/// The Vulkan offscreen capstone: run the clearing render pass on the GPU, then
/// `vkCmdCopyImageToBuffer` the result into a host-visible buffer, fence-wait,
/// map it, and return the `width`×`height` RGBA pixels — an actual GPU-rendered
/// frame read back to the CPU (so CI can turn it into a screenshot). The eventual
/// draw pipeline replaces only the "clear" with real draw calls; this readback
/// path stays. Errors if any step fails.
pub fn render_clear(width: u32, height: u32) -> Result<Vec<u8>, String> {
    unsafe {
        let gpu = Gpu::create()?;
        let target = match gpu.create_target(width, height) {
            Ok(t) => t,
            Err(e) => {
                gpu.destroy();
                return Err(e);
            }
        };
        // stipple blue (0x60, 0x9c, 0xff) — the readback pixels should be this.
        let clear = [0x60 as f32 / 255.0, 0x9c as f32 / 255.0, 1.0, 1.0];
        let result = gpu.draw_to_pixels(&target, clear, |_cmd| {});
        target.destroy(gpu.device);
        gpu.destroy();
        result
    }
}

// Precompiled SPIR-V (committed; built once from shaders/*.{vert,frag} with
// glslangValidator) so the crate has no build-time shader-compiler dependency.
const TRIANGLE_VERT_SPV: &[u8] = include_bytes!("shaders/triangle.vert.spv");
const TRIANGLE_FRAG_SPV: &[u8] = include_bytes!("shaders/triangle.frag.spv");

/// The full Vulkan render pipeline: compile two SPIR-V shader modules, build a
/// graphics pipeline (no vertex buffers — the vertex shader emits a triangle
/// from `gl_VertexIndex`), then **draw** it over a dark-cleared `width`×`height`
/// target and read the frame back to the CPU. Unlike [`render_clear`] this runs
/// real shaders through `vkCmdDraw`, so the center pixel comes out stipple green.
/// Returns the RGBA pixels. Errors if any step fails.
pub fn render_triangle(width: u32, height: u32) -> Result<Vec<u8>, String> {
    unsafe {
        let gpu = Gpu::create()?;
        let target = match gpu.create_target(width, height) {
            Ok(t) => t,
            Err(e) => {
                gpu.destroy();
                return Err(e);
            }
        };

        // Shader modules from the committed SPIR-V.
        let vert = match gpu.shader_module(TRIANGLE_VERT_SPV) {
            Ok(m) => m,
            Err(e) => {
                target.destroy(gpu.device);
                gpu.destroy();
                return Err(e);
            }
        };
        let frag = match gpu.shader_module(TRIANGLE_FRAG_SPV) {
            Ok(m) => m,
            Err(e) => {
                vkDestroyShaderModule(gpu.device, vert, core::ptr::null());
                target.destroy(gpu.device);
                gpu.destroy();
                return Err(e);
            }
        };

        // Cleans up the pipeline-related objects (and target + gpu) on exit.
        let teardown =
            |gpu: Gpu, target: Target, pipeline: VkPipeline, layout: VkPipelineLayout| {
                if pipeline != 0 {
                    vkDestroyPipeline(gpu.device, pipeline, core::ptr::null());
                }
                if layout != 0 {
                    vkDestroyPipelineLayout(gpu.device, layout, core::ptr::null());
                }
                vkDestroyShaderModule(gpu.device, frag, core::ptr::null());
                vkDestroyShaderModule(gpu.device, vert, core::ptr::null());
                target.destroy(gpu.device);
                gpu.destroy();
            };

        // An empty pipeline layout (no descriptors, no push constants).
        let pl_ci = VkPipelineLayoutCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            setLayoutCount: 0,
            pSetLayouts: core::ptr::null(),
            pushConstantRangeCount: 0,
            pPushConstantRanges: core::ptr::null(),
        };
        let mut layout: VkPipelineLayout = 0;
        if vkCreatePipelineLayout(gpu.device, &pl_ci, core::ptr::null(), &mut layout) != VK_SUCCESS
        {
            teardown(gpu, target, 0, 0);
            return Err("vkCreatePipelineLayout failed".into());
        }

        let stages = [
            VkPipelineShaderStageCreateInfo {
                sType: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                stage: VK_SHADER_STAGE_VERTEX_BIT,
                module: vert,
                pName: c"main".as_ptr(),
                pSpecializationInfo: core::ptr::null(),
            },
            VkPipelineShaderStageCreateInfo {
                sType: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                pNext: core::ptr::null(),
                flags: 0,
                stage: VK_SHADER_STAGE_FRAGMENT_BIT,
                module: frag,
                pName: c"main".as_ptr(),
                pSpecializationInfo: core::ptr::null(),
            },
        ];
        // No vertex buffers — the vertex shader generates positions.
        let vertex_input = VkPipelineVertexInputStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            vertexBindingDescriptionCount: 0,
            pVertexBindingDescriptions: core::ptr::null(),
            vertexAttributeDescriptionCount: 0,
            pVertexAttributeDescriptions: core::ptr::null(),
        };
        let input_assembly = VkPipelineInputAssemblyStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            topology: VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
            primitiveRestartEnable: 0,
        };
        let viewport = VkViewport {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
            minDepth: 0.0,
            maxDepth: 1.0,
        };
        let scissor = VkRect2D {
            offset: VkOffset2D { x: 0, y: 0 },
            extent: VkExtent2D { width, height },
        };
        let viewport_state = VkPipelineViewportStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            viewportCount: 1,
            pViewports: &viewport,
            scissorCount: 1,
            pScissors: &scissor,
        };
        let raster = VkPipelineRasterizationStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            depthClampEnable: 0,
            rasterizerDiscardEnable: 0,
            polygonMode: VK_POLYGON_MODE_FILL,
            cullMode: VK_CULL_MODE_NONE,
            frontFace: VK_FRONT_FACE_COUNTER_CLOCKWISE,
            depthBiasEnable: 0,
            depthBiasConstantFactor: 0.0,
            depthBiasClamp: 0.0,
            depthBiasSlopeFactor: 0.0,
            lineWidth: 1.0,
        };
        let multisample = VkPipelineMultisampleStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            rasterizationSamples: VK_SAMPLE_COUNT_1_BIT,
            sampleShadingEnable: 0,
            minSampleShading: 0.0,
            pSampleMask: core::ptr::null(),
            alphaToCoverageEnable: 0,
            alphaToOneEnable: 0,
        };
        let blend_attachment = VkPipelineColorBlendAttachmentState {
            blendEnable: 0,
            srcColorBlendFactor: 0,
            dstColorBlendFactor: 0,
            colorBlendOp: 0,
            srcAlphaBlendFactor: 0,
            dstAlphaBlendFactor: 0,
            alphaBlendOp: 0,
            colorWriteMask: VK_COLOR_COMPONENT_RGBA,
        };
        let color_blend = VkPipelineColorBlendStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            logicOpEnable: 0,
            logicOp: 0,
            attachmentCount: 1,
            pAttachments: &blend_attachment,
            blendConstants: [0.0; 4],
        };
        let gp_ci = VkGraphicsPipelineCreateInfo {
            sType: VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
            pNext: core::ptr::null(),
            flags: 0,
            stageCount: 2,
            pStages: stages.as_ptr(),
            pVertexInputState: &vertex_input,
            pInputAssemblyState: &input_assembly,
            pTessellationState: core::ptr::null(),
            pViewportState: &viewport_state,
            pRasterizationState: &raster,
            pMultisampleState: &multisample,
            pDepthStencilState: core::ptr::null(),
            pColorBlendState: &color_blend,
            pDynamicState: core::ptr::null(),
            layout,
            renderPass: target.render_pass,
            subpass: 0,
            basePipelineHandle: 0,
            basePipelineIndex: -1,
        };
        let mut pipeline: VkPipeline = 0;
        if vkCreateGraphicsPipelines(gpu.device, 0, 1, &gp_ci, core::ptr::null(), &mut pipeline)
            != VK_SUCCESS
        {
            teardown(gpu, target, 0, layout);
            return Err("vkCreateGraphicsPipelines failed".into());
        }

        // Dark background; the triangle is drawn stipple green over it.
        let clear = [
            0x14 as f32 / 255.0,
            0x15 as f32 / 255.0,
            0x18 as f32 / 255.0,
            1.0,
        ];
        let result = gpu.draw_to_pixels(&target, clear, |cmd| {
            vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
            vkCmdDraw(cmd, 3, 1, 0, 0);
        });
        teardown(gpu, target, pipeline, layout);
        result
    }
}
