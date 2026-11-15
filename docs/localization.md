# Localization

An app translated into a few languages needs to know which one the user
reads, and to write numbers, amounts of money, dates and times the way the
user's region does. Stipple reads the user's languages and formatting
settings from the OS on every platform, formats with built-in tables that give
the same result everywhere, picks plural forms by CLDR's rules, and has a small
message catalog for the app's strings.

<!-- live: localedemo -->

```rust
use stipple::locale::{Catalog, DateStyle, DateTime, negotiate};
use stipple::platform;

// The user's languages, most preferred first: ["fr-CA", "en-US"].
let languages = platform::preferred_languages();
// The best of the app's own languages for them: "fr".
let shown = negotiate(&languages, &["en", "fr", "ja"]).unwrap_or("en");

// Their conventions: French as written in Canada, with their clock.
let locale = platform::locale();
locale.format_number(1234.5);                      // "1 234,5"
locale.format_currency(9.99, "CAD");               // "9,99 $"
locale.format_date(&DateTime::now(), DateStyle::Long); // "16 novembre 2026"

// The app's strings, with plural forms.
let fr = Catalog::parse("fr", "notes.one = {count} note\nnotes.other = {count} notes")?;
fr.plural("notes", 3.0, &[]);                      // "3 notes"
```

## The user's languages

`platform::preferred_languages()` returns BCP 47 tags (`"fr-CA"`, `"en-US"`,
`"zh-Hant-TW"`), most preferred first, never empty (`"en-US"` when the OS says
nothing):

| Platform | Read from |
|---|---|
| macOS, iOS | `NSLocale.preferredLanguages` |
| Android | `LocaleList.getDefault()` (Android 7, API 24), else `Locale.getDefault()`, over JNI |
| Windows | `GetUserPreferredUILanguages` |
| Linux | `LANGUAGE` (gettext's list, unless the locale is `C`), then `LC_ALL`, `LC_MESSAGES` or `LANG` |
| Web | `navigator.languages` |

Pick the language to show with `locale::negotiate(&preferred, &shipped)`: the
first preferred language with an exact match, else one matching on the
language and script, else on the language alone (`fr-CA` gets `fr`, `zh-TW`
gets `zh-Hant`). It returns `None` when nothing matches: fall back to the
app's own language.

An app with a language setting of its own calls
`platform::set_preferred_languages(["ja"])`: the chosen language comes first
in `preferred_languages()`, `platform::locale()` uses it with the user's own
clock and week, and the running app rebuilds (an empty list goes back to the
OS's).

On iOS, the Settings app offers a per-app language only when the bundle's
`Info.plist` lists more than one language in `CFBundleLocalizations`;
`preferred_languages()` then starts with the one picked there.

## Formatting

A `Locale` is a language and region's conventions. `platform::locale()` is the
user's; `Locale::new("de-CH")` is any tag's, from the built-in tables.

| Method | `en-US` | `fr-FR` | `de-DE` | `ja-JP` |
|---|---|---|---|---|
| `format_number(1234567.891)` | 1,234,567.891 | 1 234 567,891 | 1.234.567,891 | 1,234,567.891 |
| `format_fixed(2.5, 2)` | 2.50 | 2,50 | 2,50 | 2.50 |
| `format_percent(0.256)` | 26% | 26 % | 26 % | 26% |
| `format_currency(1234.5, "EUR")` | €1,234.50 | 1 234,50 € | 1.234,50 € | €1,234.50 |
| `format_date(d, DateStyle::Short)` | 11/16/26 | 16/11/2026 | 16.11.26 | 2026/11/16 |
| `format_date(d, DateStyle::Medium)` | Nov 16, 2026 | 16 nov. 2026 | 16.11.2026 | 2026/11/16 |
| `format_date(d, DateStyle::Long)` | November 16, 2026 | 16 novembre 2026 | 16. November 2026 | 2026年11月16日 |
| `format_date(d, DateStyle::Full)` | Monday, November 16, 2026 | lundi 16 novembre 2026 | Montag, 16. November 2026 | 2026年11月16日月曜日 |
| `format_time(d, TimeStyle::Short)` | 9:05 PM | 21:05 | 21:05 | 21:05 |
| `format_date_time(d, Long, Short)` | November 16, 2026 at 9:05 PM | 16 novembre 2026 à 21:05 | 16. November 2026 um 21:05 | 2026年11月16日 21:05 |

- Numbers are rounded to at most three decimals; `format_fixed` gives
  exactly as many as asked. The grouping follows the language (India's
  `12,34,567`; Spanish and Polish leave four digits ungrouped) and Arabic
  writes Arabic-Indic digits (`١٬٢٣٤`), except in the Maghreb.
- Currencies take their ISO 4217 code. Each has its usual decimals (none for
  the yen and the won, three for the Kuwaiti dinar) and the locale places the
  symbol and the minus sign: `$1,234.50` at home and `US$` abroad, `€ 1.234,50`
  in Dutch, `R$ 10,00` in Brazil. Common currencies have a symbol; others show
  their code.
- `DateTime` is a date and a time of day on the Gregorian calendar:
  `DateTime::now()`, `DateTime::from_unix_local(secs)` (the user's time
  zone), `from_unix_utc`, or a literal. `format_pattern(d, "EEE d MMM")` takes
  a CLDR pattern of your own; `month_name`, `weekday_name`, `first_weekday`
  and `hour_cycle` help draw a calendar.
- Tags may carry Unicode extensions: `-u-hc-h23` for the clock, `-u-fw-mon`
  for the first day, `-u-rg-gbzzzz` for another region's conventions,
  `-u-nu-latn` for ASCII digits.

### Where the formats come from

Formatting always runs on Stipple's own tables, a subset of CLDR built into
the crate: separators, number, percent and currency patterns, date and time
patterns, month and weekday names, for English, French, German, Spanish,
Italian, Portuguese, Dutch, Swedish, Russian, Ukrainian, Polish, Czech,
Turkish, Japanese, Chinese, Korean, Arabic and Hebrew, with the regional
differences that matter most (British, Canadian, Australian and Indian
English, Canadian and Swiss French, Swiss and Austrian German, Mexican
Spanish, European Portuguese, Traditional Chinese). Other languages get
ISO-like dates (`2026-11-16`) and English names. The same `Locale` formats the
same way on every platform, and tests use the tables.

The OS supplies what the user chose: `platform::locale()` starts from the
tables for the user's region and lays the OS's settings on top.

| Platform | Region | Clock | First weekday | Separators, date and time formats |
|---|---|---|---|---|
| macOS, iOS | `NSLocale.currentLocale` (language and region set apart, as `en-FR`) | the locale's `j` pattern | `NSCalendar.firstWeekday` | `NSLocale`'s separators, `NSDateFormatter`'s patterns for each style, with the user's customizations |
| Windows | `GetUserDefaultLocaleName` | the time format | `LOCALE_IFIRSTDAYOFWEEK` | `GetLocaleInfoEx`: the decimal and thousands separators, the short and long date and time pictures (the long one is the full style here) |
| Android | the first language | `DateFormat.is24HourFormat` | `Calendar.getFirstDayOfWeek` | tables |
| Linux | `LC_ALL`, `LC_TIME` or `LANG` | GNOME's `clock-format`, through the portal | tables | tables |
| Web | the first language | `Intl.DateTimeFormat`'s hour cycle | `Intl.Locale`'s week info | tables |

Asking the OS to format each value (`NSNumberFormatter`, `GetDateFormatEx`,
`java.text`, `Intl`) would give slightly different text on each platform and
cost a call per value; reading its settings once, and its patterns where it
keeps them, gives the user's choices with one formatter everywhere.

The tables cover what most apps show. For more (other calendars, compact
numbers like "1.2K", relative times like "3 min ago", lists, units, collation)
use [`icu4x`](https://github.com/unicode-org/icu4x) with
`platform::preferred_languages()`'s tags; [`fluent`](https://projectfluent.org)
is the richer choice for message catalogs.

## Plural forms

`locale.plural(n)` gives `n`'s CLDR plural category: `Zero`, `One`, `Two`,
`Few`, `Many` or `Other`. English uses `One` and `Other`; French puts 0 and 1.5
in `One`; Russian and Ukrainian have `One` (1, 21), `Few` (2–4, 22) and `Many`
(5–20, 11); Arabic uses all six; Japanese, Chinese and Korean only `Other`.
The rules cover about forty languages. `plural_fixed(n, digits)` matches
`format_fixed`: "1.0 stars" is plural in English.

## Message catalogs

A `Catalog` holds an app's strings in one language, from a small text format:

```text
# Notes, in Russian
title = Заметки
greeting = Привет, {name}!
notes.one = {count} заметка
notes.few = {count} заметки
notes.many = {count} заметок
notes.other = {count} заметки
```

- `catalog.text("title")`; `catalog.format("greeting", &[("name", "Аня")])`.
- `catalog.plural("notes", n, &[])` picks `notes.<category>` (else
  `notes.other`, else `notes`), and `{count}` is `n` formatted for the
  catalog's locale (`set_locale(platform::locale())` for the user's
  separators).
- `\n` is a line break, `{{` and `}}` are braces.
- `.fallback(english)` looks up keys a translation doesn't have yet; a key
  found nowhere comes back as is.

Embed the files with `include_str!` and pick one with `negotiate`:

```rust
let shown = negotiate(&platform::preferred_languages(), &["en", "fr", "ru"]).unwrap_or("en");
let en = Catalog::parse("en", include_str!("../i18n/en.txt"))?;
let tr = match shown {
    "fr" => Catalog::parse("fr", include_str!("../i18n/fr.txt"))?.fallback(en),
    "ru" => Catalog::parse("ru", include_str!("../i18n/ru.txt"))?.fallback(en),
    _ => en,
};
```

## Right to left

The `App` mirrors the UI when the language it shows reads right to left
(Arabic, Hebrew, Persian, Urdu…). Say which languages the app ships with
`App::languages(["en", "fr", "he"])`: the direction then follows the language
the user gets, so an app shipping only English stays left to right on an
Arabic system. Without it, the direction follows the user's first language. A
theme with its own `layout_direction` keeps it (`Theme::follows_locale` is
then off). See [bidi.md](bidi.md#mirrored-layouts).

## Following changes

When the user's languages or formats change while the app runs, the event
loop delivers `Event::LocaleChanged`, and the `App` works the direction out
again, calls `App::on_locale_change(|state, locale| …)` and rebuilds its
views, so a view reading `platform::locale()` shows the new formats.

| Platform | Heard from |
|---|---|
| macOS | `NSCurrentLocaleDidChangeNotification` (region, languages, clock, formats) |
| iOS | `NSCurrentLocaleDidChangeNotification` (region, clock); a new language restarts the app |
| Android | the configuration change, with `locale\|layoutDirection` in the manifest's `configChanges` (see [Android](platforms/android.md#packaging)); the clock and first day are read again when the app comes back to the foreground |
| Windows | `WM_SETTINGCHANGE` for `intl` |
| Linux | GNOME's `clock-format` through the portal; the environment is fixed for the process |
| Web | `languagechange` |

## Stipple's own strings

A message box with no buttons given shows "OK" in the user's language
("Aceptar", "确定", "אישור"), and iOS's accessibility values ("on",
"checked", "expanded"…) are translated into the eighteen languages above.
Buttons and labels an app passes are shown as they are.
