//! Форматтеры ICU рендерера 0.4.7 по выгрузке `data/intl.json` (задача И4,
//! Electron 44: ICU 78, CLDR 48): даты и время (`Date#toLocale*String`), числа
//! (`Number#toLocaleString`), сокращённые суммы (`Intl.NumberFormat` compact),
//! формы множественного числа (`Intl.PluralRules`) и страны
//! (`Intl.DisplayNames` в порядке `localeCompare`).
//!
//! Шаблоны дат, названия месяцев, AM/PM, разделители и суффиксы — из
//! выгрузки; округление и группировка — правила ICU (`halfExpand`, группы по 3,
//! `minimumGroupingDigits`, у compact — MIN2). Языки — ru и en интерфейса;
//! функции «по локали системы» (`toLocaleTimeString([])`, `toLocaleString()`)
//! берут образцы ru-RU, en-GB или en-US ([`system_tag`]).

use chrono::{Datelike, TimeZone, Timelike};
use hlsr_core::language::Language;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;

const DATA: &str = include_str!("../data/intl.json");

static INTL: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_str(DATA).expect("data/intl.json (форма — tests/data.rs)"));

#[derive(Deserialize)]
struct Data {
    locales: BTreeMap<String, LocaleData>,
    #[serde(rename = "systemLocale")]
    system_locale: SystemLocale,
}

#[derive(Deserialize)]
struct LocaleData {
    dates: BTreeMap<String, DateFormat>,
    numbers: Numbers,
    compact: Compact,
    countries: Vec<Country>,
}

#[derive(Deserialize)]
struct SystemLocale {
    time: BTreeMap<String, DateFormat>,
    #[serde(rename = "dateTime")]
    date_time: BTreeMap<String, DateFormat>,
}

#[derive(Deserialize)]
struct DateFormat {
    resolved: Resolved,
    parts: Vec<Part>,
    #[serde(default)]
    months: Vec<String>,
    #[serde(default, rename = "dayPeriods")]
    day_periods: Vec<String>,
}

#[derive(Deserialize)]
struct Resolved {
    #[serde(rename = "hourCycle")]
    hour_cycle: Option<String>,
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    value: Option<String>,
}

#[derive(Deserialize)]
struct Numbers {
    group: String,
    decimal: String,
    #[serde(rename = "minimumGroupingDigits")]
    minimum_grouping_digits: usize,
}

#[derive(Deserialize)]
struct Compact {
    #[serde(rename = "byExponent")]
    by_exponent: BTreeMap<String, String>,
}

/// Страна списка `RunnerSettings.vue` (`countryList`).
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Country {
    pub id: String,
    pub label: String,
}

fn locale(lang: Language) -> &'static LocaleData {
    &INTL.locales[lang.code()]
}

// ---------------------------------------------------------------- даты

/// Вид даты — место вызова в 0.4.7 (поле `source` выгрузки).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    /// `toLocaleDateString(…, {day: "numeric", month: "long", year: "numeric"})`
    /// (`HomePanel.vue` formatDate, formatStamp): «15 октября 2026 г.».
    DateLong,
    /// То же с `month: "short"` (`HomePlayers.vue`, дата регистрации).
    DateShort,
    /// `toLocaleTimeString(…, {hour: "2-digit", minute: "2-digit"})` (formatStamp).
    Time,
    /// `toLocaleString(…)` с днём, месяцем, годом, часами и минутами
    /// (`NotificationCenter.vue` moderationDetail, `ProfileModal.vue` formatModerationDate).
    DateTimeLong,
    /// То же без года (`RunnerSettings.vue` muteUntil).
    DateTimeNoYear,
    /// `toLocaleString(#L_RUNNER_LOCALE)` без параметров (`RunnerSettings.vue` deletionDate).
    DateTimeDefault,
}

impl DateStyle {
    fn key(self) -> &'static str {
        match self {
            DateStyle::DateLong => "dateLong",
            DateStyle::DateShort => "dateShort",
            DateStyle::Time => "time",
            DateStyle::DateTimeLong => "dateTimeLong",
            DateStyle::DateTimeNoYear => "dateTimeNoYear",
            DateStyle::DateTimeDefault => "dateTimeDefault",
        }
    }
}

/// Вид «по локали системы» (`toLocaleTimeString([], …)` в `ProfileModal.vue`
/// formatVoteTime, `toLocaleString()` в `SettingsModal.vue` formatDate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemStyle {
    Time,
    DateTime,
}

/// Текст `new Date(NaN).toLocale*String()`.
pub const INVALID_DATE: &str = "Invalid Date";

/// Наибольший момент `Date` (TimeClip): ±8,64·10¹⁵ мс.
const MAX_TIME_MS: f64 = 8.64e15;

/// Дата `ms` (мс от 1970-01-01 UTC) на языке интерфейса по часам ОС.
pub fn format_date(lang: Language, style: DateStyle, ms: f64) -> String {
    format_date_in(lang, style, ms, &chrono::Local)
}

/// [`format_date`] в заданном часовом поясе.
pub fn format_date_in<Tz: TimeZone>(lang: Language, style: DateStyle, ms: f64, tz: &Tz) -> String {
    render_date(&locale(lang).dates[style.key()], ms, tz)
}

/// Локаль Chromium по умолчанию для функций «по локали системы»: из выгрузки
/// есть ru-RU, en-GB и en-US. 0.4.7 брала язык ОС; здесь — язык интерфейса
/// (ru → ru-RU, en → en-US, локаль Chromium по умолчанию), отступление
/// выгрузки И4.
pub fn system_tag(lang: Language) -> &'static str {
    match lang {
        Language::Ru => "ru-RU",
        Language::En => "en-US",
    }
}

/// Дата по образцу локали системы `tag` (ru-RU, en-GB, en-US; другое — en-US).
pub fn format_system_date_in<Tz: TimeZone>(
    tag: &str,
    style: SystemStyle,
    ms: f64,
    tz: &Tz,
) -> String {
    let table = match style {
        SystemStyle::Time => &INTL.system_locale.time,
        SystemStyle::DateTime => &INTL.system_locale.date_time,
    };
    let format = table.get(tag).unwrap_or_else(|| &table["en-US"]);
    render_date(format, ms, tz)
}

/// [`format_system_date_in`] по часам ОС.
pub fn format_system_date(tag: &str, style: SystemStyle, ms: f64) -> String {
    format_system_date_in(tag, style, ms, &chrono::Local)
}

fn render_date<Tz: TimeZone>(format: &DateFormat, ms: f64, tz: &Tz) -> String {
    if !ms.is_finite() || ms.abs() > MAX_TIME_MS {
        return INVALID_DATE.to_owned();
    }
    let Some(at) = tz.timestamp_millis_opt(ms.trunc() as i64).single() else {
        return INVALID_DATE.to_owned();
    };
    let resolved = &format.resolved;
    let numeric = |value: u32, style: Option<&String>| match style.map(String::as_str) {
        Some("2-digit") => format!("{:02}", value % 100),
        _ => value.to_string(),
    };
    let twelve = resolved.hour_cycle.as_deref() == Some("h12");
    let mut out = String::new();
    for part in &format.parts {
        match part.kind.as_str() {
            "literal" => out.push_str(part.value.as_deref().unwrap_or_default()),
            "year" => match resolved.year.as_deref() {
                Some("2-digit") => out.push_str(&format!("{:02}", at.year().rem_euclid(100))),
                _ => out.push_str(&at.year().to_string()),
            },
            "month" => match resolved.month.as_deref() {
                Some("long" | "short") => {
                    out.push_str(&format.months[at.month0() as usize]);
                }
                style => out.push_str(&numeric(at.month(), style.map(str::to_owned).as_ref())),
            },
            "day" => out.push_str(&numeric(at.day(), resolved.day.as_ref())),
            "hour" => {
                let hour = if twelve {
                    match at.hour() % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    at.hour()
                };
                out.push_str(&numeric(hour, resolved.hour.as_ref()));
            }
            "minute" => out.push_str(&numeric(at.minute(), resolved.minute.as_ref())),
            "second" => out.push_str(&numeric(at.second(), resolved.second.as_ref())),
            "dayPeriod" => {
                let index = usize::from(at.hour() >= 12);
                out.push_str(format.day_periods.get(index).map_or("", String::as_str));
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------- числа

/// Десятичная запись числа: цифры без ведущих нулей и число цифр до запятой
/// (`point`; ≤ 0 — нули после запятой). Пустые цифры — ноль.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decimal {
    digits: Vec<u8>,
    point: i32,
}

impl Decimal {
    /// Кратчайшая запись |x|, по которой x восстанавливается, — то же, с чего
    /// начинает ICU (double-conversion).
    fn of(x: f64) -> Decimal {
        let x = x.abs();
        if x == 0.0 {
            return Decimal {
                digits: Vec::new(),
                point: 1,
            };
        }
        let text = format!("{x:e}");
        let (mantissa, exponent) = text.split_once('e').expect("формат {:e}");
        let exponent: i32 = exponent.parse().expect("показатель {:e}");
        let mut digits: Vec<u8> = mantissa
            .bytes()
            .filter(u8::is_ascii_digit)
            .map(|b| b - b'0')
            .collect();
        while digits.last() == Some(&0) {
            digits.pop();
        }
        Decimal {
            digits,
            point: exponent + 1,
        }
    }

    fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// Показатель старшей цифры (getMagnitude ICU).
    fn magnitude(&self) -> i32 {
        self.point - 1
    }

    /// Умножить на 10^shift.
    fn shifted(&self, shift: i32) -> Decimal {
        Decimal {
            digits: self.digits.clone(),
            point: self.point + shift,
        }
    }

    /// Округлить до `fraction` цифр после запятой, половина — от нуля
    /// (halfExpand, по умолчанию у `Intl.NumberFormat`).
    fn rounded(&self, fraction: i32) -> Decimal {
        let keep = self.point + fraction;
        if keep >= self.digits.len() as i32 {
            return self.clone();
        }
        if keep < 0 {
            return Decimal::of(0.0);
        }
        let keep = keep as usize;
        let up = self.digits[keep] >= 5;
        let mut digits = self.digits[..keep].to_vec();
        let mut point = self.point;
        if up {
            let mut at = digits.len();
            loop {
                if at == 0 {
                    digits.insert(0, 1);
                    point += 1;
                    break;
                }
                at -= 1;
                if digits[at] == 9 {
                    digits[at] = 0;
                } else {
                    digits[at] += 1;
                    break;
                }
            }
        }
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            return Decimal::of(0.0);
        }
        Decimal { digits, point }
    }

    /// Целая часть с группами и дробная часть без хвостовых нулей.
    fn render(&self, group: &str, decimal: &str, min_grouping: usize) -> String {
        let digit = |index: i32| -> char {
            if index >= 0 && (index as usize) < self.digits.len() {
                char::from(b'0' + self.digits[index as usize])
            } else {
                '0'
            }
        };
        let integer: String = if self.point <= 0 {
            "0".to_owned()
        } else {
            (0..self.point).map(digit).collect()
        };
        let fraction: String = (self.point..self.digits.len() as i32).map(digit).collect();
        let mut out = String::new();
        let length = integer.len();
        let grouped = length >= 3 + min_grouping;
        for (index, c) in integer.chars().enumerate() {
            if grouped && index > 0 && (length - index).is_multiple_of(3) {
                out.push_str(group);
            }
            out.push(c);
        }
        if !fraction.is_empty() {
            out.push_str(decimal);
            out.push_str(&fraction);
        }
        out
    }
}

fn special(x: f64) -> Option<String> {
    if x.is_nan() {
        Some("NaN".to_owned())
    } else if x.is_infinite() {
        Some(if x < 0.0 { "-∞" } else { "∞" }.to_owned())
    } else {
        None
    }
}

/// Знак: минус у отрицательных, в том числе у −0 и округлённых до нуля
/// (signDisplay `auto` ECMA-402).
fn signed(x: f64, body: String) -> String {
    if x.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}

/// `x.toLocaleString(ru-RU | en-GB)`: до трёх знаков после запятой.
pub fn format_number(lang: Language, x: f64) -> String {
    if let Some(text) = special(x) {
        return text;
    }
    let numbers = &locale(lang).numbers;
    let body = Decimal::of(x).rounded(3).render(
        &numbers.group,
        &numbers.decimal,
        numbers.minimum_grouping_digits,
    );
    signed(x, body)
}

/// Наибольший показатель в таблице сокращений выгрузки.
const COMPACT_MAX: i32 = 18;

/// `new Intl.NumberFormat(…, {notation: "compact", maximumFractionDigits: 1}).format(x)`
/// (`SponsorsModal.vue` amountShort): «1,5 млн», «1.5m».
pub fn format_compact(lang: Language, x: f64) -> String {
    if let Some(text) = special(x) {
        return text;
    }
    let data = locale(lang);
    let pattern = |exponent: i32| -> &str {
        data.compact
            .by_exponent
            .get(&exponent.clamp(0, COMPACT_MAX).to_string())
            .map_or("{number}", String::as_str)
    };
    // Множитель — показатель начала полосы с тем же суффиксом (тыс. — 3, млн — 6…).
    let multiplier = |magnitude: i32| -> i32 {
        let magnitude = magnitude.clamp(0, COMPACT_MAX);
        let mut start = magnitude;
        while start > 0 && pattern(start - 1) == pattern(magnitude) {
            start -= 1;
        }
        start
    };
    let value = Decimal::of(x);
    let (scaled, exponent) = if value.is_zero() {
        (value, 0)
    } else {
        // CompactHandler ICU: множитель по величине, округление; если округление
        // дало следующий разряд (999,95 тыс.), множитель — по новой величине.
        let magnitude = value.magnitude();
        let shift = multiplier(magnitude);
        let mut scaled = value.shifted(-shift).rounded(1);
        let mut shift = shift;
        if !scaled.is_zero() && scaled.magnitude() == magnitude - shift + 1 {
            let bumped = multiplier(magnitude + 1);
            if bumped != shift {
                scaled = scaled.shifted(shift - bumped).rounded(1);
                shift = bumped;
            }
        }
        let exponent = if scaled.is_zero() {
            0
        } else {
            scaled.magnitude() + shift
        };
        (scaled, exponent)
    };
    let number = scaled.render(&data.numbers.group, &data.numbers.decimal, 2);
    signed(x, pattern(exponent).replace("{number}", &number))
}

/// Форма множественного числа (`Intl.PluralRules#select`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plural {
    One,
    Few,
    Many,
    Other,
}

impl Plural {
    /// Имя категории CLDR.
    pub fn as_str(self) -> &'static str {
        match self {
            Plural::One => "one",
            Plural::Few => "few",
            Plural::Many => "many",
            Plural::Other => "other",
        }
    }
}

/// Правила CLDR 48 для ru и en по операндам числа (i — целая часть, v — число
/// видимых знаков после запятой в кратчайшей записи).
pub fn plural(lang: Language, x: f64) -> Plural {
    if !x.is_finite() {
        return Plural::Other;
    }
    let value = Decimal::of(x);
    let fraction_digits = (value.digits.len() as i32 - value.point).max(0);
    let integer = x.abs().trunc();
    match lang {
        Language::En => {
            if integer == 1.0 && fraction_digits == 0 {
                Plural::One
            } else {
                Plural::Other
            }
        }
        Language::Ru => {
            if fraction_digits != 0 {
                return Plural::Other;
            }
            let i10 = integer % 10.0;
            let i100 = integer % 100.0;
            if i10 == 1.0 && i100 != 11.0 {
                Plural::One
            } else if (2.0..=4.0).contains(&i10) && !(12.0..=14.0).contains(&i100) {
                Plural::Few
            } else {
                Plural::Many
            }
        }
    }
}

// ---------------------------------------------------------------- страны

/// Страны в порядке `localeCompare` языка (`RunnerSettings.vue` countryList).
pub fn countries(lang: Language) -> &'static [Country] {
    &locale(lang).countries
}

/// Название страны по коду (`ProfileModal.vue` countryName): пустой код —
/// пустая строка, неизвестный — сам код.
pub fn country_name(lang: Language, code: &str) -> String {
    if code.is_empty() {
        return String::new();
    }
    countries(lang)
        .iter()
        .find(|country| country.id == code)
        .map_or_else(|| code.to_owned(), |country| country.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_rounds_half_away_from_zero_on_shortest_digits() {
        let r = |x: f64, f: i32| Decimal::of(x).rounded(f).render(",", ".", 1);
        assert_eq!(r(1.45, 1), "1.5");
        assert_eq!(r(0.0005, 3), "0.001");
        assert_eq!(r(0.00049, 3), "0");
        assert_eq!(r(999.9995, 3), "1,000");
        assert_eq!(r(9.96, 1), "10");
        assert_eq!(r(1234.0, 0), "1,234");
        assert_eq!(Decimal::of(1e21).render(",", ".", 2).len(), 29);
    }

    #[test]
    fn special_values() {
        assert_eq!(format_number(Language::Ru, f64::NAN), "NaN");
        assert_eq!(format_number(Language::En, f64::INFINITY), "∞");
        assert_eq!(format_number(Language::En, -0.0001), "-0");
        assert_eq!(
            format_date_in(Language::En, DateStyle::Time, f64::NAN, &chrono::Utc),
            INVALID_DATE
        );
    }

    #[test]
    fn country_names_fall_back_to_code() {
        assert_eq!(country_name(Language::Ru, "RU"), "Россия");
        assert_eq!(country_name(Language::En, "RU"), "Russia");
        assert_eq!(country_name(Language::En, ""), "");
        assert_eq!(country_name(Language::En, "ZZ"), "ZZ");
    }
}
