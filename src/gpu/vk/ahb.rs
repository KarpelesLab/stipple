//! What importing an Android `AHardwareBuffer` into Vulkan takes
//! (`VK_ANDROID_external_memory_android_hardware_buffer`), as plain data:
//! the structs (checked against the C headers' layouts by the tests), the
//! sampler YCbCr conversion a buffer's format properties ask for, and the
//! `pNext` chains of the image and its memory. The executor
//! (`exec/ahb.rs`) makes the Vulkan objects from them; this part is tested
//! on every host.
//!
//! A camera's YUV buffer (`AHARDWAREBUFFER_FORMAT_Y8Cb8Cr8_420`) usually
//! has no Vulkan format of its own: its properties give an *external
//! format* (`VK_FORMAT_UNDEFINED` plus an opaque `externalFormat`), which
//! the image and the YCbCr conversion both name through
//! `VkExternalFormatANDROID`. The conversion takes the model, range and
//! chroma siting the driver suggests, and filters linearly where the format
//! allows it; the sampler built on it is immutable in its descriptor set
//! layout, and the shader samples RGB from it like any texture.

#![allow(non_snake_case, dead_code)]

use super::sys::{FILTER_LINEAR, FILTER_NEAREST, FORMAT_UNDEFINED, VkComponentMapping, VkImage};
use std::ffi::c_void;

pub(crate) const ST_PHYSICAL_DEVICE_FEATURES_2: i32 = 1_000_059_000;
pub(crate) const ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO: i32 = 1_000_072_001;
pub(crate) const ST_MEMORY_DEDICATED_ALLOCATE_INFO: i32 = 1_000_127_001;
pub(crate) const ST_ANDROID_HARDWARE_BUFFER_PROPERTIES: i32 = 1_000_129_001;
pub(crate) const ST_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES: i32 = 1_000_129_002;
pub(crate) const ST_IMPORT_ANDROID_HARDWARE_BUFFER_INFO: i32 = 1_000_129_003;
pub(crate) const ST_EXTERNAL_FORMAT_ANDROID: i32 = 1_000_129_005;
pub(crate) const ST_SAMPLER_YCBCR_CONVERSION_CREATE_INFO: i32 = 1_000_156_000;
pub(crate) const ST_SAMPLER_YCBCR_CONVERSION_INFO: i32 = 1_000_156_001;
pub(crate) const ST_PHYSICAL_DEVICE_SAMPLER_YCBCR_CONVERSION_FEATURES: i32 = 1_000_156_004;

/// `VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID`.
pub(crate) const HANDLE_ANDROID_HARDWARE_BUFFER: u32 = 0x400;
/// `VK_QUEUE_FAMILY_FOREIGN_EXT`: the buffer's producer (the camera) owns
/// it between frames.
pub(crate) const QUEUE_FAMILY_FOREIGN: u32 = !2;
/// `VK_FORMAT_FEATURE_SAMPLED_IMAGE_YCBCR_CONVERSION_LINEAR_FILTER_BIT`.
pub(crate) const FEATURE_YCBCR_LINEAR_FILTER: u32 = 0x0004_0000;
/// The multi-planar and other YCbCr formats (`VK_FORMAT_G8B8G8R8_422_UNORM`
/// through `VK_FORMAT_G16_B16_R16_3PLANE_444_UNORM`): sampled through a
/// conversion, as an external format is.
const YCBCR_FORMATS: std::ops::RangeInclusive<u32> = 1_000_156_000..=1_000_156_033;

/// The device extensions the import takes, all of them or none: the
/// extension itself and what it builds on (on a 1.0 device; a later one
/// lists them too).
pub(crate) const DEVICE_EXTS: &[&std::ffi::CStr] = &[
    c"VK_ANDROID_external_memory_android_hardware_buffer",
    c"VK_KHR_sampler_ycbcr_conversion",
    c"VK_KHR_external_memory",
    c"VK_EXT_queue_family_foreign",
    c"VK_KHR_dedicated_allocation",
    c"VK_KHR_get_memory_requirements2",
    c"VK_KHR_maintenance1",
    c"VK_KHR_bind_memory2",
];

/// `VkAndroidHardwareBufferPropertiesANDROID`.
#[repr(C)]
pub(crate) struct HardwareBufferProperties {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub allocationSize: u64,
    pub memoryTypeBits: u32,
}

/// `VkAndroidHardwareBufferFormatPropertiesANDROID`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct HardwareBufferFormatProperties {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub format: u32,
    pub externalFormat: u64,
    pub formatFeatures: u32,
    pub samplerYcbcrConversionComponents: VkComponentMapping,
    pub suggestedYcbcrModel: u32,
    pub suggestedYcbcrRange: u32,
    pub suggestedXChromaOffset: u32,
    pub suggestedYChromaOffset: u32,
}

/// `VkImportAndroidHardwareBufferInfoANDROID`.
#[repr(C)]
pub(crate) struct ImportHardwareBufferInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub buffer: *mut c_void,
}

/// `VkExternalFormatANDROID`.
#[repr(C)]
pub(crate) struct ExternalFormatAndroid {
    pub sType: i32,
    pub pNext: *const c_void,
    pub externalFormat: u64,
}

/// `VkExternalMemoryImageCreateInfo`.
#[repr(C)]
pub(crate) struct ExternalMemoryImageCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub handleTypes: u32,
}

/// `VkMemoryDedicatedAllocateInfo`.
#[repr(C)]
pub(crate) struct MemoryDedicatedAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub image: VkImage,
    pub buffer: u64,
}

/// `VkSamplerYcbcrConversionCreateInfo`.
#[repr(C)]
pub(crate) struct SamplerYcbcrConversionCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub format: u32,
    pub ycbcrModel: u32,
    pub ycbcrRange: u32,
    pub components: VkComponentMapping,
    pub xChromaOffset: u32,
    pub yChromaOffset: u32,
    pub chromaFilter: u32,
    pub forceExplicitReconstruction: u32,
}

/// `VkSamplerYcbcrConversionInfo`: on the sampler and the image view.
#[repr(C)]
pub(crate) struct SamplerYcbcrConversionInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub conversion: u64,
}

/// `VkPhysicalDeviceSamplerYcbcrConversionFeatures`.
#[repr(C)]
pub(crate) struct SamplerYcbcrConversionFeatures {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub samplerYcbcrConversion: u32,
}

/// `VkPhysicalDeviceFeatures2`: the 55 `VkBool32`s of
/// `VkPhysicalDeviceFeatures`, untouched here, then the chain.
#[repr(C)]
pub(crate) struct PhysicalDeviceFeatures2 {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub features: [u32; 55],
}

/// `vkGetAndroidHardwareBufferPropertiesANDROID`.
pub(crate) type GetHardwareBufferProperties =
    unsafe extern "system" fn(*mut c_void, *const c_void, *mut HardwareBufferProperties) -> i32;
/// `vkCreateSamplerYcbcrConversion{,KHR}`.
pub(crate) type CreateConversion = unsafe extern "system" fn(
    *mut c_void,
    *const SamplerYcbcrConversionCreateInfo,
    *const c_void,
    *mut u64,
) -> i32;
/// `vkDestroySamplerYcbcrConversion{,KHR}`.
pub(crate) type DestroyConversion = unsafe extern "system" fn(*mut c_void, u64, *const c_void);
/// `vkGetPhysicalDeviceFeatures2{,KHR}`.
pub(crate) type GetFeatures2 = unsafe extern "system" fn(*mut c_void, *mut PhysicalDeviceFeatures2);

impl HardwareBufferFormatProperties {
    /// Empty, for the driver to fill in.
    pub(crate) fn empty() -> Self {
        HardwareBufferFormatProperties {
            sType: ST_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES,
            pNext: std::ptr::null_mut(),
            format: FORMAT_UNDEFINED,
            externalFormat: 0,
            formatFeatures: 0,
            samplerYcbcrConversionComponents: VkComponentMapping::default(),
            suggestedYcbcrModel: 0,
            suggestedYcbcrRange: 0,
            suggestedXChromaOffset: 0,
            suggestedYChromaOffset: 0,
        }
    }

    /// Whether the buffer is sampled through a YCbCr conversion: an
    /// external format, or a Vulkan YCbCr one.
    pub(crate) fn needs_conversion(&self) -> bool {
        self.format == FORMAT_UNDEFINED || YCBCR_FORMATS.contains(&self.format)
    }

    /// What tells one conversion from another: buffers alike share it, and
    /// its sampler, set layout and pipeline.
    pub(crate) fn conversion_key(&self) -> ConversionKey {
        let c = self.samplerYcbcrConversionComponents;
        ConversionKey {
            format: self.format,
            external: if self.format == FORMAT_UNDEFINED {
                self.externalFormat
            } else {
                0
            },
            model: self.suggestedYcbcrModel,
            range: self.suggestedYcbcrRange,
            chroma: [self.suggestedXChromaOffset, self.suggestedYChromaOffset],
            components: [c.r, c.g, c.b, c.a],
            filter: if self.formatFeatures & FEATURE_YCBCR_LINEAR_FILTER != 0 {
                FILTER_LINEAR
            } else {
                FILTER_NEAREST
            },
        }
    }
}

/// A YCbCr conversion's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConversionKey {
    /// The Vulkan format, `VK_FORMAT_UNDEFINED` for an external one.
    pub format: u32,
    /// The external format (zero with a Vulkan format).
    pub external: u64,
    pub model: u32,
    pub range: u32,
    /// The X and Y chroma offsets.
    pub chroma: [u32; 2],
    pub components: [u32; 4],
    /// The chroma filter, and the sampler's (they must match).
    pub filter: u32,
}

impl ConversionKey {
    /// The external format struct the image and the conversion chain
    /// (its `externalFormat` zero with a Vulkan format, which is then
    /// left out).
    pub(crate) fn external_format(&self) -> ExternalFormatAndroid {
        ExternalFormatAndroid {
            sType: ST_EXTERNAL_FORMAT_ANDROID,
            pNext: std::ptr::null(),
            externalFormat: self.external,
        }
    }

    /// The conversion's create info, chained to `external` when the format
    /// is an external one.
    pub(crate) fn conversion_info(
        &self,
        external: &ExternalFormatAndroid,
    ) -> SamplerYcbcrConversionCreateInfo {
        let [r, g, b, a] = self.components;
        SamplerYcbcrConversionCreateInfo {
            sType: ST_SAMPLER_YCBCR_CONVERSION_CREATE_INFO,
            pNext: if self.external != 0 {
                (external as *const ExternalFormatAndroid).cast()
            } else {
                std::ptr::null()
            },
            format: self.format,
            ycbcrModel: self.model,
            ycbcrRange: self.range,
            components: VkComponentMapping { r, g, b, a },
            xChromaOffset: self.chroma[0],
            yChromaOffset: self.chroma[1],
            chromaFilter: self.filter,
            forceExplicitReconstruction: 0,
        }
    }
}

/// The image's `pNext` chain: the external memory it's made for, and the
/// external format when it has one (`external`, which must outlive the
/// chain like `memory`).
pub(crate) fn image_chain(
    memory: &mut ExternalMemoryImageCreateInfo,
    external: &ExternalFormatAndroid,
) -> *const c_void {
    memory.sType = ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO;
    memory.handleTypes = HANDLE_ANDROID_HARDWARE_BUFFER;
    memory.pNext = if external.externalFormat != 0 {
        (external as *const ExternalFormatAndroid).cast()
    } else {
        std::ptr::null()
    };
    (memory as *const ExternalMemoryImageCreateInfo).cast()
}

/// The allocation's `pNext` chain: the buffer imported, then the image it
/// is dedicated to (the extension requires a dedicated allocation).
pub(crate) fn memory_chain(
    import: &mut ImportHardwareBufferInfo,
    dedicated: &mut MemoryDedicatedAllocateInfo,
    buffer: *mut c_void,
    image: VkImage,
) -> *const c_void {
    *dedicated = MemoryDedicatedAllocateInfo {
        sType: ST_MEMORY_DEDICATED_ALLOCATE_INFO,
        pNext: std::ptr::null(),
        image,
        buffer: 0,
    };
    *import = ImportHardwareBufferInfo {
        sType: ST_IMPORT_ANDROID_HARDWARE_BUFFER_INFO,
        pNext: (dedicated as *const MemoryDedicatedAllocateInfo).cast(),
        buffer,
    };
    (import as *const ImportHardwareBufferInfo).cast()
}

/// The lowest memory type in `bits`.
pub(crate) fn memory_type(bits: u32) -> Option<u32> {
    (bits != 0).then(|| bits.trailing_zeros())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// A camera buffer as the emulator's gralloc describes it: an external
    /// format, BT.601 narrow range, midpoint chroma, linear filtering.
    fn camera() -> HardwareBufferFormatProperties {
        HardwareBufferFormatProperties {
            format: FORMAT_UNDEFINED,
            externalFormat: 0x23,
            formatFeatures: 0x1 | FEATURE_YCBCR_LINEAR_FILTER,
            suggestedYcbcrModel: 3,
            suggestedYcbcrRange: 1,
            suggestedXChromaOffset: 1,
            suggestedYChromaOffset: 1,
            ..HardwareBufferFormatProperties::empty()
        }
    }

    #[test]
    fn layouts_match_the_c_headers() {
        if size_of::<usize>() != 8 {
            return;
        }
        // `sizeof` and `offsetof` from vulkan_core.h / vulkan_android.h on
        // a 64-bit target.
        assert_eq!(size_of::<HardwareBufferProperties>(), 32);
        assert_eq!(offset_of!(HardwareBufferProperties, allocationSize), 16);
        assert_eq!(offset_of!(HardwareBufferProperties, memoryTypeBits), 24);
        assert_eq!(size_of::<HardwareBufferFormatProperties>(), 72);
        assert_eq!(offset_of!(HardwareBufferFormatProperties, format), 16);
        assert_eq!(
            offset_of!(HardwareBufferFormatProperties, externalFormat),
            24
        );
        assert_eq!(
            offset_of!(HardwareBufferFormatProperties, formatFeatures),
            32
        );
        assert_eq!(
            offset_of!(
                HardwareBufferFormatProperties,
                samplerYcbcrConversionComponents
            ),
            36
        );
        assert_eq!(
            offset_of!(HardwareBufferFormatProperties, suggestedYcbcrModel),
            52
        );
        assert_eq!(
            offset_of!(HardwareBufferFormatProperties, suggestedYChromaOffset),
            64
        );
        assert_eq!(size_of::<ImportHardwareBufferInfo>(), 24);
        assert_eq!(size_of::<ExternalFormatAndroid>(), 24);
        assert_eq!(offset_of!(ExternalFormatAndroid, externalFormat), 16);
        assert_eq!(size_of::<ExternalMemoryImageCreateInfo>(), 24);
        assert_eq!(size_of::<MemoryDedicatedAllocateInfo>(), 32);
        assert_eq!(size_of::<SamplerYcbcrConversionCreateInfo>(), 64);
        assert_eq!(offset_of!(SamplerYcbcrConversionCreateInfo, format), 16);
        assert_eq!(offset_of!(SamplerYcbcrConversionCreateInfo, components), 28);
        assert_eq!(
            offset_of!(SamplerYcbcrConversionCreateInfo, xChromaOffset),
            44
        );
        assert_eq!(
            offset_of!(SamplerYcbcrConversionCreateInfo, chromaFilter),
            52
        );
        assert_eq!(
            offset_of!(
                SamplerYcbcrConversionCreateInfo,
                forceExplicitReconstruction
            ),
            56
        );
        assert_eq!(size_of::<SamplerYcbcrConversionInfo>(), 24);
        assert_eq!(size_of::<SamplerYcbcrConversionFeatures>(), 24);
        assert_eq!(size_of::<PhysicalDeviceFeatures2>(), 240);
    }

    #[test]
    fn an_external_format_is_converted_with_what_the_driver_suggests() {
        let props = camera();
        assert!(props.needs_conversion());
        let key = props.conversion_key();
        assert_eq!(key.external, 0x23);
        assert_eq!(key.filter, FILTER_LINEAR);
        let ext = key.external_format();
        assert_eq!(ext.sType, ST_EXTERNAL_FORMAT_ANDROID);
        assert_eq!(ext.externalFormat, 0x23);
        let info = key.conversion_info(&ext);
        assert_eq!(info.sType, ST_SAMPLER_YCBCR_CONVERSION_CREATE_INFO);
        assert_eq!(info.format, FORMAT_UNDEFINED);
        assert_eq!(info.pNext, (&ext as *const ExternalFormatAndroid).cast());
        assert_eq!((info.ycbcrModel, info.ycbcrRange), (3, 1));
        assert_eq!((info.xChromaOffset, info.yChromaOffset), (1, 1));
        assert_eq!(info.chromaFilter, FILTER_LINEAR);
        assert_eq!(info.forceExplicitReconstruction, 0);
    }

    #[test]
    fn a_vulkan_format_needs_no_external_format() {
        // VK_FORMAT_G8_B8R8_2PLANE_420_UNORM, nearest only.
        let props = HardwareBufferFormatProperties {
            format: 1_000_156_003,
            externalFormat: 0x99,
            formatFeatures: 0x1,
            ..camera()
        };
        assert!(props.needs_conversion());
        let key = props.conversion_key();
        assert_eq!(key.external, 0, "only an undefined format is external");
        assert_eq!(key.filter, FILTER_NEAREST);
        let ext = key.external_format();
        assert!(key.conversion_info(&ext).pNext.is_null());
        // An RGBA buffer is sampled as is.
        let rgba = HardwareBufferFormatProperties {
            format: super::super::sys::FORMAT_R8G8B8A8_UNORM,
            ..camera()
        };
        assert!(!rgba.needs_conversion());
    }

    #[test]
    fn same_buffers_share_a_conversion() {
        assert_eq!(camera().conversion_key(), camera().conversion_key());
        let other = HardwareBufferFormatProperties {
            suggestedYcbcrRange: 0,
            ..camera()
        };
        assert_ne!(camera().conversion_key(), other.conversion_key());
    }

    #[test]
    fn the_chains_link_up() {
        let key = camera().conversion_key();
        let ext = key.external_format();
        let mut mem = ExternalMemoryImageCreateInfo {
            sType: 0,
            pNext: std::ptr::null(),
            handleTypes: 0,
        };
        let head = image_chain(&mut mem, &ext);
        assert_eq!(head, (&mem as *const ExternalMemoryImageCreateInfo).cast());
        assert_eq!(mem.sType, ST_EXTERNAL_MEMORY_IMAGE_CREATE_INFO);
        assert_eq!(mem.handleTypes, HANDLE_ANDROID_HARDWARE_BUFFER);
        assert_eq!(mem.pNext, (&ext as *const ExternalFormatAndroid).cast());
        // No external format: the chain ends at the memory.
        let none = ExternalFormatAndroid {
            externalFormat: 0,
            ..key.external_format()
        };
        image_chain(&mut mem, &none);
        assert!(mem.pNext.is_null());

        let buffer = 0x1234usize as *mut c_void;
        let mut import = ImportHardwareBufferInfo {
            sType: 0,
            pNext: std::ptr::null(),
            buffer: std::ptr::null_mut(),
        };
        let mut dedicated = MemoryDedicatedAllocateInfo {
            sType: 0,
            pNext: std::ptr::null(),
            image: 0,
            buffer: 0,
        };
        let head = memory_chain(&mut import, &mut dedicated, buffer, 77);
        assert_eq!(head, (&import as *const ImportHardwareBufferInfo).cast());
        assert_eq!(import.sType, ST_IMPORT_ANDROID_HARDWARE_BUFFER_INFO);
        assert_eq!(import.buffer, buffer);
        assert_eq!(
            import.pNext,
            (&dedicated as *const MemoryDedicatedAllocateInfo).cast()
        );
        assert_eq!(dedicated.sType, ST_MEMORY_DEDICATED_ALLOCATE_INFO);
        assert_eq!(dedicated.image, 77);
        assert!(dedicated.pNext.is_null());
    }

    #[test]
    fn the_lowest_memory_type_is_taken() {
        assert_eq!(memory_type(0), None);
        assert_eq!(memory_type(0b1010_0000), Some(5));
    }
}
