"use strict";

// Режимы установщика в Windows-сборке HLSR:
//   setup     — HLSR.exe распакован из HLSR-Setup-<версия>.exe (есть
//               PORTABLE_EXECUTABLE_FILE): окно установки, а с /S — тихое
//               обновление, как его запускают обновляторы прежних версий;
//   uninstall — HLSR.exe --hlsr-uninstall из «Программ и компонентов».
// Окно то же, что у Linux-установщика (src/installer, installer-window.js).
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn, spawnSync } = require("child_process");
const layout = require("./layout");
const fileList = require("./file-list");
const { install, originOf } = require("./install");
const uninstaller = require("./uninstall");
const { createRegistry } = require("./registry");
const { createShortcuts } = require("./shortcuts");
const { openInstallerWindow } = require("../launcher-update/installer-window");
const cleanup = require("../uninstall/library-cleanup");
const cleanupCli = require("../uninstall/cleanup-cli");
const { compareSemver } = require("../launcher-update/launcher-channel");
const { DEFAULT_LIBRARY_PATH } = require("../../shared/library-path");

// Коды выхода тихого режима: обновлятор прежней версии считает
// обновление состоявшимся только при 0.
const EXIT = { OK: 0, FAILED: 1, IN_USE: 3, BAD_DIR: 4 };
const LOCK_WAIT_MS = 60000;

function detect(argv, env) {
  const args = layout.parseArgs(argv);
  if (args.uninstall) return { mode: "uninstall", args };
  if (env.PORTABLE_EXECUTABLE_FILE) return { mode: "setup", args };
  return null;
}

// Окружение без переменных распаковщика: иначе запущенный лаунчер снова
// принял бы себя за установщик.
function launchEnvironment(source = process.env) {
  const env = { ...source };
  for (const key of Object.keys(env)) if (key.startsWith("PORTABLE_EXECUTABLE_")) delete env[key];
  return env;
}

function resolveInstallDir(args, values, id) {
  if (args.dir) return path.resolve(args.dir);
  const saved = values && values.InstallLocation;
  if (saved && values[layout.SEED_FLAG] !== "1" && !layout.refusedDir(saved, process.env)) return path.resolve(saved);
  return id.defaultInstallDir;
}

function libraryRoot(app) {
  return cleanupCli.libraryPathFrom(app.getPath("userData")) || DEFAULT_LIBRARY_PATH;
}

// Сборка, которую ставит установщик или удаляет удаление: её список
// файлов и identity. Без списка удаление всё же работает — с ключами
// боевого HLSR.
function buildOf(dir, log) {
  try {
    const list = fileList.read(dir);
    return { list, identity: layout.identity(list.app), error: null };
  } catch (error) {
    log.warn(`[setup] ${error.message}`);
    return { list: null, identity: layout.identity(), error };
  }
}

// Флаги тихого удаления прежнего NSIS-деинсталлятора для очистки данных:
// /HLSRDELETESETTINGS сохраняет статистику, если не добавлен /HLSRDELETEPROFILE.
function dataCleanupArgs(args) {
  if (args.deleteSettings) return args.deleteProfile ? ["--settings"] : ["--settings", "--keep-profile"];
  if (args.deleteProfile) return ["--delete-profile"];
  return [];
}

function language(app, setupFile) {
  const origin = originOf(setupFile);
  if (origin) return origin;
  return app.getLocale().startsWith("ru") ? "ru" : "en";
}

// Лаунчер держит single-instance lock на своём профиле; установщик и
// удаление берут тот же lock: пока он у них, HLSR не запустится, а если
// взять не удалось — HLSR уже работает и его файлы заняты.
async function acquireLock(app, waitMs) {
  const deadline = Date.now() + waitMs;
  for (;;) {
    if (app.requestSingleInstanceLock()) return true;
    if (Date.now() >= deadline) return false;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

// Запуск только что записанного HLSR.exe. CreateProcess отвечает EACCES,
// пока антивирус проверяет свежий exe, и всегда, если у HLSR.exe стоит
// «Запускать от имени администратора», а установщик запущен без прав
// (ERROR_ELEVATION_REQUIRED). Флаг виден в реестре: тогда запуск сразу
// уходит в оболочку Windows (ShellExecute), которая сама спросит UAC.
// Антивирус отпускает exe сам: запуск повторяется, а в конце тоже
// пробуется оболочка.
const LAUNCH_RETRY_CODES = new Set(["EACCES", "EPERM", "EBUSY"]);
const LAUNCH_ATTEMPTS = 6;
const LAUNCH_RETRY_MS = 1000;

function spawnOnce(spawnFn, exe, installDir, args) {
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawnFn(exe, args, { cwd: installDir, detached: true, stdio: "ignore", env: launchEnvironment() });
    } catch (error) {
      return reject(error);
    }
    child.once("error", reject);
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  });
}

const COMPAT_LAYERS = "Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Layers";

// Флаг «Запускать от имени администратора» из свойств exe: значение с
// путём exe в AppCompatFlags\Layers (у пользователя или у всех), в данных
// RUNASADMIN. С ним CreateProcess из обычного процесса не запустит exe.
function runsAsAdmin(exe, query) {
  for (const hive of ["HKCU", "HKLM"]) {
    const result = query(["query", `${hive}\\${COMPAT_LAYERS}`, "/v", exe]);
    if (result && result.status === 0 && /\bRUNASADMIN\b/i.test(String(result.stdout || ""))) return true;
  }
  return false;
}

function defaultRegQuery(args) {
  const reg = path.join(process.env.SystemRoot || process.env.WINDIR || "C:\\Windows", "System32", "reg.exe");
  return spawnSync(reg, args, { windowsHide: true, encoding: "utf8", timeout: 10000 });
}

// Start-Process — это ShellExecute: учитывает флаг совместимости и сам
// спрашивает UAC, а в отличие от shell.openPath передаёт аргументы.
// Окружение без переменных распаковщика: иначе лаунчер снова принял бы
// себя за установщик.
function defaultShellStart(exe, args, cwd) {
  const powershell = path.join(process.env.SystemRoot || process.env.WINDIR || "C:\\Windows",
    "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  const command = "Start-Process -FilePath $env:HLSR_EXE -WorkingDirectory $env:HLSR_DIR" +
    (args.length ? " -ArgumentList $env:HLSR_ARGS" : "");
  const env = { ...launchEnvironment(), HLSR_EXE: exe, HLSR_DIR: cwd, HLSR_ARGS: args.join(" ") };
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(powershell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command],
        { stdio: "ignore", windowsHide: true, env });
    } catch (error) {
      return resolve(false);
    }
    child.once("error", () => resolve(false));
    child.once("exit", (code) => resolve(code === 0));
  });
}

async function openWithShell(shellStart, exe, args, cwd, log) {
  const ok = await shellStart(exe, args, cwd);
  if (!ok) log.warn("[setup] запуск HLSR.exe через оболочку не удался");
  return ok;
}

async function launchInstalled(app, installDir, deps = {}, args = []) {
  const exe = path.join(installDir, layout.EXE);
  const shellStart = deps.shellStart || (process.platform === "win32" ? defaultShellStart : null);
  const spawnFn = deps.spawn || spawn;
  const sleep = deps.sleep || ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  const log = deps.log || { warn() {} };
  app.releaseSingleInstanceLock();

  let elevated = false;
  try {
    elevated = runsAsAdmin(exe, deps.regQuery || defaultRegQuery);
  } catch (error) {
    elevated = false;
  }
  if (elevated && shellStart) {
    log.warn("[setup] у HLSR.exe стоит «Запускать от имени администратора»: запуск через оболочку");
    if (await openWithShell(shellStart, exe, args, installDir, log)) return;
  }

  let lastError = null;
  for (let attempt = 1; attempt <= LAUNCH_ATTEMPTS; attempt += 1) {
    try {
      return await spawnOnce(spawnFn, exe, installDir, args);
    } catch (error) {
      lastError = error;
      if (!LAUNCH_RETRY_CODES.has(error && error.code)) throw error;
      log.warn(`[setup] запуск HLSR.exe: ${error.code}, попытка ${attempt} из ${LAUNCH_ATTEMPTS}`);
      if (attempt < LAUNCH_ATTEMPTS) await sleep(LAUNCH_RETRY_MS);
    }
  }

  if (!shellStart || !(await openWithShell(shellStart, exe, args, installDir, log))) throw lastError;
}

function services(electron, identity) {
  const { app, shell } = electron;
  const registry = createRegistry();
  const shortcuts = createShortcuts({ shell, desktopDir: app.getPath("desktop"), identity });
  const run = (file, args) => spawnSync(file, args, { windowsHide: true, timeout: 60000 });
  return { registry, shortcuts, run };
}

async function runSetup(electron, { args }, log) {
  const { app } = electron;
  const sourceDir = path.dirname(process.execPath);
  const build = buildOf(sourceDir, log);
  const id = build.identity;
  const { registry, shortcuts, run } = services(electron, id);
  const values = registry.read(id.installKey) || {};
  let installDir = resolveInstallDir(args, values, id);
  const setupFile = process.env.PORTABLE_EXECUTABLE_FILE;
  const version = app.getVersion();
  const base = { sourceDir, installDir, registry, shortcuts, run, setupFile, log, libraryRoot: libraryRoot(app) };
  const brokenSetup = build.error ? `Установщик повреждён: ${build.error.message}. Скачайте его заново.` : null;

  if (args.silent) {
    if (brokenSetup) {
      log.error(`[setup] ${brokenSetup}`);
      return app.exit(EXIT.FAILED);
    }
    log.info(`[setup] тихая установка ${version} в ${installDir}${args.updated ? " (обновление)" : ""}`);
    if (!(await acquireLock(app, LOCK_WAIT_MS))) {
      log.warn("[setup] HLSR не завершился — обновление отменено");
      return app.exit(EXIT.IN_USE);
    }
    try {
      await install({ ...base, update: true });
      if (args.forceRun) await launchInstalled(app, installDir, { log });
      return app.exit(EXIT.OK);
    } catch (error) {
      log.error(`[setup] ${error.code || ""} ${error.stack || error.message}`);
      return app.exit(error.code === "IN_USE" ? EXIT.IN_USE : error.code === "BAD_DIR" ? EXIT.BAD_DIR : EXIT.FAILED);
    }
  }

  let window = null;
  const locked = await acquireLock(app, 0);
  app.on("second-instance", () => {
    if (!window || window.isDestroyed()) return;
    if (window.isMinimized()) window.restore();
    window.focus();
  });
  const uninstallValues = registry.read(id.uninstallKey) || {};
  const installed = layout.VERSION_RE.test(uninstallValues.DisplayVersion || "") &&
    fs.existsSync(path.join(installDir, layout.EXE)) ? uninstallValues.DisplayVersion : null;
  const update = !!installed && compareSemver(version, installed) > 0;
  const lockError = "HLSR сейчас запущен. Закройте лаунчер и нажмите «Повторить установку».";

  const backend = {
    selectDirectory(directory, { send }) {
      installDir = path.resolve(directory);
      const existing = buildOf(installDir, log);
      const currentVersion = !existing.error && fs.existsSync(path.join(installDir, layout.EXE)) ? existing.list.version : null;
      const isUpdate = !!currentVersion && compareSemver(version, currentVersion) > 0;
      send({ directory: installDir, phase: brokenSetup ? "error" : currentVersion && !isUpdate ? "done" : "idle",
        installedVersion: currentVersion, update: isUpdate, canUninstall: !!currentVersion, error: brokenSetup, warning: null });
    },
    async install(desktopShortcut, { send, signal, commit }) {
      if (!app.hasSingleInstanceLock() && !(await acquireLock(app, 0))) {
        send({ phase: "error", error: lockError });
        return false;
      }
      if (brokenSetup) {
        send({ phase: "error", error: brokenSetup });
        return false;
      }
      send({ phase: "installing", error: null, warning: null, progress: 0 });
      let shown = 0;
      const onProgress = (done, total) => {
        const progress = total ? Math.floor((done / total) * 100) : 0;
        if (progress !== shown) send({ progress: (shown = progress) });
      };
      const result = await install({ ...base, installDir, signal, update: false, desktopShortcut, onProgress, onCommit: commit });
      send({
        phase: "done", update: false, installedVersion: result.version, canUninstall: false, progress: null,
        warning: result.warnings.length ? result.warnings.join(". ") : null,
      });
      return true;
    },
    async launch() {
      await launchInstalled(app, installDir, { log });
      app.quit();
      return true;
    },
    // Удаляет установленная копия: она знает свой каталог и проверяет
    // владение. Установщик запускает её и закрывается.
    async uninstallPlan({ send }) {
      // Без обработчика ошибка spawn (EACCES при «Запускать от имени
      // администратора») уходила в uncaughtException и показывалась как
      // «ошибка запуска лаунчера».
      try {
        const back = returnSetupOf(setupFile) ? [layout.RETURN_SETUP_FLAG, setupFile] : [];
        await launchInstalled(app, installDir, { log }, [layout.UNINSTALL_FLAG, ...back]);
      } catch (error) {
        log.error(`[setup] запуск удаления не удался: ${error.stack || error.message}`);
        if (!app.hasSingleInstanceLock()) await acquireLock(app, 0);
        send({ error: `Не удалось запустить удаление: ${String(error && error.message).slice(0, 300)}` });
        return false;
      }
      app.quit();
      return false;
    },
    uninstall: async () => false,
  };

  window = openInstallerWindow(electron, {
    assets: path.join(process.resourcesPath, "installer"),
    backend, log,
    state: {
      phase: brokenSetup || !locked ? "error" : installed && !update ? "done" : "idle",
      directory: installDir,
      language: language(app, setupFile),
      canUninstall: !!installed,
      version, installedVersion: installed, update,
      error: brokenSetup || (locked ? null : lockError),
    },
  });
  app.on("window-all-closed", () => app.quit());
  return window;
}

// Установщик, к которому ведёт «Назад» после удаления: только
// существующий .exe по абсолютному пути.
function returnSetupOf(file) {
  const value = String(file || "");
  if (!path.win32.isAbsolute(value) || !/\.exe$/i.test(value) || /[\r\n\0]/.test(value)) return null;
  try { return fs.statSync(value).isFile() ? value : null; } catch (_) { return null; }
}

async function runUninstall(electron, { args }, log) {
  const { app, dialog } = electron;
  const installDir = path.dirname(process.execPath);
  // Сборка — по списку файлов, подтверждённому маркером владения; если они
  // расходятся или списка нет — по GUID маркера и не боевая (uninstall.js).
  const id = uninstaller.identityOf(installDir);
  const { registry, shortcuts } = services(electron, id);
  const returnSetup = returnSetupOf(args.returnSetup);
  // Что удалить после выхода: каталог программы, данные, опустевшую папку HLSR.
  let pending = null, relaunch = null;
  const schedule = (options = {}) =>
    uninstaller.scheduleRemoval(pending.removeProgram,
      { ...options, data: pending.removeData, parent: pending.removeParent, ownMarker: pending.ownMarker });
  app.on("will-quit", () => {
    if (pending) {
      try {
        schedule({ relaunch });
        return;
      } catch (error) {
        log.error(`[uninstall] каталог программы не удалён: ${error.message}`);
      }
    }
    if (!relaunch) return;
    try {
      spawn(relaunch, [], { cwd: path.dirname(relaunch), detached: true, stdio: "ignore", env: launchEnvironment() }).unref();
    } catch (error) {
      log.error(`[uninstall] установщик не открыт заново: ${error.message}`);
    }
  });

  if (!(await acquireLock(app, args.silent ? LOCK_WAIT_MS : 0))) {
    if (!args.silent)
      dialog.showMessageBoxSync({
        type: "warning", title: "HLSR",
        message: "HLSR сейчас запущен. Закройте лаунчер и запустите удаление ещё раз.\n\nHLSR is running. Close it and start the uninstall again.",
      });
    return app.exit(EXIT.IN_USE);
  }
  const libraryPath = libraryRoot(app);
  // Данные HLSR: настройки и вход (%APPDATA%\hlsr), журналы (%LOCALAPPDATA%\hlsr).
  const dataDirs = [app.getPath("userData")]
    .concat(process.env.LOCALAPPDATA ? [path.join(process.env.LOCALAPPDATA, app.getName())] : []);
  const run = (removeGames, data = []) => {
    const result = uninstaller.uninstall({
      installDir, libraryRoot: libraryPath, removeGames, registry, shortcuts, cleanup, identity: id, log,
      updaterCacheName: `${app.getName()}-updater`, dataDirs: data, tempRoot: os.tmpdir(),
      removeProtocolClient: (name) => app.removeAsDefaultProtocolClient(name),
    });
    pending = result.removeProgram || result.removeData.length ? result : null;
    return result;
  };

  if (args.silent) {
    // Настройки и статистика чистятся до удаления программы, как и прежде.
    const extra = dataCleanupArgs(args);
    if (extra.length) {
      const localAppData = process.env.LOCALAPPDATA || "";
      const cleaned = cleanupCli.run({
        argv: [process.execPath, cleanupCli.FLAG, ...extra, "--install-dir", installDir],
        silent: true, userData: app.getPath("userData"), appName: app.getName(), launcherVersion: app.getVersion(),
        logsDir: localAppData ? path.join(localAppData, app.getName(), "logs") : null,
        updaterCache: localAppData ? path.join(localAppData, `${app.getName()}-updater`) : null,
        installDir,
      });
      log.info(`[uninstall] очистка данных: ${cleaned.lines.join(" | ")}`);
    }
    // Тихий режим: данные — по флагам прежнего деинсталлятора (выше).
    const result = run(args.deleteGames);
    // app.exit() не шлёт will-quit, и каталог программы оставался на диске:
    // удаление планируется здесь, скрипт ждёт выхода этого HLSR.exe сам.
    if (pending) {
      try {
        schedule();
      } catch (error) {
        log.error(`[uninstall] каталог программы не удалён: ${error.message}`);
      }
      pending = null;
    }
    return app.exit(result.status === "ok" ? EXIT.OK : EXIT.FAILED);
  }

  const backend = {
    install: async () => false,
    launch: async () => false,
    uninstallPlan: () => uninstaller.plan({ installDir, libraryRoot: libraryPath, identity: id, registry }),
    async uninstall(removeGames, { send }) {
      send({ phase: "uninstalling", error: null });
      // Полное удаление («Также удалить игры») уносит и данные HLSR.
      const result = run(removeGames === true, dataDirs);
      send({ phase: "uninstalled", canUninstall: false, warning: result.warnings.length ? result.warnings.join(" ") : null });
      return true;
    },
    // Эта копия HLSR удаляется и поставить себя заново не может: «Назад»
    // закрывает окно, а установщик откроется, когда каталог будет удалён.
    back: returnSetup ? () => {
      relaunch = returnSetup;
      app.quit();
      return true;
    } : null,
  };
  const window = openInstallerWindow(electron, {
    assets: path.join(process.resourcesPath, "installer"),
    title: `${id.productName} — удаление`,
    backend, log,
    state: {
      phase: "uninstall-confirm", directory: installDir,
      language: app.getLocale().startsWith("ru") ? "ru" : "en",
      canUninstall: true, version: app.getVersion(), canGoBack: !!returnSetup,
      uninstallPlan: backend.uninstallPlan(),
    },
  });
  app.on("second-instance", () => window.isDestroyed() || window.focus());
  app.on("window-all-closed", () => app.quit());
  return window;
}

function run(electron, detected, log) {
  const task = detected.mode === "uninstall" ? runUninstall : runSetup;
  return task(electron, detected, log).catch((error) => {
    log.error(`[${detected.mode}] ${error.stack || error.message}`);
    if (!detected.args.silent) electron.dialog.showErrorBox("HLSR", String(error.message).slice(0, 600));
    electron.app.exit(EXIT.FAILED);
  });
}

module.exports = { detect, run, launchEnvironment, launchInstalled, runsAsAdmin, resolveInstallDir, dataCleanupArgs, returnSetupOf, EXIT };
