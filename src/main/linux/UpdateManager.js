"use strict";

const { app, ipcMain } = require("electron");
const fs = require("fs");
const path = require("path");
const { download: downloadInstaller } = require("../launcher-update/ArchiveDownload");
const embeddedPayload = require("../launcher-update/embedded-payload");
const log = require("electron-log");
const LauncherUpdateGate = require("../launcher-update/LauncherUpdateGate");
const ArchiveInstall = require("../launcher-update/ArchiveInstall");
const localQa = require("../launcher-update/local-qa");
const chunks = require("../launcher-update/chunks");
const launcherChannel = require("../launcher-update/launcher-channel");

const CHECK_INTERVAL_MS = 30 * 60 * 1000;
const BOOT_TIMEOUT_MS = 10000;

function errorCode(error) {
  const text = String(error && error.message || error);
  if (/429|rate.?limit/i.test(text)) return "RATE_LIMIT";
  if (/ENOSPC/i.test(text)) return "NO_SPACE";
  if (/EACCES|EPERM|EROFS/i.test(text)) return "ACCESS";
  if (/sha|подпис|метадан|AppImage изменился/i.test(text)) return "VERIFY";
  if (/HTTP|feed/i.test(text)) return "FEED";
  if (/timeout|таймаут|ENOTFOUND|ECONN|ETIMEDOUT/i.test(text)) return "NETWORK";
  return "UNKNOWN";
}

class UpdateManager {
  constructor(window, options = {}) {
    this.app = options.app || app;
    const qa = localQa.configuration(this.app.getVersion(), log);
    this.downloadInstaller = options.download || (qa && qa.download) || downloadInstaller;
    this.extractPayload = options.extractPayload || embeddedPayload.extractFrom;
    this.installRoot = options.installRoot || ArchiveInstall.managedRoot(process.execPath);
    this.ipc = options.ipc || ipcMain;
    this.window = window;
    this.gate = options.gate || (qa && qa.gate) || new LauncherUpdateGate({ currentVersion: this.app.getVersion(), logger: log });
    this.createInstaller = options.createInstaller || (() => new ArchiveInstall(this.installRoot));
    this.fetchChunk = options.fetchChunk;
    this.enabled = this.app.isPackaged && !!this.installRoot;
    this.state = "idle";
    this.info = null;
    this.progress = null;
    this.error = null;
    this.errorCode = null;
    this.isGameRunning = () => false;
    this.checkSource = "automatic";
    this.installOnQuit = false;
    this.installRequested = false;
    this.lastCheckAt = 0;
    this.backoffMs = 0;
    this.backoffUntil = 0;
    this.disposed = false;

  }

  setGameRunningCheck(fn) { this.isGameRunning = fn; }
  setNotifications(store) { this.notifications = store; }
  setVersionEvents(events) { this.versionEvents = events; }
  setUpdateTelemetry(events) { this.updateTelemetry = events; }
  setWindow(window) { this.window = window; }

  telemetry(method, ...args) {
    try { if (this.updateTelemetry) this.updateTelemetry[method](...args); }
    catch (error) { log.warn(`[updater] telemetry: ${error.message}`); }
  }

  snapshot() {
    return {
      state: this.state, enabled: this.enabled, currentVersion: this.app.getVersion(),
      version: this.info && this.info.version, releaseNotes: this.info && this.info.releaseNotes,
      info: this.info, progress: this.progress, error: this.error, errorCode: this.errorCode,
      checkSource: this.checkSource,
      cancellable: !!this.abortController && ["downloading", "verifying"].includes(this.state),
    };
  }

  send(state, extra = {}) {
    Object.assign(this, extra, { state });
    const value = this.snapshot();
    if (!this.disposed && this.window && !this.window.isDestroyed()) this.window.webContents.send("updater-state", value);
    return value;
  }

  initialize() {
    this.ipc.handle("updater-get-state", () => this.snapshot());
    this.ipc.handle("updater-check", () => this.checkUpdates("manual"));
    this.ipc.handle("updater-download", () => this.download());
    this.ipc.handle("updater-cancel", () => this.cancelDownload());
    this.ipc.handle("updater-install-now", () => this.installNow());
    this.ipc.handle("updater-install-on-quit", () => this.installLater());
    this.ipc.handle("updater-set-language", (_event, language) => language === "en" ? "en" : "ru");
    this.ipc.handle("boot-gate-run", (_event, options) => this.runBootGate(options));
    this.beforeQuit = (event) => {
      if (this.abortController) {
        event.preventDefault();
        this.abortController.abort();
        if (!this.quitPending) {
          this.quitPending = true;
          this.downloadPromise.finally(() => this.app.quit());
        }
        return;
      }
      if (!this.installOnQuit || this.installRequested || this.isGameRunning()) return;
      if (!this.applyUpdate(false)) event.preventDefault();
    };
    this.app.on("before-quit", this.beforeQuit);
  }

  handleError(error) {
    const code = errorCode(error);
    this.backoffMs = Math.min(this.backoffMs ? this.backoffMs * 2 : 60000, CHECK_INTERVAL_MS);
    this.backoffUntil = Date.now() + (code === "RATE_LIMIT" ? 15 * 60000 : this.backoffMs);
    this.installOnQuit = false;
    this.send("error", { error: String(error && error.message || error).slice(0, 500), errorCode: code, progress: null });
    this.telemetry("failed", this.app.getVersion(), this.info && this.info.version, code);
  }

  checkUpdates(source = "automatic") {
    if (!this.enabled || this.disposed || this.checkPromise || ["downloading", "verifying", "downloaded"].includes(this.state)) return false;
    if (source !== "manual" && (Date.now() < this.backoffUntil || Date.now() - this.lastCheckAt < (source === "focus" ? 5 : 10) * 60000)) return false;
    this.checkSource = source === "manual" ? "manual" : "automatic";
    this.lastCheckAt = Date.now();
    this.checkPromise = this.performCheck().catch((error) => this.handleError(error))
      .finally(() => { this.checkPromise = null; });
    return true;
  }

  async performCheck() {
    this.send("checking", { error: null, errorCode: null, info: null });
    this.gateResult = null;
    const result = await this.gate.evaluate();
    this.gateResult = result;
    if (!result.ok) throw new Error(`метаданные: ${result.problems.join("; ")}`);
    if (this.disposed) return;
    this.backoffMs = 0;
    this.backoffUntil = 0;
    if (!result.updateAvailable) { this.send("not-available"); return; }
    const info = result.manifest;
    if (this.disposed) return;
    this.info = {
      version: info.version, releaseDate: info.publishedAt,
      releaseNotes: typeof info.releaseNotes === "string" ? info.releaseNotes.replace(/<[^>]*>/g, " ").slice(0, 4000) : "",
    };
    this.send("available");
    this.telemetry("available", this.app.getVersion(), info.version);
    try {
      if (this.versionEvents) this.versionEvents.launcherUpdateAvailable(info.version, this.app.getVersion(), { hasReleaseNotes: !!this.info.releaseNotes });
    } catch (error) { log.warn(`[updater] notification: ${error.message}`); }
  }

  download() {
    if (!this.enabled || this.disposed || this.state !== "available" || !this.gateResult || !this.gateResult.ok) return false;
    this.checkSource = "manual";
    this.downloadPromise = this.performDownload().catch((error) => this.handleError(error));
    return true;
  }

  cancelDownload() {
    if (!this.abortController || !["downloading", "verifying"].includes(this.state)) return false;
    this.cancelRequested = true;
    this.abortController.abort();
    return true;
  }

  async performDownload() {
    this.cancelRequested = false;
    const manifest = this.gateResult.manifest;
    this.send("downloading", { progress: null });
    this.telemetry("downloadStarted", this.app.getVersion(), manifest.version);
    const started = Date.now();
    // Установщик, вшитый архив и распаковка вместе занимают сотни мегабайт:
    // качаем рядом с установкой, а не в /tmp (tmpfs, бывает noexec).
    const directory = this.downloadDirectory();
    this.abortController = new AbortController();
    const signal = this.abortController.signal;
    try {
      const onProgress = (progress) => { if (!this.disposed) this.send("downloading", { progress }); };
      let bytes = manifest.installer.size;
      const delta = launcherChannel.deltaOf(manifest);
      let staged = false;
      if (delta) {
        try {
          bytes = await this.downloadChunks(manifest, delta, directory, signal, onProgress);
          staged = true;
        } catch (error) {
          if (signal.aborted || this.disposed) throw error;
          if (this.installer) this.installer.discard();
          log.warn(`[updater] чанки не сработали, качаю установщик целиком: ${error.message}`);
          this.send("downloading", { progress: null });
        }
      }
      if (!staged) await this.downloadFull(manifest, directory, signal, onProgress);
      if (this.disposed) { if (this.installer) this.installer.discard(); return; }
      signal.throwIfAborted();
      this.send("downloaded");
      this.telemetry("downloadCompleted", this.app.getVersion(), manifest.version, { bytes, durationMs: Date.now() - started });
    } catch (error) {
      if (!this.cancelRequested) throw error;
      if (this.installer) this.installer.discard();
      this.send("available", { progress: null, error: null, errorCode: null });
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
      this.abortController = null;
    }
  }

  // Обновление чанками: из установленной версии берётся всё совпавшее,
  // скачиваются только недостающие куски. Возвращает скачанные байты.
  async downloadChunks(manifest, delta, directory, signal, onProgress) {
    const chunkManifest = await chunks.fetchManifest(delta, {
      isAllowedUrl: launcherChannel.isAllowedUrl, version: manifest.version, platform: "linux",
      signal, fetch: this.fetchChunk,
    });
    if (this.installer) this.installer.discard();
    this.installer = this.createInstaller();
    await this.installer.prepareFromChunks(chunkManifest, manifest, {
      cacheDir: path.join(directory, "chunks"), chunkBaseUrls: delta.chunkBaseUrls,
      fetch: this.fetchChunk, signal, onProgress, log,
    });
    const result = this.installer.chunkResult || {};
    log.info(`[updater] ${manifest.version} собрана из чанков: скачано ${result.downloadedBytes} байт, с диска ${result.reusedBytes}`);
    return result.downloadedBytes || 0;
  }

  async downloadFull(manifest, directory, signal, onProgress) {
    const file = await this.downloadInstaller(manifest, directory, { signal, onProgress });
    if (this.disposed) return;
    this.send("verifying", { progress: null });
    // AppImage уже сверен с подписанным каналом; из него берём вшитый архив
    // с описанием, а сам установщик удаляем, чтобы не держать лишнее место.
    const payload = await this.extractPayload(file, directory, { signal });
    fs.rmSync(file, { force: true });
    if (payload.manifest.version !== manifest.version)
      throw new Error(`Установщик содержит HLSR ${payload.manifest.version}, а подписанный канал — ${manifest.version}`);
    signal.throwIfAborted();
    if (this.installer) this.installer.discard();
    this.installer = this.createInstaller();
    await this.installer.prepare(payload.archive, payload.manifest, { signal });
  }

  downloadDirectory() {
    const base = path.join(this.installRoot, ".downloads");
    // Остатки прерванных загрузок: одновременно работает только один лаунчер.
    fs.rmSync(base, { recursive: true, force: true });
    fs.mkdirSync(base, { recursive: true, mode: 0o700 });
    return fs.mkdtempSync(path.join(base, "update-"));
  }

  applyUpdate(relaunch) {
    if (!this.enabled || this.state !== "downloaded" || !this.installer || this.installRequested || this.isGameRunning()) return false;
    let committed = false;
    try {
      const target = this.installer.commit();
      committed = true;
      if (relaunch) this.app.relaunch({ execPath: target, args: relaunchArguments(process.argv) });
      this.installRequested = true;
      try {
        if (this.versionEvents) this.versionEvents.writePendingReceipt(this.app.getVersion(), this.info.version);
      } catch (error) { log.warn(`[updater] receipt: ${error.message}`); }
      return true;
    } catch (error) {
      if (committed) {
        try { this.installer.rollback(); } catch (rollbackError) { log.error(`[updater] rollback: ${rollbackError.message}`); }
      }
      this.handleError(error);
      return false;
    }
  }

  installNow() {
    if (!this.applyUpdate(true)) return false;
    setImmediate(() => this.app.quit());
    return true;
  }

  installLater() {
    if (!this.enabled || this.state !== "downloaded" || !this.installer) return false;
    this.installOnQuit = true;
    return true;
  }

  runBootGate(options = {}) {
    if (options.retry && this.bootDecision && this.bootDecision.action === "blocked") this.bootPromise = null;
    if (!this.bootPromise) this.bootPromise = this.executeBootGate().then((decision) => {
      this.bootDecision = decision;
      return decision;
    });
    return this.bootPromise;
  }

  async executeBootGate() {
    if (!this.enabled) return { action: "proceed", reason: "updates-disabled" };
    this.checkUpdates("manual");
    let timer;
    const timeout = new Promise((resolve) => { timer = setTimeout(() => resolve("timeout"), BOOT_TIMEOUT_MS); });
    const result = await Promise.race([Promise.resolve(this.checkPromise), timeout]);
    clearTimeout(timer);
    if (result === "timeout") return { action: "proceed", reason: "check-timeout" };
    if (this.isGameRunning()) return { action: "proceed", reason: "game-running" };
    if (this.state === "available") {
      this.download();
      await this.downloadPromise;
    }
    if (this.state === "downloaded" && this.installNow()) return { action: "installing", reason: "installing", version: this.info.version };
    const mandatory = this.gateResult && (this.gateResult.updateRequired || this.gateResult.unsupported);
    return this.state === "error" && mandatory
      ? { action: "blocked", reason: "mandatory-update-failed", error: this.error }
      : { action: "proceed", reason: this.state };
  }

  checkOnFocus() { this.checkUpdates("focus"); }

  startCheckingInterval() {
    if (!this.enabled || this.timer) return;
    const schedule = () => {
      this.timer = setTimeout(() => { this.checkUpdates(); schedule(); }, CHECK_INTERVAL_MS + Math.floor(Math.random() * 120000));
      if (this.timer.unref) this.timer.unref();
    };
    schedule();
    if (!this.bootPromise) this.checkUpdates();
  }

  dispose() {
    this.disposed = true;
    if (this.timer) clearTimeout(this.timer);
    this.app.removeListener("before-quit", this.beforeQuit);
    if (this.abortController) this.abortController.abort();
    if (this.installer && !this.abortController) this.installer.discard();
  }
}

module.exports = UpdateManager;
// Сохраняем выбранную человеком платформу окна и остальные параметры.
function relaunchArguments(argv) {
  return ["--no-sandbox", ...argv.slice(1).filter((arg) => arg !== "--no-sandbox")];
}
module.exports.relaunchArguments = relaunchArguments;
