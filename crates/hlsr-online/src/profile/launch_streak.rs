//! Дни подряд, в которые открывали лаунчер или запускали игру (`profile/launch-streak.js` 0.4.9), —
//! для серий 7/14/30/100 дней и сводки профиля. День — по местному времени
//! (`getFullYear/getMonth/getDate`), файл `launch-days.json` `{last, streak, best}` (компактный
//! JSON через `.tmp`). Пропуск дня начинает серию с единицы; `best` — рекорд.

use chrono::{
    Datelike, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeDelta,
    TimeZone,
};
use hlsr_core::json_formats::{TmpOptions, write_compact_tmp};
use hlsr_core::json_text::json_parse;
use serde_json::{Value, json};
use std::path::Path;

use crate::js_value::safe_integer;

pub const FILE_NAME: &str = "launch-days.json";
pub const GOALS: [(&str, i64); 4] = [
    ("week-streak", 7),
    ("hard-worker", 14),
    ("day-after-day", 30),
    ("keep-the-flame", 100),
];

const INVALID_DAY: &str = "NaN-NaN-NaN";

/// Часовой пояс, по которому считаются дни: местный (в работе) или заданный смещением от UTC
/// (тесты службы профиля: их сверка с Node не должна зависеть от пояса машины).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DayZone {
    #[default]
    Local,
    /// Секунды к востоку от UTC.
    Fixed(i32),
}

fn format_day(date: NaiveDate) -> String {
    format!("{}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// `dayOf(time)`: `YYYY-MM-DD` местного дня момента `time` (мс от 1970 года).
pub fn day_of(time: i64) -> String {
    day_of_in(&Local, time)
}

fn day_of_in<Tz: TimeZone>(zone: &Tz, time: i64) -> String {
    match zone.timestamp_millis_opt(time) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => format_day(at.date_naive()),
        LocalResult::None => INVALID_DAY.to_owned(),
    }
}

/// Местное время → момент, как `new Date(y, m, d)` у V8: в пропущенный час (переход на летнее
/// время) — по смещению до перехода, в повторённый — первое из двух.
fn local_to_utc_ms<Tz: TimeZone>(zone: &Tz, naive: NaiveDateTime) -> Option<i64> {
    match zone.from_local_datetime(&naive) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => Some(at.timestamp_millis()),
        LocalResult::None => {
            // Смещение за двое суток до перехода — то, что действовало перед ним.
            let before = zone
                .offset_from_utc_datetime(&(naive - TimeDelta::hours(48)))
                .fix()
                .local_minus_utc();
            Some(
                (naive - TimeDelta::seconds(i64::from(before)))
                    .and_utc()
                    .timestamp_millis(),
            )
        }
    }
}

/// `new Date(year, month - 1, date - 1)`: месяц и день вне пределов переносятся, год 0–99 — это
/// 1900–1999.
fn make_date(year: i64, month: i64, day: i64) -> Option<NaiveDate> {
    let year = if (0..=99).contains(&year) {
        1900 + year
    } else {
        year
    };
    let year = year + month.div_euclid(12);
    let month = month.rem_euclid(12) + 1;
    let first = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, u32::try_from(month).ok()?, 1)?;
    first.checked_add_signed(TimeDelta::days(day - 1))
}

/// `previousDay(day)`: местный день накануне `day` (`YYYY-MM-DD`).
pub fn previous_day(day: &str) -> String {
    previous_day_in(&Local, day)
}

fn previous_day_in<Tz: TimeZone>(zone: &Tz, day: &str) -> String {
    let parts: Vec<Option<i64>> = day
        .split('-')
        .map(|part| {
            let number = hlsr_core::js::to_number(part);
            (number.is_finite() && number.fract() == 0.0).then_some(number as i64)
        })
        .collect();
    let (Some(Some(year)), Some(Some(month)), Some(Some(date))) =
        (parts.first(), parts.get(1), parts.get(2))
    else {
        return INVALID_DAY.to_owned();
    };
    make_date(*year, month - 1, date - 1)
        .and_then(|date| local_to_utc_ms(zone, date.and_hms_opt(0, 0, 0)?))
        .map_or_else(|| INVALID_DAY.to_owned(), |time| day_of_in(zone, time))
}

/// `/^\d{4}-\d{2}-\d{2}$/`.
fn is_day(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// Сохранённое в файле: день, серия и рекорд (`Number.isSafeInteger(best)`).
struct Saved {
    last: String,
    streak: i64,
    best: Option<i64>,
}

impl Saved {
    /// `bestOf(saved)`: рекорд; в файле до 0.4.9 его нет — тогда это текущая серия.
    fn best(&self) -> i64 {
        self.streak.max(self.best.unwrap_or(0))
    }
}

/// `read(file)`: сохранённое или `None` (нет файла, испорчен, неверные поля).
fn read(file: &Path) -> Option<Saved> {
    let bytes = std::fs::read(file).ok()?;
    let data = json_parse(&String::from_utf8_lossy(&bytes)).ok()?;
    let last = data
        .get("last")
        .and_then(Value::as_str)
        .filter(|last| is_day(last))?;
    let streak = safe_integer(data.get("streak")).filter(|streak| *streak > 0)?;
    Some(Saved {
        last: last.to_owned(),
        streak,
        best: safe_integer(data.get("best")),
    })
}

/// Обобщённая функция в поясе дней: со смещением или местном (невозможное смещение — местный).
macro_rules! in_zone {
    ($zone:expr, |$tz:ident| $body:expr) => {
        match $zone {
            DayZone::Fixed(seconds) => match FixedOffset::east_opt(seconds) {
                Some($tz) => $body,
                None => {
                    let $tz = Local;
                    $body
                }
            },
            DayZone::Local => {
                let $tz = Local;
                $body
            }
        }
    };
}

/// `record(dir, now)`: отметить день и вернуть, сколько дней подряд набралось. Сбой записи не
/// мешает: серия просто начнётся заново.
pub fn record(dir: &Path, now: i64) -> i64 {
    record_in_zone(dir, now, DayZone::Local)
}

/// [`record`] в заданном поясе дней.
pub fn record_in_zone(dir: &Path, now: i64, zone: DayZone) -> i64 {
    in_zone!(zone, |tz| record_in(&tz, dir, now))
}

fn record_in<Tz: TimeZone>(zone: &Tz, dir: &Path, now: i64) -> i64 {
    let file = dir.join(FILE_NAME);
    let today = day_of_in(zone, now);
    let saved = read(&file);
    // Сохранённый день позже сегодняшнего (часы перевели назад, перелёт на запад) — тот же день:
    // серия не обнуляется, last не переписывается. Строки ГГГГ-ММ-ДД сравниваются как даты.
    if let Some(saved) = &saved
        && saved.last >= today
    {
        return saved.streak;
    }
    let streak = match &saved {
        Some(saved) if saved.last == previous_day_in(zone, &today) => saved.streak + 1,
        _ => 1,
    };
    let best = streak.max(saved.as_ref().map_or(0, Saved::best));
    let _ = write_compact_tmp(
        &file,
        &json!({"last": today, "streak": streak, "best": best}),
        TmpOptions::default(),
    );
    streak
}

/// `stats(dir, now)`: для сводки `{days, best}` — серия, если последний день вчера или позже, и
/// рекорд; файла нет — `null`.
pub fn stats(dir: &Path, now: i64) -> Value {
    stats_in_zone(dir, now, DayZone::Local)
}

/// [`stats`] в заданном поясе дней.
pub fn stats_in_zone(dir: &Path, now: i64, zone: DayZone) -> Value {
    let Some(saved) = read(&dir.join(FILE_NAME)) else {
        return Value::Null;
    };
    let yesterday = in_zone!(zone, |tz| previous_day_in(&tz, &day_of_in(&tz, now)));
    let days = if saved.last >= yesterday {
        saved.streak
    } else {
        0
    };
    json!({"days": days, "best": saved.best()})
}

/// [`day_of`] в заданном поясе дней.
pub fn day_of_in_zone(time: i64, zone: DayZone) -> String {
    in_zone!(zone, |tz| day_of_in(&tz, time))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-16T12:00:00Z.
    const NOON_UTC: i64 = 1_792_152_000_000;
    const HOUR: i64 = 3_600_000;

    fn streak_file(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(FILE_NAME)).unwrap()
    }

    /// Заданный пояс не зависит от пояса машины: день и серия считаются по его смещению.
    #[test]
    fn fixed_zone_ignores_the_machine_zone() {
        let dir = tempfile::tempdir().unwrap();
        let utc = DayZone::Fixed(0);
        std::fs::write(
            dir.path().join(FILE_NAME),
            r#"{"last":"2026-10-15","streak":6}"#,
        )
        .unwrap();
        assert_eq!(record_in_zone(dir.path(), NOON_UTC, utc), 7);
        assert_eq!(
            streak_file(dir.path()),
            r#"{"last":"2026-10-16","streak":7,"best":7}"#
        );
        // Тот же день: серия не растёт и файл не меняется.
        assert_eq!(record_in_zone(dir.path(), NOON_UTC + 3 * HOUR, utc), 7);
        // Следующий день по UTC — продолжение.
        assert_eq!(record_in_zone(dir.path(), NOON_UTC + 12 * HOUR, utc), 8);
        // Пропущенный день — начало заново.
        assert_eq!(record_in_zone(dir.path(), NOON_UTC + 60 * HOUR, utc), 1);
    }

    #[test]
    fn fixed_zone_shifts_the_day_boundary() {
        // В UTC+9 момент 15:00Z — уже 17 октября, в UTC−5 — ещё 16-е.
        let at = NOON_UTC + 3 * HOUR;
        assert_eq!(
            day_of_in(&FixedOffset::east_opt(0).unwrap(), at),
            "2026-10-16"
        );
        assert_eq!(
            day_of_in(&FixedOffset::east_opt(9 * 3600).unwrap(), at),
            "2026-10-17"
        );
        assert_eq!(
            day_of_in(&FixedOffset::west_opt(5 * 3600).unwrap(), at),
            "2026-10-16"
        );
        let nine = FixedOffset::east_opt(9 * 3600).unwrap();
        assert_eq!(previous_day_in(&nine, "2026-10-17"), "2026-10-16");
        assert_eq!(previous_day_in(&nine, "2026-03-01"), "2026-02-28");
        assert_eq!(previous_day_in(&nine, "2024-03-01"), "2024-02-29");
    }

    #[test]
    fn impossible_offset_falls_back_to_the_local_zone() {
        let dir = tempfile::tempdir().unwrap();
        let broken = DayZone::Fixed(i32::MAX);
        assert_eq!(record_in_zone(dir.path(), NOON_UTC, broken), 1);
        assert_eq!(
            streak_file(dir.path()),
            format!(r#"{{"last":"{}","streak":1,"best":1}}"#, day_of(NOON_UTC))
        );
    }
}
