//! macOS and iOS: `NSLocale` for the languages and the region,
//! `NSDateFormatter` for the date and time formats (with the user's
//! customizations on macOS), `NSCalendar` for the first day of the week.

use super::OsLocale;
use crate::locale::{HourCycle, OsFormats, Weekday};
use crate::platform::native::objc::{Id, class, msg, nsstring, rust_string, with_pool};

/// The `NSString`s in an `NSArray`.
fn strings(array: Id) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    let count = unsafe { msg!(array, "count", () -> usize) };
    (0..count)
        .filter_map(|i| rust_string(unsafe { msg!(array, "objectAtIndex:", (i: usize) -> Id) }))
        .collect()
}

/// The pattern `NSDateFormatter` uses for a date style and a time style
/// (`NSDateFormatterStyle`: 0 none, 1 short, 2 medium, 3 long, 4 full).
fn pattern(locale: Id, date: usize, time: usize) -> Option<String> {
    unsafe {
        let f = msg!(class("NSDateFormatter"), "alloc", () -> Id);
        let f = msg!(f, "init", () -> Id);
        if f.is_null() {
            return None;
        }
        msg!(f, "setLocale:", (locale: Id) -> ());
        msg!(f, "setDateStyle:", (date: usize) -> ());
        msg!(f, "setTimeStyle:", (time: usize) -> ());
        let p = rust_string(msg!(f, "dateFormat", () -> Id));
        msg!(f, "release", () -> ());
        p.filter(|p| !p.is_empty())
    }
}

#[cfg_attr(test, allow(dead_code))]
pub(super) fn query() -> OsLocale {
    with_pool(|| unsafe {
        let ns_locale = class("NSLocale");
        let languages = strings(msg!(ns_locale, "preferredLanguages", () -> Id));
        let current = msg!(ns_locale, "currentLocale", () -> Id);
        if current.is_null() {
            return OsLocale {
                languages,
                ..OsLocale::default()
            };
        }
        let formats_tag = rust_string(msg!(current, "localeIdentifier", () -> Id));
        // `j` is the locale's preferred hour field: `h a` or `HH`, after the
        // user's 24-hour setting.
        let j = nsstring("j");
        let zero = 0usize;
        let hour = rust_string(msg!(
            class("NSDateFormatter"),
            "dateFormatFromTemplate:options:locale:",
            (j: Id, zero: usize, current: Id) -> Id
        ));
        let hour_cycle = hour.map(|h| {
            if h.contains(['h', 'K']) {
                HourCycle::H12
            } else {
                HourCycle::H23
            }
        });
        let calendar = msg!(class("NSCalendar"), "currentCalendar", () -> Id);
        let first_weekday = (!calendar.is_null())
            .then(|| msg!(calendar, "firstWeekday", () -> usize))
            .filter(|d| (1..=7).contains(d))
            // 1 is Sunday.
            .map(|d| Weekday::from_days_from_monday((d as u32 + 5) % 7));
        OsLocale {
            languages,
            formats_tag,
            formats: OsFormats {
                decimal: rust_string(msg!(current, "decimalSeparator", () -> Id)),
                group: rust_string(msg!(current, "groupingSeparator", () -> Id)),
                hour_cycle,
                first_weekday,
                dates: [1, 2, 3, 4].map(|style| pattern(current, style, 0)),
                times: [1, 2].map(|style| pattern(current, 0, style)),
            },
            utc_offset_minutes: None,
        }
    })
}
