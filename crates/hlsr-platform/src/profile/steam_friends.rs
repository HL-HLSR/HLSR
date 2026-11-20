//! `profile/steam-friends.js`: друзья текущего аккаунта из `userdata/<номер>/config/localconfig.vdf`
//! и их аватары из кеша клиента Steam (`config/avatarcache/<SteamID64>.png`). Список — украшение
//! окна: отсутствующий или битый файл Steam не повод показывать ошибку, поэтому ничего не
//! бросается, а на месте списка — пустой массив. Аватары есть только у тех, кого клиент показывал
//! недавно; у остальных интерфейс рисует букву имени.

use crate::process_inspect::node_path;
use crate::steam_account::{MAX_AVATAR_BYTES, steam_id2_of, steam_id64_of};
use crate::steam_fs::{SteamFs, read_to_string};
use crate::vdf::{self, VdfValue, get_insensitive, get_insensitive_in};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js;
use hlsr_core::js::Platform;
use hlsr_core::js::collate::{Collator, Sensitivity};
use hlsr_core::js::object::JsObject;
use hlsr_core::logger::Logger;
use serde_json::{Value, json};

/// Друзей не больше.
pub const MAX_FRIENDS: usize = 500;

/// Служебные ключи раздела `friends`, которые не друзья.
const NOT_A_FRIEND: [&str; 3] = [
    "PersonaName",
    "communitypreferences",
    "contentdescriptorpreferences",
];

/// `localConfigPath(installPath, accountId)`.
pub fn local_config_path(platform: Platform, install_path: &str, account_id: u64) -> String {
    node_path(platform).join(&[
        install_path,
        "userdata",
        &account_id.to_string(),
        "config",
        "localconfig.vdf",
    ])
}

/// Невидимые знаки, которые вырезаются из имён: U+200B–200F, U+2060–206F, U+FE00–FE0F и теги
/// Unicode U+E0000–E007F.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{206F}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0000}'..='\u{E007F}')
}

/// `personaOf(record)`: видимое имя не длиннее 64 единиц UTF-16 или `None`.
fn persona_of(record: &vdf::VdfObject) -> Option<String> {
    let VdfValue::String(name) = get_insensitive(record, "name")? else {
        return None;
    };
    let cleaned: String = name.chars().filter(|c| !is_invisible(*c)).collect();
    let visible = js::trim(&cleaned);
    if visible.is_empty() {
        return None;
    }
    Some(js::slice_utf16(visible, 64))
}

/// Друг: `{steamId64, steamId2, personaName, avatarDataUri}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    pub steam_id64: String,
    pub steam_id2: String,
    pub persona_name: String,
    pub avatar_data_uri: Option<String>,
}

impl Friend {
    pub fn to_value(&self) -> Value {
        json!({
            "steamId64": self.steam_id64,
            "steamId2": self.steam_id2,
            "personaName": self.persona_name,
            "avatarDataUri": self.avatar_data_uri,
        })
    }
}

/// Параметры `list(options)`.
pub struct ListOptions<'a> {
    pub fs: &'a dyn SteamFs,
    pub logger: &'a dyn Logger,
    pub platform: Platform,
    pub install_path: &'a str,
    /// `Number(opts.accountId)`: не целое и не положительное — пустой список.
    pub account_id: f64,
    /// `opts.avatars !== false`.
    pub avatars: bool,
}

/// `^\d{1,10}$`.
fn is_account_key(key: &str) -> bool {
    (1..=10).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_digit())
}

/// Друзья текущего аккаунта, по имени (`localeCompare` для `ru` без учёта регистра и диакритики).
pub fn list(options: &ListOptions<'_>) -> Vec<Friend> {
    let account_id = options.account_id;
    if options.install_path.is_empty()
        || account_id.fract() != 0.0
        || !account_id.is_finite()
        || account_id <= 0.0
    {
        return Vec::new();
    }
    let account_id = account_id as u64;

    let path = local_config_path(options.platform, options.install_path, account_id);
    let Ok(text) = read_to_string(options.fs, &path) else {
        return Vec::new();
    };

    let tree = match vdf::parse(&text) {
        Ok(tree) => tree,
        Err(error) => {
            options.logger.warn(&format!(
                "[friends] localconfig.vdf не разобран: {}",
                error.message
            ));
            return Vec::new();
        }
    };

    // `getInsensitive(tree, "UserLocalConfigStore") || tree`: ложное значение — пустая строка.
    let root = VdfValue::Object(tree.clone());
    let store = match get_insensitive(&tree, "UserLocalConfigStore") {
        Some(value) if !matches!(value, VdfValue::String(text) if text.is_empty()) => value,
        _ => &root,
    };
    let Some(VdfValue::Object(friends)) = get_insensitive_in(Some(store), "friends") else {
        return Vec::new();
    };

    let own = account_id.to_string();
    let mut out: Vec<Friend> = Vec::new();

    for key in friends.keys() {
        if out.len() >= MAX_FRIENDS {
            break;
        }
        if NOT_A_FRIEND.contains(&key) || key == own {
            continue;
        }
        if !is_account_key(key) {
            continue;
        }
        let Some(VdfValue::Object(record)) = friends.get(key) else {
            continue;
        };

        let number: u64 = key.parse().expect("до десяти цифр");
        let steam_id64 = steam_id64_of(number);
        let persona_name = persona_of(record).unwrap_or_else(|| steam_id64.clone());

        out.push(Friend {
            steam_id2: steam_id2_of(i128::from(number)),
            avatar_data_uri: if options.avatars {
                avatar_of(
                    options.fs,
                    options.platform,
                    options.install_path,
                    &steam_id64,
                )
            } else {
                None
            },
            persona_name,
            steam_id64,
        });
    }

    let collator = Collator::new("ru", Sensitivity::Base);
    out.sort_by(|a, b| collator.compare(&a.persona_name, &b.persona_name));
    out
}

/// Аватар из `config/avatarcache/<SteamID64>.png`: PNG (по первым четырём байтам) от 8 байт до
/// 512 КиБ как `data:`-ссылка.
pub fn avatar_of(
    fs: &dyn SteamFs,
    platform: Platform,
    install_path: &str,
    steam_id64: &str,
) -> Option<String> {
    let file = node_path(platform).join(&[
        install_path,
        "config",
        "avatarcache",
        &format!("{steam_id64}.png"),
    ]);
    let stat = fs.stat(&file).ok()?;
    if !stat.is_file || stat.size == 0 || stat.size > MAX_AVATAR_BYTES {
        return None;
    }
    let data = fs.read(&file).ok()?;
    if data.len() < 8 || data[..4] != [0x89, 0x50, 0x4e, 0x47] {
        return None;
    }
    Some(format!("data:image/png;base64,{}", STANDARD.encode(data)))
}

/// `^\d{1,20}$`.
fn is_avatar_key(id: &str) -> bool {
    (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

/// Аватары пачкой из кеша Steam (`avatarsFor`): только запрошенные номера, в интернет не ходит.
/// Между файлами отдаёт управление, чтобы окно успевало обслуживаться. Результат — объект
/// `{SteamID64: data:-ссылка}` только для тех, у кого аватар нашёлся (ключи в порядке
/// `Object.keys`).
pub async fn avatars_for(
    fs: &dyn SteamFs,
    platform: Platform,
    install_path: &str,
    ids: &[String],
) -> JsObject<String> {
    let mut out = JsObject::new();
    if install_path.is_empty() {
        return out;
    }
    for id in ids.iter().take(MAX_FRIENDS) {
        if !is_avatar_key(id) {
            continue;
        }
        tokio::task::yield_now().await;
        if let Some(uri) = avatar_of(fs, platform, install_path, id) {
            out.insert(id.clone(), uri);
        }
    }
    out
}
