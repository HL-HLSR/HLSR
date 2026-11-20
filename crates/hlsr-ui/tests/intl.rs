//! Форматтеры `intl.rs` против строк Electron 44 из `data/intl.json`
//! (`tools/reference/export-intl.js`, задача И4): каждый образец дат, чисел,
//! сокращений и форм множественного числа выгрузки, страны — в порядке выгрузки.

use chrono::{DateTime, Utc};
use hlsr_core::language::Language;
use hlsr_ui::intl::{
    self, DateStyle, Plural, SystemStyle, countries, country_name, format_compact, format_date_in,
    format_number, format_system_date_in, plural,
};
use serde_json::Value;
use std::path::Path;

fn data() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/intl.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ms(iso: &str) -> f64 {
    DateTime::parse_from_rfc3339(iso)
        .unwrap()
        .timestamp_millis() as f64
}

fn lang(code: &str) -> Language {
    match code {
        "ru" => Language::Ru,
        _ => Language::En,
    }
}

const STYLES: [(&str, DateStyle); 6] = [
    ("dateLong", DateStyle::DateLong),
    ("dateShort", DateStyle::DateShort),
    ("time", DateStyle::Time),
    ("dateTimeLong", DateStyle::DateTimeLong),
    ("dateTimeNoYear", DateStyle::DateTimeNoYear),
    ("dateTimeDefault", DateStyle::DateTimeDefault),
];

/// Собрать расхождения и показать первые.
fn report(what: &str, failures: Vec<String>, checked: usize) {
    assert!(checked > 0, "{what}: нет образцов");
    assert!(
        failures.is_empty(),
        "{what}: {} из {checked} не совпали с Electron:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn dates_match_electron() {
    let data = data();
    let mut failures = Vec::new();
    let mut checked = 0;
    for code in ["ru", "en"] {
        for (key, style) in STYLES {
            let samples = data["locales"][code]["dates"][key]["samples"]
                .as_object()
                .unwrap();
            for (iso, expected) in samples {
                checked += 1;
                let got = format_date_in(lang(code), style, ms(iso), &Utc);
                if got != expected.as_str().unwrap() {
                    failures.push(format!("{code}/{key} {iso}: {got:?} ≠ {expected}"));
                }
            }
        }
    }
    report("даты", failures, checked);
}

#[test]
fn system_locale_dates_match_electron() {
    let data = data();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (table, style) in [
        ("time", SystemStyle::Time),
        ("dateTime", SystemStyle::DateTime),
    ] {
        for (tag, format) in data["systemLocale"][table].as_object().unwrap() {
            for (iso, expected) in format["samples"].as_object().unwrap() {
                checked += 1;
                let got = format_system_date_in(tag, style, ms(iso), &Utc);
                if got != expected.as_str().unwrap() {
                    failures.push(format!("{tag}/{table} {iso}: {got:?} ≠ {expected}"));
                }
            }
        }
    }
    report("даты по локали системы", failures, checked);
    assert_eq!(intl::system_tag(Language::Ru), "ru-RU");
    assert_eq!(intl::system_tag(Language::En), "en-US");
}

#[test]
fn numbers_match_electron() {
    let data = data();
    let mut failures = Vec::new();
    let mut checked = 0;
    for code in ["ru", "en"] {
        for (input, expected) in data["locales"][code]["numbers"]["samples"]
            .as_object()
            .unwrap()
        {
            checked += 1;
            let x: f64 = input.parse().unwrap();
            let got = format_number(lang(code), x);
            if got != expected.as_str().unwrap() {
                failures.push(format!("{code} {input}: {got:?} ≠ {expected:?}"));
            }
        }
    }
    report("числа", failures, checked);
}

#[test]
fn compact_matches_electron() {
    let data = data();
    let mut failures = Vec::new();
    let mut checked = 0;
    for code in ["ru", "en"] {
        for (input, expected) in data["locales"][code]["compact"]["samples"]
            .as_object()
            .unwrap()
        {
            checked += 1;
            let x: f64 = input.parse().unwrap();
            let got = format_compact(lang(code), x);
            if got != expected.as_str().unwrap() {
                failures.push(format!("{code} {input}: {got:?} ≠ {expected:?}"));
            }
        }
    }
    report("compact", failures, checked);
    // SponsorsModal.vue amountShort: «100 млн ₽».
    assert_eq!(
        format!("{} ₽", format_compact(Language::Ru, 100_000_000.0)),
        "100\u{a0}млн ₽"
    );
}

#[test]
fn plurals_match_electron() {
    let data = data();
    let mut failures = Vec::new();
    let mut checked = 0;
    for code in ["ru", "en"] {
        for sample in data["locales"][code]["plural"]["samples"]
            .as_array()
            .unwrap()
        {
            checked += 1;
            let x = sample[0].as_f64().unwrap();
            let expected = sample[1].as_str().unwrap();
            let got = plural(lang(code), x).as_str();
            if got != expected {
                failures.push(format!("{code} {x}: {got} ≠ {expected}"));
            }
        }
    }
    report("PluralRules", failures, checked);
    // HomePlayers.vue countWord: целые → _ONE / _FEW / _MANY.
    assert_eq!(plural(Language::Ru, 21.0), Plural::One);
    assert_eq!(plural(Language::Ru, 112.0), Plural::Many);
}

#[test]
fn countries_follow_the_export() {
    let data = data();
    for code in ["ru", "en"] {
        let expected = data["locales"][code]["countries"].as_array().unwrap();
        let list = countries(lang(code));
        assert_eq!(list.len(), expected.len());
        for (country, want) in list.iter().zip(expected) {
            assert_eq!(country.id, want["id"].as_str().unwrap());
            assert_eq!(country.label, want["label"].as_str().unwrap());
            assert_eq!(country_name(lang(code), &country.id), country.label);
        }
    }
}
