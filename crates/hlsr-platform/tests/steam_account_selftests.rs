//! Самотест `linux-selftest.js` «Steam friends: Linux account API reads the selected user's
//! friends and correct IDs» — часть про аккаунт: номер аккаунта и ID2 пользователя из
//! `loginusers.vdf`, предел размера аватара. (Друзья Steam — задача 3 подпроекта.)

use hlsr_core::js::Platform;
use hlsr_delivery::errors::DeliveryError;
use hlsr_platform::linux::steam_account::SteamAccount;
use hlsr_platform::steam_account::{MAX_AVATAR_BYTES, steam_id2_of, steam_id64_of};
use hlsr_platform::steam_fs::MemoryFs;
use hlsr_platform::steam_locator::{SteamInstall, SteamLocating};
use serde_json::{Value, json};
use std::sync::Arc;

/// `locator: { locateSteam: () => ({ installPath: root }) }`.
struct Stub(String);

impl SteamLocating for Stub {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
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
fn linux_account_reads_the_selected_user_and_correct_ids() {
    let fs = Arc::new(MemoryFs::new(Platform::Linux));
    fs.add_file(
        "/steam/config/loginusers.vdf",
        "\"users\" { \"76561197960265770\" { \"MostRecent\" \"1\" \"PersonaName\" \"Owner\" } }",
    );
    let account = SteamAccount::new(Arc::new(Stub("/steam".into())), Some(fs));

    assert_eq!(account.active_account_id(), Some(42));
    let info = account.describe();
    assert_eq!(info.steam_id64.as_deref(), Some("76561197960265770"));
    assert_eq!(info.steam_id2.as_deref(), Some("STEAM_0:0:21"));
    assert_eq!(info.persona_name.as_deref(), Some("Owner"));
    assert_eq!(MAX_AVATAR_BYTES, 512 * 1024);

    // Друг из `localconfig.vdf` (задача 3): его SteamID64 и ID2.
    assert_eq!(steam_id2_of(123), "STEAM_0:1:61");
    assert_eq!(steam_id64_of(123), "76561197960265851");
}
