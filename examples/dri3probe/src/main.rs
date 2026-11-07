//! Phase-B integration probe for the DRI3 + Present GPU-present path. Connects
//! to the X server over Stipple's raw X11 socket, negotiates DRI3, and receives
//! the server's DRM device fd via SCM_RIGHTS (B.2); then binds Stipple's GPU
//! context to *that* device via GBM and round-trips a dma-buf through it (B.3) —
//! proving the X server's own GPU can export and re-import the buffers the
//! compositor will hand it. Maps no window (DRI3Open targets the root).
//!
//! Run on a machine with a real GPU + X server (DRI3):
//!   cargo run -p dri3probe --features stipple::gpu/gl
//!
//! Expected on GPU+X: the DRM fd, then "device dma-buf round-trip: PASS".

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    {
        // The Present extension (which flips the DRI3 pixmap to the window) needs
        // no GPU, so this negotiation succeeds even under Xvfb — the CI-verifiable
        // half of the zero-copy present path.
        match stipple::platform::backend::x11::present_probe() {
            Ok(s) => println!("Present probe: {s}"),
            Err(e) => println!("Present probe error: {e}"),
        }

        let fd = match stipple::platform::backend::x11::dri3_open_drm_fd() {
            Ok(Some(fd)) => {
                println!("DRI3Open ok: DRM device fd = {fd}");
                fd
            }
            Ok(None) => {
                println!("DRI3 unavailable on this server (Xvfb has no DRM)");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("DRI3 probe error: {e}");
                return ExitCode::from(2);
            }
        };

        // Bind the server's GPU via GBM and round-trip a dma-buf on it.
        match stipple::gpu::dmabuf_self_test_on_device(fd) {
            Ok(px) => {
                println!("device dma-buf round-trip: PASS ({} bytes)", px.len());
                // Compose the full zero-copy present on real hardware: export a
                // frame as a dma-buf on the server's GPU, wrap it as an X pixmap
                // (DRI3 PixmapFromBuffers, fd over the socket), and flip it to a
                // window (Present PresentPixmap) — no readback.
                present_exported_dmabuf(fd);
                ExitCode::SUCCESS
            }
            Err(e) => {
                // Without the gl feature this returns an explanatory error.
                println!("device dma-buf round-trip: {e}");
                ExitCode::from(1)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("dri3probe: Linux/X11 only");
        ExitCode::SUCCESS
    }
}

/// On real GPU hardware, compose the full zero-copy present: export a frame as a
/// dma-buf on the server's GPU (`drm_fd` from `DRI3Open`), then wrap it as an X
/// pixmap and flip it to a window (DRI3 `PixmapFromBuffers` + Present
/// `PresentPixmap`, no readback). Best-effort: prints the outcome but never fails
/// the probe — without the `gl` feature, or off real hardware, export reports
/// unsupported.
#[cfg(target_os = "linux")]
fn present_exported_dmabuf(drm_fd: i32) {
    use stipple::platform::backend::x11::DmabufImage;

    match stipple::gpu::export_dmabuf_on_device(drm_fd, 256, 256) {
        Ok(d) => {
            println!(
                "dmabuf export: {}x{} stride={} offset={} modifier={:#x} fourcc={:#x}",
                d.width, d.height, d.stride, d.offset, d.modifier, d.fourcc
            );
            let img = DmabufImage {
                width: d.width as u16,
                height: d.height as u16,
                depth: 24,
                bpp: d.bpp,
                modifier: d.modifier,
                planes: vec![(d.stride, d.offset)],
            };
            match stipple::platform::backend::x11::dri3_present_dmabuf_self_test(&img, &[d.fd]) {
                Ok(s) => println!("on-window present: {s}"),
                Err(e) => println!("on-window present error: {e}"),
            }
        }
        Err(e) => println!("dmabuf export: unsupported ({e})"),
    }
}
