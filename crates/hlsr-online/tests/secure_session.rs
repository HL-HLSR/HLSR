//! Чтение `online-session.json` 0.4.7, расшифровка `v10` и перенос входа — на всех ОС. Сверка с
//! настоящим Electron 44.5.1: фикстура `safe-storage-win.json` (`tools/reference/online/safe-storage-win.js`)
//! содержит конверт и шифротексты, снятые настоящим `SecureSession`/`safeStorage`; здесь они
//! расшифровываются известным ключом и переносятся в новое хранилище. Настоящий DPAPI проверяет
//! `safe_storage_win.rs` (`#[cfg(windows)]`), связку ключей Linux — отложенные тесты (`linux-deferred.md`).

mod support;

use hlsr_core::js::buffer_from_base64;
use hlsr_core::migration::MigrationLog;
use hlsr_core::paths::Paths;
use hlsr_online::online_profile::os_crypt::{Backend, decrypt_v10_gcm};
use hlsr_online::online_profile::secure_session::{
    SafeStorage, SecureSession, SessionRead, decode_session, encrypted,
};
use hlsr_online::online_profile::session_migration::{STEP_ONLINE_SESSION, migrate_online_session};
use hlsr_online::online_profile::session_store::{PlainVault, Session, SessionStore};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};
use support::{fixture, str_of};

/// `safeStorage` в тесте: заданный ключ (расшифровка `v10`), бэкенд и доступность.
struct FakeSafeStorage {
    key: Option<[u8; 32]>,
    backend: Option<Backend>,
    available: bool,
}

impl SafeStorage for FakeSafeStorage {
    fn is_encryption_available(&self) -> bool {
        self.available
    }
    fn selected_storage_backend(&self) -> Option<Backend> {
        self.backend
    }
    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        decrypt_v10_gcm(&self.key?, cipher)
    }
}

fn known_key() -> [u8; 32] {
    buffer_from_base64(str_of(&fixture("safe-storage-win.json")["knownKey"]))
        .try_into()
        .expect("известный ключ — 32 байта")
}

fn with_key() -> FakeSafeStorage {
    FakeSafeStorage {
        key: Some(known_key()),
        backend: None,
        available: true,
    }
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_760_000_000_000)
}

#[test]
fn decrypts_real_electron_samples() {
    let fx = fixture("safe-storage-win.json");
    let key = known_key();
    // Каждый образец настоящего safeStorage (включая пустую строку, границы блока, юникод).
    for pair in fx["known"]["samples"].as_array().unwrap() {
        let plain = str_of(&pair[0]);
        let cipher = buffer_from_base64(str_of(&pair[1]));
        let decrypted = decrypt_v10_gcm(&key, &cipher).expect("образец расшифровывается");
        assert_eq!(decrypted, plain.as_bytes(), "образец {plain:?}");
    }
    // Windows: getSelectedStorageBackend — не функция (→ cipher, не plain).
    assert_eq!(fx["backendMethod"], json!(false));
    assert_eq!(fx["available"], json!(true));
    assert_eq!(fx["generated"]["prefix"], json!("DPAPI"));
    assert_eq!(fx["generated"]["keyLenAfterDpapi"], json!(32));
}

#[test]
fn reads_real_electron_session_envelope() {
    let fx = fixture("safe-storage-win.json");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("online-session.json");
    std::fs::write(&file, str_of(&fx["known"]["session"])).unwrap();
    let got = SecureSession::new(&file, &with_key())
        .read()
        .expect("сессия прочитана");
    assert_eq!(got, fx["known"]["sessionData"]);
}

#[test]
fn cipher_needs_available_non_basic_encryption() {
    let fx = fixture("safe-storage-win.json");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("online-session.json");
    std::fs::write(&file, str_of(&fx["known"]["session"])).unwrap();

    // Доступно, бэкенда нет (Windows) или связка ключей — читается.
    assert!(encrypted(&with_key()));
    assert!(SecureSession::new(&file, &with_key()).read().is_some());

    // basic_text → шифрования «нет» → cipher не читается.
    let basic = FakeSafeStorage {
        key: Some(known_key()),
        backend: Some(Backend::BasicText),
        available: true,
    };
    assert!(!encrypted(&basic));
    assert!(SecureSession::new(&file, &basic).read().is_none());
    assert!(matches!(
        decode_session(&file, &basic),
        SessionRead::Failed(_)
    ));

    // Шифрование недоступно.
    let off = FakeSafeStorage {
        key: Some(known_key()),
        backend: None,
        available: false,
    };
    assert!(!encrypted(&off));
    assert!(SecureSession::new(&file, &off).read().is_none());

    // Нет ключа, но шифрование «доступно» → расшифровать нельзя → ошибка переноса.
    let keyless = FakeSafeStorage {
        key: None,
        backend: None,
        available: true,
    };
    assert!(matches!(
        decode_session(&file, &keyless),
        SessionRead::Failed(_)
    ));
}

#[test]
fn reads_plain_envelope_and_rejects_junk() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("online-session.json");
    let storage = FakeSafeStorage {
        key: None,
        backend: Some(Backend::BasicText),
        available: true,
    };

    let plain = json!({ "version": 1, "plain": { "token": "t", "profile": { "x": 1 } } });
    std::fs::write(&file, plain.to_string()).unwrap();
    assert_eq!(
        SecureSession::new(&file, &storage).read(),
        Some(json!({ "token": "t", "profile": { "x": 1 } }))
    );

    // plain не объект → нет сессии.
    std::fs::write(&file, json!({ "version": 1, "plain": 5 }).to_string()).unwrap();
    assert_eq!(SecureSession::new(&file, &storage).read(), None);
    assert!(matches!(
        decode_session(&file, &storage),
        SessionRead::NoSession
    ));

    // Нет файла.
    std::fs::remove_file(&file).unwrap();
    assert!(matches!(
        decode_session(&file, &storage),
        SessionRead::Missing
    ));
}

#[test]
fn oversized_and_tampered_yield_no_session() {
    let fx = fixture("safe-storage-win.json");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("online-session.json");

    // Больше 256 КиБ.
    std::fs::write(&file, "x".repeat(256 * 1024 + 1)).unwrap();
    assert!(SecureSession::new(&file, &with_key()).read().is_none());
    assert!(matches!(
        decode_session(&file, &with_key()),
        SessionRead::NoSession
    ));

    // Испорченный тег шифротекста.
    let envelope: Value = serde_json::from_str(str_of(&fx["known"]["session"])).unwrap();
    let mut bytes = buffer_from_base64(envelope["cipher"].as_str().unwrap());
    *bytes.last_mut().unwrap() ^= 1;
    let broken = json!({ "version": 1, "cipher": base64_standard(&bytes) });
    std::fs::write(&file, broken.to_string()).unwrap();
    assert!(SecureSession::new(&file, &with_key()).read().is_none());
}

fn base64_standard(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn write_legacy(data_dir: &std::path::Path, content: &str) {
    std::fs::create_dir_all(data_dir).unwrap();
    std::fs::write(data_dir.join("online-session.json"), content).unwrap();
}

#[test]
fn migrates_real_session_and_leaves_legacy_untouched() {
    let fx = fixture("safe-storage-win.json");
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let legacy = str_of(&fx["known"]["session"]);
    write_legacy(&paths.data_dir, legacy);
    let legacy_file = paths.data_dir.join("online-session.json");
    let before = std::fs::read(&legacy_file).unwrap();

    let store = SessionStore::new(&paths, Box::new(PlainVault));
    let report = migrate_online_session(&paths, now(), &with_key(), &store);
    assert!(!report.skipped);
    assert!(report.errors.is_empty(), "ошибки: {:?}", report.errors);

    let session = store.read().expect("сессия в новом хранилище");
    assert_eq!(session.token, str_of(&fx["known"]["sessionData"]["token"]));
    // Поля 0.4.9 перенос дописывает при разборе карточки; в фикстуре 0.4.7 их нет.
    assert_eq!(
        support::without_049(&session.profile),
        fx["known"]["sessionData"]["profile"]
    );

    // Файл 0.4.7 не тронут.
    assert_eq!(std::fs::read(&legacy_file).unwrap(), before);

    // Шаг отмечен; повторный запуск пропускается и хранилище не переписывает.
    assert!(MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION));
    std::fs::remove_file(paths.data_dir.join("online-session.json")).unwrap();
    let again = migrate_online_session(&paths, now(), &with_key(), &store);
    assert!(again.skipped);
    assert_eq!(store.read(), Some(session));
}

#[test]
fn migration_skips_invalid_session_without_error() {
    let good =
        str_of(&fixture("safe-storage-win.json")["known"]["sessionData"]["token"]).to_owned();
    // Чужой токен.
    for (name, plain) in [
        (
            "чужой токен",
            json!({ "token": "not-a-token", "profile": { "steamId64": "76561198000000111" } }),
        ),
        (
            "профиль отвергнут",
            json!({ "token": good, "profile": { "x": 1 } }),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        write_legacy(
            &paths.data_dir,
            &json!({ "version": 1, "plain": plain }).to_string(),
        );
        let store = SessionStore::new(&paths, Box::new(PlainVault));
        let report = migrate_online_session(&paths, now(), &with_key(), &store);
        assert!(!report.skipped);
        assert!(
            report.errors.is_empty(),
            "{name}: ошибок быть не должно: {:?}",
            report.errors
        );
        assert_eq!(store.read(), None, "{name}: сессии быть не должно");
        assert!(
            MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION),
            "{name}: шаг отмечен"
        );
    }
}

#[test]
fn migration_without_legacy_file_records_step() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let store = SessionStore::new(&paths, Box::new(PlainVault));
    let report = migrate_online_session(&paths, now(), &with_key(), &store);
    assert!(!report.skipped);
    assert!(report.errors.is_empty());
    assert_eq!(store.read(), None);
    assert!(MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION));
}

/// `migration-slint.json` не записывается (на его месте папка): шаг не отмечен, перенос повторится
/// при следующем запуске — отчёт об этом говорит.
#[test]
fn migration_reports_unrecorded_step() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    std::fs::create_dir_all(paths.data_dir.join("migration-slint.json")).unwrap();
    let store = SessionStore::new(&paths, Box::new(PlainVault));
    let report = migrate_online_session(&paths, now(), &with_key(), &store);
    assert!(!report.skipped);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(
        report.errors[0].contains("migration-slint.json"),
        "{:?}",
        report.errors
    );
    assert!(!MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION));
}

#[test]
fn migration_reports_error_when_cipher_cannot_be_decrypted() {
    let fx = fixture("safe-storage-win.json");
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    write_legacy(&paths.data_dir, str_of(&fx["known"]["session"]));
    let store = SessionStore::new(&paths, Box::new(PlainVault));
    // Ключа нет (нет/испорчен Local State), шифрование «доступно».
    let keyless = FakeSafeStorage {
        key: None,
        backend: None,
        available: true,
    };
    let report = migrate_online_session(&paths, now(), &keyless, &store);
    assert!(!report.skipped);
    assert_eq!(report.errors.len(), 1, "одна ошибка переноса");
    assert_eq!(store.read(), None);
    // Ошибка записана и в journal migration-slint.json.
    let log = std::fs::read_to_string(paths.data_dir.join("migration-slint.json")).unwrap();
    assert!(log.contains("не расшифровать"), "журнал: {log}");
}

#[test]
fn new_store_write_read_clear() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let store = SessionStore::new(&paths, Box::new(PlainVault));
    let session = Session {
        token: "hlsr_0123456789abcdef_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE".to_owned(),
        profile: json!({ "steamId64": "76561198000000111" }),
    };
    assert_eq!(store.read(), None);
    store.write(&session).unwrap();
    assert_eq!(store.read(), Some(session));
    store.clear();
    assert_eq!(store.read(), None);
    assert!(!store.file().exists());
}
