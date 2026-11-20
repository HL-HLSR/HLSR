"use strict";

const { contextBridge, ipcRenderer, webUtils } = require("electron");

const INVOKE_CHANNELS = new Set([
  "boot-gate-run",
  "diagnostics-cancel",
  "diagnostics-run",
  "discord-rpc-get",
  "discord-rpc-set",
  "ui-scale-apply",
  "demo-add-paths",
  "demo-bxtrs-cancel",
  "demo-bxtrs-install",
  "demo-bxtrs-status",
  "demo-cancel",
  "demo-encoders",
  "demo-estimate",
  "demo-export-start",
  "demo-ffmpeg-cancel",
  "demo-ffmpeg-install",
  "demo-ffmpeg-status",
  "demo-inspect",
  "demo-list",
  "demo-media-url",
  "demo-options",
  "demo-player-fullscreen",
  "demo-output-pick",
  "demo-pick-files",
  "demo-plan",
  "demo-preview-clear",
  "demo-preview-start",
  "demo-remove",
  "demo-rendered-delete",
  "demo-rendered-reveal",
  "demo-rendered-list",
  "demo-rendered-open",
  "demo-reorder",
  "demo-reveal",
  "demo-settings-get",
  "demo-timeline-read",
  "download-settings-get",
  "download-settings-set",
  "export-diagnostics",
  "export-diagnostics-zip",
  "friends-avatars",
  "friends-list",
  "game-build-export",
  "game-build-export-settings",
  "game-build-import",
  "game-build-import-confirm",
  "game-build-export-confirm",
  "game-cfg-file-read",
  "game-cfg-file-write",
  "game-cfg-files-list",
  "game-config-info",
  "game-config-open-folder",
  "game-config-read",
  "game-config-write",
  "game-crash-info",
  "game-crash-open-dump",
  "game-delivery-get-status",
  "game-delivery-info",
  "game-delivery-install",
  "game-delivery-cancel",
  "game-delivery-pause",
  "game-delivery-selected",
  "game-launch-install-sdk",
  "game-launch-open-steam",
  "game-launch-restart-elevated",
  "game-launch-steam-status",
  "game-library-migrate",
  "game-menu-apply-update",
  "game-menu-check-updates",
  "game-menu-create-shortcut",
  "game-menu-open-folder",
  "game-menu-open-screenshots",
  "game-menu-repair",
  "game-menu-state",
  "game-menu-uninstall",
  "game-menu-verify-files",
  "game-process-launch",
  "game-process-stop",
  "game-release-notes",
  "game-release-notes-all",
  "game-runs-list",
  "game-runs-delete",
  "game-runs-open-folder",
  "game-runs-reveal",
  "game-demos-list",
  "game-demos-pack",
  "game-demos-archive-reveal",
  "game-select-build",
  "game-episodes-status",
  "game-episode-install",
  "game-episode-remove",
  "game-settings-get",
  "game-settings-save",
  "livesplit-overlay-capture",
  "game-personalization-pick-asset",
  "player-config-import-pick",
  "player-config-import-apply",
  "player-config-import-undo",
  "player-config-export",
  "player-config-check",
  "player-config-setup-state",
  "player-config-setup-done",
  "game-personalization-preview-asset",
  "custom-packs-get",
  "custom-packs-set",
  "custom-packs-sound",
  "ghosting-screen-get",
  "game-personalization-code-export",
  "game-personalization-code-import",
  "game-stats-get",
  "get-library-path",
  "notifications-clear",
  "notifications-list",
  "notifications-mark-all-read",
  "notifications-mark-read",
  "open-discord",
  "open-external-link",
  "hl1-guide-open-link",
  "open-games-folder",
  "open-logs-folder",
  "profile-get",
  "runner-state",
  "runner-sync",
  "runner-login",
  "runner-login-cancel",
  "runner-logout",
  "runner-logout-all",
  "runner-search",
  "runner-players",
  "runner-profile-view",
  "runner-rate",
  "runner-customize",
  "runner-speedrun-link",
  "runner-discord-code",
  "runner-discord-unlink",
  "runner-telegram-code",
  "runner-telegram-unlink",
  "runner-speedrun-unlink",
  "runner-speedrun-bests",
  "runner-speedrun-pin",
  "runner-speedrun-unpin",
  "runner-secret-unlock",
  "runner-delete-challenge",
  "runner-delete-request",
  "runner-delete-cancel",
  "runner-open-link",
  "proton-add",
  "proton-cancel",
  "proton-download",
  "proton-select",
  "proton-status",
  "proton-update",
  "public-online-get",
  "public-news-get",
  "public-sponsors-get",
  "public-style-presets-get",
  "public-news-react",
  "public-news-mine",
  "public-news-open-link",
  "sponsor-open-link",
  "support-open",
  "select-install-folder",
  "set-library-path",
  "steam-account-get",
  "steam-health-get",
  "steam-open-profile",
  "telemetry-reset-identity",
  "tool-install",
  "tool-cancel",
  "tool-launch",
  "tool-list",
  "tool-open-folder",
  "tool-open-help",
  "tool-remove",
  "tool-splits-set",
  "tool-splits-presets",
  "tool-stop",
  "tray-hide-while-playing-set",
  "updater-check",
  "updater-cancel",
  "updater-download",
  "updater-get-state",
  "updater-install-now",
  "updater-install-on-quit",
  "updater-set-language",
  "validate-install-path",
  "window-command",
]);

const SEND_CHANNELS = new Set(["ready", "support-thanks-ready", "discord-rpc-activity"]);
const SEND_SYNC_CHANNELS = new Set([
  "app-initial-language",
  "app-official",
  "app-version",
  "native-available",
  "umu-available",
]);
const EVENT_CHANNELS = new Set([
  "achievement-unlocked",
  "demo-bxtrs-progress",
  "demo-ffmpeg-progress",
  "demo-job-done",
  "demo-job-error",
  "demo-job-progress",
  "demo-player-fullscreen-changed",
  "launcher-fullscreen-changed",
  "support-thanks",
  "diagnostics-progress",
  "discord-rpc-state",
  "game-build-import-applied",
  "game-delivery-status",
  "game-demos-pack-progress",
  "game-crash-report",
  "game-process-crashed",
  "game-process-exited",
  "game-process-running",
  "notifications-changed",
  "public-online",
  "runner-profile-state",
  "runner-login-success",
  "proton-state",
  "select-game",
  "steam-account",
  "steam-health",
  "tool-progress",
  "tool-status",
  "updater-state",
  "window-visibility",
]);

const listenerWrappers = new Map();

function requireChannel(allowed, channel) {
  if (typeof channel !== "string" || !allowed.has(channel)) {
    throw new Error(`Blocked IPC channel: ${String(channel)}`);
  }
  return channel;
}

function wrapperMap(channel) {
  let listeners = listenerWrappers.get(channel);
  if (!listeners) {
    listeners = new Map();
    listenerWrappers.set(channel, listeners);
  }
  return listeners;
}

const ipc = {
  invoke(channel, ...args) {
    return ipcRenderer.invoke(requireChannel(INVOKE_CHANNELS, channel), ...args);
  },
  send(channel, ...args) {
    ipcRenderer.send(requireChannel(SEND_CHANNELS, channel), ...args);
  },
  sendSync(channel, ...args) {
    return ipcRenderer.sendSync(requireChannel(SEND_SYNC_CHANNELS, channel), ...args);
  },
  on(channel, listener) {
    requireChannel(EVENT_CHANNELS, channel);
    if (typeof listener !== "function") throw new TypeError("IPC listener must be a function");

    const listeners = wrapperMap(channel);
    const previous = listeners.get(listener);
    if (previous) ipcRenderer.removeListener(channel, previous);

    const wrapped = (_event, ...args) => listener(undefined, ...args);
    listeners.set(listener, wrapped);
    ipcRenderer.on(channel, wrapped);
  },
  removeListener(channel, listener) {
    requireChannel(EVENT_CHANNELS, channel);
    const listeners = listenerWrappers.get(channel);
    const wrapped = listeners && listeners.get(listener);
    if (!wrapped) return;

    ipcRenderer.removeListener(channel, wrapped);
    listeners.delete(listener);
    if (!listeners.size) listenerWrappers.delete(channel);
  },
};

const store = {
  read(configName) {
    return ipcRenderer.sendSync("renderer-store-read", configName);
  },
  write(configName, data) {
    return ipcRenderer.sendSync("renderer-store-write", configName, data);
  },
  quarantine(configName) {
    return ipcRenderer.sendSync("renderer-store-quarantine", configName);
  },
};

contextBridge.exposeInMainWorld("hlsrElectron", {
  // "win32" или "linux": интерфейс показывает разделы только своей ОС.
  platform: process.platform,
  defaultLibraryPath: ipcRenderer.sendSync("app-default-library-path"),
  ipcRenderer: ipc,
  clipboard: {
    writeText(value) {
      const text = String(value || "");
      if (text.length > 1024 * 1024) return Promise.reject(new Error("Clipboard text is too large"));
      return ipcRenderer.invoke("clipboard-write-text", text);
    },
  },
  store,
  // Electron 32+: у File из перетаскивания нет .path. Путь проверяет главный
  // процесс (demo-add-paths), здесь он только достаётся из File.
  files: {
    pathForFile(file) {
      try {
        return webUtils.getPathForFile(file) || null;
      } catch (error) {
        return null;
      }
    },
  },
});
