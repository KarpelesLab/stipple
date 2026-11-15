//! Windows: `GetUserPreferredUILanguages` for the languages,
//! `GetUserDefaultLocaleName` and `GetLocaleInfoEx` for the formats (the
//! user's own, from *Region* in the settings), and the time zone through
//! `SystemTimeToTzSpecificLocalTime`.

use super::OsLocale;
use crate::locale::{HourCycle, OsFormats, Weekday, from_windows_picture};

const MUI_LANGUAGE_NAME: u32 = 0x8;
const LOCALE_SDECIMAL: u32 = 0x0E;
const LOCALE_STHOUSAND: u32 = 0x0F;
const LOCALE_SSHORTDATE: u32 = 0x1F;
const LOCALE_SLONGDATE: u32 = 0x20;
const LOCALE_STIMEFORMAT: u32 = 0x1003;
const LOCALE_IFIRSTDAYOFWEEK: u32 = 0x100C;
const LOCALE_SSHORTTIME: u32 = 0x79;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetUserPreferredUILanguages(
        flags: u32,
        count: *mut u32,
        buffer: *mut u16,
        len: *mut u32,
    ) -> i32;
    fn GetUserDefaultLocaleName(name: *mut u16, len: i32) -> i32;
    fn GetLocaleInfoEx(locale: *const u16, kind: u32, data: *mut u16, len: i32) -> i32;
    fn SystemTimeToTzSpecificLocalTime(
        zone: *const std::ffi::c_void,
        utc: *const SystemTime,
        local: *mut SystemTime,
    ) -> i32;
}

/// `GetUserPreferredUILanguages`'s list (NUL-separated, NUL-terminated).
fn languages() -> Vec<String> {
    let mut count = 0u32;
    let mut len = 0u32;
    unsafe {
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut len,
        ) == 0
            || len == 0
        {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len)
            == 0
        {
            return Vec::new();
        }
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

/// The user's value of `kind`, from their own locale (the null name is
/// `LOCALE_NAME_USER_DEFAULT`, which carries their customizations).
fn info(kind: u32) -> Option<String> {
    let mut buf = [0u16; 128];
    let n = unsafe { GetLocaleInfoEx(std::ptr::null(), kind, buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

#[cfg_attr(test, allow(dead_code))]
pub(super) fn query() -> OsLocale {
    let mut name = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    let formats_tag = (n > 1).then(|| String::from_utf16_lossy(&name[..n as usize - 1]));
    let time = info(LOCALE_STIMEFORMAT);
    let hour_cycle = time.as_deref().map(|t| {
        if t.contains('H') {
            HourCycle::H23
        } else {
            HourCycle::H12
        }
    });
    // 0 is Monday.
    let first_weekday = info(LOCALE_IFIRSTDAYOFWEEK)
        .and_then(|d| d.trim().parse::<u32>().ok())
        .filter(|d| *d < 7)
        .map(Weekday::from_days_from_monday);
    let picture = |kind| info(kind).map(|p| from_windows_picture(&p));
    // Windows has a short and a long date; its long one has the weekday,
    // which is the full style here.
    OsLocale {
        languages: languages(),
        formats_tag,
        formats: OsFormats {
            decimal: info(LOCALE_SDECIMAL),
            group: info(LOCALE_STHOUSAND),
            hour_cycle,
            first_weekday,
            dates: [
                picture(LOCALE_SSHORTDATE),
                None,
                None,
                picture(LOCALE_SLONGDATE),
            ],
            times: [
                picture(LOCALE_SSHORTTIME),
                time.map(|t| from_windows_picture(&t)),
            ],
        },
        utc_offset_minutes: None,
    }
}

/// Seconds east of UTC at `unix_secs`, from the difference between the UTC
/// time and the local one Windows converts it to.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn utc_offset(unix_secs: i64) -> i32 {
    let utc = crate::locale::DateTime::from_unix_utc(unix_secs);
    if !(1601..=30827).contains(&utc.year) {
        return 0;
    }
    let st = SystemTime {
        year: utc.year as u16,
        month: u16::from(utc.month),
        day: u16::from(utc.day),
        hour: u16::from(utc.hour),
        minute: u16::from(utc.minute),
        second: u16::from(utc.second),
        ..SystemTime::default()
    };
    let mut local = SystemTime::default();
    if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut local) } == 0 {
        return 0;
    }
    let local = crate::locale::DateTime {
        year: i32::from(local.year),
        month: local.month as u8,
        day: local.day as u8,
        hour: local.hour as u8,
        minute: local.minute as u8,
        second: local.second as u8,
    };
    (local.to_unix_utc() - utc.to_unix_utc()) as i32
}
