"use strict";

// Деинсталляция лаунчера (Linux, управляемая установка ~/.local/opt/hlsr).
// Игры НЕ удаляются молча: plan() отдаёт размеры для диалога, а фактическое
// удаление библиотеки происходит только при removeGames === true.

// Тот же капкан, что и при распаковке: внутри Electron патченный `fs`
// видит в resources/app.asar каталог-архив и рекурсивное удаление бьётся
// об виртуальные пути («Часть файлов осталась: .../app.asar/dist/...»).
// Поэтому только original-fs, в Node это тот же объект, что и fs.
const fs = require("../launcher-update/fs-compat");
const os = require("os");
const path = require("path");

const libraryCleanup = require("../linux/library-cleanup");
const { managedCliPath } = require("../linux/DesktopIntegration");

const INSTALL_MARKER = ".hlsr-install.json";
const DESKTOP_MARKER = "X-HLSR-Managed=true";

// Подкаталоги библиотеки, которые создаёт HLSR (game-delivery/paths.js
// LAYOUT, Demos — архивы демок); чужое в корне не трогаем.
const LIBRARY_KNOWN_SUBDIRS = ["Games", "Tools", "Downloads", "Backups", ".hlsr", "Demos"];

function fail(code, message) {
  const error = new Error(message || code);
  error.code = code;
  throw error;
}

function dataHome() {
  return path.isAbsolute(process.env.XDG_DATA_HOME || "")
    ? process.env.XDG_DATA_HOME
    : path.join(os.homedir(), ".local", "share");
}

function refusal(target) {
  const full = path.resolve(target);
  if (full === path.parse(full).root || full === path.resolve(os.homedir())) return true;
  return false;
}

function defaultInstallRoot() {
  return path.join(os.homedir(), ".local", "opt", "hlsr");
}

function assertManagedRoot(root) {
  const full = path.resolve(String(root || ""));
  let stat = null;
  try {
    stat = fs.lstatSync(full);
  } catch (_) {
    fail("NOT_MANAGED", "Управляемая установка HLSR не найдена");
  }
  if (!stat.isDirectory() || stat.isSymbolicLink() || refusal(full))
    fail("NOT_MANAGED", "Управляемая установка HLSR не найдена");
  let marker = null;
  try {
    marker = JSON.parse(fs.readFileSync(path.join(full, INSTALL_MARKER), "utf8"));
  } catch (_) {
    marker = null;
  }
  if (marker && marker.schema === 1) {
    let versions = null;
    try {
      versions = fs.lstatSync(path.join(full, "versions"));
    } catch (_) {
      fail("NOT_MANAGED", "Каталог не похож на установку HLSR");
    }
    if (!versions.isDirectory()) fail("NOT_MANAGED", "Каталог не похож на установку HLSR");
    return full;
  }
  // Осиротевшая установка: маркер уже снесён частичным удалением.
  // Принимаем только дефолтный корень с уцелевшим versions/ — чужое не трогаем.
  if (full === path.resolve(defaultInstallRoot())) {
    try {
      if (fs.lstatSync(path.join(full, "versions")).isDirectory()) return full;
    } catch (_) {
      /* пустой корень — не установка */
    }
  }
  fail("NOT_MANAGED", "Каталог не похож на установку HLSR");
  return full;
}

function treeSize(target) {
  let stat = null;
  try {
    stat = fs.lstatSync(target);
  } catch (_) {
    return 0;
  }
  if (stat.isSymbolicLink()) return 0;
  if (!stat.isDirectory()) return stat.size;
  let total = 0;
  let names = [];
  try {
    names = fs.readdirSync(target);
  } catch (_) {
    return 0;
  }
  for (const name of names) total += treeSize(path.join(target, name));
  return total;
}

function exists(target) {
  try {
    fs.lstatSync(target);
    return true;
  } catch (_) {
    return false;
  }
}

function managedDesktopEntry() {
  const file = path.join(dataHome(), "applications", "hlsr.desktop");
  try {
    if (fs.readFileSync(file, "utf8").includes(DESKTOP_MARKER)) return file;
  } catch (_) {
    /* нет ярлыка или чужой — не трогаем */
  }
  return null;
}

function managedDesktopCopy(home = os.homedir()) {
  const file = path.join(home, "Desktop", "hlsr.desktop");
  try {
    if (fs.readFileSync(file, "utf8").includes(DESKTOP_MARKER)) return file;
  } catch (_) {
    /* нет копии или чужая — не трогаем */
  }
  return null;
}

function managedIcon() {
  return path.join(dataHome(), "icons", "hicolor", "256x256", "apps", "hlsr-appimage.png");
}

function isManagedRoot(root) {
  try {
    assertManagedRoot(root);
    return true;
  } catch (_) {
    return false;
  }
}

function planLauncherUninstall(options = {}) {
  const installRoot = assertManagedRoot(options.installRoot);
  const rawLibrary = String(options.libraryRoot || "").trim();
  if (!rawLibrary) fail("PATH_INVALID", "Путь библиотеки не задан");
  if (!path.isAbsolute(rawLibrary)) fail("PATH_INVALID", `Недопустимый путь библиотеки: ${rawLibrary}`);
  const libraryRoot = path.resolve(rawLibrary);
  if (refusal(libraryRoot)) fail("PATH_INVALID", "Библиотекой не может быть корень диска или домашний каталог");
  const libraryExists = exists(libraryRoot);
  return {
    managed: true,
    installRoot,
    installBytes: treeSize(installRoot),
    libraryRoot,
    libraryExists,
    libraryBytes: libraryExists ? treeSize(libraryRoot) : 0,
    desktopEntry: managedDesktopEntry(),
  };
}

function removeIfManaged(file, result) {
  if (!file) return false;
  try {
    fs.unlinkSync(file);
    result.removed += 1;
    return true;
  } catch (error) {
    if (error.code !== "ENOENT") result.failed.push({ path: file, error: error.code || error.message });
    return false;
  }
}

function gameIdOf(dir) {
  try {
    return JSON.parse(fs.readFileSync(path.join(dir, "installed.json"), "utf8")).gameId;
  } catch (_) {
    return null;
  }
}

// Удаляются только наши подкаталоги, а из Games — только папки игр
// (installed.json этой игры). Tools, Downloads, Backups, Demos — целиком,
// только если корень — папка HLSR (~/Games/HLSR): во вписанной руками
// ~/Games папки с такими именами могут быть чужими. Чужое остаётся
// (result.left), корень — если опустел.
function removeLibrary(libraryRoot, result) {
  result.left = result.left || [];
  // Имя — ровно «HLSR», как его даёт сам HLSR (~/Games/HLSR, withHlsrFolder):
  // установщик Linux маркер библиотеки не пишет, отличить по нему нельзя.
  const own = path.basename(path.resolve(libraryRoot)) === "HLSR" ? LIBRARY_KNOWN_SUBDIRS : ["Games", ".hlsr"];
  for (const name of own) {
    const target = path.join(libraryRoot, name);
    let stat = null;
    try {
      stat = fs.lstatSync(target);
    } catch (_) {
      continue;
    }
    if (name !== "Games" || !stat.isDirectory() || stat.isSymbolicLink()) {
      libraryCleanup.removeTreeNoFollow(target, result);
      continue;
    }
    let games = [];
    try {
      games = fs.readdirSync(target);
    } catch (error) {
      result.failed.push({ path: target, error: error.code || error.message });
    }
    for (const game of games) {
      if (gameIdOf(path.join(target, game)) === game) libraryCleanup.removeTreeNoFollow(path.join(target, game), result);
      else result.left.push(path.join(name, game));
    }
    try { fs.rmdirSync(target); } catch (_) { /* в Games осталось чужое */ }
  }
  // Пустые папки раскладки HLSR создаёт в любой библиотеке сам: их — убрать.
  for (const name of LIBRARY_KNOWN_SUBDIRS) {
    try { fs.rmdirSync(path.join(libraryRoot, name)); } catch (_) { /* не пустая или ссылка */ }
  }
  try {
    result.left.push(...fs.readdirSync(libraryRoot).filter((name) => name !== "Games"));
    if (!fs.readdirSync(libraryRoot).length) fs.rmdirSync(libraryRoot);
  } catch (_) {
    /* корня уже нет */
  }
}

// Каталог данных HLSR (~/.config/hlsr, ~/.local/state/hlsr): своё имя,
// не корень и не домашний каталог, без ссылок на пути.
function ownDataDir(dir) {
  const full = path.resolve(String(dir || ""));
  return path.basename(full) === "hlsr" && !refusal(full) && !libraryCleanup.findReparsePoint(full);
}

function uninstallLauncher(options = {}) {
  const plan = planLauncherUninstall(options);
  const result = { removed: 0, failed: [] };

  // Игры удаляются только по явному желанию из диалога. Полное удаление
  // уносит и данные HLSR (dataDirs): настройки, вход, Proton, журналы.
  let gamesRemoved = false;
  if (options.removeGames === true && plan.libraryExists) {
    removeLibrary(plan.libraryRoot, result);
    gamesRemoved = true;
  }
  if (options.removeGames === true)
    for (const dir of options.dataDirs || []) if (ownDataDir(dir)) libraryCleanup.removeTreeNoFollow(dir, result);

  const desktop = managedDesktopEntry();
  const desktopCopy = managedDesktopCopy(options.home);
  const cliRemoved = removeIfManaged(managedCliPath(plan.installRoot, { home: options.home }), result);
  let desktopRemoved = false;
  if (removeIfManaged(desktop, result)) desktopRemoved = true;
  if (removeIfManaged(desktopCopy, result)) desktopRemoved = true;
  let iconRemoved = false;
  if (desktopRemoved) {
    try {
      fs.unlinkSync(managedIcon());
      result.removed += 1;
      iconRemoved = true;
    } catch (error) {
      if (error.code !== "ENOENT") result.failed.push({ path: managedIcon(), error: error.code || error.message });
    }
  }

  libraryCleanup.removeTreeNoFollow(plan.installRoot, result);

  return {
    status: result.failed.length ? "partial" : "ok",
    failed: result.failed,
    gamesRemoved,
    desktopRemoved,
    iconRemoved,
    cliRemoved,
    libraryRoot: plan.libraryRoot,
    left: result.left || [],
  };
}

module.exports = {
  isManagedRoot,
  planLauncherUninstall,
  uninstallLauncher,
  treeSize,
};
