//! An image's `<img>` tag: what the HTML export writes (and the Markdown
//! export, for an image `![alt](src)` can't describe), and reading one back.
//!
//! Floats are CSS floats; a centered image is `display: block; margin: 0
//! auto`; a free one carries its position in `data-stipple-x` and
//! `data-stipple-y` (percentages of the text area's width, see
//! [`ImagePlacement::Free`]) with a CSS approximation of its horizontal
//! place (`margin-left` at its center, `translateX(-50%)`). Widths are CSS
//! `width` (`px` or `%`), `min-width` and `max-width` (the natural width when
//! the image may not grow past it); `data-stipple-wrap` and
//! `data-stipple-upscale` keep the rest.

use std::sync::Arc;

use super::html::{HtmlTag, escape};
use super::model::{ImagePlacement, ImageSize, ImageWrap, RichImage, load_source};

/// `v` as the exports write numbers: at most two decimals, no trailing
/// zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0" } else { s }.to_string()
}

/// Whether Markdown's `![alt](src)` says all there is about `image`.
pub(crate) fn is_plain(image: &RichImage) -> bool {
    image.placement == ImagePlacement::Inline
        && image.size == ImageSize::Natural
        && image.wrap == ImageWrap::Both
        && image.min_width.is_none()
        && image.max_width.is_none()
        && !image.upscale
}

/// Write `image` as an `<img>` tag.
pub(crate) fn write_img(image: &RichImage, out: &mut String) {
    out.push_str("<img src=\"");
    escape(&image.src, true, out);
    out.push_str("\" alt=\"");
    escape(&image.alt, true, out);
    out.push('"');
    if let ImagePlacement::Free { x, y } = image.placement {
        out.push_str(&format!(
            " data-stipple-x=\"{}%\" data-stipple-y=\"{}%\"",
            num(x),
            num(y)
        ));
    }
    match image.wrap {
        ImageWrap::Both => {}
        ImageWrap::Largest => out.push_str(" data-stipple-wrap=\"largest\""),
        ImageWrap::None => out.push_str(" data-stipple-wrap=\"none\""),
    }
    if image.upscale {
        out.push_str(" data-stipple-upscale");
    }
    let mut css: Vec<String> = Vec::new();
    match image.placement {
        ImagePlacement::Inline => {}
        ImagePlacement::Left => css.push("float: left".into()),
        ImagePlacement::Right => css.push("float: right".into()),
        ImagePlacement::Center => {
            css.push("display: block".into());
            css.push("margin: 0 auto".into());
        }
        ImagePlacement::Free { x, .. } => {
            css.push("display: block".into());
            css.push(format!("margin-left: {}%", num(x)));
            css.push("transform: translateX(-50%)".into());
        }
    }
    match image.size {
        ImageSize::Natural => {}
        ImageSize::Pixels(px) => css.push(format!("width: {}px", num(px))),
        ImageSize::Percent(p) => css.push(format!("width: {}%", num(p))),
    }
    if let Some(min) = image.min_width {
        css.push(format!("min-width: {}px", num(min)));
    }
    // A width of its own is capped at the natural width, unless it may
    // grow; at its natural size, only its own most applies.
    let natural = image
        .natural_size()
        .map(|s| s.width)
        .filter(|_| !image.upscale && image.size != ImageSize::Natural);
    let cap = match (image.max_width, natural) {
        (Some(m), Some(n)) => Some(m.min(n)),
        (m, n) => m.or(n),
    };
    if let Some(cap) = cap {
        css.push(format!("max-width: {}px", num(cap)));
    }
    if !css.is_empty() {
        out.push_str(" style=\"");
        out.push_str(&css.join("; "));
        out.push('"');
    }
    out.push('>');
}

/// A length in pixels: `12px` or a bare number.
fn px(value: &str) -> Option<f64> {
    let v = value.trim();
    let v = v.strip_suffix("px").unwrap_or(v).trim();
    v.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
}

/// A percentage: `40%`.
fn percent(value: &str) -> Option<f64> {
    let v = value.trim().strip_suffix('%')?.trim();
    v.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Read an `<img>` tag (`decode`: decode a `data:` URI's image): what
/// [`write_img`] writes, and floats, centering, widths and the legacy
/// `align` and `width` attributes from elsewhere.
pub(crate) fn read_img(tag: &HtmlTag, decode: bool) -> RichImage {
    let src = tag.attr("src").unwrap_or_default().to_string();
    let pixmap = if decode {
        load_source(&src, false).map(Arc::new)
    } else {
        None
    };
    let alt = tag.attr("alt").unwrap_or_default();
    let mut image = RichImage::new(src, pixmap).alt(alt);
    // The style's declarations, the last of each winning.
    let decls: Vec<(String, String)> = tag
        .attr("style")
        .unwrap_or_default()
        .split(';')
        .filter_map(|d| {
            let (p, v) = d.split_once(':')?;
            Some((p.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase()))
        })
        .collect();
    let css = |name: &str| {
        decls
            .iter()
            .rev()
            .find(|(p, _)| p == name)
            .map(|(_, v)| v.as_str())
    };
    let align = tag.attr("align").map(str::to_ascii_lowercase);
    let auto = |v: Option<&str>| v.is_some_and(|v| v.trim() == "auto");
    let margin_auto = css("margin").is_some_and(|m| {
        let parts: Vec<&str> = m.split_whitespace().collect();
        matches!(parts.as_slice(), ["auto"] | [_, "auto"] | [_, "auto", _])
            || matches!(parts.as_slice(), [_, "auto", _, "auto"])
    }) || (auto(css("margin-left")) && auto(css("margin-right")));
    let placement = if let Some(x) = tag.attr("data-stipple-x").and_then(percent) {
        let y = tag
            .attr("data-stipple-y")
            .and_then(percent)
            .unwrap_or_default();
        ImagePlacement::Free { x, y }
    } else {
        match css("float").or(align.as_deref()) {
            Some("left") => ImagePlacement::Left,
            Some("right") => ImagePlacement::Right,
            Some("center" | "middle") => ImagePlacement::Center,
            _ if css("display") == Some("block") && margin_auto => ImagePlacement::Center,
            _ => ImagePlacement::Inline,
        }
    };
    image = image.placed(placement);
    image = image.wrapped(match tag.attr("data-stipple-wrap") {
        Some("largest") => ImageWrap::Largest,
        Some("none") => ImageWrap::None,
        _ => ImageWrap::Both,
    });
    image = image.upscale(tag.attr("data-stipple-upscale").is_some());
    let width = css("width").or(tag.attr("width"));
    let size = match width {
        Some(w) => match percent(w) {
            Some(p) => ImageSize::Percent(p),
            None => px(w).map_or(ImageSize::Natural, ImageSize::Pixels),
        },
        None => ImageSize::Natural,
    };
    image = image.sized(size);
    image = image.min_width(css("min-width").and_then(px));
    // A most that is the natural width is the cap the export writes, not
    // one of the image's own.
    let natural = image.natural_size().map(|s| s.width);
    let max = css("max-width")
        .and_then(px)
        .filter(|m| image.upscale || image.size == ImageSize::Natural || natural != Some(*m));
    image.max_width(max)
}
