"use strict";

// Окно установщика HLSR, общее для Linux и Windows: одна страница
// (src/installer), один набор IPC и одна логика занятости. Чем отличаются
// платформы — что делают «Установить», «Открыть» и «Удалить» — решает
// backend, который передаётся в openInstallerWindow.
//
// Методы backend получают ctx = { send, signal, closed } и сами шлют
// окну изменения состояния через ctx.send(patch):
//   install(desktopShortcut, ctx) -> boolean
//   launch(ctx)                   -> boolean
//   uninstallPlan(ctx)            -> план для диалога или false
//   uninstall(removeGames, ctx)   -> boolean
//   back(ctx)                     -> «Назад» после удаления (необязательно)
const path = require("path");
const { LINKS } = require("../../shared/links");

// Код ошибки ОС (ENOSPC, EACCES…) у причины или у самой ошибки: окно
// объясняет его своими словами, сырой текст остаётся в журнале.
function systemCode(error) {
  const code = String((error && error.cause && error.cause.code) || (error && error.code) || "");
  return /^E[A-Z]+$/.test(code) || code === "UNKNOWN" ? code : null;
}

function openInstallerWindow(electron, options) {
  const { BrowserWindow, ipcMain, dialog } = electron;
  const { assets, icon, backend } = options;
  const log = options.log || { error() {} };
  const window = new BrowserWindow({
    width: 640, height: 560, minWidth: 640, maxWidth: 640, minHeight: 560, maxHeight: 560,
    resizable: false, maximizable: false, fullscreenable: false, frame: false,
    // Как у окна лаунчера: углы и контур рисует страница, системное
    // скругление поверх CSS давало бы два разных радиуса.
    transparent: true, roundedCorners: false, backgroundColor: "#00000000",
    title: options.title || "HLSR — установка", autoHideMenuBar: true,
    icon,
    webPreferences: { preload: path.join(assets, "preload.js"), nodeIntegration: false, contextIsolation: true, sandbox: true },
  });
  window.removeMenu();
  // В Linux maximizable игнорируется Electron: фиксируем ограничения
  // размеров и отклоняем команды разворачивания от оконного менеджера.
  window.on("maximize", () => window.unmaximize());
  window.on("enter-full-screen", () => window.setFullScreen(false));
  window.webContents.setZoomFactor(1);
  // До загрузки страницы кадра ещё нет, и вызов отказывает
  // («Render frame was disposed»): ограничение ставится после загрузки.
  window.webContents.on("did-finish-load", () => {
    Promise.resolve(window.webContents.setVisualZoomLevelLimits(1, 1)).catch(() => {});
  });
  window.webContents.on("before-input-event", (event, input) => {
    if ((input.control || input.meta) && ["+", "=", "-", "0"].includes(input.key)) event.preventDefault();
    if (input.key === "F11") event.preventDefault();
  });
  window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  window.webContents.on("will-navigate", (event) => event.preventDefault());

  let busy = false, controller = null, closed = false, closing = false, choosingDirectory = false;
  // platform: в Windows перед установкой есть шаг про SmartScreen.
  let state = { platform: process.platform, ...options.state, canChooseDirectory: !!backend.selectDirectory, cancellable: false };
  const send = (patch) => {
    // Новая ошибка без кода не наследует код прежней (ENOSPC, ANTIVIRUS…).
    const reset = patch.error && !("errorCode" in patch) ? { errorCode: null, errorFile: null } : {};
    state = { ...state, ...reset, ...patch };
    if (!closed) window.webContents.send("installer-state", state);
  };
  const ctx = () => ({ send, signal: controller ? controller.signal : null, closed: () => closed, state: () => state,
    commit: () => send({ cancellable: false }),
  });

  // Долгие операции: одна за раз; закрытие окна во время работы ждёт её конца.
  async function exclusive(fn, cancellable = false) {
    if (busy || choosingDirectory || closing) return false;
    busy = true;
    controller = new AbortController();
    send({ cancellable, error: null, warning: null, errorCode: null, errorFile: null });
    try {
      return await fn(ctx());
    } catch (error) {
      if (controller.signal.aborted) {
        send({ phase: "cancelled", error: null, progress: null });
        return false;
      }
      log.error(`[installer] ${(error && error.stack) || error}${error && error.cause ? `\n${error.cause.message}` : ""}`);
      // ANTIVIRUS и коды ОС окно объясняет своим текстом (install.js, installer.js).
      const code = error && error.code === "ANTIVIRUS" ? "ANTIVIRUS" : systemCode(error);
      send({ phase: "error", error: String(error && error.message).slice(0, 400),
        errorCode: code || (error && error.code) || null, errorFile: (error && error.file) || null });
      return false;
    } finally {
      busy = false;
      controller = null;
      send({ cancellable: false });
      if (closing && !closed) window.destroy();
    }
  }

  const handlers = [];
  function handle(name, fn) {
    handlers.push(name);
    ipcMain.handle(name, (event, ...args) => {
      if (event.sender !== window.webContents) throw new Error("Недоступный IPC установщика");
      return fn(...args);
    });
  }
  handle("installer-get-state", () => state);
  handle("installer-window", (command) => {
    if (command === "minimize") window.minimize();
    else if (command === "close") window.close();
  });
  handle("installer-cancel", () => {
    if (!busy || !state.cancellable || !controller) return false;
    send({ phase: "cancelling", cancellable: false });
    controller.abort();
    return true;
  });
  handle("installer-select-directory", async () => {
    if (busy || choosingDirectory || closing || !backend.selectDirectory) return false;
    choosingDirectory = true;
    try {
      const result = await dialog.showOpenDialog(window, {
        defaultPath: state.directory,
        properties: ["openDirectory", "createDirectory"],
      });
      if (closed || result.canceled || !result.filePaths.length) return false;
      await backend.selectDirectory(result.filePaths[0], ctx());
      return state;
    } finally { choosingDirectory = false; }
  });
  handle("installer-install", (desktopShortcut) => {
    if (state.phase === "done") return false;
    return exclusive((c) => backend.install(desktopShortcut === true, c), true);
  });
  handle("installer-launch", async () => {
    if (state.phase !== "done" || busy) return false;
    try {
      return await backend.launch(ctx());
    } catch (error) {
      send({ phase: "done", error: String(error && error.message).slice(0, 400) });
      return false;
    }
  });
  handle("installer-uninstall-plan", async () => {
    if (busy) return false;
    return backend.uninstallPlan(ctx());
  });
  // «Назад» после удаления: тот же каталог заново проверяет backend, как при
  // выборе папки, и окно возвращается к установке.
  handle("installer-back", async () => {
    if (busy || closing || state.phase !== "uninstalled" || state.canGoBack === false) return false;
    if (backend.back) return backend.back(ctx());
    if (backend.selectDirectory) {
      try {
        await backend.selectDirectory(state.directory, ctx());
        return state;
      } catch (_) { /* остатки в каталоге: установка сама объяснит, что не так */ }
    }
    send({ phase: "idle", update: false, installedVersion: null, canUninstall: false, error: null, warning: null, progress: null });
    return state;
  });
  // Помощь при ошибке установки. Ссылка своя: страница может только попросить
  // открыть Discord HLSR, но не передать адрес.
  handle("installer-open-discord", async () => {
    if (state.phase !== "error" || !electron.shell) return false;
    await electron.shell.openExternal(LINKS.discord);
    return true;
  });
  handle("installer-uninstall", (removeGames) => {
    if (!state.canUninstall) return false;
    return exclusive((c) => backend.uninstall(removeGames === true, c));
  });

  window.on("close", (event) => {
    if (!busy) return;
    event.preventDefault();
    closing = true;
    if (controller && state.cancellable) {
      send({ phase: "cancelling", cancellable: false });
      controller.abort();
    }
  });
  window.on("closed", () => {
    closed = true;
    if (controller) controller.abort();
    for (const name of handlers) ipcMain.removeHandler(name);
  });
  window.loadFile(path.join(assets, "index.html"));
  return window;
}

module.exports = { openInstallerWindow };
