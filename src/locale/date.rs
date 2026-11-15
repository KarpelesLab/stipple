//! Calendar dates and times of day, and the CLDR date patterns that format
//! them.

use super::Locale;

/// A day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// The days from Monday to Sunday.
    pub const ALL: [Weekday; 7] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ];

    /// Days since Monday: 0 for Monday, 6 for Sunday.
    pub fn days_from_monday(self) -> u8 {
        self as u8
    }

    /// The day `n` days after Monday (wrapping every week).
    pub fn from_days_from_monday(n: u32) -> Weekday {
        Self::ALL[(n % 7) as usize]
    }

    /// The day after this one.
    pub fn succ(self) -> Weekday {
        Self::from_days_from_monday(u32::from(self.days_from_monday()) + 1)
    }
}

/// How much of a date to show: `11/16/26`, `Nov 16, 2026`,
/// `November 16, 2026`, `Monday, November 16, 2026` in English.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DateStyle {
    /// Numbers only.
    Short,
    /// An abbreviated month name.
    #[default]
    Medium,
    /// The month's full name.
    Long,
    /// The weekday too.
    Full,
}

/// How much of a time to show: `9:05 PM` or `9:05:30 PM` in English.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TimeStyle {
    /// Hours and minutes.
    #[default]
    Short,
    /// Seconds too.
    Medium,
}

/// Whether clocks show 12 or 24 hours. It follows the region (12 in the
/// United States, 24 in France), and the user can override it on every
/// desktop and phone OS; [`platform::locale`](crate::platform::locale) reads
/// that setting where the OS has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HourCycle {
    /// 1 to 12, with AM and PM: `9:05 PM`.
    H12,
    /// 0 to 23: `21:05`.
    H23,
}

/// A date and a time of day on the Gregorian calendar, in no particular time
/// zone: what a clock on the wall and a calendar show. Make one from Unix
/// time with [`from_unix_local`](Self::from_unix_local) (the user's time
/// zone) or [`from_unix_utc`](Self::from_unix_utc), and format it with
/// [`Locale::format_date`] and friends.
///
/// ```
/// use stipple::locale::{DateTime, Weekday};
/// let moon = DateTime::from_unix_utc(-14_182_940);
/// assert_eq!((moon.year, moon.month, moon.day), (1969, 7, 20));
/// assert_eq!((moon.hour, moon.minute, moon.second), (20, 17, 40));
/// assert_eq!(moon.weekday(), Weekday::Sunday);
/// let new_year = DateTime::new(2027, 1, 1);
/// assert_eq!(new_year.to_unix_utc(), 1_798_761_600);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    /// 0 to 23.
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Midnight at the start of a day.
    pub fn new(year: i32, month: u8, day: u8) -> Self {
        Self {
            year,
            month,
            day,
            ..Self::default()
        }
    }

    /// The same day at `hour:minute`.
    pub fn at(self, hour: u8, minute: u8) -> Self {
        Self {
            hour,
            minute,
            second: 0,
            ..self
        }
    }

    /// The date and time in UTC `secs` seconds after the Unix epoch.
    pub fn from_unix_utc(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (rem / 3600) as u8,
            minute: (rem / 60 % 60) as u8,
            second: (rem % 60) as u8,
        }
    }

    /// The date and time `secs` seconds after the Unix epoch in the user's
    /// time zone (the OS's; UTC where it has none).
    pub fn from_unix_local(secs: i64) -> Self {
        Self::from_unix_utc(secs + i64::from(crate::platform::utc_offset(secs)))
    }

    /// The date and time now, in the user's time zone.
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self::from_unix_local(secs)
    }

    /// Seconds since the Unix epoch, reading this as a UTC time.
    pub fn to_unix_utc(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }

    /// The day of the week.
    pub fn weekday(&self) -> Weekday {
        // 1970-01-01 was a Thursday, 3 days after a Monday.
        let days = days_from_civil(self.year, self.month, self.day);
        Weekday::from_days_from_monday((days + 3).rem_euclid(7) as u32)
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let (m, d) = (i64::from(m.clamp(1, 12)), i64::from(d));
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` days after 1970-01-01.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m, d)
}

/// One piece of a CLDR date pattern.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    /// Text copied as it is.
    Literal(String),
    /// A field: its letter and how many times it's repeated.
    Field(char, usize),
}

/// Cut a CLDR date pattern into fields and literal text. Letters A–Z and
/// a–z are fields (repeated for width), `'…'` is literal text (`''` a
/// quote), everything else is copied.
fn tokens(pattern: &str) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                lit.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        lit.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                lit.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if !c.is_ascii_alphabetic() {
            lit.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] == c {
            i += 1;
        }
        if !lit.is_empty() {
            out.push(Token::Literal(std::mem::take(&mut lit)));
        }
        out.push(Token::Field(c, i - start));
    }
    if !lit.is_empty() {
        out.push(Token::Literal(lit));
    }
    out
}

/// Write tokens back as a pattern, quoting literal letters.
fn untokens(tokens: &[Token]) -> String {
    let mut out = String::new();
    for t in tokens {
        match t {
            Token::Field(c, n) => out.extend(std::iter::repeat_n(*c, *n)),
            Token::Literal(s) if s.contains(|c: char| c.is_ascii_alphabetic() || c == '\'') => {
                out.push('\'');
                out.push_str(&s.replace('\'', "''"));
                out.push('\'');
            }
            Token::Literal(s) => out.push_str(s),
        }
    }
    out
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{202f}')
}

/// Format `dt` by a CLDR date pattern. Fields Stipple doesn't know (eras,
/// time zones) are left out with the space before them.
pub(crate) fn format_pattern(locale: &Locale, dt: &DateTime, pattern: &str) -> String {
    let mut out = String::new();
    for token in tokens(pattern) {
        match token {
            Token::Literal(s) => out.push_str(&s),
            Token::Field(c, width) => match field(locale, dt, c, width) {
                Some(text) => out.push_str(&text),
                None => {
                    while out.ends_with(is_space) {
                        out.pop();
                    }
                }
            },
        }
    }
    out.trim_matches(is_space).to_string()
}

fn field(locale: &Locale, dt: &DateTime, c: char, width: usize) -> Option<String> {
    let num = |n: i64, pad: usize| locale.digits(&format!("{n:0pad$}"));
    let hour12 = |h: u8| {
        if h.is_multiple_of(12) {
            12
        } else {
            i64::from(h % 12)
        }
    };
    Some(match c {
        'y' | 'u' | 'Y' => {
            if width == 2 {
                num(i64::from(dt.year).rem_euclid(100), 2)
            } else {
                num(i64::from(dt.year), width)
            }
        }
        'M' | 'L' => {
            let m = dt.month.clamp(1, 12);
            match width {
                1 | 2 => num(i64::from(m), width),
                3 => locale.month_abbr(m).to_string(),
                _ => locale.month_name(m).to_string(),
            }
        }
        'd' => num(i64::from(dt.day), width.min(2)),
        'E' | 'c' | 'e' => {
            let w = dt.weekday();
            match width {
                1 | 2 if c != 'E' => num(i64::from(w.days_from_monday()) + 1, width),
                4 => locale.weekday_name(w).to_string(),
                _ => locale.weekday_abbr(w).to_string(),
            }
        }
        'a' | 'b' | 'B' => locale.am_pm(dt.hour).to_string(),
        'h' => num(hour12(dt.hour), width.min(2)),
        'H' => num(i64::from(dt.hour), width.min(2)),
        'K' => num(i64::from(dt.hour % 12), width.min(2)),
        'k' => num(
            if dt.hour == 0 { 24 } else { i64::from(dt.hour) },
            width.min(2),
        ),
        'm' => num(i64::from(dt.minute), width.min(2)),
        's' => num(i64::from(dt.second), width.min(2)),
        _ => return None,
    })
}

/// A Windows date or time picture (`dddd, MMMM d, yyyy`, `h:mm tt`) in CLDR's
/// syntax (`EEEE, MMMM d, y`, `h:mm a`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn from_windows_picture(picture: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = picture.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // Quoted text is quoted the same way.
            let end = chars[i + 1..]
                .iter()
                .position(|&q| q == '\'')
                .map_or(chars.len(), |p| i + 1 + p);
            out.extend(&chars[i..(end + 1).min(chars.len())]);
            i = end + 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] == c {
            i += 1;
        }
        let n = i - start;
        match c {
            'd' if n >= 3 => out.push_str(if n == 3 { "EEE" } else { "EEEE" }),
            'y' if n >= 3 => out.push('y'),
            't' => out.push('a'),
            'g' => {}
            _ => out.extend(std::iter::repeat_n(c, n)),
        }
    }
    out
}

/// Turn a time pattern to the other clock: `h:mm a` ↔ `HH:mm`.
pub(crate) fn to_hour_cycle(pattern: &str, cycle: HourCycle) -> String {
    let toks = tokens(pattern);
    let has = |set: &[char]| {
        toks.iter()
            .any(|t| matches!(t, Token::Field(c, _) if set.contains(c)))
    };
    match cycle {
        HourCycle::H23 if has(&['h', 'K']) => {
            let mut out: Vec<Token> = Vec::new();
            for t in toks {
                match t {
                    Token::Field('h' | 'K', n) => out.push(Token::Field('H', n)),
                    // The AM/PM marker goes, with the space before it…
                    Token::Field('a' | 'b' | 'B', _) => {
                        if let Some(Token::Literal(s)) = out.last_mut() {
                            let trimmed = s.trim_end_matches(is_space).len();
                            s.truncate(trimmed);
                            if s.is_empty() {
                                out.pop();
                            }
                        }
                    }
                    // …or after it, when it comes first.
                    Token::Literal(s) if out.is_empty() => {
                        let s = s.trim_start_matches(is_space);
                        if !s.is_empty() {
                            out.push(Token::Literal(s.to_string()));
                        }
                    }
                    t => out.push(t),
                }
            }
            untokens(&out).trim_matches(is_space).to_string()
        }
        HourCycle::H12 if has(&['H', 'k']) => {
            let out: Vec<Token> = toks
                .into_iter()
                .map(|t| match t {
                    Token::Field('H' | 'k', _) => Token::Field('h', 1),
                    t => t,
                })
                .collect();
            format!("{} a", untokens(&out))
        }
        _ => pattern.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-1_000_000i64, -1, 0, 1, 10_957, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(DateTime::new(2026, 11, 16).weekday(), Weekday::Monday);
        assert_eq!(DateTime::new(2000, 1, 1).weekday(), Weekday::Saturday);
        assert_eq!(DateTime::from_unix_utc(-1).second, 59);
    }

    #[test]
    fn windows_pictures() {
        assert_eq!(
            from_windows_picture("dddd, MMMM d, yyyy"),
            "EEEE, MMMM d, y"
        );
        assert_eq!(from_windows_picture("h:mm tt"), "h:mm a");
        assert_eq!(from_windows_picture("dd/MM/yyyy"), "dd/MM/y");
        assert_eq!(from_windows_picture("d' de 'MMMM"), "d' de 'MMMM");
    }

    #[test]
    fn hour_cycles() {
        assert_eq!(to_hour_cycle("h:mm a", HourCycle::H23), "H:mm");
        assert_eq!(to_hour_cycle("a h:mm", HourCycle::H23), "H:mm");
        assert_eq!(to_hour_cycle("HH:mm", HourCycle::H12), "h:mm a");
        assert_eq!(to_hour_cycle("HH:mm", HourCycle::H23), "HH:mm");
        assert_eq!(to_hour_cycle("aK:mm", HourCycle::H23), "H:mm");
        assert_eq!(to_hour_cycle("HH 'h' mm", HourCycle::H12), "h' h 'mm a");
        assert_eq!(
            tokens("d 'de' MMMM"),
            [
                Token::Field('d', 1),
                Token::Literal(" de ".into()),
                Token::Field('M', 4)
            ]
        );
    }
}
