"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");
const paths = require("./paths");
const cfgFiles = require("./cfg-files");
const userConfigs = require("./user-configs");
const { writeFileAtomic } = require("./atomic");

// Экспорт и импорт сборки: настройки игры из лаунчера и cfg игрока в одном
// файле .hlsrbuild (JSON). Файлы сборки не переносятся: при импорте
// лаунчер ставит или обновляет сборку сам, а cfg игрока переживают
// обновление (user-configs.js).

const FORMAT = "hlsr-build";
const FORMAT_VERSION = 1;
const EXTENSION = "hlsrbuild";
const MAX_FILES = 400;
const MAX_TOTAL_BYTES = 16 * 1024 * 1024;
const MAX_TEXT_BYTES = 24 * 1024 * 1024;

class TransferError extends Error {
  constructor(code, message) {
    super(message || code);
    this.code = code;
  }
}

function sha256(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex");
}

// cfg игрока: изменённые или свои, которых нет в сборке. Без files.json
// установленной версии (сборка из Steam) переносятся все cfg.
function collect(installPath, previousFiles, options = {}) {
  const fs = options.fs || nodeFs;
  const { files } = cfgFiles.list(installPath, { fs });
  const known = previousFiles && Array.isArray(previousFiles.files);
  const picked = [];
  let total = 0;
  for (const file of files) {
    if (known && !userConfigs.isUserEdited(installPath, file.path, previousFiles, { fs })) continue;
    let data;
    try {
      data = fs.readFileSync(path.join(installPath, file.path.split("/").join(path.sep)));
    } catch (error) {
      // Файл держит другая программа или нет прав: окно назовёт его.
      throw Object.assign(new TransferError("FILE_LOCKED", `${file.path}: ${error.code || error.message}`), { file: file.path });
    }
    total += data.length;
    if (picked.length >= MAX_FILES || total > MAX_TOTAL_BYTES) throw new TransferError("TOO_LARGE");
    picked.push({ path: file.path, data });
  }
  return picked;
}

function serialize(build) {
  return JSON.stringify(
    {
      format: FORMAT,
      version: FORMAT_VERSION,
      exportedAt: new Date(build.exportedAt || Date.now()).toISOString(),
      launcherVersion: build.launcherVersion || null,
      game: { launcherId: build.launcherId, gameId: build.gameId, build: build.build || null, version: build.version || null },
      settings: build.settings || null,
      files: build.files.map((file) => ({ path: file.path, sha256: sha256(file.data), data: file.data.toString("base64") })),
    },
    null,
    1
  );
}

function validPath(relative) {
  const text = String(relative || "");
  return (
    text.length <= 260 &&
    /\.cfg$/i.test(text) &&
    !text.includes("\\") &&
    !path.isAbsolute(text) &&
    !/^[a-z]:/i.test(text) &&
    text.split("/").every((part) => part && part !== "." && part !== ".." && !/[<>:"|?*]/.test(part) && ![...part].some((ch) => ch.charCodeAt(0) < 32))
  );
}

// isKnownGame — проверка gameId по реестру манифестов лаунчера.
function parse(text, options = {}) {
  if (typeof text !== "string" || Buffer.byteLength(text) > MAX_TEXT_BYTES) throw new TransferError("TOO_LARGE");
  let raw;
  try {
    raw = JSON.parse(text);
  } catch (error) {
    throw new TransferError("BAD_FORMAT");
  }
  if (!raw || raw.format !== FORMAT || typeof raw.version !== "number") throw new TransferError("BAD_FORMAT");
  if (raw.version > FORMAT_VERSION) throw new TransferError("NEWER_FORMAT");
  const game = raw.game || {};
  const gameId = String(game.gameId || "");
  if (!/^[a-z0-9][a-z0-9-]{0,63}$/i.test(gameId) || (options.isKnownGame && !options.isKnownGame(gameId)))
    throw new TransferError("UNKNOWN_GAME");
  if (!Array.isArray(raw.files) || raw.files.length > MAX_FILES) throw new TransferError("BAD_FORMAT");
  const files = [];
  const seen = new Set();
  let total = 0;
  for (const file of raw.files) {
    if (!file || !validPath(file.path) || typeof file.data !== "string") throw new TransferError("BAD_FORMAT");
    const key = file.path.toLowerCase();
    if (seen.has(key)) throw new TransferError("BAD_FORMAT");
    seen.add(key);
    const data = Buffer.from(file.data, "base64");
    if (file.sha256 && sha256(data) !== file.sha256) throw new TransferError("CORRUPT");
    total += data.length;
    if (data.length > cfgFiles.MAX_FILE_BYTES || total > MAX_TOTAL_BYTES) throw new TransferError("TOO_LARGE");
    files.push({ path: file.path, data });
  }
  const settings = raw.settings && typeof raw.settings === "object" && !Array.isArray(raw.settings) ? raw.settings : null;
  return {
    gameId,
    launcherId: typeof game.launcherId === "string" ? game.launcherId : null,
    build: typeof game.build === "string" ? game.build : null,
    version: typeof game.version === "string" ? game.version : null,
    exportedAt: typeof raw.exportedAt === "string" ? raw.exportedAt : null,
    settings,
    files,
  };
}

// Запись не должна уйти через ссылку за пределы папки игры (paths.assertRealInside).
function assertWritable(installPath, full, fs) {
  try {
    paths.assertRealInside(installPath, full, "Конфиг", fs);
  } catch (error) {
    throw new TransferError("PATH_INVALID");
  }
}

function applyFiles(installPath, files, options = {}) {
  const fs = options.fs || nodeFs;
  const fulls = files.map((file) => {
    let full;
    try {
      full = paths.assertInside(installPath, path.join(installPath, file.path.split("/").join(path.sep)), "Конфиг");
    } catch (error) {
      throw new TransferError("PATH_INVALID");
    }
    assertWritable(installPath, full, fs);
    return full;
  });
  files.forEach((file, index) => writeFileAtomic(fulls[index], file.data));
  return files.map((file) => file.path);
}

module.exports = { FORMAT, FORMAT_VERSION, EXTENSION, TransferError, collect, serialize, parse, applyFiles };
