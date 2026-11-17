//! The Vulkan scene executor (Linux, Android; feature `vk`).
//!
//! `record` turns a lowered plan into render passes and draws, `shaders`
//! holds the checked-in SPIR-V, and `alloc` hands out parts of device memory
//! blocks: those three are plain Rust, tested on every host. The executor
//! itself (`exec`, over the raw bindings in `sys`) is built only where
//! there's a Vulkan loader to talk to.

#![cfg_attr(
    not(all(
        any(target_os = "linux", target_os = "android", target_os = "macos"),
        feature = "vk"
    )),
    allow(dead_code)
)]

pub(crate) mod alloc;
pub(crate) mod record;
pub(crate) mod shaders;

#[cfg(all(
    any(target_os = "linux", target_os = "android", target_os = "macos"),
    feature = "vk"
))]
mod exec;
// The Android hardware buffer import's structs and chains (tested on every
// host).
#[cfg(any(
    test,
    all(
        any(target_os = "linux", target_os = "android", target_os = "macos"),
        feature = "vk"
    )
))]
mod ahb;
// Also built for the tests, which check its structs against the C headers.
#[cfg(any(
    test,
    all(
        any(target_os = "linux", target_os = "android", target_os = "macos"),
        feature = "vk"
    )
))]
mod sys;

#[cfg(all(
    any(target_os = "linux", target_os = "android", target_os = "macos"),
    feature = "vk"
))]
pub(crate) use exec::VkExecutor;
