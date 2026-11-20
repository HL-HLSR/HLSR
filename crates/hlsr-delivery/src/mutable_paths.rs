//! Изменяемые пути установки (`mutable-paths.js`): то, что меняют игрок, игра
//! и сам лаунчер, — проверка файлов и обновление это не трогают.

use crate::custom_packs;
use crate::path_match::Matcher;
use std::sync::LazyLock;

pub const USER_DATA: &[&str] = &[
    "ghostingmod/SAVE",
    "ghostingmod/runs",
    "ghostingmod/screenshots",
    "ghostingmod/cfg",
    "ghostingmod/custom",
    "ghostingmod/synclogs",
    "ghostingmod/DownloadLists",
    "ghostingmod/download",
    "**/*.dem",
    "steam_appid.txt",
    "hl2/SAVE",
    "hl2/screenshots",
    "hl2/cfg",
    "hl2/custom",
    "hl2/download",
    "hl2/DownloadLists",
    "lostcoast/SAVE",
    "lostcoast/screenshots",
    "lostcoast/cfg",
    "lostcoast/custom",
    "lostcoast/download",
    "hl2_russian/SAVE",
    "hl2_russian/cfg",
    "hl2_russian/custom",
    "Half-Life/valve_WON/SAVE",
    "Half-Life/valve_WON/config.cfg",
    "Half-Life/valve_WON/userconfig.cfg",
    // Языковая папка (valve_WON_russian): там config.cfg игры и файл игрока, если игра пишет туда.
    "Half-Life/valve_WON_*/config.cfg",
    "Half-Life/valve_WON_*/userconfig.cfg",
    // Настройки вкладки «Конфиги» (shared/game-config.js).
    "Half-Life/valve_WON/hlsr-config.cfg",
    // Импортированный конфиг игрока (вкладка «Клавиатура»).
    "Half-Life/valve_WON/hlsr-import.cfg",
    // Файлы цепочки exec импортированного конфига (shared/cfg-import.js).
    "Half-Life/valve_WON/hlsr-import-*.cfg",
    "Half-Life/valve_WON/custom.hpk",
    "Half-Life/valve_WON/*.bmp",
    "Half-Life/errorlogs",
    "Half-Life/steam_appid.txt",
];

pub const RUNTIME_STATE: &[&str] = &[
    "ghostingmod/demoheader.tmp",
    "ghostingmod/gamestate.txt",
    "ghostingmod/stats.txt",
    "ghostingmod/ep1_gamestats.dat",
    "ghostingmod/voice_ban.dt",
    "**/*.log",
    "**/demoheader.tmp",
    "**/gamestate.txt",
    "**/stats.txt",
    "**/voice_ban.dt",
    "**/NavPlace.db",
    "**/shader_cache.cfg",
    "**/demoheader.dmf",
];

pub const ENGINE_CACHES: &[&str] = &[
    "ghostingmod/modelsounds.cache",
    "**/soundcache/**",
    "**/sound.cache",
    "**/*.vpk.sound.cache",
    "**/modelsounds.cache",
    "**/scene.cache",
    // Vulkan (DXVK) для Ghosting: кэш шейдеров и журналы (vulkan_backend.rs).
    "renderer-cache/**",
    "renderer-logs/**",
];

pub const LIBRARY_INTERNALS: &[&str] = &[
    ".hlsr",
    ".hlsr/**",
    "Downloads",
    "Downloads/**",
    "staging",
    "rollback",
    "installed.json",
    "installed.json.*",
    "*.hlsr-backup",
];

/// Зависимости, которые лаунчер обновляет перед запуском сам (как update.bat
/// сборки HL1): проверка файлов не должна откатывать их к версии из архива.
pub const UPDATED_DEPENDENCIES: &[&str] = &[
    "Bunnymod XT/BunnymodXT.dll",
    "Bunnymod XT/Injector.exe",
    "Bunnymod XT/version",
    "Bunnymod XT/.hlsr-*",
];

/// Спрайты мода HL1 — слой кастомизации: перед запуском лаунчер кладёт туда
/// выбранные HUD и прицелы, а оригиналы хранит в .hlsr/packs. Рядом — cfg
/// с настройками BXT и переназначениями клавиш, которые лаунчер переписывает
/// перед запуском. Сюда же — копии включённых модов (`custom_packs`; у Ghosting
/// они в ghostingmod/custom, это и так данные игрока): проверка их не судит,
/// починка не трогает.
pub static CUSTOMIZATION: LazyLock<Vec<String>> = LazyLock::new(|| {
    [
        "Half-Life/valve_WON/sprites/**",
        "Half-Life/valve_WON/hlsr-personalization.cfg",
        "Half-Life/valve_WON/hlsr-binds.cfg",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(custom_packs::targets())
    .collect()
});

/// Эпизоды Ghosting ставятся отдельно в папку игры: проверка их не судит, ремонт не трогает.
/// В `USER_DATA` их нет — при обновлении это 6 ГБ копий; установка откладывает их rename на
/// время подмены сборки и возвращает в новую.
pub const EPISODE_DIRS: &[&str] = &["episodic", "ep2"];

/// `MUTABLE_PATTERNS`: все списки подряд.
pub static MUTABLE_PATTERNS: LazyLock<Vec<String>> = LazyLock::new(|| {
    [
        USER_DATA,
        RUNTIME_STATE,
        ENGINE_CACHES,
        LIBRARY_INTERNALS,
        UPDATED_DEPENDENCIES,
    ]
    .concat()
    .into_iter()
    .map(str::to_owned)
    .chain(CUSTOMIZATION.iter().cloned())
    .chain(EPISODE_DIRS.iter().map(|dir| (*dir).to_owned()))
    .collect()
});

static MUTABLE: LazyLock<Matcher> = LazyLock::new(|| Matcher::new(&MUTABLE_PATTERNS));

/// `matchesMutable(relativePath)`.
pub fn matches_mutable(relative_path: &str) -> bool {
    MUTABLE.matches(relative_path)
}
