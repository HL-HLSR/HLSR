"use strict";

const fs = require("fs");
const path = require("path");

const MAX_FILE_BYTES = 2 * 1024 * 1024;
const MAX_ARCHIVES = 3;

const CATEGORIES = [
  "launcher",
  "launcher-update",
  "game-delivery",
  "game-process",
  "steam-preflight",
];

function isCategory(name) {
  return CATEGORIES.indexOf(String(name)) !== -1;
}

function tag(category) {
  if (!isCategory(category)) throw new Error(`Неизвестная категория лога: ${category}`);
  return `[${category}]`;
}

function archiveName(file, index) {
  const dir = path.dirname(file);
  const ext = path.extname(file);
  const base = path.basename(file, ext);
  return path.join(dir, `${base}.${index}${ext}`);
}

function rotate(file, options) {
  const maxArchives = (options && options.maxArchives) || MAX_ARCHIVES;

  try {
    const oldest = archiveName(file, maxArchives);
    if (fs.existsSync(oldest)) fs.unlinkSync(oldest);

    for (let i = maxArchives - 1; i >= 1; i--) {
      const from = archiveName(file, i);
      if (fs.existsSync(from)) fs.renameSync(from, archiveName(file, i + 1));
    }

    if (fs.existsSync(file)) fs.renameSync(file, archiveName(file, 1));
    return true;
  } catch (error) {
    return false;
  }
}

function logFiles(dir, baseName, options) {
  const maxArchives = (options && options.maxArchives) || MAX_ARCHIVES;
  const files = [];
  const main = path.join(dir, baseName);

  if (fs.existsSync(main)) files.push(main);

  for (let i = 1; i <= maxArchives; i++) {
    const archive = archiveName(main, i);
    if (fs.existsSync(archive)) files.push(archive);
  }

  return files;
}

// Вывод umu и Proton последнего запуска каждой игры (Linux, ProtonRunner).
const GAME_LOG_RE = /^proton-[a-z0-9]+(?:-[a-z0-9]+)*\.log$/;

function gameLogFiles(dir) {
  let names = [];
  try { names = fs.readdirSync(dir); } catch (error) { return []; }
  return names.filter((name) => GAME_LOG_RE.test(name)).sort().map((name) => path.join(dir, name));
}

function applyPolicy(log, options) {
  const opts = options || {};
  const logsDir = opts.logsDir;

  log.transports.file.level = opts.level || "info";
  log.transports.file.maxSize = opts.maxFileBytes || MAX_FILE_BYTES;
  log.transports.file.archiveLogFn = (file) => {
    rotate(typeof file === "string" ? file : file.path, opts);
  };

  if (logsDir) {
    log.transports.file.resolvePathFn = (variables) =>
      path.join(logsDir, (variables && variables.fileName) || "main.log");
  }

  return log;
}

module.exports = {
  MAX_FILE_BYTES,
  MAX_ARCHIVES,
  CATEGORIES,
  isCategory,
  tag,
  archiveName,
  rotate,
  logFiles,
  gameLogFiles,
  applyPolicy,
};
