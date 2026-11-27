//! The user's languages and formatting settings: [`preferred_languages`]
//! and [`locale`], and [`Event::LocaleChanged`](crate::platform::Event::LocaleChanged)
//! when they change.
//!
//! | | Languages | Formats | Changes heard |
//! |---|---|---|---|
//! | macOS, iOS | `NSLocale.preferredLanguages` | `NSLocale.currentLocale` (region, separators), `NSDateFormatter`'s patterns, `NSCalendar.firstWeekday` | `NSCurrentLocaleDidChangeNotification` (a new language restarts an iOS app) |
//! | Android | `LocaleList.getDefault()` (API 24; `Locale.getDefault()` before) | `DateFormat.is24HourFormat`, `Calendar.getFirstDayOfWeek` | the configuration change (with `locale` in `configChanges`), and coming back to the foreground |
//! | Windows | `GetUserPreferredUILanguages` | `GetUserDefaultLocaleName`, `GetLocaleInfoEx` (separators, date and time pictures, first day) | `WM_SETTINGCHANGE` for `intl` |
//! | Linux | `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` | `LC_ALL`, `LC_TIME`, `LANG`; GNOME's `clock-format` through the portal | the portal's `clock-format` |
//! | Web | `navigator.languages` | `Intl` (the clock, the first day) | `languagechange` |
//!
//! See `docs/localization.md`.

use crate::locale::{HourCycle, Locale, OsFormats, canonical_tag};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod apple;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;

/// What the OS says of the user's languages and formats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OsLocale {
    /// BCP 47 tags, most preferred first.
    pub languages: Vec<String>,
    /// The tag formats follow, when it isn't the first language's (macOS's
    /// region setting: `en-FR` is English with France's conventions).
    pub formats_tag: Option<String>,
    pub formats: OsFormats,
    /// Minutes east of UTC now (the web, which only says that).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub utc_offset_minutes: Option<i32>,
}

/// The OS's answer, kept until it says something changed.
static CACHE: Mutex<Option<OsLocale>> = Mutex::new(None);
/// The app's own choice of languages ([`set_preferred_languages`]).
static OVERRIDE: Mutex<Option<Vec<String>>> = Mutex::new(None);
static CHANGED: AtomicBool = AtomicBool::new(false);

fn os_locale() -> OsLocale {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = cache.as_ref() {
        return cached.clone();
    }
    let mut fresh = query();
    fresh.languages = fresh
        .languages
        .iter()
        .map(|l| canonical_tag(l))
        .filter(|l| !l.is_empty())
        .fold(Vec::new(), |mut seen, l| {
            if !seen.contains(&l) {
                seen.push(l);
            }
            seen
        });
    if fresh.languages.is_empty() {
        fresh.languages.push("en-US".into());
    }
    *cache = Some(fresh.clone());
    fresh
}

/// The user's preferred languages as BCP 47 tags (`"fr-CA"`, `"en-US"`,
/// `"zh-Hant-TW"`), most preferred first; never empty (`"en-US"` when the
/// OS says nothing). An app's choice through [`set_preferred_languages`]
/// comes first.
///
/// Pick the language to show with
/// [`locale::negotiate`](crate::locale::negotiate) against the languages the
/// app ships. On iOS, an app's own language setting (in the Settings app)
/// is only offered when the bundle's `Info.plist` lists the app's languages
/// in `CFBundleLocalizations`; this then returns the one picked there first.
///
/// ```no_run
/// let languages = stipple::platform::preferred_languages();
/// let shown = stipple::locale::negotiate(&languages, &["en", "fr", "ja"]).unwrap_or("en");
/// ```
pub fn preferred_languages() -> Vec<String> {
    let mut languages = os_locale().languages;
    if let Some(chosen) = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        let mut first: Vec<String> = chosen.iter().map(|l| canonical_tag(l)).collect();
        languages.retain(|l| !first.contains(l));
        first.append(&mut languages);
        return first;
    }
    languages
}

/// The user's formatting conventions: the built-in tables for their
/// region's language ([`Locale::new`]), with what the OS lets them change on
/// top: the 12- or 24-hour clock, the first day of the week, and, where the
/// OS keeps them (macOS, iOS, Windows), the decimal and grouping separators
/// and the date and time formats. See `docs/localization.md`.
///
/// An app's choice through [`set_preferred_languages`] picks the language;
/// the user's clock and week stay.
///
/// ```no_run
/// use stipple::locale::{DateStyle, DateTime};
/// let locale = stipple::platform::locale();
/// println!("{}", locale.format_date(&DateTime::now(), DateStyle::Long));
/// ```
pub fn locale() -> Locale {
    let os = os_locale();
    let chosen = OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|l| l.first().cloned());
    match chosen {
        Some(tag) => {
            // The app's language; the user's clock and week.
            let mut l = Locale::new(&tag);
            if let Some(hc) = os.formats.hour_cycle {
                l.set_hour_cycle(hc);
            }
            if let Some(fw) = os.formats.first_weekday {
                l.set_first_weekday(fw);
            }
            l
        }
        None => {
            let tag = os
                .formats_tag
                .clone()
                .unwrap_or_else(|| os.languages[0].clone());
            let mut l = Locale::new(&tag);
            l.apply(&os.formats);
            l
        }
    }
}

/// Put `languages` (BCP 47 tags) ahead of the user's, for an app with a
/// language setting of its own; an empty list goes back to the OS's. The
/// running app hears it as
/// [`Event::LocaleChanged`](crate::platform::Event::LocaleChanged), and an
/// [`App`](crate::App) rebuilds its views.
pub fn set_preferred_languages<I, T>(languages: I)
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let list: Vec<String> = languages.into_iter().map(Into::into).collect();
    *OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = (!list.is_empty()).then_some(list);
    note_changed();
}

/// Forget what the OS said and have the event loop deliver
/// [`Event::LocaleChanged`](crate::platform::Event::LocaleChanged). Safe from
/// any thread.
pub(crate) fn note_changed() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CHANGED.store(true, Ordering::Release);
    crate::platform::wake::wake();
}

/// Whether the locale may have changed since the last call.
pub(crate) fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::AcqRel)
}

/// Seconds east of UTC in the user's time zone at `unix_secs`.
pub(crate) fn utc_offset(unix_secs: i64) -> i32 {
    #[cfg(test)]
    {
        let _ = unix_secs;
        0
    }
    #[cfg(all(
        not(test),
        any(
            target_os = "linux",
            target_os = "fullrust",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        )
    ))]
    {
        unix::utc_offset(unix_secs)
    }
    #[cfg(all(not(test), target_os = "windows"))]
    {
        windows::utc_offset(unix_secs)
    }
    #[cfg(all(not(test), target_arch = "wasm32"))]
    {
        let _ = unix_secs;
        os_locale().utc_offset_minutes.unwrap_or(0) * 60
    }
    #[cfg(all(
        not(test),
        not(any(
            target_os = "linux",
            target_os = "fullrust",
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "windows",
            target_arch = "wasm32"
        ))
    ))]
    {
        let _ = unix_secs;
        0
    }
}

#[cfg(all(
    not(test),
    any(
        target_os = "linux",
        target_os = "fullrust",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )
))]
#[allow(unsafe_code)]
mod unix {
    use std::ffi::{c_char, c_int, c_long};

    /// `struct tm` up to `tm_gmtoff`, laid out alike by glibc, musl, bionic
    /// and Darwin.
    #[repr(C)]
    struct Tm {
        fields: [c_int; 9],
        gmtoff: c_long,
        zone: *const c_char,
    }

    unsafe extern "C" {
        fn localtime_r(time: *const c_long, out: *mut Tm) -> *mut Tm;
    }

    pub(super) fn utc_offset(unix_secs: i64) -> i32 {
        let Some(t) = c_long::try_from(unix_secs).ok() else {
            return 0;
        };
        let mut tm = Tm {
            fields: [0; 9],
            gmtoff: 0,
            zone: std::ptr::null(),
        };
        let ok = unsafe { !localtime_r(&t, &mut tm).is_null() };
        if ok { tm.gmtoff as i32 } else { 0 }
    }
}

// ---- Asking the OS -----------------------------------------------------------

/// The OS's answer. Tests get a fixed one, so the App's layout doesn't follow
/// the machine running them.
fn query() -> OsLocale {
    #[cfg(test)]
    {
        OsLocale {
            languages: vec!["en-US".into()],
            ..OsLocale::default()
        }
    }
    #[cfg(all(not(test), any(target_os = "macos", target_os = "ios")))]
    {
        apple::query()
    }
    #[cfg(all(not(test), target_os = "windows"))]
    {
        windows::query()
    }
    #[cfg(all(not(test), target_os = "android"))]
    {
        crate::platform::backend::android::locale().unwrap_or_else(|| from_env(env))
    }
    #[cfg(all(not(test), any(target_os = "linux", target_os = "fullrust")))]
    {
        let mut os = from_env(env);
        os.formats.hour_cycle = linux_clock();
        os
    }
    #[cfg(all(not(test), target_arch = "wasm32"))]
    {
        WEB.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    #[cfg(all(
        not(test),
        not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "android",
            target_os = "linux",
            target_os = "fullrust",
            target_arch = "wasm32"
        ))
    ))]
    {
        from_env(env)
    }
}

// Not every target reads the environment.
#[allow(dead_code)]
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The languages and the formats' locale the POSIX environment names:
/// `LANGUAGE` (a `:`-separated list, which gettext reads unless the locale
/// is `C`), then `LC_ALL`, `LC_MESSAGES` and `LANG` for the languages;
/// `LC_ALL`, `LC_TIME` and `LANG` for the formats.
#[allow(dead_code)]
fn from_env(var: impl Fn(&str) -> Option<String>) -> OsLocale {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    let posix = |v: &str| {
        let base = v.split(['.', '@']).next().unwrap_or("");
        base.is_empty() || base == "C" || base == "POSIX"
    };
    let messages = set("LC_ALL")
        .or_else(|| set("LC_MESSAGES"))
        .or_else(|| set("LANG"));
    let mut languages = Vec::new();
    if let Some(list) = set("LANGUAGE")
        && !messages.as_deref().is_some_and(posix)
    {
        languages.extend(
            list.split(':')
                .filter(|l| !l.is_empty() && !posix(l))
                .map(canonical_tag),
        );
    }
    if let Some(m) = messages.as_deref().filter(|m| !posix(m)) {
        languages.push(canonical_tag(m));
    }
    let formats_tag = set("LC_ALL")
        .or_else(|| set("LC_TIME"))
        .or_else(|| set("LANG"))
        .filter(|v| !posix(v))
        .map(|v| canonical_tag(&v));
    OsLocale {
        formats_tag: formats_tag.filter(|t| languages.first() != Some(t)),
        languages,
        ..OsLocale::default()
    }
}

/// GNOME's clock setting (`org.gnome.desktop.interface clock-format`), read
/// through the desktop portal.
#[cfg(all(not(test), any(target_os = "linux", target_os = "fullrust")))]
fn linux_clock() -> Option<HourCycle> {
    let mut bus = crate::platform::a11y::DBus::connect_session().ok()?;
    match bus
        .portal_setting_string("org.gnome.desktop.interface", "clock-format")?
        .as_str()
    {
        "12h" => Some(HourCycle::H12),
        "24h" => Some(HourCycle::H23),
        _ => None,
    }
}

// ---- The web -------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
static WEB: Mutex<OsLocale> = Mutex::new(OsLocale {
    languages: Vec::new(),
    formats_tag: None,
    formats: OsFormats {
        decimal: None,
        group: None,
        hour_cycle: None,
        first_weekday: None,
        dates: [None, None, None, None],
        times: [None, None],
    },
    utc_offset_minutes: None,
});

/// What `web/stipple.js` read of the page's languages and `Intl`: lines of
/// `key=value` (`languages=fr-CA,en`, `hour12=0`, `first_day=1` (Monday) …
/// `7` (Sunday), `utc_offset=-300` in minutes). Returns whether it changed.
#[cfg(target_arch = "wasm32")]
pub(crate) fn set_web(blob: &str) -> bool {
    let os = parse_web(blob);
    let mut web = WEB.lock().unwrap_or_else(|e| e.into_inner());
    if *web == os {
        return false;
    }
    *web = os;
    drop(web);
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    true
}

#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn parse_web(blob: &str) -> OsLocale {
    let mut os = OsLocale::default();
    for line in blob.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "languages" => {
                os.languages = value
                    .split(',')
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| canonical_tag(l.trim()))
                    .collect();
            }
            "hour12" => {
                os.formats.hour_cycle = match value {
                    "1" | "true" => Some(HourCycle::H12),
                    "0" | "false" => Some(HourCycle::H23),
                    _ => None,
                }
            }
            "first_day" => {
                os.formats.first_weekday = value
                    .parse::<u32>()
                    .ok()
                    .filter(|d| (1..=7).contains(d))
                    .map(|d| crate::locale::Weekday::from_days_from_monday(d - 1));
            }
            "utc_offset" => os.utc_offset_minutes = value.parse().ok(),
            _ => {}
        }
    }
    os
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn posix_environment() {
        let os = from_env(env_of(&[("LANG", "fr_CA.UTF-8")]));
        assert_eq!(os.languages, ["fr-CA"]);
        assert_eq!(os.formats_tag, None);

        let os = from_env(env_of(&[
            ("LANGUAGE", "de:en_GB"),
            ("LANG", "de_DE.UTF-8"),
            ("LC_TIME", "en_GB.UTF-8"),
        ]));
        assert_eq!(os.languages, ["de", "en-GB", "de-DE"]);
        assert_eq!(os.formats_tag.as_deref(), Some("en-GB"));

        // LC_ALL wins; a C locale turns LANGUAGE off.
        let os = from_env(env_of(&[("LC_ALL", "C"), ("LANGUAGE", "fr")]));
        assert!(os.languages.is_empty());
        let os = from_env(env_of(&[("LC_ALL", "he_IL.UTF-8"), ("LANG", "en_US")]));
        assert_eq!(os.languages, ["he-IL"]);
    }

    #[test]
    fn web_blob() {
        let os = parse_web("languages=fr-CA,en\nhour12=0\nfirst_day=7\nutc_offset=-300\n");
        assert_eq!(os.languages, ["fr-CA", "en"]);
        assert_eq!(os.formats.hour_cycle, Some(HourCycle::H23));
        assert_eq!(
            os.formats.first_weekday,
            Some(crate::locale::Weekday::Sunday)
        );
        assert_eq!(os.utc_offset_minutes, Some(-300));
    }

    #[test]
    fn tests_see_a_fixed_locale_and_an_override() {
        assert_eq!(preferred_languages()[0], "en-US");
        assert_eq!(locale().tag(), "en-US");
    }
}
