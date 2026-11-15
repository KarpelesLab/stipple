use super::*;

fn dt() -> DateTime {
    DateTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 21,
        minute: 5,
        second: 9,
    }
}

#[test]
fn tags_are_canonical() {
    assert_eq!(canonical_tag("en_US.UTF-8"), "en-US");
    assert_eq!(canonical_tag("zh_Hant_TW"), "zh-Hant-TW");
    assert_eq!(canonical_tag("fr-ca"), "fr-CA");
    assert_eq!(canonical_tag("C"), "en");
    assert_eq!(canonical_tag("POSIX"), "en");
    assert_eq!(canonical_tag("sr-Latn-RS"), "sr-Latn-RS");
    assert_eq!(canonical_tag("es-419"), "es-419");
    assert_eq!(canonical_tag("iw_IL"), "he-IL");
    assert_eq!(canonical_tag("de_DE@euro"), "de-DE");
    assert_eq!(canonical_tag("en-US-u-hc-h23"), "en-US");
}

#[test]
fn regions_default_to_the_likely_one() {
    assert_eq!(Locale::new("fr").region(), Some("FR"));
    assert_eq!(Locale::new("pt").region(), Some("BR"));
    assert_eq!(Locale::new("en").region(), Some("US"));
    assert_eq!(Locale::new("zh-Hant").region(), Some("CN"));
    assert_eq!(Locale::new("da").region(), Some("DK"));
}

#[test]
fn numbers() {
    let n = 1_234_567.891;
    let cases = [
        ("en-US", "1,234,567.891"),
        ("fr-FR", "1\u{202f}234\u{202f}567,891"),
        ("de-DE", "1.234.567,891"),
        ("de-CH", "1’234’567.891"),
        ("es-ES", "1.234.567,891"),
        ("ru", "1\u{a0}234\u{a0}567,891"),
        ("ja", "1,234,567.891"),
        ("en-IN", "12,34,567.891"),
        ("ar-EG", "١٬٢٣٤٬٥٦٧٫٨٩١"),
        ("ar-MA", "1.234.567,891"),
    ];
    for (tag, want) in cases {
        assert_eq!(Locale::new(tag).format_number(n), want, "{tag}");
    }
    let en = Locale::new("en");
    assert_eq!(en.format_number(-0.5), "-0.5");
    assert_eq!(en.format_number(-0.0001), "0");
    assert_eq!(en.format_number(2.0), "2");
    assert_eq!(en.format_number(1.23456), "1.235");
    assert_eq!(en.format_fixed(2.5, 2), "2.50");
    assert_eq!(en.format_number(f64::NAN), "NaN");
    assert_eq!(en.format_number(f64::NEG_INFINITY), "-∞");
    // Spanish and Polish don't group four digits.
    assert_eq!(Locale::new("es").format_number(1234.0), "1234");
    assert_eq!(Locale::new("es").format_number(12345.0), "12.345");
    assert_eq!(Locale::new("pl").format_number(1234.0), "1234");
    assert_eq!(Locale::new("sv").format_number(-3.0), "\u{2212}3");
}

#[test]
fn percentages() {
    assert_eq!(Locale::new("en").format_percent(0.256), "26%");
    assert_eq!(Locale::new("fr").format_percent(0.5), "50\u{202f}%");
    assert_eq!(Locale::new("de").format_percent(0.5), "50\u{a0}%");
    assert_eq!(Locale::new("tr").format_percent(0.5), "%50");
    assert_eq!(Locale::new("tr").format_percent(-0.5), "-%50");
}

#[test]
fn currencies() {
    let cases = [
        ("en-US", 1234.5, "USD", "$1,234.50"),
        ("en-US", -1234.5, "USD", "-$1,234.50"),
        ("en-GB", 3.0, "USD", "US$3.00"),
        ("en-GB", 3.0, "GBP", "£3.00"),
        ("fr-FR", 1234.5, "EUR", "1\u{202f}234,50\u{a0}€"),
        ("fr-FR", -1.0, "EUR", "-1,00\u{a0}€"),
        ("de-DE", 9.99, "EUR", "9,99\u{a0}€"),
        ("nl-NL", 1234.5, "EUR", "€\u{a0}1.234,50"),
        ("nl-NL", -1.0, "EUR", "€\u{a0}-1,00"),
        ("pt-BR", 10.0, "BRL", "R$\u{a0}10,00"),
        ("ja-JP", 1500.0, "JPY", "￥1,500"),
        ("en-US", 1500.4, "JPY", "¥1,500"),
        ("en-US", 12.0, "CHF", "CHF\u{a0}12.00"),
        ("de-CH", 12.0, "CHF", "12.00\u{a0}CHF"),
        ("en-CA", 5.0, "CAD", "$5.00"),
        ("en-US", 5.0, "CAD", "CA$5.00"),
        ("ko", 5000.0, "KRW", "₩5,000"),
        ("en", 1.0, "KWD", "KWD\u{a0}1.000"),
    ];
    for (tag, amount, code, want) in cases {
        assert_eq!(
            Locale::new(tag).format_currency(amount, code),
            want,
            "{tag} {amount} {code}"
        );
    }
}

#[test]
fn dates() {
    let d = dt();
    let cases = [
        (
            "en-US",
            [
                "11/6/26",
                "Nov 6, 2026",
                "November 6, 2026",
                "Friday, November 6, 2026",
            ],
        ),
        (
            "en-GB",
            [
                "06/11/2026",
                "6 Nov 2026",
                "6 November 2026",
                "Friday 6 November 2026",
            ],
        ),
        (
            "fr-FR",
            [
                "06/11/2026",
                "6 nov. 2026",
                "6 novembre 2026",
                "vendredi 6 novembre 2026",
            ],
        ),
        (
            "fr-CA",
            [
                "2026-11-06",
                "6 nov. 2026",
                "6 novembre 2026",
                "vendredi 6 novembre 2026",
            ],
        ),
        (
            "de-DE",
            [
                "06.11.26",
                "06.11.2026",
                "6. November 2026",
                "Freitag, 6. November 2026",
            ],
        ),
        (
            "es-ES",
            [
                "6/11/26",
                "6 nov 2026",
                "6 de noviembre de 2026",
                "viernes, 6 de noviembre de 2026",
            ],
        ),
        (
            "ru-RU",
            [
                "06.11.2026",
                "6 нояб. 2026 г.",
                "6 ноября 2026 г.",
                "пятница, 6 ноября 2026 г.",
            ],
        ),
        (
            "ja-JP",
            [
                "2026/11/06",
                "2026/11/06",
                "2026年11月6日",
                "2026年11月6日金曜日",
            ],
        ),
        (
            "zh-CN",
            [
                "2026/11/6",
                "2026年11月6日",
                "2026年11月6日",
                "2026年11月6日星期五",
            ],
        ),
        (
            "ko-KR",
            [
                "26. 11. 6.",
                "2026. 11. 6.",
                "2026년 11월 6일",
                "2026년 11월 6일 금요일",
            ],
        ),
        (
            "he-IL",
            [
                "6.11.2026",
                "6 בנוב׳ 2026",
                "6 בנובמבר 2026",
                "יום שישי, 6 בנובמבר 2026",
            ],
        ),
        (
            "tr-TR",
            [
                "6.11.2026",
                "6 Kas 2026",
                "6 Kasım 2026",
                "6 Kasım 2026 Cuma",
            ],
        ),
        (
            "sv-SE",
            [
                "2026-11-06",
                "6 nov. 2026",
                "6 november 2026",
                "fredag 6 november 2026",
            ],
        ),
        (
            "xx",
            [
                "2026-11-06",
                "2026 Nov 6",
                "2026 November 6",
                "2026 November 6, Friday",
            ],
        ),
    ];
    let styles = [
        DateStyle::Short,
        DateStyle::Medium,
        DateStyle::Long,
        DateStyle::Full,
    ];
    for (tag, want) in cases {
        let l = Locale::new(tag);
        for (style, want) in styles.iter().zip(want) {
            assert_eq!(l.format_date(&d, *style), want, "{tag} {style:?}");
        }
    }
    assert_eq!(
        Locale::new("ar-EG").format_date(&d, DateStyle::Long),
        "٦ نوفمبر ٢٠٢٦"
    );
}

#[test]
fn times_follow_the_clock() {
    let d = dt();
    let cases = [
        ("en-US", "9:05 PM", "9:05:09 PM"),
        ("en-GB", "21:05", "21:05:09"),
        ("en-AU", "9:05 pm", "9:05:09 pm"),
        ("fr-FR", "21:05", "21:05:09"),
        ("fr-CA", "21 h 05", "21 h 05 min 09 s"),
        ("de", "21:05", "21:05:09"),
        ("ja", "21:05", "21:05:09"),
        ("ko", "오후 9:05", "오후 9:05:09"),
        ("zh-TW", "下午9:05", "下午9:05:09"),
        ("zh-CN", "21:05", "21:05:09"),
        ("ar-EG", "٩:٠٥ م", "٩:٠٥:٠٩ م"),
    ];
    for (tag, short, medium) in cases {
        let l = Locale::new(tag);
        assert_eq!(l.format_time(&d, TimeStyle::Short), short, "{tag}");
        assert_eq!(l.format_time(&d, TimeStyle::Medium), medium, "{tag}");
    }
    // The user's choice wins over the region's.
    let mut us = Locale::new("en-US");
    us.set_hour_cycle(HourCycle::H23);
    assert_eq!(us.format_time(&d, TimeStyle::Short), "21:05");
    let mut ja = Locale::new("ja-JP-u-hc-h12");
    assert_eq!(ja.format_time(&d, TimeStyle::Short), "午後9:05");
    ja.set_hour_cycle(HourCycle::H23);
    assert_eq!(ja.format_time(&d, TimeStyle::Short), "21:05");
    let midnight = DateTime::new(2026, 1, 1);
    assert_eq!(
        Locale::new("en-US").format_time(&midnight, TimeStyle::Short),
        "12:00 AM"
    );
}

#[test]
fn dates_and_times_together() {
    let d = dt();
    assert_eq!(
        Locale::new("en-US").format_date_time(&d, DateStyle::Medium, TimeStyle::Short),
        "Nov 6, 2026, 9:05 PM"
    );
    assert_eq!(
        Locale::new("en-US").format_date_time(&d, DateStyle::Long, TimeStyle::Short),
        "November 6, 2026 at 9:05 PM"
    );
    assert_eq!(
        Locale::new("fr").format_date_time(&d, DateStyle::Long, TimeStyle::Short),
        "6 novembre 2026 à 21:05"
    );
    assert_eq!(
        Locale::new("de").format_date_time(&d, DateStyle::Short, TimeStyle::Short),
        "06.11.26, 21:05"
    );
    assert_eq!(
        Locale::new("en").format_pattern(&d, "EEE d MMM ''yy G"),
        "Fri 6 Nov '26"
    );
}

#[test]
fn weeks_and_clocks_by_region() {
    let first = |t: &str| Locale::new(t).first_weekday();
    assert_eq!(first("en-US"), Weekday::Sunday);
    assert_eq!(first("en-GB"), Weekday::Monday);
    assert_eq!(first("fr"), Weekday::Monday);
    assert_eq!(first("ar-EG"), Weekday::Saturday);
    assert_eq!(first("he"), Weekday::Sunday);
    assert_eq!(first("ja"), Weekday::Sunday);
    assert_eq!(first("en-US-u-fw-mon"), Weekday::Monday);
    let clock = |t: &str| Locale::new(t).hour_cycle();
    assert_eq!(clock("en-US"), HourCycle::H12);
    assert_eq!(clock("en-CA"), HourCycle::H12);
    assert_eq!(clock("fr-CA"), HourCycle::H23);
    assert_eq!(clock("en-GB"), HourCycle::H23);
    assert_eq!(clock("ko"), HourCycle::H12);
    assert_eq!(clock("en-US-u-hc-h23"), HourCycle::H23);
    // `rg`: English, with the UK's conventions.
    let rg = Locale::new("en-US-u-rg-gbzzzz");
    assert_eq!(rg.format_date(&dt(), DateStyle::Short), "06/11/2026");
    assert_eq!(rg.hour_cycle(), HourCycle::H23);
    assert_eq!(rg.region(), Some("US"));
}

#[test]
fn names() {
    let fr = Locale::new("fr");
    assert_eq!(fr.month_name(2), "février");
    assert_eq!(fr.month_abbr(2), "févr.");
    assert_eq!(fr.weekday_name(Weekday::Sunday), "dimanche");
    assert_eq!(Locale::new("zh-TW").weekday_abbr(Weekday::Monday), "週一");
    assert_eq!(Locale::new("zh").weekday_abbr(Weekday::Monday), "周一");
}

#[test]
fn os_settings_override_the_tables() {
    let mut l = Locale::new("en-US");
    l.apply(&OsFormats {
        decimal: Some(",".into()),
        group: Some(".".into()),
        hour_cycle: Some(HourCycle::H23),
        dates: [Some("dd.MM.y".into()), None, None, None],
        times: [Some("HH:mm".into()), None],
        ..OsFormats::default()
    });
    assert_eq!(l.format_number(1234.5), "1.234,5");
    assert_eq!(l.format_date(&dt(), DateStyle::Short), "06.11.2026");
    assert_eq!(l.format_date(&dt(), DateStyle::Medium), "Nov 6, 2026");
    assert_eq!(l.format_time(&dt(), TimeStyle::Short), "21:05");
    // Switched back to 12 hours, the 24-hour pattern is converted.
    l.set_hour_cycle(HourCycle::H12);
    assert_eq!(l.format_time(&dt(), TimeStyle::Short), "9:05 PM");
}

#[test]
fn negotiation() {
    let shipped = ["en", "fr", "ja", "pt-PT", "zh-Hans", "zh-Hant"];
    assert_eq!(negotiate(&["fr-CA"], &shipped), Some("fr"));
    assert_eq!(negotiate(&["de", "ja"], &shipped), Some("ja"));
    assert_eq!(negotiate(&["pt-BR"], &shipped), Some("pt-PT"));
    assert_eq!(negotiate(&["zh-CN"], &shipped), Some("zh-Hans"));
    assert_eq!(negotiate(&["zh-HK"], &shipped), Some("zh-Hant"));
    assert_eq!(negotiate(&["zh-Hant-TW"], &shipped), Some("zh-Hant"));
    assert_eq!(negotiate(&["de"], &shipped), None);
    assert_eq!(negotiate(&["en_GB.UTF-8"], &shipped), Some("en"));
    let regional = ["en-US", "en-GB"];
    assert_eq!(negotiate(&["en-GB"], &regional), Some("en-GB"));
    assert_eq!(negotiate(&["en-AU"], &regional), Some("en-US"));
}

#[test]
fn builtin_strings() {
    assert_eq!(builtin_in("en", false, Builtin::Ok), "OK");
    assert_eq!(builtin_in("es", false, Builtin::Ok), "Aceptar");
    assert_eq!(builtin_in("zh", true, Builtin::Ok), "確定");
    assert_eq!(builtin_in("he", false, Builtin::On), "פועל");
    assert_eq!(builtin_in("xx", false, Builtin::Collapsed), "collapsed");
}

#[test]
fn rtl_languages() {
    assert!(Locale::new("ar").layout_direction().is_rtl());
    assert!(Locale::new("he-IL").layout_direction().is_rtl());
    assert!(!Locale::new("fr").layout_direction().is_rtl());
}
