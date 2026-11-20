"use strict";

const fs = require("fs");
const path = require("path");
const { DeliveryError } = require("./errors");
const libraryCleanup = require("../uninstall/library-cleanup");

const LAYOUT = {
  games: "Games",
  tools: "Tools",
  downloads: "Downloads",
  backups: "Backups",
  internal: ".hlsr",
  staging: path.join(".hlsr", "staging"),
  state: path.join(".hlsr", "state"),
  rollback: path.join(".hlsr", "rollback"),
};

// В Windows библиотека обязана лежать на диске с буквой, в Linux — по
// абсолютному пути.
function isAbsoluteLibraryPath(raw, resolved) {
  if (process.platform === "win32") return /^[A-Za-z]:[\\/]/.test(resolved);
  return path.isAbsolute(raw);
}

function resolveLibrary(libraryRoot) {
  if (typeof libraryRoot !== "string" || !libraryRoot.trim())
    throw new DeliveryError("PATH_INVALID", "Путь библиотеки не задан");

  const root = path.resolve(libraryRoot.trim());

  if (!isAbsoluteLibraryPath(libraryRoot.trim(), root))
    throw new DeliveryError("PATH_INVALID", `Недопустимый путь библиотеки: ${libraryRoot}`);

  if (path.dirname(root) === root)
    throw new DeliveryError("PATH_INVALID", "Библиотекой не может быть корень диска");

  return {
    root,
    games: path.join(root, LAYOUT.games),
    tools: path.join(root, LAYOUT.tools),
    downloads: path.join(root, LAYOUT.downloads),
    backups: path.join(root, LAYOUT.backups),
    internal: path.join(root, LAYOUT.internal),
    staging: path.join(root, LAYOUT.staging),
    state: path.join(root, LAYOUT.state),
    rollback: path.join(root, LAYOUT.rollback),
  };
}

function ensureLayout(paths) {
  // Папки не было — её создаёт сам HLSR (в том числе подпапку HLSR из диалога
  // выбора, AppManager withHlsrFolder): полное удаление унесёт её целиком.
  const created = !fs.existsSync(paths.root);
  for (const dir of [
    paths.root,
    paths.games,
    paths.tools,
    paths.downloads,
    paths.backups,
    paths.internal,
    paths.staging,
    paths.state,
    paths.rollback,
  ]) {
    try {
      fs.mkdirSync(dir, { recursive: true });
    } catch (e) {
      throw new DeliveryError("PATH_NOT_WRITABLE", `Не удалось создать ${dir}: ${e.message}`);
    }
  }

  try {
    if (!libraryCleanup.readMarker(paths.root)) libraryCleanup.writeMarker(paths.root, created ? { createdBy: "launcher" } : undefined);
  } catch (ignored) {
  }

  return paths;
}

// Папки библиотеки, которые лаунчер создал сам при выборе пути
// (validateInstallPath создаёт папку для проверки записи). При сохранении
// выбора (set-library-path) такая папка получает отметку «создал лаунчер».
const createdRoots = new Set();
const rootKey = (root) => (process.platform === "win32" ? path.resolve(root).toLowerCase() : path.resolve(root));

function prepareLibraryRoot(target) {
  const existed = fs.existsSync(target);
  fs.mkdirSync(target, { recursive: true });
  if (!existed) createdRoots.add(rootKey(target));
}

function markCreatedLibrary(root) {
  if (!createdRoots.has(rootKey(root))) return false;
  try {
    if (!libraryCleanup.readMarker(root)) libraryCleanup.writeMarker(root, { createdBy: "launcher" });
    return true;
  } catch (ignored) {
    return false;
  }
}

function isInside(root, candidate) {
  const normalizedRoot = path.resolve(root);
  const normalized = path.resolve(candidate);

  if (normalized === normalizedRoot) return true;

  const prefix = normalizedRoot.endsWith(path.sep) ? normalizedRoot : normalizedRoot + path.sep;
  if (process.platform !== "win32") return normalized.startsWith(prefix);
  return normalized.toLowerCase().startsWith(prefix.toLowerCase());
}

function assertInside(root, candidate, what) {
  if (!isInside(root, candidate))
    throw new DeliveryError(
      "PATH_INVALID",
      `${what || "Путь"} выходит за пределы разрешённого каталога: ${candidate}`
    );

  return path.resolve(candidate);
}

// Ближайшая существующая папка на пути к файлу и сам файл должны лежать в root
// и после разворота ссылок (junction, symlink): иначе запись ушла бы наружу.
// Сам файл-ссылка не годится. fsImpl — для тестов.
function assertRealInside(root, full, what, fsImpl) {
  const io = fsImpl || fs;
  const fail = () =>
    new DeliveryError("PATH_INVALID", `${what || "Путь"} ведёт через ссылку за пределы ${root}: ${full}`);
  const exists = (target) => {
    try {
      io.lstatSync(target);
      return true;
    } catch (error) {
      return false;
    }
  };
  const real = (target) => {
    try {
      return io.realpathSync(target);
    } catch (error) {
      throw fail();
    }
  };

  const top = real(root);
  let probe = path.dirname(full);
  while (!exists(probe)) probe = path.dirname(probe);
  if (!isInside(top, real(probe))) throw fail();
  if (exists(full) && (io.lstatSync(full).isSymbolicLink() || !isInside(top, real(full)))) throw fail();
}

function removeWithinZone(zoneRoot, target) {
  const zone = path.resolve(zoneRoot);
  const victim = path.resolve(target);

  if (victim === zone)
    throw new DeliveryError("PATH_INVALID", `Отказ удалять саму зону ${zone}`);

  if (!isInside(zone, victim))
    throw new DeliveryError("PATH_INVALID", `Отказ удалять ${victim}: путь вне ${zone}`);

  if (path.dirname(victim) === victim)
    throw new DeliveryError("PATH_INVALID", "Отказ удалять корень диска");

  fs.rmSync(victim, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}

function gameInstallPath(paths, gameId) {
  if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(String(gameId || "")))
    throw new DeliveryError("PATH_INVALID", `Недопустимый gameId: ${gameId}`);

  return assertInside(paths.games, path.join(paths.games, gameId), "Папка игры");
}

module.exports = {
  LAYOUT,
  resolveLibrary,
  ensureLayout,
  prepareLibraryRoot,
  markCreatedLibrary,
  isInside,
  assertInside,
  assertRealInside,
  removeWithinZone,
  gameInstallPath,
};
