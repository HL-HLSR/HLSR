//! Связка ключей Linux (`#[cfg(target_os = "linux")]`): здесь только проверка типов через
//! `scripts/slint/check-linux.sh`; запуск — на Linux (`docs/superpowers/notes/2026-10-16-linux-deferred.md`).
//! Поддельные Secret Service и KWallet (`support/fake_keyring.rs`, соединение точка-точка, без шины)
//! с известным паролем: чтение входа 0.4.7 (`v11`/`v10`, запасные ключи), запись сессии 0.5 не видна
//! поиску Chromium, новое хранилище (`keyring`, без службы — `plain` 0600).
#![cfg(target_os = "linux")]

#[path = "support/fake_keyring.rs"]
mod fake_keyring;

use fake_keyring::{FakeState, Shared, p2p};
use hlsr_online::online_profile::os_crypt::linux::{
    self, CHROMIUM_SCHEMA, Connect, KWALLET_FOLDER, KWALLET_KEY, LinuxKeyringVault,
    LinuxSafeStorage, SESSION_SCHEMA,
};
use hlsr_online::online_profile::os_crypt::{Backend, CBC_IV, SALT};
use hlsr_online::online_profile::secure_session::SafeStorage;
use hlsr_online::online_profile::session_crypto::{cbc_encrypt, pbkdf2_sha1};
use hlsr_online::online_profile::session_store::{
    SESSION_FILE, Session, SessionStore, SessionVault,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Пароль связки (как у Chromium: base64 от 16 случайных байт) и шифротекст `v11` этим паролем.
const PASSWORD: &str = "aGxzci1maXh0dXJlLXB3MDE=";

fn v11(password: &[u8], plain: &[u8]) -> Vec<u8> {
    let key = pbkdf2_sha1(password, SALT, 1, 16);
    let mut out = b"v11".to_vec();
    out.extend(cbc_encrypt(&key, &CBC_IV, plain));
    out
}

fn libsecret_seed() -> Value {
    json!({
        "items": [{
            "label": "Chromium Safe Storage",
            "attributes": { "application": "hlsr", "xdg:schema": CHROMIUM_SCHEMA },
            "secret": PASSWORD,
        }],
    })
}

fn kwallet_seed() -> Value {
    json!({ "kwallet": { "folders": { KWALLET_FOLDER: { KWALLET_KEY: PASSWORD } } } })
}

async fn connect(seed: Value) -> zbus::Connection {
    let state = Arc::new(Mutex::new(fake_keyring::state_from_seed(&seed)));
    p2p(state).await
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_libsecret_password_and_decrypts_v11() {
    let conn = connect(libsecret_seed()).await;
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::GnomeLibsecret).await;
    assert!(storage.is_encryption_available());
    assert_eq!(
        storage.selected_storage_backend(),
        Some(Backend::GnomeLibsecret)
    );
    let cipher = v11(PASSWORD.as_bytes(), b"{\"token\":\"ok\"}");
    assert_eq!(storage.decrypt(&cipher).unwrap(), b"{\"token\":\"ok\"}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_kwallet_password_and_decrypts_v11() {
    let conn = connect(kwallet_seed()).await;
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::Kwallet6).await;
    assert!(storage.is_encryption_available());
    let cipher = v11(PASSWORD.as_bytes(), b"payload");
    assert_eq!(storage.decrypt(&cipher).unwrap(), b"payload");
}

#[tokio::test(flavor = "multi_thread")]
async fn basic_text_has_no_encryption() {
    let conn = connect(json!({})).await;
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::BasicText).await;
    assert!(!storage.is_encryption_available());
    assert_eq!(storage.decrypt(&v11(PASSWORD.as_bytes(), b"x")), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_keyring_item_has_no_password() {
    let conn = connect(json!({})).await;
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::GnomeLibsecret).await;
    assert!(!storage.is_encryption_available());
}

#[tokio::test(flavor = "multi_thread")]
async fn new_store_libsecret_round_trip() {
    let conn = connect(json!({})).await;
    assert_eq!(linux::load_libsecret(&conn).await, None);
    linux::store_libsecret(&conn, b"{\"token\":\"t\"}")
        .await
        .expect("положен");
    assert_eq!(
        linux::load_libsecret(&conn).await.unwrap(),
        b"{\"token\":\"t\"}"
    );
    linux::forget_libsecret(&conn).await.expect("удалён");
    assert_eq!(linux::load_libsecret(&conn).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn new_store_kwallet_round_trip() {
    let conn = connect(json!({})).await;
    assert_eq!(linux::load_kwallet(&conn, Backend::Kwallet6).await, None);
    linux::store_kwallet(&conn, Backend::Kwallet6, b"payload")
        .await
        .expect("положен");
    assert_eq!(
        linux::load_kwallet(&conn, Backend::Kwallet6).await.unwrap(),
        b"payload"
    );
    linux::forget_kwallet(&conn, Backend::Kwallet6)
        .await
        .expect("удалён");
    assert_eq!(linux::load_kwallet(&conn, Backend::Kwallet6).await, None);
}

fn chromium_item(attributes: Value) -> Value {
    json!({ "label": "Chromium Safe Storage", "attributes": attributes, "secret": PASSWORD })
}

/// Запись сессии 0.5 не попадает в поиск Chromium 0.4.7 (только `application`, `DONT_MATCH_NAME`):
/// иначе на бете 0.4.7 мог бы взять её секрет за свой пароль `os_crypt`.
#[tokio::test(flavor = "multi_thread")]
async fn session_item_is_invisible_to_chromium_search() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let conn = p2p(state.clone()).await;
    // Запись 0.5 — первой: поиск подделки отдаёт записи в порядке создания.
    linux::store_libsecret(&conn, b"{\"token\":\"t\"}")
        .await
        .expect("положен");
    state.lock().unwrap().add_item(
        "Chromium Safe Storage",
        BTreeMap::from([
            ("application".to_owned(), "hlsr".to_owned()),
            ("xdg:schema".to_owned(), CHROMIUM_SCHEMA.to_owned()),
        ]),
        PASSWORD.as_bytes(),
    );
    let chromium_query = HashMap::from([("application".to_owned(), "hlsr".to_owned())]);
    assert_eq!(state.lock().unwrap().matches(&chromium_query).len(), 1);
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::GnomeLibsecret).await;
    let cipher = v11(PASSWORD.as_bytes(), b"payload");
    assert_eq!(storage.decrypt(&cipher).unwrap(), b"payload");
    // Запись 0.5 читается своей схемой.
    assert_eq!(
        linux::load_libsecret(&conn).await.unwrap(),
        b"{\"token\":\"t\"}"
    );
}

/// Чужая запись с тем же `application` раньше записи Chromium: пароль отбирается по `xdg:schema`.
#[tokio::test(flavor = "multi_thread")]
async fn chromium_password_is_picked_by_schema() {
    let conn = connect(json!({
        "items": [
            { "label": "other", "secret": "{\"token\":\"t\"}",
              "attributes": { "application": "hlsr", "xdg:schema": "hlsr_launcher_session_v2" } },
            chromium_item(json!({ "application": "hlsr", "xdg:schema": CHROMIUM_SCHEMA })),
        ],
    }))
    .await;
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::GnomeLibsecret).await;
    let cipher = v11(PASSWORD.as_bytes(), b"payload");
    assert_eq!(storage.decrypt(&cipher).unwrap(), b"payload");
}

/// Запись Chromium без `xdg:schema` (корпус `safe-storage.js`): находится, как у Chromium, по одному
/// `application`.
#[tokio::test(flavor = "multi_thread")]
async fn chromium_password_without_schema_is_found() {
    let conn = connect(json!({ "items": [chromium_item(json!({ "application": "hlsr" }))] })).await;
    let storage = LinuxSafeStorage::with_backend(&conn, Backend::GnomeLibsecret).await;
    assert!(storage.is_encryption_available());
    let cipher = v11(PASSWORD.as_bytes(), b"payload");
    assert_eq!(storage.decrypt(&cipher).unwrap(), b"payload");
}

fn sample_session() -> Session {
    Session {
        token: "hlsr_0123456789abcdef_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE".to_owned(),
        profile: json!({ "steamId64": "76561198000000111", "personaName": "Fake runner" }),
    }
}

/// Соединение хранилища с поддельными службами (новая пара точка-точка на каждый вызов, общее
/// состояние) — вместо шины пользователя.
fn fake_connect(state: Shared) -> Connect {
    Arc::new(move || {
        let state = state.clone();
        Box::pin(async move { Some(p2p(state).await) })
    })
}

fn envelope(store: &SessionStore) -> Value {
    serde_json::from_str(&std::fs::read_to_string(store.file()).unwrap()).unwrap()
}

#[test]
fn keyring_vault_round_trip_through_libsecret() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let vault =
        LinuxKeyringVault::with_connect(Backend::GnomeLibsecret, fake_connect(state.clone()));
    assert_eq!(vault.kind(), "keyring");
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::at(dir.path().join(SESSION_FILE), Box::new(vault));
    store.write(&sample_session()).unwrap();
    let written = envelope(&store);
    assert_eq!(written["kind"], json!("keyring"));
    assert_eq!(written["backend"], json!("gnome_libsecret"));
    assert!(!written.to_string().contains("hlsr_0123456789abcdef"));
    {
        let state = state.lock().unwrap();
        assert_eq!(state.items.len(), 1);
        assert!(!state.items[0].attributes.contains_key("application"));
        assert_eq!(state.items[0].attributes["xdg:schema"], SESSION_SCHEMA);
    }
    assert_eq!(store.read(), Some(sample_session()));
    store.clear();
    assert!(state.lock().unwrap().items.is_empty());
    assert!(!store.file().exists());
    assert_eq!(store.read(), None);
}

#[test]
fn keyring_vault_round_trip_through_kwallet() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let vault = LinuxKeyringVault::with_connect(Backend::Kwallet6, fake_connect(state.clone()));
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::at(dir.path().join(SESSION_FILE), Box::new(vault));
    store.write(&sample_session()).unwrap();
    assert_eq!(envelope(&store)["kind"], json!("keyring"));
    assert_eq!(store.read(), Some(sample_session()));
    store.clear();
    assert_eq!(store.read(), None);
}

fn assert_plain_fallback(vault: LinuxKeyringVault) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::at(dir.path().join(SESSION_FILE), Box::new(vault));
    store.write(&sample_session()).expect("без службы — plain");
    let written = envelope(&store);
    assert_eq!(written["kind"], json!("plain"));
    assert_eq!(written["data"]["token"], json!(sample_session().token));
    let mode = std::fs::metadata(store.file())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(store.read(), Some(sample_session()));
    store.clear();
    assert_eq!(store.read(), None);
}

/// Служба выбрана, но шины/службы нет (XFCE без gnome-keyring): как 0.4.7 — `plain` 0600.
#[test]
fn keyring_vault_without_service_falls_back_to_plain() {
    let none: Connect = Arc::new(|| Box::pin(async { None }));
    assert_plain_fallback(LinuxKeyringVault::with_connect(
        Backend::GnomeLibsecret,
        none,
    ));
}

/// KWallet выключен (`open` → -1): `plain` 0600.
#[test]
fn keyring_vault_with_disabled_kwallet_falls_back_to_plain() {
    let state = Arc::new(Mutex::new(fake_keyring::state_from_seed(
        &json!({ "kwallet": { "enabled": false } }),
    )));
    assert_plain_fallback(LinuxKeyringVault::with_connect(
        Backend::Kwallet6,
        fake_connect(state),
    ));
}

/// Чужой `kind` конверта не читается и не трогает шину.
#[test]
fn keyring_vault_rejects_foreign_envelope() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let connect: Connect = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { None })
    });
    let vault = LinuxKeyringVault::with_connect(Backend::GnomeLibsecret, connect);
    assert_eq!(
        vault.unprotect(&json!({ "version": 2, "kind": "dpapi", "blob": "AA==" })),
        None
    );
    vault.forget(&json!({ "version": 2, "kind": "plain", "data": {} }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
