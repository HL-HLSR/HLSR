"use strict";

const nodeFs = require("fs");
const path = require("path");

const cfgSearch = require("./cfg-search");
const gameBinds = require("../../shared/game-binds");
const gameConfig = require("../../shared/game-config");
const cfgDocument = require("../../shared/cfg-document");
const playerConfig = require("./player-config");

// Редактор конфигов на диске: чтение итоговых значений из cfg игрока и
// запись изменений (правила — в src/shared/game-config.js).

const MAX_FILE = 1024 * 1024;

function reader(dirs, fs, encoding) {
  const files = cfgSearch.files(dirs, { fs });
  const cache = new Map();
  const read = (name) => {
    const lower = name.toLowerCase();
    if (cache.has(lower)) return cache.get(lower);
    let text = null;
    if (files.exists(name)) {
      try {
        const file = files.pathOf(name);
        const stat = fs.statSync(file);
        if (stat.isFile() && stat.size <= MAX_FILE) text = fs.readFileSync(file, encoding);
      } catch (error) {
        text = null;
      }
    }
    cache.set(lower, text);
    return text;
  };
  return { read, files };
}

function contextOf(installPath, launchConfig, gameId, engine, fs, options = {}) {
  const catalog = gameConfig.catalogFor(gameId, engine);
  if (!catalog || !launchConfig || !launchConfig.gameDirectory) return null;
  // Языковая папка GoldSrc (valve_WON_russian) ищется первой, как в игре.
  const dirs = cfgSearch.searchDirs(installPath, launchConfig.gameDirectory, engine, "language" in options ? { fs, language: options.language } : { fs });
  const encoding = gameBinds.engineOf(engine).encoding;
  return { catalog, encoding, engine, ...reader(dirs, fs, encoding) };
}

// Переменные, у которых в файле игрока больше одной команды: строка выше перекрыта нижней.
function shadowedOf(text) {
  const counts = new Map();
  for (const item of cfgDocument.commandsOf(cfgDocument.parse(text))) {
    if (item.key && item.key.startsWith("cvar:")) counts.set(item.key, (counts.get(item.key) || 0) + 1);
  }
  return [...counts].filter(([, count]) => count > 1).map(([key]) => key.slice(5));
}

// Текущие значения настроек: { cvar: строка из cfg }, файл, где каждое задано
// последним, отпечаток файла игрока и перекрытые переменные.
function readConfig(installPath, launchConfig, gameId, engine, options = {}) {
  const fs = options.fs || nodeFs;
  const context = contextOf(installPath, launchConfig, gameId, engine, fs, options);
  if (!context) return null;
  const { found, files } = gameConfig.execute(
    context.read,
    gameConfig.ENGINE_ROOTS[engine],
    context.catalog.fields.map((field) => field.cvar)
  );
  const values = {};
  const sources = {};
  for (const field of context.catalog.fields) {
    const hit = found[field.cvar.toLowerCase()];
    if (!hit) continue;
    values[field.cvar] = hit.value;
    sources[field.cvar] = hit.file;
  }
  const current = playerConfig.read(installPath, gameId, launchConfig.gameDirectory, { ...options, fs });
  return { values, sources, files, sha256: current.sha256, playerFile: path.basename(current.file), shadowed: shadowedOf(current.text) };
}

class ConfigError extends Error {
  constructor(code, cvar) {
    super(`${code}${cvar ? `: ${cvar}` : ""}`);
    this.code = code;
    this.cvar = cvar || null;
  }
}

// changes — { cvar: значение в единицах окна }. Всё пишется в файл игрока
// (раздел settings, на месте, если переменная там уже есть): он выполняется
// последним и перекрывает config.cfg и файлы, которые лаунчер не правит.
// options.expectedSha256 — отпечаток файла из readConfig; не совпал — CONFIG_CHANGED.
// shortcut: переменную, которую перебивает чужой exec после её строки, не отслеживаем.
function writeConfig(installPath, launchConfig, gameId, engine, changes, options = {}) {
  const fs = options.fs || nodeFs;
  const context = contextOf(installPath, launchConfig, gameId, engine, fs, options);
  if (!context) throw new ConfigError("UNSUPPORTED");
  if (!changes || typeof changes !== "object" || Array.isArray(changes)) throw new ConfigError("INVALID");

  const { found } = gameConfig.execute(
    context.read,
    gameConfig.ENGINE_ROOTS[engine],
    context.catalog.fields.map((field) => field.cvar)
  );

  const gameDirectory = launchConfig.gameDirectory;
  const current = playerConfig.read(installPath, gameId, gameDirectory, { ...options, fs });
  let doc = cfgDocument.parse(current.text);
  for (const [cvar, value] of Object.entries(changes)) {
    const field = context.catalog.byCvar.get(String(cvar).toLowerCase());
    if (!field) throw new ConfigError("UNKNOWN_SETTING", cvar);
    const hit = found[field.cvar.toLowerCase()];
    const next = gameConfig.toCfg(field, value, hit ? hit.value : undefined);
    if (next === null) throw new ConfigError("INVALID_VALUE", field.cvar);
    if (hit && hit.value === next) continue;
    doc = cfgDocument.set(doc, `cvar:${field.cvar.toLowerCase()}`, next, "settings");
  }

  const text = cfgDocument.serialize(doc);
  if (text === current.text) return { written: [], sha256: current.sha256 };
  const result = playerConfig.write(installPath, gameId, gameDirectory, text, { ...options, fs });
  return { written: [path.basename(result.file)], sha256: result.sha256 };
}

module.exports = { readConfig, writeConfig, ConfigError };
