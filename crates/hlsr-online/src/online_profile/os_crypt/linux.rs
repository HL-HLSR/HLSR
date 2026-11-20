//! Linux: пароль `os_crypt` из связки ключей по D-Bus и секрет нового хранилища сессии. Бэкенд —
//! по окружению ([`super::select_backend`]). Secret Service (`org.freedesktop.secrets`, как GNOME
//! Keyring): запись Chromium — схема `chrome_libsecret_os_crypt_password_v2`, Chromium ищет её
//! только по `application = "hlsr"` (`DONT_MATCH_NAME`); здесь сначала вместе с `xdg:schema` этой
//! схемы, затем как Chromium. KWallet 6/5/4 (`org.kde.kwalletd*`): папка `Chromium Keys`, ключ
//! `Chromium Safe Storage`, appid `hlsr`. Расшифровка — [`super::decrypt_linux`].
//!
//! Запись сессии 0.5 — своя схема [`SESSION_SCHEMA`] и **без** атрибута `application`: иначе
//! Chromium 0.4.7 на бете мог бы найти её своим поиском и взять секрет сессии за пароль `os_crypt`
//! (общий вход 0.4.7 и 0.5, карта §6.2). Служба не ответила при записи — конверт `plain` 0600, как
//! у 0.4.7 (`isEncryptionAvailable()` = false).
//!
//! Собирается и проверяется только под Linux (`cfg(target_os = "linux")`; на Windows — через
//! `scripts/slint/check-linux.sh`). Запуск, D-Bus и подтверждение имён записей связки — на Linux
//! (`docs/superpowers/notes/2026-10-16-linux-deferred.md`). zbus асинхронный (как во всём крейте);
//! синхронные методы хранилища мостятся через отдельный поток со своим `current_thread`-рантаймом,
//! поэтому зовутся из любого контекста (задача 14 — через `spawn_blocking`).

use super::super::secure_session::SafeStorage;
use super::super::session_store::{PlainVault, SESSION_VERSION, SessionError, SessionVault};
use super::{Backend, decrypt_linux};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as Variant};
use zbus::{Connection, proxy};

/// Имя приложения 0.4.7 (атрибут `application`, appid KWallet): из `app.name`, как каталог данных.
pub const APP_NAME: &str = "hlsr";
/// Схема записи Chromium в Secret Service.
pub const CHROMIUM_SCHEMA: &str = "chrome_libsecret_os_crypt_password_v2";
/// Папка и ключ Chromium в KWallet.
pub const KWALLET_FOLDER: &str = "Chromium Keys";
pub const KWALLET_KEY: &str = "Chromium Safe Storage";
/// Схема (`xdg:schema`), второй атрибут и метка записи сессии 0.5 в Secret Service. Атрибута
/// `application` у записи нет (см. описание модуля).
pub const SESSION_SCHEMA: &str = "hlsr_launcher_session_v2";
pub const SESSION_ATTRIBUTE: (&str, &str) = ("hlsr-launcher", "session");
pub const SESSION_LABEL: &str = "HLSR Launcher Session";
/// Папка и ключ записи сессии 0.5 в KWallet (Chromium — [`KWALLET_FOLDER`]/[`KWALLET_KEY`]).
pub const SESSION_KWALLET_FOLDER: &str = "HLSR";
pub const SESSION_KWALLET_KEY: &str = "session";

/// Секрет Secret Service: сессия, параметры, значение, тип содержимого.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[proxy(
    interface = "org.freedesktop.Secret.Service",
    default_service = "org.freedesktop.secrets",
    default_path = "/org/freedesktop/secrets"
)]
trait SecretService {
    fn open_session(
        &self,
        algorithm: &str,
        input: &Variant<'_>,
    ) -> zbus::Result<(OwnedValue, OwnedObjectPath)>;
    fn search_items(
        &self,
        attributes: HashMap<&str, &str>,
    ) -> zbus::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;
    fn unlock(
        &self,
        objects: &[OwnedObjectPath],
    ) -> zbus::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)>;
    fn get_secrets(
        &self,
        items: &[OwnedObjectPath],
        session: &OwnedObjectPath,
    ) -> zbus::Result<HashMap<OwnedObjectPath, Secret>>;
    fn read_alias(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.Secret.Collection",
    default_service = "org.freedesktop.secrets"
)]
trait SecretCollection {
    fn create_item(
        &self,
        properties: HashMap<&str, Variant<'_>>,
        secret: &Secret,
        replace: bool,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
    fn search_items(&self, attributes: HashMap<&str, &str>) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[proxy(
    interface = "org.freedesktop.Secret.Item",
    default_service = "org.freedesktop.secrets"
)]
trait SecretItem {
    fn delete(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(interface = "org.kde.KWallet")]
trait KWallet {
    #[zbus(name = "networkWallet")]
    fn network_wallet(&self) -> zbus::Result<String>;
    #[zbus(name = "open")]
    fn open(&self, wallet: &str, w_id: i64, appid: &str) -> zbus::Result<i32>;
    #[zbus(name = "hasFolder")]
    fn has_folder(&self, handle: i32, folder: &str, appid: &str) -> zbus::Result<bool>;
    #[zbus(name = "createFolder")]
    fn create_folder(&self, handle: i32, folder: &str, appid: &str) -> zbus::Result<bool>;
    #[zbus(name = "readPassword")]
    fn read_password(
        &self,
        handle: i32,
        folder: &str,
        key: &str,
        appid: &str,
    ) -> zbus::Result<String>;
    #[zbus(name = "writePassword")]
    fn write_password(
        &self,
        handle: i32,
        folder: &str,
        key: &str,
        value: &str,
        appid: &str,
    ) -> zbus::Result<i32>;
    #[zbus(name = "removeEntry")]
    fn remove_entry(&self, handle: i32, folder: &str, key: &str, appid: &str) -> zbus::Result<i32>;
    #[zbus(name = "close")]
    fn close(&self, handle: i32, force: bool, appid: &str) -> zbus::Result<i32>;
}

/// Служба и путь KWallet для версии KDE.
fn kwallet_endpoint(backend: Backend) -> Option<(&'static str, &'static str)> {
    match backend {
        Backend::Kwallet6 => Some(("org.kde.kwalletd6", "/modules/kwalletd6")),
        Backend::Kwallet5 => Some(("org.kde.kwalletd5", "/modules/kwalletd5")),
        Backend::Kwallet4 => Some(("org.kde.kwalletd", "/modules/kwalletd")),
        _ => None,
    }
}

/// Соединение с D-Bus для хранилища сессии: открывается в рантайме [`run_dbus`]. По умолчанию —
/// шина сеанса ([`session_bus`]); тесты подставляют поддельные службы. `None` — шины нет.
pub type Connect =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<Connection>> + Send>> + Send + Sync>;

/// Шина сеанса пользователя.
pub fn session_bus() -> Connect {
    Arc::new(|| Box::pin(async { Connection::session().await.ok() }))
}

/// Выполнить работу с D-Bus в отдельном потоке со своим рантаймом: зовётся из синхронного кода в
/// любом контексте (вложенного рантайма не возникает).
fn run_dbus<T, F>(connect: Connect, work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce(Connection) -> Pin<Box<dyn Future<Output = Option<T>> + Send>> + Send + 'static,
{
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        rt.block_on(async move {
            let connection = connect().await?;
            work(connection).await
        })
    })
    .join()
    .ok()
    .flatten()
}

/// Открыть сессию Secret Service (`plain`) и прочитать секрет первой подходящей записи: поиски
/// `searches` по порядку, берётся первый, нашедший хоть одну запись.
async fn libsecret_lookup(
    connection: &Connection,
    searches: &[HashMap<&str, &str>],
) -> Option<Vec<u8>> {
    let service = SecretServiceProxy::new(connection).await.ok()?;
    let (_output, session) = service
        .open_session("plain", &Variant::from(""))
        .await
        .ok()?;
    let mut found = None;
    for attributes in searches {
        let (unlocked, locked) = service.search_items(attributes.clone()).await.ok()?;
        if !unlocked.is_empty() || !locked.is_empty() {
            found = Some((unlocked, locked));
            break;
        }
    }
    let (unlocked, locked) = found?;
    let mut items = unlocked;
    if items.is_empty() && !locked.is_empty() {
        // Закрытая связка показывает окно ввода пароля (как 0.4.7 на старте). `Unlock` возвращает
        // либо уже открытые записи, либо `Prompt`; ожидание окна проверяется на Linux (отложено).
        let (now_unlocked, _prompt) = service.unlock(&locked).await.ok()?;
        items = now_unlocked;
    }
    let first = items.into_iter().next()?;
    let secrets = service
        .get_secrets(std::slice::from_ref(&first), &session)
        .await
        .ok()?;
    secrets.get(&first).map(|secret| secret.2.clone())
}

/// Поиски пароля Chromium по порядку: сначала с `xdg:schema` (libsecret кладёт его в каждую запись,
/// сохранённую по схеме, — так чужая запись с тем же `application` не попадёт), затем как сам
/// Chromium — только `application` (запись без `xdg:schema`).
fn chromium_searches() -> [HashMap<&'static str, &'static str>; 2] {
    [
        HashMap::from([("application", APP_NAME), ("xdg:schema", CHROMIUM_SCHEMA)]),
        HashMap::from([("application", APP_NAME)]),
    ]
}

/// Прочитать пароль `os_crypt` из KWallet (чтение входа 0.4.7 и секрета нового хранилища).
async fn kwallet_read(
    connection: &Connection,
    backend: Backend,
    folder: &str,
    key: &str,
) -> Option<Vec<u8>> {
    let (service, path) = kwallet_endpoint(backend)?;
    let wallet = KWalletProxy::builder(connection)
        .destination(service)
        .ok()?
        .path(path)
        .ok()?
        .build()
        .await
        .ok()?;
    let name = wallet.network_wallet().await.ok()?;
    let handle = wallet.open(&name, 0, APP_NAME).await.ok()?;
    if handle < 0 || !wallet.has_folder(handle, folder, APP_NAME).await.ok()? {
        let _ = wallet.close(handle, false, APP_NAME).await;
        return None;
    }
    let password = wallet
        .read_password(handle, folder, key, APP_NAME)
        .await
        .ok()?;
    let _ = wallet.close(handle, false, APP_NAME).await;
    if password.is_empty() {
        None
    } else {
        Some(password.into_bytes())
    }
}

/// `safeStorage` Linux: пароль связки по бэкенду, расшифровка `v11`/`v10`.
pub struct LinuxSafeStorage {
    backend: Backend,
    password: Option<Vec<u8>>,
}

impl LinuxSafeStorage {
    /// Определить бэкенд по окружению и прочитать пароль связки (чтение входа 0.4.7).
    pub async fn detect(connection: &Connection) -> LinuxSafeStorage {
        let backend = super::select_backend(|name| std::env::var(name).ok());
        Self::with_backend(connection, backend).await
    }

    /// То же с заданным бэкендом (тесты: без чтения окружения процесса).
    pub async fn with_backend(connection: &Connection, backend: Backend) -> LinuxSafeStorage {
        let password = Self::read_password(connection, backend).await;
        LinuxSafeStorage { backend, password }
    }

    async fn read_password(connection: &Connection, backend: Backend) -> Option<Vec<u8>> {
        match backend {
            Backend::BasicText => None,
            Backend::GnomeLibsecret => libsecret_lookup(connection, &chromium_searches()).await,
            _ => kwallet_read(connection, backend, KWALLET_FOLDER, KWALLET_KEY).await,
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }
}

impl SafeStorage for LinuxSafeStorage {
    fn is_encryption_available(&self) -> bool {
        // Связка выбрана и пароль получен (если нет — Chromium писал бы `plain`).
        self.backend != Backend::BasicText && self.password.is_some()
    }

    fn selected_storage_backend(&self) -> Option<Backend> {
        Some(self.backend)
    }

    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        decrypt_linux(self.password.as_deref()?, cipher)
    }
}

/// Новое хранилище сессии в связке ключей Linux: секрет (открытый текст сессии) — в Secret Service
/// или KWallet, файл — локатор `{version:2, kind:"keyring", backend, …}`. Служба не ответила при
/// записи — конверт `plain` ([`PlainVault`]); читаются оба.
pub struct LinuxKeyringVault {
    backend: Backend,
    connect: Connect,
}

impl LinuxKeyringVault {
    /// Хранилище на шине сеанса.
    pub fn new(backend: Backend) -> LinuxKeyringVault {
        LinuxKeyringVault::with_connect(backend, session_bus())
    }

    /// То же с заданным соединением (тесты: поддельные службы вместо шины пользователя).
    pub fn with_connect(backend: Backend, connect: Connect) -> LinuxKeyringVault {
        LinuxKeyringVault { backend, connect }
    }
}

/// Атрибуты записи нового хранилища: своя схема и свой атрибут, без `application`.
fn session_attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("xdg:schema", SESSION_SCHEMA), SESSION_ATTRIBUTE])
}

/// Свойства `CreateItem` записи сессии. `Attributes` по спецификации Secret Service — `a{ss}`
/// (GNOME Keyring отвергает другое), поэтому значения словаря — `&str`, а не `Variant` (`a{sv}`).
fn session_item_properties() -> HashMap<&'static str, Variant<'static>> {
    HashMap::from([
        (
            "org.freedesktop.Secret.Item.Label",
            Variant::from(SESSION_LABEL),
        ),
        (
            "org.freedesktop.Secret.Item.Attributes",
            Variant::from(session_attributes()),
        ),
    ])
}

/// Положить секрет сессии в Secret Service на соединении `connection` (задача 14 и отложенные тесты
/// Linux).
pub async fn store_libsecret(connection: &Connection, plaintext: &[u8]) -> Option<()> {
    let service = SecretServiceProxy::new(connection).await.ok()?;
    let (_output, session) = service
        .open_session("plain", &Variant::from(""))
        .await
        .ok()?;
    let collection_path = service.read_alias("default").await.ok()?;
    let collection = SecretCollectionProxy::builder(connection)
        .path(collection_path)
        .ok()?
        .build()
        .await
        .ok()?;
    let secret: Secret = (
        session,
        Vec::new(),
        plaintext.to_vec(),
        "application/octet-stream".to_owned(),
    );
    collection
        .create_item(session_item_properties(), &secret, true)
        .await
        .ok()?;
    Some(())
}

/// Прочитать секрет сессии из Secret Service.
pub async fn load_libsecret(connection: &Connection) -> Option<Vec<u8>> {
    libsecret_lookup(connection, &[session_attributes()]).await
}

/// Удалить секрет сессии из Secret Service.
pub async fn forget_libsecret(connection: &Connection) -> Option<()> {
    let service = SecretServiceProxy::new(connection).await.ok()?;
    let (unlocked, locked) = service.search_items(session_attributes()).await.ok()?;
    for item in unlocked.into_iter().chain(locked) {
        if let Ok(builder) = SecretItemProxy::builder(connection).path(item)
            && let Ok(proxy) = builder.build().await
        {
            let _ = proxy.delete().await;
        }
    }
    Some(())
}

/// Положить секрет сессии в KWallet (значение base64, чтобы не зависеть от кодировки).
pub async fn store_kwallet(
    connection: &Connection,
    backend: Backend,
    plaintext: &[u8],
) -> Option<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(plaintext);
    let (service, path) = kwallet_endpoint(backend)?;
    let wallet = KWalletProxy::builder(connection)
        .destination(service)
        .ok()?
        .path(path)
        .ok()?
        .build()
        .await
        .ok()?;
    let name = wallet.network_wallet().await.ok()?;
    let handle = wallet.open(&name, 0, APP_NAME).await.ok()?;
    if handle < 0 {
        return None;
    }
    if !wallet
        .has_folder(handle, SESSION_KWALLET_FOLDER, APP_NAME)
        .await
        .ok()?
    {
        wallet
            .create_folder(handle, SESSION_KWALLET_FOLDER, APP_NAME)
            .await
            .ok()?;
    }
    let written = wallet
        .write_password(
            handle,
            SESSION_KWALLET_FOLDER,
            SESSION_KWALLET_KEY,
            &encoded,
            APP_NAME,
        )
        .await
        .ok()?;
    let _ = wallet.close(handle, false, APP_NAME).await;
    (written == 0).then_some(())
}

/// Прочитать секрет сессии из KWallet.
pub async fn load_kwallet(connection: &Connection, backend: Backend) -> Option<Vec<u8>> {
    let encoded = kwallet_read(
        connection,
        backend,
        SESSION_KWALLET_FOLDER,
        SESSION_KWALLET_KEY,
    )
    .await?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
}

/// Удалить секрет сессии из KWallet.
pub async fn forget_kwallet(connection: &Connection, backend: Backend) -> Option<()> {
    let (service, path) = kwallet_endpoint(backend)?;
    let wallet = KWalletProxy::builder(connection)
        .destination(service)
        .ok()?
        .path(path)
        .ok()?
        .build()
        .await
        .ok()?;
    let name = wallet.network_wallet().await.ok()?;
    let handle = wallet.open(&name, 0, APP_NAME).await.ok()?;
    if handle >= 0 {
        let _ = wallet
            .remove_entry(
                handle,
                SESSION_KWALLET_FOLDER,
                SESSION_KWALLET_KEY,
                APP_NAME,
            )
            .await;
        let _ = wallet.close(handle, false, APP_NAME).await;
    }
    Some(())
}

impl SessionVault for LinuxKeyringVault {
    fn kind(&self) -> &'static str {
        "keyring"
    }

    fn protect(&self, plaintext: &[u8]) -> Result<Value, SessionError> {
        let backend = self.backend;
        let payload = plaintext.to_vec();
        let stored = run_dbus(self.connect.clone(), move |connection| {
            Box::pin(async move {
                match backend {
                    Backend::GnomeLibsecret => store_libsecret(&connection, &payload).await,
                    Backend::Kwallet6 | Backend::Kwallet5 | Backend::Kwallet4 => {
                        store_kwallet(&connection, backend, &payload).await
                    }
                    Backend::BasicText => None,
                }
            })
        });
        match stored {
            Some(()) => Ok(json!({
                "version": SESSION_VERSION,
                "kind": "keyring",
                "backend": self.backend.as_str(),
                "schema": SESSION_SCHEMA,
            })),
            // Служба выбрана, но не ответила (нет шины или службы, KWallet выключен, связка закрыта
            // и окно закрыто): как 0.4.7 — у него тогда `isEncryptionAvailable()` = false и `plain`
            // (спецификация, решение 4: «без службы — plain 0600»).
            None => PlainVault.protect(plaintext),
        }
    }

    fn unprotect(&self, envelope: &Value) -> Option<Vec<u8>> {
        match envelope.get("kind").and_then(Value::as_str) {
            Some("keyring") => {}
            Some("plain") => return PlainVault.unprotect(envelope),
            _ => return None,
        }
        let backend = self.backend;
        run_dbus(self.connect.clone(), move |connection| {
            Box::pin(async move {
                match backend {
                    Backend::GnomeLibsecret => load_libsecret(&connection).await,
                    Backend::Kwallet6 | Backend::Kwallet5 | Backend::Kwallet4 => {
                        load_kwallet(&connection, backend).await
                    }
                    Backend::BasicText => None,
                }
            })
        })
    }

    /// Секрет в связке убирается только для конверта `keyring`: у `plain` связки не было (не
    /// открывать окно пароля KWallet при выходе ради записи, которой нет).
    fn forget(&self, envelope: &Value) {
        if envelope.get("kind").and_then(Value::as_str) != Some("keyring") {
            return;
        }
        let backend = self.backend;
        let _ = run_dbus(self.connect.clone(), move |connection| {
            Box::pin(async move {
                match backend {
                    Backend::GnomeLibsecret => forget_libsecret(&connection).await,
                    Backend::Kwallet6 | Backend::Kwallet5 | Backend::Kwallet4 => {
                        forget_kwallet(&connection, backend).await
                    }
                    Backend::BasicText => Some(()),
                }
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Атрибуты записи сессии уходят как `a{ss}` и без `application` (её не найдёт поиск Chromium).
    #[test]
    fn session_item_attributes_are_string_dict_without_application() {
        let properties = session_item_properties();
        let attributes = &properties["org.freedesktop.Secret.Item.Attributes"];
        assert_eq!(attributes.value_signature(), "a{ss}");
        let Variant::Dict(dict) = attributes else {
            panic!("Attributes — словарь");
        };
        let keys: Vec<String> = dict
            .iter()
            .map(|(key, _)| match key {
                Variant::Str(text) => text.as_str().to_owned(),
                other => format!("{other:?}"),
            })
            .collect();
        assert!(keys.contains(&"xdg:schema".to_owned()));
        assert!(!keys.contains(&"application".to_owned()));
        assert_eq!(
            properties["org.freedesktop.Secret.Item.Label"].value_signature(),
            "s"
        );
    }

    #[test]
    fn chromium_search_prefers_schema_then_application_only() {
        let [strict, loose] = chromium_searches();
        assert_eq!(strict.get("xdg:schema"), Some(&CHROMIUM_SCHEMA));
        assert_eq!(strict.get("application"), Some(&APP_NAME));
        assert_eq!(loose, HashMap::from([("application", APP_NAME)]));
    }
}
