//! Android hardware buffers imported as Vulkan images
//! (`VK_ANDROID_external_memory_android_hardware_buffer`): the camera's YUV
//! frames sampled with no copy, as GLES samples them through an external
//! texture.
//!
//! The buffer's properties (`vkGetAndroidHardwareBufferPropertiesANDROID`)
//! give its memory types and allocation size, and its format: a Vulkan one,
//! or (a camera's `Y8Cb8Cr8_420`, as a rule) an external format only the
//! driver knows. An image of that format is made for the buffer's memory,
//! imported into a dedicated allocation, and sampled through a sampler
//! YCbCr conversion with the model, range and chroma siting the driver
//! suggests: the shader reads RGB from it like any texture (`quad.frag`).
//! The conversion's sampler is immutable in a descriptor set layout of its
//! own, so each conversion has its set layout, pipeline layout and quads
//! pipeline; buffers alike share them. Each buffer's import (image, memory,
//! view and descriptor set) is cached by the buffer, which the import holds
//! a reference to: a camera's reader cycles through a few buffers, each
//! imported once. Every frame that draws one takes it over from the
//! producer (`VK_QUEUE_FAMILY_FOREIGN_EXT`) before sampling it.
//!
//! A device without the extension or the conversion feature, or a buffer
//! it won't import, is drawn from its pixels read on the CPU, as before.

use super::external::ExtView;
use super::*;
use crate::gpu::vk::ahb::*;
use crate::render::{ExternalFormat, ExternalImage};

/// What one conversion samples with.
#[derive(Clone, Copy)]
struct Conversion {
    conversion: u64,
    sampler: VkSampler,
    set_layout: VkDescriptorSetLayout,
    layout: VkPipelineLayout,
    pipeline: VkPipeline,
}

/// One buffer's import.
struct Import {
    image: VkImage,
    memory: VkDeviceMemory,
    view: VkImageView,
    /// The pool of its one descriptor set, and the set, through its
    /// conversion (null for a buffer of a plain Vulkan format, which is
    /// sampled as any image).
    pool: VkDescriptorPool,
    set: VkDescriptorSet,
    conversion: Option<Conversion>,
}

/// A buffer drawn: its import (`None` if it failed, not tried again while
/// the buffer is drawn), and the frame that last drew it.
struct Entry {
    import: Option<Import>,
    used: u64,
}

/// The executor's hardware buffer imports.
pub(super) struct Ahb {
    get_properties: GetHardwareBufferProperties,
    create_conversion: CreateConversion,
    destroy_conversion: DestroyConversion,
    conversions: HashMap<ConversionKey, Conversion>,
    /// By buffer.
    imports: HashMap<usize, Entry>,
    /// A failed import was reported.
    warned: bool,
}

/// Whether to import hardware buffers on `phys`: on Android, where the
/// device has the extension and what it builds on (the instance is a 1.0
/// one: listed, not taken from a later core) and can convert YCbCr. The
/// extensions to enable go into `exts`.
pub(super) unsafe fn wanted(
    vk: &Fns,
    phys: VkPhysicalDevice,
    dev_has: &dyn Fn(&CStr) -> bool,
    exts: &mut Vec<*const c_char>,
) -> bool {
    if !cfg!(target_os = "android") || (on_emulator() && !forced()) {
        return false;
    }
    if !DEVICE_EXTS.iter().all(|e| dev_has(e)) {
        return false;
    }
    let Some(features2) = vk.vkGetPhysicalDeviceFeatures2KHR else {
        return false;
    };
    unsafe {
        let mut ycbcr = SamplerYcbcrConversionFeatures {
            sType: ST_PHYSICAL_DEVICE_SAMPLER_YCBCR_CONVERSION_FEATURES,
            pNext: std::ptr::null_mut(),
            samplerYcbcrConversion: 0,
        };
        let mut f = PhysicalDeviceFeatures2 {
            sType: ST_PHYSICAL_DEVICE_FEATURES_2,
            pNext: (&mut ycbcr as *mut SamplerYcbcrConversionFeatures).cast(),
            features: [0; 55],
        };
        let features2: GetFeatures2 = std::mem::transmute(features2);
        features2(phys, &mut f);
        if ycbcr.samplerYcbcrConversion == 0 {
            return false;
        }
    }
    for e in DEVICE_EXTS {
        if !exts.iter().any(|&d| unsafe { CStr::from_ptr(d) } == *e) {
            exts.push(e.as_ptr());
        }
    }
    true
}

/// Whether this is the Android emulator, whose Vulkan (gfxstream, over
/// SwiftShader or the host's GPU) samples a camera's `Y8Cb8Cr8_420` buffer
/// as two-plane NV12 where its gralloc laid it out as three planes (I420:
/// a chroma pixel stride of 1): the picture comes out with its chroma
/// scrambled. Its camera frames are drawn from their pixels there, as GLES
/// draws them right.
fn on_emulator() -> bool {
    property(c"ro.boot.qemu") == "1" || property(c"ro.kernel.qemu") == "1"
}

/// Whether `debug.stipple.vk_ahb` is `1` (`adb shell setprop`): import
/// anyway, to exercise the path on the emulator.
fn forced() -> bool {
    property(c"debug.stipple.vk_ahb") == "1"
}

/// A system property's value (empty if unset).
#[cfg(target_os = "android")]
fn property(name: &CStr) -> String {
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> i32;
    }
    // PROP_VALUE_MAX, the value and its terminating zero.
    let mut value = [0 as c_char; 92];
    unsafe {
        __system_property_get(name.as_ptr(), value.as_mut_ptr());
        CStr::from_ptr(value.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(not(target_os = "android"))]
fn property(_: &CStr) -> String {
    String::new()
}

/// The feature the device is created with when it imports buffers.
pub(super) fn feature() -> SamplerYcbcrConversionFeatures {
    SamplerYcbcrConversionFeatures {
        sType: ST_PHYSICAL_DEVICE_SAMPLER_YCBCR_CONVERSION_FEATURES,
        pNext: std::ptr::null_mut(),
        samplerYcbcrConversion: 1,
    }
}

impl Ahb {
    /// The device's entry points for it, if it has them all.
    pub(super) unsafe fn load(vk: &Fns, device: VkDevice) -> Option<Ahb> {
        unsafe {
            let gdpa = vk.vkGetDeviceProcAddr?;
            let f = |name: &CStr| {
                let p = gdpa(device, name.as_ptr());
                (!p.is_null()).then_some(p)
            };
            let get = f(c"vkGetAndroidHardwareBufferPropertiesANDROID")?;
            let create = f(c"vkCreateSamplerYcbcrConversionKHR")
                .or_else(|| f(c"vkCreateSamplerYcbcrConversion"))?;
            let destroy = f(c"vkDestroySamplerYcbcrConversionKHR")
                .or_else(|| f(c"vkDestroySamplerYcbcrConversion"))?;
            Some(Ahb {
                get_properties: std::mem::transmute::<
                    PFN_vkVoidFunction,
                    GetHardwareBufferProperties,
                >(get),
                create_conversion: std::mem::transmute::<PFN_vkVoidFunction, CreateConversion>(
                    create,
                ),
                destroy_conversion: std::mem::transmute::<PFN_vkVoidFunction, DestroyConversion>(
                    destroy,
                ),
                conversions: HashMap::new(),
                imports: HashMap::new(),
                warned: false,
            })
        }
    }
}

/// `image`'s `AHardwareBuffer*`, if it is one.
fn hardware_buffer(image: &ExternalImage) -> Option<*mut c_void> {
    #[cfg(target_os = "android")]
    if let crate::platform::external::Handle::HardwareBuffer(b) = *image.handle() {
        return Some(b);
    }
    let _ = image;
    None
}

impl VkExecutor {
    /// What `image` is drawn from if it is a hardware buffer this device
    /// imports (imported now or found imported), with the barrier taking
    /// it over from its producer added to `barriers`; `None` otherwise.
    pub(super) unsafe fn import_hardware_buffer(
        &mut self,
        image: &ExternalImage,
        barriers: &mut Vec<VkImageMemoryBarrier>,
    ) -> Option<ExtView> {
        let buffer = hardware_buffer(image)?;
        let mut ahb = self.externals.ahb.take()?;
        image.wait_acquire();
        let key = buffer as usize;
        if !ahb.imports.contains_key(&key) {
            let made = unsafe { self.import_buffer(&mut ahb, buffer, image) };
            if made.is_none() && !std::mem::replace(&mut ahb.warned, true) {
                eprintln!(
                    "stipple: Vulkan didn't import the {:?} hardware buffer; drawing it from its pixels",
                    image.format()
                );
            }
            ahb.imports.insert(
                key,
                Entry {
                    import: made,
                    used: self.count,
                },
            );
        }
        let entry = ahb.imports.get_mut(&key).unwrap();
        entry.used = self.count;
        let view = match &entry.import {
            Some(i) => {
                barriers.push(VkImageMemoryBarrier {
                    sType: ST_IMAGE_MEMORY_BARRIER,
                    pNext: std::ptr::null(),
                    srcAccessMask: 0,
                    dstAccessMask: external::ACCESS_SHADER_READ,
                    // The producer wrote it since: its contents come over
                    // with the queue family (as Skia and ANGLE take them).
                    oldLayout: LAYOUT_UNDEFINED,
                    newLayout: LAYOUT_SHADER_READ_ONLY,
                    srcQueueFamilyIndex: QUEUE_FAMILY_FOREIGN,
                    dstQueueFamilyIndex: self.family,
                    image: i.image,
                    subresourceRange: subresource(ASPECT_COLOR),
                });
                Some(match i.conversion {
                    Some(c) => ExtView::Ycbcr {
                        set: i.set,
                        pipeline: c.pipeline,
                        layout: c.layout,
                    },
                    None => ExtView::Imported(i.view),
                })
            }
            _ => None,
        };
        self.externals.ahb = Some(ahb);
        view
    }

    /// Import `buffer` (of `image`): its image, memory and view, and its
    /// descriptor set through its conversion where it has one.
    unsafe fn import_buffer(
        &mut self,
        ahb: &mut Ahb,
        buffer: *mut c_void,
        image: &ExternalImage,
    ) -> Option<Import> {
        unsafe {
            let d = self.device;
            let mut format = HardwareBufferFormatProperties::empty();
            let mut props = HardwareBufferProperties {
                sType: ST_ANDROID_HARDWARE_BUFFER_PROPERTIES,
                pNext: (&mut format as *mut HardwareBufferFormatProperties).cast(),
                allocationSize: 0,
                memoryTypeBits: 0,
            };
            if (ahb.get_properties)(d, buffer, &mut props) != VK_SUCCESS {
                return None;
            }
            let key = format.needs_conversion().then(|| format.conversion_key());
            let conversion = match key {
                Some(k) => Some(self.conversion(ahb, k)?),
                None => None,
            };
            let external = match key {
                Some(k) => k.external_format(),
                None => ExternalFormatAndroid {
                    sType: ST_EXTERNAL_FORMAT_ANDROID,
                    pNext: std::ptr::null(),
                    externalFormat: 0,
                },
            };
            let mut memory_ci = ExternalMemoryImageCreateInfo {
                sType: 0,
                pNext: std::ptr::null(),
                handleTypes: 0,
            };
            let size = image.size();
            let ci = VkImageCreateInfo {
                sType: ST_IMAGE_CREATE_INFO,
                pNext: image_chain(&mut memory_ci, &external),
                flags: 0,
                imageType: IMAGE_TYPE_2D,
                format: format.format,
                extent: VkExtent3D {
                    width: size.width,
                    height: size.height,
                    depth: 1,
                },
                mipLevels: 1,
                arrayLayers: 1,
                samples: SAMPLE_COUNT_1,
                tiling: TILING_OPTIMAL,
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
            let n = std::ptr::null();
            let Some(type_index) = memory_type(props.memoryTypeBits) else {
                (self.vk.vkDestroyImage)(d, img, n);
                return None;
            };
            let mut import = ImportHardwareBufferInfo {
                sType: 0,
                pNext: std::ptr::null(),
                buffer: std::ptr::null_mut(),
            };
            let mut dedicated = MemoryDedicatedAllocateInfo {
                sType: 0,
                pNext: std::ptr::null(),
                image: NULL,
                buffer: NULL,
            };
            let ai = VkMemoryAllocateInfo {
                sType: ST_MEMORY_ALLOCATE_INFO,
                pNext: memory_chain(&mut import, &mut dedicated, buffer, img),
                allocationSize: props.allocationSize,
                memoryTypeIndex: type_index,
            };
            let mut mem = NULL;
            if (self.vk.vkAllocateMemory)(d, &ai, n, &mut mem) != VK_SUCCESS {
                (self.vk.vkDestroyImage)(d, img, n);
                return None;
            }
            let fail = |vk: &Fns, view: VkImageView| {
                if view != NULL {
                    (vk.vkDestroyImageView)(d, view, n);
                }
                (vk.vkDestroyImage)(d, img, n);
                (vk.vkFreeMemory)(d, mem, n);
                None
            };
            if (self.vk.vkBindImageMemory)(d, img, mem, 0) != VK_SUCCESS {
                return fail(&self.vk, NULL);
            }
            let ycbcr = SamplerYcbcrConversionInfo {
                sType: ST_SAMPLER_YCBCR_CONVERSION_INFO,
                pNext: std::ptr::null(),
                conversion: conversion.map_or(NULL, |c| c.conversion),
            };
            // A view through a conversion maps no components itself.
            let alpha = if conversion.is_none() && image.format() == ExternalFormat::Rgbx8 {
                external::SWIZZLE_ONE
            } else {
                external::SWIZZLE_IDENTITY
            };
            let vi = VkImageViewCreateInfo {
                sType: ST_IMAGE_VIEW_CREATE_INFO,
                pNext: if conversion.is_some() {
                    (&ycbcr as *const SamplerYcbcrConversionInfo).cast()
                } else {
                    std::ptr::null()
                },
                flags: 0,
                image: img,
                viewType: VIEW_TYPE_2D,
                format: format.format,
                components: VkComponentMapping {
                    a: alpha,
                    ..Default::default()
                },
                subresourceRange: subresource(ASPECT_COLOR),
            };
            let mut view = NULL;
            if (self.vk.vkCreateImageView)(d, &vi, n, &mut view) != VK_SUCCESS {
                return fail(&self.vk, NULL);
            }
            let (mut pool, mut set) = (NULL, NULL);
            if let Some(c) = conversion {
                // A conversion's sampler may take up to three descriptors
                // (one a plane).
                let size = VkDescriptorPoolSize {
                    ty: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                    descriptorCount: 3,
                };
                let pi = VkDescriptorPoolCreateInfo {
                    sType: ST_DESCRIPTOR_POOL_CREATE_INFO,
                    pNext: std::ptr::null(),
                    flags: 0,
                    maxSets: 1,
                    poolSizeCount: 1,
                    pPoolSizes: &size,
                };
                if (self.vk.vkCreateDescriptorPool)(d, &pi, n, &mut pool) != VK_SUCCESS {
                    return fail(&self.vk, view);
                }
                let sa = VkDescriptorSetAllocateInfo {
                    sType: ST_DESCRIPTOR_SET_ALLOCATE_INFO,
                    pNext: std::ptr::null(),
                    descriptorPool: pool,
                    descriptorSetCount: 1,
                    pSetLayouts: &c.set_layout,
                };
                if (self.vk.vkAllocateDescriptorSets)(d, &sa, &mut set) != VK_SUCCESS {
                    (self.vk.vkDestroyDescriptorPool)(d, pool, n);
                    return fail(&self.vk, view);
                }
                let info = VkDescriptorImageInfo {
                    sampler: c.sampler,
                    imageView: view,
                    imageLayout: LAYOUT_SHADER_READ_ONLY,
                };
                let write = VkWriteDescriptorSet {
                    sType: ST_WRITE_DESCRIPTOR_SET,
                    pNext: std::ptr::null(),
                    dstSet: set,
                    dstBinding: 0,
                    dstArrayElement: 0,
                    descriptorCount: 1,
                    descriptorType: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                    pImageInfo: &info,
                    pBufferInfo: std::ptr::null(),
                    pTexelBufferView: std::ptr::null(),
                };
                (self.vk.vkUpdateDescriptorSets)(d, 1, &write, 0, std::ptr::null());
            }
            Some(Import {
                image: img,
                memory: mem,
                view,
                pool,
                set,
                conversion,
            })
        }
    }

    /// The conversion `key` describes, its sampler, set layout, pipeline
    /// layout and quads pipeline, made the first time.
    unsafe fn conversion(&mut self, ahb: &mut Ahb, key: ConversionKey) -> Option<Conversion> {
        if let Some(&c) = ahb.conversions.get(&key) {
            return Some(c);
        }
        unsafe {
            let d = self.device;
            let n = std::ptr::null();
            let external = key.external_format();
            let info = key.conversion_info(&external);
            let mut c = Conversion {
                conversion: NULL,
                sampler: NULL,
                set_layout: NULL,
                layout: NULL,
                pipeline: NULL,
            };
            if (ahb.create_conversion)(d, &info, n, &mut c.conversion) != VK_SUCCESS {
                return None;
            }
            let made = self.conversion_objects(&mut c, key.filter);
            if made.is_err() {
                self.free_conversion(ahb, c);
                return None;
            }
            ahb.conversions.insert(key, c);
            Some(c)
        }
    }

    /// The sampler, layouts and pipeline of the conversion `c`, filtering
    /// with `filter` (the conversion's chroma filter).
    unsafe fn conversion_objects(&mut self, c: &mut Conversion, filter: u32) -> Result<(), ()> {
        unsafe {
            let d = self.device;
            let n = std::ptr::null();
            let ok = |r| if r == VK_SUCCESS { Ok(()) } else { Err(()) };
            let ycbcr = SamplerYcbcrConversionInfo {
                sType: ST_SAMPLER_YCBCR_CONVERSION_INFO,
                pNext: std::ptr::null(),
                conversion: c.conversion,
            };
            let s = VkSamplerCreateInfo {
                sType: ST_SAMPLER_CREATE_INFO,
                pNext: (&ycbcr as *const SamplerYcbcrConversionInfo).cast(),
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
            ok((self.vk.vkCreateSampler)(d, &s, n, &mut c.sampler))?;
            // The sampler is immutable: it is part of the layout.
            let binding = VkDescriptorSetLayoutBinding {
                binding: 0,
                descriptorType: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                descriptorCount: 1,
                stageFlags: SHADER_STAGE_FRAGMENT,
                pImmutableSamplers: &c.sampler,
            };
            let dsl = VkDescriptorSetLayoutCreateInfo {
                sType: ST_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
                pNext: std::ptr::null(),
                flags: 0,
                bindingCount: 1,
                pBindings: &binding,
            };
            ok((self.vk.vkCreateDescriptorSetLayout)(
                d,
                &dsl,
                n,
                &mut c.set_layout,
            ))?;
            // The same push constants as every other pipeline's.
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
                pSetLayouts: &c.set_layout,
                pushConstantRangeCount: 1,
                pPushConstantRanges: &range,
            };
            ok((self.vk.vkCreatePipelineLayout)(d, &pl, n, &mut c.layout))?;
            let vs = self.shader(shaders::QUAD_VERT).map_err(|_| ())?;
            let fs = match self.shader(shaders::QUAD_FRAG) {
                Ok(m) => m,
                Err(_) => {
                    (self.vk.vkDestroyShaderModule)(d, vs, n);
                    return Err(());
                }
            };
            let pipeline = self.pipeline(Pipe::Quads, vs, fs, c.layout);
            (self.vk.vkDestroyShaderModule)(d, vs, n);
            (self.vk.vkDestroyShaderModule)(d, fs, n);
            c.pipeline = pipeline.map_err(|_| ())?;
            Ok(())
        }
    }

    unsafe fn free_conversion(&self, ahb: &Ahb, c: Conversion) {
        unsafe {
            let d = self.device;
            let n = std::ptr::null();
            if c.pipeline != NULL {
                (self.vk.vkDestroyPipeline)(d, c.pipeline, n);
            }
            if c.layout != NULL {
                (self.vk.vkDestroyPipelineLayout)(d, c.layout, n);
            }
            if c.set_layout != NULL {
                (self.vk.vkDestroyDescriptorSetLayout)(d, c.set_layout, n);
            }
            if c.sampler != NULL {
                (self.vk.vkDestroySampler)(d, c.sampler, n);
            }
            if c.conversion != NULL {
                (ahb.destroy_conversion)(d, c.conversion, n);
            }
        }
    }

    unsafe fn free_buffer_import(&self, i: Import) {
        unsafe {
            let d = self.device;
            let n = std::ptr::null();
            if i.pool != NULL {
                (self.vk.vkDestroyDescriptorPool)(d, i.pool, n);
            }
            (self.vk.vkDestroyImageView)(d, i.view, n);
            (self.vk.vkDestroyImage)(d, i.image, n);
            // Lets go of the import's reference to the buffer.
            (self.vk.vkFreeMemory)(d, i.memory, n);
        }
    }

    /// Free the buffer imports not drawn for `keep` frames (all of them,
    /// and the conversions too, with `keep` zero: the device is going
    /// away).
    pub(super) unsafe fn free_buffer_imports(&mut self, keep: u64) {
        let Some(mut ahb) = self.externals.ahb.take() else {
            return;
        };
        let count = self.count;
        let gone: Vec<usize> = ahb
            .imports
            .iter()
            .filter(|(_, e)| keep == 0 || count.saturating_sub(e.used) >= keep)
            .map(|(&k, _)| k)
            .collect();
        for k in gone {
            if let Some(Entry {
                import: Some(i), ..
            }) = ahb.imports.remove(&k)
            {
                unsafe { self.free_buffer_import(i) };
            }
        }
        if keep == 0 {
            for (_, c) in std::mem::take(&mut ahb.conversions) {
                unsafe { self.free_conversion(&ahb, c) };
            }
        }
        self.externals.ahb = Some(ahb);
    }
}
