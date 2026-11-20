//! Очередь событий телеметрии (`telemetry/Queue.js`): файл `telemetry-queue.jsonl`, одно событие
//! на строку (компактный JSON и `\n`). Новое событие дописывается; при переполнении (1000 событий
//! или 1 МиБ) файл перезаписывается через `<файл>.<pid>.tmp` без самых старых. Если битых строк не
//! меньше, чем целых, файл уходит в карантин `<файл>.corrupt-<время36>`. `generation` растёт при
//! `purge`: отправка, начатая до очистки, не удаляет события, записанные после неё.

use hlsr_core::js::{self, fs as js_fs};
use hlsr_core::json_formats::{append_jsonl_line, compact, rewrite_jsonl};
use hlsr_core::json_text::json_parse;
use hlsr_delivery::ports::Logger;
use hlsr_delivery::timing::Clock;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Предел строки события в байтах UTF-8.
const MAX_LINE_BYTES: usize = 8 * 1024;

/// `readAll()`: события и число битых строк.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadAll {
    pub events: Vec<Value>,
    pub broken: usize,
}

pub struct TelemetryQueue {
    file: PathBuf,
    max_events: usize,
    max_bytes: usize,
    logger: Arc<dyn Logger>,
    clock: Clock,
    quarantined: bool,
    count: Option<usize>,
    generation: u64,
}

impl TelemetryQueue {
    /// `new TelemetryQueue({file, maxEvents, maxBytes, logger})`; 0 в пределах — умолчание
    /// (1000 событий, 1 МиБ), как `opts.maxEvents || 1000`.
    pub fn new(
        file: impl Into<PathBuf>,
        max_events: usize,
        max_bytes: usize,
        logger: Arc<dyn Logger>,
        clock: Clock,
    ) -> TelemetryQueue {
        TelemetryQueue {
            file: file.into(),
            max_events: if max_events == 0 { 1000 } else { max_events },
            max_bytes: if max_bytes == 0 {
                1024 * 1024
            } else {
                max_bytes
            },
            logger,
            clock,
            quarantined: false,
            count: None,
            generation: 0,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// Номер очистки: растёт при каждом [`TelemetryQueue::purge`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn exists(&self) -> bool {
        self.file.exists()
    }

    /// `readAll()`: события-объекты из строк файла. Битых строк (не JSON или не объект) не меньше,
    /// чем целых, — карантин и пустая очередь.
    pub fn read_all(&mut self) -> ReadAll {
        if !self.exists() {
            return ReadAll::default();
        }
        let raw = match js_fs::read_file(&self.file) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => {
                self.logger.warn(&format!(
                    "[telemetry] очередь не читается: {}",
                    js::errno::fs_message(&error, "open", &self.file.to_string_lossy(), None)
                ));
                return ReadAll::default();
            }
        };
        let mut events = Vec::new();
        let mut broken = 0;
        for line in raw.split('\n') {
            let trimmed = js::trim(line);
            if trimmed.is_empty() {
                continue;
            }
            match json_parse(trimmed) {
                Ok(event @ Value::Object(_)) => events.push(event),
                _ => broken += 1,
            }
        }
        if broken > 0 && broken >= events.len() {
            self.quarantine("нечитаемое содержимое");
            self.count = Some(0);
            return ReadAll {
                events: Vec::new(),
                broken,
            };
        }
        self.count = Some(events.len());
        ReadAll { events, broken }
    }

    /// `append(event)`: дописать событие; `false` — очередь в карантине, событие больше 8 КиБ или
    /// запись не удалась.
    pub fn append(&mut self, event: &Value) -> bool {
        if self.quarantined {
            return false;
        }
        let line = compact(event);
        if line.len() > MAX_LINE_BYTES {
            self.logger
                .warn("[telemetry] событие слишком велико и отброшено");
            return false;
        }
        if self.count.is_none() {
            self.read_all();
        }
        if let Err(error) = append_jsonl_line(&self.file, &line) {
            self.logger.warn(&format!(
                "[telemetry] запись в очередь не удалась: {}",
                error.message()
            ));
            return false;
        }
        self.count = Some(self.count.unwrap_or(0) + 1);
        self.enforce_limits();
        true
    }

    /// `enforceLimits()`: при переполнении оставить последние `maxEvents` событий, затем выбросить
    /// старые, пока файл больше `maxBytes`.
    fn enforce_limits(&mut self) {
        let Ok(size) = std::fs::metadata(&self.file).map(|meta| meta.len() as usize) else {
            return;
        };
        if size <= self.max_bytes && self.count.unwrap_or(0) <= self.max_events {
            return;
        }
        let events = self.read_all().events;
        if events.len() <= self.max_events && size <= self.max_bytes {
            return;
        }
        let mut serialized: Vec<String> = events[events.len().saturating_sub(self.max_events)..]
            .iter()
            .map(compact)
            .collect();
        let mut bytes: usize = serialized.iter().map(|line| line.len() + 1).sum();
        let mut first = 0;
        while bytes > self.max_bytes && first < serialized.len() {
            bytes -= serialized[first].len() + 1;
            first += 1;
        }
        serialized.drain(..first);
        let dropped = events.len() - serialized.len();
        if dropped > 0 {
            self.logger.warn(&format!(
                "[telemetry] очередь переполнена, отброшено самых старых событий: {dropped}"
            ));
        }
        self.rewrite(&serialized);
    }

    /// `rewrite(lines)`: заменить содержимое через `<файл>.<pid>.tmp`.
    fn rewrite(&mut self, lines: &[String]) {
        if lines.is_empty() && !self.exists() {
            self.count = Some(0);
            return;
        }
        match rewrite_jsonl(&self.file, lines) {
            Ok(()) => self.count = Some(lines.len()),
            Err(error) => self.logger.warn(&format!(
                "[telemetry] перезапись очереди не удалась: {}",
                error.message()
            )),
        }
    }

    /// `peek(maxEvents, maxBytes)`: пачка с начала очереди — не больше `max_events` событий и
    /// `max_bytes` байт тела (`2 + Σ(длина + 1)`); первое событие берётся всегда.
    pub fn peek(&mut self, max_events: usize, max_bytes: usize) -> Vec<Value> {
        let events = self.read_all().events;
        let mut batch = Vec::new();
        let mut bytes = 2;
        for event in events {
            if batch.len() >= max_events {
                break;
            }
            let size = compact(&event).len() + 1;
            if !batch.is_empty() && bytes + size > max_bytes {
                break;
            }
            bytes += size;
            batch.push(event);
        }
        batch
    }

    /// `remove(count)`: убрать первые `count` событий.
    pub fn remove(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let events = self.read_all().events;
        let kept: Vec<String> = events.iter().skip(count).map(compact).collect();
        self.rewrite(&kept);
    }

    /// `size()`: число целых событий.
    pub fn size(&mut self) -> usize {
        self.read_all().events.len()
    }

    /// `purge()`: удалить файл очереди; `generation` растёт.
    pub fn purge(&mut self) -> bool {
        if self.exists()
            && let Err(error) = std::fs::remove_file(&self.file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            self.logger.warn(&format!(
                "[telemetry] очередь не удалена: {}",
                js::errno::fs_message(&error, "rm", &self.file.to_string_lossy(), None)
            ));
            return false;
        }
        self.count = Some(0);
        self.generation += 1;
        true
    }

    /// `quarantine(reason)`: переименовать файл в `<файл>.corrupt-<время36>`; не вышло — удалить.
    fn quarantine(&mut self, reason: &str) {
        if self.quarantined {
            return;
        }
        self.quarantined = true;
        let millis = u128::try_from((self.clock)()).unwrap_or(0);
        let mut target = self.file.as_os_str().to_owned();
        target.push(format!(".corrupt-{}", js::base36(millis)));
        let target = PathBuf::from(target);
        match std::fs::rename(&self.file, &target) {
            Ok(()) => self.logger.warn(&format!(
                "[telemetry] очередь карантинирована ({reason}): {}",
                target
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default()
            )),
            Err(_) => {
                let _ = std::fs::remove_file(&self.file);
            }
        }
        self.quarantined = false;
    }
}
