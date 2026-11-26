//! `AXAttributedStringForRange`: a text's runs as an attributed string,
//! as VoiceOver reads text attributes ("bold", "link", a language switch).
//!
//! Each element answers `accessibilityAttributedStringForRange:` (the
//! NSAccessibility protocol's method AppKit serves the parameterized
//! attribute with) from its node's runs, which the tree builds only now
//! (see `platform::access::runs`); what each run carries is worked out in
//! [`ax_plan::attributed_runs`](super::ax_plan::attributed_runs).

use std::ffi::c_void;

use super::ax_plan::{self, AttrRun};
use super::{Id, Sel, class, msg_id, nsstring, objc_msgSend, sel};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> *mut c_void;
    fn CGColorRelease(color: *mut c_void);
}

/// `NSRange`.
#[repr(C)]
#[derive(Clone, Copy)]
struct NsRange {
    location: u64,
    length: u64,
}

/// `NSUnderlineStyleSingle`.
const UNDERLINE_SINGLE: i64 = 1;

/// The method, as `add_common` registers it on every element class.
pub(super) const METHOD: (&str, *const c_void, &std::ffi::CStr) = (
    "accessibilityAttributedStringForRange:",
    attributed_string_for_range as *const c_void,
    c"@@:{_NSRange=QQ}",
);

/// An `NSString` of `s` (without NULs, which `nsstring` can't take).
fn ns(s: &str) -> Id {
    if s.contains('\0') {
        nsstring(&s.replace('\0', ""))
    } else {
        nsstring(s)
    }
}

unsafe fn number_f64(v: f64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, f64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(class("NSNumber"), sel("numberWithDouble:"), v)
    }
}

unsafe fn number_i64(v: i64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, i64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(class("NSNumber"), sel("numberWithInteger:"), v)
    }
}

/// `[dict setObject:value forKey:key]`.
unsafe fn put(dict: Id, key: &str, value: Id) {
    if value.is_null() {
        return;
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(dict, sel("setObject:forKey:"), value, nsstring(key));
    }
}

/// `[string addAttribute:key value:value range:range]`.
unsafe fn add(string: Id, key: &str, value: Id, range: NsRange) {
    if value.is_null() {
        return;
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id, NsRange) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(
            string,
            sel("addAttribute:value:range:"),
            nsstring(key),
            value,
            range,
        );
    }
}

/// Add color `c` (`0..=1` components, sRGB) as attribute `key`.
unsafe fn add_color(string: Id, key: &str, c: [f64; 4], range: NsRange) {
    unsafe {
        let color = CGColorCreateSRGB(c[0], c[1], c[2], c[3]);
        if color.is_null() {
            return;
        }
        // The string retains it; ours goes.
        add(string, key, color as Id, range);
        CGColorRelease(color);
    }
}

/// Put run `r`'s attributes on `string`.
unsafe fn apply(string: Id, r: &AttrRun) {
    let range = NsRange {
        location: r.range.0 as u64,
        length: r.range.1 as u64,
    };
    unsafe {
        let font = msg_id(class("NSMutableDictionary"), sel("dictionary"));
        put(font, "AXFontName", ns(&r.font_name));
        if !r.family.is_empty() {
            put(font, "AXFontFamily", ns(&r.family));
            put(font, "AXVisibleName", ns(&r.visible_name));
        }
        put(font, "AXFontSize", number_f64(r.size));
        add(string, "AXFont", font, range);
        if let Some(c) = r.foreground {
            add_color(string, "AXForegroundColor", c, range);
        }
        if let Some(c) = r.background {
            add_color(string, "AXBackgroundColor", c, range);
        }
        if r.underline {
            add(string, "AXUnderline", number_i64(UNDERLINE_SINGLE), range);
        }
        if r.strikethrough {
            add(string, "AXStrikethrough", number_i64(1), range);
        }
        if let Some(link) = &r.link {
            let url = msg_id_str(class("NSURL"), "URLWithString:", ns(link));
            add(
                string,
                "AXLink",
                if url.is_null() { ns(link) } else { url },
                range,
            );
        }
        if let Some(lang) = &r.language {
            add(string, "AXLanguage", ns(lang), range);
        }
    }
}

unsafe fn msg_id_str(obj: Id, name: &str, a: Id) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, sel(name), a)
    }
}

/// `accessibilityAttributedStringForRange:`: the element's text over
/// `range` (UTF-16 units), each run with its attributes; nil for an
/// element without text.
extern "C" fn attributed_string_for_range(this: Id, _cmd: Sel, range: NsRange) -> Id {
    let built = super::ax_value::with_element_node(this, |node| {
        let text = ax_plan::attributed_text(node)?;
        let (loc, len) = (range.location as usize, range.length as usize);
        let units: Vec<u16> = text.encode_utf16().skip(loc).take(len).collect();
        let runs = ax_plan::attributed_runs(node, loc, len);
        Some((String::from_utf16_lossy(&units), runs))
    })
    .flatten();
    let Some((text, runs)) = built else {
        return std::ptr::null_mut();
    };
    unsafe {
        let alloc = msg_id(class("NSMutableAttributedString"), sel("alloc"));
        let string = msg_id_str(alloc, "initWithString:", ns(&text));
        if string.is_null() {
            return string;
        }
        for r in &runs {
            apply(string, r);
        }
        if std::env::var_os("STIPPLE_COCOA_A11Y").is_some() {
            println!(
                "Cocoa a11y: AXAttributedStringForRange {},{}: {} runs",
                range.location,
                range.length,
                runs.len()
            );
        }
        msg_id(string, sel("autorelease"))
    }
}
