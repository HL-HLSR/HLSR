"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");

// cfg, который игрок поменял сам или в редакторе конфигов лаунчера,
// обновление, переустановка и проверка файлов не возвращают к версии сборки.
// Изменённым считается cfg, чей SHA-256 не совпадает с прежней версией
// сборки, или cfg, которого в ней не было. Недостающий cfg по-прежнему
// восстанавливается: его игрок не менял.

function isConfigPath(relative) {
  return /\.cfg$/i.test(String(relative || ""));
}

function sha256Of(file, fs) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

// previousFiles — files.json прежней версии (null — неизвестна: тогда
// изменённым не считается ничего, как было раньше). newSha256 — хеш файла в
// новой версии: совпавший с ним файл менять и так незачем.
function isUserEdited(installPath, relative, previousFiles, options = {}) {
  if (!isConfigPath(relative) || !previousFiles || !Array.isArray(previousFiles.files)) return false;
  const fs = options.fs || nodeFs;
  const full = path.join(installPath, String(relative).split("/").join(path.sep));
  let digest;
  try {
    if (!fs.statSync(full).isFile()) return false;
    digest = sha256Of(full, fs);
  } catch (error) {
    // Нечитаемый cfg считаем изменённым: его перенесут (или сбой переноса
    // остановит обновление), а не потеряют молча. Нет файла — не изменён.
    return error.code !== "ENOENT" && error.code !== "ENOTDIR";
  }
  if (options.newSha256 && digest === options.newSha256) return false;
  const previous = previousFiles.files.find((file) => file.path === relative);
  return !previous || previous.sha256 !== digest;
}

// Пути патча (заменяемые и удаляемые), которые остаются как у игрока.
function keptByPatch(installPath, patchManifest, previousFiles, options = {}) {
  const kept = new Set();
  for (const file of patchManifest.files || []) {
    if (isUserEdited(installPath, file.path, previousFiles, Object.assign({}, options, { newSha256: file.sha256 })))
      kept.add(file.path);
  }
  for (const relative of patchManifest.deletedFiles || []) {
    if (isUserEdited(installPath, relative, previousFiles, options)) kept.add(relative);
  }
  return kept;
}

module.exports = { isConfigPath, isUserEdited, keptByPatch };
