//! Чтение `online-session.json` 0.4.7 (`src/main/online-profile/SecureSession.js`). Конверт
//! `{version:1, cipher:"<base64>"}` или `{version:1, plain:{…}}`; `cipher` расшифровывается через
//! [`SafeStorage`] — то же, что `safeStorage` Electron. 0.5 этот файл **не пишет и не удаляет**
//! (бета: 0.4.7 и 0.5 делят вход, карта §6.2): переносится только чтение, `write`/`clear` 0.4.7 не
//! нужны. Новое хранилище сессии — [`super::session_store`].

use super::os_crypt::Backend;
use hlsr_core::js::buffer_from_base64;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Предел чтения `online-session.json` (как `256 * 1024` в 0.4.7).
pub const MAX_SESSION_BYTES: u64 = 256 * 1024;

/// `safeStorage` Electron: доступно ли шифрование, выбранный бэкенд (на Windows/macOS метода нет —
/// `None`) и расшифровка. Над ним [`SecureSession`] повторяет `SecureSession.read`.
pub trait SafeStorage {
    /// `safeStorage.isEncryptionAvailable()`.
    fn is_encryption_available(&self) -> bool;
    /// `safeStorage.getSelectedStorageBackend()`; `None` — метода нет (Windows, macOS).
    fn selected_storage_backend(&self) -> Option<Backend>;
    /// `safeStorage.decryptString(Buffer)`: открытый текст или `None` при любой ошибке.
    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>>;
}

/// `SecureSession.encrypted()`: шифрование доступно и бэкенд — не `basic_text` (или метода нет).
pub fn encrypted(storage: &dyn SafeStorage) -> bool {
    storage.is_encryption_available()
        && !matches!(storage.selected_storage_backend(), Some(Backend::BasicText))
}

/// Итог чтения файла 0.4.7. [`SecureSession::read`] сводит всё, кроме [`Session`](Self::Session), к
/// `None`; перенос ([`super::session_migration`]) отличает [`Failed`](Self::Failed) (записать ошибку
/// шага) от [`Missing`](Self::Missing)/[`NoSession`](Self::NoSession) (переносить нечего, не ошибка).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionRead {
    /// Файла нет (или `statSync` не удался) — как `null` 0.4.7.
    Missing,
    /// Открытый текст конверта (ожидается `{token, profile}`).
    Session(Value),
    /// Файл есть, но сессии нет (`plain` не объект, файл больше предела) — не ошибка.
    NoSession,
    /// Перенос не удался: не прочитать, не JSON или не расшифровать.
    Failed(String),
}

/// Разобрать файл сессии 0.4.7: та же логика, что у `SecureSession.read`, но с причиной отказа для
/// переноса. Открытый текст конверта не проверяется (токен и `normalize` — на вызывающем).
pub fn decode_session(file: &Path, storage: &dyn SafeStorage) -> SessionRead {
    let Ok(meta) = fs::metadata(file) else {
        return SessionRead::Missing;
    };
    if meta.len() > MAX_SESSION_BYTES {
        return SessionRead::NoSession;
    }
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            return SessionRead::Failed(format!("online-session.json не прочитан: {error}"));
        }
    };
    let envelope: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return SessionRead::Failed(format!("online-session.json не JSON: {error}")),
    };
    if let Some(cipher) = envelope.get("cipher").and_then(Value::as_str) {
        if !encrypted(storage) {
            return SessionRead::Failed(
                "шифрование недоступно: online-session.json с cipher не прочитать".to_owned(),
            );
        }
        let Some(plain) = storage.decrypt(&buffer_from_base64(cipher)) else {
            return SessionRead::Failed(
                "online-session.json не расшифровать (нет или испорчен Local State)".to_owned(),
            );
        };
        return match String::from_utf8(plain)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            Some(value) => SessionRead::Session(value),
            None => SessionRead::Failed("расшифрованный online-session.json не JSON".to_owned()),
        };
    }
    // `envelope.plain && typeof envelope.plain === "object"`: объект или массив (не `null`).
    match envelope.get("plain") {
        Some(plain) if plain.is_object() || plain.is_array() => SessionRead::Session(plain.clone()),
        _ => SessionRead::NoSession,
    }
}

/// Чтение файла сессии 0.4.7 поверх [`SafeStorage`].
pub struct SecureSession<'a> {
    file: PathBuf,
    storage: &'a dyn SafeStorage,
}

impl<'a> SecureSession<'a> {
    pub fn new(file: impl Into<PathBuf>, storage: &'a dyn SafeStorage) -> SecureSession<'a> {
        SecureSession {
            file: file.into(),
            storage,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `SecureSession.read`: `null` (→ `None`) при любой ошибке и на `cipher` без доступного
    /// шифрования. Возвращает открытый текст конверта (ожидается `{token, profile}`); проверку
    /// токена и `normalize` делает вызывающий, как `OnlineProfileService`.
    pub fn read(&self) -> Option<Value> {
        match decode_session(&self.file, self.storage) {
            SessionRead::Session(value) => Some(value),
            _ => None,
        }
    }
}
