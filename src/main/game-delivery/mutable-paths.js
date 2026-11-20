"use strict";

const { createMatcher } = require("./path-match");

const USER_DATA = [
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
  // Настройки вкладки «Конфиги» (src/shared/game-config.js).
  "Half-Life/valve_WON/hlsr-config.cfg",
  // Импортированный конфиг игрока (вкладка «Клавиатура»).
  "Half-Life/valve_WON/hlsr-import.cfg",
  // Файлы цепочки exec импортированного конфига (src/shared/cfg-import.js).
  "Half-Life/valve_WON/hlsr-import-*.cfg",
  "Half-Life/valve_WON/custom.hpk",
  "Half-Life/valve_WON/*.bmp",
  "Half-Life/errorlogs",
  "Half-Life/steam_appid.txt",
];

const RUNTIME_STATE = [
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

const ENGINE_CACHES = [
  "ghostingmod/modelsounds.cache",
  "**/soundcache/**",
  "**/sound.cache",
  "**/*.vpk.sound.cache",
  "**/modelsounds.cache",
  "**/scene.cache",
  // Vulkan (DXVK) для Ghosting: кэш шейдеров и журналы, vulkan-backend.js.
  "renderer-cache/**",
  "renderer-logs/**",
];

const LIBRARY_INTERNALS = [
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

// Зависимости, которые лаунчер обновляет перед запуском сам (как update.bat
// сборки HL1): проверка файлов не должна откатывать их к версии из архива.
const UPDATED_DEPENDENCIES = [
  "Bunnymod XT/BunnymodXT.dll",
  "Bunnymod XT/Injector.exe",
  "Bunnymod XT/version",
  "Bunnymod XT/.hlsr-*",
];

// Спрайты мода HL1 — слой кастомизации: перед запуском лаунчер кладёт туда
// выбранные HUD и прицелы, а оригиналы хранит в .hlsr/packs. Рядом — cfg
// с настройками BXT и переназначениями клавиш, которые лаунчер переписывает
// перед запуском. Сюда же — копии включённых модов (custom-packs.js; у Ghosting
// они в ghostingmod/custom, это и так данные игрока): проверка их не судит,
// починка не трогает.
const CUSTOMIZATION = [
  "Half-Life/valve_WON/sprites/**",
  "Half-Life/valve_WON/hlsr-personalization.cfg",
  "Half-Life/valve_WON/hlsr-binds.cfg",
].concat(require("./custom-packs").targets());

// Эпизоды Ghosting (episodes.js) ставятся отдельно в папку игры: проверка их не судит, ремонт не трогает.
// В USER_DATA их нет — при обновлении это 6 ГБ копий; InstallService.parkEpisodes / unparkEpisodes
// откладывают их rename на время подмены сборки и возвращают в новую.
const EPISODE_DIRS = ["episodic", "ep2"];

const MUTABLE_PATTERNS = USER_DATA.concat(
  RUNTIME_STATE,
  ENGINE_CACHES,
  LIBRARY_INTERNALS,
  UPDATED_DEPENDENCIES,
  CUSTOMIZATION,
  EPISODE_DIRS
);

const matchesMutable = createMatcher(MUTABLE_PATTERNS);

module.exports = {
  USER_DATA,
  RUNTIME_STATE,
  ENGINE_CACHES,
  LIBRARY_INTERNALS,
  UPDATED_DEPENDENCIES,
  CUSTOMIZATION,
  EPISODE_DIRS,
  MUTABLE_PATTERNS,
  matchesMutable,
  createMatcher,
};
