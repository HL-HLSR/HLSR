"use strict";

// Electron's patched fs treats paths ending in *.asar as virtual archives.
// Cleanup must operate on the real files instead.
const fs = require("../launcher-update/fs-compat");
const os = require("os");
const path = require("path");

const CODES = {
  OK: 0,
  USAGE: 2,
  NOTHING_TO_DO: 3,
  REFUSED_PATH: 10,
  REFUSED_MARKER: 11,
  REFUSED_REPARSE: 12,
  REFUSED_RUNNING: 13,
  PARTIAL: 14,
  INTERNAL: 20,
};

const MARKER_SCHEMA = 1;
const MARKER_ID = "hlsr-library";
const MARKER_RELATIVE = path.join(".hlsr", "library.json");

const OWNED_ZONES = [".hlsr", "Games", "Downloads"];
const PROFILE_FILES = ["game-stats.json"];

const WINDOWS_ROOT_RE = /^[A-Za-z]:[\\/]/;
const DANGEROUS_CHARS_RE = /[*?"<>|\0]/;

function lower(value) {
  return String(value || "").toLowerCase();
}

function markerPath(root) {
  return path.join(root, MARKER_RELATIVE);
}

// .hlsr или library.json — ссылка (symlink, junction): такой маркер не
// считается и через неё не пишется — он мог бы прийти из чужой папки.
function markerLinked(root) {
  for (const part of [path.join(root, ".hlsr"), markerPath(root)]) {
    try {
      if (fs.lstatSync(part).isSymbolicLink()) return true;
    } catch (error) {
      return false;
    }
  }
  return false;
}

function writeMarker(root, extra) {
  const file = markerPath(root);
  if (markerLinked(root)) throw new Error(`маркер библиотеки — ссылка: ${file}`);
  const payload = Object.assign(
    {
      schema: MARKER_SCHEMA,
      id: MARKER_ID,
      root: path.resolve(root),
      createdAt: new Date().toISOString(),
    },
    extra || {}
  );

  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(payload, null, 2) + "\n", "utf8");
  return file;
}

function readMarker(root) {
  if (markerLinked(root)) return null;
  try {
    const raw = fs.readFileSync(markerPath(root), "utf8").replace(/^\uFEFF/, "");
    const data = JSON.parse(raw);
    if (!data || typeof data !== "object") return null;
    if (data.id !== MARKER_ID) return null;
    if (data.schema !== MARKER_SCHEMA) return null;
    return data;
  } catch (error) {
    return null;
  }
}

function forbiddenZones(options) {
  const env = (options && options.env) || process.env;
  const home = (options && options.home) || env.USERPROFILE || os.homedir();
  const zones = [];

  const add = (value) => {
    if (!value) return;
    const resolved = path.resolve(String(value));
    if (zones.some((z) => lower(z) === lower(resolved))) return;
    zones.push(resolved);
  };

  add(env.SystemRoot || env.windir);
  add(env.ProgramFiles);
  add(env["ProgramFiles(x86)"]);
  add(env.ProgramW6432);
  add(env.ProgramData);
  add(env.PUBLIC);
  add(env.APPDATA);
  add(env.LOCALAPPDATA);
  add(env.TEMP);
  add(env.TMP);

  if (home) {
    for (const name of [
      "Desktop",
      "Documents",
      "Downloads",
      "Pictures",
      "Videos",
      "Music",
      "OneDrive",
      "AppData",
    ])
      add(path.join(home, name));
  }

  for (const extra of (options && options.extraZones) || []) add(extra);

  return zones;
}

function steamZones(options) {
  const opts = options || {};
  if (opts.steamLibraries) return opts.steamLibraries.map((p) => path.resolve(p));

  try {
    const SteamLocator = require("../game-delivery/SteamLocator");
    const locator = new SteamLocator({});
    const steam = locator.locateSteam();
    return locator.steamLibraries(steam).map((p) => path.resolve(p));
  } catch (error) {
    return [];
  }
}

function isSameOrInside(parent, candidate) {
  const a = lower(path.resolve(parent));
  const b = lower(path.resolve(candidate));
  if (a === b) return true;
  return b.startsWith(a.endsWith(path.sep) ? a : a + path.sep);
}

function pathChain(target) {
  const chain = [];
  let current = path.resolve(target);

  for (;;) {
    chain.unshift(current);
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }

  return chain;
}

function findReparsePoint(target) {
  for (const part of pathChain(target)) {
    let stat;
    try {
      stat = fs.lstatSync(part);
    } catch (error) {
      if (error.code === "ENOENT") continue;
      return part;
    }

    if (stat.isSymbolicLink()) return part;
  }

  return null;
}

function checkLibraryRoot(rawPath, options) {
  const opts = options || {};

  if (typeof rawPath !== "string" || !rawPath.trim())
    return { ok: false, code: CODES.REFUSED_PATH, reason: "путь библиотеки не задан" };

  const raw = rawPath.trim();

  if (DANGEROUS_CHARS_RE.test(raw))
    return { ok: false, code: CODES.REFUSED_PATH, reason: "путь содержит недопустимые символы" };

  if (raw.startsWith("\\\\") || raw.startsWith("//"))
    return { ok: false, code: CODES.REFUSED_PATH, reason: "сетевые и device-пути не поддерживаются" };

  if (raw.split(/[\\/]/).some((part) => part === ".."))
    return { ok: false, code: CODES.REFUSED_PATH, reason: "путь содержит .." };

  if (!WINDOWS_ROOT_RE.test(raw))
    return { ok: false, code: CODES.REFUSED_PATH, reason: "нужен абсолютный путь с буквой диска" };

  const root = path.resolve(raw);

  if (path.dirname(root) === root)
    return { ok: false, code: CODES.REFUSED_PATH, reason: "корень диска библиотекой быть не может" };

  const zones = forbiddenZones(opts).concat(steamZones(opts));

  if (opts.installDir) {
    const installDir = path.resolve(opts.installDir);
    const isOwnedLauncherSubdir =
      lower(path.dirname(installDir)) === lower(root) &&
      lower(path.basename(installDir)) === "launcher";

    if (
      !isOwnedLauncherSubdir &&
      (isSameOrInside(installDir, root) || isSameOrInside(root, installDir))
    )
      return {
        ok: false,
        code: CODES.REFUSED_PATH,
        reason: `путь пересекается с каталогом лаунчера ${installDir}`,
        root,
      };
  }

  for (const zone of zones) {
    if (isSameOrInside(zone, root) || isSameOrInside(root, zone))
      return {
        ok: false,
        code: CODES.REFUSED_PATH,
        reason: `путь пересекается с системным каталогом ${zone}`,
        root,
      };
  }

  if (!fs.existsSync(root))
    return { ok: false, code: CODES.NOTHING_TO_DO, reason: "каталог библиотеки не существует", root };

  const reparse = findReparsePoint(root);
  if (reparse)
    return {
      ok: false,
      code: CODES.REFUSED_REPARSE,
      reason: `в пути есть символьная ссылка или junction: ${reparse}`,
      root,
    };

  let real;
  try {
    real = fs.realpathSync.native(root);
  } catch (error) {
    return { ok: false, code: CODES.REFUSED_PATH, reason: `путь не разрешается: ${error.message}`, root };
  }

  if (lower(real) !== lower(root))
    return {
      ok: false,
      code: CODES.REFUSED_REPARSE,
      reason: `каноничный путь отличается: ${real}`,
      root,
    };

  let stat;
  try {
    stat = fs.statSync(root);
  } catch (error) {
    return { ok: false, code: CODES.REFUSED_PATH, reason: error.message, root };
  }

  if (!stat.isDirectory())
    return { ok: false, code: CODES.REFUSED_PATH, reason: "путь не является каталогом", root };

  const marker = readMarker(root);
  if (!marker)
    return {
      ok: false,
      code: CODES.REFUSED_MARKER,
      reason: `нет маркера ${MARKER_RELATIVE} с ожидаемой схемой и id`,
      root,
    };

  return { ok: true, code: CODES.OK, root, marker };
}

function isAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

function runningGames(root) {
  const stateDir = path.join(root, ".hlsr", "state");
  const running = [];

  let names;
  try {
    names = fs.readdirSync(stateDir);
  } catch (error) {
    return running;
  }

  for (const name of names) {
    if (!name.endsWith(".process.json")) continue;

    let record;
    try {
      record = JSON.parse(fs.readFileSync(path.join(stateDir, name), "utf8"));
    } catch (error) {
      continue;
    }

    if (!record || !record.pid) continue;
    if (["running", "starting", "stopping"].indexOf(record.state) === -1) continue;
    if (!isAlive(Number(record.pid))) continue;

    running.push({ gameId: record.gameId || name.replace(/\.process\.json$/, ""), pid: Number(record.pid) });
  }

  return running;
}

// Маркер владения Windows-установки HLSR (windows-install/layout.js OWNER_MARKER).
const INSTALL_OWNER_MARKER = ".hlsr-install-owner.json";

// Нечитаемый маркер (занят, нет прав): считается чужим, не равен ни одному настоящему.
const UNREADABLE_MARKER = "\0нечитаемый маркер";

// null — маркера нет; при любой другой ошибке чтения — UNREADABLE_MARKER.
function ownerMarkerOf(dir) {
  try {
    return fs.readFileSync(path.join(dir, INSTALL_OWNER_MARKER), "utf8").replace(/[\r\n]+$/, "");
  } catch (error) {
    return error.code === "ENOENT" ? null : UNREADABLE_MARKER;
  }
}

// ownMarker — маркер владения своей установки: каталог с другим маркером
// (чужая установка HLSR) не удаляется никогда, он и его предки остаются в kept.
function removeTreeNoFollow(target, result, ownMarker) {
  const out = result || { removed: 0, failed: [] };
  out.kept = out.kept || [];

  let stat;
  try {
    stat = fs.lstatSync(target);
  } catch (error) {
    if (error.code === "ENOENT") return out;
    out.failed.push({ path: target, error: error.code || error.message });
    return out;
  }

  if (stat.isSymbolicLink()) {
    try {
      fs.unlinkSync(target);
      out.removed += 1;
    } catch (error) {
      try {
        fs.rmdirSync(target);
        out.removed += 1;
      } catch (inner) {
        out.failed.push({ path: target, error: inner.code || inner.message });
      }
    }
    return out;
  }

  if (stat.isDirectory()) {
    const marker = ownerMarkerOf(target);
    if (marker !== null && marker !== ownMarker) {
      out.kept.push(target);
      return out;
    }

    let entries = [];
    try {
      entries = fs.readdirSync(target);
    } catch (error) {
      out.failed.push({ path: target, error: error.code || error.message });
      return out;
    }

    const kept = out.kept.length;
    for (const entry of entries) removeTreeNoFollow(path.join(target, entry), out, ownMarker);
    if (out.kept.length > kept) return out;

    try {
      fs.rmdirSync(target);
      out.removed += 1;
    } catch (error) {
      out.failed.push({ path: target, error: error.code || error.message });
    }

    return out;
  }

  try {
    fs.unlinkSync(target);
    out.removed += 1;
  } catch (error) {
    out.failed.push({ path: target, error: error.code || error.message });
  }

  return out;
}

// Что в корне библиотеки создаёт HLSR (game-delivery/paths.js LAYOUT; Demos —
// архивы демок, game-delivery/index.js). Эти папки полное удаление уносит
// целиком, только если корень — папка HLSR, созданная установщиком (C:\HLSR):
// в другой библиотеке (D:\Games, своя D:\HLSR) папки с такими именами могут
// быть чужими. Там своё — только .hlsr и Games\<id> с installed.json.
const OWNED_ENTRIES = [".hlsr", "Tools", "Downloads", "Backups", "Demos"];
const HLSR_FOLDER = "hlsr";
const GAMES_DIR = "Games";
const GAME_ID_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
// Остатки прерванной замены программы (windows-install/install.js LEFTOVER_RE).
const INSTALL_LEFTOVER_RE = /^\..+\.hlsr-(?:new|old)-[0-9a-f]{8}$/;

function knownGameIds() {
  try {
    return Object.keys(require("../game-delivery/manifests").LAUNCH_CONFIG);
  } catch (error) {
    return [];
  }
}

function isEmptyDir(dir) {
  try {
    const stat = fs.lstatSync(dir);
    return stat.isDirectory() && !stat.isSymbolicLink() && fs.readdirSync(dir).length === 0;
  } catch (error) {
    return false;
  }
}

function installedGameId(dir) {
  try {
    return JSON.parse(fs.readFileSync(path.join(dir, "installed.json"), "utf8")).gameId;
  } catch (error) {
    return null;
  }
}

// Свои записи в корне библиотеки (targets) и чужие (left, имена). В папке
// HLSR папка игры — Games\<известный id> (и недокачанная) или с installed.json
// этой игры, в корне (старая раскладка) — только с installed.json. В другой
// библиотеке — только Games\<id> с installed.json и .hlsr.
function ownedEntries(root, installDir) {
  // Папка HLSR — имя HLSR и маркер «папку создал HLSR»: установщик
  // (windows-install/install.js) или лаунчер (game-delivery/paths.js) ставят
  // его, только если создали её сами. Папку с таким именем мог завести и человек.
  const marker = readMarker(root);
  const hlsrFolder = lower(path.basename(path.resolve(root))) === HLSR_FOLDER && !!marker &&
    (marker.createdBy === "installer" || marker.createdBy === "launcher");
  const known = hlsrFolder ? knownGameIds() : [];
  const ownNames = hlsrFolder ? OWNED_ENTRIES : [".hlsr"];
  const targets = [], left = [];
  let games = GAMES_DIR;
  for (const name of fs.readdirSync(root)) {
    const full = path.join(root, name);
    if (installDir && isSameOrInside(full, installDir)) continue;
    if (lower(name) === lower(GAMES_DIR)) {
      games = name;
      const stat = fs.lstatSync(full);
      // Ссылка или файл на месте Games: удаляется сама ссылка, без перехода.
      if (!stat.isDirectory() || stat.isSymbolicLink()) {
        targets.push(full);
        continue;
      }
      for (const game of fs.readdirSync(full)) {
        const dir = path.join(full, game);
        const own = GAME_ID_RE.test(game) && (known.includes(game) || installedGameId(dir) === game);
        if (own) targets.push(dir);
        else left.push(path.join(name, game));
      }
      continue;
    }
    // Пустые Tools, Downloads, Backups создаёт в любой библиотеке сам HLSR (paths.ensureLayout).
    const own = ownNames.some((entry) => lower(entry) === lower(name)) ||
      (OWNED_ENTRIES.some((entry) => lower(entry) === lower(name)) && isEmptyDir(full)) ||
      (hlsrFolder && (INSTALL_LEFTOVER_RE.test(name) || (GAME_ID_RE.test(name) && installedGameId(full) === name)));
    if (own) targets.push(full);
    else left.push(name);
  }
  return { targets, left, games };
}

function cleanupLibrary(rawPath, options) {
  const opts = options || {};
  const check = checkLibraryRoot(rawPath, opts);

  if (!check.ok) return { ok: false, code: check.code, reason: check.reason, root: check.root || null };

  const running = runningGames(check.root);
  if (running.length)
    return {
      ok: false,
      code: CODES.REFUSED_RUNNING,
      reason: `игра запущена (${running.map((r) => `${r.gameId} PID ${r.pid}`).join(", ")})`,
      root: check.root,
    };

  const result = { removed: 0, failed: [], kept: [] };
  const zones = [];

  const installDir = opts.installDir ? path.resolve(opts.installDir) : null;

  let targets, owned;
  if (opts.purge) {
    try {
      owned = ownedEntries(check.root, installDir);
    } catch (error) {
      return {
        ok: false,
        code: CODES.REFUSED_PATH,
        reason: `каталог библиотеки не читается: ${error.message}`,
        root: check.root,
      };
    }
    targets = owned.targets;
  } else {
    targets = OWNED_ZONES.map((zone) => path.join(check.root, zone));
  }

  for (const target of targets) {
    if (!isSameOrInside(check.root, target) || path.resolve(target) === check.root) continue;
    if (installDir && isSameOrInside(target, installDir)) continue;
    if (!fs.existsSync(target)) continue;

    zones.push(target);
    removeTreeNoFollow(target, result, opts.ownMarker);
  }
  // Games уходит, только если в ней не осталось чужого.
  if (opts.purge) {
    try {
      fs.rmdirSync(path.join(check.root, owned.games));
    } catch (ignored) {
    }
  }

  let rootRemoved = false;
  try {
    if (fs.readdirSync(check.root).length === 0) {
      fs.rmdirSync(check.root);
      rootRemoved = true;
    }
  } catch (ignored) {
  }

  return {
    ok: result.failed.length === 0,
    code: result.failed.length === 0 ? (zones.length ? CODES.OK : CODES.NOTHING_TO_DO) : CODES.PARTIAL,
    root: check.root,
    zones,
    rootRemoved,
    removed: result.removed,
    failed: result.failed,
    // Не своё: чужие файлы и папки и каталоги других установок HLSR.
    left: (owned ? owned.left : []).concat(result.kept.map((dir) => path.relative(check.root, dir))),
  };
}

function settingsTargets(options) {
  const opts = options || {};
  const env = opts.env || process.env;
  const targets = [];

  const add = (value) => {
    if (!value) return;
    const resolved = path.resolve(String(value));
    if (targets.some((t) => lower(t) === lower(resolved))) return;
    targets.push(resolved);
  };

  if (opts.userData) add(opts.userData);
  if (opts.logsDir) add(opts.logsDir);
  if (opts.updaterCache) add(opts.updaterCache);

  const appName = opts.appName || null;
  if (appName) {
    if (env.APPDATA) add(path.join(env.APPDATA, appName));
    if (env.LOCALAPPDATA) {
      add(path.join(env.LOCALAPPDATA, `${appName}-updater`));
    }
  }

  return targets;
}

function cleanupSettings(options) {
  const opts = options || {};
  const targets = settingsTargets(opts);
  const result = { removed: 0, failed: [] };
  const touched = [];

  const zones = forbiddenZones(opts);

  const userData = opts.userData ? path.resolve(String(opts.userData)) : null;
  const installDir = opts.installDir ? path.resolve(String(opts.installDir)) : null;

  for (const target of targets) {
    if (path.dirname(target) === target) continue;
    if (zones.some((zone) => lower(zone) === lower(target))) continue;
    if (zones.some((zone) => isSameOrInside(target, zone))) continue;
    if (
      installDir &&
      (isSameOrInside(installDir, target) || isSameOrInside(target, installDir))
    )
      continue;
    if (findReparsePoint(target)) continue;
    if (!fs.existsSync(target)) continue;

    touched.push(target);

    if (opts.preserveProfile && userData && lower(target) === lower(userData)) {
      let entries = [];
      try {
        entries = fs.readdirSync(target);
      } catch (error) {
        result.failed.push({ path: target, error: error.code || error.message });
        continue;
      }

      for (const entry of entries) {
        if (PROFILE_FILES.some((name) => lower(name) === lower(entry))) {
          const profileFile = path.join(target, entry);
          try {
            const stat = fs.lstatSync(profileFile);
            if (stat.isFile() && !stat.isSymbolicLink()) continue;
          } catch (ignored) {
          }
        }

        removeTreeNoFollow(path.join(target, entry), result);
      }
    } else {
      removeTreeNoFollow(target, result);
    }
  }

  return {
    ok: result.failed.length === 0,
    code: result.failed.length === 0 ? (touched.length ? CODES.OK : CODES.NOTHING_TO_DO) : CODES.PARTIAL,
    targets: touched,
    removed: result.removed,
    failed: result.failed,
  };
}

function cleanupProfile(options) {
  const opts = options || {};
  const result = { removed: 0, failed: [] };

  if (!opts.userData)
    return {
      ok: true,
      code: CODES.NOTHING_TO_DO,
      targets: [],
      removed: 0,
      failed: [],
    };

  const userData = path.resolve(String(opts.userData));
  const installDir = opts.installDir ? path.resolve(String(opts.installDir)) : null;
  const zones = forbiddenZones(opts);

  if (path.dirname(userData) === userData)
    return { ok: false, code: CODES.REFUSED_PATH, targets: [], removed: 0, failed: [] };
  if (zones.some((zone) => lower(zone) === lower(userData)))
    return { ok: false, code: CODES.REFUSED_PATH, targets: [], removed: 0, failed: [] };
  if (zones.some((zone) => isSameOrInside(userData, zone)))
    return { ok: false, code: CODES.REFUSED_PATH, targets: [], removed: 0, failed: [] };
  if (
    installDir &&
    (isSameOrInside(installDir, userData) || isSameOrInside(userData, installDir))
  )
    return { ok: false, code: CODES.REFUSED_PATH, targets: [], removed: 0, failed: [] };
  if (findReparsePoint(userData))
    return { ok: false, code: CODES.REFUSED_REPARSE, targets: [], removed: 0, failed: [] };

  const target = path.join(userData, PROFILE_FILES[0]);
  if (!fs.existsSync(target))
    return {
      ok: true,
      code: CODES.NOTHING_TO_DO,
      targets: [],
      removed: 0,
      failed: [],
    };

  removeTreeNoFollow(target, result);

  return {
    ok: result.failed.length === 0,
    code: result.failed.length === 0 ? CODES.OK : CODES.PARTIAL,
    targets: [target],
    removed: result.removed,
    failed: result.failed,
  };
}

module.exports = {
  CODES,
  MARKER_SCHEMA,
  MARKER_ID,
  MARKER_RELATIVE,
  OWNED_ZONES,
  PROFILE_FILES,
  markerPath,
  writeMarker,
  readMarker,
  forbiddenZones,
  steamZones,
  isSameOrInside,
  pathChain,
  findReparsePoint,
  checkLibraryRoot,
  runningGames,
  removeTreeNoFollow,
  ownedEntries,
  cleanupLibrary,
  settingsTargets,
  cleanupSettings,
  cleanupProfile,
};
