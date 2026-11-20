import { app, ipcMain } from "electron";
import { autoUpdater } from "electron-updater";
import log from "electron-log";
import fs from "fs";
import path from "path";
const { CancellationToken } = require("builder-util-runtime");

const LauncherUpdateGate = require("./launcher-update/LauncherUpdateGate");
const bootGate = require("./launcher-update/boot-gate");
const updaterLaunch = require("./launcher-update/updater-launch");
const updaterMode = require("./launcher-update/updater-mode");
const installLocation = require("./launcher-update/install-location");
const endpoints = require("../shared/delivery-endpoints");

const CHECK_TIMEOUT_MS = 30000;

const LAUNCHER_TICK_MS = 30 * 60 * 1000;
const LAUNCHER_TICK_JITTER_MS = 2 * 60 * 1000;
const MIN_CHECK_AGE_MS = 10 * 60 * 1000;
const FOCUS_CHECK_AGE_MS = 5 * 60 * 1000;
const RATE_LIMIT_PAUSE_MS = 15 * 60 * 1000;
const BACKOFF_BASE_MS = 60 * 1000;
const BACKOFF_MAX_MS = 30 * 60 * 1000;

const BOOT_CHECK_TIMEOUT_MS = 10000;

const PRIMARY_FEED_URL = endpoints.launcher.feedUrls[0];

const FALLBACK_FEED_URL = endpoints.launcher.feedUrls[1];

const BUILD_CHANNEL = process.env.HLSR_BUILD_CHANNEL || "development";
const IS_QA_BUILD = BUILD_CHANNEL === "qa";

const LOOPBACK_FEED_RE = /^https?:\/\/(127\.0\.0\.1|localhost)(:\d+)?(\/|$)/;

function loopbackFeedOrNull(value, logger) {
  if (!value) return null;

  if (!LOOPBACK_FEED_RE.test(value)) {
    logger.warn(`Update feed override rejected (not loopback): ${value}`);
    return null;
  }

  return value;
}

const UPDATE_CONFIG_FILE = "app-update.yml";

const INSTALL_OWNER_MARKER = ".hlsr-install-owner.json";

const STATE = {
  IDLE: "idle",
  CHECKING: "checking",
  AVAILABLE: "available",
  NOT_AVAILABLE: "not-available",
  DOWNLOADING: "downloading",
  VERIFYING: "verifying",
  DOWNLOADED: "downloaded",
  ERROR: "error",
};

function classifyUpdateError(error) {
  const text = error ? String(error.message || error) : "";

  if (/\b429\b|rate ?limit/i.test(text)) return "RATE_LIMIT";
  if (/ENOTFOUND|EAI_AGAIN|ECONNRESET|ECONNREFUSED|ETIMEDOUT|ERR_INTERNET_DISCONNECTED|network/i.test(text))
    return "NETWORK";
  if (/\b(404|403|5\d\d)\b|HttpError/i.test(text)) return "FEED";
  if (/ENOSPC|no space/i.test(text)) return "NO_SPACE";
  if (/EPERM|EACCES|denied/i.test(text)) return "ACCESS";
  if (/sha512|checksum|signature|integrity/i.test(text)) return "VERIFY";

  return "UNKNOWN";
}

function sanitizeReleaseNotes(notes) {
  if (!notes) return "";

  let text = Array.isArray(notes)
    ? notes.map((n) => (n && n.note) || "").join("\n\n")
    : String(notes);

  return text
    .replace(/<\s*(script|style)[^>]*>[\s\S]*?<\s*\/\s*\1\s*>/gi, "")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/[ \t]+/g, " ")
    .replace(/\n{3,}/g, "\n\n")
    .trim()
    .slice(0, 4000);
}

class UpdateManager {
  constructor(window) {
    this.window = window;
    this.state = STATE.IDLE;
    this.info = null;
    this.progress = null;
    this.error = null;
    this.userAcceptedDownload = false;
    this.checkTimeout = null;
    this.usingFallbackFeed = false;
    this.feedFreshnessChecked = false;
    this.verifiedVersion = null;

    this.lastActualCheckAt = 0;
    this.backoffMs = 0;
    this.backoffUntil = 0;
    this.rateLimitedUntil = 0;
    this.intervalTimer = null;

    this.isGameRunning = () => false;

    this.installRequested = false;

    this.installLocationChecked = false;
    this.installLocationResult = null;

    this.gate = new LauncherUpdateGate({ currentVersion: app.getVersion(), logger: log });
    this.gateResult = null;

    this.bootMode = false;
    this.bootPromise = null;
    this.bootDecision = null;
    this.bootCompletedAt = 0;
    this.onBootState = null;

    this.notifications = null;
    this.updateTelemetry = null;
    this.pendingVersion = null;
    this.downloadStartedAt = 0;
    this.lastTransferred = 0;

    this.language = "ru";
    this.checkSource = "automatic";
    this.externalUpdaterDisabled = false;

    this.enabled = app.isPackaged && process.env.NODE_ENV === "production";
    this.buildChannel = BUILD_CHANNEL;

    autoUpdater.logger = log;
    autoUpdater.autoDownload = false;
    autoUpdater.autoInstallOnAppQuit = false;
    autoUpdater.allowPrerelease = false;
    autoUpdater.allowDowngrade = false;
    autoUpdater.fullChangelog = false;

    const qaFeed = IS_QA_BUILD
      ? loopbackFeedOrNull(process.env.HLSR_UPDATE_FEED, log)
      : null;

    if (process.env.HLSR_UPDATE_FEED && !IS_QA_BUILD)
      log.warn(
        `HLSR_UPDATE_FEED ignored: build channel is "${BUILD_CHANNEL}", not "qa"`
      );

    if (qaFeed) {
      log.warn(`QA channel: using loopback update feed ${qaFeed}`);
      autoUpdater.setFeedURL({ provider: "generic", url: qaFeed });
      this.enabled = true;

      const base = qaFeed.replace(/\/+$/, "");
      const qaKey = process.env.HLSR_UPDATE_CHANNEL_KEY;

      this.gate = new LauncherUpdateGate({
        currentVersion: app.getVersion(),
        logger: log,
        channelUrls: [`${base}/launcher-channel.json`],
        latestYmlUrls: [`${base}/latest.yml`],
        httpGet: LauncherUpdateGate.qaHttpGet,
        publicKeys: qaKey ? [qaKey.replace(/\\n/g, "\n")] : undefined,
      });
    } else if (this.enabled) {
      autoUpdater.setFeedURL({ provider: "generic", url: PRIMARY_FEED_URL });
      log.info(`Update feed: ${PRIMARY_FEED_URL} (fallback: ${FALLBACK_FEED_URL})`);
    }
  }

  setGameRunningCheck(fn) {
    if (typeof fn === "function") this.isGameRunning = fn;
  }

  setNotifications(store) {
    this.notifications = store;
  }

  setVersionEvents(events) {
    this.versionEvents = events;
  }

  setUpdateTelemetry(events) {
    this.updateTelemetry = events;
  }

  telemetry(method, ...args) {
    if (!this.updateTelemetry) return;

    try {
      this.updateTelemetry[method](...args);
    } catch (error) {
      log.warn(`[telemetry] событие обновления ${method} не записано: ${error.message}`);
    }
  }

  updateConfigState() {
    try {
      const exeDir = path.dirname(app.getPath("exe"));

      if (fs.existsSync(path.join(exeDir, "resources", UPDATE_CONFIG_FILE))) return "ok";

      return fs.existsSync(path.join(exeDir, INSTALL_OWNER_MARKER))
        ? "missing-installed"
        : "missing-local";
    } catch (e) {
      return "missing-local";
    }
  }

  handleMissingUpdateConfig(err) {
    const text = String((err && err.message) || err || "");
    if (!/app-update\.yml/i.test(text)) return false;

    if (this.updateConfigState() === "missing-installed") {
      this.notify(
        "error",
        "LU_UPDATE_CONFIG_MISSING",
        "В установленной сборке нет конфигурации обновлений"
      );
      this.send({ state: STATE.ERROR, error: "LU_UPDATE_CONFIG_MISSING", errorCode: "CONFIG" });
      return true;
    }

    log.info("[updater] локальная сборка без конфигурации обновлений: проверка пропущена");

    this.send({ state: STATE.NOT_AVAILABLE, error: null });
    return true;
  }

  notify(level, code, detail) {
    if (!this.notifications) return;
    try {
      this.notifications.add({ level, code, detail });
    } catch (e) {
      /* уведомление не должно ломать обновление */
    }
  }

  setWindow(window) {
    this.window = window;
  }

  send(patch) {
    Object.assign(this, patch);

    if (this.window && !this.window.isDestroyed())
      this.window.webContents.send("updater-state", {
        state: this.state,
        enabled: this.enabled,
        currentVersion: app.getVersion(),
        version: this.info ? this.info.version : null,
        releaseDate: this.info ? this.info.releaseDate : null,
        releaseNotes: this.info ? this.info.releaseNotes : null,
        progress: this.progress,
        error: this.error,
        errorCode: this.errorCode || null,
        checkSource: this.checkSource,
      });

    if (this.onBootState) {
      try {
        this.onBootState(this.state);
      } catch (e) {
        log.error("Boot gate state handler failed:", e.message);
      }
    }
  }

  clearCheckTimeout() {
    if (this.checkTimeout) {
      clearTimeout(this.checkTimeout);
      this.checkTimeout = null;
    }
  }

  maybeUseFallbackFeed() {
    if (this.usingFallbackFeed || !this.enabled) return false;
    if (IS_QA_BUILD) return false;

    this.usingFallbackFeed = true;
    log.warn(`Switching launcher update feed to fallback: ${FALLBACK_FEED_URL}`);
    autoUpdater.setFeedURL({ provider: "generic", url: FALLBACK_FEED_URL });
    return true;
  }

  restorePrimaryFeed() {
    if (!this.usingFallbackFeed || !this.enabled || IS_QA_BUILD) return false;

    log.warn(`Fallback feed failed too — back to primary: ${PRIMARY_FEED_URL}`);
    autoUpdater.setFeedURL({ provider: "generic", url: PRIMARY_FEED_URL });
    return true;
  }

  checkFeedFreshness() {
    if (this.feedFreshnessChecked || !this.enabled || !this.gate) return;
    this.feedFreshnessChecked = true;

    Promise.resolve()
      .then(() => this.gate.probeLatest())
      .then((outcome) => {
        if (!outcome || !outcome.ok || !outcome.updateAvailable) return;

        const version = outcome.manifest ? outcome.manifest.version : null;
        if (!version) return;

        log.warn(
          `Feed says no update, but the signed channel offers ${version} ` +
            `(installed ${app.getVersion()}) — latest.yml on the host is stale`
        );

        this.notify("warning", "LU_FEED_STALE", version);
      })
      .catch((error) => {
        log.warn(`Feed freshness probe failed: ${error ? error.message : error}`);
      });
  }

  initialize() {
    log.info(
      `App start: HLSR ${app.getVersion()} | packaged=${app.isPackaged} | ` +
        `updates=${this.enabled ? "enabled" : "disabled"}`
    );

    autoUpdater.on("checking-for-update", () => {
      log.info("Update check started");
      this.send({ state: STATE.CHECKING, error: null });
    });

    autoUpdater.on("update-available", async (info) => {
      this.clearCheckTimeout();
      log.info(`Update available: ${info.version}`);

      this.backoffMs = 0;
      this.backoffUntil = 0;

      let gateResult;
      try {
        gateResult = await this.gate.evaluate(info.version);
      } catch (error) {
        gateResult = { ok: false, problems: [error ? error.message : "unknown"] };
      }

      this.gateResult = gateResult;
      this.pendingVersion = info.version;
      this.downloadStartedAt = 0;
      this.lastTransferred = 0;

      if (!gateResult.ok) {
        log.warn(
          "Launcher update rejected by signed metadata: " + gateResult.problems.join("; ")
        );

        this.notify(
          "error",
          gateResult.code || "LAUNCHER_UPDATE_ERROR",
          gateResult.problems[0] || "rejected"
        );

        this.send({
          state: STATE.ERROR,
          error: `metadata: ${gateResult.problems[0] || "rejected"}`.slice(0, 500),
          errorCode: "METADATA",
        });

        this.telemetry("failed", app.getVersion(), info.version, gateResult.code);
        return;
      }

      this.send({
        state: STATE.AVAILABLE,
        info: {
          version: info.version,
          releaseDate: info.releaseDate,
          releaseNotes: sanitizeReleaseNotes(info.releaseNotes),
        },
      });

      this.telemetry("available", app.getVersion(), info.version);

      if (this.versionEvents)
        try {
          this.versionEvents.launcherUpdateAvailable(info.version, app.getVersion(), {
            hasReleaseNotes: !!sanitizeReleaseNotes(info.releaseNotes),
          });
        } catch (error) {
          log.warn(`[versions] уведомление о версии не создано: ${error.message}`);
        }
    });

    autoUpdater.on("update-not-available", () => {
      this.clearCheckTimeout();
      log.info("No update available");
      this.backoffMs = 0;
      this.backoffUntil = 0;
      this.send({ state: STATE.NOT_AVAILABLE, info: null });

      this.checkFeedFreshness();
    });

    autoUpdater.on("download-progress", (p) => {
      this.send({
        state: STATE.DOWNLOADING,
        progress: {
          percent: p.percent,
          transferred: p.transferred,
          total: p.total,
          bytesPerSecond: p.bytesPerSecond,
        },
      });

      if (this.gateResult && this.gateResult.ok && p.transferred > 0) {
        this.telemetry("downloadStarted", app.getVersion(), this.pendingVersion);
        if (!this.downloadStartedAt) this.downloadStartedAt = Date.now();
      }

      this.lastTransferred = p.transferred;

      if (this.window && !this.window.isDestroyed())
        this.window.setProgressBar(p.percent / 100);
    });

    autoUpdater.on("update-downloaded", async (info) => {
      log.info(`Update downloaded: ${info.version}`);

      if (this.window && !this.window.isDestroyed())
        this.window.setProgressBar(-1);

      autoUpdater.autoInstallOnAppQuit = false;
      this.send({ state: STATE.VERIFYING });

      const filePath = info ? info.downloadedFile : null;

      let verdict;
      try {
        verdict = filePath
          ? await this.gate.verifyInstaller(filePath, { expectedVersion: info.version })
          : { ok: false, problems: ["нет пути к скачанному файлу"], code: "LU_NO_FILE" };
      } catch (error) {
        verdict = {
          ok: false,
          problems: [error ? error.message : "unknown"],
          code: "LU_VERIFY_FAILED",
        };
      }

      if (!verdict.ok) {
        autoUpdater.autoInstallOnAppQuit = false;
        this.verifiedVersion = null;

        log.error(
          `Downloaded installer rejected [${verdict.code || "?"}]: ` +
            verdict.problems.join("; ")
        );

        this.notify(
          "error",
          verdict.code || "LAUNCHER_UPDATE_ERROR",
          verdict.problems[0] || "rejected"
        );

        this.send({
          state: STATE.ERROR,
          error: `${verdict.code || "verify"}: ${verdict.problems[0] || "rejected"}`.slice(0, 500),
          errorCode: "VERIFY",
        });

        this.telemetry("failed", app.getVersion(), info.version, verdict.code);
        return;
      }

      log.info("Downloaded installer verified against signed metadata");
      this.verifiedVersion = info.version;

      this.telemetry("downloadCompleted", app.getVersion(), info.version, {
        bytes: this.lastTransferred || null,
        durationMs: this.downloadStartedAt ? Date.now() - this.downloadStartedAt : null,
      });

      this.armInstallOnQuit(this.userAcceptedDownload);

      this.send({ state: STATE.DOWNLOADED });
    });

    autoUpdater.on("error", (err) => {
      this.clearCheckTimeout();
      log.error("Updater error:", err ? err.message : err);

      if (this.window && !this.window.isDestroyed())
        this.window.setProgressBar(-1);

      if (this.handleMissingUpdateConfig(err)) return;

      if (this.state === STATE.CHECKING && this.maybeUseFallbackFeed()) {
        this.checkUpdates({ manual: true, reason: "fallback" });
        return;
      }

      if (this.state === STATE.CHECKING) this.restorePrimaryFeed();

      const text = err ? String(err.message || err) : "";

      if (/\b429\b|rate ?limit/i.test(text)) {
        this.rateLimitedUntil = Date.now() + RATE_LIMIT_PAUSE_MS;
        log.warn("Update feed rate limited — checks paused");
      } else if (this.state === STATE.CHECKING) {
        this.backoffMs = Math.min(this.backoffMs ? this.backoffMs * 2 : BACKOFF_BASE_MS, BACKOFF_MAX_MS);
        this.backoffUntil = Date.now() + this.backoffMs;
      }

      const message = err ? String(err.message || err).slice(0, 500) : "unknown";
      if (this.state === STATE.CHECKING)
        this.notify("warning", "LU_FEED_UNAVAILABLE", message);
      else this.notify("error", "LAUNCHER_UPDATE_ERROR", message);

      if (this.state !== STATE.CHECKING)
        this.telemetry("failed", app.getVersion(), this.pendingVersion, "LAUNCHER_UPDATE_ERROR");

      this.send({ state: STATE.ERROR, error: message, errorCode: classifyUpdateError(err) });
    });

    ipcMain.handle("updater-check", () => this.checkUpdates({ manual: true, reason: "manual" }));

    autoUpdater.on("update-cancelled", () => {
      this.userAcceptedDownload = false;
      this.downloadCancellation = null;
      if (this.window && !this.window.isDestroyed()) this.window.setProgressBar(-1);
      this.send({ state: STATE.AVAILABLE, progress: null, error: null, errorCode: null });
    });
    ipcMain.handle("updater-cancel", () => {
      if (this.state !== STATE.DOWNLOADING || !this.downloadCancellation) return false;
      this.downloadCancellation.cancel();
      return true;
    });

    ipcMain.handle("updater-download", async () => {
      if (!this.enabled) return false;

      if (
        this.state === STATE.DOWNLOADING ||
        this.state === STATE.VERIFYING ||
        this.state === STATE.DOWNLOADED
      )
        return false;

      const retryAfterError = this.state === STATE.ERROR && !!this.pendingVersion;

      if (this.state !== STATE.AVAILABLE && !retryAfterError) {
        log.info(`updater-download отклонён: состояние ${this.state}, качать нечего`);
        return false;
      }

      log.info("User accepted update download");

      // The user is waiting for this download. Keep a failure visible even if
      // the update was found by a background check.
      this.checkSource = "manual";

      if (this.canUseExternalUpdater()) {
        const started = await this.startExternalUpdate();
        if (started) return "external";

        log.warn("External updater unavailable, falling back to electron-updater");
      }

      this.userAcceptedDownload = true;
      this.send({ state: STATE.DOWNLOADING, progress: null });

      this.downloadCancellation = new CancellationToken();
      autoUpdater.downloadUpdate(this.downloadCancellation).catch((err) => {
        log.error("downloadUpdate failed:", err ? err.message : err);
      });

      return true;
    });

    ipcMain.handle("boot-gate-run", (event, options) => this.runBootGate(options));

    ipcMain.handle("updater-install-now", () => {
      if (this.installRequested) {
        log.info("quitAndInstall уже запрошен — повторное нажатие проигнорировано");
        return false;
      }

      if (this.state !== STATE.DOWNLOADED) return false;

      if (!this.verifiedVersion) {
        log.warn("quitAndInstall refused: installer was not verified");
        return false;
      }

      if (this.isGameRunning()) {
        log.info("quitAndInstall deferred: game is running");
        return false;
      }

      log.info("quitAndInstall requested by user");

      if (this.versionEvents)
        try {
          this.versionEvents.writePendingReceipt(app.getVersion(), this.verifiedVersion);
        } catch (error) {
          log.warn(`[versions] receipt не сохранён: ${error.message}`);
        }

      this.installRequested = true;

      this.prepareInstallLocation();

      autoUpdater.autoInstallOnAppQuit = true;
      setImmediate(() => autoUpdater.quitAndInstall(true, true));
      return true;
    });

    ipcMain.handle("updater-install-on-quit", () => {
      if (this.state !== STATE.DOWNLOADED || !this.verifiedVersion) return false;

      log.info("Update will be installed on app quit");

      this.prepareInstallLocation();

      return this.armInstallOnQuit(true);
    });

    ipcMain.handle("updater-set-language", (event, value) => {
      this.language = value === "en" ? "en" : "ru";
      return this.language;
    });

    ipcMain.handle("updater-get-state", () => ({
      state: this.state,
      enabled: this.enabled,
      currentVersion: app.getVersion(),
      version: this.info ? this.info.version : null,
      releaseNotes: this.info ? this.info.releaseNotes : null,
      progress: this.progress,
      error: this.error,
      errorCode: this.errorCode || null,
      checkSource: this.checkSource,
    }));
  }

  armInstallOnQuit(enabled) {
    autoUpdater.autoInstallOnAppQuit = enabled === true;

    if (!enabled) return false;

    if (typeof autoUpdater.addQuitHandler !== "function") {
      log.warn("[updater] addQuitHandler недоступен — установка при выходе не сработает");
      return false;
    }

    const registerQuitHandlerLibrarySkippedWhenFlagWasOffAtDownload = () =>
      autoUpdater.addQuitHandler();

    try {
      registerQuitHandlerLibrarySkippedWhenFlagWasOffAtDownload();
    } catch (error) {
      log.warn(`[updater] обработчик выхода не зарегистрирован: ${error.message}`);
      return false;
    }

    return true;
  }

  prepareInstallLocation() {
    if (this.installLocationChecked) return this.installLocationResult;
    this.installLocationChecked = true;

    try {
      this.installLocationResult = installLocation.ensureInstallLocation({
        installDir: path.dirname(app.getPath("exe")),
        logger: log,
      });
    } catch (error) {
      log.warn(`[update] проверка InstallLocation не удалась: ${error.message}`);
      this.installLocationResult = { status: "error" };
    }

    return this.installLocationResult;
  }

  canUseExternalUpdater() {
    if (!app.isPackaged) return false;
    if (this.externalUpdaterDisabled) return false;

    const gate = this.gateResult;
    if (!gate || !gate.ok || !gate.manifest) return false;

    try {
      // Апдейтер внутри app.asar (updater-mode.js): fs Electron читает asar.
      return fs.existsSync(updaterMode.mainPath());
    } catch (error) {
      return false;
    }
  }

  buildUpdatePlan() {
    const manifest = this.gateResult.manifest;

    return {
      schema: 1,
      manifest,
      currentVersion: app.getVersion(),
      targetVersion: manifest.version,
      installDir: path.dirname(app.getPath("exe")),
      launcherExe: app.getPath("exe"),
      receiptFile: path.join(app.getPath("userData"), "version-state.json"),
      language: this.language || "ru",
    };
  }

  async startExternalUpdate() {
    if (this.isGameRunning()) {
      log.info("External updater deferred: game is running");
      this.notify("warning", "GAME_RUNNING", "обновление лаунчера отложено");
      return false;
    }

    try {
      const plan = this.buildUpdatePlan();

      const result = await updaterLaunch.launch({
        appDir: path.dirname(app.getPath("exe")),
        resourcesPath: process.resourcesPath,
        plan,
        logger: log,
      });

      log.info(`External updater started (PID ${result.pid}); quitting launcher`);

      setImmediate(() => app.quit());
      return true;
    } catch (error) {
      log.error(`External updater failed to start: ${error ? error.message : error}`);
      this.notify(
        "error",
        "LAUNCHER_UPDATE_ERROR",
        `окно обновления не запустилось: ${error ? error.message : "unknown"}`
      );

      this.externalUpdaterDisabled = true;
      return false;
    }
  }

  checkUpdates(options) {
    const opts = options || {};
    // A fallback feed is part of the same check, so keep the original source.
    if (opts.reason !== "fallback")
      this.checkSource = opts.reason === "manual" ? "manual" : "automatic";

    if (!this.enabled) {
      log.info("Update check skipped (development build)");
      this.send({ state: STATE.NOT_AVAILABLE });
      return false;
    }

    if (
      this.state === STATE.CHECKING ||
      this.state === STATE.DOWNLOADING ||
      this.state === STATE.DOWNLOADED
    )
      return false;

    const now = Date.now();

    if (now < this.rateLimitedUntil) {
      log.info("Update check skipped: rate limited");
      return false;
    }

    if (!opts.manual) {
      if (now < this.backoffUntil) return false;

      const minAge = opts.reason === "focus" ? FOCUS_CHECK_AGE_MS : MIN_CHECK_AGE_MS;
      if (this.lastActualCheckAt && now - this.lastActualCheckAt < minAge) return false;
    }

    this.lastActualCheckAt = now;

    this.clearCheckTimeout();
    this.checkTimeout = setTimeout(() => {
      if (this.state === STATE.CHECKING) {
        log.warn("Update check timed out");
        this.notify("warning", "UPDATE_CHECK_TIMEOUT", "проверка обновления лаунчера");
        this.send({ state: STATE.ERROR, error: "timeout", errorCode: "TIMEOUT" });
      }
    }, CHECK_TIMEOUT_MS);

    autoUpdater.checkForUpdates().catch((err) => {
      this.clearCheckTimeout();

      if (this.handleMissingUpdateConfig(err)) return;

      log.error("checkForUpdates failed:", err ? err.message : err);
      this.send({
        state: STATE.ERROR,
        error: err ? String(err.message || err).slice(0, 500) : "unknown",
      });
    });

    return true;
  }

  startCheckingInterval() {
    this.startBackgroundTimer();

    setImmediate(() => {
      try {
        const result = updaterLaunch.cleanupOldStages();
        if (result.removed) log.info(`[updater] убрано старых копий в TEMP: ${result.removed}`);
      } catch (error) {
        log.warn(`[updater] уборка TEMP не выполнена: ${error.message}`);
      }
    });

    if (this.bootPromise && !this.bootDecision) return;
    if (this.bootCompletedAt && Date.now() - this.bootCompletedAt < 60 * 1000) return;

    this.checkUpdates({ reason: "startup" });
  }

  startBackgroundTimer() {
    if (this.intervalTimer) return;

    const schedule = () => {
      const delay = LAUNCHER_TICK_MS + Math.floor(Math.random() * LAUNCHER_TICK_JITTER_MS);
      this.intervalTimer = setTimeout(() => {
        this.checkUpdates({ reason: "scheduled" });
        schedule();
      }, delay);
      if (typeof this.intervalTimer.unref === "function") this.intervalTimer.unref();
    };

    schedule();
  }

  checkOnFocus() {
    this.checkUpdates({ reason: "focus" });
  }

  dispose() {
    if (this.intervalTimer) {
      clearTimeout(this.intervalTimer);
      this.intervalTimer = null;
    }
    this.clearCheckTimeout();
  }

  isBootMandatory() {
    const gate = this.gateResult;
    if (!gate) return false;
    return !!(gate.updateRequired || gate.unsupported);
  }

  runBootGate(options) {
    const opts = options || {};

    if (opts.retry && this.bootDecision && this.bootDecision.action === "blocked") {
      this.bootPromise = null;
      this.bootDecision = null;
      this.send({ state: STATE.IDLE, error: null, progress: null });
    }

    if (!this.bootPromise) this.bootPromise = this.executeBootGate();
    return this.bootPromise;
  }

  finishBoot(decision) {
    this.bootMode = false;
    this.bootCompletedAt = Date.now();
    this.bootDecision = decision;

    if (decision.warn)
      this.notify("warning", decision.warn, decision.error || decision.reason);

    log.info(
      `[boot] decision: ${decision.action} (${decision.reason})` +
        (decision.error ? ` — ${decision.error}` : "")
    );

    return decision;
  }

  async executeBootGate() {
    this.bootMode = true;

    const outcome = await bootGate.runBootGate({
      enabled: this.enabled,
      timeoutMs: BOOT_CHECK_TIMEOUT_MS,
      log,
      getState: () => this.state,
      getError: () => this.error,
      getVersion: () => (this.info ? this.info.version : null),
      isGameRunning: () => this.isGameRunning(),
      isMandatory: () => this.isBootMandatory(),
      check: () => this.checkUpdates({ manual: true, reason: "boot" }),
      download: () => {
        this.userAcceptedDownload = true;
        this.send({ state: STATE.DOWNLOADING, progress: null });
        this.downloadCancellation = new CancellationToken();
        autoUpdater.downloadUpdate(this.downloadCancellation).catch((err) => {
          log.error("[boot] downloadUpdate failed:", err ? err.message : err);
        });
      },
      install: () => {
        this.prepareInstallLocation();

        if (this.versionEvents && this.verifiedVersion)
          try {
            this.versionEvents.writePendingReceipt(app.getVersion(), this.verifiedVersion);
          } catch (error) {
            log.warn(`[versions] receipt не сохранён: ${error.message}`);
          }

        autoUpdater.autoInstallOnAppQuit = true;
        setImmediate(() => autoUpdater.quitAndInstall(true, true));
      },
      subscribe: (fn) => {
        this.onBootState = fn;
        return () => {
          this.onBootState = null;
        };
      },
    });

    return this.finishBoot(outcome);
  }
}

export default UpdateManager;
