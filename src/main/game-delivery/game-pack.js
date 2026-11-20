"use strict";

const nodeFs = require("fs");
const path = require("path");
const { createMatcher } = require("./path-match");
const { ENGINE_CACHES } = require("./mutable-paths");

// Полная сборка в одном архиве .hlsrpack (7z): папка игры целиком — файлы
// сборки, сейвы, cfg, демо — и служебный .hlsr-pack.json в корне с версией
// и настройками лаунчера. Архив переносят на флешку и ставят без загрузки.
//
// Архиву с флешки доверять нельзя: при импорте файлы сборки сверяются с
// подписанным списком версии, изменённые заменяются оригиналами с сервера
// (контент откладывается, setAside), посторонние файлы отбрасываются, а
// исполняемые файлы, не подтверждённые списком, удаляются и там, где
// проверка их не трогает (cfg, сейвы, Bunnymod XT).

const FORMAT = "hlsr-pack";
const FORMAT_VERSION = 1;
const EXTENSION = "hlsrpack";
const META_FILE = ".hlsr-pack.json";
const MAX_META_BYTES = 4 * 1024 * 1024;
const MAX_ENTRIES = 300000;
const MAX_UNCOMPRESSED_BYTES = 64 * 1024 * 1024 * 1024;

// В архив не идут запись об установке (при импорте пишется заново),
// резервные копии лаунчера (в том числе прежние конфиги игрока из импорта
// и перехода — другим игрокам они ни к чему) и кэши движка, которые игра
// пересобирает сама.
const SKIPPED = createMatcher(
  ["installed.json", "installed.json.*", "*.hlsr-backup", "**/.hlsr-update-*", ".hlsr/backup", ".hlsr/cfg-import-backup", ".hlsr/imported-modified", META_FILE]
    .concat(ENGINE_CACHES)
);

// Изменённые файлы сборки из архива (кастомный HUD поверх файлов игры, setAside).
// У Ghosting контент из папок, которые игра ищет через custom/*, ложится в
// ghostingmod/custom/imported: игра читает custom первым и выглядит как раньше.
// Остальное (HL1, другие игры, файлы вне этих папок) — копией в .hlsr/imported-modified
// внутри папки игры: проверка файлов её не судит, в следующий экспорт она не едет.
const IMPORTED_BACKUP = ".hlsr/imported-modified";
const CUSTOM_LAYERS = { "ghosting-renewed": { dir: "ghostingmod/custom/imported", roots: ["ghostingmod/", "hl2/"] } };
// Изменённые программы и библиотеки не принимаются никогда, даже если заголовок
// испорчен: по расширению тоже (Miles и движок грузят и .asi, .flt, .mix, .m3d).
const BINARY_EXTENSIONS = new Set([".dll", ".exe", ".so", ".dylib", ".asi", ".flt", ".mix", ".m3d", ".sys", ".drv", ".ocx"]);

// Скрипты и прочее, что Windows или Wine запустит как программу.
const SCRIPT_EXTENSIONS = new Set([
  ".bat", ".cmd", ".com", ".cpl", ".hta", ".jar", ".js", ".jse", ".lnk", ".msi", ".msp", ".pif",
  ".ps1", ".psm1", ".reg", ".scr", ".sh", ".vbe", ".vbs", ".wsf", ".wsh",
]);

class PackError extends Error {
  constructor(code, message) {
    super(message || code);
    this.code = code;
  }
}

function toPosix(relative) {
  return relative.split(path.sep).join("/");
}

// Файлы папки игры для архива. Симлинки не идут: в архиве они бы указывали
// на чужую машину.
function collect(installPath, options = {}) {
  const fs = options.fs || nodeFs;
  const files = [];
  let bytes = 0;
  const stack = [""];
  while (stack.length) {
    const dir = stack.pop();
    let items;
    try {
      items = fs.readdirSync(path.join(installPath, dir), { withFileTypes: true });
    } catch (error) {
      if (!dir) throw error;
      continue;
    }
    for (const item of items) {
      const relative = dir ? path.join(dir, item.name) : item.name;
      const posix = toPosix(relative);
      if (item.isSymbolicLink() || SKIPPED(posix)) continue;
      if (item.isDirectory()) stack.push(relative);
      else if (item.isFile()) {
        files.push(posix);
        bytes += fs.statSync(path.join(installPath, relative)).size;
      }
      if (files.length > MAX_ENTRIES) throw new PackError("TOO_LARGE");
    }
  }
  return { files: files.sort(), bytes };
}

function meta(input) {
  const installed = input.installed || {};
  return {
    format: FORMAT,
    version: FORMAT_VERSION,
    exportedAt: new Date(input.exportedAt || Date.now()).toISOString(),
    launcherVersion: input.launcherVersion || null,
    game: {
      launcherId: input.launcherId,
      gameId: installed.gameId,
      build: input.build || null,
      version: installed.version,
      publishedAt: installed.publishedAt || null,
      manifestSha256: installed.manifestSha256 || null,
      archiveSha256: installed.archiveSha256 || null,
    },
    files: input.files,
    bytes: input.bytes,
    settings: input.settings || null,
  };
}

const HEX = /^[a-f0-9]{64}$/i;
const VERSION = /^[A-Za-z0-9][A-Za-z0-9._-]{0,31}$/;

function optionalHex(value) {
  return typeof value === "string" && HEX.test(value) ? value.toLowerCase() : null;
}

function optionalDate(value) {
  return typeof value === "string" && value.length <= 40 && !Number.isNaN(Date.parse(value)) ? value : null;
}

// isKnownGame — проверка gameId по реестру; isDepotBuild — сборки из
// Steam-депо целиком не переносятся.
function parseMeta(text, options = {}) {
  if (typeof text !== "string" || Buffer.byteLength(text) > MAX_META_BYTES) throw new PackError("BAD_FORMAT");
  let raw;
  try {
    raw = JSON.parse(text);
  } catch (error) {
    throw new PackError("BAD_FORMAT");
  }
  if (!raw || raw.format !== FORMAT || typeof raw.version !== "number") throw new PackError("BAD_FORMAT");
  if (raw.version > FORMAT_VERSION) throw new PackError("NEWER_FORMAT");
  const game = raw.game || {};
  const gameId = String(game.gameId || "");
  if (!/^[a-z0-9][a-z0-9-]{0,63}$/i.test(gameId) || (options.isKnownGame && !options.isKnownGame(gameId)))
    throw new PackError("UNKNOWN_GAME");
  if (options.isDepotBuild && options.isDepotBuild(gameId)) throw new PackError("UNKNOWN_GAME");
  const version = String(game.version || "");
  if (!VERSION.test(version)) throw new PackError("BAD_FORMAT");
  const settings = raw.settings && typeof raw.settings === "object" && !Array.isArray(raw.settings) ? raw.settings : null;
  return {
    gameId,
    version,
    launcherId: typeof game.launcherId === "string" || typeof game.launcherId === "number" ? String(game.launcherId) : null,
    build: typeof game.build === "string" ? game.build.slice(0, 32) : null,
    publishedAt: optionalDate(game.publishedAt),
    manifestSha256: optionalHex(game.manifestSha256),
    archiveSha256: optionalHex(game.archiveSha256),
    exportedAt: optionalDate(raw.exportedAt),
    files: Number.isInteger(raw.files) && raw.files >= 0 ? raw.files : null,
    bytes: Number.isFinite(raw.bytes) && raw.bytes >= 0 ? raw.bytes : null,
    settings,
  };
}

// PE (Windows), ELF (Linux, Wine) и Mach-O — по первым байтам, а не по
// расширению: движок и Miles грузят как библиотеки и .asi, .flt, .mix.
function isExecutable(file, options = {}) {
  const fs = options.fs || nodeFs;
  if (SCRIPT_EXTENSIONS.has(path.extname(file).toLowerCase())) return true;
  const head = Buffer.alloc(4);
  let read = 0;
  let fd = null;
  try {
    fd = fs.openSync(file, "r");
    read = fs.readSync(fd, head, 0, 4, 0);
  } catch (error) {
    return false;
  } finally {
    if (fd != null) fs.closeSync(fd);
  }
  if (read >= 2 && head[0] === 0x4d && head[1] === 0x5a) return true;
  if (read >= 4 && head.readUInt32BE(0) === 0x7f454c46) return true;
  if (read >= 4 && [0xfeedface, 0xfeedfacf, 0xcefaedfe, 0xcffaedfe, 0xcafebabe].includes(head.readUInt32BE(0)))
    return true;
  return false;
}

function walkFiles(root, fs) {
  const files = [];
  const stack = [""];
  while (stack.length) {
    const dir = stack.pop();
    let items;
    try {
      items = fs.readdirSync(path.join(root, dir), { withFileTypes: true });
    } catch (error) {
      continue;
    }
    for (const item of items) {
      const relative = dir ? path.join(dir, item.name) : item.name;
      if (item.isDirectory()) stack.push(relative);
      else files.push(toPosix(relative));
    }
  }
  return files;
}

function removeInside(root, relative, fs) {
  const full = path.join(root, relative.split("/").join(path.sep));
  const inside = path.relative(root, full);
  if (!inside || inside.startsWith("..") || path.isAbsolute(inside)) return false;
  fs.rmSync(full, { force: true });
  return true;
}

// Чистка распакованного архива после сверки со списком версии:
// extra — посторонние файлы из проверки (IntegrityService.findExtraFiles),
// verified — подтверждённые списком. Исполняемые файлы вне verified
// удаляются; у зависимостей без файла убирается и файл версии, чтобы
// лаунчер скачал их заново перед запуском (goldsrc-dependencies.js).
function sanitize(root, check, options = {}) {
  const fs = options.fs || nodeFs;
  const dropped = [];
  for (const relative of check.extra || []) if (removeInside(root, relative, fs)) dropped.push(relative);

  const verified = new Set((check.valid || []).map((item) => item.toLowerCase()));
  for (const relative of walkFiles(root, fs)) {
    if (verified.has(relative.toLowerCase())) continue;
    if (!isExecutable(path.join(root, relative.split("/").join(path.sep)), { fs })) continue;
    if (removeInside(root, relative, fs)) dropped.push(relative);
  }

  for (const dependency of options.dependencies || []) {
    if (!dependency || !dependency.target || !dependency.versionFile) continue;
    if (fs.existsSync(path.join(root, dependency.target.split("/").join(path.sep)))) continue;
    removeInside(root, dependency.versionFile, fs);
  }

  for (const name of fs.readdirSync(root)) {
    if (name === META_FILE || name === "installed.json" || name.startsWith("installed.json."))
      fs.rmSync(path.join(root, name), { force: true });
  }
  return { dropped: dropped.sort() };
}

// Изменённые файлы сборки (changed — пути из подписанного списка) уходят с
// мест сборки, чтобы туда легли оригиналы: контент — в слой custom или копией в
// IMPORTED_BACKUP, исполняемые удаляются.
function setAside(root, gameId, changed, options = {}) {
  const fs = options.fs || nodeFs;
  const layer = CUSTOM_LAYERS[gameId] || null;
  const result = { custom: [], backup: [], rejected: [] };
  for (const relative of changed) {
    const full = path.join(root, ...relative.split("/"));
    if (!fs.existsSync(full)) continue;
    if (BINARY_EXTENSIONS.has(path.extname(relative).toLowerCase()) || isExecutable(full, { fs })) {
      fs.rmSync(full, { force: true });
      result.rejected.push(relative);
      continue;
    }
    const prefix = layer && layer.roots.find((item) => relative.toLowerCase().startsWith(item));
    const target = prefix ? `${layer.dir}/${relative.slice(prefix.length)}` : `${IMPORTED_BACKUP}/${relative}`;
    const destination = path.join(root, ...target.split("/"));
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.renameSync(full, destination);
    (prefix ? result.custom : result.backup).push(target);
  }
  return result;
}

const customLayerOf = (gameId) => (CUSTOM_LAYERS[gameId] ? CUSTOM_LAYERS[gameId].dir : null);

module.exports = {
  FORMAT,
  FORMAT_VERSION,
  EXTENSION,
  META_FILE,
  MAX_ENTRIES,
  MAX_UNCOMPRESSED_BYTES,
  PackError,
  collect,
  meta,
  parseMeta,
  isExecutable,
  sanitize,
  setAside,
  customLayerOf,
  IMPORTED_BACKUP,
};
