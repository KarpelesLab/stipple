//! Localization: the user's preferred languages, numbers, money, dates and
//! times written for their locale, plural forms from a message catalog, and
//! a right-to-left layout for Arabic and Hebrew.
//!
//! "System" shows what `platform::locale()` reads from the OS (in the
//! browser, `navigator.languages` and `Intl`). The other buttons put a
//! language ahead of the user's with `platform::set_preferred_languages`,
//! as an app's own language setting would: the views are built again, the
//! UI mirrors for Arabic and Hebrew, and the catalog's language follows.
//!
//! ```sh
//! cargo run --example localedemo
//! ```

use stipple::locale::{Catalog, DateStyle, DateTime, HourCycle, Locale, TimeStyle, negotiate};
use stipple::platform;
use stipple::prelude::*;

/// The languages the buttons offer: a tag and its name in itself.
const LANGUAGES: &[(&str, &str)] = &[
    ("en-US", "English (US)"),
    ("en-GB", "English (UK)"),
    ("fr-FR", "Français"),
    ("de-DE", "Deutsch"),
    ("es-MX", "Español (MX)"),
    ("ru-RU", "Русский"),
    ("ja-JP", "日本語"),
    ("ar-EG", "العربية"),
    ("he-IL", "עברית"),
];

/// The app's strings: English, and translations of some of them. A key a
/// translation lacks comes from the English catalog.
const EN: &str = "
title = Localization
preferred = Preferred languages: {list}
formats = Formats: {tag}, {clock}, weeks start on {day}
clock.12 = 12-hour clock
clock.24 = 24-hour clock
system = System
number = Number
percent = Percent
money = Money
date.short = Short date
date.medium = Medium date
date.long = Long date
date.full = Full date
time = Time
notes.one = {count} note
notes.other = {count} notes
";

const FR: &str = "
title = Localisation
preferred = Langues préférées : {list}
formats = Formats : {tag}, {clock}, la semaine commence le {day}
clock.12 = horloge sur 12 heures
clock.24 = horloge sur 24 heures
system = Système
number = Nombre
percent = Pourcentage
money = Montant
date.short = Date courte
date.medium = Date moyenne
date.long = Date longue
date.full = Date complète
time = Heure
notes.one = {count} note
notes.other = {count} notes
";

const RU: &str = "
title = Локализация
number = Число
money = Сумма
time = Время
notes.one = {count} заметка
notes.few = {count} заметки
notes.many = {count} заметок
notes.other = {count} заметки
";

const JA: &str = "
title = ローカライズ
number = 数値
money = 金額
time = 時刻
notes.other = {count} 件のメモ
";

const AR: &str = "
title = الترجمة
number = رقم
money = مبلغ
time = الوقت
notes.zero = لا ملاحظات
notes.one = ملاحظة واحدة
notes.two = ملاحظتان
notes.few = {count} ملاحظات
notes.many = {count} ملاحظة
notes.other = {count} ملاحظة
";

const HE: &str = "
title = לוקליזציה
number = מספר
money = סכום
time = שעה
notes.one = פתק אחד
notes.two = שני פתקים
notes.other = {count} פתקים
";

/// The languages the catalog ships.
const SHIPPED: &[&str] = &["en", "fr", "ru", "ja", "ar", "he"];

fn catalog(language: &str) -> Catalog {
    let en = Catalog::parse("en", EN).expect("English catalog");
    let text = match language {
        "fr" => FR,
        "ru" => RU,
        "ja" => JA,
        "ar" => AR,
        "he" => HE,
        _ => return en,
    };
    Catalog::parse(language, text)
        .expect("catalog")
        .fallback(en)
}

struct Demo {
    /// The language picked with a button; `None` follows the system.
    choice: Option<usize>,
    notes: u32,
}

/// The sample every format shows: 16 November 2026, 21:05:09.
fn sample() -> DateTime {
    DateTime {
        year: 2026,
        month: 11,
        day: 16,
        hour: 21,
        minute: 5,
        second: 9,
    }
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let (languages, locale) = match state.choice {
        Some(i) => (
            vec![LANGUAGES[i].0.to_string()],
            Locale::new(LANGUAGES[i].0),
        ),
        None => (platform::preferred_languages(), platform::locale()),
    };
    let shown = negotiate(&languages, SHIPPED).unwrap_or("en");
    let mut tr = catalog(shown);
    if locale.language() == shown {
        tr.set_locale(locale.clone());
    }

    let clock = match locale.hour_cycle() {
        HourCycle::H12 => tr.text("clock.12"),
        _ => tr.text("clock.24"),
    };
    let day = locale.weekday_name(locale.first_weekday()).to_string();
    let dt = sample();

    let mut buttons = vec![{
        let system = button(cx, tr.text("system"));
        system.on_tap(cx, |s| {
            s.choice = None;
            platform::set_preferred_languages(Vec::<String>::new());
        })
    }];
    for (i, (tag, name)) in LANGUAGES.iter().enumerate() {
        let b = button(cx, *name);
        buttons.push(b.on_tap(cx, move |s| {
            s.choice = Some(i);
            platform::set_preferred_languages([*tag]);
        }));
    }

    let rows: Vec<(String, String)> = vec![
        (tr.text("number"), locale.format_number(1_234_567.891)),
        (tr.text("percent"), locale.format_percent(0.256)),
        (
            tr.text("money"),
            format!(
                "{}   {}   {}",
                locale.format_currency(1234.5, "EUR"),
                locale.format_currency(-42.0, "USD"),
                locale.format_currency(1500.0, "JPY")
            ),
        ),
        (
            tr.text("date.short"),
            locale.format_date(&dt, DateStyle::Short),
        ),
        (
            tr.text("date.medium"),
            locale.format_date(&dt, DateStyle::Medium),
        ),
        (
            tr.text("date.long"),
            locale.format_date(&dt, DateStyle::Long),
        ),
        (
            tr.text("date.full"),
            locale.format_date(&dt, DateStyle::Full),
        ),
        (
            tr.text("time"),
            format!(
                "{}   {}",
                locale.format_time(&dt, TimeStyle::Short),
                locale.format_time(&dt, TimeStyle::Medium)
            ),
        ),
    ];
    let mut cells = Vec::new();
    for (name, value) in rows {
        cells.push(label(cx, name));
        cells.push(label(cx, value));
    }

    let notes = tr.plural("notes", f64::from(state.notes), &[]);
    let fewer = button(cx, "−").on_tap(cx, |s| s.notes = s.notes.saturating_sub(1));
    let more = button(cx, "+").on_tap(cx, |s| s.notes += 1);
    let lots = button(cx, "+100").on_tap(cx, |s| s.notes += 100);

    column(vec![
        heading(cx, tr.text("title")),
        label(
            cx,
            tr.format("preferred", &[("list", &languages.join(", "))]),
        ),
        label(
            cx,
            tr.format(
                "formats",
                &[("tag", locale.tag()), ("clock", &clock), ("day", &day)],
            ),
        ),
        row(buttons).gap(6.0).wrap(),
        grid(vec![Track::Auto, Track::Fr(1.0)], cells)
            .gap(8.0)
            .padding(Insets::uniform(4.0)),
        row(vec![fewer, label(cx, notes).width(200.0), more, lots])
            .gap(8.0)
            .align(Align::Start, Align::Center),
    ])
    .gap(12.0)
    .padding(Insets::uniform(24.0))
    .grow(1.0)
}

fn app() -> App<Demo> {
    App::new(
        Demo {
            choice: None,
            notes: 3,
        },
        view,
    )
    .title("Stipple — localization")
    .logical_size(Size::new(720.0, 560.0))
    .languages(SHIPPED.iter().copied())
}

fn main() {
    app().run();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo(tag: &str, notes: u32) -> App<Demo> {
        let i = LANGUAGES.iter().position(|(t, _)| *t == tag).unwrap();
        App::new(
            Demo {
                choice: Some(i),
                notes,
            },
            view,
        )
        .logical_size(Size::new(720.0, 560.0))
    }

    #[test]
    fn french_formats_and_strings() {
        let mut app = demo("fr-FR", 2);
        assert!(app.find_text("Localisation").is_some());
        assert!(app.find_text("1\u{202f}234\u{202f}567,891").is_some());
        assert!(app.find_text("lundi 16 novembre 2026").is_some());
        assert!(app.find_text("2 notes").is_some());
    }

    #[test]
    fn plural_forms_follow_the_language() {
        assert!(demo("ru-RU", 3).find_text("3 заметки").is_some());
        assert!(demo("ru-RU", 5).find_text("5 заметок").is_some());
        assert!(demo("ar-EG", 2).find_text("ملاحظتان").is_some());
        assert!(demo("ja-JP", 1).find_text("1 件のメモ").is_some());
        // German isn't shipped: English strings, German formats.
        let mut de = demo("de-DE", 1);
        assert!(de.find_text("1 note").is_some());
        assert!(de.find_text("Montag, 16. November 2026").is_some());
    }

    #[test]
    fn it_renders() {
        let pm = demo("he-IL", 2).render_once();
        assert!(pm.size().width > 0);
    }
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
