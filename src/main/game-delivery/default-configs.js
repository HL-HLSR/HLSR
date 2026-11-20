"use strict";

const nodeFs = require("fs");
const playerConfig = require("./player-config");
const gameBinds = require("../../shared/game-binds");
const bindPresets = require("../../shared/bind-presets");
const hl1Guide = require("../../shared/hl1-guide");
const cfgSearch = require("./cfg-search");
const gameBindsInstall = require("./game-binds-install");

// Файл игрока: userconfig.cfg у HL1, autoexec.cfg у Ghosting и сборок HL2. Единственный путь
// подготовки: переход с 0.4.8 (если нужен), иначе шаблон вместо отсутствующего
// или стокового. Зовётся первым везде, где читают или пишут конфиги, — раньше любого
// ensure: иначе стандартный файл 0.4.8 теряет строки exec до перехода.
// null — у игры нет файла игрока. options: { settings, launcherVersion, fs, language, platform }.
function prepareConfig(gameId, installPath, launchConfig, options = {}) {
  if (gameId === "ghosting-renewed") return playerConfig.prepare(installPath, gameId, "ghostingmod", options);
  if (bindPresets.engineFor(gameId) && launchConfig && launchConfig.gameDirectory)
    return playerConfig.prepare(installPath, gameId, launchConfig.gameDirectory, options);
  return null;
}

// Запуск игры: только подготовка файла игрока и строка auto_alias в game.cfg для
// скриптов гайда. Сам файл игрока на запуске не переписывается.
function prepareForLaunch(gameId, installPath, launchConfig, settings, options = {}) {
  const result = prepareConfig(gameId, installPath, launchConfig, Object.assign({}, options, { settings }));
  const clean = gameBinds.sanitizeOverrides(settings && settings.binds);
  if (result && bindPresets.engineFor(gameId) && bindPresets.needsAutoAlias(gameId, clean)) {
    const engine = bindPresets.engineFor(gameId);
    const fs = options.fs || nodeFs;
    const files = cfgSearch.filesOf(installPath, launchConfig.gameDirectory, engine, "language" in options ? { fs, language: options.language } : { fs });
    gameBindsInstall.ensureExecLine(files.pathOf(hl1Guide.GAME_CFG), fs, hl1Guide.AUTO_ALIAS, gameBinds.engineOf(engine).encoding);
  }
  return result;
}

module.exports = { prepareConfig, prepareForLaunch };
