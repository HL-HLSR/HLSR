//! Друзья Steam против 0.4.7: сценарии `tools/reference/platform/steam-friends.js` →
//! `steam-friends.json` (настоящая временная папка Steam; список, аватары из кеша клиента,
//! пачка `avatarsFor`, путь к `localconfig.vdf`) и самотест `linux-selftest.js` «Steam friends:
//! Linux account API reads the selected user's friends and correct IDs».

#[path = "support/blobs.rs"]
mod blobs;
mod support;

use blobs::{digest, place};
use hlsr_core::js::{self, Platform};
use hlsr_core::logger::MemoryLogger;
use hlsr_platform::linux::steam_account::SteamAccount;
use hlsr_platform::profile::steam_friends::{
    Friend, ListOptions, MAX_FRIENDS, avatar_of, avatars_for, list, local_config_path,
};
use hlsr_platform::steam_account::MAX_AVATAR_BYTES;
use hlsr_platform::steam_fs::{MemoryFs, RealFs};
use hlsr_platform::steam_locator::{SteamInstall, SteamLocating};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{Mismatches, fixture, items, str_of};

/// `Number(value)` для значения сценария.
fn number_of(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap(),
        Value::String(text) => js::to_number(text),
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Null => 0.0,
        // `{ "undefined": true }`
        _ => f64::NAN,
    }
}

fn friends_value(friends: &[Friend]) -> Value {
    Value::Array(
        friends
            .iter()
            .map(|friend| {
                json!({
                    "steamId64": friend.steam_id64,
                    "steamId2": friend.steam_id2,
                    "personaName": friend.persona_name,
                    "avatarDataUri": digest(friend.avatar_data_uri.as_deref()),
                })
            })
            .collect(),
    )
}

#[test]
fn constants_like_0_4_7() {
    let fixture = fixture("steam-friends.json");
    assert_eq!(fixture["constants"]["MAX_FRIENDS"], json!(MAX_FRIENDS));
}

#[test]
fn local_config_path_like_0_4_7() {
    let fixture = fixture("steam-friends.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["localConfigPath"]) {
        let install = str_of(&case["installPath"]);
        let account = case["accountId"].as_u64().unwrap();
        for (key, platform) in [("win32", Platform::Win32), ("linux", Platform::Linux)] {
            mismatches.check_eq(
                local_config_path(platform, install, account),
                str_of(&case[key]).to_owned(),
                format!("{key}: localConfigPath({install:?}, {account})"),
            );
        }
    }
    mismatches.finish("localConfigPath");
}

#[test]
fn list_like_0_4_7() {
    let fixture = fixture("steam-friends.json");
    let scenarios = items(&fixture["list"]);
    assert!(scenarios.len() >= 25, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        let root = tempfile::tempdir().unwrap();
        place(root.path(), &scenario["files"]);
        let install = root.path().to_string_lossy().into_owned();
        let logger = MemoryLogger::default();
        let result = list(&ListOptions {
            fs: &RealFs,
            logger: &logger,
            platform: Platform::CURRENT,
            install_path: if scenario["noInstallPath"] == json!(true) {
                ""
            } else {
                &install
            },
            account_id: number_of(&scenario["accountId"]),
            avatars: scenario["avatars"] != json!(false),
        });
        mismatches.check(
            &friends_value(&result),
            &scenario["result"],
            format!("«{name}»: список"),
        );
        let warnings: Vec<String> = logger
            .take()
            .into_iter()
            .map(|(level, text)| {
                assert_eq!(level, "warn");
                text
            })
            .collect();
        let expected: Vec<String> = items(&scenario["warnings"])
            .iter()
            .map(|text| str_of(text).to_owned())
            .collect();
        mismatches.check_eq(warnings, expected, format!("«{name}»: журнал"));
    }
    mismatches.finish("steam-friends list");
}

#[test]
fn avatar_of_like_0_4_7() {
    let fixture = fixture("steam-friends.json");
    let scenario = &fixture["avatarsFor"][0];
    let root = tempfile::tempdir().unwrap();
    place(root.path(), &scenario["files"]);
    let install = root.path().to_string_lossy().into_owned();
    let cases = items(&fixture["avatarOf"]);
    assert!(cases.len() >= 10);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let id = str_of(&case["id"]);
        let actual = avatar_of(&RealFs, Platform::CURRENT, &install, id);
        mismatches.check(
            &digest(actual.as_deref()),
            &case["result"],
            format!("avatarOf({id})"),
        );
    }
    mismatches.finish("steam-friends avatarOf");
}

#[tokio::test]
async fn avatars_for_like_0_4_7() {
    let fixture = fixture("steam-friends.json");
    let scenarios = items(&fixture["avatarsFor"]);
    assert!(scenarios.len() >= 9);
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        let root = tempfile::tempdir().unwrap();
        place(root.path(), &scenario["files"]);
        let install = root.path().to_string_lossy().into_owned();
        // Не массив (`null`, строка) для JS — «нечего читать».
        let ids: Vec<String> = scenario["ids"]
            .as_array()
            .map(|ids| ids.iter().map(js::to_string).collect())
            .unwrap_or_default();
        let result = avatars_for(
            &RealFs,
            Platform::CURRENT,
            if scenario["noInstallPath"] == json!(true) {
                ""
            } else {
                &install
            },
            &ids,
        )
        .await;
        let actual: serde_json::Map<String, Value> = result
            .iter()
            .map(|(key, uri)| (key.to_owned(), digest(Some(uri))))
            .collect();
        mismatches.check(
            &Value::Object(actual),
            &scenario["result"],
            format!("«{name}»: результат"),
        );
        let order: Vec<String> = result.keys().map(str::to_owned).collect();
        let expected: Vec<String> = items(&scenario["order"])
            .iter()
            .map(|key| str_of(key).to_owned())
            .collect();
        mismatches.check_eq(order, expected, format!("«{name}»: порядок ключей"));
    }
    mismatches.finish("steam-friends avatarsFor");
}

// --- linux-selftest.js

struct Stub(String);

impl SteamLocating for Stub {
    fn locate_steam(&self) -> Result<SteamInstall, hlsr_delivery::errors::DeliveryError> {
        Ok(SteamInstall {
            install_path: self.0.clone(),
            steam_exe: String::new(),
            steam_args: None,
        })
    }

    fn describe_steam_only(&self) -> Value {
        json!({"ok": true})
    }
}

#[test]
fn linux_account_api_reads_the_selected_users_friends_and_correct_ids() {
    let fs = Arc::new(MemoryFs::new(Platform::Linux));
    fs.add_file(
        "/steam/config/loginusers.vdf",
        "\"users\" { \"76561197960265770\" { \"MostRecent\" \"1\" \"PersonaName\" \"Owner\" } }",
    );
    let config = local_config_path(Platform::Linux, "/steam", 42);
    assert_eq!(config, "/steam/userdata/42/config/localconfig.vdf");
    fs.add_file(
        &config,
        "\"UserLocalConfigStore\" { \"friends\" { \"42\" { \"name\" \"Owner\" } \"123\" { \"name\" \"Friend\" } } }",
    );
    let account = SteamAccount::new(Arc::new(Stub("/steam".into())), Some(fs.clone()));
    assert_eq!(account.active_account_id(), Some(42));
    assert_eq!(
        account.describe().steam_id2.as_deref(),
        Some("STEAM_0:0:21")
    );

    let result = list(&ListOptions {
        fs: fs.as_ref(),
        logger: &MemoryLogger::default(),
        platform: Platform::Linux,
        install_path: "/steam",
        account_id: account.active_account_id().unwrap() as f64,
        avatars: false,
    });
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].persona_name, "Friend");
    assert_eq!(result[0].steam_id64, "76561197960265851");
    assert_eq!(result[0].steam_id2, "STEAM_0:1:61");
    assert_eq!(result[0].avatar_data_uri, None);
    assert_eq!(MAX_AVATAR_BYTES, 512 * 1024);
}

#[test]
fn linux_avatar_cache_is_read_through_the_same_port() {
    let fs = MemoryFs::new(Platform::Linux);
    let mut png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(&[1, 2, 3]);
    fs.add_file("/steam/config/avatarcache/76561197960265851.png", &png);
    let uri = avatar_of(&fs, Platform::Linux, "/steam", "76561197960265851").unwrap();
    assert!(uri.starts_with("data:image/png;base64,iVBORw0KGgoBAgM"));
    assert_eq!(
        avatar_of(&fs, Platform::Linux, "/steam", "76561197960265852"),
        None
    );
}
