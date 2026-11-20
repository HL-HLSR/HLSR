//! Новое хранилище сессии 0.5 (спецификация, решение 4): отдельный файл `online-session.v2.json`
//! `{version:2, kind:"dpapi"|"keyring"|"plain", …}`, атомарная запись с правами 0600 через
//! `hlsr-core`. Шифрование — за трейтом [`SessionVault`]: Windows — DPAPI ([`DpapiVault`]);
//! Linux — секрет в связке ключей (Secret Service / KWallet, [`super::os_crypt::linux`]); без службы
//! (окружение без связки или служба не ответила) — `plain` 0600 ([`PlainVault`]). Файл 0.4.7 `online-session.json` этим не трогается. API для
//! задачи 9: [`SessionStore::read`] / [`SessionStore::write`] / [`SessionStore::clear`].

use super::secure_session::MAX_SESSION_BYTES;
use hlsr_core::json_formats::{TmpOptions, WriteError, write_compact_tmp};
use hlsr_core::paths::Paths;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Имя файла нового хранилища.
pub const SESSION_FILE: &str = "online-session.v2.json";
/// Версия конверта нового хранилища.
pub const SESSION_VERSION: u64 = 2;

/// Сессия входа: токен и нормализованный профиль (`{token, profile}` 0.4.7).
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub token: String,
    pub profile: Value,
}

impl Session {
    /// Открытый текст хранилища: `{"token":…,"profile":…}` (порядок ключей как у 0.4.7).
    fn plaintext(&self) -> Vec<u8> {
        serde_json::to_vec(&json!({ "token": self.token, "profile": self.profile }))
            .expect("сессия сериализуется")
    }
}

/// Сбой записи нового хранилища.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Служба ОС (DPAPI, связка ключей) не зашифровала секрет.
    #[error("хранилище сессии: {0}")]
    Vault(String),
    /// Файл не записан.
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// Шифрование нового хранилища: конверт файла и побочные записи в связку ключей.
pub trait SessionVault {
    /// `kind` конверта (`"dpapi"` / `"keyring"` / `"plain"`).
    fn kind(&self) -> &'static str;
    /// Собрать конверт `{version:2, kind, …}` из открытого текста (и, для связки, положить секрет).
    fn protect(&self, plaintext: &[u8]) -> Result<Value, SessionError>;
    /// Достать открытый текст из конверта; `None` — не тот `kind`, нет секрета или не расшифровать.
    fn unprotect(&self, envelope: &Value) -> Option<Vec<u8>>;
    /// Убрать побочный секрет из связки при очистке (для `plain`/`dpapi` — ничего).
    fn forget(&self, envelope: &Value);
}

/// Хранилище сессии: файл и способ шифрования.
pub struct SessionStore {
    file: PathBuf,
    vault: Box<dyn SessionVault + Send + Sync>,
}

impl SessionStore {
    /// Хранилище в каталоге данных (`online-session.v2.json`).
    pub fn new(paths: &Paths, vault: Box<dyn SessionVault + Send + Sync>) -> SessionStore {
        SessionStore {
            file: paths.file(SESSION_FILE),
            vault,
        }
    }

    /// Хранилище по заданному пути (тесты и особые случаи).
    pub fn at(
        file: impl Into<PathBuf>,
        vault: Box<dyn SessionVault + Send + Sync>,
    ) -> SessionStore {
        SessionStore {
            file: file.into(),
            vault,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// Прочитать сессию; `None` — файла нет, он больше предела, не той версии или не расшифровать.
    pub fn read(&self) -> Option<Session> {
        let meta = fs::metadata(&self.file).ok()?;
        if meta.len() > MAX_SESSION_BYTES {
            return None;
        }
        let envelope: Value = serde_json::from_str(&fs::read_to_string(&self.file).ok()?).ok()?;
        if envelope.get("version").and_then(Value::as_u64) != Some(SESSION_VERSION) {
            return None;
        }
        let bytes = self.vault.unprotect(&envelope)?;
        let data: Value = serde_json::from_slice(&bytes).ok()?;
        let token = data.get("token")?.as_str()?.to_owned();
        let profile = data.get("profile")?.clone();
        Some(Session { token, profile })
    }

    /// Записать сессию (атомарно, 0600).
    pub fn write(&self, session: &Session) -> Result<(), SessionError> {
        let envelope = self.vault.protect(&session.plaintext())?;
        write_compact_tmp(
            &self.file,
            &envelope,
            TmpOptions {
                private: true,
                clear_stale: true,
            },
        )?;
        Ok(())
    }

    /// Очистить сессию: убрать секрет из связки и удалить файл.
    pub fn clear(&self) {
        if let Ok(text) = fs::read_to_string(&self.file)
            && let Ok(envelope) = serde_json::from_str::<Value>(&text)
        {
            self.vault.forget(&envelope);
        }
        let _ = fs::remove_file(&self.file);
    }
}

/// `plain`: открытый текст в файле (без службы ключей; права 0600). Как хранят вход Steam и gh —
/// токен привязан к устройству и отзывается на сервере.
pub struct PlainVault;

impl SessionVault for PlainVault {
    fn kind(&self) -> &'static str {
        "plain"
    }

    fn protect(&self, plaintext: &[u8]) -> Result<Value, SessionError> {
        let data: Value =
            serde_json::from_slice(plaintext).map_err(|e| SessionError::Vault(e.to_string()))?;
        Ok(json!({ "version": SESSION_VERSION, "kind": "plain", "data": data }))
    }

    fn unprotect(&self, envelope: &Value) -> Option<Vec<u8>> {
        if envelope.get("kind").and_then(Value::as_str) != Some("plain") {
            return None;
        }
        serde_json::to_vec(envelope.get("data")?).ok()
    }

    fn forget(&self, _envelope: &Value) {}
}

/// `dpapi` (Windows): открытый текст под `CryptProtectData` текущего пользователя, base64 в поле
/// `blob`.
#[cfg(windows)]
pub struct DpapiVault;

#[cfg(windows)]
impl SessionVault for DpapiVault {
    fn kind(&self) -> &'static str {
        "dpapi"
    }

    fn protect(&self, plaintext: &[u8]) -> Result<Value, SessionError> {
        use base64::Engine;
        let blob = super::os_crypt::windows::dpapi_protect(plaintext)
            .ok_or_else(|| SessionError::Vault("CryptProtectData".to_owned()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&blob);
        Ok(json!({ "version": SESSION_VERSION, "kind": "dpapi", "blob": encoded }))
    }

    fn unprotect(&self, envelope: &Value) -> Option<Vec<u8>> {
        if envelope.get("kind").and_then(Value::as_str) != Some("dpapi") {
            return None;
        }
        let blob = hlsr_core::js::buffer_from_base64(envelope.get("blob")?.as_str()?);
        super::os_crypt::windows::dpapi_unprotect(&blob)
    }

    fn forget(&self, _envelope: &Value) {}
}

/// Хранилище сессии этой ОС (для задачи 14): Windows — DPAPI; Linux — связка ключей по бэкенду
/// окружения, без неё (`basic_text`) — `plain` 0600; служба выбрана, но не ответила при записи
/// (не запущена, KWallet выключен) — тоже `plain` 0600 (`LinuxKeyringVault`, как 0.4.7 с
/// `isEncryptionAvailable()` = false); прочее — `plain`.
#[cfg(windows)]
pub fn detect_vault() -> Box<dyn SessionVault + Send + Sync> {
    Box::new(DpapiVault)
}

#[cfg(target_os = "linux")]
pub fn detect_vault() -> Box<dyn SessionVault + Send + Sync> {
    use super::os_crypt::{Backend, select_backend};
    match select_backend(|name| std::env::var(name).ok()) {
        Backend::BasicText => Box::new(PlainVault),
        backend => Box::new(super::os_crypt::linux::LinuxKeyringVault::new(backend)),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn detect_vault() -> Box<dyn SessionVault + Send + Sync> {
    Box::new(PlainVault)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Поддельный сейф для кросс-платформенной проверки файловой логики: XOR с ключом 0x5a.
    struct XorVault;
    impl SessionVault for XorVault {
        fn kind(&self) -> &'static str {
            "test-xor"
        }
        fn protect(&self, plaintext: &[u8]) -> Result<Value, SessionError> {
            use base64::Engine;
            let xored: Vec<u8> = plaintext.iter().map(|b| b ^ 0x5a).collect();
            let encoded = base64::engine::general_purpose::STANDARD.encode(&xored);
            Ok(json!({ "version": SESSION_VERSION, "kind": "test-xor", "blob": encoded }))
        }
        fn unprotect(&self, envelope: &Value) -> Option<Vec<u8>> {
            if envelope.get("kind").and_then(Value::as_str) != Some("test-xor") {
                return None;
            }
            let bytes = hlsr_core::js::buffer_from_base64(envelope.get("blob")?.as_str()?);
            Some(bytes.iter().map(|b| b ^ 0x5a).collect())
        }
        fn forget(&self, _envelope: &Value) {}
    }

    fn sample() -> Session {
        Session {
            token: "hlsr_0123456789abcdef_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE".to_owned(),
            profile: json!({ "steamId64": "76561198000000111", "personaName": "тест" }),
        }
    }

    #[test]
    fn write_read_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SESSION_FILE);
        let store = SessionStore::at(file.clone(), Box::new(XorVault));
        assert_eq!(store.read(), None);
        store.write(&sample()).unwrap();
        assert!(file.exists());
        assert_eq!(store.read(), Some(sample()));
        store.clear();
        assert!(!file.exists());
        assert_eq!(store.read(), None);
    }

    #[test]
    fn plain_vault_keeps_payload_readable() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::at(dir.path().join(SESSION_FILE), Box::new(PlainVault));
        store.write(&sample()).unwrap();
        let text = std::fs::read_to_string(store.file()).unwrap();
        let envelope: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(envelope["version"], json!(2));
        assert_eq!(envelope["kind"], json!("plain"));
        assert_eq!(envelope["data"]["token"], sample().token.as_str());
        assert_eq!(store.read(), Some(sample()));
    }

    #[test]
    fn wrong_version_or_kind_yields_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SESSION_FILE);
        let store = SessionStore::at(file.clone(), Box::new(PlainVault));
        std::fs::write(
            &file,
            json!({ "version": 1, "kind": "plain", "data": {} }).to_string(),
        )
        .unwrap();
        assert_eq!(store.read(), None);
        std::fs::write(
            &file,
            json!({ "version": 2, "kind": "dpapi", "blob": "x" }).to_string(),
        )
        .unwrap();
        assert_eq!(store.read(), None);
    }

    #[test]
    fn oversized_file_yields_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SESSION_FILE);
        let store = SessionStore::at(file.clone(), Box::new(PlainVault));
        let big = "x".repeat((MAX_SESSION_BYTES as usize) + 1);
        std::fs::write(&file, big).unwrap();
        assert_eq!(store.read(), None);
    }

    #[cfg(unix)]
    #[test]
    fn plain_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::at(dir.path().join(SESSION_FILE), Box::new(PlainVault));
        store.write(&sample()).unwrap();
        let mode = std::fs::metadata(store.file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
