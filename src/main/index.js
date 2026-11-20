"use strict";
import fs from "fs";
import os from "os";
import path from "path";
import { app, ipcMain, protocol } from "electron";
import log from "electron-log";
import AppManager from "./AppManager";
import createWindow from "./CreateWindow";
import cleanupCli from "./uninstall/cleanup-cli";
import logPolicy from "./diagnostics/log-policy";
import desktopIntegration from "./linux/DesktopIntegration";
import { DEFAULT_LIBRARY_PATH } from "../shared/library-path";

const IS_WINDOWS = process.platform === "win32";
const IS_LINUX = process.platform === "linux";
require("./online-profile/local-qa").prepare(app);

let window = null;
let appManager = null;

// Рендерер не знает домашний каталог пользователя: путь библиотеки по
// умолчанию он получает из main-процесса.
ipcMain.on("app-default-library-path", (event) => {
  event.returnValue = DEFAULT_LIBRARY_PATH;
});

function reportStartupError(error) {
  const message = error ? String(error.stack || error.message || error) : "unknown";

  try {
    log.error("Startup error:", message);
  } catch (e) {
    /* ignore */
  }

  try {
    const NotificationStore = require("./notifications/NotificationStore");
    const store = new NotificationStore({
      file: path.join(app.getPath("userData"), "notifications.json"),
    });
    store.add({ level: "error", code: "STARTUP_ERROR", detail: message.slice(0, 400) });
  } catch (e) {
    /* ignore */
  }

  try {
    const { dialog } = require("electron");
    dialog.showErrorBox(
      "HLSR — ошибка запуска / startup error",
      "Лаунчер не смог запуститься. Подробности в логе.\n" +
        "The launcher failed to start. See the log for details.\n\n" +
        message.slice(0, 600)
    );
  } catch (e) {
    /* ignore */
  }
}

const isUninstallCleanup = cleanupCli.isCleanupInvocation(process.argv);
// Установщик и удаление Windows-сборки: HLSR.exe, распакованный из
// HLSR-Setup-<версия>.exe, или HLSR.exe --hlsr-uninstall.
const windowsSetup = IS_WINDOWS ? require("./windows-install/setup").detect(process.argv, process.env) : null;

function logsDir() {
  if (IS_LINUX) return path.join(os.homedir(), ".local", "state", "hlsr", "logs");

  return process.env.LOCALAPPDATA ? path.join(process.env.LOCALAPPDATA, app.getName(), "logs") : null;
}

logPolicy.applyPolicy(log, {
  level: "info",
  logsDir: logsDir(),
});

log.transports.console.level = process.env.NODE_ENV === "development" ? "debug" : "info";
log.errorHandler.startCatching({ showDialog: false });

const userProfile = IS_WINDOWS ? process.env.USERPROFILE || "" : os.homedir();
const userName = userProfile ? path.basename(userProfile) : "";

log.hooks.push((message) => {
  if (!userName) return message;

  message.data = message.data.map((item) => {
    if (typeof item !== "string") return item;
    return item.split(`\\${userName}\\`).join("\\***\\").split(`/${userName}/`).join("/***/");
  });

  return message;
});

const FRAMELESS_WINDOW_BLACK_FRAME_ON_ALT_TAB_OVER_FULLSCREEN_GAME =
  "CalculateNativeWinOcclusion";

const DISABLED_CHROMIUM_FEATURES = [
  "OutOfBlinkCors",
  FRAMELESS_WINDOW_BLACK_FRAME_ON_ALT_TAB_OVER_FULLSCREEN_GAME,
];

const appendedCommandLineSwitches = new Set();

function appendCommandLineSwitchOnce(name, value) {
  if (appendedCommandLineSwitches.has(name)) {
    throw new Error(
      `Ключ ${name} уже задан: повторный appendSwitch не дописывает значение, ` +
        `а конфликтует с прежним, и часть списка молча теряется`
    );
  }

  appendedCommandLineSwitches.add(name);

  if (value === undefined) {
    app.commandLine.appendSwitch(name);
    return;
  }

  app.commandLine.appendSwitch(name, value);
}

if (IS_WINDOWS) {
  appendCommandLineSwitchOnce("disable-features", DISABLED_CHROMIUM_FEATURES.join(","));

  appendCommandLineSwitchOnce("disable-backgrounding-occluded-windows");
}

const gpuAccelerationEnabled = require("./gpu-preference").apply(app);
if (IS_LINUX) app.setDesktopName("hlsr.desktop");
log.info(`[gpu] аппаратное ускорение: ${gpuAccelerationEnabled ? "включено" : "выключено пользователем"}`);

app.allowRendererProcessReuse = false;

// Плеер «Демо»: протокол hlsr-media регистрируется до ready, иначе <video>
// не получит потоковую отдачу (stream) и перемотку.
{
  const demoMedia = require("./demo-tool/media");
  protocol.registerSchemesAsPrivileged([{ scheme: demoMedia.SCHEME, privileges: demoMedia.PRIVILEGES }]);
}

let started = false;

process.on("uncaughtException", (error) => {
  if (!started) {
    reportStartupError(error);
    app.exit(1);
    return;
  }

  log.error(`Необработанное исключение после старта: ${(error && error.stack) || error}`);
});

process.on("unhandledRejection", (reason) => {
  log.error(`Необработанный отказ промиса: ${(reason && reason.stack) || reason}`);
});

// Архивная установка в Linux: команда hlsr в ~/.local/bin и пункт меню
// приложений указывают на start текущей установки.
function ensureLinuxIntegration() {
  const installRoot = require("./launcher-update/ArchiveInstall").managedRoot(process.execPath);
  if (!installRoot) return;

  try {
    desktopIntegration.ensureManagedCli({ installRoot, home: app.getPath("home") });
  } catch (error) {
    log.warn(`[cli] ${error.message}`);
  }

  try {
    desktopIntegration.ensureAppImageDesktop({ executable: path.join(installRoot, "start") });
  } catch (error) {
    log.warn(`[desktop] ${error.message}`);
  }
}

app.on("ready", () => {
  if (windowsSetup) {
    require("./windows-install/setup").run(require("electron"), windowsSetup, log);
    return;
  }

  if (isUninstallCleanup) {
    cleanupCli.runFromMain(app);
    return;
  }

  try {
    if (IS_LINUX) ensureLinuxIntegration();

    window = createWindow();
    if (!window) return;

    appManager = new AppManager(window);
    appManager.initialize();

    started = true;
  } catch (error) {
    reportStartupError(error);
    app.exit(1);
  }
});

// Linux-обновлятор ставит обновление в своём before-quit, поэтому службы
// там освобождаются позже, на will-quit.
app.on(IS_LINUX ? "will-quit" : "before-quit", () => {
  if (appManager) appManager.dispose();
});

app.on("window-all-closed", () => {
  app.quit();
  window = null;
});

app.on("activate", () => {
  if (window === null) {
    window = createWindow();
  }
});

app.setAppUserModelId("com.hlsrteam.hlsr");

if (IS_WINDOWS && process.env.NODE_ENV === "production") {
  let dir = app.getPath("exe").split("\\");
  dir.splice(-1, 1);

  require("process").chdir(dir.join("\\"));

  try {
    let legacyAppIdFile = dir.join("\\") + "\\steam_appid.txt";

    if (fs.existsSync(legacyAppIdFile)) {
      let content = fs
        .readFileSync(legacyAppIdFile, "utf8")
        .replace(/^\uFEFF/, "")
        .replace(/\0+$/, "")
        .trim();

      if (content === "250820") {
        fs.unlinkSync(legacyAppIdFile);
        log.info("Removed legacy steam_appid.txt (SteamVR AppID 250820) next to HLSR.exe");
      } else {
        log.warn(
          `steam_appid.txt next to HLSR.exe has unexpected content, leaving it untouched`
        );
      }
    }
  } catch (e) {
    log.warn("Legacy steam_appid.txt migration failed:", e.message);
  }
}
