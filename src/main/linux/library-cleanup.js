"use strict";

// ВАЖНО: только непатченный fs (см. ../launcher-update/fs-compat).
// Внутри Electron обычный `fs` считает любой путь `*.asar` архивом:
// lstat врёт isDirectory() === true, readdir отдаёт виртуальное
// содержимое, а unlink виртуальных путей падает. Рекурсивное удаление
// через патченный fs оставляет настоящий app.asar на диске.
const fs = require("../launcher-update/fs-compat");
const os = require("os");
const path = require("path");

const MARKER = path.join(".hlsr", "library.json");

function markerPath(root) { return path.join(root, MARKER); }

function writeMarker(root) {
  const file = markerPath(root);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify({ schema: 1, id: "hlsr-library", root: path.resolve(root) }), "utf8");
  return file;
}

function readMarker(root) {
  try {
    const value = JSON.parse(fs.readFileSync(markerPath(root), "utf8"));
    return value && value.schema === 1 && value.id === "hlsr-library" &&
      value.root === path.resolve(root) ? value : null;
  } catch (error) { return null; }
}

function findReparsePoint(target) {
  const full = path.resolve(target);
  const parts = full.split(path.sep).filter(Boolean);
  let current = path.parse(full).root;
  for (const part of parts) {
    current = path.join(current, part);
    try { if (fs.lstatSync(current).isSymbolicLink()) return current; }
    catch (error) { return null; }
  }
  return null;
}

function removeTreeNoFollow(target, result) {
  const full = path.resolve(target);
  if (full === path.parse(full).root || full === os.homedir())
    throw new Error("Отказ удалять корневой или домашний каталог");
  let stat;
  try { stat = fs.lstatSync(full); } catch (error) { return result; }
  if (stat.isDirectory() && !stat.isSymbolicLink()) {
    for (const name of fs.readdirSync(full)) removeTreeNoFollow(path.join(full, name), result);
    try { fs.rmdirSync(full); result.removed++; }
    catch (error) { result.failed.push({ path: full, error: error.code || error.message }); }
  } else {
    try { fs.unlinkSync(full); result.removed++; }
    catch (error) { result.failed.push({ path: full, error: error.code || error.message }); }
  }
  return result;
}

module.exports = { markerPath, writeMarker, readMarker, findReparsePoint, removeTreeNoFollow };
