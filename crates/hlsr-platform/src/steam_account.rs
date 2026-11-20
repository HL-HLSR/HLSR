//! Активный аккаунт Steam на Windows (`SteamAccount.js`): `ActiveUser` из реестра
//! (`HKCU\Software\Valve\Steam\ActiveProcess`), SteamID64 и ID2, имя из `config/loginusers.vdf`,
//! аватар из `config/avatarcache/<SteamID64>.png`. Состояние связи со Steam — по поиску его
//! процесса.

use crate::steam_fs::{RealFs, SteamFs, read_to_string};
use crate::steam_locator::{ACTIVE_PROCESS_KEY, SteamLocating, read_registry_value};
use crate::vdf::{self, VdfValue, get_insensitive, get_insensitive_in};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_delivery::ports::{Logger, Registry, SystemRegistry, TracingLogger};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// `STEAM_ID64_BASE`: SteamID64 аккаунта с номером 0.
pub const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;
/// `MAX_AVATAR_BYTES`: аватар больше 512 КиБ не берётся.
pub const MAX_AVATAR_BYTES: u64 = 512 * 1024;
/// Начало PNG.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// `steamId64Of(accountId)`.
pub fn steam_id64_of(account_id: u64) -> String {
    (STEAM_ID64_BASE + account_id).to_string()
}

/// `steamId2Of(accountId)`: `STEAM_0:<чётность>:<номер / 2>`. Знак числа — как у `BigInt` в Linux:
/// остаток и деление, как в JS.
pub fn steam_id2_of(account_id: i128) -> String {
    format!("STEAM_0:{}:{}", account_id % 2, account_id / 2)
}

/// `describe()`: что лаунчер показывает об аккаунте.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    pub signed_in: bool,
    pub steam_id64: Option<String>,
    pub steam_id2: Option<String>,
    pub persona_name: Option<String>,
    pub avatar_data_uri: Option<String>,
    /// `checking`, `connected`, `offline`, `elevated`, `unknown`.
    pub connection: String,
}

impl AccountInfo {
    /// Аккаунта нет.
    pub fn empty(connection: &str) -> AccountInfo {
        AccountInfo {
            signed_in: false,
            steam_id64: None,
            steam_id2: None,
            persona_name: None,
            avatar_data_uri: None,
            connection: connection.to_owned(),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "signedIn": self.signed_in,
            "steamId64": self.steam_id64,
            "steamId2": self.steam_id2,
            "personaName": self.persona_name,
            "avatarDataUri": self.avatar_data_uri,
            "connection": self.connection,
        })
    }
}

/// Аватар из `config/avatarcache`: PNG не больше [`MAX_AVATAR_BYTES`] как `data:`-ссылка.
pub fn avatar_data_uri(fs: &dyn SteamFs, file: &str) -> Option<String> {
    let stat = fs.stat(file).ok()?;
    if !stat.is_file || stat.size == 0 || stat.size > MAX_AVATAR_BYTES {
        return None;
    }
    let data = fs.read(file).ok()?;
    if data.len() < PNG_SIGNATURE.len() || data[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return None;
    }
    Some(format!("data:image/png;base64,{}", STANDARD.encode(data)))
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct AccountOptions {
    pub fs: Option<Arc<dyn SteamFs>>,
    pub registry: Option<Arc<dyn Registry>>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `process.platform`; по умолчанию — эта ОС.
    pub platform: Option<Platform>,
}

struct Memory {
    last_known: Option<AccountInfo>,
    connection: String,
}

/// `SteamAccount` (Windows).
pub struct SteamAccount {
    locator: Arc<dyn SteamLocating>,
    fs: Arc<dyn SteamFs>,
    registry: Arc<dyn Registry>,
    logger: Arc<dyn Logger>,
    platform: Platform,
    path: NodePath,
    memory: Mutex<Memory>,
}

impl SteamAccount {
    pub fn new(locator: Arc<dyn SteamLocating>, options: AccountOptions) -> SteamAccount {
        let platform = options.platform.unwrap_or(Platform::CURRENT);
        SteamAccount {
            locator,
            fs: options.fs.unwrap_or_else(|| Arc::new(RealFs)),
            registry: options.registry.unwrap_or_else(|| Arc::new(SystemRegistry)),
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            platform,
            path: crate::process_inspect::node_path(platform),
            memory: Mutex::new(Memory {
                last_known: None,
                connection: "checking".to_owned(),
            }),
        }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Состояние связи на сейчас (`this.connection`).
    pub fn connection(&self) -> String {
        self.memory().connection.clone()
    }

    /// `describe()`.
    pub fn describe(&self) -> AccountInfo {
        let connection = self.connection();
        let empty = AccountInfo::empty(&connection);

        if self.platform != Platform::Win32 {
            return empty;
        }
        let Ok(steam) = self.locator.locate_steam() else {
            return empty;
        };
        let Some(account_id) = self.active_account_id() else {
            return empty;
        };

        let steam_id64 = steam_id64_of(account_id);
        let mut result = AccountInfo {
            signed_in: true,
            steam_id2: Some(steam_id2_of(i128::from(account_id))),
            persona_name: self.persona_name_of(&steam.install_path, &steam_id64),
            avatar_data_uri: self.avatar_of(&steam.install_path, &steam_id64),
            steam_id64: Some(steam_id64.clone()),
            connection: empty.connection,
        };

        let mut memory = self.memory();
        if result.avatar_data_uri.is_none()
            && let Some(last) = &memory.last_known
            && last.steam_id64.as_deref() == Some(steam_id64.as_str())
        {
            result.avatar_data_uri = last.avatar_data_uri.clone();
        }
        memory.last_known = Some(result.clone());
        result
    }

    /// `refreshConnection()`.
    pub fn refresh_connection(&self) -> String {
        let connection = self.measure_connection();
        self.memory().connection = connection.clone();
        connection
    }

    fn measure_connection(&self) -> String {
        if self.platform != Platform::Win32 {
            return "unknown".to_owned();
        }
        let state = self.locator.describe_steam_only();
        if state.get("ok").and_then(Value::as_bool) == Some(true) {
            return "connected".to_owned();
        }
        if state.get("code").and_then(Value::as_str) == Some("STEAM_ELEVATED") {
            return "elevated".to_owned();
        }
        "offline".to_owned()
    }

    /// `activeAccountId()`: `ActiveUser` из реестра — целое больше нуля.
    pub fn active_account_id(&self) -> Option<u64> {
        let raw = read_registry_value(self.registry.as_ref(), ACTIVE_PROCESS_KEY, "ActiveUser");
        let number = raw.to_number();
        (number.fract() == 0.0 && number > 0.0 && number < 9.0e15).then_some(number as u64)
    }

    /// `personaNameOf(installPath, steamId64)`.
    pub fn persona_name_of(&self, install_path: &str, steam_id64: &str) -> Option<String> {
        let file = self.path.join(&[install_path, "config", "loginusers.vdf"]);
        let text = read_to_string(self.fs.as_ref(), &file).ok()?;
        let tree = match vdf::parse(&text) {
            Ok(tree) => tree,
            Err(error) => {
                self.logger.warn(&format!(
                    "[steam] loginusers.vdf не разобран: {}",
                    error.message
                ));
                return None;
            }
        };
        let users = get_insensitive(&tree, "users").and_then(VdfValue::as_object)?;
        let record = get_insensitive(users, steam_id64)?;
        let name = get_insensitive_in(Some(record), "PersonaName").and_then(VdfValue::as_str)?;
        let name = js::trim(name);
        (!name.is_empty()).then(|| name.to_owned())
    }

    /// `avatarOf(installPath, steamId64)`.
    pub fn avatar_of(&self, install_path: &str, steam_id64: &str) -> Option<String> {
        let file = self.path.join(&[
            install_path,
            "config",
            "avatarcache",
            &format!("{steam_id64}.png"),
        ]);
        avatar_data_uri(self.fs.as_ref(), &file)
    }
}
