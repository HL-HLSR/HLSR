"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const MANAGED_MARKER = "X-HLSR-Managed=true";
const CLI_MARKER = "# HLSR managed CLI launcher";

function cliScript(installRoot) {
  const start = path.join(path.resolve(installRoot), "start");
  const quoted = `'${start.replace(/'/g, "'\\''")}'`;
  // A direct symlink to start changes $0; its dirname would become ~/.local/bin
  // and the launcher would look for current/ in the wrong directory.
  return `#!/bin/sh\n${CLI_MARKER}\nexec ${quoted} "$@"\n`;
}

function cliPath(options = {}) {
  return path.join(options.home || os.homedir(), ".local", "bin", "hlsr");
}

function managedCliPath(installRoot, options = {}) {
  const file = cliPath(options);
  try {
    const stat = fs.lstatSync(file);
    if (stat.isSymbolicLink()) {
      return path.resolve(path.dirname(file), fs.readlinkSync(file)) === path.join(path.resolve(installRoot), "start") ? file : null;
    }
    const expected = cliScript(installRoot);
    return stat.isFile() && stat.size === Buffer.byteLength(expected) && fs.readFileSync(file, "utf8") === expected ? file : null;
  } catch (error) {
    return null;
  }
}

function ensureManagedCli(options = {}) {
  if (!path.isAbsolute(options.installRoot || "")) return false;
  const root = path.resolve(options.installRoot);
  try {
    const marker = JSON.parse(fs.readFileSync(path.join(root, ".hlsr-install.json"), "utf8"));
    if (marker.schema !== 1) return false;
    fs.accessSync(path.join(root, "start"), fs.constants.X_OK);
  } catch (error) { return false; }

  const file = cliPath(options);
  let previous;
  try { previous = fs.lstatSync(file); } catch (error) { if (error.code !== "ENOENT") throw error; }
  if (previous) {
    if (!managedCliPath(root, options)) return false;
    if (!previous.isSymbolicLink()) {
      fs.chmodSync(file, 0o755);
      return true;
    }
    fs.unlinkSync(file);
  }
  fs.mkdirSync(path.dirname(file), { recursive: true });
  try {
    // Do not replace a foreign file or link created while the installer runs.
    fs.writeFileSync(file, cliScript(root), { flag: "wx", mode: 0o755 });
  } catch (error) {
    if (error.code === "EEXIST") return false;
    throw error;
  }
  return true;
}

function desktopExecPath(file) {
  if (!path.isAbsolute(file) || /[\r\n\0]/.test(file)) return null;
  const escaped = file.replace(/\\/g, "\\\\").replace(/"/g, '\\"')
    .replace(/[$`]/g, "\\$&").replace(/%/g, "%%");
  return `"${escaped}"`;
}

function defaultDataHome() {
  return path.isAbsolute(process.env.XDG_DATA_HOME || "")
    ? process.env.XDG_DATA_HOME
    : path.join(os.homedir(), ".local", "share");
}

function refreshMenuCaches(dataHome = defaultDataHome()) {
  // Иначе запись HLSR может появиться в «Пуске» только после перелогина.
  // Best-effort: отсутствие утилит или их ошибки никогда не роняют установку.
  const applications = path.join(dataHome, "applications");
  const hicolor = path.join(dataHome, "icons", "hicolor");
  for (const [command, args] of [
    ["update-desktop-database", [applications]],
    ["gtk-update-icon-cache", ["-f", "-t", hicolor]],
  ]) {
    try {
      spawnSync(command, args, { stdio: "ignore", timeout: 10000 });
    } catch (_) { /* меню обновится само при следующем входе */ }
  }
}

function refreshKdeMenuCache(dataHome = defaultDataHome()) {
  // У AppImage могут быть свои библиотеки и Qt-плагины; kbuildsycoca нужен
  // из установленной в системе Plasma, чтобы сразу убрать запись из меню.
  const env = { ...process.env, XDG_DATA_HOME: dataHome };
  for (const key of ["APPIMAGE", "APPDIR", "LD_LIBRARY_PATH", "LD_PRELOAD", "QT_PLUGIN_PATH", "QT_QPA_PLATFORM_PLUGIN_PATH"])
    delete env[key];
  for (const command of ["kbuildsycoca6", "kbuildsycoca5"]) {
    try { spawnSync(command, ["--noincremental"], { stdio: "ignore", timeout: 10000, env }); }
    catch (_) { /* KDE обновит меню при следующем входе */ }
  }
}

function ensureAppImageDesktop(options = {}) {
  const appImage = options.executable || options.appImage || process.env.APPIMAGE;
  const iconSource = options.iconSource || path.join(process.resourcesPath, "icons", "hlsr.png");
  const dataHome = options.dataHome || defaultDataHome();
  const execPath = desktopExecPath(appImage);
  if (!execPath || !fs.existsSync(appImage) || !fs.existsSync(iconSource)) return false;

  const desktopPath = path.join(dataHome, "applications", "hlsr.desktop");
  const iconPath = path.join(dataHome, "icons", "hicolor", "256x256", "apps", "hlsr-appimage.png");
  let previous = null;
  try { previous = fs.readFileSync(desktopPath, "utf8"); } catch (error) { /* new entry */ }
  if (previous && !previous.includes(MANAGED_MARKER)) return false;

  const desktop = [
    "[Desktop Entry]",
    "Type=Application",
    "Name=HLSR",
    `Exec=${execPath} --no-sandbox %U`,
    "Icon=hlsr-appimage",
    "StartupWMClass=hlsr",
    "Terminal=false",
    "Categories=Game;",
    MANAGED_MARKER,
    "",
  ].join("\n");

  fs.mkdirSync(path.dirname(desktopPath), { recursive: true });
  fs.mkdirSync(path.dirname(iconPath), { recursive: true });
  const desktopTemp = `${desktopPath}.${process.pid}.tmp`;
  const iconTemp = `${iconPath}.${process.pid}.tmp`;
  try {
    fs.copyFileSync(iconSource, iconTemp);
    fs.renameSync(iconTemp, iconPath);
    if (previous !== desktop) {
      fs.writeFileSync(desktopTemp, desktop, { mode: 0o644 });
      fs.renameSync(desktopTemp, desktopPath);
    }
  } finally {
    for (const file of [desktopTemp, iconTemp]) {
      try { fs.unlinkSync(file); } catch (error) { /* already renamed */ }
    }
  }
  if (options.refresh !== false) refreshMenuCaches(dataHome);
  return true;
}

module.exports = { ensureAppImageDesktop, desktopExecPath, refreshMenuCaches, refreshKdeMenuCache, ensureManagedCli, managedCliPath };
