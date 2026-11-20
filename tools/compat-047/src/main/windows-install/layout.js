"use strict";

// Раскладка Windows-установки HLSR. Значения совпадают с тем, что писал
// прежний NSIS-установщик electron-builder: новый установщик подхватывает
// уже установленные копии, а обновлятор прежних версий (src/updater)
// запускает HLSR-Setup-<версия>.exe /S --updated /D=<каталог> и ждёт кода 0.
const crypto = require("crypto");
const path = require("path");

// Пространство имён UUID v5, от которого electron-builder (NsisTarget)
// считал GUID установки из appId. Боевой HLSR (com.hlsrteam.hlsr) получает
// 3f824379-6b41-5725-b424-59cb87f2acae — под этим ключом NSIS
// регистрировал установку; другой GUID оставил бы прежнюю запись в
// «Программах и компонентах» рядом с новой.
const ELECTRON_BUILDER_NS = "50e065bc-3134-11e6-9bab-38c9862bdaf3";
const PRODUCTION = { appId: "com.hlsrteam.hlsr", productName: "HLSR" };

const ORIGIN_KEY = "HKCU\\Software\\HLSR";
// Значение, которое NSIS ставил на время незавершённой установки.
const SEED_FLAG = "HLSRSeededInstallLocation";
// Выбор человека по ярлыку на рабочем столе: "1" или "0".
const DESKTOP_VALUE = "HLSRDesktopShortcut";

const LIBRARY_ROOT = "C:\\HLSR";
const EXE = "HLSR.exe";
const PUBLISHER = "HLSR Team";
const OWNER_MARKER = ".hlsr-install-owner.json";
// Запись в «Программах и компонентах» запускает удаление самим HLSR.exe.
const UNINSTALL_FLAG = "--hlsr-uninstall";
// Путь к установщику, из которого запущено удаление: «Назад» снова
// откроет его, когда каталог программы будет удалён.
const RETURN_SETUP_FLAG = "--hlsr-return-setup";

const VERSION_RE = /^\d+\.\d+\.\d+$/;
const INSTALL_ID_RE = /^\{[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}\}$/;
// Символы, которые нельзя безопасно передать в cmd.exe и reg.exe.
const UNSAFE_PATH_RE = /["%^&|<>*?\0\r\n]/;

function guidFor(appId) {
  const hash = crypto.createHash("sha1")
    .update(Buffer.from(ELECTRON_BUILDER_NS.replace(/-/g, ""), "hex"))
    .update(appId, "utf8")
    .digest();
  hash[6] = (hash[6] & 0x0f) | 0x50;
  hash[8] = (hash[8] & 0x3f) | 0x80;
  const hex = hash.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

// Чем установка этой сборки отличается от установки другой: ключи
// реестра, каталог, ярлык. appId и имя продукта берутся из списка файлов
// сборки (file-list.js); у QA-сборки они свои, и она не встаёт поверх
// боевого HLSR.
function identity(app) {
  const appId = (app && app.appId) || PRODUCTION.appId;
  const productName = (app && app.productName) || PRODUCTION.productName;
  if (!/^[A-Za-z0-9][A-Za-z0-9 ._-]*$/.test(productName)) throw new Error(`Недопустимое имя продукта: ${productName}`);
  const guid = guidFor(appId);
  return {
    appId,
    productName,
    guid,
    installKey: `HKCU\\Software\\${guid}`,
    uninstallKey: `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${guid}`,
    defaultInstallDir: `C:\\${productName}\\Launcher`,
    shortcutName: productName,
    ownerMarker: (installId) => `{"schemaVersion":1,"appId":"${guid}","installId":"${installId}"}`,
  };
}

// Аргументы так же, как их понимал NSIS: /S — тихий режим, /D=<каталог> —
// последний аргумент, путь до конца строки (с пробелами).
function parseArgs(argv) {
  const list = (argv || []).slice(1).map(String);
  const args = {
    silent: false,
    updated: false,
    forceRun: false,
    uninstall: false,
    returnSetup: null,
    dir: null,
    deleteGames: false,
    deleteSettings: false,
    deleteProfile: false,
  };

  for (let i = 0; i < list.length; i++) {
    const arg = list[i];
    if (arg.startsWith("/D=")) {
      args.dir = [arg.slice(3)].concat(list.slice(i + 1)).join(" ").trim() || null;
      break;
    }
    const upper = arg.toUpperCase();
    if (upper === "/S") args.silent = true;
    else if (arg === "--updated") args.updated = true;
    else if (arg === "--force-run") args.forceRun = true;
    else if (arg === UNINSTALL_FLAG) args.uninstall = true;
    else if (arg === RETURN_SETUP_FLAG && i + 1 < list.length) args.returnSetup = list[++i];
    else if (upper === "/HLSRDELETEGAMES") args.deleteGames = true;
    else if (upper === "/HLSRDELETESETTINGS") args.deleteSettings = true;
    else if (upper === "/HLSRDELETEPROFILE") args.deleteProfile = true;
  }

  return args;
}

function lower(value) {
  return path.resolve(value).replace(/[\\/]+$/, "").toLowerCase();
}

// Каталоги, в которые HLSR не ставится и которые удаление не трогает.
function protectedDirs(env) {
  const e = env || {};
  const list = [LIBRARY_ROOT];
  const windir = e.WINDIR || e.SystemRoot;
  if (windir) list.push(windir, path.join(windir, "System32"));
  for (const name of ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "USERPROFILE", "TEMP", "TMP", "APPDATA", "LOCALAPPDATA"])
    if (e[name]) list.push(e[name]);
  if (e.USERPROFILE)
    for (const name of ["Desktop", "Documents", "Downloads"]) list.push(path.join(e.USERPROFILE, name));
  return list;
}

// Причина отказа или null. Проверка одна для установки и удаления: в
// каталог, куда нельзя ставить, нельзя и удалять рекурсивно.
function refusedDir(dir, env) {
  if (typeof dir !== "string" || !dir.trim()) return "каталог не задан";
  if (!path.isAbsolute(dir)) return `путь не абсолютный: ${dir}`;
  if (/^[\\/]{2}/.test(dir)) return `сетевой путь: ${dir}`;
  if (dir.split(/[\\/]/).includes("..")) return `путь с «..»: ${dir}`;
  if (UNSAFE_PATH_RE.test(dir)) return `недопустимые символы в пути: ${dir}`;

  const full = lower(dir);
  if (path.parse(path.resolve(dir)).root.toLowerCase().replace(/[\\/]+$/, "") === full) return "корень диска";
  for (const blocked of protectedDirs(env))
    if (lower(blocked) === full) return `системный каталог: ${dir}`;

  return null;
}

function isSameDir(a, b) {
  return lower(a) === lower(b);
}

function isInside(child, parent) {
  const c = lower(child);
  const p = lower(parent);
  return c === p || c.startsWith(p + "\\") || c.startsWith(p + "/");
}

module.exports = {
  PRODUCTION,
  ORIGIN_KEY,
  SEED_FLAG,
  DESKTOP_VALUE,
  LIBRARY_ROOT,
  EXE,
  PUBLISHER,
  OWNER_MARKER,
  UNINSTALL_FLAG,
  RETURN_SETUP_FLAG,
  VERSION_RE,
  INSTALL_ID_RE,
  guidFor,
  identity,
  parseArgs,
  protectedDirs,
  refusedDir,
  isSameDir,
  isInside,
};
