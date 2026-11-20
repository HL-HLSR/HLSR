//! Хранилище настроек — правила `Store.js` 0.4.7.

use crate::json_store::{ReadState, quarantine, read_state, write_state};
use crate::json_text::JsonFormat;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::SystemTime;

/// Файл JSON с объектом в корне (`settings.json`, `library.json`, …).
///
/// Нет файла → умолчания, запись разрешена. Не JSON-объект → файл уходит
/// в карантин, умолчания, запись разрешена. Не прочитан → умолчания, запись
/// запрещена: содержимое неизвестно, писать поверх нельзя. Умолчания с
/// прочитанным файлом не сливаются. В отличие от 0.4.7, `get` не перечитывает
/// файл: все записи идут через один `Store`.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    name: String,
    defaults: Value,
    data: Value,
    writable: bool,
}

impl Store {
    pub fn open(path: PathBuf, defaults: Value) -> Store {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut store = Store {
            path,
            name,
            data: object_or_empty(&defaults),
            defaults,
            writable: false,
        };
        store.reload();
        store
    }

    /// Перечитать файл.
    pub fn reload(&mut self) {
        match read_state(&self.path) {
            ReadState::Valid(data) => {
                self.data = data;
                self.writable = true;
            }
            ReadState::Missing => {
                self.data = object_or_empty(&self.defaults);
                self.writable = true;
            }
            ReadState::Unparsable => {
                self.quarantine();
                self.data = object_or_empty(&self.defaults);
                self.writable = true;
            }
            ReadState::Unreadable { code } => {
                self.data = object_or_empty(&self.defaults);
                self.writable = false;
                self.warn(&format!(
                    "не прочитан ({code}); запись отключена до успешного чтения, настройки не тронуты"
                ));
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// Записать значение. Память меняется всегда; `false` — файл не записан:
    /// либо запись запрещена (файл есть, но не прочитан), либо не удалась.
    pub fn set(&mut self, key: &str, value: Value) -> bool {
        if let Some(map) = self.data.as_object_mut() {
            map.insert(key.to_owned(), value);
        }
        if !self.writable {
            self.warn(&format!(
                "запись \"{key}\" пропущена: файл есть, но не прочитан"
            ));
            return false;
        }
        match write_state(&self.path, &self.data, JsonFormat::Store) {
            Ok(()) => true,
            Err(error) => {
                self.warn(&format!("запись \"{key}\" не удалась: {error}"));
                false
            }
        }
    }

    pub fn data(&self) -> &Value {
        &self.data
    }

    /// Заменить данные в памяти, не записывая файл.
    pub fn replace_data(&mut self, data: Value) {
        self.data = data;
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    fn quarantine(&self) {
        match quarantine(&self.path, SystemTime::now()) {
            Ok(target) => {
                let moved = target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.warn(&format!("не разобран как JSON, отложен: {moved}"));
            }
            Err(error) => {
                self.warn(&format!(
                    "не разобран как JSON, отложить не удалось: {error}"
                ));
            }
        }
    }

    fn warn(&self, message: &str) {
        tracing::warn!("[store] {}: {message}", self.name);
    }
}

/// Копия умолчаний; не объект → пустой объект (как `copyDefaults`).
fn object_or_empty(defaults: &Value) -> Value {
    match defaults {
        Value::Object(_) => defaults.clone(),
        _ => Value::Object(Map::new()),
    }
}
