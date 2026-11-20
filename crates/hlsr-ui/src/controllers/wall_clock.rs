//! Календарные часы и пояс для текстов (`Date.now()`, `new Date(ts)` в местном поясе): время
//! уведомлений и «сегодня». Часы подменяются в тестах и снимках — как `Clock` панелей «Игр»,
//! но с доступом к полям даты.

use crate::intl::{self, DateStyle, SystemStyle};
use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone, Timelike, Utc};
use hlsr_core::language::Language;
use std::rc::Rc;

/// Часы и пояс; клон — те же часы.
#[derive(Clone)]
pub struct WallClock {
    now_ms: Rc<dyn Fn() -> i64>,
    /// Пояс; `None` — местный пояс ОС.
    offset: Option<FixedOffset>,
}

/// Дата и время в местном поясе (`getFullYear()`, `getMonth() + 1`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local4 {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl WallClock {
    /// Часы и пояс системы.
    pub fn system() -> WallClock {
        WallClock {
            now_ms: Rc::new(|| Utc::now().timestamp_millis()),
            offset: None,
        }
    }

    /// Свои часы и пояс в секундах к востоку от Гринвича (тесты, снимки).
    pub fn fixed(now_ms: i64, offset_seconds: i32) -> WallClock {
        WallClock {
            now_ms: Rc::new(move || now_ms),
            offset: FixedOffset::east_opt(offset_seconds),
        }
    }

    /// `Date.now()`.
    pub fn now_ms(&self) -> i64 {
        (self.now_ms)()
    }

    /// Поля даты момента `ms` в местном поясе; `None` — момент вне диапазона.
    pub fn local(&self, ms: i64) -> Option<Local4> {
        fn parts<Tz: TimeZone>(at: DateTime<Tz>) -> Local4 {
            Local4 {
                year: at.year(),
                month: at.month(),
                day: at.day(),
                hour: at.hour(),
                minute: at.minute(),
            }
        }
        match self.offset {
            Some(offset) => offset.timestamp_millis_opt(ms).single().map(parts),
            None => Local.timestamp_millis_opt(ms).single().map(parts),
        }
    }
}

impl WallClock {
    /// `new Date(ms).toLocale…String(…)` в поясе часов.
    pub fn format(&self, lang: Language, style: DateStyle, ms: f64) -> String {
        match self.offset {
            Some(offset) => intl::format_date_in(lang, style, ms, &offset),
            None => intl::format_date(lang, style, ms),
        }
    }
}

impl WallClock {
    /// `new Date(ms).toLocale…String([], …)`: «по локали системы» (язык интерфейса, отступление
    /// выгрузки И4) в поясе часов.
    pub fn format_system(&self, lang: Language, style: SystemStyle, ms: f64) -> String {
        let tag = intl::system_tag(lang);
        match self.offset {
            Some(offset) => intl::format_system_date_in(tag, style, ms, &offset),
            None => intl::format_system_date(tag, style, ms),
        }
    }
}

impl std::fmt::Debug for WallClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WallClock")
            .field("now_ms", &self.now_ms())
            .field("offset", &self.offset)
            .finish()
    }
}
