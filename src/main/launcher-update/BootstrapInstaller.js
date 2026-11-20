"use strict";
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");
const ArchiveInstall = require("./ArchiveInstall");
const embeddedPayload = require("./embedded-payload");
const channel = require("./launcher-channel");
const uninstaller = require("../uninstall/launcher-uninstall");
const { openInstallerWindow } = require("./installer-window");
const { DEFAULT_LIBRARY_PATH } = require("../../shared/library-path");
const { refreshMenuCaches, refreshKdeMenuCache, ensureAppImageDesktop, ensureManagedCli } = require("../linux/DesktopIntegration");

function launchEnvironment(source = process.env) {
  const env = { ...source };
  for (const key of ["APPIMAGE", "APPDIR", "ARGV0", "OWD"]) delete env[key];
  if (source.APPDIR && env.LD_LIBRARY_PATH) {
    env.LD_LIBRARY_PATH = env.LD_LIBRARY_PATH.split(":").filter((entry) => entry && !entry.startsWith(source.APPDIR)).join(":");
    if (!env.LD_LIBRARY_PATH) delete env.LD_LIBRARY_PATH;
  }
  return env;
}

function rendererLibraryPath(userData) {
  // Путь библиотеки лежит в сторе обычного лаунчера (../library.json),
  // установщик живёт в соседнем userData .../installer и своего стора не имеет.
  const file = path.join(path.dirname(userData), "library.json");
  try {
    const data = JSON.parse(fs.readFileSync(file, "utf8"));
    if (data && typeof data.libraryPath === "string" && data.libraryPath.trim())
      return data.libraryPath.trim();
  } catch (_) {
    /* стора нет — библиотека по умолчанию */
  }
  return DEFAULT_LIBRARY_PATH;
}

// readCurrent отдаёт "versions/<версия>-<sha>": установленная версия нужна,
// чтобы предложить обновление, если встроенная новее.
function versionOf(current) {
  const match = /^versions\/([0-9]+\.[0-9]+\.[0-9]+)-/.exec(current || "");
  return match ? match[1] : null;
}

function openInstaller(electron, options = {}) {
  const { app } = electron;
  let root = options.root || ArchiveInstall.defaultRoot();
  const assets = options.assets || path.join(process.resourcesPath, "installer");
  let installed = null;
  try { installed = ArchiveInstall.readCurrent(root); } catch (_) { /* reported during installation */ }
  // Удаление предлагаем и для битой установки (есть маркер, но нет current).
  const preexisting = !!installed || uninstaller.isManagedRoot(root);
  let payload = null, payloadError = null;
  try { payload = embeddedPayload.read(options.payload || embeddedPayload.payloadDir()); }
  catch (error) { payloadError = `${error.message}. Скачайте установщик заново.`; }
  const installedVersion = versionOf(installed);
  const update = !!(payload && installedVersion &&
    channel.compareSemver(payload.manifest.version, installedVersion) > 0);
  const libraryRoot = () => rendererLibraryPath(app.getPath("userData"));

  const backend = {
    selectDirectory(directory, { send }) {
      const selectedRoot = path.resolve(directory);
      let current = null;
      try { current = ArchiveInstall.readCurrent(selectedRoot); } catch (_) { /* install validates the target */ }
      if (!current && !uninstaller.isManagedRoot(selectedRoot) && fs.existsSync(selectedRoot) && fs.readdirSync(selectedRoot).length)
        throw new Error("Выберите пустую папку или каталог установленного HLSR");
      root = selectedRoot;
      const currentVersion = versionOf(current);
      const isUpdate = !!(payload && currentVersion && channel.compareSemver(payload.manifest.version, currentVersion) > 0);
      send({ directory: root, phase: payloadError ? "error" : current && !isUpdate ? "done" : "idle",
        installedVersion: currentVersion, update: isUpdate, canUninstall: !!current || uninstaller.isManagedRoot(root), error: payloadError, warning: null });
    },
    async install(desktopShortcut, { send, signal, closed, commit }) {
      if (!payload) { send({ phase: "error", error: payloadError }); return false; }
      const installer = new ArchiveInstall(root);
      try {
        // Сеть не нужна: встроенный архив проверяется по размеру и хешам из
        // описания и распаковывается тем же ArchiveInstall, что и обновления.
        send({ phase: "verifying", error: null });
        await installer.prepare(payload.archive, payload.manifest, { signal });
        if (closed()) return false;
        signal.throwIfAborted();
        commit();
        send({ phase: "installing" });
        installer.commit();
        let warning = null;
        try {
          if (!ensureManagedCli({ installRoot: root, home: app.getPath("home") }))
            warning = "Существующая команда hlsr сохранена без изменений";
        } catch (error) { warning = error.message; }
        try {
          if (!ensureAppImageDesktop({ executable: path.join(root, "start"), iconSource: path.join(root, "current/resources/icons/hlsr.png") }))
            throw new Error("Ярлык в меню принадлежит другому приложению");
          if (desktopShortcut === true) {
            const desktop = app.getPath("desktop");
            fs.mkdirSync(desktop, { recursive: true });
            const dataHome = path.isAbsolute(process.env.XDG_DATA_HOME || "") ? process.env.XDG_DATA_HOME : path.join(os.homedir(), ".local/share");
            const target = path.join(desktop, "hlsr.desktop");
            if (fs.existsSync(target) && !fs.readFileSync(target, "utf8").includes("X-HLSR-Managed=true")) throw new Error("Существующий ярлык рабочего стола сохранён");
            fs.copyFileSync(path.join(dataHome, "applications/hlsr.desktop"), target);
            fs.chmodSync(target, 0o755);
          }
        } catch (error) { warning = [warning, error.message].filter(Boolean).join(". "); }
        send({ phase: "done", warning, canUninstall: false, update: false, installedVersion: payload.manifest.version });
        return true;
      } finally {
        installer.discard();
      }
    },
    async launch({ send }) {
      try {
        const target = path.join(root, ArchiveInstall.readCurrent(root), "hlsr");
        await new Promise((resolve, reject) => {
          const child = spawn(target, ["--no-sandbox"], { detached: true, stdio: "ignore", env: launchEnvironment() });
          child.once("error", reject);
          child.once("spawn", () => { child.unref(); resolve(); });
        });
        app.quit();
        return true;
      } catch (error) { send({ phase: "done", error: error.message }); return false; }
    },
    uninstallPlan() {
      return uninstaller.planLauncherUninstall({ installRoot: root, libraryRoot: libraryRoot() });
    },
    async uninstall(removeGames, { send, commit }) {
      commit();
      send({ phase: "uninstalling", error: null });
      // Синхронное удаление больших игр блокирует цикл на секунды — фазу
      // уже отправили, окно покажет «Удаление…» до блокировки.
      // Данные лаунчера: ~/.config/hlsr (установщик живёт в .../hlsr/installer)
      // и журналы ~/.local/state/hlsr (index.js); уходят только при полном удалении.
      const result = uninstaller.uninstallLauncher({
        installRoot: root, libraryRoot: libraryRoot(), removeGames: removeGames === true,
        dataDirs: [path.dirname(app.getPath("userData")), path.join(app.getPath("home"), ".local", "state", "hlsr")],
      });
      try {
        refreshMenuCaches();
        if (result.desktopRemoved) refreshKdeMenuCache();
      } catch (_) {
        /* меню обновится само при следующем входе */
      }
      const failures = result.failed || [];
      const failed = failures.slice(0, 3).map((item) => item.path).join(", ");
      const left = result.left.slice(0, 5).join(", ");
      send({
        phase: "uninstalled", canUninstall: false,
        warning: [result.status === "partial" ? `Часть файлов осталась (${failures.length}): ${failed}` : null,
          left ? `Оставлено в ${result.libraryRoot} (${result.left.length}), удаление это не трогает: ${left}` : null].filter(Boolean).join(". ") || null,
      });
      return true;
    },
  };

  return openInstallerWindow(electron, {
    assets, backend, log: console,
    icon: path.join(process.resourcesPath, "icons/hlsr.png"),
    state: {
      phase: payloadError ? "error" : installed && !update ? "done" : "idle", directory: root,
      language: app.getLocale().startsWith("ru") ? "ru" : "en",
      canUninstall: preexisting,
      version: payload ? payload.manifest.version : null,
      installedVersion, update, error: payloadError,
    },
  });
}
module.exports = { openInstaller, launchEnvironment, versionOf };
