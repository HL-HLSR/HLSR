//! Центр уведомлений 0.4.7 (`notifications/NotificationStore.js`): файл `notifications.json`
//! `{version: 1, items}` (запись `writeJsonAtomic`), не больше 100 записей, `detail` после
//! `redactText` не длиннее 500 единиц UTF-16, белый список `meta`, склейка с последним
//! непрочитанным тем же уведомлением за минуту (`count++`). Им пользуются все подпроекты:
//! ошибка запуска (`STARTUP_ERROR`), доставка, Proton, модерация, события версий.

pub mod catalog;

use crate::js;
use crate::json_store::{read_json_safe, write_state};
use crate::json_text::{JsonFormat, js_number};
use crate::logger::{Logger, TracingLogger};
use crate::redact::redact_text;
use serde_json::{Map, Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const FILE_VERSION: u64 = 1;
pub const FILE_NAME: &str = "notifications.json";
pub const MAX_ITEMS: usize = 100;
pub const MAX_DETAIL: usize = 500;
/// `message.slice(0, 400)` в `reportStartupError`.
const STARTUP_DETAIL: usize = 400;
pub const COALESCE_WINDOW_MS: f64 = 60_000.0;
pub const LEVELS: [&str; 3] = ["error", "warning", "info"];

/// Часы: миллисекунды от 1970 года (`Date.now()`).
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Сводка для окна (`notifications-changed`): `{unread, total}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub unread: usize,
    pub total: usize,
}

/// Подписчик на изменения (`onChange`).
pub type OnChange = Box<dyn Fn(Summary) + Send + Sync>;

pub struct NotificationStore {
    file: PathBuf,
    logger: Arc<dyn Logger>,
    now: Clock,
    on_change: Mutex<Option<OnChange>>,
    seq: AtomicU64,
}

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// `CODE_RE`: `^[A-Z0-9][A-Z0-9_.-]{0,63}$` без учёта регистра.
pub fn is_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `/^[0-9A-Za-z.\-+]{1,32}$/` — версия в `meta` и в событиях версий.
pub fn is_meta_version(text: &str) -> bool {
    (1..=32).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

/// `/^[a-z0-9]+([._-][a-z0-9]+)*$/` и длина ≤ 64 — id игры в `meta` и в событиях версий.
pub fn is_meta_game_id(text: &str) -> bool {
    text.len() <= 64
        && text
            .as_bytes()
            .split(|b| matches!(b, b'.' | b'_' | b'-'))
            .all(|part| {
                !part.is_empty()
                    && part
                        .iter()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
}

/// `/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}([+-]\d{2}:\d{2}|Z)$/`.
fn is_until(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
    let head = b.len() >= 20
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b'T'
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    head && match &b[19..] {
        [b'Z'] => true,
        [sign, h1, h2, b':', m1, m2] => {
            matches!(sign, b'+' | b'-') && [h1, h2, m1, m2].iter().all(|d| d.is_ascii_digit())
        }
        _ => false,
    }
}

/// Белый список `meta` (`add`): только известные поля с проверенными значениями; пусто — `None`.
pub fn sanitize_meta(raw: &Value) -> Option<Value> {
    let Value::Object(raw) = raw else {
        // `raw.meta && typeof raw.meta === "object"`: массив — тоже объект, но без полей.
        return None;
    };
    let mut meta = Map::new();
    let text = |key: &str| raw.get(key).and_then(Value::as_str);
    let mut put = |key: &str, value: Value| {
        meta.insert(key.to_owned(), value);
    };
    if let Some(Value::Bool(has_dump)) = raw.get("hasDump") {
        put("hasDump", json!(has_dump));
    }
    if let Some(code) = raw
        .get("exitCode")
        .filter(|value| value.is_number())
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite() && x.fract() == 0.0)
    {
        put("exitCode", js_number(code));
    }
    if let Some(hint) =
        text("hint").filter(|hint| *hint == "DEP_ALWAYS_ON" || *hint == "DEP_EXEMPTION_FAILED")
    {
        put("hint", json!(hint));
    }
    for key in ["oldVersion", "newVersion"] {
        if let Some(version) = text(key).filter(|version| is_meta_version(version)) {
            put(key, json!(version));
        }
    }
    if let Some(target) =
        text("target").filter(|target| matches!(*target, "launcher" | "game" | "proton"))
    {
        put("target", json!(target));
    }
    if let Some(Value::Bool(notes)) = raw.get("hasReleaseNotes") {
        put("hasReleaseNotes", json!(notes));
    }
    if let Some(game_id) = text("gameId").filter(|id| is_meta_game_id(id)) {
        put("gameId", json!(game_id));
    }
    if let Some(id) = text("launcherId")
        .filter(|id| (1..=8).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()))
    {
        put("launcherId", json!(id));
    }
    if let Some(until) = text("until").filter(|until| is_until(until)) {
        put("until", json!(until));
    }
    if let Some(Value::Bool(forever)) = raw.get("forever") {
        put("forever", json!(forever));
    }
    (!meta.is_empty()).then_some(Value::Object(meta))
}

fn is_valid_item(item: &Value) -> bool {
    let Value::Object(item) = item else {
        return false;
    };
    matches!(item.get("id"), Some(Value::String(_)))
        && matches!(item.get("level"), Some(Value::String(level)) if LEVELS.contains(&level.as_str()))
        && matches!(item.get("ts"), Some(Value::Number(_)))
        // CODE_RE.test(item.code || ""): значение приводится к строке.
        && is_code(
            &item
                .get("code")
                .filter(|code| js::truthy(code))
                .map(js::to_string)
                .unwrap_or_default(),
        )
}

fn is_read(item: &Value) -> bool {
    item.get("read").is_some_and(js::truthy)
}

fn ts_of(item: &Value) -> f64 {
    item.get("ts").and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// `(count || 1) + 1` в JS: число +1, строка, массив и объект — склейка с "1".
fn bump_count(count: Option<&Value>) -> Value {
    match count.filter(|count| js::truthy(count)) {
        None | Some(Value::Bool(true)) => js_number(2.0),
        Some(Value::Number(n)) => js_number(n.as_f64().unwrap_or(f64::NAN) + 1.0),
        Some(other) => Value::String(js::to_string(other) + "1"),
    }
}

impl NotificationStore {
    /// Хранилище файла `file`; журнал — `tracing`, часы — системные.
    pub fn new(file: impl Into<PathBuf>) -> NotificationStore {
        NotificationStore::with(file, Arc::new(TracingLogger), Arc::new(system_now))
    }

    pub fn with(
        file: impl Into<PathBuf>,
        logger: Arc<dyn Logger>,
        now: Clock,
    ) -> NotificationStore {
        NotificationStore {
            file: file.into(),
            logger,
            now,
            on_change: Mutex::default(),
            seq: AtomicU64::new(0),
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `onChange = fn`: вызывается после каждой записи со сводкой.
    pub fn set_on_change(&self, on_change: Option<OnChange>) {
        *self.on_change.lock().unwrap_or_else(|p| p.into_inner()) = on_change;
    }

    /// `readAll().items`: записи с верными `id`, `level`, `ts` и кодом; остальные отброшены.
    pub fn read_all(&self) -> Vec<Value> {
        match read_json_safe(&self.file) {
            Value::Object(mut raw) => match raw.remove("items") {
                Some(Value::Array(items)) => items.into_iter().filter(is_valid_item).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn write(&self, items: Vec<Value>) -> io::Result<Summary> {
        let summary = summarize(&items);
        write_state(
            &self.file,
            &json!({ "version": FILE_VERSION, "items": items }),
            JsonFormat::Atomic,
        )?;
        if let Some(on_change) = &*self.on_change.lock().unwrap_or_else(|p| p.into_inner()) {
            // Подписчик не должен ронять хранилище.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_change(summary)));
        }
        Ok(summary)
    }

    /// `summarize()`: непрочитанные и все записи файла.
    pub fn summarize(&self) -> Summary {
        summarize(&self.read_all())
    }

    /// `list()`: записи от новых к старым и число непрочитанных.
    pub fn list(&self) -> (Vec<Value>, usize) {
        let mut items = self.read_all();
        js::sort::sort_by_comparator(&mut items, |a, b| ts_of(b) - ts_of(a));
        let unread = items.iter().filter(|item| !is_read(item)).count();
        (items, unread)
    }

    /// `add({level, code, detail, meta})`: новая запись или склейка с последней. Неизвестный
    /// уровень или неверный код — `Ok(None)` и строка в журнале. Ошибка — сбой записи файла.
    pub fn add(
        &self,
        level: &str,
        code: &str,
        detail: Option<&str>,
        meta: Option<&Value>,
    ) -> io::Result<Option<Value>> {
        if !LEVELS.contains(&level) || !is_code(code) {
            self.logger.warn(&format!(
                "[notifications] отклонено уведомление: level={level} code={code}"
            ));
            return Ok(None);
        }
        let detail = js::slice_utf16(&redact_text(detail.unwrap_or("")), MAX_DETAIL);
        let meta = meta.and_then(sanitize_meta);

        let mut items = self.read_all();
        let ts = (self.now)();
        let ts_value = js_number(ts as f64);

        if let Some(newest) = items.last_mut()
            && !is_read(newest)
            && newest.get("code").and_then(Value::as_str) == Some(code)
            && newest.get("level").and_then(Value::as_str) == Some(level)
            && newest.get("detail").and_then(Value::as_str) == Some(detail.as_str())
            && ts as f64 - ts_of(newest) <= COALESCE_WINDOW_MS
        {
            let count = bump_count(newest.get("count"));
            newest["count"] = count;
            newest["ts"] = ts_value;
            let updated = newest.clone();
            self.write(items)?;
            return Ok(Some(updated));
        }

        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let mut item = json!({
            "id": format!("{}-{}", js::base36(u128::try_from(ts).unwrap_or(0)), js::base36(u128::from(seq))),
            "ts": ts_value,
            "level": level,
            "code": code,
            "detail": detail,
            "read": false,
            "count": 1,
        });
        if let Some(meta) = meta {
            item["meta"] = meta;
        }
        items.push(item.clone());
        if items.len() > MAX_ITEMS {
            items.drain(..items.len() - MAX_ITEMS);
        }
        self.write(items)?;
        Ok(Some(item))
    }

    /// `removeWhere(predicate)`: убрать записи, для которых `predicate` — `true`; число убранных.
    pub fn remove_where(&self, predicate: impl Fn(&Value) -> bool) -> io::Result<usize> {
        let items = self.read_all();
        let total = items.len();
        let kept: Vec<Value> = items.into_iter().filter(|item| !predicate(item)).collect();
        let removed = total - kept.len();
        if removed > 0 {
            self.write(kept)?;
        }
        Ok(removed)
    }

    /// `markRead(id)`: `true`, если запись была непрочитанной.
    pub fn mark_read(&self, id: &str) -> io::Result<bool> {
        let mut items = self.read_all();
        let Some(item) = items
            .iter_mut()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        else {
            return Ok(false);
        };
        if is_read(item) {
            return Ok(false);
        }
        item["read"] = Value::Bool(true);
        self.write(items)?;
        Ok(true)
    }

    /// `markAllRead()`: `true`, если что-то изменилось.
    pub fn mark_all_read(&self) -> io::Result<bool> {
        let mut items = self.read_all();
        let mut changed = false;
        for item in &mut items {
            if !is_read(item) {
                item["read"] = Value::Bool(true);
                changed = true;
            }
        }
        if changed {
            self.write(items)?;
        }
        Ok(changed)
    }

    /// `clear()`: `true`, если было что удалять.
    pub fn clear(&self) -> io::Result<bool> {
        if self.read_all().is_empty() {
            return Ok(false);
        }
        self.write(Vec::new())?;
        Ok(true)
    }
}

fn summarize(items: &[Value]) -> Summary {
    Summary {
        unread: items.iter().filter(|item| !is_read(item)).count(),
        total: items.len(),
    }
}

/// Записать ошибку запуска, как `reportStartupError` (`message.slice(0, 400)`) и
/// `NotificationStore.add` нового хранилища (`seq` с нуля).
pub fn add_startup_error(file: &Path, detail: &str, now_ms: u64) -> io::Result<()> {
    let store = NotificationStore::with(
        file,
        Arc::new(TracingLogger),
        Arc::new(move || now_ms as i64),
    );
    store
        .add(
            "error",
            "STARTUP_ERROR",
            Some(&js::slice_utf16(detail, STARTUP_DETAIL)),
            None,
        )
        .map(|_| ())
}
