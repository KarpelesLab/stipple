//! A message catalog: an app's strings in one language, with their plural
//! forms, loaded from a small text format.

use super::Locale;
use std::collections::HashMap;

/// An app's strings in one language: keys to templates with `{name}`
/// placeholders, and plural forms chosen by CLDR's rules.
///
/// The text format is one `key = template` per line; `#` starts a comment
/// line, `\n` is a line break, `\\` a backslash, `{{` and `}}` are braces.
/// A plural message has one key per category the language uses, named
/// `key.one`, `key.few`, `key.other`… (see
/// [`PluralCategory`](super::PluralCategory)); `{count}` is the number,
/// formatted for the locale.
///
/// ```
/// use stipple::locale::Catalog;
///
/// let fr = Catalog::parse("fr", "
///     title = Mes notes
///     greeting = Bonjour, {name} !
///     notes.one = {count} note
///     notes.other = {count} notes
/// ").unwrap();
/// assert_eq!(fr.text("title"), "Mes notes");
/// assert_eq!(fr.format("greeting", &[("name", "Ana")]), "Bonjour, Ana !");
/// assert_eq!(fr.plural("notes", 1.0, &[]), "1 note");
/// assert_eq!(fr.plural("notes", 0.0, &[]), "0 note"); // French: 0 is singular
/// assert_eq!(fr.plural("notes", 1500.0, &[]), "1\u{202f}500 notes");
/// ```
///
/// Pick the catalog with [`negotiate`](super::negotiate), and give it the
/// app's own language as a [`fallback`](Self::fallback) for keys a
/// translation doesn't have yet. A key found nowhere comes back as is, so a
/// missing string shows up in the UI rather than an empty label.
#[derive(Clone, Debug)]
pub struct Catalog {
    /// Plural rules: the catalog's language's.
    rules: Locale,
    /// How `{count}` is written.
    locale: Locale,
    entries: HashMap<String, String>,
    fallback: Option<Box<Catalog>>,
}

/// A line of a catalog that isn't `key = template`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CatalogError {
    /// The line, counting from 1.
    pub line: usize,
    /// What's wrong with it.
    pub message: String,
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "catalog line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for CatalogError {}

impl Catalog {
    /// An empty catalog for `language` (a language tag: `"fr"`, `"pt-BR"`).
    pub fn new(language: &str) -> Self {
        let locale = Locale::new(language);
        Self {
            rules: locale.clone(),
            locale,
            entries: HashMap::new(),
            fallback: None,
        }
    }

    /// A catalog for `language` from `text` (see the [type docs](Self)).
    pub fn parse(language: &str, text: &str) -> Result<Self, CatalogError> {
        let mut catalog = Self::new(language);
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let error = |message: &str| CatalogError {
                line: i + 1,
                message: message.into(),
            };
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| error("expected `key = template`"))?;
            let key = key.trim();
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                return Err(error("a key is letters, digits, `_`, `-` and `.`"));
            }
            catalog
                .entries
                .insert(key.to_string(), unescape(value.trim()));
        }
        Ok(catalog)
    }

    /// Look keys this catalog doesn't have up in `other` (the app's own
    /// language, usually).
    pub fn fallback(mut self, other: Catalog) -> Self {
        self.fallback = Some(Box::new(other));
        self
    }

    /// Add or replace one template.
    pub fn insert(&mut self, key: impl Into<String>, template: impl Into<String>) {
        self.entries.insert(key.into(), template.into());
    }

    /// The catalog's language tag.
    pub fn language(&self) -> &str {
        self.rules.tag()
    }

    /// The locale `{count}` is written in.
    pub fn locale(&self) -> &Locale {
        &self.locale
    }

    /// Write `{count}` in `locale`: the user's
    /// ([`platform::locale`](crate::platform::locale)) when its language is
    /// the catalog's, for their separators. Plural forms keep following the
    /// catalog's language.
    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }

    /// Whether this catalog (or its fallback) has `key`.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key) || self.fallback.as_ref().is_some_and(|f| f.contains(key))
    }

    /// The template for `key`, as it is; `key` itself when there is none.
    pub fn text(&self, key: &str) -> String {
        self.format(key, &[])
    }

    /// The template for `key` with each `{name}` replaced by its value in
    /// `args`. Placeholders without a value stay as they are.
    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        match self.lookup(key) {
            Some(template) => fill(template, args),
            None => key.to_string(),
        }
    }

    /// The plural form of `key` for `n` (`key.one`, `key.other`…, falling
    /// back to `key.other`, then to `key`), with `{count}` the number written
    /// for the locale and the other placeholders from `args`.
    pub fn plural(&self, key: &str, n: f64, args: &[(&str, &str)]) -> String {
        match self.lookup_plural(key, n) {
            Some((template, catalog)) => {
                let count = catalog.locale.format_number(n);
                let mut all: Vec<(&str, &str)> = Vec::with_capacity(args.len() + 1);
                all.push(("count", &count));
                all.extend_from_slice(args);
                fill(template, &all)
            }
            None => key.to_string(),
        }
    }

    fn lookup(&self, key: &str) -> Option<&str> {
        self.entries
            .get(key)
            .map(String::as_str)
            .or_else(|| self.fallback.as_ref()?.lookup(key))
    }

    /// The template for `n` and the catalog it came from (whose rules chose it).
    fn lookup_plural(&self, key: &str, n: f64) -> Option<(&str, &Catalog)> {
        let category = self.rules.plural(n).name();
        let own = [
            format!("{key}.{category}"),
            format!("{key}.other"),
            key.into(),
        ]
        .iter()
        .find_map(|k| self.entries.get(k));
        match own {
            Some(t) => Some((t.as_str(), self)),
            None => self.fallback.as_ref()?.lookup_plural(key, n),
        }
    }
}

/// A template's text with `\n`, `\t` and `\\` read.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// `template` with `{name}`s replaced from `args` and `{{`/`}}` as braces.
fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            let name = tail[1..end].trim();
            match args.iter().find(|(k, _)| *k == name) {
                Some((_, v)) => out.push_str(v),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_escapes() {
        assert_eq!(fill("a {x} b {y}", &[("x", "1")]), "a 1 b {y}");
        assert_eq!(fill("{{x}} {x}", &[("x", "1")]), "{x} 1");
        assert_eq!(fill("open {", &[]), "open {");
        assert_eq!(unescape(r"a\nb\\c"), "a\nb\\c");
    }

    #[test]
    fn errors_name_the_line() {
        let e = Catalog::parse("en", "ok = fine\nnot a pair\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(Catalog::parse("en", "bad key = x").is_err());
    }

    #[test]
    fn plural_forms_and_fallbacks() {
        let en = Catalog::parse(
            "en",
            "files.one = {count} file\nfiles.other = {count} files\nquit = Quit",
        )
        .unwrap();
        let ru = Catalog::parse(
            "ru",
            "files.one = {count} файл\nfiles.few = {count} файла\nfiles.many = {count} файлов\nfiles.other = {count} файла",
        )
        .unwrap()
        .fallback(en.clone());
        assert_eq!(ru.plural("files", 1.0, &[]), "1 файл");
        assert_eq!(ru.plural("files", 3.0, &[]), "3 файла");
        assert_eq!(ru.plural("files", 5.0, &[]), "5 файлов");
        assert_eq!(ru.plural("files", 1.5, &[]), "1,5 файла");
        assert_eq!(ru.text("quit"), "Quit");
        assert_eq!(ru.text("missing"), "missing");
        assert_eq!(en.plural("files", 1.0, &[]), "1 file");
        assert_eq!(en.plural("files", 1200.0, &[]), "1,200 files");
        // Japanese has one form: `.other`.
        let ja = Catalog::parse("ja", "files.other = {count} 個のファイル").unwrap();
        assert_eq!(ja.plural("files", 1.0, &[]), "1 個のファイル");
    }
}
