//! A sample folder made from nothing: pictures (PNG through the toolkit's
//! encoder, BMP written by hand), sounds (WAV from the synthesizer), text and
//! Markdown in a few languages. The web build browses it in memory; the
//! selftest writes it to a temporary folder and browses that.

use std::time::Duration;
use stipple::audio::{Sound, Tone, Waveform, note_frequency};
use stipple::prelude::*;

/// 2026-11-17 00:00 UTC, the sample's newest date.
const NEWEST: i64 = 1_794_873_600;
const DAY: i64 = 86_400;

/// A picture of `w`×`h`, colors from `f(x, y)`.
fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Pixmap {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = f(x, y);
            data.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(w, h), data)
}

/// A 24-bit BMP file of `pixmap`, written by hand (bottom-up rows, padded
/// to four bytes).
pub fn bmp(pixmap: &Pixmap) -> Vec<u8> {
    let size = pixmap.size();
    let (w, h) = (size.width, size.height);
    let row = (w * 3).div_ceil(4) * 4;
    let image = row * h;
    let mut out = Vec::with_capacity(54 + image as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + image).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&image.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, _] = pixmap.pixel(x, y).unwrap_or([0; 4]);
            out.extend_from_slice(&[b, g, r]);
        }
        out.resize(out.len() + (row - w * 3) as usize, 0);
    }
    out
}

fn sunset() -> Pixmap {
    picture(320, 200, |x, y| {
        let sun = {
            let (dx, dy) = (x as f64 - 220.0, y as f64 - 120.0);
            (dx * dx + dy * dy).sqrt() < 34.0
        };
        if y > 140 {
            [20, 40 + (y - 140) as u8, 90 + (y - 140) as u8]
        } else if sun {
            [255, 220, 120]
        } else {
            [255, (90 + y / 2) as u8, (60 + y / 3) as u8]
        }
    })
}

fn checker() -> Pixmap {
    picture(96, 96, |x, y| {
        if (x / 12 + y / 12) % 2 == 0 {
            [40, 40, 48]
        } else {
            [230, 230, 220]
        }
    })
}

fn mountain(seed: u32) -> Pixmap {
    picture(240, 160, |x, y| {
        let ridge = 90.0 + 30.0 * ((x as f64 + seed as f64 * 13.0) / 23.0).sin()
            - 20.0 * ((x as f64) / 61.0).cos();
        if (y as f64) > ridge {
            [60 + (seed * 30 % 120) as u8, 110, 70]
        } else {
            [120, 170, 230 - (y / 2) as u8]
        }
    })
}

/// A small tile, one of many in a folder, so the grid has thumbnails to load
/// as it scrolls.
fn tile(i: u32) -> Pixmap {
    let hue = (i * 47) % 360;
    let (r, g, b) = hsv(hue as f64, 0.6, 0.9);
    picture(64, 64, move |x, y| {
        let ring = (((x as f64 - 32.0).hypot(y as f64 - 32.0) / 8.0) as u32).is_multiple_of(2);
        if ring {
            [r, g, b]
        } else {
            [r / 2, g / 2, b / 2]
        }
    })
}

fn hsv(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |c: f64| ((c + m) * 255.0).round() as u8;
    (to(r), to(g), to(b))
}

/// A chime: three notes, 1.5 s, mono WAV.
pub fn chime() -> Vec<u8> {
    let rate = 22_050;
    let ms = Duration::from_millis;
    let notes = [76, 79, 84].map(|n| {
        Tone::new(Waveform::Sine, note_frequency(n), ms(500))
            .volume(0.4)
            .release(ms(300))
            .render(rate)
    });
    Sound::concat(&notes).to_wav()
}

/// A C major scale, 2.4 s, mono WAV.
fn scale() -> Vec<u8> {
    let rate = 22_050;
    let ms = Duration::from_millis;
    let notes: Vec<Sound> = [60, 62, 64, 65, 67, 69, 71, 72]
        .iter()
        .map(|&n| {
            Tone::new(Waveform::Triangle, note_frequency(n), ms(300))
                .volume(0.35)
                .release(ms(80))
                .render(rate)
        })
        .collect();
    Sound::concat(&notes).to_wav()
}

const README: &str = "# Media browser sample\n\nThis folder was **generated** by the \
    `mediabrowser` example: pictures, sounds and documents to browse.\n\n## What to try\n\n\
    - Sort the list by *name*, *size*, *kind* or *modified*\n- Switch to the grid and scroll \
    through `Pictures/Tiles`\n- Play `Music/chime.wav`\n- Change the language in **View**\n\n\
    > Everything is drawn by Stipple and decoded by oxideav.\n";

const NOTES: &str = "Shopping list\n- bread\n- cheese\n- apples\n\nCall the plumber on Tuesday.\n";
const NOTES_FR: &str =
    "Liste de courses\n- pain\n- fromage\n- pommes\n\nAppeler le plombier mardi.\n";
const NOTES_AR: &str = "قائمة التسوق\n- خبز\n- جبن\n- تفاح\n\nالاتصال بالسباك يوم الثلاثاء.\n";
const NOTES_JA: &str = "買い物リスト\n- パン\n- チーズ\n- りんご\n\n火曜日に配管工に電話する。\n";
const CSV: &str = "month,visitors,sales\nJanuary,1200,340\nFebruary,1350,365\nMarch,1610,402\n";

/// Every file of the sample: its path, bytes and modification time.
pub fn files() -> Vec<(String, Vec<u8>, i64)> {
    let mut out: Vec<(String, Vec<u8>, i64)> = vec![
        ("README.md".into(), README.as_bytes().to_vec(), NEWEST),
        (
            "Pictures/sunset.png".into(),
            sunset().encode_png(),
            NEWEST - 3 * DAY,
        ),
        (
            "Pictures/checker.bmp".into(),
            bmp(&checker()),
            NEWEST - 40 * DAY,
        ),
        (
            "Pictures/Holidays/mountain.png".into(),
            mountain(1).encode_png(),
            NEWEST - 200 * DAY,
        ),
        (
            "Pictures/Holidays/valley.png".into(),
            mountain(4).encode_png(),
            NEWEST - 199 * DAY,
        ),
        ("Music/chime.wav".into(), chime(), NEWEST - 10 * DAY),
        ("Music/scale.wav".into(), scale(), NEWEST - 12 * DAY),
        (
            "Documents/notes.txt".into(),
            NOTES.as_bytes().to_vec(),
            NEWEST - DAY,
        ),
        (
            "Documents/notes-fr.txt".into(),
            NOTES_FR.as_bytes().to_vec(),
            NEWEST - 2 * DAY,
        ),
        (
            "Documents/notes-ar.txt".into(),
            NOTES_AR.as_bytes().to_vec(),
            NEWEST - 2 * DAY,
        ),
        (
            "Documents/notes-ja.txt".into(),
            NOTES_JA.as_bytes().to_vec(),
            NEWEST - 2 * DAY,
        ),
        (
            "Documents/Reports/2026.csv".into(),
            CSV.as_bytes().to_vec(),
            NEWEST - 30 * DAY,
        ),
    ];
    for i in 0..48 {
        out.push((
            format!("Pictures/Tiles/tile-{i:02}.png"),
            tile(i).encode_png(),
            NEWEST - i64::from(i) * 3600,
        ));
    }
    out
}

/// Write the sample under `dir`, with its modification times.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_to(dir: &std::path::Path) -> std::io::Result<()> {
    for (path, bytes, modified) in files() {
        let target = path.split('/').fold(dir.to_path_buf(), |p, c| p.join(c));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, bytes)?;
        let time = std::time::UNIX_EPOCH + Duration::from_secs(modified as u64);
        std::fs::File::options()
            .write(true)
            .open(&target)?
            .set_modified(time)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_decodes_with_the_toolkits_decoders() {
        let files = files();
        let get = |p: &str| &files.iter().find(|f| f.0 == p).unwrap().1;
        let png = Pixmap::decode(get("Pictures/sunset.png")).unwrap();
        assert_eq!(png.size(), PhysicalSize::new(320, 200));
        let bmp = Pixmap::decode(get("Pictures/checker.bmp")).unwrap();
        assert_eq!(bmp.size(), PhysicalSize::new(96, 96));
        assert_eq!(bmp.pixel(0, 0), Some([40, 40, 48, 255]));
        assert_eq!(bmp.pixel(12, 0), Some([230, 230, 220, 255]));
        let wav = Sound::decode(get("Music/chime.wav")).unwrap();
        assert!((wav.duration().as_secs_f64() - 1.5).abs() < 0.05);
    }
}
