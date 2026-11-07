//! Exercises the raw Metal backend in `stipple::gpu` and prints what it created.
//! The macOS visual CI job builds this with `--features stipple::gpu/mtl` and runs
//! it, grepping the output to confirm a Metal device came up.
//!
//! Without the `mtl` feature (or off macOS) the entry point returns an error,
//! which is printed non-fatally so the demo still exits cleanly.

const W: u32 = 420;
const H: u32 = 300;

fn main() {
    match stipple::gpu::metal_device() {
        Ok(name) => println!("Metal device: {name}"),
        Err(e) => println!("Metal unavailable: {e}"),
    }
    // Render a frame on the GPU and dump the read-back RGBA. We print the
    // top-left pixel so CI can confirm the cleared color tool-free (the macOS
    // runner has no guaranteed image tooling), and still write the raw buffer as
    // an artifact.
    match stipple::gpu::metal_render_clear(W, H) {
        Ok(pixels) => {
            std::fs::write("metal-clear.raw", &pixels).expect("write raw");
            let px = &pixels[0..4];
            println!(
                "Metal readback: {} bytes ({W}x{H}) first pixel {},{},{},{}",
                pixels.len(),
                px[0],
                px[1],
                px[2],
                px[3]
            );
        }
        Err(e) => println!("Metal readback unavailable: {e}"),
    }
    // The full Metal pipeline: a triangle drawn by a compiled .metal shader. The
    // center pixel must come back stipple green; we print it so CI can check it.
    match stipple::gpu::metal_render_triangle(W, H) {
        Ok(pixels) => {
            std::fs::write("metal-tri.raw", &pixels).expect("write raw");
            let i = ((H / 2) as usize * W as usize + (W / 2) as usize) * 4;
            let px = &pixels[i..i + 4];
            println!(
                "Metal triangle: {} bytes ({W}x{H}) center pixel {},{},{},{}",
                pixels.len(),
                px[0],
                px[1],
                px[2],
                px[3]
            );
        }
        Err(e) => println!("Metal triangle unavailable: {e}"),
    }
    // Export a shareable IOSurface — the macOS analog of a dma-buf for the
    // compositor's content path. Print its global id (or the failing call).
    match stipple::gpu::metal_export_iosurface(W, H) {
        Ok(summary) => println!("Metal IOSurface: {summary}"),
        Err(e) => println!("Metal IOSurface unavailable: {e}"),
    }
}
