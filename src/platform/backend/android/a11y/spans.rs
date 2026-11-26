//! A node's text as `AccessibilityNodeInfo` reads it: a `SpannableString`
//! with its runs' spans (planned in `text_spans`), or a plain `String`.

use crate::platform::access::A11yNode;

use super::super::jni::{JObject, JValue, Jni};
use super::super::text_spans::{Kind, plan};

/// `Spanned.SPAN_EXCLUSIVE_EXCLUSIVE`.
const SPAN_EXCLUSIVE_EXCLUSIVE: i32 = 33;

/// The text a node's `AccessibilityNodeInfo` reads: a `SpannableString`
/// with `node`'s runs as spans, or a plain `String` for text drawn alike.
pub(super) fn text_of(jni: &Jni, node: &A11yNode) -> Option<JObject> {
    let plain = jni.string16(&node.name)?;
    let Some(spans) = plan(&node.name, &node.text_runs(), node.font.as_ref()) else {
        return Some(plain);
    };
    let Some(text) = jni.new_named(
        c"android/text/SpannableString",
        c"(Ljava/lang/CharSequence;)V",
        &[JValue { l: plain }],
    ) else {
        return Some(plain);
    };
    for s in &spans {
        // Each span's locals go with it.
        if !jni.push_frame() {
            break;
        }
        if let Some(what) = span_object(jni, &s.kind) {
            let _ = jni.call_void(
                text,
                c"setSpan",
                c"(Ljava/lang/Object;III)V",
                &[
                    JValue { l: what },
                    JValue { i: s.start },
                    JValue { i: s.end },
                    JValue {
                        i: SPAN_EXCLUSIVE_EXCLUSIVE,
                    },
                ],
            );
        }
        jni.pop_frame();
    }
    Some(text)
}

/// The span object `kind` stands for.
fn span_object(jni: &Jni, kind: &Kind) -> Option<JObject> {
    let int = |class: &std::ffi::CStr, v: i32| jni.new_named(class, c"(I)V", &[JValue { i: v }]);
    match kind {
        Kind::Style(s) => int(c"android/text/style/StyleSpan", *s),
        Kind::Underline => jni.new_named(c"android/text/style/UnderlineSpan", c"()V", &[]),
        Kind::Strikethrough => jni.new_named(c"android/text/style/StrikethroughSpan", c"()V", &[]),
        Kind::Foreground(c) => int(c"android/text/style/ForegroundColorSpan", *c),
        Kind::Background(c) => int(c"android/text/style/BackgroundColorSpan", *c),
        Kind::Url(url) => {
            let url = jni.string16(url)?;
            jni.new_named(
                c"android/text/style/URLSpan",
                c"(Ljava/lang/String;)V",
                &[JValue { l: url }],
            )
        }
        Kind::Locale(tag) => {
            let tag = jni.string16(tag)?;
            let locale = jni.call_static_object(
                c"java/util/Locale",
                c"forLanguageTag",
                c"(Ljava/lang/String;)Ljava/util/Locale;",
                &[JValue { l: tag }],
            )?;
            jni.new_named(
                c"android/text/style/LocaleSpan",
                c"(Ljava/util/Locale;)V",
                &[JValue { l: locale }],
            )
        }
        Kind::Size(dip) => jni.new_named(
            c"android/text/style/AbsoluteSizeSpan",
            c"(IZ)V",
            &[JValue { i: *dip }, JValue { z: 1 }],
        ),
    }
}
