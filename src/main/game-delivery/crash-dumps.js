"use strict";

const fs = require("fs");
const path = require("path");

const { isInside } = require("./paths");
const { LAUNCH_CONFIG } = require("./manifests");

const DUMP_EXTENSIONS = [".dmp", ".mdmp"];
const DUMP_SETTLE_MS = 5000;
const DUMP_POLL_MS = 500;

function isDumpFile(name) {
  const ext = path.extname(String(name || "")).toLowerCase();
  return DUMP_EXTENSIONS.indexOf(ext) !== -1;
}

const MOD_DIRECTORIES = Object.keys(LAUNCH_CONFIG)
  .map((gameId) => LAUNCH_CONFIG[gameId].gameDirectory)
  .filter((name) => typeof name === "string" && name.length > 0);

// Папки, где ищется дамп. С gameId — только папка мода этой игры: иначе
// дамп другой игры той же библиотеки выдавался бы за дамп этой.
function dumpDirectories(installPath, env, gameId) {
  const dirs = [];
  const config = gameId ? LAUNCH_CONFIG[gameId] : null;
  const mods = config && config.gameDirectory ? [config.gameDirectory] : MOD_DIRECTORIES;

  if (installPath) {
    const root = path.resolve(installPath);
    for (const modDirectory of mods)
      dirs.push({ id: "game-mod", dir: path.join(root, modDirectory) });
    dirs.push({ id: "game-root", dir: root });
  }

  const source = env || process.env;

  const localAppData = source.LOCALAPPDATA;
  if (localAppData)
    dirs.push({ id: "wer-crashdumps", dir: path.join(localAppData, "CrashDumps") });

  const dataDir = source.XDG_DATA_HOME;
  if (dataDir)
    dirs.push({ id: "wine-crashdumps", dir: path.join(dataDir, "hlsr", "crashdumps") });

  return dirs;
}

const ZONE_RANK = { "game-mod": 0, "game-root": 1, "wer-crashdumps": 2, "wine-crashdumps": 2 };
const SHARED_ZONES = new Set(["wer-crashdumps", "wine-crashdumps"]);

// В CrashDumps Windows кладёт дампы всех программ как «<exe>.<pid>.dmp».
// Дамп игры — только с её exe: дамп LiveSplit, браузера или Discord,
// упавших во время игры, к ней не относится.
function processPrefixOf(gameId) {
  const config = gameId ? LAUNCH_CONFIG[gameId] : null;
  const exe = config && config.executable ? path.basename(String(config.executable).replace(/\\/g, "/")) : "";
  return exe ? exe.toLowerCase() + "." : null;
}

function belongsToGame(zone, name, prefix) {
  return !prefix || !SHARED_ZONES.has(zone) || String(name).toLowerCase().startsWith(prefix);
}

function zoneRank(zone) {
  return ZONE_RANK[zone] == null ? 9 : ZONE_RANK[zone];
}

function snapshotDumps(installPath, env, gameId) {
  const snapshot = new Map();
  const prefix = processPrefixOf(gameId);

  for (const { id, dir } of dumpDirectories(installPath, env, gameId)) {
    let entries = [];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (e) {
      continue;
    }

    for (const entry of entries) {
      if (!entry.isFile() || !isDumpFile(entry.name) || !belongsToGame(id, entry.name, prefix)) continue;

      const full = path.join(dir, entry.name);
      try {
        const stat = fs.statSync(full);
        snapshot.set(full, { size: stat.size, mtimeMs: stat.mtimeMs, zone: id });
      } catch (e) {
        /* файл исчез между readdir и stat */
      }
    }
  }

  return snapshot;
}

function findNewDumps(installPath, snapshot, startedAtMs, env, gameId) {
  const current = snapshotDumps(installPath, env, gameId);
  const found = [];

  for (const [file, info] of current) {
    const before = snapshot ? snapshot.get(file) : null;
    const isNew = !before;
    const isChanged = before && (before.size !== info.size || before.mtimeMs !== info.mtimeMs);

    if (!isNew && !isChanged) continue;
    if (startedAtMs && info.mtimeMs < startedAtMs - 2000) continue;

    found.push({ path: file, size: info.size, mtimeMs: info.mtimeMs, zone: info.zone });
  }

  found.sort((a, b) => zoneRank(a.zone) - zoneRank(b.zone) || b.mtimeMs - a.mtimeMs);
  return found;
}

/**
 * Игра под Proton при падении часто выходит с кодом 0: Wine не передаёт код
 * исключения, а Source успевает записать минидамп. Свежий дамп после
 * «штатного» выхода и есть падение.
 */
function leftNewDump(context, env) {
  if (!context || !context.installPath) return false;
  try {
    return findNewDumps(context.installPath, context.snapshot || new Map(), context.startedAtMs || null, env,
      context.gameId || null).length > 0;
  } catch (e) {
    return false;
  }
}

function waitForDump(installPath, snapshot, startedAtMs, options) {
  const opts = options || {};
  const settleMs = opts.settleMs == null ? DUMP_SETTLE_MS : opts.settleMs;
  const pollMs = opts.pollMs == null ? DUMP_POLL_MS : opts.pollMs;
  const env = opts.env;
  const gameId = opts.gameId || null;

  return new Promise((resolve) => {
    const deadline = Date.now() + settleMs;
    let lastFound = [];

    const tick = () => {
      lastFound = findNewDumps(installPath, snapshot, startedAtMs, env, gameId);

      if (lastFound.length && tick.lastSize === lastFound[0].size && lastFound[0].size > 0)
        return resolve(lastFound);

      tick.lastSize = lastFound.length ? lastFound[0].size : null;

      if (Date.now() >= deadline) return resolve(lastFound.filter((d) => d.size > 0));
      setTimeout(tick, pollMs);
    };

    tick();
  });
}

// Архив сбоя, который игра собирает сама: Ghosting пакует ghosting_crash-<время>.txt
// и .dmp в .zip с тем же именем в папке мода. Ищется не по дампу: если дамп
// не записался, в архиве один лог. Самый новый после запуска или null.
const CRASH_ARCHIVE_RE = /_crash-.*\.zip$/i;

function crashArchiveOf(installPath, startedAtMs, gameId, env) {
  let newest = null;
  for (const { id, dir } of dumpDirectories(installPath, env, gameId)) {
    if (id !== "game-mod") continue;
    let names = [];
    try {
      names = fs.readdirSync(dir);
    } catch (e) {
      continue;
    }
    for (const name of names) {
      if (!CRASH_ARCHIVE_RE.test(name)) continue;
      const full = path.join(dir, name);
      try {
        const stat = fs.statSync(full);
        if (!stat.isFile() || !stat.size || (startedAtMs && stat.mtimeMs < startedAtMs - 2000)) continue;
        if (!newest || stat.mtimeMs > newest.mtimeMs) newest = { path: full, mtimeMs: stat.mtimeMs };
      } catch (e) {
        /* файл исчез между readdir и stat */
      }
    }
  }
  return newest ? newest.path : null;
}

function isAllowedDumpPath(candidate, installPath, env) {
  if (typeof candidate !== "string" || !candidate) return false;

  let real;
  try {
    real = fs.realpathSync(candidate);
  } catch (e) {
    return false;
  }

  if (!isDumpFile(real) && !CRASH_ARCHIVE_RE.test(path.basename(real))) return false;

  return dumpDirectories(installPath, env).some(({ dir }) => {
    try {
      return isInside(fs.existsSync(dir) ? fs.realpathSync(dir) : dir, real) && real !== dir;
    } catch (e) {
      return false;
    }
  });
}

// Текстовый лог краша рядом с дампом под тем же именем: Ghosting пишет пару
// ghosting_crash-<время>.txt и .dmp. Его и просим прислать — он читается
// без отладчика. Нет такого файла — null (тогда присылают сам дамп).
function crashLogOf(dumpPath) {
  if (typeof dumpPath !== "string" || !isDumpFile(dumpPath)) return null;
  const log = dumpPath.slice(0, -path.extname(dumpPath).length) + ".txt";
  try {
    return fs.statSync(log).isFile() ? log : null;
  } catch (e) {
    return null;
  }
}

module.exports = {
  DUMP_EXTENSIONS,
  DUMP_SETTLE_MS,
  dumpDirectories,
  snapshotDumps,
  findNewDumps,
  leftNewDump,
  waitForDump,
  isAllowedDumpPath,
  isDumpFile,
  crashLogOf,
  crashArchiveOf,
};
