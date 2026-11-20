"use strict";

// Vulkan для HL2 Ghosting: переключатель в «Параметрах запуска» (настройка
// vulkan, у игры в LAUNCH_CONFIG — vulkan: true). Игра рисует через Direct3D 9;
// прокси d3d9.dll лаунчера (scripts/livesplit-overlay/proxy.h) при
// HLSR_D3D9_BACKEND=vulkan передаёт вызовы не системной d3d9.dll, а DXVK из
// backend\dxvk_d3d9.dll рядом с hl2.exe. DXVK приносит сборка Ghosting (с 1.0.13),
// лаунчер его не скачивает. Только Windows: под Proton d3d9 и так идёт через DXVK.

const path = require("path");

const BACKEND = "backend/dxvk_d3d9.dll";
const CONFIG_FILE = "dxvk.conf";
// Журнал и кэш шейдеров DXVK — в папке игры, как у Play Vulkan.cmd из сборки
// DXVK: локальный диск рядом с игрой (не перемещаемый профиль %APPDATA%),
// уходят вместе с игрой, проверка файлов их пропускает (mutable-paths.js).
const LOG_DIR = "renderer-logs";
const CACHE_DIR = "renderer-cache";
// Свои значения пользователя поменяли бы DXVK: Play Vulkan.cmd их тоже стирает.
const CLEARED = ["DXVK_CONFIG", "DXVK_HUD", "DXVK_FILTER_DEVICE_NAME"];
const MODE = "HLSR_D3D9_BACKEND";

function supported(config, platform) {
  return platform === "win32" && !!(config && config.vulkan);
}

// null — у игры нет переключателя (или не Windows), иначе есть ли DXVK в папке игры.
function availability(installPath, config, fs, platform) {
  if (!supported(config, platform)) return null;
  return !!installPath && fs.existsSync(path.join(installPath, BACKEND));
}

function deleteKey(env, name) {
  for (const key of Object.keys(env)) if (key.toLowerCase() === name.toLowerCase()) delete env[key];
}

// Окружение игры. Vulkan — как Play Vulkan.cmd (без его аргументов запуска),
// иначе без HLSR_D3D9_BACKEND: прокси берёт системную d3d9.dll (DirectX 9).
function applyEnvironment(env, installPath, vulkan) {
  deleteKey(env, MODE);
  if (!vulkan) return env;
  for (const name of CLEARED.concat(["DXVK_LOG_PATH", "DXVK_SHADER_CACHE_PATH", "DXVK_CONFIG_FILE", "DXVK_LOG_LEVEL"]))
    deleteKey(env, name);
  env[MODE] = "vulkan";
  env.DXVK_LOG_PATH = path.join(installPath, LOG_DIR);
  env.DXVK_SHADER_CACHE_PATH = path.join(installPath, CACHE_DIR);
  env.DXVK_CONFIG_FILE = path.join(installPath, CONFIG_FILE);
  env.DXVK_LOG_LEVEL = "info";
  return env;
}

module.exports = {
  BACKEND,
  LOG_DIR,
  CACHE_DIR,
  MODE,
  supported,
  availability,
  applyEnvironment,
};
