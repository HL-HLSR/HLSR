"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { app, BrowserWindow, ipcMain } = require("electron");

const planLib = require("./core/plan");
const qaTrust = require("./core/qa-trust");
const pathsLib = require("./core/paths");
const singleInstance = require("./core/single-instance");
const UpdaterLog = require("./core/log");
const UpdateSession = require("./core/session");
const installLib = require("./core/install");
const locales = require("./locales");
const { CODES, UpdaterError, toUpdaterError } = require("./core/errors");

const WINDOW = { width: 460, height: 280 };

const WINDOW_TITLE = "HLSR Updater";

let window = null;
let session = null;
let log = null;
let lock = null;
let language = "ru";

let launcherExe = null;
let installed = false;

function logsDir() {
  const base = process.env.LOCALAPPDATA || os.tmpdir();
  return path.join(base, "HLSR", "logs");
}

function lockDir() {
  const base = process.env.LOCALAPPDATA || os.tmpdir();
  return path.join(base, "HLSR");
}

function planPathFromArgv(argv) {
  for (const arg of argv.slice(1)) {
    const match = /^--plan=(.+)$/.exec(arg);
    if (match) return match[1];
  }
  return null;
}

function qaTrustOptions(resourcesPath = process.resourcesPath) {
  return qaTrust.read(resourcesPath);
}

function readPlanFile(file) {
  if (!file) throw new UpdaterError(CODES.PLAN_UNREADABLE, "путь к плану не передан");

  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (error) {
    throw new UpdaterError(CODES.PLAN_UNREADABLE, `план не читается: ${error.message}`);
  }

  try {
    return JSON.parse(raw);
  } catch (error) {
    throw new UpdaterError(CODES.PLAN_UNREADABLE, "план не разбирается как JSON");
  }
}

function createWindow() {
  const win = new BrowserWindow({
    width: WINDOW.width,
    height: WINDOW.height,
    resizable: false,
    maximizable: false,
    minimizable: true,
    fullscreenable: false,
    frame: true,
    title: WINDOW_TITLE,
    show: false,
    backgroundColor: "#f3f3f3",
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, "window", "preload.js"),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      devTools: false,
      backgroundThrottling: false,
    },
  });

  win.setMenu(null);
  win.once("ready-to-show", () => win.show());

  win.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  win.webContents.on("will-navigate", (event) => event.preventDefault());

  win.loadFile(path.join(__dirname, "window", "index.html"));
  return win;
}

function send(channelName, payload) {
  if (!window || window.isDestroyed()) return;
  window.webContents.send(channelName, payload);
}

function allowClose() {
  return !session || session.finished || session.isCancellable();
}

async function start() {
  let plan;
  try {
    const raw = readPlanFile(planPathFromArgv(process.argv));
    language = raw && raw.language === "en" ? "en" : "ru";

    send("updater:init", {
      language,
      strings: locales.pack(language),
      targetVersion: raw && typeof raw.targetVersion === "string" ? raw.targetVersion : "",
    });

    const qa = qaTrustOptions();
    if (qa) log.warn("[updater] ВКЛЮЧЕНО QA-доверие (файл QA_TRUST): это не боевой режим");

    plan = planLib.validate(raw, qa || undefined);
  } catch (error) {
    const updaterError = toUpdaterError(error, CODES.BAD_PLAN);
    log.error(`[updater] план отклонён: ${updaterError.code} ${updaterError.message}`);
    send("updater:stage", { stage: "error", error: updaterError.toPublic() });
    return;
  }

  log.info(`[updater] план принят: ${plan.currentVersion} -> ${plan.targetVersion}`);

  launcherExe = plan.launcherExe;

  const workDir = path.join(path.dirname(app.getPath("exe")), "work");

  try {
    pathsLib.assertSafeWorkDir(workDir, { installDir: plan.installDir });
  } catch (error) {
    const updaterError = toUpdaterError(error, CODES.STAGING_FAILED);
    log.error(`[updater] ${updaterError.code}: ${updaterError.message}`);
    send("updater:stage", { stage: "error", error: updaterError.toPublic() });
    return;
  }

  pathsLib.assertWritable(path.join(workDir, plan.installer.filename), {
    allowedRoots: [workDir],
    allowedFiles: [plan.receiptFile],
  });

  session = new UpdateSession({ plan, workDir, logger: log });

  session.on("stage", (payload) => send("updater:stage", payload));
  session.on("progress", (payload) => send("updater:progress", payload));

  send("updater:init", {
    language,
    strings: locales.pack(language),
    targetVersion: plan.targetVersion,
  });

  const result = await session.run();

  if (result.ok) {
    installed = true;
    log.info("[updater] обновление завершено, лаунчер запущен");
    setTimeout(() => app.quit(), 2500);
  }
}

function relaunchLauncherIfNeeded() {
  if (installed || !launcherExe) return;

  try {
    installLib.relaunch(launcherExe, {});
    if (log) log.info("[updater] обновление не состоялось — запущена прежняя версия");
  } catch (error) {
    if (log) log.warn(`[updater] прежнюю версию запустить не удалось: ${error.message}`);
  }
}

app.on("ready", async () => {
  log = new UpdaterLog({ file: path.join(logsDir(), "updater.log") });
  log.info(`[updater] запуск, версия Electron ${process.versions.electron}`);

  try {
    lock = singleInstance.acquire(lockDir());
  } catch (error) {
    const updaterError = toUpdaterError(error, CODES.ALREADY_RUNNING);
    log.warn(`[updater] ${updaterError.code}: ${updaterError.message}`);

    window = createWindow();
    window.webContents.once("did-finish-load", () => {
      send("updater:init", {
        language,
        strings: locales.pack(language),
        targetVersion: "",
      });
      send("updater:stage", { stage: "error", error: updaterError.toPublic() });
    });
    return;
  }

  window = createWindow();
  window.on("close", (event) => {
    if (allowClose()) return;
    event.preventDefault();
  });

  window.webContents.once("did-finish-load", () => {
    start().catch((error) => {
      const updaterError = toUpdaterError(error, CODES.INSTALL_FAILED);
      log.error(`[updater] непредвиденная ошибка: ${updaterError.message}`);
      send("updater:stage", { stage: "error", error: updaterError.toPublic() });
    });
  });
});

ipcMain.on("updater:cancel", () => {
  if (!session) return;

  const accepted = session.cancel();
  if (log) log.info(`[updater] отмена ${accepted ? "принята" : "отклонена (поздно)"}`);
});

ipcMain.on("updater:close", () => {
  relaunchLauncherIfNeeded();
  app.quit();
});

app.on("window-all-closed", () => app.quit());

app.on("will-quit", () => {
  if (lock) {
    lock.release();
    lock = null;
  }
});

app.on("web-contents-created", (event, contents) => {
  contents.setWindowOpenHandler(() => ({ action: "deny" }));
});

module.exports = { planPathFromArgv, readPlanFile, qaTrustOptions, WINDOW, WINDOW_TITLE };
