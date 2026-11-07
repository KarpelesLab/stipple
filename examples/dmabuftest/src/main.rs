//! Phase-A spike for the browser content path: prove that a GPU texture can be
//! **exported as a `dma-buf`** and **re-imported** as a texture — the zero-copy
//! handoff a web content process will use to hand rendered pages to the Stipple UI
//! process. Runs surfaceless (no window), so it's safe to run on a GPU box.
//!
//! Run on real GPU hardware:
//!   cargo run -p dmabuftest --features stipple::gpu/gl
//!
//! Exit codes (so CI can tell a real failure from "no GPU here"):
//!   0 = self-test passed (dma-buf export+import works)
//!   2 = unsupported on this device (software Mesa / missing extensions / no EGL)
//!   1 = the extensions are present but the round-trip produced wrong pixels (bug)

use std::process::ExitCode;

/// Open a DRM device read/write, returning its raw fd (leaked for the run).
#[cfg(target_os = "linux")]
fn open_drm(path: &str) -> std::io::Result<i32> {
    use std::os::fd::IntoRawFd;
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .into_raw_fd())
}

/// The dma-buf spike depends on Linux-only EGL/GBM + `std::os::fd`; on other
/// targets it can't run, so report unsupported (exit 2) and keep the workspace
/// building everywhere (the Windows/macOS `--all-targets` build compiles it).
#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    println!("RESULT: UNSUPPORTED (dma-buf spike is Linux-only)");
    ExitCode::from(2)
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    // 1) What EGL extensions does this device advertise?
    let exts = match stipple::gpu::dmabuf_extensions() {
        Ok(s) => s,
        Err(e) => {
            println!("EGL unavailable: {e}");
            println!("RESULT: UNSUPPORTED (no EGL)");
            return ExitCode::from(2);
        }
    };
    let has_export = exts.contains("EGL_MESA_image_dma_buf_export");
    let has_import = exts.contains("EGL_EXT_image_dma_buf_import");
    println!("EGL_MESA_image_dma_buf_export: {has_export}");
    println!("EGL_EXT_image_dma_buf_import: {has_import}");
    if !(has_export && has_import) {
        println!("RESULT: UNSUPPORTED (dma-buf import/export extensions absent)");
        return ExitCode::from(2);
    }

    // Optional: bind to a specific GPU by DRM device path (e.g. a render node
    // /dev/dri/renderD128), the way the compositor will bind to the X server's
    // device from DRI3Open. Without it, EGL picks the device (surfaceless).
    let device = std::env::args().nth(1);
    if let Some(path) = device.as_deref() {
        println!("binding GPU via GBM device: {path}");
        let fd = match open_drm(path) {
            Ok(fd) => fd,
            Err(e) => {
                println!("open {path}: {e}");
                println!("RESULT: UNSUPPORTED (cannot open device)");
                return ExitCode::from(2);
            }
        };
        return match stipple::gpu::dmabuf_self_test_on_device(fd) {
            Ok(pixels) => {
                println!("imported {} bytes on device; corners present", pixels.len());
                // Also exercise the export-only descriptor path — the exact data
                // fed to DRI3 PixmapFromBuffers for a zero-copy present. Best
                // effort: a failure here doesn't fail the round-trip probe.
                match stipple::gpu::export_dmabuf_on_device(fd, 256, 256) {
                    Ok(d) => println!(
                        "dmabuf export: fd={} {}x{} stride={} offset={} \
                         modifier={:#x} fourcc={:#x} bpp={}",
                        d.fd, d.width, d.height, d.stride, d.offset, d.modifier, d.fourcc, d.bpp
                    ),
                    Err(e) => println!("dmabuf export: unsupported ({e})"),
                }
                println!("RESULT: PASS");
                ExitCode::SUCCESS
            }
            Err(e) => {
                println!("on-device self-test error: {e}");
                println!("RESULT: FAIL");
                ExitCode::from(1)
            }
        };
    }

    // 2) Export a GPU texture as a dma-buf and re-import it; verify the pixels.
    match stipple::gpu::dmabuf_export_import_self_test() {
        Ok(pixels) => {
            println!(
                "imported {} bytes; corners present (red/green/blue/white)",
                pixels.len()
            );
            println!("RESULT: PASS");
            ExitCode::SUCCESS
        }
        Err(e) => {
            // Some software stacks advertise the extensions but can't actually
            // export/import; treat that as unsupported rather than a code bug.
            let unsupported = e.contains("missing entry point")
                || e.contains("failed")
                || e.contains("FBO incomplete");
            println!("self-test error: {e}");
            if unsupported {
                println!("RESULT: UNSUPPORTED (export/import not functional here)");
                ExitCode::from(2)
            } else {
                println!("RESULT: FAIL (pixels did not survive the round-trip)");
                ExitCode::from(1)
            }
        }
    }
}
