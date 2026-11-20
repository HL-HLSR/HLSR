//! Выгрузки из 0.4.7 в `crates/hlsr-ui/data/` (задача И4): форма JSON зафиксирована
//! структурами `serde` с `deny_unknown_fields` — новое поле или другой тип в выгрузке
//! ломает тест, а не форматтеры и значки молча. Скрипты: `tools/reference/export-intl.js`,
//! `export-fa.js`, `export-changelog.js`.

use serde::Deserialize;
use skrifa::{FontRef, MetadataProvider};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn read<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    let path = data_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// ---------------------------------------------------------------- intl.json

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Intl {
    generator: String,
    runtime: Runtime,
    locales: BTreeMap<String, LocaleData>,
    #[serde(rename = "systemLocale")]
    system_locale: SystemLocale,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runtime {
    electron: String,
    node: String,
    v8: String,
    icu: String,
    cldr: String,
    unicode: String,
    tz: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocaleData {
    tag: String,
    language: String,
    months: Months,
    dates: BTreeMap<String, DateFormat>,
    numbers: Numbers,
    compact: Compact,
    plural: Plural,
    countries: Vec<Country>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Months {
    long: Vec<String>,
    short: Vec<String>,
    #[serde(rename = "longStandalone")]
    long_standalone: Vec<String>,
    #[serde(rename = "shortStandalone")]
    short_standalone: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DateFormat {
    source: String,
    locale: String,
    options: BTreeMap<String, String>,
    resolved: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "timeZone")]
    time_zone: String,
    pattern: String,
    parts: Vec<Part>,
    #[serde(default)]
    months: Vec<String>,
    #[serde(default, rename = "dayPeriods")]
    day_periods: Vec<String>,
    samples: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Numbers {
    source: String,
    locale: String,
    group: String,
    decimal: String,
    #[serde(rename = "minimumGroupingDigits")]
    minimum_grouping_digits: u8,
    samples: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compact {
    source: String,
    locale: String,
    options: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "byExponent")]
    by_exponent: BTreeMap<String, String>,
    samples: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plural {
    source: String,
    locale: String,
    categories: Vec<String>,
    samples: Vec<(f64, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Country {
    id: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemLocale {
    note: String,
    time: BTreeMap<String, DateFormat>,
    #[serde(rename = "dateTime")]
    date_time: BTreeMap<String, DateFormat>,
}

const DATE_KINDS: [&str; 6] = [
    "dateLong",
    "dateShort",
    "time",
    "dateTimeLong",
    "dateTimeNoYear",
    "dateTimeDefault",
];

#[test]
fn intl_form() {
    let intl: Intl = read("intl.json");
    assert_eq!(intl.generator, "tools/reference/export-intl.js");
    assert_eq!(intl.runtime.electron, "44.5.1");
    assert_eq!(intl.runtime.tz, "UTC");
    for field in [
        &intl.runtime.node,
        &intl.runtime.v8,
        &intl.runtime.icu,
        &intl.runtime.cldr,
        &intl.runtime.unicode,
    ] {
        assert!(!field.is_empty());
    }
    assert_eq!(
        intl.locales.keys().collect::<Vec<_>>(),
        ["en", "ru"],
        "языки интерфейса 0.4.7"
    );
    assert_eq!(intl.locales["ru"].tag, "ru-RU");
    assert_eq!(intl.locales["en"].tag, "en-GB");

    for (language, locale) in &intl.locales {
        assert_eq!(&locale.language, language, "#L_RUNNER_LOCALE");
        for months in [
            &locale.months.long,
            &locale.months.short,
            &locale.months.long_standalone,
            &locale.months.short_standalone,
        ] {
            assert_eq!(months.len(), 12, "{language}: 12 месяцев");
        }
        assert_eq!(
            locale.dates.keys().map(String::as_str).collect::<Vec<_>>(),
            {
                let mut sorted = DATE_KINDS.to_vec();
                sorted.sort_unstable();
                sorted
            },
            "{language}: виды дат"
        );
        for (kind, format) in &locale.dates {
            check_date_format(language, kind, format);
        }
        assert_eq!(locale.numbers.locale, locale.tag);
        assert!(!locale.numbers.group.is_empty());
        assert!(!locale.numbers.decimal.is_empty());
        assert!(locale.numbers.minimum_grouping_digits >= 1);
        assert!(locale.numbers.samples.len() >= 20);
        assert!(!locale.numbers.source.is_empty());
        assert_eq!(locale.compact.locale, locale.tag);
        assert!(!locale.compact.source.is_empty());
        assert_eq!(
            locale
                .compact
                .options
                .get("notation")
                .and_then(|v| v.as_str()),
            Some("compact")
        );
        assert_eq!(
            locale
                .compact
                .options
                .get("maximumFractionDigits")
                .and_then(|v| v.as_u64()),
            Some(1),
            "SponsorsModal.vue:126"
        );
        assert_eq!(
            locale.compact.by_exponent.len(),
            19,
            "{language}: показатели 0..18"
        );
        assert!(locale.compact.samples.len() >= 20);
        assert_eq!(locale.plural.locale, locale.tag);
        assert!(!locale.plural.source.is_empty());
        assert!(locale.plural.categories.contains(&"one".to_string()));
        assert!(locale.plural.categories.contains(&"other".to_string()));
        assert!(locale.plural.samples.len() >= 130);
        for (_, category) in &locale.plural.samples {
            assert!(
                locale.plural.categories.contains(category),
                "{language}: категория {category}"
            );
        }
        assert!(
            locale.countries.len() > 200,
            "{language}: стран {}",
            locale.countries.len()
        );
        let mut ids: Vec<&str> = locale.countries.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            locale.countries.len(),
            "{language}: коды стран без повторов"
        );
        for country in &locale.countries {
            assert_eq!(country.id.len(), 2);
            assert!(country.id.bytes().all(|b| b.is_ascii_uppercase()));
            assert!(!country.label.is_empty());
            assert_ne!(country.label, country.id);
        }
    }
    // Русский: one/few/many/other, 1 → one, 2 → few, 5 → many, 1,5 → other.
    let ru = &intl.locales["ru"].plural;
    let sample = |n: f64| {
        &ru.samples
            .iter()
            .find(|(value, _)| *value == n)
            .unwrap_or_else(|| panic!("нет образца {n}"))
            .1
    };
    assert_eq!(sample(1.0), "one");
    assert_eq!(sample(2.0), "few");
    assert_eq!(sample(5.0), "many");
    assert_eq!(sample(21.0), "one");
    assert_eq!(sample(1.5), "other");
    let en = &intl.locales["en"].plural;
    assert_eq!(en.categories, ["one", "other"]);

    assert!(!intl.system_locale.note.is_empty());
    for table in [&intl.system_locale.time, &intl.system_locale.date_time] {
        assert_eq!(
            table.keys().map(String::as_str).collect::<Vec<_>>(),
            ["en-GB", "en-US", "ru-RU"]
        );
        for (tag, format) in table {
            check_date_format(tag, "system", format);
        }
    }
}

fn check_date_format(language: &str, kind: &str, format: &DateFormat) {
    assert!(!format.source.is_empty(), "{language}/{kind}: source");
    assert!(!format.locale.is_empty(), "{language}/{kind}: locale");
    assert_eq!(format.time_zone, "UTC");
    assert!(!format.pattern.is_empty());
    // resolvedOptions(): локаль ICU и, у времени, цикл часов (h23 / h12).
    assert!(
        format.resolved.get("locale").is_some_and(|v| v.is_string()),
        "{language}/{kind}: resolved.locale"
    );
    let has_hour = format.parts.iter().any(|part| part.kind == "hour");
    assert_eq!(
        format.resolved.contains_key("hourCycle"),
        has_hour,
        "{language}/{kind}: hourCycle только у времени"
    );
    assert!(
        format.samples.len() >= 20,
        "{language}/{kind}: образцов {}",
        format.samples.len()
    );
    let has_month = format.parts.iter().any(|part| part.kind == "month");
    let month_named = matches!(
        format.options.get("month").map(String::as_str),
        Some("long" | "short")
    );
    assert_eq!(
        !format.months.is_empty(),
        has_month && month_named,
        "{language}/{kind}: таблица месяцев только у именованного месяца"
    );
    if !format.months.is_empty() {
        assert_eq!(format.months.len(), 12);
    }
    let has_period = format.parts.iter().any(|part| part.kind == "dayPeriod");
    assert_eq!(
        format.day_periods.len() == 2,
        has_period,
        "{language}/{kind}: AM/PM"
    );
    // Шаблон — конкатенация частей: поля в фигурных скобках, литералы как есть.
    let rebuilt: String = format
        .parts
        .iter()
        .map(|part| {
            if part.kind == "literal" {
                part.value.clone().expect("литерал со значением")
            } else {
                assert!(
                    part.value.is_none(),
                    "{language}/{kind}: у поля {} нет значения",
                    part.kind
                );
                format!("{{{}}}", part.kind)
            }
        })
        .collect();
    assert_eq!(
        rebuilt, format.pattern,
        "{language}/{kind}: pattern = parts"
    );
    for key in format.samples.keys() {
        assert!(
            key.ends_with('Z') && key.len() == 20,
            "{language}/{kind}: ключ образца {key} — ISO UTC"
        );
    }
}

// ------------------------------------------------------------------ fa.json

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fa {
    generator: String,
    version: String,
    license: String,
    fonts: BTreeMap<String, FaFont>,
    prefixes: BTreeMap<String, String>,
    icons: BTreeMap<String, FaIcon>,
    aliases: BTreeMap<String, String>,
    v4: BTreeMap<String, FaV4>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FaFont {
    file: String,
    family: String,
    weight: u16,
    glyphs: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FaIcon {
    code: String,
    styles: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FaV4 {
    code: String,
    #[serde(default)]
    style: Option<String>,
}

fn code_point(code: &str) -> char {
    // Escape CSS без «\»: fa-0 … fa-9 — "30" … "39", остальное — PUA f000+/e000+.
    assert!(
        (1..=6).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_hexdigit()),
        "кодовая точка {code}"
    );
    char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap_or_else(|| panic!("{code}"))
}

#[test]
fn fa_form_and_fonts() {
    let fa: Fa = read("fa.json");
    assert_eq!(fa.generator, "tools/reference/export-fa.js");
    assert_eq!(fa.version, "7.3.1");
    assert!(fa.license.contains("CC BY 4.0") && fa.license.contains("OFL"));
    assert_eq!(
        fa.fonts.keys().map(String::as_str).collect::<Vec<_>>(),
        ["brands", "regular", "solid"]
    );
    assert_eq!(fa.fonts["solid"].weight, 900);
    assert_eq!(fa.fonts["regular"].weight, 400);
    assert_eq!(fa.fonts["brands"].weight, 400);
    assert_eq!(fa.fonts["solid"].family, "Font Awesome 7 Free");
    assert_eq!(fa.fonts["regular"].family, "Font Awesome 7 Free");
    assert_eq!(fa.fonts["brands"].family, "Font Awesome 7 Brands");
    for prefix in [
        "fa",
        "fas",
        "far",
        "fab",
        "fa-solid",
        "fa-regular",
        "fa-brands",
        "fa-classic",
    ] {
        let style = &fa.prefixes[prefix];
        assert!(fa.fonts.contains_key(style), "{prefix} → {style}");
    }
    assert!(fa.icons.len() > 1900, "значков {}", fa.icons.len());
    assert!(fa.aliases.len() > 500, "псевдонимов {}", fa.aliases.len());
    assert!(fa.v4.len() > 400, "v4 {}", fa.v4.len());
    for (name, target) in &fa.aliases {
        assert!(fa.icons.contains_key(target), "псевдоним {name} → {target}");
        assert!(
            !fa.icons.contains_key(name),
            "псевдоним {name} — имя значка"
        );
    }
    for (name, shim) in &fa.v4 {
        code_point(&shim.code);
        if let Some(style) = &shim.style {
            assert!(fa.fonts.contains_key(style), "v4 {name}: стиль {style}");
        }
    }

    // Кодовая точка каждого значка есть в шрифте каждого из его стилей, и только там.
    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts");
    let bytes: BTreeMap<&str, Vec<u8>> = fa
        .fonts
        .iter()
        .map(|(style, font)| {
            let path = fonts_dir.join(&font.file);
            (
                style.as_str(),
                std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
            )
        })
        .collect();
    let faces: BTreeMap<&str, FontRef> = bytes
        .iter()
        .map(|(style, data)| (*style, FontRef::new(data).expect("шрифт FA")))
        .collect();
    for (style, font) in &fa.fonts {
        let charmap = faces[style.as_str()].charmap();
        let glyphs = charmap.mappings().count();
        assert_eq!(glyphs, font.glyphs, "{style}: глифов в cmap");
    }
    for (name, icon) in &fa.icons {
        assert!(!icon.styles.is_empty(), "{name}: без стилей");
        let ch = code_point(&icon.code);
        for (style, face) in &faces {
            let present = face.charmap().map(ch).is_some();
            assert_eq!(
                present,
                icon.styles.iter().any(|s| s == style),
                "{name} U+{}: стиль {style}",
                icon.code.to_uppercase()
            );
        }
    }
    // Имена, которые 0.4.7 склеивает в коде, разрешаются через псевдонимы.
    assert_eq!(fa.aliases["sync-alt"], "rotate");
    assert_eq!(fa.aliases["times"], "xmark");
    assert_eq!(fa.icons["check"].code, "f00c");
    assert_eq!(fa.icons["check"].styles, ["solid"]);
    assert!(fa.icons["circle"].styles.contains(&"regular".to_string()));
    assert_eq!(fa.icons["discord"].styles, ["brands"]);
}

// ----------------------------------------------------------- changelog.json

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Changelog {
    generator: String,
    version: String,
    title: BTreeMap<String, String>,
    releases: Vec<Release>,
    legacy: BTreeMap<String, Vec<Entry>>,
    built: BTreeMap<String, Vec<Entry>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    version: String,
    ru: Vec<String>,
    en: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: Option<String>,
    title: String,
    groups: Vec<Group>,
    #[serde(default)]
    current: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    title: Option<String>,
    items: Vec<String>,
}

#[test]
fn changelog_form() {
    let changelog: Changelog = read("changelog.json");
    assert_eq!(changelog.generator, "tools/reference/export-changelog.js");
    assert_eq!(changelog.version, "0.4.9");
    assert_eq!(changelog.title["ru"], "Обновление HLSR");
    assert_eq!(changelog.title["en"], "HLSR update");
    // Первым стоит 0.5.0: в него влита запись 0.4.10 (эпизоды Ghosting).
    assert_eq!(changelog.releases[0].version, "0.5.0");
    let current = changelog
        .releases
        .iter()
        .position(|release| release.version == changelog.version)
        .unwrap();
    assert_eq!(current, 1);
    let shown = changelog.releases.len() - current;
    assert_eq!(changelog.releases.last().unwrap().version, "0.3.0");
    for release in &changelog.releases {
        assert_eq!(
            release.ru.len(),
            release.en.len(),
            "{}: пунктов ru и en",
            release.version
        );
        assert!(!release.ru.is_empty(), "{}: пусто", release.version);
    }
    for language in ["ru", "en"] {
        let legacy = &changelog.legacy[language];
        assert!(
            !legacy.is_empty(),
            "{language}: старый журнал из #UI_CHANGELOG_CONTENT"
        );
        for entry in legacy {
            assert!(entry.current.is_none());
            assert!(entry.groups.iter().any(|group| !group.items.is_empty()));
        }
        let built = &changelog.built[language];
        assert_eq!(built.len(), shown + legacy.len());
        assert_eq!(built[0].version.as_deref(), Some("0.4.9"));
        assert_eq!(built[0].current, Some(true));
        assert_eq!(
            built[0].title,
            format!("{} 0.4.9", changelog.title[language])
        );
        assert!(built[1..shown].iter().all(|e| e.current == Some(false)));
        assert_eq!(built[0].groups.len(), 1);
        assert!(built[0].groups[0].title.is_none());
        assert_eq!(
            built[0].groups[0].items,
            changelog.releases[current][language]
        );
    }
}

impl std::ops::Index<&str> for Release {
    type Output = Vec<String>;
    fn index(&self, language: &str) -> &Vec<String> {
        match language {
            "ru" => &self.ru,
            "en" => &self.en,
            other => panic!("язык {other}"),
        }
    }
}
