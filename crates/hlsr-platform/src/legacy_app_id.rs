//! `steam_appid.txt` рядом с `HLSR.exe` (`src/main/index.js:213-240`): Steamworks `hlsr-native`
//! требовал файл с AppID 250820 (SteamVR), 0.5 его не использует. В сборке выпуска на Windows
//! лаунчер переходит в каталог exe и удаляет такой файл; файл с другим содержимым остаётся.

use hlsr_delivery::ports::Logger;
use std::path::Path;

/// AppID, который писал `hlsr-native`.
pub const LEGACY_APP_ID: &str = "250820";

/// Убрать `steam_appid.txt` с AppID 250820 из каталога exe. `true` — файл удалён.
pub fn remove_legacy_steam_app_id(exe_dir: &Path, logger: &dyn Logger) -> bool {
    let file = exe_dir.join("steam_appid.txt");
    if !file.exists() {
        return false;
    }
    let read = hlsr_core::js::fs::read_file(&file).map_err(|error| {
        hlsr_core::js::errno::fs_message(&error, "open", &file.to_string_lossy(), None)
    });
    let result = read.and_then(|bytes| {
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
        let content = hlsr_core::js::trim(text.trim_end_matches('\0'));
        if content == LEGACY_APP_ID {
            std::fs::remove_file(&file).map_err(|error| {
                hlsr_core::js::errno::fs_message(&error, "unlink", &file.to_string_lossy(), None)
            })?;
            logger.info("Removed legacy steam_appid.txt (SteamVR AppID 250820) next to HLSR.exe");
            Ok(true)
        } else {
            logger.warn(
                "steam_appid.txt next to HLSR.exe has unexpected content, leaving it untouched",
            );
            Ok(false)
        }
    });
    result.unwrap_or_else(|error| {
        logger.warn(&format!("Legacy steam_appid.txt migration failed: {error}"));
        false
    })
}
