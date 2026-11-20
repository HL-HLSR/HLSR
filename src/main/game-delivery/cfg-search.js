"use strict";

const nodeFs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const { assertInside } = require("./paths");
const gameBinds = require("../../shared/game-binds");
const runsScan = require("./runs-scan");

// Где движок ищет cfg игры. GoldSrc с языком Steam, отличным от
// английского, ставит папку «<мод>_<язык>» (valve_WON_russian) перед папкой
// мода и туда же пишет config.cfg при выходе. Её config.cfg перекрывает
// config.cfg мода, поэтому лаунчер должен читать и править именно его.
//
// Какая папка активна, решает язык клиента Steam (HKCU\Software\Valve\Steam,
// значение Language): с ним игра запускается. Если язык не прочитать (Linux:
// игра в префиксе Proton без клиента Steam), активной считается папка,
// чей config.cfg записан последним: при выходе игра пишет его туда, откуда
// читает.

const STEAM_KEY = "HKCU\\Software\\Valve\\Steam";
const LANGUAGE_TTL_MS = 30000;
let languageCache = null;

function readSteamLanguage() {
  if (process.platform !== "win32") return null;
  if (languageCache && Date.now() - languageCache.at < LANGUAGE_TTL_MS) return languageCache.value;
  let value = null;
  try {
    const output = execFileSync("reg.exe", ["query", STEAM_KEY, "/v", "Language"], {
      encoding: "utf8",
      windowsHide: true,
      timeout: 5000,
    });
    const match = /^\s*Language\s+REG_\w+\s+(\S+)\s*$/im.exec(String(output));
    if (match) value = match[1].toLowerCase();
  } catch (error) {
    value = null;
  }
  languageCache = { at: Date.now(), value };
  return value;
}

function modDirOf(installPath, gameDirectory, engine) {
  const gameDir = assertInside(
    installPath,
    path.join(installPath, String(gameDirectory).replace(/\//g, path.sep)),
    "Папка игры"
  );
  const sub = gameBinds.engineOf(engine).cfgDir;
  return sub ? path.join(gameDir, sub) : gameDir;
}

// Имя на диске без учёта регистра: игра под Windows и Wine его не различает.
function listing(dir, fs) {
  let names = [];
  try {
    names = fs.readdirSync(dir);
  } catch (error) {
    if (error.code !== "ENOENT" && error.code !== "ENOTDIR") throw error;
  }
  return { dir, byLower: new Map(names.map((name) => [name.toLowerCase(), name])) };
}

function configTime(entry, fs) {
  const real = entry.byLower.get("config.cfg");
  if (!real) return -1;
  try {
    return fs.statSync(path.join(entry.dir, real)).mtimeMs;
  } catch (error) {
    return -1;
  }
}

// Папки cfg в порядке поиска движка: языковая (если игра сейчас пишет
// туда), затем папка мода. Новые файлы создаются в последней.
function searchDirs(installPath, gameDirectory, engine, options = {}) {
  const fs = options.fs || nodeFs;
  const modDir = modDirOf(installPath, gameDirectory, engine);
  if (engine !== "goldsrc") return [modDir];

  const language = "language" in options ? options.language : readSteamLanguage();
  if (/^[a-z]+$/.test(language || "")) {
    if (language === "english") return [modDir];
    // Папку игра создаёт при первом выходе с этим языком; пока её нет,
    // движок читает config.cfg из папки мода.
    const wanted = `${path.basename(String(gameDirectory).replace(/\\/g, "/"))}_${language}`.toLowerCase();
    const match = runsScan.languageDirs(installPath, gameDirectory)
      .find((relative) => path.posix.basename(relative).toLowerCase() === wanted);
    return match ? [modDirOf(installPath, match, engine), modDir] : [modDir];
  }

  let best = null;
  let bestTime = configTime(listing(modDir, fs), fs);
  for (const relative of runsScan.languageDirs(installPath, gameDirectory)) {
    const dir = modDirOf(installPath, relative, engine);
    const time = configTime(listing(dir, fs), fs);
    if (time > bestTime) {
      best = dir;
      bestTime = time;
    }
  }
  return best ? [best, modDir] : [modDir];
}

// Файлы в папках поиска: путь, откуда движок выполнит cfg с этим именем,
// а для нового файла — путь в папке мода.
function files(dirs, options = {}) {
  const fs = options.fs || nodeFs;
  const entries = dirs.map((dir) => listing(dir, fs));
  const find = (name) => entries.find((entry) => entry.byLower.has(String(name).toLowerCase()));
  return {
    dirs,
    modDir: dirs[dirs.length - 1],
    exists: (name) => Boolean(find(name)),
    pathOf: (name) => {
      const hit = find(name);
      return hit
        ? path.join(hit.dir, hit.byLower.get(String(name).toLowerCase()))
        : path.join(dirs[dirs.length - 1], name);
    },
  };
}

function filesOf(installPath, gameDirectory, engine, options = {}) {
  return files(searchDirs(installPath, gameDirectory, engine, options), options);
}

module.exports = { modDirOf, searchDirs, files, filesOf };
