//! Exercises the raw Direct3D 11 backend in `stipple::gpu` and prints what it
//! created. The Windows visual CI job builds this with `--features
//! stipple::gpu/d3d` and runs it against **WARP** (Windows' software rasterizer),
//! grepping the output to confirm the D3D device came up.
//!
//! Without the `d3d` feature (or off Windows) the entry point returns an error,
//! which is printed non-fatally so the demo still exits cleanly.

const W: u32 = 420;
const H: u32 = 300;

fn main() {
    match stipple::gpu::d3d11_device() {
        Ok(summary) => println!("D3D11 device: {summary}"),
        Err(e) => println!("D3D11 unavailable: {e}"),
    }
    // Render a frame on the GPU (WARP) and read it back. We print the top-left
    // pixel so CI can confirm the cleared color tool-free, and write the raw
    // buffer as an artifact.
    match stipple::gpu::d3d11_render_clear(W, H) {
        Ok(pixels) => {
            std::fs::write("d3d-clear.raw", &pixels).expect("write raw");
            let px = &pixels[0..4];
            println!(
                "D3D11 readback: {} bytes ({W}x{H}) first pixel {},{},{},{}",
                pixels.len(),
                px[0],
                px[1],
                px[2],
                px[3]
            );
        }
        Err(e) => println!("D3D11 readback unavailable: {e}"),
    }
    // The full D3D pipeline: a triangle drawn by HLSL shaders. The center pixel
    // must come back stipple green; print it so CI can check it.
    match stipple::gpu::d3d11_render_triangle(W, H) {
        Ok(pixels) => {
            std::fs::write("d3d-tri.raw", &pixels).expect("write raw");
            let i = ((H / 2) as usize * W as usize + (W / 2) as usize) * 4;
            let px = &pixels[i..i + 4];
            println!(
                "D3D11 triangle: {} bytes ({W}x{H}) center pixel {},{},{},{}",
                pixels.len(),
                px[0],
                px[1],
                px[2],
                px[3]
            );
        }
        Err(e) => println!("D3D11 triangle unavailable: {e}"),
    }
    // Export a shareable texture's cross-process handle — the Windows analog of a
    // dma-buf for the compositor's content path. WARP (software) may decline
    // shared resources, so this is best-effort: print the outcome either way.
    match stipple::gpu::d3d11_export_shared_handle(W, H) {
        Ok(summary) => println!("D3D11 shared handle: {summary}"),
        Err(e) => println!("D3D11 shared handle unavailable: {e}"),
    }
}
