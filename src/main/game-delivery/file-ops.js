"use strict";

const fs = require("fs");
const fsp = require("fs/promises");
const path = require("path");
const crypto = require("crypto");

const { DeliveryError } = require("./errors");
const { assertInside } = require("./paths");

const LOCK_CODES = ["EBUSY", "EPERM", "EACCES", "ETXTBSY"];

function isLockError(error) {
  return !!error && LOCK_CODES.indexOf(error.code) !== -1;
}

function wrapLockError(error, relativePath) {
  if (isLockError(error))
    return new DeliveryError(
      "FILE_LOCKED",
      `Файл занят другим процессом: ${relativePath}. Закройте игру и повторите.`,
      { path: relativePath, cause: error.code }
    );

  return null;
}

async function hashFile(file) {
  const hash = crypto.createHash("sha256");
  const handle = await fsp.open(file, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);

  try {
    for (;;) {
      const { bytesRead } = await handle.read(buffer, 0, buffer.length, null);
      if (bytesRead <= 0) break;
      hash.update(buffer.subarray(0, bytesRead));
    }
  } finally {
    await handle.close();
  }

  return hash.digest("hex");
}

async function exists(file) {
  try {
    await fsp.access(file);
    return true;
  } catch (e) {
    return false;
  }
}

function copyFileSync(source, target, relativePath) {
  fs.mkdirSync(path.dirname(target), { recursive: true });

  try {
    fs.copyFileSync(source, target);
  } catch (error) {
    const locked = wrapLockError(error, relativePath || target);
    if (locked) throw locked;
    throw error;
  }
}

// Windows: файл с атрибутом «только чтение» нельзя удалить или заменить
// (rmSync и rename дают EPERM), а copyFile этот атрибут переносит на копию.
function makeWritableSync(file) {
  if (process.platform !== "win32") return;
  try {
    // chmod идёт по ссылке: атрибут цели вне папки игры не трогаем
    if (fs.lstatSync(file).isSymbolicLink()) return;
    fs.chmodSync(file, 0o666);
  } catch (error) {
    // файла нет — нечего снимать
  }
}

function removeFileSync(target, relativePath) {
  try {
    makeWritableSync(target);
    fs.rmSync(target, { force: true });
  } catch (error) {
    const locked = wrapLockError(error, relativePath || target);
    if (locked) throw locked;
    throw error;
  }
}

function resolveInside(root, relativePath, what) {
  const normalized = String(relativePath).replace(/\//g, path.sep);
  return assertInside(root, path.join(root, normalized), what || `Путь "${relativePath}"`);
}

// Linux не запирает исполняемый файл: запущенную игру ищем по командной
// строке процессов текущего пользователя в /proc.
function isGameRunningPosix(installPath, candidates) {
  let processes = [];
  try {
    processes = fs.readdirSync("/proc").filter((name) => /^\d+$/.test(name));
  } catch (error) {
    return { running: false, path: null };
  }

  for (const relative of candidates) {
    const target = path.join(installPath, relative.replace(/\//g, path.sep));
    if (!fs.existsSync(target)) continue;

    for (const pid of processes) {
      try {
        const dir = `/proc/${pid}`;
        if (fs.statSync(dir).uid !== process.getuid()) continue;
        const args = fs.readFileSync(path.join(dir, "cmdline")).toString("utf8").split("\0");
        if (args.some((arg) => arg === target)) return { running: true, path: relative };
      } catch (error) {
        // процесс успел завершиться
      }
    }
  }

  return { running: false, path: null };
}

function isGameRunning(installPath, executables) {
  const candidates = executables && executables.length ? executables : ["hl2.exe"];

  if (process.platform !== "win32") return isGameRunningPosix(installPath, candidates);

  for (const relative of candidates) {
    const target = path.join(installPath, relative.replace(/\//g, path.sep));
    if (!fs.existsSync(target)) continue;

    try {
      const fd = fs.openSync(target, "r+");
      fs.closeSync(fd);
    } catch (error) {
      if (isLockError(error)) return { running: true, path: relative };
    }
  }

  return { running: false, path: null };
}

function pruneEmptyDirs(root, startDir) {
  let current = path.resolve(startDir);
  const stop = path.resolve(root);

  const inside =
    process.platform === "win32"
      ? (value) => value.toLowerCase().startsWith(stop.toLowerCase())
      : (value) => value.startsWith(stop + path.sep);

  while (current !== stop && inside(current)) {
    let items;
    try {
      items = fs.readdirSync(current);
    } catch (e) {
      return;
    }

    if (items.length) return;

    try {
      fs.rmdirSync(current);
    } catch (e) {
      return;
    }

    current = path.dirname(current);
  }
}

module.exports = {
  isLockError,
  wrapLockError,
  hashFile,
  exists,
  copyFileSync,
  removeFileSync,
  makeWritableSync,
  resolveInside,
  isGameRunning,
  pruneEmptyDirs,
};
