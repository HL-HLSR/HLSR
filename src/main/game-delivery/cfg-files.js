"use strict";

const nodeFs = require("fs");
const path = require("path");
const paths = require("./paths");
const { writeFileAtomic } = require("./atomic");

// Редактор cfg-файлов в настройках игры: все cfg в папке сборки, текст
// файла и его запись. Правленый файл лаунчер потом не трогает: обновление и
// проверка файлов сравнивают его с версией сборки (user-configs.js).

const MAX_FILE_BYTES = 512 * 1024;
const MAX_FILES = 400;
const MAX_DEPTH = 5;
const MAX_ENTRIES = 40000;

// Папки с ресурсами игры: cfg там не бывает, а файлов тысячи.
const SKIP_DIRS = new Set([
  "maps", "models", "materials", "sound", "sounds", "sprites", "gfx", "media",
  "resource", "save", "screenshots", "download", "downloads", "logs", "particles",
  "scripts_compiled", "shaders", "bin", "platform_bin", "steam_api", "__macosx",
]);

class CfgFileError extends Error {
  constructor(code, message) {
    super(message || code);
    this.code = code;
  }
}

function toRelative(root, full) {
  return path.relative(root, full).split(path.sep).join("/");
}

function list(installPath, options = {}) {
  const fs = options.fs || nodeFs;
  const files = [];
  const queue = [{ dir: installPath, depth: 0 }];
  let seen = 0;
  let truncated = false;
  while (queue.length && !truncated) {
    const { dir, depth } = queue.shift();
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (error) {
      continue;
    }
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      if (++seen > MAX_ENTRIES) {
        truncated = true;
        break;
      }
      if (entry.name.startsWith(".")) continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (depth < MAX_DEPTH && !SKIP_DIRS.has(entry.name.toLowerCase())) queue.push({ dir: full, depth: depth + 1 });
        continue;
      }
      if (!entry.isFile() || !/\.cfg$/i.test(entry.name)) continue;
      let stat;
      try {
        stat = fs.statSync(full);
      } catch (error) {
        continue;
      }
      if (stat.size > MAX_FILE_BYTES) continue;
      files.push({ path: toRelative(installPath, full), size: stat.size, mtime: stat.mtimeMs });
      if (files.length >= MAX_FILES) {
        truncated = true;
        break;
      }
    }
  }
  files.sort((a, b) => a.path.localeCompare(b.path, undefined, { sensitivity: "base" }));
  return { files, truncated };
}

// Путь из рендерера: только cfg внутри папки игры, в том числе после
// разворота симлинков (ссылка на файл снаружи не читается и не пишется).
function resolve(installPath, relative, fs) {
  const text = String(relative || "").replace(/\\/g, "/");
  if (!/\.cfg$/i.test(text) || text.split("/").some((part) => !part || part === "." || part === ".."))
    throw new CfgFileError("PATH_INVALID");
  let full;
  try {
    full = paths.assertInside(installPath, path.join(installPath, text.split("/").join(path.sep)), "Конфиг");
  } catch (error) {
    throw new CfgFileError("PATH_INVALID");
  }
  let real;
  try {
    real = fs.realpathSync(full);
  } catch (error) {
    throw new CfgFileError("NOT_FOUND");
  }
  if (!paths.isInside(fs.realpathSync(installPath), real)) throw new CfgFileError("PATH_INVALID");
  return full;
}

// Текст отдаётся как есть. cfg не в UTF-8 (старые файлы в cp1251)
// читаются побайтно как latin1 и так же записываются, чтобы не испортить
// байты, которые редактор не трогал.
function read(installPath, relative, options = {}) {
  const fs = options.fs || nodeFs;
  const full = resolve(installPath, relative, fs);
  let buffer;
  try {
    if (fs.statSync(full).size > MAX_FILE_BYTES) throw new CfgFileError("TOO_LARGE");
    buffer = fs.readFileSync(full);
  } catch (error) {
    if (error instanceof CfgFileError) throw error;
    throw new CfgFileError("NOT_FOUND");
  }
  const utf8 = buffer.toString("utf8");
  const encoding = Buffer.from(utf8, "utf8").equals(buffer) ? "utf8" : "latin1";
  const content = encoding === "utf8" ? utf8 : buffer.toString("latin1");
  return {
    path: toRelative(installPath, full),
    content: content.replace(/\r\n/g, "\n"),
    encoding,
    eol: /\r\n/.test(content) ? "crlf" : "lf",
    mtime: fs.statSync(full).mtimeMs,
  };
}

// Запись только поверх существующего cfg. expectedMtime — время изменения
// файла при чтении: если файл с тех пор поменяли (игра переписала
// config.cfg), запись отклоняется, чтобы не затереть чужие правки молча.
function write(installPath, relative, content, options = {}) {
  const fs = options.fs || nodeFs;
  const full = resolve(installPath, relative, fs);
  let stat;
  try {
    stat = fs.statSync(full);
  } catch (error) {
    throw new CfgFileError("NOT_FOUND");
  }
  if (options.expectedMtime != null && Math.abs(stat.mtimeMs - Number(options.expectedMtime)) > 1)
    throw new CfgFileError("CHANGED");
  const text = String(content == null ? "" : content).replace(/\r\n/g, "\n");
  const withEol = options.eol === "crlf" ? text.replace(/\n/g, "\r\n") : text;
  const encoding = options.encoding === "latin1" ? "latin1" : "utf8";
  const buffer = Buffer.from(withEol, encoding);
  if (buffer.length > MAX_FILE_BYTES) throw new CfgFileError("TOO_LARGE");
  if (encoding === "latin1" && buffer.toString("latin1") !== withEol) throw new CfgFileError("ENCODING");
  writeFileAtomic(full, buffer);
  return { path: toRelative(installPath, full), mtime: fs.statSync(full).mtimeMs };
}

module.exports = { list, read, write, CfgFileError, MAX_FILE_BYTES };
