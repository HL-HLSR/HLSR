//! Настройки, которые 0.4.7–0.4.9 держали в localStorage (`Local Storage/leveldb`
//! Chromium), переносятся в `launcher-prefs.json`.

use super::{MigrationLog, MigrationReport, STEP_LOCAL_STORAGE};
use crate::error::CoreError;
use crate::js;
use crate::json_store::write_state;
use crate::json_text::{JsonFormat, js_number, parse_js_json};
use crate::launcher_prefs::{PREFS_FILE, sanitize_prefs};
use crate::paths::Paths;
use crate::tasks::payload_text;
use rusty_leveldb::{DB, LdbIterator, Options};
use serde_json::{Map, Value, json};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::SystemTime;

/// Ключи localStorage страницы `file://`: `_` + origin + `\0`.
const ORIGIN_PREFIX: &[u8] = b"_file://\x00";

const KEY_PREFS: &str = "hlsr-launcher-prefs";
const KEY_KEYBOARD_RUN_MODE: &str = "hlsr.keyboard.runMode";
const KEY_WAS_LAUNCHED: &str = "wasLaunched";
/// `PREFIX` из `pane-size.js` 0.4.9: размер панели у разделителя, `hlsr.pane.<key>`.
const PANE_PREFIX: &str = "hlsr.pane.";

/// Значения ключей localStorage 0.4.7–0.4.9 (`None` — ключа нет).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalStorageValues {
    pub prefs: Option<String>,
    pub keyboard_run_mode: Option<String>,
    pub was_launched: Option<String>,
    /// `hlsr.pane.<key>` 0.4.9: `(key, значение)` в порядке ключей LevelDB.
    pub pane: Vec<(String, String)>,
    /// Обход `hlsr.pane.*` не удался: размеров панелей нет, прочие ключи прочитаны.
    pub pane_error: Option<String>,
}

/// Прочитать ключи 0.4.7–0.4.9 из каталога LevelDB.
pub fn read_local_storage(leveldb_dir: &Path) -> Result<LocalStorageValues, CoreError> {
    with_db(leveldb_dir, |db| {
        let mut values = LocalStorageValues {
            prefs: get_item(db, KEY_PREFS),
            keyboard_run_mode: get_item(db, KEY_KEYBOARD_RUN_MODE),
            was_launched: get_item(db, KEY_WAS_LAUNCHED),
            ..LocalStorageValues::default()
        };
        // Обход открывает все таблицы, а не только те, где лежат ключи выше: его сбой
        // (ошибка или паника) не должен стоить настроек, прочитанных через get.
        let pane = catch_unwind(AssertUnwindSafe(|| {
            pane_items(db).map_err(|e| e.to_string())
        }))
        .unwrap_or_else(|payload| Err(payload_text(payload.as_ref()).to_owned()));
        match pane {
            Ok(pane) => values.pane = pane,
            Err(e) => {
                values.pane_error = Some(format!(
                    "не прочитаны ключи {PANE_PREFIX}* {}: {e}",
                    leveldb_dir.display()
                ));
            }
        }
        Ok(values)
    })
}

/// Прочитать один ключ localStorage страницы `file://`.
pub fn read_local_storage_item(
    leveldb_dir: &Path,
    name: &str,
) -> Result<Option<String>, CoreError> {
    with_db(leveldb_dir, |db| Ok(get_item(db, name)))
}

/// `rusty-leveldb` на испорченной таблице падает с паникой (`assert` подписи
/// `.ldb`), поэтому чтение идёт под `catch_unwind`: паника — обычная ошибка.
fn with_db<T>(
    leveldb_dir: &Path,
    read: impl FnOnce(&mut DB) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    catch_unwind(AssertUnwindSafe(|| {
        let (_copy, mut db) = open_copy(leveldb_dir)?;
        read(&mut db)
    }))
    .unwrap_or_else(|payload| {
        Err(CoreError::LocalStorage(format!(
            "LevelDB упал при чтении {}: {}",
            leveldb_dir.display(),
            payload_text(payload.as_ref())
        )))
    })
}

/// Значение localStorage Chromium: `0x01` + Latin-1 или `0x00` + UTF-16LE.
/// Одиночные суррогаты UTF-16 становятся U+FFFD.
pub fn decode_value(bytes: &[u8]) -> Option<String> {
    let (&kind, rest) = bytes.split_first()?;
    match kind {
        0x01 => Some(rest.iter().map(|&b| char::from(b)).collect()),
        0x00 if rest.len() % 2 == 0 => {
            let units = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&pair| u16::from_le_bytes(pair));
            Some(
                char::decode_utf16(units)
                    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Ключ так же, как его кодирует Chromium: Latin-1, если хватает, иначе UTF-16LE.
fn encode_key(name: &str) -> Vec<u8> {
    let mut key = ORIGIN_PREFIX.to_vec();
    if name.chars().all(|c| u32::from(c) <= 0xFF) {
        key.push(0x01);
        key.extend(name.chars().map(|c| u32::from(c) as u8));
    } else {
        key.push(0x00);
        key.extend(name.encode_utf16().flat_map(u16::to_le_bytes));
    }
    key
}

fn get_item(db: &mut DB, name: &str) -> Option<String> {
    db.get(&encode_key(name))
        .and_then(|value| decode_value(&value))
}

/// Все ключи `hlsr.pane.*` страницы `file://`: ключ в LevelDB — Latin-1 или UTF-16, поэтому
/// обходятся все ключи страницы, а не диапазон одной кодировки.
fn pane_items(db: &mut DB) -> Result<Vec<(String, String)>, rusty_leveldb::Status> {
    let mut iter = db.new_iter()?;
    iter.seek(ORIGIN_PREFIX);
    let mut items = Vec::new();
    while let Some((key, value)) = iter.current() {
        let Some(name) = key.strip_prefix(ORIGIN_PREFIX) else {
            break;
        };
        if let (Some(name), Some(value)) = (decode_value(name), decode_value(&value))
            && let Some(pane) = name.strip_prefix(PANE_PREFIX)
        {
            items.push((pane.to_owned(), value));
        }
        iter.advance();
    }
    Ok(items)
}

/// LevelDB при открытии пишет в каталог (восстанавливает журнал, заводит
/// MANIFEST), поэтому открывается копия. `LOCK` не копируется: его может
/// держать запущенный Chromium, а содержимого у него нет.
fn open_copy(leveldb_dir: &Path) -> Result<(tempfile::TempDir, DB), CoreError> {
    let error = |what: &str, e: &dyn std::fmt::Display| {
        CoreError::LocalStorage(format!("{what} {}: {e}", leveldb_dir.display()))
    };
    let copy = tempfile::Builder::new()
        .prefix("hlsr-leveldb-")
        .tempdir()
        .map_err(|e| error("не создана копия", &e))?;
    let entries = fs::read_dir(leveldb_dir).map_err(|e| error("не прочитан", &e))?;
    for entry in entries {
        let entry = entry.map_err(|e| error("не прочитан", &e))?;
        let name = entry.file_name();
        if name == "LOCK" || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        fs::copy(entry.path(), copy.path().join(&name))
            .map_err(|e| error(&format!("не скопирован {}", name.to_string_lossy()), &e))?;
    }

    let options = Options {
        create_if_missing: false,
        ..Options::default()
    };
    let db = DB::open(copy.path(), options).map_err(|e| error("не открыт", &e))?;
    Ok((copy, db))
}

/// Собрать `launcher-prefs.json` из значений 0.4.7–0.4.9 по правилам их чтения:
/// `load()` из `launcher-prefs.js`, `readRunMode()` из `GameKeyboard.vue`,
/// `if (localStorage.getItem("wasLaunched"))` из `Loading.vue`, `readSize()` из
/// `pane-size.js` (`pane`: `{key: px}`, только если есть хоть один размер).
pub fn build_prefs(values: &LocalStorageValues) -> (Value, Vec<String>) {
    let mut errors: Vec<String> = values.pane_error.iter().cloned().collect();
    let raw = match values.prefs.as_deref().map(parse_js_json) {
        None => Value::Null,
        Some(Ok(value)) => value,
        Some(Err(e)) => {
            errors.push(format!("{KEY_PREFS}: не JSON: {e}"));
            Value::Null
        }
    };
    let mut prefs = sanitize_prefs(raw);
    let run_mode = match values.keyboard_run_mode.as_deref() {
        Some("scripted") => "scripted",
        _ => "scriptless",
    };
    prefs["keyboardRunMode"] = json!(run_mode);
    prefs["wasLaunched"] = json!(
        values
            .was_launched
            .as_deref()
            .is_some_and(|v| !v.is_empty())
    );
    // readSize: Number(значение), конечное и больше нуля; иначе размер не запоминали.
    let pane: Map<String, Value> = values
        .pane
        .iter()
        .filter_map(|(key, text)| {
            let size = js::to_number(text);
            (size.is_finite() && size > 0.0).then(|| (key.clone(), js_number(size)))
        })
        .collect();
    if !pane.is_empty() {
        prefs["pane"] = Value::Object(pane);
    }
    (prefs, errors)
}

/// Перенести localStorage 0.4.7–0.4.9 в `launcher-prefs.json` (один раз).
///
/// Шаг уже сделан → `skipped`. `launcher-prefs.json` уже есть или нет
/// `Local Storage/leveldb` → шаг отмечается без переноса. Ошибка чтения или
/// разбора → умолчания, текст ошибки в отчёт и в журнал. Шаг отмечается
/// всегда, исходный каталог LevelDB не меняется.
pub fn migrate_local_storage(paths: &Paths, now: SystemTime) -> MigrationReport {
    let mut log = MigrationLog::open(paths);
    if log.is_done(STEP_LOCAL_STORAGE) {
        return MigrationReport {
            skipped: true,
            errors: Vec::new(),
        };
    }

    let mut errors = Vec::new();
    let prefs_file = paths.file(PREFS_FILE);
    let leveldb = paths.data_dir.join("Local Storage").join("leveldb");
    if !prefs_file.exists() && leveldb.is_dir() {
        let values = read_local_storage(&leveldb).unwrap_or_else(|e| {
            errors.push(e.to_string());
            LocalStorageValues::default()
        });
        let (prefs, build_errors) = build_prefs(&values);
        errors.extend(build_errors);
        if let Err(e) = write_state(&prefs_file, &prefs, JsonFormat::Store) {
            errors.push(format!("{PREFS_FILE} не записан: {e}"));
        }
    }

    for error in &errors {
        tracing::warn!("[migration] localStorage: {error}");
    }
    if !log.record(STEP_LOCAL_STORAGE, &errors, now) {
        tracing::warn!("[migration] шаг localStorage не отмечен, перенос повторится");
    }
    MigrationReport {
        skipped: false,
        errors,
    }
}
