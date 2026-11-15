//! CLDR's cardinal plural rules: which form of a word a number takes ("1
//! item", "2 items"; Russian's "1 файл", "2 файла", "5 файлов").

/// The plural category a number falls in, in a language: CLDR's six.
/// Every language uses [`Other`](Self::Other); English uses `One` too,
/// Arabic all six.
///
/// ```
/// use stipple::locale::{Locale, PluralCategory};
/// assert_eq!(Locale::new("en").plural(1.0), PluralCategory::One);
/// assert_eq!(Locale::new("en").plural(2.0), PluralCategory::Other);
/// assert_eq!(Locale::new("ru").plural(3.0), PluralCategory::Few);
/// assert_eq!(Locale::new("ru").plural(5.0), PluralCategory::Many);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

impl PluralCategory {
    /// The category's CLDR name: `"zero"`, `"one"`, `"two"`, `"few"`,
    /// `"many"` or `"other"` (what a [`Catalog`](super::Catalog) key ends in).
    pub fn name(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Two => "two",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }
}

/// CLDR's plural operands of a decimal number as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Operands {
    /// The integer digits (`i`).
    i: u64,
    /// The number of visible fraction digits, trailing zeros included (`v`).
    v: u32,
    /// The visible fraction digits as an integer (`f`).
    f: u64,
    /// The same without trailing zeros (`t`).
    t: u64,
}

impl Operands {
    /// The operands of a number written as `text` (`"1"`, `"1.50"`,
    /// `"-3"`); the sign is ignored.
    pub(crate) fn parse(text: &str) -> Self {
        let text = text.trim().trim_start_matches(['-', '+']);
        let (int, frac) = text.split_once('.').unwrap_or((text, ""));
        let digits = |s: &str| {
            s.bytes().filter(u8::is_ascii_digit).fold(0u64, |acc, d| {
                acc.wrapping_mul(10).wrapping_add(u64::from(d - b'0'))
            })
        };
        let frac: String = frac.chars().filter(char::is_ascii_digit).take(18).collect();
        let trimmed = frac.trim_end_matches('0');
        Self {
            i: digits(int),
            v: frac.len() as u32,
            f: digits(&frac),
            t: digits(trimmed),
        }
    }

    /// The operands of `n` as Rust writes it (shortest round trip: `1.5`, `2`).
    pub(crate) fn from_f64(n: f64) -> Self {
        if !n.is_finite() {
            return Self::parse("0.1"); // "other" everywhere but where 0.1 isn't
        }
        Self::parse(&format!("{}", n.abs()))
    }

    /// `n` when it is a whole number (no nonzero fraction digit).
    fn n(&self) -> Option<u64> {
        (self.f == 0).then_some(self.i)
    }
}

fn within(x: Option<u64>, lo: u64, hi: u64) -> bool {
    x.is_some_and(|x| (lo..=hi).contains(&x))
}

/// The category of `o` in `lang` (a lowercase primary language subtag;
/// `region` tells European Portuguese from Brazilian).
pub(crate) fn category(lang: &str, region: Option<&str>, o: Operands) -> PluralCategory {
    use PluralCategory::*;
    let Operands { i, v, f, t } = o;
    let n = o.n();
    let n_is = |k: u64| n == Some(k);
    // fr, es, it, pt, ca: "1 million de", "1 millón de".
    let millions = || v == 0 && i != 0 && i % 1_000_000 == 0;
    match lang {
        "ja" | "zh" | "ko" | "vi" | "th" | "id" | "ms" | "lo" | "my" | "km" | "yue" | "bo"
        | "jv" | "su" | "yo" | "ig" | "to" | "wo" | "ii" | "sah" => Other,
        "en" | "de" | "nl" | "sv" | "fi" | "et" | "ur" | "sw" | "gl" | "nn" | "yi" | "ji"
        | "fy" | "ia" | "io" | "lb" | "sc" => {
            if i == 1 && v == 0 {
                One
            } else {
                Other
            }
        }
        "it" | "ca" => {
            if i == 1 && v == 0 {
                One
            } else if millions() {
                Many
            } else {
                Other
            }
        }
        "pt" if region == Some("PT") => {
            if i == 1 && v == 0 {
                One
            } else if millions() {
                Many
            } else {
                Other
            }
        }
        "fr" | "pt" => {
            if i <= 1 {
                One
            } else if millions() {
                Many
            } else {
                Other
            }
        }
        "es" => {
            if n_is(1) {
                One
            } else if millions() {
                Many
            } else {
                Other
            }
        }
        "tr" | "el" | "hu" | "nb" | "no" | "bg" | "az" | "ka" | "kk" | "ky" | "ta" | "te"
        | "ml" | "uz" | "eu" | "sq" | "mn" | "ne" | "ps" | "tk" | "ug" | "af" | "or" => {
            if n_is(1) {
                One
            } else {
                Other
            }
        }
        "da" => {
            if n_is(1) || (t != 0 && i <= 1) {
                One
            } else {
                Other
            }
        }
        "is" => {
            if (t == 0 && i % 10 == 1 && i % 100 != 11) || (t % 10 == 1 && t % 100 != 11) {
                One
            } else {
                Other
            }
        }
        "hi" | "bn" | "fa" | "gu" | "kn" | "zu" | "am" | "as" => {
            if i == 0 || n_is(1) {
                One
            } else {
                Other
            }
        }
        "mr" => {
            if n_is(1) {
                One
            } else {
                Other
            }
        }
        "ru" | "uk" => {
            if v != 0 {
                Other
            } else if i % 10 == 1 && i % 100 != 11 {
                One
            } else if (2..=4).contains(&(i % 10)) && !(12..=14).contains(&(i % 100)) {
                Few
            } else {
                Many
            }
        }
        "be" => {
            let m10 = n.map(|n| n % 10);
            let m100 = n.map(|n| n % 100);
            if m10 == Some(1) && m100 != Some(11) {
                One
            } else if within(m10, 2, 4) && !within(m100, 12, 14) {
                Few
            } else if m10 == Some(0) || within(m10, 5, 9) || within(m100, 11, 14) {
                Many
            } else {
                Other
            }
        }
        "pl" => {
            if v != 0 {
                Other
            } else if i == 1 {
                One
            } else if (2..=4).contains(&(i % 10)) && !(12..=14).contains(&(i % 100)) {
                Few
            } else {
                Many
            }
        }
        "cs" | "sk" => {
            if v != 0 {
                Many
            } else if i == 1 {
                One
            } else if (2..=4).contains(&i) {
                Few
            } else {
                Other
            }
        }
        "hr" | "sr" | "bs" | "sh" => {
            let ends_one = |x: u64| x % 10 == 1 && x % 100 != 11;
            let ends_few = |x: u64| (2..=4).contains(&(x % 10)) && !(12..=14).contains(&(x % 100));
            if (v == 0 && ends_one(i)) || ends_one(f) {
                One
            } else if (v == 0 && ends_few(i)) || ends_few(f) {
                Few
            } else {
                Other
            }
        }
        "sl" => {
            if v == 0 && i % 100 == 1 {
                One
            } else if v == 0 && i % 100 == 2 {
                Two
            } else if (v == 0 && (3..=4).contains(&(i % 100))) || v != 0 {
                Few
            } else {
                Other
            }
        }
        "lt" => {
            let m10 = n.map(|n| n % 10);
            let teen = within(n.map(|n| n % 100), 11, 19);
            if m10 == Some(1) && !teen {
                One
            } else if within(m10, 2, 9) && !teen {
                Few
            } else if f != 0 {
                Many
            } else {
                Other
            }
        }
        "lv" => {
            let m10 = n.map(|n| n % 10);
            let m100 = n.map(|n| n % 100);
            if m10 == Some(0) || within(m100, 11, 19) || (v == 2 && (11..=19).contains(&(f % 100)))
            {
                Zero
            } else if (m10 == Some(1) && m100 != Some(11))
                || (v == 2 && f % 10 == 1 && f % 100 != 11)
                || (v != 2 && f % 10 == 1)
            {
                One
            } else {
                Other
            }
        }
        "ro" | "mo" => {
            if i == 1 && v == 0 {
                One
            } else if v != 0 || n_is(0) || (n != Some(1) && within(n.map(|n| n % 100), 2, 19)) {
                Few
            } else {
                Other
            }
        }
        "ar" => {
            let m100 = n.map(|n| n % 100);
            if n_is(0) {
                Zero
            } else if n_is(1) {
                One
            } else if n_is(2) {
                Two
            } else if within(m100, 3, 10) {
                Few
            } else if within(m100, 11, 99) {
                Many
            } else {
                Other
            }
        }
        "he" | "iw" => {
            if (i == 1 && v == 0) || (i == 0 && v != 0) {
                One
            } else if i == 2 && v == 0 {
                Two
            } else {
                Other
            }
        }
        "ga" => {
            if n_is(1) {
                One
            } else if n_is(2) {
                Two
            } else if within(n, 3, 6) {
                Few
            } else if within(n, 7, 10) {
                Many
            } else {
                Other
            }
        }
        "cy" => match n {
            Some(0) => Zero,
            Some(1) => One,
            Some(2) => Two,
            Some(3) => Few,
            Some(6) => Many,
            _ => Other,
        },
        "fil" | "tl" => {
            let bad = |x: u64| matches!(x % 10, 4 | 6 | 9);
            if (v == 0 && (1..=3).contains(&i)) || (v == 0 && !bad(i)) || (v != 0 && !bad(f)) {
                One
            } else {
                Other
            }
        }
        // English's rule is the commonest; a language without one of its own
        // gets it.
        _ => {
            if i == 1 && v == 0 {
                One
            } else {
                Other
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PluralCategory::*;
    use super::*;

    fn cat(lang: &str, n: &str) -> PluralCategory {
        category(lang, None, Operands::parse(n))
    }

    #[test]
    fn operands() {
        let o = Operands::parse("-1.50");
        assert_eq!((o.i, o.v, o.f, o.t), (1, 2, 50, 5));
        assert_eq!(Operands::from_f64(2.0), Operands::parse("2"));
        assert_eq!(Operands::from_f64(0.25), Operands::parse("0.25"));
    }

    #[test]
    fn english_and_german() {
        for lang in ["en", "de", "nl", "sv"] {
            assert_eq!(cat(lang, "1"), One);
            assert_eq!(cat(lang, "0"), Other);
            assert_eq!(cat(lang, "2"), Other);
            assert_eq!(cat(lang, "1.0"), Other); // "1.0 items"
        }
    }

    #[test]
    fn french_portuguese_spanish() {
        assert_eq!(cat("fr", "0"), One);
        assert_eq!(cat("fr", "1.5"), One);
        assert_eq!(cat("fr", "2"), Other);
        assert_eq!(cat("fr", "1000000"), Many);
        assert_eq!(cat("pt", "0"), One);
        assert_eq!(category("pt", Some("PT"), Operands::parse("0")), Other);
        assert_eq!(cat("es", "1"), One);
        assert_eq!(cat("es", "0"), Other);
        assert_eq!(cat("es", "2000000"), Many);
        assert_eq!(cat("it", "1"), One);
    }

    #[test]
    fn slavic() {
        for lang in ["ru", "uk"] {
            assert_eq!(cat(lang, "1"), One);
            assert_eq!(cat(lang, "21"), One);
            assert_eq!(cat(lang, "11"), Many);
            assert_eq!(cat(lang, "2"), Few);
            assert_eq!(cat(lang, "24"), Few);
            assert_eq!(cat(lang, "12"), Many);
            assert_eq!(cat(lang, "5"), Many);
            assert_eq!(cat(lang, "0"), Many);
            assert_eq!(cat(lang, "1.5"), Other);
        }
        assert_eq!(cat("pl", "1"), One);
        assert_eq!(cat("pl", "22"), Few);
        assert_eq!(cat("pl", "21"), Many);
        assert_eq!(cat("pl", "1.5"), Other);
        assert_eq!(cat("cs", "1"), One);
        assert_eq!(cat("cs", "3"), Few);
        assert_eq!(cat("cs", "5"), Other);
        assert_eq!(cat("cs", "1.5"), Many);
    }

    #[test]
    fn arabic_and_hebrew() {
        let ar: Vec<_> = ["0", "1", "2", "3", "11", "100", "102", "111"]
            .iter()
            .map(|n| cat("ar", n))
            .collect();
        assert_eq!(ar, [Zero, One, Two, Few, Many, Other, Other, Many]);
        assert_eq!(cat("he", "1"), One);
        assert_eq!(cat("he", "2"), Two);
        assert_eq!(cat("he", "3"), Other);
        assert_eq!(cat("he", "20"), Other);
    }

    #[test]
    fn east_asian_and_turkish() {
        for lang in ["ja", "zh", "ko"] {
            assert_eq!(cat(lang, "1"), Other);
        }
        assert_eq!(cat("tr", "1"), One);
        assert_eq!(cat("tr", "2"), Other);
    }
}
