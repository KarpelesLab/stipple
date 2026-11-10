//! The clipboard's data formats, independent of any platform: what each
//! representation of a [`ClipboardContent`] is called and how its bytes look.
//!
//! - **MIME types** (X11 targets, Wayland, the web): text as UTF-8 under
//!   `text/plain;charset=utf-8`, `UTF8_STRING` and friends (`STRING` is
//!   Latin-1), HTML as `text/html` (UTF-8 written; UTF-16 with a byte-order
//!   mark accepted, as Firefox sends it), images as `image/png` (written) or any
//!   format [`Pixmap::decode`] reads.
//! - **`CF_HTML`** (Windows' registered `HTML Format`): a header of byte
//!   offsets, then a document whose fragment is marked by comments.
//! - **DIB** (Windows' `CF_DIB` / `CF_DIBV5`): a `BITMAPINFOHEADER` or
//!   `BITMAPV5HEADER` then the pixels, bottom-up BGRA rows.

// The MIME helpers serve the Linux backends.
#![cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]

use super::{ClipboardContent, ClipboardFormats};
use crate::geometry::PhysicalSize;
use crate::render::Pixmap;

/// The text types, written and read in this order of preference.
pub(crate) const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];
/// HTML's type.
pub(crate) const HTML_MIME: &str = "text/html";
/// The image type written.
pub(crate) const PNG_MIME: &str = "image/png";
/// The image types read, in this order of preference (all decode through
/// [`Pixmap::decode`]).
pub(crate) const IMAGE_MIMES: [&str; 6] = [
    "image/png",
    "image/bmp",
    "image/x-bmp",
    "image/jpeg",
    "image/gif",
    "image/webp",
];

/// Encode `pixmap` as PNG.
pub fn encode_png(pixmap: &Pixmap) -> Option<Vec<u8>> {
    let size = pixmap.size();
    oxideav_png::encode_rgba8(
        size.width,
        size.height,
        pixmap.as_bytes(),
        &oxideav_png::EncodeOptions::default(),
    )
    .ok()
}

/// Content ready to hand out under any of its types: the content itself and
/// its image encoded once as PNG.
#[derive(Clone, Debug, Default)]
pub(crate) struct Payload {
    pub(crate) content: ClipboardContent,
    pub(crate) png: Option<Vec<u8>>,
}

impl Payload {
    pub(crate) fn new(content: ClipboardContent) -> Payload {
        let png = content.image.as_ref().and_then(encode_png);
        Payload { content, png }
    }

    /// Whether there is nothing to offer.
    pub(crate) fn is_empty(&self) -> bool {
        self.content.text.is_none() && self.content.html.is_none() && self.png.is_none()
    }

    /// The MIME types this content can be read as.
    pub(crate) fn mimes(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.content.text.is_some() {
            out.extend(TEXT_MIMES);
        }
        if self.content.html.is_some() {
            out.push(HTML_MIME);
        }
        if self.png.is_some() {
            out.push(PNG_MIME);
        }
        out
    }

    /// The bytes for `mime`, if this content has that type.
    pub(crate) fn bytes(&self, mime: &str) -> Option<Vec<u8>> {
        let is = |m: &str| mime.eq_ignore_ascii_case(m);
        if is("STRING") {
            // Latin-1; characters beyond it become '?'.
            let text = self.content.text.as_ref()?;
            return Some(
                text.chars()
                    .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                    .collect(),
            );
        }
        if TEXT_MIMES.iter().any(|m| is(m)) {
            return self.content.text.clone().map(String::into_bytes);
        }
        if is(HTML_MIME) {
            return self.content.html.clone().map(String::into_bytes);
        }
        if is(PNG_MIME) {
            return self.png.clone();
        }
        None
    }
}

/// Which of `offered` to read for each of `formats`: the best text type, then
/// HTML, then the best image type.
pub(crate) fn pick<S: AsRef<str>>(offered: &[S], formats: ClipboardFormats) -> Vec<&'static str> {
    let has = |want: &str| {
        offered
            .iter()
            .any(|m| m.as_ref().eq_ignore_ascii_case(want))
    };
    let mut out = Vec::new();
    if formats.text
        && let Some(m) = TEXT_MIMES.into_iter().find(|m| has(m))
    {
        out.push(m);
    }
    if formats.html && has(HTML_MIME) {
        out.push(HTML_MIME);
    }
    if formats.image
        && let Some(m) = IMAGE_MIMES.into_iter().find(|m| has(m))
    {
        out.push(m);
    }
    out
}

/// Put `bytes`, read as `mime`, into `content`.
pub(crate) fn decode_into(content: &mut ClipboardContent, mime: &str, bytes: &[u8]) {
    let is = |m: &str| mime.eq_ignore_ascii_case(m);
    if is("STRING") {
        let text: String = bytes.iter().map(|&b| b as char).collect();
        content.text = Some(text.trim_end_matches('\0').to_string());
    } else if TEXT_MIMES.iter().any(|m| is(m)) {
        content.text = Some(utf8(bytes));
    } else if is(HTML_MIME) {
        content.html = Some(html_text(bytes));
    } else if IMAGE_MIMES.iter().any(|m| is(m)) {
        content.image = Pixmap::decode(bytes).ok();
    }
}

/// UTF-8 text, lossily, without the trailing NULs some sources add.
fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .to_string()
}

/// HTML bytes as text: UTF-8, or UTF-16 when they start with a byte-order mark
/// (Firefox writes `text/html` as UTF-16).
pub(crate) fn html_text(bytes: &[u8]) -> String {
    let utf16 = |le: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
            .trim_end_matches('\0')
            .to_string()
    };
    match bytes {
        [0xff, 0xfe, ..] => utf16(true),
        [0xfe, 0xff, ..] => utf16(false),
        [0xef, 0xbb, 0xbf, rest @ ..] => utf8(rest),
        _ => utf8(bytes),
    }
}

// ---- CF_HTML ----------------------------------------------------------------

const CF_HTML_PREFIX: &str = "<html>\r\n<body>\r\n<!--StartFragment-->";
const CF_HTML_SUFFIX: &str = "<!--EndFragment-->\r\n</body>\r\n</html>";

fn cf_html_header(start_html: usize, end_html: usize, start: usize, end: usize) -> String {
    format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
         StartFragment:{start:010}\r\nEndFragment:{end:010}\r\n"
    )
}

/// Wrap an HTML `fragment` in Windows' `CF_HTML` format (UTF-8, byte offsets
/// in a fixed-width header). Not NUL-terminated.
pub fn cf_html_encode(fragment: &str) -> Vec<u8> {
    let header_len = cf_html_header(0, 0, 0, 0).len();
    let start = header_len + CF_HTML_PREFIX.len();
    let end = start + fragment.len();
    let end_html = end + CF_HTML_SUFFIX.len();
    let mut out = cf_html_header(header_len, end_html, start, end).into_bytes();
    out.extend_from_slice(CF_HTML_PREFIX.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(CF_HTML_SUFFIX.as_bytes());
    out
}

/// The HTML fragment in `CF_HTML` data: between `StartFragment` and
/// `EndFragment`, else between `StartHTML` and `EndHTML`, else between the
/// fragment comments.
pub fn cf_html_decode(data: &[u8]) -> Option<String> {
    let data = match data.iter().position(|&b| b == 0) {
        Some(nul) => &data[..nul],
        None => data,
    };
    let field = |name: &str| -> Option<usize> {
        let mut at = 0;
        for line in data.split(|&b| b == b'\n') {
            at += line.len() + 1;
            if line.first() == Some(&b'<') || at > 4096 {
                break;
            }
            let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
            if let Some((key, value)) = line.split_once(':')
                && key.eq_ignore_ascii_case(name)
            {
                return value.trim().parse().ok();
            }
        }
        None
    };
    let range = |s: Option<usize>, e: Option<usize>| {
        let (s, e) = (s?, e?);
        (s <= e && e <= data.len()).then(|| String::from_utf8_lossy(&data[s..e]).into_owned())
    };
    if let Some(html) = range(field("StartFragment"), field("EndFragment")) {
        return Some(html);
    }
    if let Some(html) = range(field("StartHTML"), field("EndHTML")) {
        return Some(html);
    }
    let text = String::from_utf8_lossy(data);
    let s = text.find("<!--StartFragment-->")? + "<!--StartFragment-->".len();
    let e = s + text[s..].find("<!--EndFragment-->")?;
    Some(text[s..e].to_string())
}

// ---- DIB --------------------------------------------------------------------

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// `BITMAPV5HEADER`'s size.
const V5_SIZE: usize = 124;
/// `LCS_sRGB`.
const LCS_SRGB: u32 = 0x7352_4742;

/// Encode `pixmap` as a `CF_DIBV5`: a `BITMAPV5HEADER` with BGRA bit masks,
/// then bottom-up 32-bit rows (straight alpha).
pub fn dib_encode(pixmap: &Pixmap) -> Vec<u8> {
    let PhysicalSize { width, height } = pixmap.size();
    let mut out = Vec::with_capacity(V5_SIZE + (width * height * 4) as usize);
    let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    u32le(&mut out, V5_SIZE as u32);
    u32le(&mut out, width);
    u32le(&mut out, height); // positive: bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    u32le(&mut out, BI_BITFIELDS);
    u32le(&mut out, width * height * 4); // image size
    u32le(&mut out, 2835); // 72 dpi, in pixels per meter
    u32le(&mut out, 2835);
    u32le(&mut out, 0); // colors used
    u32le(&mut out, 0); // colors important
    u32le(&mut out, 0x00ff_0000); // red mask
    u32le(&mut out, 0x0000_ff00); // green
    u32le(&mut out, 0x0000_00ff); // blue
    u32le(&mut out, 0xff00_0000); // alpha
    u32le(&mut out, LCS_SRGB);
    out.resize(out.len() + 36, 0); // endpoints
    out.resize(out.len() + 12, 0); // gamma
    u32le(&mut out, 4); // intent: LCS_GM_IMAGES
    out.resize(out.len() + 12, 0); // profile data, size, reserved
    debug_assert_eq!(out.len(), V5_SIZE);
    let stride = width as usize * 4;
    let bytes = pixmap.as_bytes();
    for row in bytes.chunks_exact(stride.max(1)).rev() {
        for px in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

/// Decode a `CF_DIB` / `CF_DIBV5` (a bitmap info header, optional masks or
/// palette, then the pixels). 24- and 32-bit uncompressed and bit-field
/// bitmaps are read directly; anything else goes through the BMP decoder.
/// 32-bit pixels whose alpha is all zero (as `BI_RGB` bitmaps usually leave
/// it) are taken as opaque.
pub fn dib_decode(data: &[u8]) -> Option<Pixmap> {
    let u32at = |o: usize| {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let header = u32at(0)? as usize;
    if header < 40 || data.len() < header {
        return None;
    }
    let width = i32::try_from(u32at(4)?).ok()?;
    let height = u32at(8)? as i32;
    let bits = u16::from_le_bytes([data[14], data[15]]);
    let compression = u32at(16)?;
    let colors_used = u32at(32)? as usize;
    if width <= 0 || height == 0 || width > 1 << 15 || height.unsigned_abs() > 1 << 15 {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let top_down = height < 0;
    // Bit-field masks follow a 40-byte header; later headers hold them.
    let masks_after = if compression == BI_BITFIELDS && header == 40 {
        12
    } else {
        0
    };
    let palette = if bits <= 8 {
        if colors_used == 0 {
            1 << bits
        } else {
            colors_used
        }
    } else {
        colors_used
    } * 4;
    let pixels_at = header + masks_after + palette;
    let direct = matches!(
        (bits, compression),
        (24, BI_RGB) | (32, BI_RGB) | (32, BI_BITFIELDS)
    );
    if !direct {
        // Make it a BMP file and let the decoder deal with it.
        let mut file = Vec::with_capacity(14 + data.len());
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&((14 + data.len()) as u32).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&((14 + pixels_at) as u32).to_le_bytes());
        file.extend_from_slice(data);
        return Pixmap::decode(&file).ok();
    }
    let masks = if compression == BI_BITFIELDS {
        let at = 40;
        let alpha = if header >= 56 { u32at(52)? } else { 0 };
        [u32at(at)?, u32at(at + 4)?, u32at(at + 8)?, alpha]
    } else if bits == 32 {
        [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000]
    } else {
        [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0]
    };
    let channel = |px: u32, mask: u32| -> Option<u8> {
        if mask == 0 {
            return None;
        }
        let v = (px & mask) >> mask.trailing_zeros();
        let max = mask >> mask.trailing_zeros();
        Some(if max == 255 {
            v as u8
        } else {
            (v * 255 / max.max(1)) as u8
        })
    };
    let bpp = bits as usize / 8;
    let stride = (w * bpp).div_ceil(4) * 4;
    let pixels = data.get(pixels_at..pixels_at + stride * h)?;
    let mut rgba = Vec::with_capacity(w * h * 4);
    let mut any_alpha = false;
    for y in 0..h {
        let row = if top_down { y } else { h - 1 - y };
        let row = &pixels[row * stride..row * stride + w * bpp];
        for px in row.chunks_exact(bpp) {
            let v = if bpp == 4 {
                u32::from_le_bytes([px[0], px[1], px[2], px[3]])
            } else {
                u32::from_le_bytes([px[0], px[1], px[2], 0])
            };
            let a = channel(v, masks[3]);
            any_alpha |= a.is_some_and(|a| a != 0);
            rgba.extend_from_slice(&[
                channel(v, masks[0]).unwrap_or(0),
                channel(v, masks[1]).unwrap_or(0),
                channel(v, masks[2]).unwrap_or(0),
                a.unwrap_or(255),
            ]);
        }
    }
    if !any_alpha {
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
    Some(Pixmap::from_rgba8(
        PhysicalSize::new(w as u32, h as u32),
        rgba,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Pixmap {
        // 3x2, with distinct colors and alphas.
        let px: Vec<u8> = vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, //
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120,
        ];
        Pixmap::from_rgba8(PhysicalSize::new(3, 2), px)
    }

    #[test]
    fn cf_html_offsets_point_at_the_fragment() {
        let frag = "<b>bold</b> caf\u{e9}";
        let data = cf_html_encode(frag);
        let text = std::str::from_utf8(&data).unwrap();
        assert!(text.starts_with("Version:0.9\r\nStartHTML:"));
        let field = |name: &str| -> usize {
            let at = text.find(name).unwrap() + name.len() + 1;
            text[at..at + 10].parse().unwrap()
        };
        let (s, e) = (field("StartFragment"), field("EndFragment"));
        assert_eq!(&data[s..e], frag.as_bytes());
        assert_eq!(field("EndHTML"), data.len());
        assert!(text[field("StartHTML")..].starts_with("<html>"));
        assert_eq!(cf_html_decode(&data).as_deref(), Some(frag));
        // NUL-terminated, as it comes off the clipboard.
        let mut nul = data.clone();
        nul.extend_from_slice(b"\0\0garbage");
        assert_eq!(cf_html_decode(&nul).as_deref(), Some(frag));
    }

    #[test]
    fn cf_html_from_other_writers() {
        // Offsets of -1 for the optional StartHTML, and \n line ends.
        let body = "<html><body><!--StartFragment--><i>x</i><!--EndFragment--></body></html>";
        let head = "Version:1.0\nStartHTML:-1\nEndHTML:-1\nStartFragment:0000000000\nEndFragment:0000000000\n";
        let s = head.len() + body.find("<i>").unwrap();
        let e = head.len() + body.find("<!--End").unwrap();
        let head = head
            .replacen(
                "StartFragment:0000000000",
                &format!("StartFragment:{s:010}"),
                1,
            )
            .replacen("EndFragment:0000000000", &format!("EndFragment:{e:010}"), 1);
        let data = format!("{head}{body}");
        assert_eq!(cf_html_decode(data.as_bytes()).as_deref(), Some("<i>x</i>"));
        // Broken offsets: fall back to the markers.
        let broken = "Version:0.9\r\nStartFragment:999999\r\nEndFragment:9999999\r\n<html><!--StartFragment-->y<!--EndFragment-->";
        assert_eq!(cf_html_decode(broken.as_bytes()).as_deref(), Some("y"));
        assert_eq!(cf_html_decode(b"no html here"), None);
    }

    #[test]
    fn dib_round_trips_with_alpha() {
        let pm = checker();
        let dib = dib_encode(&pm);
        assert_eq!(dib.len(), 124 + 3 * 2 * 4);
        assert_eq!(&dib[0..4], &124u32.to_le_bytes());
        // Bottom-up: the first stored row is the image's last, in BGRA.
        assert_eq!(&dib[124..128], &[30, 20, 10, 40]);
        assert_eq!(dib_decode(&dib), Some(pm));
    }

    #[test]
    fn dib_reads_24_bit_and_opaque_32_bit() {
        // 2x2, 24-bit BI_RGB, bottom-up, rows padded to 8 bytes.
        let mut dib = Vec::new();
        for v in [40u32, 2, 2] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]); // compression .. colors important
        dib.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]); // bottom row (BGR)
        dib.extend_from_slice(&[7, 8, 9, 10, 11, 12, 0, 0]); // top row
        let pm = dib_decode(&dib).unwrap();
        assert_eq!(pm.size(), PhysicalSize::new(2, 2));
        assert_eq!(pm.pixel(0, 0), Some([9, 8, 7, 255]));
        assert_eq!(pm.pixel(1, 1), Some([6, 5, 4, 255]));

        // 1x1 top-down 32-bit BI_RGB with a zero alpha byte: opaque.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1u32.to_le_bytes());
        dib.extend_from_slice(&(-1i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[3, 2, 1, 0]);
        assert_eq!(dib_decode(&dib).unwrap().pixel(0, 0), Some([1, 2, 3, 255]));
        assert_eq!(dib_decode(&dib[..20]), None);
    }

    #[test]
    fn payload_offers_and_serves_each_type() {
        let content = ClipboardContent::text("caf\u{e9} \u{2713}")
            .with_html("<p>hi</p>")
            .with_image(checker());
        let p = Payload::new(content);
        let mimes = p.mimes();
        assert!(mimes.contains(&"UTF8_STRING") && mimes.contains(&HTML_MIME));
        assert!(mimes.contains(&PNG_MIME));
        assert_eq!(p.bytes("STRING").unwrap(), b"caf\xe9 ?");
        assert_eq!(
            p.bytes("text/plain;charset=utf-8").unwrap(),
            "caf\u{e9} \u{2713}".as_bytes()
        );
        assert_eq!(p.bytes("text/html").unwrap(), b"<p>hi</p>");
        assert_eq!(p.bytes("image/jpeg"), None);
        // The PNG decodes back to the image.
        let mut back = ClipboardContent::default();
        decode_into(&mut back, PNG_MIME, &p.bytes(PNG_MIME).unwrap());
        decode_into(&mut back, "STRING", b"caf\xe9\0");
        assert_eq!(back.image, Some(checker()));
        assert_eq!(back.text.as_deref(), Some("caf\u{e9}"));
    }

    #[test]
    fn picks_the_best_type_per_format() {
        let offered = [
            "TARGETS",
            "STRING",
            "UTF8_STRING",
            "text/html",
            "image/bmp",
            "image/png",
        ];
        assert_eq!(pick(&offered, ClipboardFormats::TEXT), vec!["UTF8_STRING"]);
        assert_eq!(
            pick(&offered, ClipboardFormats::ALL),
            vec!["UTF8_STRING", "text/html", "image/png"]
        );
        assert!(pick(&["image/tiff"], ClipboardFormats::ALL).is_empty());
    }

    #[test]
    fn html_may_be_utf16() {
        let mut bytes = vec![0xff, 0xfe];
        for u in "<b>\u{e9}</b>".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(html_text(&bytes), "<b>\u{e9}</b>");
        assert_eq!(html_text(b"\xef\xbb\xbf<i>"), "<i>");
    }
}
