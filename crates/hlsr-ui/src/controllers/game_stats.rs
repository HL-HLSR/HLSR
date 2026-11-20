//! Три строки статистики героя (`GameStats.vue`, в 0.4.9 как в 0.4.7): последний запуск, время
//! в игре, обновление сборки. Пока статус доставки и статистика не пришли (оба `null`), строки
//! берутся из запасных данных игры (`demo`); после — только из ответов main.
//!
//! Даты считаются по местному времени, как `new Date(iso).getDate()`: «вчера» — это другой
//! календарный день, а не 24 часа назад.

use super::game_demo::GameDemo;
use super::jsv;
use crate::tr::tr;
use chrono::{Datelike, TimeZone};
use hlsr_core::js::{self, date_parse};
use hlsr_core::language::Language;
use serde_json::Value;

/// Значки строк (`fas fa-…`), одинаковые у всех игр.
pub const ICONS: [&str; 3] = ["fas fa-play-circle", "fas fa-clock", "fas fa-sync"];
/// Подписи строк (ключи локализации).
pub const LABELS: [&str; 3] = ["#L_LAST_LAUNCH", "#L_TOTAL_PLAYTIME", "#L_BUILD_UPDATED"];

/// Строка статистики: значок, подпись, значение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsRow {
    pub icon: &'static str,
    pub label: String,
    pub value: String,
}

/// Что нужно строкам: язык, запасные данные игры, ответы main и часы.
pub struct Input<'a, Tz: TimeZone> {
    pub lang: Language,
    pub demo: &'a GameDemo,
    /// `playtimeHours` игры (готовая строка, «36,5»).
    pub playtime_hours: &'a str,
    /// `game-stats-get`; `None` — ещё не пришёл (`null`).
    pub stats: Option<&'a Value>,
    /// `DeliveryStatus` игры; `None` — ещё не пришёл.
    pub delivery: Option<&'a Value>,
    pub tz: &'a Tz,
    /// `Date.now()`, мс.
    pub now_ms: i64,
}

/// `rows`: три строки по порядку.
pub fn rows<Tz: TimeZone>(input: &Input<'_, Tz>) -> [StatsRow; 3] {
    let values = [
        last_launch_text(input),
        playtime_text(input),
        build_updated_text(input),
    ];
    let mut index = 0;
    values.map(|value| {
        let row = StatsRow {
            icon: ICONS[index],
            label: tr(input.lang, LABELS[index], &[]),
            value,
        };
        index += 1;
        row
    })
}

/// `honest`: хоть что-то пришло от main.
fn honest<Tz: TimeZone>(input: &Input<'_, Tz>) -> bool {
    input.delivery.is_some() || input.stats.is_some()
}

fn no_data<Tz: TimeZone>(input: &Input<'_, Tz>) -> String {
    tr(input.lang, "#L_NO_DATA", &[])
}

/// `stats && stats.available` (`available` истинно).
fn stats_available(stats: Option<&Value>) -> bool {
    jsv::truthy(jsv::field(stats, "available"))
}

fn last_launch_text<Tz: TimeZone>(input: &Input<'_, Tz>) -> String {
    let lang = input.lang;
    if honest(input) {
        let at = jsv::field(input.stats, "lastLaunchAt");
        if !stats_available(input.stats) || !jsv::truthy(at) {
            return no_data(input);
        }
        return relative_date(input, at.expect("истинное значение есть"));
    }
    match input.demo.last_launch_days {
        None => tr(lang, "#L_NEVER", &[]),
        Some(0) => tr(lang, "#L_TODAY", &[]),
        Some(1) => tr(lang, "#L_YESTERDAY", &[]),
        Some(days) => tr(lang, "#L_DAYS_AGO", &[&days.to_string()]),
    }
}

fn playtime_text<Tz: TimeZone>(input: &Input<'_, Tz>) -> String {
    if honest(input) {
        if !stats_available(input.stats) {
            return no_data(input);
        }
        let seconds = jsv::or_zero(jsv::field(input.stats, "totalPlaySeconds"));
        return duration_text(input.lang, seconds);
    }
    tr(input.lang, "#L_HOURS", &[input.playtime_hours])
}

fn build_updated_text<Tz: TimeZone>(input: &Input<'_, Tz>) -> String {
    if honest(input) {
        let at = jsv::field(input.delivery, "buildDate");
        if !jsv::truthy(at) {
            return no_data(input);
        }
        return absolute_date(input, at.expect("истинное значение есть"));
    }
    input.demo.build_updated.unwrap_or_default().to_owned()
}

/// `new Date(value).getTime()`: строка ISO (и прочие форматы `Date.parse`) или число мс;
/// `None` — `NaN` (Invalid Date).
fn parse_ms(value: &Value) -> Option<i64> {
    match value {
        Value::String(text) => date_parse(text),
        Value::Number(_) => {
            let ms = js::value::number(value)?;
            // TimeClip: |ms| ≤ 8,64e15.
            (ms.is_finite() && ms.abs() <= 8.64e15).then(|| ms.trunc() as i64)
        }
        _ => None,
    }
}

/// Полночь местного дня `ms`: `new Date(y, m, d).getTime()`.
fn start_of_day<Tz: TimeZone>(tz: &Tz, ms: i64) -> Option<i64> {
    let moment = tz.timestamp_millis_opt(ms).single()?;
    let midnight = tz.with_ymd_and_hms(moment.year(), moment.month(), moment.day(), 0, 0, 0);
    // В переходе на летнее время полночи может не быть: `new Date` сдвигает вперёд.
    let found = midnight.clone().earliest().or_else(|| midnight.latest())?;
    Some(found.timestamp_millis())
}

/// `relativeDate(iso)`.
fn relative_date<Tz: TimeZone>(input: &Input<'_, Tz>, iso: &Value) -> String {
    let lang = input.lang;
    let Some(ms) = parse_ms(iso) else {
        return no_data(input);
    };
    let (Some(today), Some(day)) = (
        start_of_day(input.tz, input.now_ms),
        start_of_day(input.tz, ms),
    ) else {
        return no_data(input);
    };
    let days = jsv::round((today - day) as f64 / 86_400_000.0);
    if days <= 0.0 {
        tr(lang, "#L_TODAY", &[])
    } else if days == 1.0 {
        tr(lang, "#L_YESTERDAY", &[])
    } else if days <= 6.0 {
        tr(lang, "#L_DAYS_AGO", &[&jsv::number_text(days)])
    } else {
        absolute_date(input, iso)
    }
}

/// `absoluteDate(iso)`: «дд.мм.гггг» по местному времени.
fn absolute_date<Tz: TimeZone>(input: &Input<'_, Tz>, iso: &Value) -> String {
    let Some(ms) = parse_ms(iso) else {
        return no_data(input);
    };
    let Some(moment) = input.tz.timestamp_millis_opt(ms).single() else {
        return no_data(input);
    };
    format!(
        "{:02}.{:02}.{}",
        moment.day(),
        moment.month(),
        moment.year()
    )
}

/// `durationText(totalSeconds)`: «1 ч 5 мин» или «42 мин».
pub fn duration_text(lang: Language, total_seconds: f64) -> String {
    let minutes = (total_seconds / 60.0).floor();
    let hours = (minutes / 60.0).floor();
    let rest = minutes % 60.0;
    if hours > 0.0 {
        return tr(
            lang,
            "#L_DUR_HM",
            &[&jsv::number_text(hours), &jsv::number_text(rest)],
        );
    }
    tr(lang, "#L_DUR_M", &[&jsv::number_text(minutes)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};
    use serde_json::json;

    /// 2026-10-16T12:00:00Z.
    const NOW: i64 = 1_792_152_000_000;

    fn texts(
        lang: Language,
        demo: &GameDemo,
        stats: Option<Value>,
        delivery: Option<Value>,
    ) -> Vec<String> {
        let input = Input {
            lang,
            demo,
            playtime_hours: "36,5",
            stats: stats.as_ref(),
            delivery: delivery.as_ref(),
            tz: &Utc,
            now_ms: NOW,
        };
        rows(&input).into_iter().map(|row| row.value).collect()
    }

    fn launched(at: &str) -> Vec<String> {
        texts(
            Language::En,
            &GameDemo::default(),
            Some(json!({"available": true, "lastLaunchAt": at, "totalPlaySeconds": 0})),
            None,
        )
    }

    #[test]
    fn before_anything_arrives_the_demo_values_show() {
        let demo = GameDemo {
            last_launch_days: Some(6),
            build_updated: Some("02.02.2025 19:05"),
            ..GameDemo::default()
        };
        assert_eq!(
            texts(Language::En, &demo, None, None),
            ["6 days ago", "36,5 h", "02.02.2025 19:05"]
        );
        assert_eq!(
            texts(Language::Ru, &demo, None, None),
            ["6 дн. назад", "36,5 ч.", "02.02.2025 19:05"]
        );
        let fresh = GameDemo::default();
        assert_eq!(
            texts(Language::En, &fresh, None, None),
            ["Never", "36,5 h", ""]
        );
        for (days, text) in [(0, "Today"), (1, "Yesterday")] {
            let demo = GameDemo {
                last_launch_days: Some(days),
                ..GameDemo::default()
            };
            assert_eq!(texts(Language::En, &demo, None, None)[0], text);
        }
    }

    #[test]
    fn labels_and_icons_are_fixed() {
        let input = Input {
            lang: Language::Ru,
            demo: &GameDemo::default(),
            playtime_hours: "0",
            stats: None,
            delivery: None,
            tz: &Utc,
            now_ms: NOW,
        };
        let rows = rows(&input);
        assert_eq!(
            rows.iter().map(|row| row.icon).collect::<Vec<_>>(),
            ["fas fa-play-circle", "fas fa-clock", "fas fa-sync"]
        );
        assert_eq!(rows[0].label, "Последний запуск");
        assert_eq!(rows[1].label, "Общее время игры");
        assert_eq!(rows[2].label, "Последнее обновление сборки");
    }

    #[test]
    fn once_main_answers_missing_data_says_no_data() {
        // Статистики нет (игра не запускалась): ответ {available: false}.
        let none = json!({"available": false});
        assert_eq!(
            texts(Language::En, &GameDemo::default(), Some(none), None),
            ["No data", "No data", "No data"]
        );
        // Доставка пришла, статистики нет: тоже честный режим.
        assert_eq!(
            texts(
                Language::En,
                &GameDemo::default(),
                None,
                Some(json!({"buildDate": null}))
            ),
            ["No data", "No data", "No data"]
        );
        assert_eq!(
            texts(
                Language::Ru,
                &GameDemo::default(),
                Some(json!({"available": true, "lastLaunchAt": null})),
                None
            )[0],
            "Нет данных"
        );
    }

    #[test]
    fn last_launch_counts_calendar_days_up_to_six_then_the_date() {
        assert_eq!(launched("2026-10-16T00:00:01Z")[0], "Today");
        assert_eq!(launched("2026-10-15T23:59:59Z")[0], "Yesterday");
        assert_eq!(launched("2026-10-14T05:00:00Z")[0], "2 days ago");
        assert_eq!(launched("2026-10-10T23:00:00Z")[0], "6 days ago");
        assert_eq!(launched("2026-10-09T23:00:00Z")[0], "09.10.2026");
        assert_eq!(launched("2025-01-02T10:00:00.000Z")[0], "02.01.2025");
        // Дата в будущем — «сегодня» (days ≤ 0).
        assert_eq!(launched("2026-10-20T00:00:00Z")[0], "Today");
        // Не дата — «нет данных».
        assert_eq!(launched("давно")[0], "No data");
    }

    /// Значение первой строки при часах `now_ms` и поясе `tz`.
    fn launch_text<Tz: TimeZone>(tz: &Tz, now_ms: i64, at: &str) -> String {
        let stats = json!({"available": true, "lastLaunchAt": at});
        let input = Input {
            lang: Language::En,
            demo: &GameDemo::default(),
            playtime_hours: "0",
            stats: Some(&stats),
            delivery: None,
            tz,
            now_ms,
        };
        rows(&input)[0].value.clone()
    }

    #[test]
    fn days_are_local_calendar_days() {
        let zone = FixedOffset::east_opt(3 * 3600).unwrap();
        // Сейчас 15:00 16 октября по местному (12:00Z). Запуск в 01:30 того же дня
        // (22:30Z предыдущего) — «сегодня» по местному, «вчера» по Гринвичу.
        assert_eq!(launch_text(&zone, NOW, "2026-10-15T22:30:00Z"), "Today");
        assert_eq!(launch_text(&Utc, NOW, "2026-10-15T22:30:00Z"), "Yesterday");
        // 23:59 15-го по местному — вчера, хотя прошло меньше 16 часов.
        assert_eq!(launch_text(&zone, NOW, "2026-10-15T20:59:00Z"), "Yesterday");
        // Ночью (00:30 17-го по местному) запуск в 15:00 16-го — уже «вчера»;
        // по Гринвичу ещё один день.
        let night = NOW + 9 * 3_600_000 + 30 * 60_000;
        assert_eq!(
            launch_text(&zone, night, "2026-10-16T12:00:00Z"),
            "Yesterday"
        );
        assert_eq!(launch_text(&Utc, night, "2026-10-16T12:00:00Z"), "Today");
    }

    #[test]
    fn playtime_is_hours_and_minutes() {
        let at = |seconds: Value| {
            texts(
                Language::En,
                &GameDemo::default(),
                Some(json!({"available": true, "totalPlaySeconds": seconds})),
                None,
            )[1]
            .clone()
        };
        assert_eq!(at(json!(0)), "0 min");
        assert_eq!(at(json!(59)), "0 min");
        assert_eq!(at(json!(60)), "1 min");
        assert_eq!(at(json!(3599)), "59 min");
        assert_eq!(at(json!(3600)), "1 h 0 min");
        assert_eq!(at(json!(3725)), "1 h 2 min");
        assert_eq!(at(json!(36 * 3600 + 1800)), "36 h 30 min");
        // Нет поля или null — 0.
        assert_eq!(at(json!(null)), "0 min");
        let ru = texts(
            Language::Ru,
            &GameDemo::default(),
            Some(json!({"available": true, "totalPlaySeconds": 7380})),
            None,
        );
        assert_eq!(ru[1], "2 ч 3 мин");
    }

    #[test]
    fn build_date_is_absolute_in_local_time() {
        let delivery = json!({"buildDate": "2025-01-17T12:44:00.000Z"});
        assert_eq!(
            texts(Language::En, &GameDemo::default(), None, Some(delivery))[2],
            "17.01.2025"
        );
        let zone = FixedOffset::east_opt(3 * 3600).unwrap();
        let delivery = json!({"buildDate": "2025-01-17T22:30:00Z"});
        let input = Input {
            lang: Language::En,
            demo: &GameDemo::default(),
            playtime_hours: "0",
            stats: None,
            delivery: Some(&delivery),
            tz: &zone,
            now_ms: NOW,
        };
        // 01:30 следующего дня по местному.
        assert_eq!(rows(&input)[2].value, "18.01.2025");
        let bad = json!({"buildDate": "не дата"});
        let input = Input {
            delivery: Some(&bad),
            ..input
        };
        assert_eq!(rows(&input)[2].value, "No data");
    }
}
