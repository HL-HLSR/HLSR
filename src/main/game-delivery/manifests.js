"use strict";

const ghostingRenewed = require("../../../config/games/ghosting-renewed.manifest.json");
const ghostingRenewed100 = require("../../../config/games/ghosting-renewed-1.0.0.files.json");
const ghostingRenewed101 = require("../../../config/games/ghosting-renewed-1.0.1.files.json");
const ghostingRenewed102 = require("../../../config/games/ghosting-renewed-1.0.2.files.json");
const ghostingRenewed103 = require("../../../config/games/ghosting-renewed-1.0.3.files.json");
const ghostingRenewed104 = require("../../../config/games/ghosting-renewed-1.0.4.files.json");
const ghostingRenewed105 = require("../../../config/games/ghosting-renewed-1.0.5.files.json");
const ghostingRenewed106 = require("../../../config/games/ghosting-renewed-1.0.6.files.json");
const ghostingRenewed107 = require("../../../config/games/ghosting-renewed-1.0.7.files.json");
const ghostingRenewed108 = require("../../../config/games/ghosting-renewed-1.0.8.files.json");
const ghostingRenewed109 = require("../../../config/games/ghosting-renewed-1.0.9.files.json");
const ghostingRenewed1010 = require("../../../config/games/ghosting-renewed-1.0.10.files.json");
const ghostingRenewed1011 = require("../../../config/games/ghosting-renewed-1.0.11.files.json");
const ghostingRenewed1012 = require("../../../config/games/ghosting-renewed-1.0.12.files.json");
const ghostingRenewed1013 = require("../../../config/games/ghosting-renewed-1.0.13.files.json");

const hl24260 = require("../../../config/games/hl2-4260.manifest.json");
const hl24260Files = require("../../../config/games/hl2-4260-1.0.0.files.json");
const hl22707 = require("../../../config/games/hl2-2707.manifest.json");
const hl22707Files = require("../../../config/games/hl2-2707-1.0.0.files.json");
const hl1Won = require("../../../config/games/hl1-won.manifest.json");
const hl1WonFiles = require("../../../config/games/hl1-won-1.0.0.files.json");

const MANIFESTS = {
  "ghosting-renewed": ghostingRenewed,
  "hl2-4260": hl24260,
  "hl2-2707": hl22707,
  "hl1-won": hl1Won,
};

const FILES_MANIFESTS = {
  "ghosting-renewed": {
    "1.0.0": ghostingRenewed100,
    "1.0.1": ghostingRenewed101,
    "1.0.2": ghostingRenewed102,
    "1.0.3": ghostingRenewed103,
    "1.0.4": ghostingRenewed104,
    "1.0.5": ghostingRenewed105,
    "1.0.6": ghostingRenewed106,
    "1.0.7": ghostingRenewed107,
    "1.0.8": ghostingRenewed108,
    "1.0.9": ghostingRenewed109,
    "1.0.10": ghostingRenewed1010,
    "1.0.11": ghostingRenewed1011,
    "1.0.12": ghostingRenewed1012,
    "1.0.13": ghostingRenewed1013,
  },
  "hl2-4260": { "1.0.0": hl24260Files },
  "hl2-2707": { "1.0.0": hl22707Files },
  "hl1-won": { "1.0.0": hl1WonFiles },
};

function filesManifestOf(gameId, version) {
  const byVersion = FILES_MANIFESTS[gameId];
  if (!byVersion || version == null) return null;
  return byVersion[String(version)] || null;
}

function filesManifestVersions(gameId) {
  return Object.keys(FILES_MANIFESTS[gameId] || {}).sort();
}

const BY_LAUNCHER_ID = {
  70: "hl1-won",
  218: "ghosting-renewed",
  220: "hl2-4260",
};

const BUILDS_BY_LAUNCHER_ID = {
  220: [
    { build: "4260", gameId: "hl2-4260" },
    { build: "2707", gameId: "hl2-2707" },
    { build: "5288", gameId: "hl2-5288", delivery: "steam-depot" },
  ],
};

function buildGameIdOf(launcherId, build) {
  const builds = BUILDS_BY_LAUNCHER_ID[String(launcherId)] || [];
  const found = builds.find((item) => item.build === String(build));

  return found ? found.gameId : null;
}

function buildsOf(launcherId) {
  return (BUILDS_BY_LAUNCHER_ID[String(launcherId)] || []).map((item) => item.build);
}

const DEPOT_BUILDS = new Set(
  Object.keys(BUILDS_BY_LAUNCHER_ID)
    .map((key) => BUILDS_BY_LAUNCHER_ID[key])
    .reduce((all, list) => all.concat(list), [])
    .filter((item) => item.delivery === "steam-depot")
    .map((item) => item.gameId)
);

function isDepotBuild(gameId) {
  return DEPOT_BUILDS.has(String(gameId));
}

// Игры, у которых работают «Проверить файлы» и «Восстановить игру» по файлам
// (меню, окно «Почему недоступно» и сам сервис доставки).
const FILE_REPAIR_GAMES = new Set(["ghosting-renewed"]);

function isFileRepairGame(gameId) {
  return FILE_REPAIR_GAMES.has(String(gameId));
}

const LOCAL_CONFIG = {
  "ghosting-renewed": {
    screenshotsPath: "ghostingmod/screenshots",
    runsPath: "ghostingmod/runs",
    // Демо лежат в ghostingmod и ghostingmod/demos, без вложенных папок.
    demoDirs: ["ghostingmod", "ghostingmod/demos"],
    demoExtension: ".dem",
    configPath: "ghostingmod/cfg",
    executables: ["hl2.exe"],
    settingsSection: "games",
    shortcutName: "HLSR - Ghosting Mod Renewed",
  },
  "hl2-4260": {
    screenshotsPath: "hl2/screenshots",
    runsPath: null,
    configPath: "hl2/cfg",
    executables: ["hl2.exe"],
    settingsSection: "games",
    shortcutName: "HLSR - Half-Life 2 (4260)",
  },
  "hl2-2707": {
    screenshotsPath: "hl2/screenshots",
    runsPath: null,
    configPath: "hl2/cfg",
    executables: ["hl2.exe"],
    settingsSection: "games",
    shortcutName: "HLSR - Half-Life 2 (2707)",
  },
  "hl2-5288": {
    screenshotsPath: "hl2/screenshots",
    runsPath: null,
    configPath: "hl2/cfg",
    executables: ["hl2.exe"],
    settingsSection: "games",
    shortcutName: "HLSR - Half-Life 2 (5288)",
  },
  "hl1-won": {
    screenshotsPath: "Half-Life/valve_WON",
    // Демо (record/stop) движок пишет в папку мода.
    runsPath: "Half-Life/valve_WON",
    runsExtension: ".dem",
    runsLanguageDirs: true,
    configPath: "Half-Life/valve_WON",
    executables: ["hl.exe"],
    settingsSection: "games",
    shortcutName: "HLSR - Half-Life (WON)",
  },
};

function isKnownGame(gameId) {
  const value = String(gameId || "");
  return !!MANIFESTS[value] || isDepotBuild(value);
}

const LAUNCH_CONFIG = {
  "ghosting-renewed": {
    steamAppId: 243730,
    executable: "hl2.exe",
    gameDirectory: "ghostingmod",
    arguments: ["-steam", "-game", "ghostingmod"],
    sourceSdkRequired: false,
    // Движок сам переписывает steam_appid.txt значением из gameinfo.txt мода,
    // с которым запущен: hl2.exe без -game (двойной клик) оставляет "220".
    // Лаунчер возвращает файл байт в байт как в сборке (движок дописывает \0),
    // чтобы проверка целостности его не браковала.
    ownsSteamAppIdFile: true,
    steamAppIdFileText: "243730\n\0",
    // client.dll шлёт карту, FPS и таймер забега для Discord (game-presence).
    presenceBridge: true,
    // Linux: umu отключает lsteamclient, и Steam не видел игру. Через мост
    // к клиенту Steam она видна под AppID 243730, как на Windows.
    steamBridge: true,
    // Windows: переключатель Vulkan (DXVK из сборки игры, vulkan-backend.js).
    vulkan: true,
  },

  "hl2-4260": {
    steamAppId: 220,
    requiresOwnedAppIds: [220],
    executable: "hl2.exe",
    gameDirectory: "hl2",
    arguments: ["-steam", "-game", "hl2"],
    sourceSdkRequired: false,
    ownsSteamAppIdFile: true,
  },
  "hl2-2707": {
    steamAppId: 220,
    requiresOwnedAppIds: [220],
    executable: "hl2.exe",
    gameDirectory: "hl2",
    arguments: ["-steam", "-game", "hl2"],
    sourceSdkRequired: false,
    ownsSteamAppIdFile: true,
  },
  "hl2-5288": {
    steamAppId: 220,
    requiresOwnedAppIds: [220],
    executable: "hl2.exe",
    gameDirectory: "hl2",
    arguments: ["-steam", "-game", "hl2"],
    sourceSdkRequired: false,
    ownsSteamAppIdFile: true,
  },

  // GoldSrc: вместо gameinfo.txt мод описывает liblist.gam, а steam_appid.txt
  // лежит рядом с hl.exe. Игра стартует только через инжектор Bunnymod XT.
  // Старому steam.dll нужен настоящий мост к Steam, поэтому на Linux игра
  // идёт через Proton в режиме Steam, а не через umu.
  "hl1-won": {
    steamAppId: 70,
    requiresOwnedAppIds: [70],
    engine: "goldsrc",
    executable: "Half-Life/hl.exe",
    gameDirectory: "Half-Life/valve_WON",
    steamAppIdFile: "Half-Life/steam_appid.txt",
    injector: "Bunnymod XT/Injector.exe",
    rinput: { executable: "RInput/RInput.exe" },
    // На Wayland-драйвере Wine OpenGL идёт через EGL и упирается в частоту
    // композитора (в окне 60 FPS); через XWayland (GLX) лимита нет.
    environment: { BXT_DISABLE_VSYNC: "1", PROTON_ENABLE_WAYLAND: "0", __GL_SYNC_TO_VBLANK: "0" },
    // Готовые HUD и прицелы кладутся в спрайты мода поверх файлов valve.
    packs: { spritesDir: "Half-Life/valve_WON/sprites", baseSpritesDir: "Half-Life/valve/sprites" },
    // Обновляются перед каждым запуском с GitHub, как update.bat сборки.
    dependencies: [
      {
        id: "bxt",
        title: "Bunnymod XT",
        repo: "YaLTeR/BunnymodXT",
        asset: "^(\\d+-)?BunnymodXT-Windows\\.7z$",
        extract: "BunnymodXT.dll",
        target: "Bunnymod XT/BunnymodXT.dll",
        versionFile: "Bunnymod XT/version",
      },
      {
        id: "injector",
        title: "Bunnymod XT Injector",
        repo: "YaLTeR/BunnymodXT-Injector",
        asset: "^Injector\\.exe$",
        target: "Bunnymod XT/Injector.exe",
        versionFile: "Bunnymod XT/.hlsr-injector-version",
        // У этого релиза GitHub не отдаёт SHA-256, а Injector запускается при каждом старте игры.
        // v3.2: тот же Injector, что в сборке игры; Microsoft Defender метит v3.3 как HackTool:Win32/Injector!MTB, v3.2 не трогает.
        pin: { tag: "v3.2", sha256: "c4d576f87dbb0ce46ad36e7015e67b1666f9aff1e834d5f0f7ef12e102b5dad9" },
      },
    ],
    arguments: ["-steam", "-gl", "-32bpp", "-game", "valve_WON", "-noforcemparms"],
    sourceSdkRequired: false,
    ownsSteamAppIdFile: true,
    steamBridge: true,
  },
};

const OWNED_APP_TITLES = {
  70: "Half-Life",
  220: "Half-Life 2",
};

function requiredOwnedApps() {
  const seen = new Map();

  for (const gameId of Object.keys(LAUNCH_CONFIG))
    for (const appId of LAUNCH_CONFIG[gameId].requiresOwnedAppIds || [])
      if (!seen.has(appId))
        seen.set(appId, { appId, title: OWNED_APP_TITLES[appId] || `AppID ${appId}` });

  return Array.from(seen.values());
}

function localConfigOf(gameId) {
  return LOCAL_CONFIG[gameId] || null;
}

function launchConfigOf(gameId) {
  return LAUNCH_CONFIG[gameId] || null;
}

function forLauncherId(launcherId) {
  const gameId = BY_LAUNCHER_ID[String(launcherId)];
  return gameId ? MANIFESTS[gameId] : null;
}

function launcherIdOf(gameId) {
  const value = String(gameId || "");

  const direct = Object.keys(BY_LAUNCHER_ID).find((key) => BY_LAUNCHER_ID[key] === value);
  if (direct) return direct;

  const viaBuild = Object.keys(BUILDS_BY_LAUNCHER_ID).find((key) =>
    BUILDS_BY_LAUNCHER_ID[key].some((item) => item.gameId === value)
  );

  return viaBuild || null;
}

module.exports = {
  MANIFESTS,
  FILES_MANIFESTS,
  BY_LAUNCHER_ID,
  BUILDS_BY_LAUNCHER_ID,
  DEPOT_BUILDS,
  buildGameIdOf,
  buildsOf,
  isDepotBuild,
  isFileRepairGame,
  isKnownGame,
  LOCAL_CONFIG,
  LAUNCH_CONFIG,
  OWNED_APP_TITLES,
  requiredOwnedApps,
  localConfigOf,
  launchConfigOf,
  filesManifestOf,
  filesManifestVersions,
  forLauncherId,
  launcherIdOf,
};
