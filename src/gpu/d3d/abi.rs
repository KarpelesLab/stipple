//! The Direct3D 11 / DXGI ABI Stipple calls by hand: the `#[repr(C)]`
//! descriptors, the enum values, the COM vtable slots, and the pure helpers
//! around them (packing shader constants, reading HRESULTs, converting the
//! read-back pixels).
//!
//! It holds no FFI calls, so it is compiled on every platform when testing:
//! the descriptor sizes and the helpers are unit-tested on any host, while
//! the calls through these slots run only on Windows (CI's `visual-windows`
//! job, on WARP).
//!
//! Every vtable slot is the method's index in its interface's vtable, counted
//! from the headers' declaration order with the inherited methods first
//! (`IUnknown`: `QueryInterface` 0, `AddRef` 1, `Release` 2).
#![cfg_attr(not(all(target_os = "windows", feature = "d3d")), allow(dead_code))]
#![allow(non_snake_case)]

use core::ffi::c_void;

pub(crate) type Hresult = i32;
pub(crate) const S_OK: Hresult = 0;

// --- d3dcommon.h ---
pub(crate) const D3D_DRIVER_TYPE_HARDWARE: u32 = 1;
/// The software rasterizer shipped with Windows (no GPU required).
pub(crate) const D3D_DRIVER_TYPE_WARP: u32 = 5;
pub(crate) const D3D_FEATURE_LEVEL_10_0: u32 = 0xa000;
pub(crate) const D3D_FEATURE_LEVEL_10_1: u32 = 0xa100;
pub(crate) const D3D_FEATURE_LEVEL_11_0: u32 = 0xb000;
pub(crate) const D3D11_SDK_VERSION: u32 = 7;

// --- d3dcompiler.h ---
pub(crate) const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 1 << 11;
pub(crate) const D3DCOMPILE_OPTIMIZATION_LEVEL3: u32 = 1 << 15;

// --- dxgiformat.h ---
pub(crate) const DXGI_FORMAT_UNKNOWN: u32 = 0;
pub(crate) const DXGI_FORMAT_R32G32B32A32_FLOAT: u32 = 2;
pub(crate) const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28;
pub(crate) const DXGI_FORMAT_D24_UNORM_S8_UINT: u32 = 45;
pub(crate) const DXGI_FORMAT_R8_UNORM: u32 = 61;
pub(crate) const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;

// --- d3d11.h: resources ---
pub(crate) const D3D11_USAGE_DEFAULT: u32 = 0;
pub(crate) const D3D11_USAGE_IMMUTABLE: u32 = 1;
pub(crate) const D3D11_USAGE_DYNAMIC: u32 = 2;
pub(crate) const D3D11_USAGE_STAGING: u32 = 3;
pub(crate) const D3D11_BIND_VERTEX_BUFFER: u32 = 0x1;
pub(crate) const D3D11_BIND_CONSTANT_BUFFER: u32 = 0x4;
pub(crate) const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
pub(crate) const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
pub(crate) const D3D11_BIND_DEPTH_STENCIL: u32 = 0x40;
pub(crate) const D3D11_CPU_ACCESS_WRITE: u32 = 0x1_0000;
pub(crate) const D3D11_CPU_ACCESS_READ: u32 = 0x2_0000;
/// Texture `MiscFlags`: shareable across processes (the `dma-buf` analog).
pub(crate) const D3D11_RESOURCE_MISC_SHARED: u32 = 0x2;
pub(crate) const D3D11_MAP_READ: u32 = 1;
pub(crate) const D3D11_MAP_WRITE_DISCARD: u32 = 4;
pub(crate) const D3D11_MAP_WRITE_NO_OVERWRITE: u32 = 5;
pub(crate) const D3D11_CLEAR_STENCIL: u32 = 0x2;
pub(crate) const D3D11_INPUT_PER_VERTEX_DATA: u32 = 0;
pub(crate) const D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST: u32 = 4;
/// The largest texture side at feature level 11 (10.x: 8192).
pub(crate) const D3D11_MAX_TEXTURE_SIZE_11: i32 = 16384;
pub(crate) const D3D10_MAX_TEXTURE_SIZE: i32 = 8192;

// --- d3d11.h: pipeline state ---
pub(crate) const D3D11_BLEND_ONE: u32 = 2;
pub(crate) const D3D11_BLEND_INV_SRC_ALPHA: u32 = 6;
pub(crate) const D3D11_BLEND_OP_ADD: u32 = 1;
pub(crate) const D3D11_COLOR_WRITE_ENABLE_ALL: u8 = 0xf;
pub(crate) const D3D11_FILL_SOLID: u32 = 3;
pub(crate) const D3D11_CULL_NONE: u32 = 1;
pub(crate) const D3D11_FILTER_MIN_MAG_MIP_POINT: u32 = 0;
pub(crate) const D3D11_FILTER_MIN_MAG_MIP_LINEAR: u32 = 0x15;
pub(crate) const D3D11_TEXTURE_ADDRESS_CLAMP: u32 = 3;
pub(crate) const D3D11_FLOAT32_MAX: f32 = 3.402_823_5e38;
pub(crate) const D3D11_DEPTH_WRITE_MASK_ZERO: u32 = 0;
// D3D11_COMPARISON_FUNC
pub(crate) const D3D11_COMPARISON_NEVER: u32 = 1;
pub(crate) const D3D11_COMPARISON_EQUAL: u32 = 3;
pub(crate) const D3D11_COMPARISON_NOT_EQUAL: u32 = 6;
pub(crate) const D3D11_COMPARISON_ALWAYS: u32 = 8;
// D3D11_STENCIL_OP
pub(crate) const D3D11_STENCIL_OP_KEEP: u32 = 1;
pub(crate) const D3D11_STENCIL_OP_INCR_SAT: u32 = 4;
pub(crate) const D3D11_STENCIL_OP_DECR_SAT: u32 = 5;
pub(crate) const D3D11_STENCIL_OP_INVERT: u32 = 6;
pub(crate) const D3D11_STENCIL_OP_INCR: u32 = 7;
pub(crate) const D3D11_STENCIL_OP_DECR: u32 = 8;

// --- dxgi.h / dxgi1_2.h ---
pub(crate) const DXGI_USAGE_RENDER_TARGET_OUTPUT: u32 = 0x20;
pub(crate) const DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL: u32 = 3;
/// Windows 10 and later.
pub(crate) const DXGI_SWAP_EFFECT_FLIP_DISCARD: u32 = 4;
pub(crate) const DXGI_SCALING_STRETCH: u32 = 0;
pub(crate) const DXGI_ALPHA_MODE_IGNORE: u32 = 3;
/// Windows 8.1 and later (`IDXGISwapChain2`).
pub(crate) const DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT: u32 = 0x40;
pub(crate) const DXGI_MWA_NO_ALT_ENTER: u32 = 0x2;
/// `Present` succeeded but the window isn't visible.
pub(crate) const DXGI_STATUS_OCCLUDED: Hresult = 0x087a_0001;
pub(crate) const DXGI_ERROR_DEVICE_REMOVED: Hresult = 0x887a_0005_u32 as i32;
pub(crate) const DXGI_ERROR_DEVICE_HUNG: Hresult = 0x887a_0006_u32 as i32;
pub(crate) const DXGI_ERROR_DEVICE_RESET: Hresult = 0x887a_0007_u32 as i32;
pub(crate) const DXGI_ERROR_DRIVER_INTERNAL_ERROR: Hresult = 0x887a_0020_u32 as i32;

// --- winbase.h ---
pub(crate) const WAIT_OBJECT_0: u32 = 0;

// --- vtable slots ---

/// `IUnknown`.
pub(crate) const VT_QUERY_INTERFACE: usize = 0;
pub(crate) const VT_RELEASE: usize = 2;

/// `ID3D11Device` (`IUnknown` 0-2, then its own methods in d3d11.h order).
pub(crate) const VT_DEVICE_CREATE_BUFFER: usize = 3;
// 4 CreateTexture1D
pub(crate) const VT_DEVICE_CREATE_TEXTURE2D: usize = 5;
// 6 CreateTexture3D
pub(crate) const VT_DEVICE_CREATE_SRV: usize = 7;
// 8 CreateUnorderedAccessView
pub(crate) const VT_DEVICE_CREATE_RTV: usize = 9;
pub(crate) const VT_DEVICE_CREATE_DSV: usize = 10;
pub(crate) const VT_DEVICE_CREATE_INPUT_LAYOUT: usize = 11;
pub(crate) const VT_DEVICE_CREATE_VERTEX_SHADER: usize = 12;
// 13 CreateGeometryShader, 14 CreateGeometryShaderWithStreamOutput
pub(crate) const VT_DEVICE_CREATE_PIXEL_SHADER: usize = 15;
// 16 CreateHullShader, 17 CreateDomainShader, 18 CreateComputeShader,
// 19 CreateClassLinkage
pub(crate) const VT_DEVICE_CREATE_BLEND_STATE: usize = 20;
pub(crate) const VT_DEVICE_CREATE_DEPTH_STENCIL_STATE: usize = 21;
pub(crate) const VT_DEVICE_CREATE_RASTERIZER_STATE: usize = 22;
pub(crate) const VT_DEVICE_CREATE_SAMPLER_STATE: usize = 23;
// 24 CreateQuery, 25 CreatePredicate, 26 CreateCounter,
// 27 CreateDeferredContext, 28 OpenSharedResource, 29 CheckFormatSupport,
// 30 CheckMultisampleQualityLevels, 31 CheckCounterInfo, 32 CheckCounter,
// 33 CheckFeatureSupport, 34 GetPrivateData, 35 SetPrivateData,
// 36 SetPrivateDataInterface, 37 GetFeatureLevel, 38 GetCreationFlags
pub(crate) const VT_DEVICE_GET_DEVICE_REMOVED_REASON: usize = 39;

/// `ID3D11DeviceContext` (`IUnknown` 0-2, `ID3D11DeviceChild` 3-6:
/// GetDevice, GetPrivateData, SetPrivateData, SetPrivateDataInterface).
pub(crate) const VT_CTX_VS_SET_CONSTANT_BUFFERS: usize = 7;
pub(crate) const VT_CTX_PS_SET_SHADER_RESOURCES: usize = 8;
pub(crate) const VT_CTX_PS_SET_SHADER: usize = 9;
pub(crate) const VT_CTX_PS_SET_SAMPLERS: usize = 10;
pub(crate) const VT_CTX_VS_SET_SHADER: usize = 11;
// 12 DrawIndexed
pub(crate) const VT_CTX_DRAW: usize = 13;
pub(crate) const VT_CTX_MAP: usize = 14;
pub(crate) const VT_CTX_UNMAP: usize = 15;
pub(crate) const VT_CTX_PS_SET_CONSTANT_BUFFERS: usize = 16;
pub(crate) const VT_CTX_IA_SET_INPUT_LAYOUT: usize = 17;
pub(crate) const VT_CTX_IA_SET_VERTEX_BUFFERS: usize = 18;
// 19 IASetIndexBuffer, 20 DrawIndexedInstanced, 21 DrawInstanced,
// 22 GSSetConstantBuffers, 23 GSSetShader
pub(crate) const VT_CTX_IA_SET_PRIMITIVE_TOPOLOGY: usize = 24;
// 25 VSSetShaderResources, 26 VSSetSamplers, 27 Begin, 28 End, 29 GetData,
// 30 SetPredication, 31 GSSetShaderResources, 32 GSSetSamplers
pub(crate) const VT_CTX_OM_SET_RENDER_TARGETS: usize = 33;
// 34 OMSetRenderTargetsAndUnorderedAccessViews
pub(crate) const VT_CTX_OM_SET_BLEND_STATE: usize = 35;
pub(crate) const VT_CTX_OM_SET_DEPTH_STENCIL_STATE: usize = 36;
// 37 SOSetTargets, 38 DrawAuto, 39 DrawIndexedInstancedIndirect,
// 40 DrawInstancedIndirect, 41 Dispatch, 42 DispatchIndirect
pub(crate) const VT_CTX_RS_SET_STATE: usize = 43;
pub(crate) const VT_CTX_RS_SET_VIEWPORTS: usize = 44;
pub(crate) const VT_CTX_RS_SET_SCISSOR_RECTS: usize = 45;
// 46 CopySubresourceRegion
pub(crate) const VT_CTX_COPY_RESOURCE: usize = 47;
pub(crate) const VT_CTX_UPDATE_SUBRESOURCE: usize = 48;
// 49 CopyStructureCount
pub(crate) const VT_CTX_CLEAR_RTV: usize = 50;
// 51 ClearUnorderedAccessViewUint, 52 ClearUnorderedAccessViewFloat
pub(crate) const VT_CTX_CLEAR_DSV: usize = 53;
// 54 GenerateMips … 109 CSGetConstantBuffers (the HS/DS/CS setters at
// 59-71, then the getters from VSGetConstantBuffers at 72)
pub(crate) const VT_CTX_CLEAR_STATE: usize = 110;
pub(crate) const VT_CTX_FLUSH: usize = 111;

/// `ID3DBlob` (`IUnknown` 0-2).
pub(crate) const VT_BLOB_GET_BUFFER_POINTER: usize = 3;
pub(crate) const VT_BLOB_GET_BUFFER_SIZE: usize = 4;

/// `IDXGIObject` (`IUnknown` 0-2, SetPrivateData 3,
/// SetPrivateDataInterface 4, GetPrivateData 5).
pub(crate) const VT_DXGI_OBJECT_GET_PARENT: usize = 6;
/// `IDXGIDevice` (`IDXGIObject` 0-6).
pub(crate) const VT_DXGI_DEVICE_GET_ADAPTER: usize = 7;
/// `IDXGIResource` (`IDXGIObject` 0-6, `IDXGIDeviceSubObject::GetDevice` 7).
pub(crate) const VT_DXGI_RESOURCE_GET_SHARED_HANDLE: usize = 8;
/// `IDXGIFactory` (`IDXGIObject` 0-6, EnumAdapters 7).
pub(crate) const VT_FACTORY_MAKE_WINDOW_ASSOCIATION: usize = 8;
// 9 GetWindowAssociation, 10 CreateSwapChain, 11 CreateSoftwareAdapter,
// IDXGIFactory1: 12 EnumAdapters1, 13 IsCurrent,
// IDXGIFactory2: 14 IsWindowedStereoEnabled
pub(crate) const VT_FACTORY2_CREATE_SWAP_CHAIN_FOR_HWND: usize = 15;
/// `IDXGISwapChain` (`IDXGIObject` 0-6, `IDXGIDeviceSubObject::GetDevice` 7).
pub(crate) const VT_SWAP_PRESENT: usize = 8;
pub(crate) const VT_SWAP_GET_BUFFER: usize = 9;
// 10 SetFullscreenState, 11 GetFullscreenState, 12 GetDesc
pub(crate) const VT_SWAP_RESIZE_BUFFERS: usize = 13;
// 14 ResizeTarget, 15 GetContainingOutput, 16 GetFrameStatistics,
// 17 GetLastPresentCount; IDXGISwapChain1 18-28 (GetDesc1, GetFullscreenDesc,
// GetHwnd, GetCoreWindow, Present1, IsTemporaryMonoSupported,
// GetRestrictToOutput, SetBackgroundColor, GetBackgroundColor, SetRotation,
// GetRotation); IDXGISwapChain2: 29 SetSourceSize, 30 GetSourceSize
/// `IDXGISwapChain1::Present1` (see the list above).
pub(crate) const VT_SWAP1_PRESENT1: usize = 22;
pub(crate) const VT_SWAP2_SET_MAXIMUM_FRAME_LATENCY: usize = 31;
// 32 GetMaximumFrameLatency
pub(crate) const VT_SWAP2_GET_FRAME_LATENCY_WAITABLE_OBJECT: usize = 33;

/// A COM interface id.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// {035f3ab4-482e-4e50-b41f-8a7f8bd8960b}
pub(crate) const IID_IDXGI_RESOURCE: Guid = Guid {
    data1: 0x035f_3ab4,
    data2: 0x482e,
    data3: 0x4e50,
    data4: [0xb4, 0x1f, 0x8a, 0x7f, 0x8b, 0xd8, 0x96, 0x0b],
};
/// {54ec77fa-1377-44e6-8c32-88fd5f44c84c}
pub(crate) const IID_IDXGI_DEVICE: Guid = Guid {
    data1: 0x54ec_77fa,
    data2: 0x1377,
    data3: 0x44e6,
    data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
};
/// {50c83a1c-e072-4c48-87b0-3630fa36a6d0}
pub(crate) const IID_IDXGI_FACTORY2: Guid = Guid {
    data1: 0x50c8_3a1c,
    data2: 0xe072,
    data3: 0x4c48,
    data4: [0x87, 0xb0, 0x36, 0x30, 0xfa, 0x36, 0xa6, 0xd0],
};
/// {a8be2ac4-199f-4946-b331-79599fb98de7}
pub(crate) const IID_IDXGI_SWAP_CHAIN2: Guid = Guid {
    data1: 0xa8be_2ac4,
    data2: 0x199f,
    data3: 0x4946,
    data4: [0xb3, 0x31, 0x79, 0x59, 0x9f, 0xb9, 0x8d, 0xe7],
};
/// {6f15aaf2-d208-4e89-9ab4-489535d34f9c}
pub(crate) const IID_ID3D11_TEXTURE2D: Guid = Guid {
    data1: 0x6f15_aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DxgiSampleDesc {
    pub Count: u32,
    pub Quality: u32,
}

/// `D3D11_TEXTURE2D_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Texture2dDesc {
    pub Width: u32,
    pub Height: u32,
    pub MipLevels: u32,
    pub ArraySize: u32,
    pub Format: u32,
    pub SampleDesc: DxgiSampleDesc,
    pub Usage: u32,
    pub BindFlags: u32,
    pub CPUAccessFlags: u32,
    pub MiscFlags: u32,
}

impl Texture2dDesc {
    /// A single-sampled, single-mip `w`×`h` texture.
    pub(crate) fn new(w: u32, h: u32, format: u32, usage: u32, bind: u32, cpu: u32) -> Self {
        Texture2dDesc {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DxgiSampleDesc {
                Count: 1,
                Quality: 0,
            },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        }
    }
}

/// `D3D11_BUFFER_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct BufferDesc {
    pub ByteWidth: u32,
    pub Usage: u32,
    pub BindFlags: u32,
    pub CPUAccessFlags: u32,
    pub MiscFlags: u32,
    pub StructureByteStride: u32,
}

/// `D3D11_SUBRESOURCE_DATA`
#[repr(C)]
pub(crate) struct SubresourceData {
    pub pSysMem: *const c_void,
    pub SysMemPitch: u32,
    pub SysMemSlicePitch: u32,
}

/// `D3D11_MAPPED_SUBRESOURCE`
#[repr(C)]
pub(crate) struct MappedSubresource {
    pub pData: *mut c_void,
    pub RowPitch: u32,
    pub DepthPitch: u32,
}

/// `D3D11_VIEWPORT`
#[repr(C)]
pub(crate) struct Viewport {
    pub TopLeftX: f32,
    pub TopLeftY: f32,
    pub Width: f32,
    pub Height: f32,
    pub MinDepth: f32,
    pub MaxDepth: f32,
}

/// `D3D11_RECT` (a `RECT`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct D3dRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// `D3D11_BOX`
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct D3dBox {
    pub left: u32,
    pub top: u32,
    pub front: u32,
    pub right: u32,
    pub bottom: u32,
    pub back: u32,
}

impl D3dBox {
    /// The pixels `[x0, y0, x1, y1]` of a 2D texture.
    pub(crate) fn pixels(r: [u32; 4]) -> Self {
        D3dBox {
            left: r[0],
            top: r[1],
            front: 0,
            right: r[2],
            bottom: r[3],
            back: 1,
        }
    }
}

/// `D3D11_INPUT_ELEMENT_DESC`
#[repr(C)]
pub(crate) struct InputElementDesc {
    pub SemanticName: *const u8,
    pub SemanticIndex: u32,
    pub Format: u32,
    pub InputSlot: u32,
    pub AlignedByteOffset: u32,
    pub InputSlotClass: u32,
    pub InstanceDataStepRate: u32,
}

/// `D3D11_RENDER_TARGET_BLEND_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct RenderTargetBlendDesc {
    pub BlendEnable: i32,
    pub SrcBlend: u32,
    pub DestBlend: u32,
    pub BlendOp: u32,
    pub SrcBlendAlpha: u32,
    pub DestBlendAlpha: u32,
    pub BlendOpAlpha: u32,
    pub RenderTargetWriteMask: u8,
}

/// `D3D11_BLEND_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct BlendDesc {
    pub AlphaToCoverageEnable: i32,
    pub IndependentBlendEnable: i32,
    pub RenderTarget: [RenderTargetBlendDesc; 8],
}

/// `D3D11_DEPTH_STENCILOP_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct StencilOpDesc {
    pub StencilFailOp: u32,
    pub StencilDepthFailOp: u32,
    pub StencilPassOp: u32,
    pub StencilFunc: u32,
}

/// `D3D11_DEPTH_STENCIL_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DepthStencilDesc {
    pub DepthEnable: i32,
    pub DepthWriteMask: u32,
    pub DepthFunc: u32,
    pub StencilEnable: i32,
    pub StencilReadMask: u8,
    pub StencilWriteMask: u8,
    pub FrontFace: StencilOpDesc,
    pub BackFace: StencilOpDesc,
}

/// `D3D11_RASTERIZER_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct RasterizerDesc {
    pub FillMode: u32,
    pub CullMode: u32,
    pub FrontCounterClockwise: i32,
    pub DepthBias: i32,
    pub DepthBiasClamp: f32,
    pub SlopeScaledDepthBias: f32,
    pub DepthClipEnable: i32,
    pub ScissorEnable: i32,
    pub MultisampleEnable: i32,
    pub AntialiasedLineEnable: i32,
}

/// `D3D11_SAMPLER_DESC`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SamplerDesc {
    pub Filter: u32,
    pub AddressU: u32,
    pub AddressV: u32,
    pub AddressW: u32,
    pub MipLODBias: f32,
    pub MaxAnisotropy: u32,
    pub ComparisonFunc: u32,
    pub BorderColor: [f32; 4],
    pub MinLOD: f32,
    pub MaxLOD: f32,
}

/// `DXGI_SWAP_CHAIN_DESC1`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SwapChainDesc1 {
    pub Width: u32,
    pub Height: u32,
    pub Format: u32,
    pub Stereo: i32,
    pub SampleDesc: DxgiSampleDesc,
    pub BufferUsage: u32,
    pub BufferCount: u32,
    pub Scaling: u32,
    pub SwapEffect: u32,
    pub AlphaMode: u32,
    pub Flags: u32,
}

/// The swap chains to try for a window, best first: flip-sequential with a
/// frame-latency waitable object (Windows 8.1), whose buffers keep their
/// frames (so a frame can redraw only what changed, and present only that
/// with `Present1`'s dirty rectangles), then flip-discard (Windows 10, no
/// partial presents) with one, then flip-sequential without.
/// `(swap effect, flags)`.
pub(crate) const SWAP_CHAIN_LADDER: [(u32, u32); 3] = [
    (
        DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    ),
    (
        DXGI_SWAP_EFFECT_FLIP_DISCARD,
        DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    ),
    (DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 0),
];

/// How many frames old buffer 0 of a [`swap_chain_desc`] swap chain is,
/// after `presents` frames presented since it was made or resized: two
/// buffers taking turns, so the one drawn into next holds the frame before
/// last; `0` (unknown) until both have been presented, and always with
/// flip-discard, whose buffers don't keep their frames.
pub(crate) fn back_buffer_age(effect: u32, presents: u64) -> u32 {
    if effect == DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL && presents >= 2 {
        2
    } else {
        0
    }
}

/// `DXGI_PRESENT_PARAMETERS`
#[repr(C)]
pub(crate) struct PresentParameters {
    pub DirtyRectsCount: u32,
    pub pDirtyRects: *const D3dRect,
    pub pScrollRect: *const D3dRect,
    pub pScrollOffset: *const c_void,
}

/// A `w`×`h` flip-model swap chain of two BGRA8 buffers, stretched while
/// the window is resized ahead of its next frame, its alpha ignored (the
/// desktop doesn't blend a window's swap chain).
pub(crate) fn swap_chain_desc(w: u32, h: u32, effect: u32, flags: u32) -> SwapChainDesc1 {
    SwapChainDesc1 {
        Width: w,
        Height: h,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: 0,
        SampleDesc: DxgiSampleDesc {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: effect,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: flags,
    }
}

/// What an HRESULT from a frame means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It worked (including `DXGI_STATUS_OCCLUDED`: the window is hidden).
    Ok,
    /// The device was removed or reset: set it up again.
    Lost,
    /// Some other failure.
    Failed,
}

pub(crate) fn outcome(hr: Hresult) -> Outcome {
    match hr {
        S_OK | DXGI_STATUS_OCCLUDED => Outcome::Ok,
        h if h >= 0 => Outcome::Ok,
        DXGI_ERROR_DEVICE_REMOVED
        | DXGI_ERROR_DEVICE_HUNG
        | DXGI_ERROR_DEVICE_RESET
        | DXGI_ERROR_DRIVER_INTERNAL_ERROR => Outcome::Lost,
        _ => Outcome::Failed,
    }
}

/// The shaders' constants: one `cbuffer` of up to eight `float4`s, the first
/// always the vertex shader's device-to-clip mapping (`xf`). Each pixel
/// shader reads its own fields from the rest (see `scene.hlsl`).
pub(crate) type Constants = [[f32; 4]; 8];
/// The constant buffer's size in bytes.
pub(crate) const CONSTANTS_BYTES: usize = std::mem::size_of::<Constants>();

/// The batched quads' vertices (`crate::gpu::batch`): five `float4`s, the
/// semantics `QUAD0`..`QUAD4` of `quad_vs`.
pub(crate) const QUAD_STRIDE: u32 = 80;

/// The input layout of `quad_vs`: `QUAD0`..`QUAD4`, a `float4` each, one
/// after the other in one vertex buffer.
pub(crate) fn quad_elements() -> [InputElementDesc; 5] {
    std::array::from_fn(|i| InputElementDesc {
        SemanticName: c"QUAD".as_ptr() as *const u8,
        SemanticIndex: i as u32,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        InputSlot: 0,
        AlignedByteOffset: i as u32 * 16,
        InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
        InstanceDataStepRate: 0,
    })
}

/// `quad_ps` and `quadbox_ps`: `u1.x` whether the batch's texture is
/// sampled nearest.
pub(crate) fn quad_constants(xf: [f32; 4], nearest: bool) -> [[f32; 4]; 2] {
    [xf, [if nearest { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]]
}

/// `flat_ps`: `u1` the color.
pub(crate) fn flat_constants(xf: [f32; 4], color: [f32; 4]) -> [[f32; 4]; 2] {
    [xf, color]
}

/// `resolve_ps`: `u1` the color, `u2` the reciprocal scratch size and the
/// tap offset.
pub(crate) fn resolve_constants(
    xf: [f32; 4],
    color: [f32; 4],
    inv: [f32; 2],
    tap: f32,
) -> [[f32; 4]; 3] {
    [xf, color, [inv[0], inv[1], tap, 0.0]]
}

/// Device pixels of a `w`×`h` target to clip space (y up), as
/// `[scale.x, scale.y, offset.x, offset.y]`.
pub(crate) fn device_to_clip(w: i32, h: i32) -> [f32; 4] {
    [2.0 / w as f32, -2.0 / h as f32, -1.0, 1.0]
}

/// Premultiplied BGRA (what the targets hold) to straight RGBA (a `Pixmap`),
/// in place.
pub(crate) fn bgra_premul_to_rgba(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        let a = px[3];
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            }
        }
    }
}

/// Each polygon as a triangle fan from its first point, flattened to a
/// triangle list of `[x, y, 0, 0]` vertices.
pub(crate) fn fan_triangles<'a>(polys: impl Iterator<Item = &'a [[f32; 2]]>) -> Vec<f32> {
    let mut v = Vec::new();
    for p in polys {
        if p.len() < 3 {
            continue;
        }
        for i in 1..p.len() - 1 {
            for q in [p[0], p[i], p[i + 1]] {
                v.extend_from_slice(&[q[0], q[1], 0.0, 0.0]);
            }
        }
    }
    v
}

/// The pixel box `[x0, y0, x1, y1]` as a scissor rectangle.
pub(crate) fn scissor_rect(s: [i32; 4]) -> D3dRect {
    D3dRect {
        left: s[0],
        top: s[1],
        right: s[2],
        bottom: s[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// The descriptors match the sizes the Windows SDK headers give them
    /// (64-bit), so the driver reads every field where it expects it.
    #[test]
    fn descriptors_have_the_sdk_layouts() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(size_of::<Texture2dDesc>(), 44);
        assert_eq!(size_of::<BufferDesc>(), 24);
        assert_eq!(size_of::<RenderTargetBlendDesc>(), 32);
        assert_eq!(size_of::<BlendDesc>(), 264);
        assert_eq!(size_of::<StencilOpDesc>(), 16);
        assert_eq!(size_of::<DepthStencilDesc>(), 52);
        assert_eq!(std::mem::offset_of!(DepthStencilDesc, FrontFace), 20);
        assert_eq!(size_of::<RasterizerDesc>(), 40);
        assert_eq!(size_of::<SamplerDesc>(), 52);
        assert_eq!(size_of::<SwapChainDesc1>(), 48);
        assert_eq!(size_of::<Viewport>(), 24);
        assert_eq!(size_of::<D3dRect>(), 16);
        assert_eq!(size_of::<D3dBox>(), 24);
        if size_of::<usize>() == 8 {
            assert_eq!(size_of::<PresentParameters>(), 32);
        }
        if size_of::<usize>() == 8 {
            assert_eq!(size_of::<InputElementDesc>(), 32);
            assert_eq!(size_of::<SubresourceData>(), 16);
            assert_eq!(size_of::<MappedSubresource>(), 16);
        }
        assert_eq!(CONSTANTS_BYTES, 128);
        assert_eq!(
            CONSTANTS_BYTES % 16,
            0,
            "constant buffers are whole float4s"
        );
    }

    #[test]
    fn the_device_is_lost_only_on_removal_or_reset() {
        assert_eq!(outcome(S_OK), Outcome::Ok);
        assert_eq!(outcome(DXGI_STATUS_OCCLUDED), Outcome::Ok);
        assert_eq!(outcome(DXGI_ERROR_DEVICE_REMOVED), Outcome::Lost);
        assert_eq!(outcome(DXGI_ERROR_DEVICE_RESET), Outcome::Lost);
        assert_eq!(outcome(DXGI_ERROR_DEVICE_HUNG), Outcome::Lost);
        assert_eq!(outcome(0x8007_0057_u32 as i32), Outcome::Failed); // E_INVALIDARG
    }

    #[test]
    fn swap_chains_are_flip_model_best_first() {
        let (effect, flags) = SWAP_CHAIN_LADDER[0];
        // Buffers that keep their frames come first, for partial presents.
        assert_eq!(effect, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL);
        assert_eq!(flags, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT);
        for (effect, _) in SWAP_CHAIN_LADDER {
            assert!(
                effect >= DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                "flip model only"
            );
        }
        assert_eq!(SWAP_CHAIN_LADDER.last().unwrap().1, 0);
        let d = swap_chain_desc(640, 480, effect, flags);
        assert_eq!((d.Width, d.Height, d.BufferCount), (640, 480, 2));
        // Flip-model swap chains can't be sRGB or multisampled.
        assert_eq!(d.Format, DXGI_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(d.SampleDesc.Count, 1);
        assert_eq!(d.AlphaMode, DXGI_ALPHA_MODE_IGNORE);
    }

    #[test]
    fn the_back_buffer_holds_the_frame_before_last() {
        assert_eq!(back_buffer_age(DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 0), 0);
        assert_eq!(back_buffer_age(DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 1), 0);
        assert_eq!(back_buffer_age(DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 2), 2);
        assert_eq!(back_buffer_age(DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 9), 2);
        assert_eq!(back_buffer_age(DXGI_SWAP_EFFECT_FLIP_DISCARD, 9), 0);
        assert_eq!(swap_chain_desc(1, 1, 0, 0).BufferCount, 2, "two buffers");
    }

    #[test]
    fn constants_are_laid_out_as_the_shaders_read_them() {
        let xf = device_to_clip(200, 100);
        assert_eq!(xf, [0.01, -0.02, -1.0, 1.0]);
        let q = quad_constants(xf, true);
        assert_eq!(q, [xf, [1.0, 0.0, 0.0, 0.0]]);
        assert_eq!(quad_constants(xf, false)[1][0], 0.0);
        let r = resolve_constants(xf, [1.0; 4], [0.25, 0.5], 1.0);
        assert_eq!(r[2], [0.25, 0.5, 1.0, 0.0]);
        assert_eq!(flat_constants(xf, [0.0; 4])[1], [0.0; 4]);
    }

    #[test]
    fn the_quad_layout_is_the_batch_vertex_format() {
        use crate::gpu::batch::QUAD_FLOATS;
        assert_eq!(QUAD_STRIDE as usize, QUAD_FLOATS * 4);
        let e = quad_elements();
        for (i, e) in e.iter().enumerate() {
            assert_eq!(e.SemanticIndex, i as u32);
            assert_eq!(e.AlignedByteOffset, i as u32 * 16);
            assert_eq!(e.Format, DXGI_FORMAT_R32G32B32A32_FLOAT);
        }
        assert!(e[4].AlignedByteOffset + 16 <= QUAD_STRIDE);
        assert_eq!(
            D3dBox::pixels([1, 2, 5, 9]),
            D3dBox {
                left: 1,
                top: 2,
                front: 0,
                right: 5,
                bottom: 9,
                back: 1
            }
        );
    }

    #[test]
    fn read_back_pixels_are_unpremultiplied_rgba() {
        // Opaque blue, half-transparent premultiplied red, clear.
        let mut px = vec![255, 0, 0, 255, 0, 0, 64, 128, 9, 9, 9, 0];
        bgra_premul_to_rgba(&mut px);
        assert_eq!(&px[0..4], &[0, 0, 255, 255]);
        assert_eq!(&px[4..8], &[128, 0, 0, 128]);
        assert_eq!(&px[8..12], &[9, 9, 9, 0]);
    }

    #[test]
    fn polygons_become_fans() {
        let square = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let v = fan_triangles(std::iter::once(&square[..]));
        assert_eq!(v.len(), 2 * 3 * 4);
        let line = [[0.0, 0.0], [1.0, 0.0]];
        assert!(fan_triangles(std::iter::once(&line[..])).is_empty());
        assert_eq!(
            scissor_rect([1, 2, 3, 4]),
            D3dRect {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4
            }
        );
    }
}
