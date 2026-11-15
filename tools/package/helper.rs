//! The packaging tool's host helper, built by `cargo-stipple` in a scratch
//! crate against the app's own Stipple: it draws the app icons with Stipple's
//! image code (any format `Pixmap::decode` reads, and SVG drawn at each size),
//! and writes the `classes.dex` an Android package carries.
//!
//!     helper dex <lib_name> <out.dex>
//!     helper icons <jobs.tsv>
//!
//! Each line of the jobs file is one PNG to write, tab-separated:
//! `out size scale background source`, where the source image is drawn
//! centered over `scale` (a fraction) of a `size`×`size` square, over
//! `background` (`rrggbbaa`, or `-` for transparent).

use std::collections::HashMap;
use std::process::ExitCode;

use stipple::geometry::PhysicalSize;
use stipple::render::{ImageFit, Pixmap, Svg};

enum Source {
    Svg(Svg),
    Raster(Pixmap),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["dex", lib, out] => std::fs::write(out, stipple::platform::android_receiver_dex(lib))
            .map_err(|e| format!("{out}: {e}")),
        ["icons", jobs] => icons(jobs),
        _ => Err("usage: helper dex <lib> <out> | helper icons <jobs.tsv>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stipple-package helper: {e}");
            ExitCode::FAILURE
        }
    }
}

fn icons(jobs: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(jobs).map_err(|e| format!("{jobs}: {e}"))?;
    let mut sources: HashMap<String, Source> = HashMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let [out, size, scale, background, source] = f[..] else {
            return Err(format!("bad job line: {line:?}"));
        };
        let size: u32 = size.parse().map_err(|_| format!("bad size in {line:?}"))?;
        let scale: f64 = scale
            .parse()
            .map_err(|_| format!("bad scale in {line:?}"))?;
        let background = parse_rgba(background)?;
        if !sources.contains_key(source) {
            sources.insert(source.to_string(), load(source)?);
        }
        let inner = ((size as f64 * scale).round() as u32).clamp(1, size);
        let image = match &sources[source] {
            Source::Svg(svg) => svg.rasterize(PhysicalSize::new(inner, inner), ImageFit::Contain),
            Source::Raster(pm) => contain(pm, inner),
        };
        let offset = (size - inner) / 2;
        let canvas = compose(size, background, &image, offset);
        canvas.save_png(out).map_err(|e| format!("{out}: {e}"))?;
    }
    Ok(())
}

fn load(path: &str) -> Result<Source, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if let Ok(svg) = Svg::parse(&bytes) {
        return Ok(Source::Svg(svg));
    }
    let pm = Pixmap::decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let s = pm.size();
    if s.width < 1024 || s.height < 1024 {
        eprintln!(
            "stipple-package helper: warning: {path} is {}x{}; the App Store asks for 1024x1024",
            s.width, s.height
        );
    }
    Ok(Source::Raster(pm))
}

fn parse_rgba(s: &str) -> Result<Option<[u8; 4]>, String> {
    if s == "-" {
        return Ok(None);
    }
    let v = u32::from_str_radix(s, 16).map_err(|_| format!("bad colour {s:?}"))?;
    Ok(Some(v.to_be_bytes()))
}

/// `src` scaled to fit a `side`×`side` square (centered, transparent around),
/// each output pixel the area-weighted average of the source pixels it covers,
/// in premultiplied alpha (so transparent pixels don't darken edges).
fn contain(src: &Pixmap, side: u32) -> Pixmap {
    let (sw, sh) = (src.size().width as f64, src.size().height as f64);
    let k = (side as f64 / sw).min(side as f64 / sh);
    let (dw, dh) = (
        ((sw * k).round() as u32).max(1),
        ((sh * k).round() as u32).max(1),
    );
    let (ox, oy) = ((side - dw.min(side)) / 2, (side - dh.min(side)) / 2);
    let mut out = Pixmap::new(PhysicalSize::new(side, side));
    let data = src.as_bytes();
    let stride = src.stride();
    // The source span [a, b) an output pixel `i` covers along one axis.
    let span = |i: u32, n: u32, len: f64| {
        let a = i as f64 * len / n as f64;
        let b = ((i + 1) as f64 * len / n as f64).max(a + 1e-9);
        (a, b)
    };
    for y in 0..dh.min(side) {
        let (y0, y1) = span(y, dh, sh);
        for x in 0..dw.min(side) {
            let (x0, x1) = span(x, dw, sw);
            let mut acc = [0f64; 4];
            let mut area = 0f64;
            let mut sy = y0.floor();
            while sy < y1 {
                let wy = (y1.min(sy + 1.0) - y0.max(sy)).max(0.0);
                let row = (sy as usize).min(sh as usize - 1) * stride;
                let mut sx = x0.floor();
                while sx < x1 {
                    let wx = (x1.min(sx + 1.0) - x0.max(sx)).max(0.0);
                    let w = wx * wy;
                    let p = row + (sx as usize).min(sw as usize - 1) * 4;
                    let a = data[p + 3] as f64 / 255.0;
                    for c in 0..3 {
                        acc[c] += data[p + c] as f64 * a * w;
                    }
                    acc[3] += a * w;
                    area += w;
                    sx += 1.0;
                }
                sy += 1.0;
            }
            let a = acc[3] / area;
            let px = if a <= 0.0 {
                [0, 0, 0, 0]
            } else {
                let c = |v: f64| (v / area / a).round().clamp(0.0, 255.0) as u8;
                [c(acc[0]), c(acc[1]), c(acc[2]), (a * 255.0).round() as u8]
            };
            let o = ((y + oy) * side + x + ox) as usize * 4;
            out.as_bytes_mut()[o..o + 4].copy_from_slice(&px);
        }
    }
    out
}

/// `image` over `background` (or transparency) on a `size`×`size` canvas, at
/// `offset` from the top left.
fn compose(size: u32, background: Option<[u8; 4]>, image: &Pixmap, offset: u32) -> Pixmap {
    let mut canvas = Pixmap::new(PhysicalSize::new(size, size));
    if let Some(bg) = background {
        for px in canvas.as_bytes_mut().as_chunks_mut::<4>().0 {
            *px = bg;
        }
    }
    let iw = image.size().width;
    let src = image.as_bytes();
    let dst = canvas.as_bytes_mut();
    for y in 0..image.size().height.min(size - offset) {
        for x in 0..iw.min(size - offset) {
            let s = ((y * iw + x) * 4) as usize;
            let d = (((y + offset) * size + x + offset) * 4) as usize;
            let sa = src[s + 3] as f32 / 255.0;
            let da = dst[d + 3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let v = (src[s + c] as f32 * sa + dst[d + c] as f32 * da * (1.0 - sa)) / oa;
                dst[d + c] = v.round().clamp(0.0, 255.0) as u8;
            }
            dst[d + 3] = (oa * 255.0).round() as u8;
        }
    }
    canvas
}
