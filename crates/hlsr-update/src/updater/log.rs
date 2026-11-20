//! Журнал апдейтера (`updater/core/log.js`): `%LOCALAPPDATA%\HLSR\logs\updater.log`, строки
//! `[ISO] [level] текст` с маскированием (`redactText`), текст не длиннее 2000 единиц UTF-16;
//! файл от 512 КиБ уходит в `updater.log.1`. Ошибки записи журнала обновлению не мешают.

use crate::ports::{Clock, Logger, SystemClock};
use hlsr_core::js;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `MAX_BYTES`.
pub const MAX_BYTES: u64 = 512 * 1024;

/// `UpdaterLog`.
pub struct UpdaterLog {
    file: Option<PathBuf>,
    clock: Arc<dyn Clock>,
    /// `echo`: строка уходит и в `tracing` (в 0.4.7 — `console.log`).
    echo: bool,
}

impl UpdaterLog {
    pub fn new(file: Option<PathBuf>) -> UpdaterLog {
        UpdaterLog {
            file,
            clock: Arc::new(SystemClock),
            echo: true,
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> UpdaterLog {
        self.clock = clock;
        self
    }

    pub fn with_echo(mut self, echo: bool) -> UpdaterLog {
        self.echo = echo;
        self
    }

    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    fn rotate_if_needed(file: &Path) {
        let Ok(metadata) = std::fs::metadata(file) else {
            return;
        };
        if metadata.len() < MAX_BYTES {
            return;
        }
        let mut rotated = file.as_os_str().to_owned();
        rotated.push(".1");
        let _ = std::fs::rename(file, PathBuf::from(rotated));
    }

    /// `write(level, message)`.
    pub fn write(&self, level: &str, message: &str) {
        let safe = js::slice_utf16(&hlsr_core::redact::redact_text(message), 2000);
        let line = format!(
            "[{}] [{level}] {safe}",
            js::iso::iso_from_ms(self.clock.now_ms())
        );
        if self.echo {
            tracing::info!(target: "updater", "{line}");
        }
        let Some(file) = &self.file else {
            return;
        };
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::rotate_if_needed(file);
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut out| out.write_all(format!("{line}\n").as_bytes()));
    }
}

impl Logger for UpdaterLog {
    fn info(&self, message: &str) {
        self.write("info", message);
    }

    fn warn(&self, message: &str) {
        self.write("warn", message);
    }

    fn error(&self, message: &str) {
        self.write("error", message);
    }
}
