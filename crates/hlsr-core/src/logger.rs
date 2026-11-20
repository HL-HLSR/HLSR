//! Журнал служб: `logger` модулей 0.4.7 (`{info, warn, error}`). Реализация приложения пишет в
//! `tracing` (строки `main.log` как у electron-log), тестовая — в память. Общий для доставки и онлайна.

use std::sync::Mutex;

/// Журнал: `logger` служб 0.4.7 (`electron-log` без области; префикс вроде
/// `[game-delivery]` — часть текста).
pub trait Logger: Send + Sync {
    fn info(&self, message: &str);
    fn warn(&self, message: &str);
    fn error(&self, message: &str);
}

/// Журнал приложения: события `tracing` (в `main.log` — строки как у electron-log).
#[derive(Debug, Clone, Copy, Default)]
pub struct TracingLogger;

impl Logger for TracingLogger {
    fn info(&self, message: &str) {
        tracing::info!("{message}");
    }
    fn warn(&self, message: &str) {
        tracing::warn!("{message}");
    }
    fn error(&self, message: &str) {
        tracing::error!("{message}");
    }
}

/// Журнал в памяти для тестов: (`info`/`warn`/`error`, текст).
#[derive(Debug, Default)]
pub struct MemoryLogger {
    lines: Mutex<Vec<(&'static str, String)>>,
}

impl MemoryLogger {
    /// Строки с прошлого вызова.
    pub fn take(&self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut *self.lines.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn push(&self, level: &'static str, message: &str) {
        self.lines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((level, message.to_owned()));
    }
}

impl Logger for MemoryLogger {
    fn info(&self, message: &str) {
        self.push("info", message);
    }
    fn warn(&self, message: &str) {
        self.push("warn", message);
    }
    fn error(&self, message: &str) {
        self.push("error", message);
    }
}
