"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { assertInside } = require("./paths");
const { readableName } = require("../../shared/ansi-name");

const MAX_RESULTS = 1000;
const MAX_ENTRIES_SCANNED = 5000;
const EXTENSION = ".run";

function containedRealpath(root, target) {
  let realRoot;
  let realTarget;

  try {
    realRoot = fs.realpathSync(root);
    realTarget = fs.realpathSync(target);
  } catch (e) {
    return null;
  }

  const prefix = realRoot.endsWith(path.sep) ? realRoot : realRoot + path.sep;
  if (
    realTarget.toLowerCase() !== realRoot.toLowerCase() &&
    !realTarget.toLowerCase().startsWith(prefix.toLowerCase())
  )
    return null;

  return realTarget;
}

function stableId(relative) {
  return crypto.createHash("sha256").update(relative, "utf8").digest("hex").slice(0, 16);
}

// Записи Ghosting — .run, демо Half-Life 1 — .dem (runsExtension в manifests.js).
function extensionOf(extension) {
  return typeof extension === "string" && /^\.[a-z0-9]{1,8}$/.test(extension) ? extension : EXTENSION;
}

function scan(installPath, relativeDir, extension) {
  const wanted = extensionOf(extension);
  if (typeof installPath !== "string" || !installPath)
    return { status: "error", code: "NOT_INSTALLED" };

  let directory;
  try {
    directory = assertInside(
      installPath,
      path.join(installPath, String(relativeDir).replace(/\//g, path.sep)),
      "Каталог записей"
    );
  } catch (error) {
    return { status: "error", code: "PATH_INVALID" };
  }

  let stat;
  try {
    stat = fs.lstatSync(directory);
  } catch (e) {
    return { status: "no-directory", path: directory, items: [] };
  }

  if (stat.isSymbolicLink() || !stat.isDirectory())
    return { status: "error", code: "PATH_INVALID" };

  if (!containedRealpath(installPath, directory))
    return { status: "error", code: "PATH_ESCAPE" };

  let entries;
  try {
    entries = fs.readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    return { status: "error", code: "PATH_NOT_READABLE" };
  }

  const items = [];
  let scanned = 0;
  let truncated = false;

  for (const entry of entries) {
    if (++scanned > MAX_ENTRIES_SCANNED) {
      truncated = true;
      break;
    }

    if (!entry.isFile()) continue;
    if (entry.isSymbolicLink && entry.isSymbolicLink()) continue;
    if (path.extname(entry.name).toLowerCase() !== wanted) continue;

    const full = path.join(directory, entry.name);

    const real = containedRealpath(installPath, full);
    if (!real) continue;

    let fileStat;
    try {
      fileStat = fs.statSync(full);
    } catch (e) {
      continue;
    }
    if (!fileStat.isFile()) continue;

    items.push({
      id: stableId(entry.name),
      name: entry.name,
      // Для показа: имя от игры в UTF-8, записанное через cp1251 (ansi-name.js).
      displayName: readableName(entry.name),
      size: fileStat.size,
      modifiedAt: fileStat.mtime.toISOString(),
    });

    if (items.length >= MAX_RESULTS) {
      truncated = true;
      break;
    }
  }

  items.sort((a, b) => {
    if (a.modifiedAt !== b.modifiedAt) return a.modifiedAt < b.modifiedAt ? 1 : -1;
    return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
  });

  return { status: "ok", path: directory, items, truncated };
}

function resolveItem(installPath, relativeDir, name, extension) {
  if (typeof name !== "string" || !name) return null;
  if (name !== path.basename(name)) return null;
  if (path.extname(name).toLowerCase() !== extensionOf(extension)) return null;

  let target;
  try {
    const directory = assertInside(
      installPath,
      path.join(installPath, String(relativeDir).replace(/\//g, path.sep)),
      "Каталог записей"
    );
    target = assertInside(directory, path.join(directory, name), "Файл записи");
  } catch (error) {
    return null;
  }

  if (!fs.existsSync(target)) return null;
  if (!containedRealpath(installPath, target)) return null;

  return target;
}

function deleteItem(installPath, relativeDir, name, extension) {
  const listed = scan(installPath, relativeDir, extension);
  if (listed.status !== "ok") return { status: "error", code: listed.code || "PATH_INVALID" };
  if (typeof name !== "string" || name !== path.basename(name) || /[\\/]/.test(name) ||
      path.extname(name).toLowerCase() !== extensionOf(extension))
    return { status: "error", code: "PATH_INVALID" };
  const target = path.join(listed.path, name);
  try {
    const stat = fs.lstatSync(target);
    if (!stat.isFile() || stat.isSymbolicLink() || !containedRealpath(listed.path, target))
      return { status: "error", code: "PATH_INVALID" };
    fs.unlinkSync(target);
    return { status: "ok", name };
  } catch (error) {
    return { status: "error", code: error.code === "ENOENT" ? "FILE_NOT_FOUND" : "PATH_NOT_WRITABLE" };
  }
}

// Языки Steam, для которых движок заводит папку «<мод>_<язык>».
const GAME_LANGUAGES = new Set([
  "russian", "ukrainian", "german", "french", "spanish", "latam", "italian", "polish", "portuguese",
  "brazilian", "czech", "hungarian", "turkish", "swedish", "danish", "dutch", "finnish", "norwegian",
  "romanian", "bulgarian", "greek", "thai", "vietnamese", "japanese", "koreana", "schinese", "tchinese",
]);

// Языковые папки мода рядом с основной: GoldSrc с языком игры, отличным от
// английского, пишет демо в «<мод>_<язык>» (valve_WON_russian). Регистр
// в имени не важен: игра под Windows и Proton его не различает.
function languageDirs(installPath, relativeDir) {
  const relative = String(relativeDir).replace(/\\/g, "/");
  const parent = path.posix.dirname(relative);
  const base = path.posix.basename(relative).toLowerCase();
  let entries = [];
  try {
    const directory = assertInside(installPath, path.join(installPath, parent.replace(/\//g, path.sep)), "Каталог записей");
    entries = fs.readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    return [];
  }
  return entries
    .filter((entry) => entry.isDirectory() && entry.name.toLowerCase().startsWith(base + "_") &&
      GAME_LANGUAGES.has(entry.name.slice(base.length + 1).toLowerCase()))
    .map((entry) => (parent === "." ? entry.name : `${parent}/${entry.name}`))
    .sort();
}

// Все каталоги записей: языковые папки, потом основная.
function runDirs(installPath, relativeDir) {
  return languageDirs(installPath, relativeDir).concat([String(relativeDir)]);
}

// Записи из нескольких каталогов одним списком. Одинаковое имя в двух
// каталогах показывается один раз: из первого, где оно нашлось (так же
// их находят показ и удаление).
function scanMany(installPath, relativeDirs, extension) {
  const results = relativeDirs.map((relative) => scan(installPath, relative, extension));
  const ok = results.filter((result) => result.status === "ok");
  if (!ok.length) return results[results.length - 1] || { status: "no-directory", items: [] };
  const items = [];
  const names = new Set();
  for (const result of ok) {
    for (const item of result.items) {
      const key = item.name.toLowerCase();
      if (names.has(key)) continue;
      names.add(key);
      items.push(item);
    }
  }
  items.sort((a, b) => {
    if (a.modifiedAt !== b.modifiedAt) return a.modifiedAt < b.modifiedAt ? 1 : -1;
    return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
  });
  return {
    status: "ok",
    path: ok[0].path,
    items: items.slice(0, MAX_RESULTS),
    truncated: ok.some((result) => result.truncated) || items.length > MAX_RESULTS,
  };
}

module.exports = {
  scan,
  scanMany,
  languageDirs,
  runDirs,
  resolveItem,
  deleteItem,
  MAX_RESULTS,
  MAX_ENTRIES_SCANNED,
  EXTENSION,
};
