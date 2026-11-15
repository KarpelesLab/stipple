//! Formatting for the user's language and region: numbers, percentages,
//! amounts of money, dates and times, plural forms, and a small message
//! catalog.
//!
//! A [`Locale`] is a language and region's conventions: `1,234.5` or
//! `1 234,5`, `11/16/26` or `16/11/2026`, `9:05 PM` or `21:05`, Sunday or
//! Monday first. [`platform::locale`](crate::platform::locale) is the user's,
//! with what they changed in the OS's settings (the 12/24-hour clock, the
//! first day of the week, the separators and date formats where the OS
//! keeps them); [`Locale::new`] is any language tag's, from tables built
//! into Stipple (a subset of CLDR), the same on every platform.
//!
//! ```
//! use stipple::locale::{DateStyle, DateTime, Locale, TimeStyle};
//!
//! let fr = Locale::new("fr-FR");
//! assert_eq!(fr.format_number(1234.5), "1\u{202f}234,5");
//! assert_eq!(fr.format_currency(9.99, "EUR"), "9,99\u{a0}€");
//! let dt = DateTime::new(2026, 11, 16).at(21, 5);
//! assert_eq!(fr.format_date(&dt, DateStyle::Long), "16 novembre 2026");
//! assert_eq!(fr.format_time(&dt, TimeStyle::Short), "21:05");
//!
//! let us = Locale::new("en-US");
//! assert_eq!(us.format_date_time(&dt, DateStyle::Medium, TimeStyle::Short),
//!            "Nov 16, 2026, 9:05 PM");
//! ```
//!
//! Plural forms follow CLDR's rules ([`Locale::plural`]), and a [`Catalog`]
//! holds an app's translated strings with their plural forms. See
//! `docs/localization.md` in the repository.

mod catalog;
mod data;
mod date;
mod plural;

pub use catalog::{Catalog, CatalogError};
pub use date::{DateStyle, DateTime, HourCycle, TimeStyle, Weekday};
pub use plural::PluralCategory;

#[cfg(target_os = "windows")]
pub(crate) use date::from_windows_picture;

use crate::layout::LayoutDirection;
use data::LangData;

/// A language and region's conventions for writing numbers, money, dates and
/// times, and choosing plural forms. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locale {
    tag: String,
    language: String,
    script: Option<String>,
    region: Option<String>,
    data: &'static LangData,
    pub(crate) decimal: String,
    pub(crate) group: String,
    min_group: u8,
    /// The size of the groups after the first (3; 2 in India's lakh and crore).
    secondary: u8,
    minus: String,
    percent: String,
    currency: String,
    pub(crate) dates: [String; 4],
    pub(crate) time12: [String; 2],
    pub(crate) time23: [String; 2],
    glue: [String; 2],
    am_pm: [String; 2],
    days_abbr: [&'static str; 7],
    hour_cycle: HourCycle,
    first_weekday: Weekday,
    /// Arabic-Indic digits (٠١٢٣٤٥٦٧٨٩) rather than ASCII ones.
    native_digits: bool,
}

impl Default for Locale {
    /// `en-US`.
    fn default() -> Self {
        Self::new("en-US")
    }
}

impl Locale {
    /// The conventions of `tag`, a BCP 47 language tag (`"fr-CA"`,
    /// `"zh-Hant-TW"`) or a POSIX locale name (`"de_DE.UTF-8"`), from
    /// Stipple's built-in tables.
    ///
    /// The tables cover English, French, German, Spanish, Italian,
    /// Portuguese, Dutch, Swedish, Russian, Ukrainian, Polish, Czech,
    /// Turkish, Japanese, Chinese, Korean, Arabic and Hebrew, with the
    /// regional differences that matter most (`en-GB` dates, `fr-CA`'s,
    /// `de-CH` separators, India's grouping, …). Other languages get
    /// ISO-style dates (`2026-11-16`), English names, and their region's
    /// clock and week; plural rules cover many more.
    ///
    /// Unicode extensions are read: `-u-hc-h12` or `-u-hc-h23` for the
    /// clock, `-u-fw-mon` … `-u-fw-sun` for the first day of the week,
    /// `-u-rg-gbzzzz` for the region's conventions, and `-u-nu-latn` or
    /// `-u-nu-arab` for the digits.
    pub fn new(tag: &str) -> Self {
        let parsed = Tag::parse(tag);
        let data = data::lang(&parsed.language).unwrap_or(&data::ROOT);
        let region = parsed
            .region
            .clone()
            .or_else(|| (!data.region.is_empty()).then(|| data.region.to_string()))
            .or_else(|| {
                let r = data::likely_region(&parsed.language);
                (!r.is_empty()).then(|| r.to_string())
            })
            .or_else(
                || match (parsed.language.as_str(), parsed.script.as_deref()) {
                    ("zh", Some("Hant")) => Some("TW".into()),
                    _ => None,
                },
            );
        let s = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let arr2 = |a: [&str; 2]| a.map(str::to_string);
        let mut l = Self {
            tag: parsed.canonical(),
            language: parsed.language.clone(),
            script: parsed.script.clone(),
            region: region.clone(),
            data,
            decimal: data.decimal.into(),
            group: data.group.into(),
            min_group: data.min_group,
            secondary: 3,
            minus: data.minus.into(),
            percent: data.percent.into(),
            currency: data.currency.into(),
            dates: s(&data.dates).try_into().expect("four dates"),
            time12: arr2(data.time12),
            time23: arr2(data.time23),
            glue: arr2(data.glue),
            am_pm: arr2(data.am_pm),
            days_abbr: data.days_abbr,
            hour_cycle: HourCycle::H23,
            first_weekday: Weekday::Monday,
            native_digits: data.lang == "ar",
        };
        // The region's conventions: `-u-rg-` if given, else the tag's.
        let conventions = parsed.rg.clone().or(region);
        if let Some(r) = conventions.as_deref() {
            l.apply_region(r);
        }
        if let Some(hc) = parsed.hc {
            l.hour_cycle = hc;
        }
        if let Some(fw) = parsed.fw {
            l.first_weekday = fw;
        }
        if let Some(native) = parsed.nu_native {
            l.native_digits = native;
        }
        l
    }

    /// What differs by region within a language, and the region's clock and
    /// week.
    fn apply_region(&mut self, region: &str) {
        let set4 = |a: [&str; 4]| a.map(str::to_string);
        let lang = self.language.as_str();
        match (lang, region) {
            ("en", "US" | "PR" | "UM" | "VI" | "AS" | "GU" | "MP" | "PH") => {}
            ("en", "CA") => {
                self.dates = set4(["y-MM-dd", "MMM d, y", "MMMM d, y", "EEEE, MMMM d, y"]);
                self.am_pm = ["a.m.".into(), "p.m.".into()];
            }
            ("en", "AU" | "NZ") => {
                self.dates = set4(["d/M/yy", "d MMM y", "d MMMM y", "EEEE d MMMM y"]);
                self.am_pm = ["am".into(), "pm".into()];
            }
            ("en", "IN") => {
                self.dates = set4(["dd/MM/yy", "d MMM y", "d MMMM y", "EEEE, d MMMM y"]);
                self.secondary = 2;
                self.am_pm = ["am".into(), "pm".into()];
            }
            ("en", "ZA") => {
                self.dates = set4(["y/MM/dd", "dd MMM y", "dd MMMM y", "EEEE, dd MMMM y"]);
                self.decimal = ",".into();
                self.group = "\u{a0}".into();
            }
            ("en", _) => {
                // CLDR's en-001 (the UK, Ireland, and English elsewhere).
                self.dates = set4(["dd/MM/y", "d MMM y", "d MMMM y", "EEEE d MMMM y"]);
                self.am_pm = ["am".into(), "pm".into()];
                if region == "CH" {
                    self.group = "’".into();
                }
            }
            ("fr", "CA") => {
                self.dates = set4(["y-MM-dd", "d MMM y", "d MMMM y", "EEEE d MMMM y"]);
                self.time23 = ["HH 'h' mm".into(), "HH 'h' mm 'min' ss 's'".into()];
                self.group = "\u{a0}".into();
                self.percent = "#\u{a0}%".into();
                self.am_pm = ["a.m.".into(), "p.m.".into()];
            }
            ("fr", "CH") => {
                self.dates[0] = "dd.MM.yy".into();
            }
            ("fr", "BE") => {
                self.dates[0] = "d/MM/yy".into();
            }
            ("de" | "it" | "rm", "CH" | "LI") => {
                self.decimal = ".".into();
                self.group = "’".into();
            }
            ("de", "AT") => {
                self.group = "\u{a0}".into();
            }
            ("es", "MX") => {
                self.decimal = ".".into();
                self.group = ",".into();
                self.min_group = 1;
                self.currency = "¤#".into();
                self.percent = "#%".into();
                self.dates[0] = "dd/MM/yy".into();
            }
            ("es", "US") => {
                self.decimal = ".".into();
                self.group = ",".into();
                self.min_group = 1;
                self.currency = "¤#".into();
                self.dates[0] = "d/M/y".into();
            }
            ("pt", "BR") => {}
            ("pt", _) => {
                self.group = "\u{a0}".into();
                self.min_group = 2;
                self.currency = "#\u{a0}¤".into();
                self.dates[0] = "dd/MM/yy".into();
                self.dates[1] = "dd/MM/y".into();
            }
            ("nl", "BE") => {
                self.dates[0] = "d/MM/y".into();
            }
            ("zh", "TW" | "HK" | "MO") => {
                self.days_abbr = data::ZH_HANT_DAYS_ABBR;
                if region != "TW" {
                    self.dates[0] = "d/M/y".into();
                }
            }
            ("ar", "MA" | "DZ" | "TN" | "LY" | "EH" | "MR") => {
                self.native_digits = false;
                self.decimal = ",".into();
                self.group = ".".into();
            }
            _ => {}
        }
        if self.language == "zh" && self.script.as_deref() == Some("Hant") {
            self.days_abbr = data::ZH_HANT_DAYS_ABBR;
        }
        self.hour_cycle = if data::H12_REGIONS.contains(&region) && (lang, region) != ("fr", "CA") {
            HourCycle::H12
        } else {
            HourCycle::H23
        };
        self.first_weekday = if data::SUNDAY_REGIONS.contains(&region) {
            Weekday::Sunday
        } else if data::SATURDAY_REGIONS.contains(&region) {
            Weekday::Saturday
        } else {
            Weekday::Monday
        };
    }

    /// The canonical language tag: `"fr-CA"`, `"zh-Hant-TW"`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The language subtag, lowercase: `"fr"`.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// The script subtag, if the tag has one: `"Hant"`.
    pub fn script(&self) -> Option<&str> {
        self.script.as_deref()
    }

    /// The region, uppercase: the tag's, or the language's most likely one
    /// (`"FR"` for `fr`). `None` for a language Stipple doesn't place.
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }

    /// Whether the clock shows 12 or 24 hours.
    pub fn hour_cycle(&self) -> HourCycle {
        self.hour_cycle
    }

    /// Show the clock with 12 or 24 hours, whatever the region does.
    pub fn set_hour_cycle(&mut self, cycle: HourCycle) {
        self.hour_cycle = cycle;
    }

    /// The day weeks start on, in calendars.
    pub fn first_weekday(&self) -> Weekday {
        self.first_weekday
    }

    /// Start weeks on `day`.
    pub fn set_first_weekday(&mut self, day: Weekday) {
        self.first_weekday = day;
    }

    /// The decimal separator: `"."` or `","`.
    pub fn decimal_separator(&self) -> &str {
        &self.decimal
    }

    /// The thousands separator: `","`, `"."`, a narrow no-break space…
    pub fn grouping_separator(&self) -> &str {
        &self.group
    }

    /// The direction a UI in this language reads in (see
    /// [`LayoutDirection::for_language`]).
    pub fn layout_direction(&self) -> LayoutDirection {
        LayoutDirection::for_language(&self.language)
    }

    /// The plural category of `n` (written as Rust writes it: `1`, `2.5`).
    /// See [`PluralCategory`].
    pub fn plural(&self, n: f64) -> PluralCategory {
        plural::category(
            &self.language,
            self.region.as_deref(),
            plural::Operands::from_f64(n),
        )
    }

    /// The plural category of `n` written with exactly `digits` fraction
    /// digits, as [`format_fixed`](Self::format_fixed) writes it: in English
    /// `1` is "1 star" but `1.0` is "1.0 stars".
    pub fn plural_fixed(&self, n: f64, digits: usize) -> PluralCategory {
        let text = format!("{:.*}", digits.min(18), n.abs());
        plural::category(
            &self.language,
            self.region.as_deref(),
            plural::Operands::parse(&text),
        )
    }

    // ---- Numbers ----------------------------------------------------------

    /// `n` with up to three fraction digits and the thousands grouped:
    /// `1,234.5` in English, `1.234,5` in German.
    pub fn format_number(&self, n: f64) -> String {
        self.number(n, 0, 3)
    }

    /// `n` with exactly `digits` fraction digits: `format_fixed(2.5, 2)` is
    /// `2.50` in English.
    pub fn format_fixed(&self, n: f64, digits: usize) -> String {
        self.number(n, digits, digits)
    }

    /// `fraction` as a whole percentage: `0.256` is `26%` in English,
    /// `26 %` in French.
    pub fn format_percent(&self, fraction: f64) -> String {
        let body = self.number(fraction * 100.0, 0, 0);
        self.place(&self.percent, &body, None)
    }

    /// `amount` of the currency named by its ISO 4217 `code` (`"USD"`,
    /// `"EUR"`, `"JPY"`), with that currency's usual decimals and the
    /// locale's symbol placement: `$1,234.50` in `en-US`, `1 234,50 €` in
    /// `fr-FR`, `€ 1.234,50` in `nl-NL`. Common currencies have their
    /// symbol (`$` at home, `US$` abroad); others show their code.
    pub fn format_currency(&self, amount: f64, code: &str) -> String {
        let code = code.to_ascii_uppercase();
        let digits = currency_digits(&code);
        let body = self.number(amount, digits, digits);
        let symbol = self.currency_symbol(&code);
        self.place(&self.currency, &body, Some(&symbol))
    }

    /// Put `body` (a formatted number, maybe with a minus) in `pattern`,
    /// with the minus at the front when the pattern starts with the symbol.
    fn place(&self, pattern: &str, body: &str, symbol: Option<&str>) -> String {
        let (negative, digits) = match body.strip_prefix(self.minus.as_str()) {
            Some(rest) => (true, rest),
            None => (false, body),
        };
        let mut symbol = symbol.unwrap_or("").to_string();
        // A code ("CHF") next to the number gets a space.
        if symbol.chars().last().is_some_and(|c| c.is_alphabetic()) && pattern.contains("¤#") {
            symbol.push('\u{a0}');
        }
        if symbol.chars().next().is_some_and(|c| c.is_alphabetic()) && pattern.contains("#¤") {
            symbol.insert(0, '\u{a0}');
        }
        let core = pattern.trim_start_matches(['\u{200f}', '\u{200e}']);
        let mark = &pattern[..pattern.len() - core.len()];
        // The minus goes before the symbol, except in Dutch ("€ -1,00").
        let symbol_first = core.starts_with(['¤', '%']) && self.language != "nl";
        let filled = core.replace('¤', &symbol);
        let out = if negative && !symbol_first {
            filled.replace('#', &format!("{}{digits}", self.minus))
        } else {
            filled.replace('#', digits)
        };
        if negative && symbol_first {
            format!("{mark}{}{out}", self.minus)
        } else {
            format!("{mark}{out}")
        }
    }

    /// `n` rounded to `max` fraction digits, keeping at least `min`, grouped,
    /// with the locale's separators, minus sign and digits.
    fn number(&self, n: f64, min: usize, max: usize) -> String {
        if n.is_nan() {
            return "NaN".into();
        }
        let min = min.min(18);
        let max = max.clamp(min, 18);
        if n.is_infinite() {
            let sign = if n < 0.0 { self.minus.as_str() } else { "" };
            return format!("{sign}∞");
        }
        let text = format!("{:.*}", max, n.abs());
        let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
        let mut frac = frac.to_string();
        while frac.len() > min && frac.ends_with('0') {
            frac.pop();
        }
        let zero = int.bytes().all(|b| b == b'0') && frac.bytes().all(|b| b == b'0');
        let mut out = String::new();
        if n < 0.0 && !zero {
            out.push_str(&self.minus);
        }
        out.push_str(&self.grouped(int));
        if !frac.is_empty() {
            out.push_str(&self.decimal);
            out.push_str(&frac);
        }
        self.digits(&out)
    }

    /// The integer digits `int` with the grouping separator.
    fn grouped(&self, int: &str) -> String {
        let len = int.len();
        if len < 4 + usize::from(self.min_group.saturating_sub(1)) {
            return int.to_string();
        }
        // Group boundaries counted from the right: 3, then `secondary`.
        let mut cuts = Vec::new();
        let mut at = len - 3;
        cuts.push(at);
        let step = usize::from(self.secondary.max(1));
        while at > step {
            at -= step;
            cuts.push(at);
        }
        let mut out = String::new();
        let mut prev = 0;
        for &cut in cuts.iter().rev() {
            out.push_str(&int[prev..cut]);
            out.push_str(&self.group);
            prev = cut;
        }
        out.push_str(&int[prev..]);
        out
    }

    /// `text` with its ASCII digits in the locale's digits.
    pub(crate) fn digits(&self, text: &str) -> String {
        if !self.native_digits {
            return text.to_string();
        }
        text.chars()
            .map(|c| match c.to_digit(10) {
                Some(d) if c.is_ascii_digit() => char::from_u32(0x660 + d).unwrap_or(c),
                _ => c,
            })
            .collect()
    }

    fn currency_symbol(&self, code: &str) -> String {
        let region = self.region.as_deref().unwrap_or("");
        let lang = self.language.as_str();
        let local = |home: &[&str], here: &str, abroad: &str| {
            if home.contains(&region) { here } else { abroad }.to_string()
        };
        match code {
            "USD" => local(
                &["US", "PR", "UM", "VI", "AS", "GU", "MP", "EC", "SV"],
                "$",
                "US$",
            ),
            "CAD" => local(&["CA"], "$", "CA$"),
            "AUD" => local(&["AU"], "$", "A$"),
            "NZD" => local(&["NZ"], "$", "NZ$"),
            "MXN" => local(&["MX"], "$", "MX$"),
            "HKD" => local(&["HK"], "HK$", "HK$"),
            "TWD" => local(&["TW"], "$", "NT$"),
            "SGD" => local(&["SG"], "$", "SGD"),
            "EUR" => "€".into(),
            "GBP" => "£".into(),
            "JPY" if lang == "ja" => "￥".into(),
            "JPY" => "¥".into(),
            "CNY" if lang == "zh" && region == "CN" => "¥".into(),
            "CNY" => "CN¥".into(),
            "KRW" => "₩".into(),
            "INR" => "₹".into(),
            "RUB" if lang == "ru" => "₽".into(),
            "UAH" if lang == "uk" => "₴".into(),
            "PLN" if lang == "pl" => "zł".into(),
            "CZK" if lang == "cs" => "Kč".into(),
            "TRY" if lang == "tr" => "₺".into(),
            "ILS" => "₪".into(),
            "BRL" => "R$".into(),
            "VND" => "₫".into(),
            "SEK" if region == "SE" => "kr".into(),
            "NOK" if region == "NO" => "kr".into(),
            "DKK" if region == "DK" => "kr.".into(),
            "EGP" if lang == "ar" => "ج.م.\u{200f}".into(),
            "SAR" if lang == "ar" => "ر.س.\u{200f}".into(),
            other => other.to_string(),
        }
    }

    // ---- Dates and times --------------------------------------------------

    /// The date of `dt`: `11/16/26`, `Nov 16, 2026`, `November 16, 2026` or
    /// `Monday, November 16, 2026` in `en-US`.
    pub fn format_date(&self, dt: &DateTime, style: DateStyle) -> String {
        date::format_pattern(self, dt, self.date_pattern(style))
    }

    /// The time of `dt`: `9:05 PM` in `en-US`, `21:05` in `fr-FR`, on the
    /// locale's [`hour_cycle`](Self::hour_cycle).
    pub fn format_time(&self, dt: &DateTime, style: TimeStyle) -> String {
        date::format_pattern(self, dt, &self.time_pattern(style))
    }

    /// The date and the time of `dt`, joined as the language joins them:
    /// `Nov 16, 2026, 9:05 PM`, `16 novembre 2026 à 21:05`.
    pub fn format_date_time(&self, dt: &DateTime, date: DateStyle, time: TimeStyle) -> String {
        let glue = match date {
            DateStyle::Short | DateStyle::Medium => &self.glue[0],
            _ => &self.glue[1],
        };
        let pattern = glue
            .replace("{1}", self.date_pattern(date))
            .replace("{0}", &self.time_pattern(time));
        date::format_pattern(self, dt, &pattern)
    }

    /// `dt` by a CLDR date pattern of your own: `"EEEE"` is the weekday's
    /// name, `"MMM y"` the abbreviated month and the year, `"d MMMM"` the
    /// day and the month. Letters are fields (`y`, `M`, `d`, `E`, `h`, `H`,
    /// `m`, `s`, `a`), `'…'` is quoted text.
    pub fn format_pattern(&self, dt: &DateTime, pattern: &str) -> String {
        date::format_pattern(self, dt, pattern)
    }

    fn date_pattern(&self, style: DateStyle) -> &str {
        let i = match style {
            DateStyle::Short => 0,
            DateStyle::Medium => 1,
            DateStyle::Long => 2,
            DateStyle::Full => 3,
        };
        &self.dates[i]
    }

    fn time_pattern(&self, style: TimeStyle) -> String {
        let i = match style {
            TimeStyle::Short => 0,
            TimeStyle::Medium => 1,
        };
        let (own, other) = match self.hour_cycle {
            HourCycle::H12 => (&self.time12[i], &self.time23[i]),
            HourCycle::H23 => (&self.time23[i], &self.time12[i]),
        };
        if own.is_empty() {
            date::to_hour_cycle(other, self.hour_cycle)
        } else {
            own.clone()
        }
    }

    /// The name of `month` (1 to 12) as a date writes it: "November",
    /// "novembre", and in Slavic languages the form a date takes ("ноября").
    pub fn month_name(&self, month: u8) -> &str {
        self.data.months[usize::from(month.clamp(1, 12) - 1)]
    }

    /// The abbreviated name of `month` (1 to 12): "Nov", "nov.".
    pub fn month_abbr(&self, month: u8) -> &str {
        self.data.months_abbr[usize::from(month.clamp(1, 12) - 1)]
    }

    /// The name of `day`: "Monday", "lundi".
    pub fn weekday_name(&self, day: Weekday) -> &str {
        self.data.days[usize::from(day.days_from_monday())]
    }

    /// The abbreviated name of `day`: "Mon", "lun.".
    pub fn weekday_abbr(&self, day: Weekday) -> &str {
        self.days_abbr[usize::from(day.days_from_monday())]
    }

    /// The AM or PM marker for `hour` (0 to 23).
    pub(crate) fn am_pm(&self, hour: u8) -> &str {
        &self.am_pm[usize::from(hour >= 12)]
    }

    // ---- What the OS says -------------------------------------------------

    /// Take the user's settings the OS reported on top of the tables.
    pub(crate) fn apply(&mut self, os: &OsFormats) {
        if let Some(d) = os.decimal.as_ref().filter(|s| !s.is_empty()) {
            self.decimal = d.clone();
        }
        if let Some(g) = &os.group {
            self.group = g.clone();
        }
        if let Some(hc) = os.hour_cycle {
            self.hour_cycle = hc;
        }
        if let Some(fw) = os.first_weekday {
            self.first_weekday = fw;
        }
        for (slot, pattern) in self.dates.iter_mut().zip(&os.dates) {
            if let Some(p) = pattern.as_ref().filter(|p| !p.is_empty()) {
                *slot = p.clone();
            }
        }
        for (i, pattern) in os.times.iter().enumerate() {
            let Some(p) = pattern.as_ref().filter(|p| !p.is_empty()) else {
                continue;
            };
            // An OS pattern is on the user's clock; the other clock's is
            // derived when asked for.
            if p.contains(['h', 'K']) {
                self.time12[i] = p.clone();
                self.time23[i] = String::new();
            } else {
                self.time23[i] = p.clone();
                self.time12[i] = String::new();
            }
        }
    }
}

/// The user's formatting settings an OS reports beside the language: each
/// one the OS has replaces the built-in table's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OsFormats {
    pub decimal: Option<String>,
    pub group: Option<String>,
    pub hour_cycle: Option<HourCycle>,
    pub first_weekday: Option<Weekday>,
    /// Short, medium, long and full date patterns, in CLDR's syntax.
    pub dates: [Option<String>; 4],
    /// Short and medium time patterns.
    pub times: [Option<String>; 2],
}

/// How many decimals amounts in `code` have (ISO 4217's minor units).
fn currency_digits(code: &str) -> usize {
    match code {
        "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "UGX" | "PYG" | "XAF" | "XOF" | "RWF" => 0,
        "BHD" | "KWD" | "OMR" | "JOD" | "TND" | "LYD" | "IQD" => 3,
        _ => 2,
    }
}

/// A language tag cut into the parts Stipple uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Tag {
    language: String,
    script: Option<String>,
    region: Option<String>,
    hc: Option<HourCycle>,
    fw: Option<Weekday>,
    rg: Option<String>,
    nu_native: Option<bool>,
}

impl Tag {
    fn parse(tag: &str) -> Self {
        // POSIX: language_REGION.charset@modifier. ICU: lang_REGION@key=value;…
        let (base, modifiers) = tag.trim().split_once('@').unwrap_or((tag.trim(), ""));
        let base = base.split('.').next().unwrap_or("");
        let mut t = Tag::default();
        let mut parts = base.split(['-', '_']).filter(|s| !s.is_empty());
        let lang = parts.next().unwrap_or("").to_ascii_lowercase();
        t.language = match lang.as_str() {
            "" | "c" | "posix" | "und" | "root" => "en".into(),
            "iw" => "he".into(),
            "in" => "id".into(),
            "ji" => "yi".into(),
            l if l.len() <= 8 && l.bytes().all(|b| b.is_ascii_alphabetic()) => l.to_string(),
            _ => "en".into(),
        };
        let mut ext: Option<String> = None;
        let mut ext_key: Option<String> = None;
        for part in parts {
            let lower = part.to_ascii_lowercase();
            if let Some(kind) = &ext {
                if kind != "u" {
                    continue;
                }
                if lower.len() == 2 {
                    ext_key = Some(lower);
                } else if let Some(key) = ext_key.take() {
                    t.keyword(&key, &lower);
                }
                continue;
            }
            if lower.len() == 1 {
                ext = Some(lower);
                continue;
            }
            if t.script.is_none() && t.region.is_none() && part.len() == 4 && is_alpha(part) {
                let mut s = lower.clone();
                s[..1].make_ascii_uppercase();
                t.script = Some(s);
            } else if t.region.is_none()
                && ((part.len() == 2 && is_alpha(part))
                    || (part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit())))
            {
                t.region = Some(part.to_ascii_uppercase());
            }
        }
        for kv in modifiers.split(';') {
            if let Some((k, v)) = kv.split_once('=') {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().to_ascii_lowercase();
                let k = if k == "hours" { "hc".to_string() } else { k };
                t.keyword(&k, &v);
            }
        }
        t
    }

    /// A Unicode extension keyword (`hc`, `fw`, `rg`, `nu`).
    fn keyword(&mut self, key: &str, value: &str) {
        match key {
            "hc" => {
                self.hc = match value {
                    "h12" | "h11" => Some(HourCycle::H12),
                    "h23" | "h24" => Some(HourCycle::H23),
                    _ => self.hc,
                }
            }
            "fw" => {
                let days = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
                if let Some(i) = days.iter().position(|d| *d == value) {
                    self.fw = Some(Weekday::from_days_from_monday(i as u32));
                }
            }
            "rg" if value.len() >= 2 && is_alpha(&value[..2]) => {
                self.rg = Some(value[..2].to_ascii_uppercase());
            }
            "nu" => match value {
                "latn" => self.nu_native = Some(false),
                "arab" => self.nu_native = Some(true),
                _ => {}
            },
            _ => {}
        }
    }

    fn canonical(&self) -> String {
        let mut s = self.language.clone();
        for part in [&self.script, &self.region].into_iter().flatten() {
            s.push('-');
            s.push_str(part);
        }
        s
    }
}

fn is_alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

/// `tag` (a BCP 47 tag, a POSIX locale name such as `fr_CA.UTF-8`, or an
/// Apple or ICU identifier such as `zh_Hant_TW`) as a canonical BCP 47 tag:
/// `fr-CA`, `zh-Hant-TW`. `C` and `POSIX` are `en`.
pub fn canonical_tag(tag: &str) -> String {
    Tag::parse(tag).canonical()
}

/// The best of the `available` languages (the ones an app ships) for a user
/// who prefers `preferred`, in order: the first preferred language with an
/// exact match, else a match on the language and script, else on the
/// language alone. `None` when nothing matches; fall back to the app's own
/// language then.
///
/// ```
/// use stipple::locale::negotiate;
/// let shipped = ["en", "fr", "ja", "zh-Hant"];
/// assert_eq!(negotiate(&["fr-CA", "en-US"], &shipped), Some("fr"));
/// assert_eq!(negotiate(&["de-DE", "ja-JP"], &shipped), Some("ja"));
/// assert_eq!(negotiate(&["zh-TW"], &shipped), Some("zh-Hant"));
/// assert_eq!(negotiate(&["de"], &shipped), None);
/// ```
pub fn negotiate<'a, T: AsRef<str>>(preferred: &[T], available: &[&'a str]) -> Option<&'a str> {
    let avail: Vec<(Tag, &'a str)> = available.iter().map(|a| (Tag::parse(a), *a)).collect();
    for want in preferred {
        let want = Tag::parse(want.as_ref());
        let script = want.script.clone().or_else(|| implied_script(&want));
        // Exact language, script and region; then language and script, or
        // language and region; then the language alone.
        let found = avail
            .iter()
            .find(|(t, _)| {
                t.language == want.language && t.region == want.region && t.script == want.script
            })
            .or_else(|| {
                avail.iter().find(|(t, _)| {
                    t.language == want.language
                        && t.script.is_some()
                        && (t.script == script)
                        && (t.region.is_none() || t.region == want.region)
                })
            })
            .or_else(|| {
                avail.iter().find(|(t, _)| {
                    t.language == want.language
                        && t.region.is_none()
                        && (t.script.is_none() || t.script == script)
                })
            })
            .or_else(|| {
                avail.iter().find(|(t, _)| {
                    t.language == want.language
                        && (t.script.is_none() || script.is_none() || t.script == script)
                })
            });
        if let Some((_, a)) = found {
            return Some(a);
        }
    }
    None
}

/// The script a tag implies without saying: Traditional Chinese in Taiwan,
/// Hong Kong and Macau, Simplified elsewhere.
fn implied_script(t: &Tag) -> Option<String> {
    (t.language == "zh").then(|| {
        match t.region.as_deref() {
            Some("TW" | "HK" | "MO") => "Hant",
            _ => "Hans",
        }
        .to_string()
    })
}

/// Strings Stipple itself shows, in the user's language: the default
/// button of a message box, and the state words screen readers hear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) enum Builtin {
    Ok,
    On,
    Off,
    Checked,
    Unchecked,
    Expanded,
    Collapsed,
}

/// `what` in `language` (a lowercase language subtag; `hant` for
/// Traditional Chinese), English for a language Stipple doesn't have.
pub(crate) fn builtin_in(language: &str, hant: bool, what: Builtin) -> &'static str {
    let row: [&str; 7] = match language {
        "fr" => [
            "OK",
            "activé",
            "désactivé",
            "coché",
            "non coché",
            "développé",
            "réduit",
        ],
        "de" => [
            "OK",
            "ein",
            "aus",
            "aktiviert",
            "nicht aktiviert",
            "erweitert",
            "reduziert",
        ],
        "es" => [
            "Aceptar",
            "activado",
            "desactivado",
            "marcado",
            "no marcado",
            "expandido",
            "contraído",
        ],
        "it" => [
            "OK",
            "attivo",
            "disattivo",
            "selezionato",
            "non selezionato",
            "espanso",
            "compresso",
        ],
        "pt" => [
            "OK",
            "ativado",
            "desativado",
            "marcado",
            "desmarcado",
            "expandido",
            "recolhido",
        ],
        "nl" => [
            "OK",
            "aan",
            "uit",
            "aangevinkt",
            "niet aangevinkt",
            "uitgevouwen",
            "samengevouwen",
        ],
        "sv" => [
            "OK",
            "på",
            "av",
            "markerad",
            "inte markerad",
            "expanderad",
            "komprimerad",
        ],
        "ru" => [
            "ОК",
            "вкл.",
            "выкл.",
            "отмечено",
            "не отмечено",
            "развернуто",
            "свернуто",
        ],
        "uk" => [
            "OK",
            "увімкнено",
            "вимкнено",
            "позначено",
            "не позначено",
            "розгорнуто",
            "згорнуто",
        ],
        "pl" => [
            "OK",
            "włączone",
            "wyłączone",
            "zaznaczone",
            "niezaznaczone",
            "rozwinięte",
            "zwinięte",
        ],
        "cs" => [
            "OK",
            "zapnuto",
            "vypnuto",
            "zaškrtnuto",
            "nezaškrtnuto",
            "rozbaleno",
            "sbaleno",
        ],
        "tr" => [
            "Tamam",
            "açık",
            "kapalı",
            "işaretli",
            "işaretsiz",
            "genişletilmiş",
            "daraltılmış",
        ],
        "ja" => [
            "OK",
            "オン",
            "オフ",
            "チェックあり",
            "チェックなし",
            "展開",
            "折りたたみ",
        ],
        "zh" if hant => [
            "確定",
            "開啟",
            "關閉",
            "已勾選",
            "未勾選",
            "已展開",
            "已收合",
        ],
        "zh" => ["确定", "开", "关", "已选中", "未选中", "已展开", "已折叠"],
        "ko" => ["확인", "켬", "끔", "선택됨", "선택 안 됨", "펼쳐짐", "접힘"],
        "ar" => ["حسنًا", "تشغيل", "إيقاف", "محدد", "غير محدد", "موسّع", "مطوي"],
        "he" => [
            "אישור",
            "פועל",
            "כבוי",
            "מסומן",
            "לא מסומן",
            "מורחב",
            "מכווץ",
        ],
        _ => [
            "OK",
            "on",
            "off",
            "checked",
            "unchecked",
            "expanded",
            "collapsed",
        ],
    };
    row[what as usize]
}

/// `what` in the user's language (the first preferred one).
pub(crate) fn builtin(what: Builtin) -> &'static str {
    let first = crate::platform::preferred_languages();
    let tag = Tag::parse(first.first().map_or("en", String::as_str));
    let script = tag.script.clone().or_else(|| implied_script(&tag));
    let hant = script.as_deref() == Some("Hant");
    builtin_in(&tag.language, hant, what)
}

#[cfg(test)]
mod tests;
