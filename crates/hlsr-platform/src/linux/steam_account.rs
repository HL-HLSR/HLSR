//! Активный аккаунт Steam на Linux (`linux/SteamAccount.js`): последний вошедший из
//! `config/loginusers.vdf` (`MostRecent`), без реестра; SteamID64 и ID2 — через `BigInt`,
//! поэтому знак разности с базой сохраняется.

use crate::steam_account::{AccountInfo, STEAM_ID64_BASE, avatar_data_uri, steam_id2_of};
use crate::steam_fs::{RealFs, SteamFs, read_to_string};
use crate::steam_locator::SteamLocating;
use crate::vdf::{self, VdfObject, VdfValue, get_insensitive, get_insensitive_in};
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use std::sync::{Arc, Mutex};

/// `steamId64Of(accountId)` с `BigInt`: база плюс номер (номер бывает отрицательным у
/// SteamID64 ниже базы).
pub fn steam_id64_of(account_id: i128) -> String {
    (i128::from(STEAM_ID64_BASE) + account_id).to_string()
}

/// `/^7656119\d{10}$/`.
fn is_steam_id64(id: &str) -> bool {
    id.len() == 17 && id.starts_with("7656119") && id.bytes().all(|b| b.is_ascii_digit())
}

/// `SteamAccount` (Linux).
pub struct SteamAccount {
    locator: Arc<dyn SteamLocating>,
    fs: Arc<dyn SteamFs>,
    path: NodePath,
    connection: Mutex<String>,
}

impl SteamAccount {
    pub fn new(locator: Arc<dyn SteamLocating>, fs: Option<Arc<dyn SteamFs>>) -> SteamAccount {
        SteamAccount {
            locator,
            fs: fs.unwrap_or_else(|| Arc::new(RealFs)),
            path: NodePath::new(Platform::Linux, "/"),
            connection: Mutex::new("checking".to_owned()),
        }
    }

    /// Состояние связи на сейчас (`this.connection`).
    pub fn connection(&self) -> String {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// `readUsers(root)`: раздел `users` из `loginusers.vdf`; нет файла или он не разобран — пусто.
    pub fn read_users(&self, root: &str) -> VdfObject {
        let file = self.path.join(&[root, "config", "loginusers.vdf"]);
        let Ok(text) = read_to_string(self.fs.as_ref(), &file) else {
            return VdfObject::new();
        };
        let Ok(tree) = vdf::parse(&text) else {
            return VdfObject::new();
        };
        match get_insensitive(&tree, "users") {
            Some(VdfValue::Object(users)) => users.clone(),
            _ => VdfObject::new(),
        }
    }

    /// `avatarOf(root, steamId64)`.
    pub fn avatar_of(&self, root: &str, steam_id64: &str) -> Option<String> {
        let file = self
            .path
            .join(&[root, "config", "avatarcache", &format!("{steam_id64}.png")]);
        avatar_data_uri(self.fs.as_ref(), &file)
    }

    /// `describe()`.
    pub fn describe(&self) -> AccountInfo {
        let connection = self.connection();
        let empty = AccountInfo::empty(&connection);
        let Ok(steam) = self.locator.locate_steam() else {
            return empty;
        };
        let users = self.read_users(&steam.install_path);
        let ids: Vec<&str> = users.keys().filter(|id| is_steam_id64(id)).collect();
        let most_recent = ids.iter().copied().find(|candidate| {
            // `String(getInsensitive(users[candidate], "MostRecent")) === "1"`.
            get_insensitive_in(users.get(candidate), "MostRecent").map(VdfValue::js_string)
                == Some("1".to_owned())
        });
        let Some(id) = most_recent.or_else(|| ids.first().copied()) else {
            return empty;
        };
        let persona_name = get_insensitive_in(users.get(id), "PersonaName")
            .and_then(VdfValue::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let account = id.parse::<i128>().unwrap_or(0) - i128::from(STEAM_ID64_BASE);
        AccountInfo {
            signed_in: connection == "connected",
            steam_id64: Some(id.to_owned()),
            steam_id2: Some(steam_id2_of(account)),
            persona_name,
            avatar_data_uri: self.avatar_of(&steam.install_path, id),
            connection,
        }
    }

    /// `activeAccountId()`: номер аккаунта из SteamID64 активного пользователя.
    pub fn active_account_id(&self) -> Option<i64> {
        let id = self.describe().steam_id64?;
        Some((id.parse::<i128>().ok()? - i128::from(STEAM_ID64_BASE)) as i64)
    }

    /// `refreshConnection()`.
    pub fn refresh_connection(&self) -> String {
        let ok = self
            .locator
            .describe_steam_only()
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        let connection = if ok { "connected" } else { "offline" };
        *self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = connection.to_owned();
        connection.to_owned()
    }
}
