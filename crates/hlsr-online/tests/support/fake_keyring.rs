//! Поддельные службы ключей на D-Bus (только Linux): Secret Service (`org.freedesktop.secrets`,
//! как GNOME Keyring) и KWallet (`org.kde.kwalletd6`, `org.kde.kwalletd5`, `org.kde.kwalletd`).
//! Журнал вызовов — JSON. Одна реализация на два случая: тесты Rust (соединение точка-точка, без
//! шины) и съём фикстуры с настоящим Electron 0.4.7 (`examples/fake_keyring.rs` на шине сеанса
//! `headless.sh`, `tools/reference/online/safe-storage.js`).

#![allow(dead_code)]

use hlsr_online::online_profile::session_crypto::{cbc_decrypt, cbc_encrypt};
use num_bigint::BigUint;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use zbus::object_server::{ObjectServer, SignalEmitter};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as Variant};
use zbus::{Connection, fdo, interface};

pub const SERVICE_PATH: &str = "/org/freedesktop/secrets";
pub const COLLECTION_PATH: &str = "/org/freedesktop/secrets/collection/login";
/// Та же коллекция по псевдониму `default` (спецификация Secret Service): Chromium пишет туда
/// служебную запись «Chrome Safe Storage Control».
pub const DEFAULT_ALIAS_PATH: &str = "/org/freedesktop/secrets/aliases/default";
pub const DH_ALGORITHM: &str = "dh-ietf1024-sha256-aes128-cbc-pkcs7";
/// Вторая группа Oakley (RFC 2409), 1024 бита, генератор 2.
const OAKLEY2: &str = "ffffffffffffffffc90fdaa22168c234c4c6628b80dc1cd129024e088a67cc74020bbea63b139b22514a08798e3404ddef9519b3cd3a431b302b0a6df25f14374fe1356d6d51c245e485b576625e7ec6f44c42e9a637ed6b0bff5cb6f406b7edee386bfb5a899fa5ae9f24117c4b1fe649286651ece65381ffffffffffffffff";
/// Закрытый ключ DH подделки: постоянный — подделке не нужна стойкость, а журнал повторяется.
const DH_PRIVATE: u64 = 0x5eed_c0de_1234_5678;
/// Вектор CBC секретов, которые отдаёт подделка.
const SECRET_IV: [u8; 16] = [0x42; 16];
/// Имена и пути KWallet: KDE 6, 5 и 4.
pub const KWALLET: [(&str, &str); 3] = [
    ("org.kde.kwalletd6", "/modules/kwalletd6"),
    ("org.kde.kwalletd5", "/modules/kwalletd5"),
    ("org.kde.kwalletd", "/modules/kwalletd"),
];

/// Секрет Secret Service: `(oayays)` — сессия, параметры, значение, тип содержимого.
pub type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[derive(Debug, Clone)]
pub struct FakeItem {
    pub id: u32,
    pub label: String,
    pub attributes: BTreeMap<String, String>,
    pub secret: Vec<u8>,
    pub content_type: String,
}

impl FakeItem {
    pub fn path(&self) -> String {
        format!("{COLLECTION_PATH}/{}", self.id)
    }
}

#[derive(Debug, Clone)]
pub struct FakeKWallet {
    pub enabled: bool,
    pub wallet: String,
    /// Папка → ключ → пароль.
    pub folders: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for FakeKWallet {
    fn default() -> Self {
        FakeKWallet {
            enabled: true,
            wallet: "kdewallet".to_owned(),
            folders: BTreeMap::new(),
        }
    }
}

/// Состояние служб и журнал.
#[derive(Debug, Default)]
pub struct FakeState {
    pub journal: Vec<Value>,
    pub items: Vec<FakeItem>,
    /// Связка закрыта: поиск отдаёт записи в `locked`, `Unlock` требует подсказки.
    pub locked: bool,
    /// Пользователь закрыл окно ввода пароля.
    pub dismiss_prompt: bool,
    pub next_id: u32,
    pub next_session: u32,
    /// Сессии: путь → ключ AES-128 (DH) или `None` (plain).
    pub sessions: HashMap<String, Option<[u8; 16]>>,
    pub next_prompt: u32,
    pub kwallet: FakeKWallet,
    /// Файл, куда после каждого вызова переписывается журнал (съём фикстуры).
    pub journal_file: Option<std::path::PathBuf>,
}

pub type Shared = Arc<Mutex<FakeState>>;

impl FakeState {
    fn log(&mut self, call: &str, args: Value) {
        self.journal.push(json!({ "call": call, "args": args }));
        if let Some(file) = &self.journal_file {
            let lines: Vec<String> = self.journal.iter().map(Value::to_string).collect();
            let _ = std::fs::write(file, lines.join("\n") + "\n");
        }
    }

    pub fn add_item(
        &mut self,
        label: &str,
        attributes: BTreeMap<String, String>,
        secret: &[u8],
    ) -> FakeItem {
        self.next_id += 1;
        let item = FakeItem {
            id: self.next_id,
            label: label.to_owned(),
            attributes,
            secret: secret.to_vec(),
            content_type: "text/plain".to_owned(),
        };
        self.items.push(item.clone());
        item
    }

    /// Пути записей, у которых есть все атрибуты `attributes` (как `SearchItems` службы).
    pub fn matches(&self, attributes: &HashMap<String, String>) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| {
                attributes
                    .iter()
                    .all(|(k, v)| item.attributes.get(k) == Some(v))
            })
            .map(FakeItem::path)
            .collect()
    }

    /// Секрет для ответа: в сессии DH — AES-128-CBC с вектором в `parameters`.
    fn wrap(&self, session: &OwnedObjectPath, value: &[u8], content_type: &str) -> Secret {
        match self.sessions.get(session.as_str()).copied().flatten() {
            Some(key) => (
                session.clone(),
                SECRET_IV.to_vec(),
                cbc_encrypt(&key, &SECRET_IV, value),
                content_type.to_owned(),
            ),
            None => (
                session.clone(),
                Vec::new(),
                value.to_vec(),
                content_type.to_owned(),
            ),
        }
    }

    /// Секрет от клиента в открытом виде.
    fn unwrap(&self, secret: &Secret) -> Vec<u8> {
        match self.sessions.get(secret.0.as_str()).copied().flatten() {
            Some(key) => {
                let iv: [u8; 16] = secret.1.as_slice().try_into().expect("вектор 16 байт");
                cbc_decrypt(&key, &iv, &secret.2).expect("секрет расшифровывается")
            }
            None => secret.2.clone(),
        }
    }

    fn item_by_path(&self, path: &str) -> Option<&FakeItem> {
        self.items.iter().find(|item| item.path() == path)
    }
}

/// Состояние из описания (`seed`): `{items: [{label, attributes, secret}], locked, dismissPrompt,
/// kwallet: {enabled, wallet, folders}}`.
pub fn state_from_seed(seed: &Value) -> FakeState {
    let mut state = FakeState {
        locked: seed["locked"] == json!(true),
        dismiss_prompt: seed["dismissPrompt"] == json!(true),
        ..FakeState::default()
    };
    for item in seed["items"].as_array().into_iter().flatten() {
        let attributes = item["attributes"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned()))
            .collect();
        state.add_item(
            item["label"].as_str().unwrap_or(""),
            attributes,
            item["secret"].as_str().unwrap_or("").as_bytes(),
        );
    }
    if let Some(kwallet) = seed.get("kwallet") {
        state.kwallet.enabled = kwallet["enabled"] != json!(false);
        if let Some(wallet) = kwallet["wallet"].as_str() {
            state.kwallet.wallet = wallet.to_owned();
        }
        for (folder, entries) in kwallet["folders"].as_object().into_iter().flatten() {
            let entries = entries
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned()))
                .collect();
            state.kwallet.folders.insert(folder.clone(), entries);
        }
    }
    state
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Обмен ключами `dh-ietf1024-sha256-aes128-cbc-pkcs7`, как у GNOME Keyring и libsecret:
/// общий секрет дополняется нулями слева до длины простого числа, ключ AES-128 — HKDF-SHA256 без
/// соли и без `info`. Возвращает открытый ключ подделки и ключ сессии.
pub fn dh_exchange(client_public: &[u8]) -> (Vec<u8>, [u8; 16]) {
    let prime = BigUint::parse_bytes(OAKLEY2.as_bytes(), 16).expect("простое число");
    let private = BigUint::from(DH_PRIVATE);
    let public = BigUint::from(2u32).modpow(&private, &prime);
    let shared = BigUint::from_bytes_be(client_public).modpow(&private, &prime);
    let mut ikm = vec![0u8; 128];
    let bytes = shared.to_bytes_be();
    ikm[128 - bytes.len()..].copy_from_slice(&bytes);
    let prk = hmac_sha256(&[0u8; 32], &ikm);
    let okm = hmac_sha256(&prk, &[1]);
    (public.to_bytes_be(), okm[..16].try_into().expect("16 байт"))
}

fn path(text: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(text.to_owned()).expect("путь D-Bus")
}

fn sorted(map: &HashMap<String, String>) -> Value {
    let sorted: BTreeMap<_, _> = map.iter().collect();
    json!(sorted)
}

fn paths_json(paths: &[OwnedObjectPath]) -> Value {
    json!(
        paths
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect::<Vec<_>>()
    )
}

/// Значение варианта для журнала.
fn variant_json(value: &Variant<'_>) -> Value {
    match value {
        Variant::Str(text) => json!(text.as_str()),
        Variant::Bool(b) => json!(b),
        Variant::U32(n) => json!(n),
        Variant::I32(n) => json!(n),
        Variant::U64(n) => json!(n),
        Variant::I64(n) => json!(n),
        Variant::Value(inner) => variant_json(inner),
        Variant::Array(array) => {
            if let Ok(bytes) = Vec::<u8>::try_from(value.try_clone().expect("копия значения"))
            {
                return json!({ "bytes": String::from_utf8_lossy(&bytes) });
            }
            Value::Array(array.iter().map(variant_json).collect())
        }
        Variant::Dict(dict) => {
            let mut map = BTreeMap::new();
            for (k, v) in dict.iter() {
                let key = match k {
                    Variant::Str(text) => text.as_str().to_owned(),
                    other => format!("{other:?}"),
                };
                map.insert(key, variant_json(v));
            }
            json!(map)
        }
        other => json!(format!("{other:?}")),
    }
}

// --- Secret Service ---

pub struct Service {
    state: Shared,
}

#[interface(name = "org.freedesktop.Secret.Service")]
impl Service {
    async fn open_session(
        &self,
        algorithm: &str,
        input: Variant<'_>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<(OwnedValue, OwnedObjectPath)> {
        let (session, output) = {
            let mut state = self.state.lock().unwrap();
            let shown = if algorithm == DH_ALGORITHM {
                json!("<открытый ключ>")
            } else {
                variant_json(&input)
            };
            state.log(
                "Service.OpenSession",
                json!({ "algorithm": algorithm, "input": shown }),
            );
            let (key, output) = match algorithm {
                "plain" => (None, OwnedValue::from(zbus::zvariant::Str::from(""))),
                DH_ALGORITHM => {
                    let client = Vec::<u8>::try_from(input.try_clone().expect("копия"))
                        .map_err(|_| fdo::Error::InvalidArgs("открытый ключ — ay".to_owned()))?;
                    let (public, key) = dh_exchange(&client);
                    let output = OwnedValue::try_from(Variant::from(public)).expect("значение");
                    (Some(key), output)
                }
                other => return Err(fdo::Error::NotSupported(format!("алгоритм {other}"))),
            };
            state.next_session += 1;
            let session = format!("{SERVICE_PATH}/session/{}", state.next_session);
            state.sessions.insert(session.clone(), key);
            (session, output)
        };
        server
            .at(
                session.as_str(),
                Session {
                    state: self.state.clone(),
                },
            )
            .await?;
        Ok((output, path(&session)))
    }

    async fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        let mut state = self.state.lock().unwrap();
        state.log(
            "Service.SearchItems",
            json!({ "attributes": sorted(&attributes) }),
        );
        let found: Vec<OwnedObjectPath> =
            state.matches(&attributes).iter().map(|p| path(p)).collect();
        if state.locked {
            (Vec::new(), found)
        } else {
            (found, Vec::new())
        }
    }

    async fn unlock(
        &self,
        objects: Vec<OwnedObjectPath>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        let prompt = {
            let mut state = self.state.lock().unwrap();
            state.log("Service.Unlock", json!({ "objects": paths_json(&objects) }));
            if !state.locked {
                return Ok((objects, path("/")));
            }
            state.next_prompt += 1;
            format!("{SERVICE_PATH}/prompt/{}", state.next_prompt)
        };
        server
            .at(
                prompt.as_str(),
                Prompt {
                    state: self.state.clone(),
                    objects: objects.clone(),
                },
            )
            .await?;
        Ok((Vec::new(), path(&prompt)))
    }

    async fn get_secrets(
        &self,
        items: Vec<OwnedObjectPath>,
        session: OwnedObjectPath,
    ) -> HashMap<OwnedObjectPath, Secret> {
        let mut state = self.state.lock().unwrap();
        state.log(
            "Service.GetSecrets",
            json!({ "items": paths_json(&items), "session": session.as_str() }),
        );
        items
            .into_iter()
            .filter_map(|item| {
                let found = state.item_by_path(item.as_str())?;
                let secret = state.wrap(&session, &found.secret, &found.content_type);
                Some((item, secret))
            })
            .collect()
    }

    async fn read_alias(&self, name: &str) -> OwnedObjectPath {
        self.state
            .lock()
            .unwrap()
            .log("Service.ReadAlias", json!({ "name": name }));
        if name == "default" {
            path(COLLECTION_PATH)
        } else {
            path("/")
        }
    }

    async fn create_collection(
        &self,
        properties: HashMap<String, Variant<'_>>,
        alias: &str,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        let props: BTreeMap<_, _> = properties
            .iter()
            .map(|(k, v)| (k.clone(), variant_json(v)))
            .collect();
        self.state.lock().unwrap().log(
            "Service.CreateCollection",
            json!({ "properties": props, "alias": alias }),
        );
        (path(COLLECTION_PATH), path("/"))
    }

    #[zbus(property)]
    async fn collections(&self) -> Vec<OwnedObjectPath> {
        vec![path(COLLECTION_PATH)]
    }
}

pub struct Session {
    state: Shared,
}

#[interface(name = "org.freedesktop.Secret.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<()> {
        let at = header
            .path()
            .map(|p| p.as_str().to_owned())
            .unwrap_or_default();
        {
            let mut state = self.state.lock().unwrap();
            state.log("Session.Close", json!({ "session": at }));
            state.sessions.remove(&at);
        }
        server.remove::<Session, _>(at.as_str()).await?;
        Ok(())
    }
}

pub struct Prompt {
    state: Shared,
    objects: Vec<OwnedObjectPath>,
}

#[interface(name = "org.freedesktop.Secret.Prompt")]
impl Prompt {
    async fn prompt(
        &self,
        window_id: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let dismissed = {
            let mut state = self.state.lock().unwrap();
            state.log("Prompt.Prompt", json!({ "windowId": window_id }));
            let dismissed = state.dismiss_prompt;
            if !dismissed {
                state.locked = false;
            }
            dismissed
        };
        let result = if dismissed {
            Variant::from(zbus::zvariant::Str::from(""))
        } else {
            Variant::from(self.objects.clone())
        };
        let emitter = emitter.to_owned();
        // Ответ на Prompt уходит раньше сигнала, как у настоящей службы.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let _ = Prompt::completed(&emitter, dismissed, result).await;
        });
        Ok(())
    }

    async fn dismiss(&self) {
        self.state.lock().unwrap().log("Prompt.Dismiss", json!({}));
    }

    #[zbus(signal)]
    async fn completed(
        emitter: &SignalEmitter<'_>,
        dismissed: bool,
        result: Variant<'_>,
    ) -> zbus::Result<()>;
}

pub struct Collection {
    state: Shared,
}

#[interface(name = "org.freedesktop.Secret.Collection")]
impl Collection {
    async fn create_item(
        &self,
        properties: HashMap<String, Variant<'_>>,
        secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
        replace: bool,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        // Как GNOME Keyring: `Attributes` — строго `a{ss}`, `Label` — `s` (иначе `InvalidArgs`).
        for (name, signature) in [
            ("org.freedesktop.Secret.Item.Attributes", "a{ss}"),
            ("org.freedesktop.Secret.Item.Label", "s"),
        ] {
            if let Some(value) = properties.get(name)
                && value.value_signature() != signature
            {
                return Err(fdo::Error::InvalidArgs(format!(
                    "{name}: {} вместо {signature}",
                    value.value_signature()
                )));
            }
        }
        let props: BTreeMap<_, _> = properties
            .iter()
            .map(|(k, v)| (k.clone(), variant_json(v)))
            .collect();
        let label = props
            .get("org.freedesktop.Secret.Item.Label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let attributes: BTreeMap<String, String> = props
            .get("org.freedesktop.Secret.Item.Attributes")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned()))
            .collect();
        let (item, fresh) = {
            let mut state = self.state.lock().unwrap();
            let value = state.unwrap(&secret);
            state.log(
                "Collection.CreateItem",
                json!({
                    "properties": props, "replace": replace,
                    "secret": { "session": secret.0.as_str(), "value": String::from_utf8_lossy(&value),
                                "contentType": secret.3 },
                }),
            );
            let existing = state
                .items
                .iter_mut()
                .find(|item| replace && item.attributes == attributes);
            match existing {
                Some(item) => {
                    item.secret = value;
                    item.label = label.clone();
                    item.content_type = secret.3.clone();
                    (item.clone(), false)
                }
                None => {
                    let item = state.add_item(&label, attributes, &value);
                    (item, true)
                }
            }
        };
        if fresh {
            server
                .at(
                    item.path().as_str(),
                    Item {
                        state: self.state.clone(),
                        id: item.id,
                    },
                )
                .await?;
        }
        Ok((path(&item.path()), path("/")))
    }

    async fn search_items(&self, attributes: HashMap<String, String>) -> Vec<OwnedObjectPath> {
        let mut state = self.state.lock().unwrap();
        state.log(
            "Collection.SearchItems",
            json!({ "attributes": sorted(&attributes) }),
        );
        state.matches(&attributes).iter().map(|p| path(p)).collect()
    }

    #[zbus(property)]
    async fn items(&self) -> Vec<OwnedObjectPath> {
        let state = self.state.lock().unwrap();
        state.items.iter().map(|item| path(&item.path())).collect()
    }

    #[zbus(property)]
    async fn label(&self) -> String {
        "Login".to_owned()
    }

    #[zbus(property)]
    async fn locked(&self) -> bool {
        self.state.lock().unwrap().locked
    }

    #[zbus(property)]
    async fn created(&self) -> u64 {
        1_700_000_000
    }

    #[zbus(property)]
    async fn modified(&self) -> u64 {
        1_700_000_000
    }
}

pub struct Item {
    state: Shared,
    id: u32,
}

impl Item {
    fn with<T>(&self, f: impl FnOnce(&FakeItem) -> T) -> Option<T> {
        let state = self.state.lock().unwrap();
        state.items.iter().find(|item| item.id == self.id).map(f)
    }
}

#[interface(name = "org.freedesktop.Secret.Item")]
impl Item {
    async fn get_secret(&self, session: OwnedObjectPath) -> fdo::Result<Secret> {
        let mut state = self.state.lock().unwrap();
        let id = self.id;
        state.log(
            "Item.GetSecret",
            json!({ "item": id, "session": session.as_str() }),
        );
        let item = state
            .items
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| fdo::Error::UnknownObject("нет записи".to_owned()))?;
        Ok(state.wrap(&session, &item.secret, &item.content_type))
    }

    async fn delete(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        let at = {
            let mut state = self.state.lock().unwrap();
            let id = self.id;
            state.log("Item.Delete", json!({ "item": id }));
            state.items.retain(|item| item.id != id);
            format!("{COLLECTION_PATH}/{id}")
        };
        server.remove::<Item, _>(at.as_str()).await?;
        Ok(path("/"))
    }

    #[zbus(property)]
    async fn locked(&self) -> bool {
        self.state.lock().unwrap().locked
    }

    #[zbus(property)]
    async fn attributes(&self) -> HashMap<String, String> {
        self.with(|item| item.attributes.clone().into_iter().collect())
            .unwrap_or_default()
    }

    #[zbus(property)]
    async fn label(&self) -> String {
        self.with(|item| item.label.clone()).unwrap_or_default()
    }

    #[zbus(property)]
    async fn created(&self) -> u64 {
        1_700_000_000
    }

    #[zbus(property)]
    async fn modified(&self) -> u64 {
        1_700_000_000
    }
}

// --- KWallet ---

pub struct KWallet {
    state: Shared,
    /// Имя службы: `kwalletd6`, `kwalletd5`, `kwalletd`.
    daemon: &'static str,
}

impl KWallet {
    fn log(&self, method: &str, args: Value) {
        let mut object = Map::new();
        object.insert("daemon".to_owned(), json!(self.daemon));
        if let Value::Object(args) = args {
            object.extend(args);
        }
        self.state
            .lock()
            .unwrap()
            .log(&format!("KWallet.{method}"), Value::Object(object));
    }
}

const HANDLE: i32 = 7;

#[interface(name = "org.kde.KWallet")]
impl KWallet {
    #[zbus(name = "isEnabled")]
    async fn is_enabled(&self) -> bool {
        self.log("isEnabled", json!({}));
        self.state.lock().unwrap().kwallet.enabled
    }

    #[zbus(name = "networkWallet")]
    async fn network_wallet(&self) -> String {
        self.log("networkWallet", json!({}));
        self.state.lock().unwrap().kwallet.wallet.clone()
    }

    #[zbus(name = "localWallet")]
    async fn local_wallet(&self) -> String {
        self.log("localWallet", json!({}));
        self.state.lock().unwrap().kwallet.wallet.clone()
    }

    #[zbus(name = "open")]
    async fn open(&self, wallet: &str, w_id: i64, appid: &str) -> i32 {
        self.log(
            "open",
            json!({ "wallet": wallet, "wId": w_id, "appid": appid }),
        );
        let state = self.state.lock().unwrap();
        if state.kwallet.enabled && wallet == state.kwallet.wallet {
            HANDLE
        } else {
            -1
        }
    }

    #[zbus(name = "hasFolder")]
    async fn has_folder(&self, handle: i32, folder: &str, appid: &str) -> bool {
        self.log(
            "hasFolder",
            json!({ "handle": handle, "folder": folder, "appid": appid }),
        );
        self.state
            .lock()
            .unwrap()
            .kwallet
            .folders
            .contains_key(folder)
    }

    #[zbus(name = "createFolder")]
    async fn create_folder(&self, handle: i32, folder: &str, appid: &str) -> bool {
        self.log(
            "createFolder",
            json!({ "handle": handle, "folder": folder, "appid": appid }),
        );
        self.state
            .lock()
            .unwrap()
            .kwallet
            .folders
            .entry(folder.to_owned())
            .or_default();
        true
    }

    #[zbus(name = "hasEntry")]
    async fn has_entry(&self, handle: i32, folder: &str, key: &str, appid: &str) -> bool {
        self.log(
            "hasEntry",
            json!({ "handle": handle, "folder": folder, "key": key, "appid": appid }),
        );
        let state = self.state.lock().unwrap();
        state
            .kwallet
            .folders
            .get(folder)
            .is_some_and(|f| f.contains_key(key))
    }

    #[zbus(name = "entryType")]
    async fn entry_type(&self, handle: i32, folder: &str, key: &str, appid: &str) -> i32 {
        self.log(
            "entryType",
            json!({ "handle": handle, "folder": folder, "key": key, "appid": appid }),
        );
        let state = self.state.lock().unwrap();
        // 1 — пароль (KWallet::Wallet::Password), 0 — нет записи.
        i32::from(
            state
                .kwallet
                .folders
                .get(folder)
                .is_some_and(|f| f.contains_key(key)),
        )
    }

    #[zbus(name = "readPassword")]
    async fn read_password(&self, handle: i32, folder: &str, key: &str, appid: &str) -> String {
        self.log(
            "readPassword",
            json!({ "handle": handle, "folder": folder, "key": key, "appid": appid }),
        );
        let state = self.state.lock().unwrap();
        state
            .kwallet
            .folders
            .get(folder)
            .and_then(|f| f.get(key))
            .cloned()
            .unwrap_or_default()
    }

    #[zbus(name = "writePassword")]
    async fn write_password(
        &self,
        handle: i32,
        folder: &str,
        key: &str,
        value: &str,
        appid: &str,
    ) -> i32 {
        self.log(
            "writePassword",
            json!({ "handle": handle, "folder": folder, "key": key, "value": value, "appid": appid }),
        );
        self.state
            .lock()
            .unwrap()
            .kwallet
            .folders
            .entry(folder.to_owned())
            .or_default()
            .insert(key.to_owned(), value.to_owned());
        0
    }

    #[zbus(name = "close")]
    async fn close(&self, handle: i32, force: bool, appid: &str) -> i32 {
        self.log(
            "close",
            json!({ "handle": handle, "force": force, "appid": appid }),
        );
        0
    }
}

/// Поставить объекты служб на соединение `connection`.
pub async fn serve(connection: &Connection, state: Shared) -> zbus::Result<()> {
    let server = connection.object_server();
    server
        .at(
            SERVICE_PATH,
            Service {
                state: state.clone(),
            },
        )
        .await?;
    for at in [COLLECTION_PATH, DEFAULT_ALIAS_PATH] {
        server
            .at(
                at,
                Collection {
                    state: state.clone(),
                },
            )
            .await?;
    }
    let ids: Vec<(u32, String)> = state
        .lock()
        .unwrap()
        .items
        .iter()
        .map(|item| (item.id, item.path()))
        .collect();
    for (id, at) in ids {
        server
            .at(
                at.as_str(),
                Item {
                    state: state.clone(),
                    id,
                },
            )
            .await?;
    }
    for (name, at) in KWALLET {
        let daemon = name.trim_start_matches("org.kde.");
        server
            .at(
                at,
                KWallet {
                    state: state.clone(),
                    daemon,
                },
            )
            .await?;
    }
    Ok(())
}

/// Тест: пара соединений точка-точка — клиент и поддельные службы на другом конце.
#[cfg(test)]
pub async fn p2p(state: Shared) -> Connection {
    let (left, right) = tokio::net::UnixStream::pair().expect("пара сокетов");
    let guid = zbus::Guid::generate();
    let server = zbus::connection::Builder::unix_stream(left)
        .server(guid)
        .expect("guid")
        .p2p()
        .build();
    let client = zbus::connection::Builder::unix_stream(right).p2p().build();
    let (server, client) = tokio::join!(server, client);
    let server = server.expect("сервер D-Bus");
    serve(&server, state).await.expect("службы");
    // Соединение сервера живёт, пока живёт клиент: его держит задача.
    tokio::spawn(async move {
        let _keep = server;
        std::future::pending::<()>().await;
    });
    client.expect("клиент D-Bus")
}
