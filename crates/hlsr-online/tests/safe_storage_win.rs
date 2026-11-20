//! Настоящий DPAPI Windows (`#[cfg(windows)]`, запускается на этой машине): защитить известный
//! тестовый ключ, собрать `Local State` и `online-session.json` из фикстуры во временном каталоге,
//! перенести вход и проверить новое хранилище (DPAPI). Данные только тестовые — ключ из фикстуры и
//! сессия; чужие секреты не трогаются. Чистую расшифровку `v10` на всех ОС проверяет
//! `secure_session.rs`.
#![cfg(windows)]

mod support;

use hlsr_core::js::buffer_from_base64;
use hlsr_core::migration::MigrationLog;
use hlsr_core::paths::Paths;
use hlsr_online::online_profile::os_crypt::windows::{WindowsSafeStorage, dpapi_protect};
use hlsr_online::online_profile::session_migration::{STEP_ONLINE_SESSION, migrate_online_session};
use hlsr_online::online_profile::session_store::{DpapiVault, Session, SessionStore};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, SystemTime};
use support::{fixture, str_of};

fn known_key() -> [u8; 32] {
    buffer_from_base64(str_of(&fixture("safe-storage-win.json")["knownKey"]))
        .try_into()
        .unwrap()
}

fn base64_standard(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `Local State` с ключом под настоящим DPAPI текущего пользователя (как пишет Electron на Windows).
fn seed_local_state(data_dir: &Path, key: &[u8; 32]) {
    let protected = dpapi_protect(key).expect("DPAPI защитил тестовый ключ");
    let mut blob = b"DPAPI".to_vec();
    blob.extend_from_slice(&protected);
    let local_state = json!({ "os_crypt": { "encrypted_key": base64_standard(&blob) } });
    std::fs::create_dir_all(data_dir).unwrap();
    std::fs::write(data_dir.join("Local State"), local_state.to_string()).unwrap();
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_760_000_000_000)
}

#[test]
fn reads_key_through_real_dpapi() {
    let dir = tempfile::tempdir().unwrap();
    seed_local_state(dir.path(), &known_key());
    let storage = WindowsSafeStorage::from_data_dir(dir.path());
    assert_eq!(storage.key(), Some(known_key()));
}

#[test]
fn migrates_real_session_through_dpapi_chain() {
    let fx = fixture("safe-storage-win.json");
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    seed_local_state(&paths.data_dir, &known_key());
    std::fs::write(
        paths.data_dir.join("online-session.json"),
        str_of(&fx["known"]["session"]),
    )
    .unwrap();
    let legacy_before = std::fs::read(paths.data_dir.join("online-session.json")).unwrap();

    let storage = WindowsSafeStorage::from_data_dir(&paths.data_dir);
    let store = SessionStore::new(&paths, Box::new(DpapiVault));
    let report = migrate_online_session(&paths, now(), &storage, &store);
    assert!(!report.skipped);
    assert!(report.errors.is_empty(), "ошибки: {:?}", report.errors);

    let session = store.read().expect("сессия перенесена");
    assert_eq!(session.token, str_of(&fx["known"]["sessionData"]["token"]));
    // Поля 0.4.9 перенос дописывает при разборе карточки; в фикстуре 0.4.7 их нет.
    assert_eq!(
        support::without_049(&session.profile),
        fx["known"]["sessionData"]["profile"]
    );

    // Новое хранилище — DPAPI, не открытый текст.
    let envelope: Value =
        serde_json::from_str(&std::fs::read_to_string(store.file()).unwrap()).unwrap();
    assert_eq!(envelope["kind"], json!("dpapi"));
    assert_eq!(envelope["version"], json!(2));
    assert!(envelope.get("data").is_none());
    assert!(
        !std::fs::read_to_string(store.file())
            .unwrap()
            .contains(&session.token)
    );

    // Файл 0.4.7 не тронут; шаг отмечен; повтор пропускается.
    assert_eq!(
        std::fs::read(paths.data_dir.join("online-session.json")).unwrap(),
        legacy_before
    );
    assert!(MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION));
    let again = migrate_online_session(&paths, now(), &storage, &store);
    assert!(again.skipped);
}

#[test]
fn missing_or_corrupt_local_state_reports_error() {
    let fx = fixture("safe-storage-win.json");
    // Нет Local State.
    {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(
            paths.data_dir.join("online-session.json"),
            str_of(&fx["known"]["session"]),
        )
        .unwrap();
        let storage = WindowsSafeStorage::from_data_dir(&paths.data_dir);
        assert_eq!(storage.key(), None);
        let store = SessionStore::new(&paths, Box::new(DpapiVault));
        let report = migrate_online_session(&paths, now(), &storage, &store);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(store.read(), None);
    }
    // Испорченный Local State.
    {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(paths.data_dir.join("Local State"),
            json!({ "os_crypt": { "encrypted_key": base64_standard(b"DPAPInot-a-real-dpapi-blob") } }).to_string()).unwrap();
        std::fs::write(
            paths.data_dir.join("online-session.json"),
            str_of(&fx["known"]["session"]),
        )
        .unwrap();
        let storage = WindowsSafeStorage::from_data_dir(&paths.data_dir);
        assert_eq!(storage.key(), None);
        let store = SessionStore::new(&paths, Box::new(DpapiVault));
        let report = migrate_online_session(&paths, now(), &storage, &store);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(store.read(), None);
    }
}

#[test]
fn new_store_round_trips_through_dpapi() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let store = SessionStore::new(&paths, Box::new(DpapiVault));
    let session = Session {
        token: "hlsr_fedcba9876543210_ZyXwVuTsRqPoNmLkJiHgFeDcBa9876543210-_ZyXwV".to_owned(),
        profile: json!({ "steamId64": "76561198000000222", "personaName": "DPAPI" }),
    };
    assert_eq!(store.read(), None);
    store.write(&session).unwrap();
    assert_eq!(store.read(), Some(session));
    store.clear();
    assert_eq!(store.read(), None);
    assert!(!store.file().exists());
}
