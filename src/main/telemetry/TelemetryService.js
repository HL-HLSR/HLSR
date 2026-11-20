"use strict";

const os = require("os");
const path = require("path");

const config = require("./config");
const consent = require("./consent");
const identity = require("./identity");
const schema = require("./event-schema");
const crashMarker = require("./crash-marker");
const installReceipt = require("./install-receipt");
const TelemetryQueue = require("./Queue");
const Sender = require("./Sender");

function windowsMajor() {
  if (process.platform !== "win32") return "unknown";

  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(os.release() || "");
  if (!match) return "unknown";

  const major = Number(match[1]);
  const build = Number(match[3]);

  if (major === 10) return build >= 22000 ? "11" : "10";
  return "unknown";
}

function windowsArch() {
  return "x64";
}

function jitter(valueMs, ratio) {
  const spread = valueMs * ratio;
  return Math.max(1000, Math.round(valueMs - spread + Math.random() * spread * 2));
}

class TelemetryService {
  constructor(options) {
    const opts = options || {};
    if (!opts.userDataDir) throw new Error("TelemetryService: не задан userDataDir");

    this.userDataDir = opts.userDataDir;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.localeProvider = opts.localeProvider || (() => "ru");
    this.isWindowVisible = opts.isWindowVisible || (() => true);
    this.backendConfigured =
      opts.backendConfigured === undefined ? config.BACKEND_CONFIGURED : opts.backendConfigured;
    this.installDir = opts.installDir || null;
    this.authorizationProvider = opts.authorizationProvider || null;

    this.queue = new TelemetryQueue({
      file: path.join(this.userDataDir, config.QUEUE.fileName),
      maxEvents: config.QUEUE.maxEvents,
      maxBytes: config.QUEUE.maxBytes,
      logger: this.logger,
    });

    this.sender = new Sender({
      queue: this.queue,
      endpoint: opts.endpoint || config.ENDPOINTS.batch,
      launcherVersion: this.launcherVersion,
      logger: this.logger,
      now: this.now,
      request: opts.request,
      authorizationProvider: this.authorizationProvider,
    });

    this.launcherSessionId = null;
    this.launcherSessionStartedMs = null;
    this.gameSessions = new Map();
    this.launcherTimer = null;
    this.gameTimer = null;
    this.started = false;
  }

  enabled() {
    if (!this.backendConfigured) return false;
    if (this.authorizationProvider && !this.authorizationProvider()) return false;

    try {
      return consent.read(this.userDataDir).enabled === true;
    } catch (error) {
      return false;
    }
  }

  start() {
    if (this.started) return;
    this.started = true;

    if (!this.enabled()) {
      this.purgeLocalData();
      return;
    }

    try {
      const id = identity.ensure(this.userDataDir);

      const crashed = crashMarker.detect(this.userDataDir, this.now());
      if (crashed) {
        const queued = this.push("launcher_crash_detected", {
          sessionId: crashed.sessionId,
          occurredAt: crashed.lastAliveAt,
        });
        if (queued) crashMarker.consume(this.userDataDir);
      }

      this.reportInstallIfNeeded();

      this.launcherSessionId = identity.randomUuid();
      this.launcherSessionStartedMs = this.now();
      crashMarker.open(this.userDataDir, this.launcherSessionId, this.now());

      this.push("launcher_session_started", {});

      this.scheduleLauncherHeartbeat();
      this.sender.start();
      this.sender.flush().catch(() => {});

      this.logger.info(`[telemetry] включена, установка ${id.installationId.slice(0, 8)}…`);
    } catch (error) {
      this.logger.warn(`[telemetry] запуск не удался: ${error.message}`);
    }
  }

  shutdown() {
    if (!this.enabled()) {
      this.stopTimers();
      return;
    }

    try {
      if (this.launcherSessionId) {
        this.push("launcher_session_ended", {
          durationSeconds: this.elapsedSeconds(this.launcherSessionStartedMs),
        });
      }

      for (const gameId of Array.from(this.gameSessions.keys()))
        this.endGameSession(gameId, { exitReason: "unknown" });

      crashMarker.close(this.userDataDir);
      this.sender.flush().catch(() => {});
    } catch (error) {
      this.logger.warn(`[telemetry] завершение: ${error.message}`);
    } finally {
      this.stopTimers();
    }
  }

  reportInstallIfNeeded() {
    if (!this.installDir) return false;

    let detected;
    try {
      detected = installReceipt.detectInstall(this.userDataDir, this.installDir);
    } catch (error) {
      return false;
    }

    if (!detected.report) return false;

    if (this.push("launcher_install_completed", { sessionId: null })) {
      installReceipt.confirmInstall(this.userDataDir, detected.installId);
      return true;
    }

    return false;
  }

  reportUninstallInitiated() {
    if (!this.enabled()) return false;
    return this.push("launcher_uninstall_initiated", { sessionId: null });
  }

  stopTimers() {
    if (this.launcherTimer) {
      clearTimeout(this.launcherTimer);
      this.launcherTimer = null;
    }
    if (this.gameTimer) {
      clearTimeout(this.gameTimer);
      this.gameTimer = null;
    }
    this.sender.stop();
  }

  setEnabled(enabled) {
    const previous = this.enabled();
    consent.set(this.userDataDir, enabled === true);

    if (enabled === true && !previous) {
      this.started = false;
      this.start();
      return { enabled: true };
    }

    if (enabled !== true && previous) {
      this.stopTimers();
      this.launcherSessionId = null;
      this.gameSessions.clear();
      this.purgeLocalData();
      this.logger.info("[telemetry] выключена, локальные данные телеметрии удалены");
      return { enabled: false, purged: true };
    }

    return { enabled: enabled === true };
  }

  purgeLocalData() {
    this.queue.purge();
    crashMarker.close(this.userDataDir);
  }

  resetIdentity() {
    this.queue.purge();
    const had = identity.reset(this.userDataDir);
    crashMarker.close(this.userDataDir);

    this.launcherSessionId = null;
    this.gameSessions.clear();

    if (this.enabled()) {
      this.started = false;
      this.start();
    }

    return { ok: true, hadIdentity: had };
  }

  elapsedSeconds(startedMs) {
    if (!startedMs) return 0;
    return Math.max(0, Math.min(schema.LIMITS.durationSeconds, Math.floor((this.now() - startedMs) / 1000)));
  }

  locale() {
    const value = String(this.localeProvider() || "ru").toLowerCase();
    return value.startsWith("en") ? "en" : "ru";
  }

  push(type, fields) {
    if (!this.enabled()) return false;

    const spec = schema.TYPES[type];
    if (!spec) return false;

    if (!schema.isEnabledType(type)) return false;

    let installationId;
    try {
      installationId = identity.ensure(this.userDataDir).installationId;
    } catch (error) {
      return false;
    }

    const overrides = fields || {};

    let sessionId = null;
    if (spec.kind === "launcher") sessionId = overrides.sessionId || this.launcherSessionId;
    else if (spec.kind === "game") sessionId = overrides.sessionId || null;

    const event = {
      schemaVersion: schema.SCHEMA_VERSION,
      eventId: identity.randomUuid(),
      installationId,
      sessionId,
      type,
      occurredAt: overrides.occurredAt || new Date(this.now()).toISOString().replace(/\.\d{3}Z$/, ".000Z"),
      launcherVersion: this.launcherVersion,
      locale: this.locale(),
      windowsMajor: windowsMajor(),
      windowsArch: windowsArch(),
    };

    for (const field of spec.fields)
      if (field in overrides) event[field] = overrides[field];

    const check = schema.validate(event);
    if (!check.ok) {
      this.logger.warn(`[telemetry] событие ${type} отброшено: ${check.reason}`);
      return false;
    }

    return this.queue.append(event);
  }

  emit(type, fields) {
    try {
      return this.push(type, fields);
    } catch (error) {
      return false;
    }
  }

  scheduleLauncherHeartbeat() {
    if (this.launcherTimer) clearTimeout(this.launcherTimer);

    this.launcherTimer = setTimeout(() => {
      this.launcherHeartbeat();
      if (this.enabled()) this.scheduleLauncherHeartbeat();
    }, jitter(config.HEARTBEAT.launcherMs, config.HEARTBEAT.jitterRatio));

    if (typeof this.launcherTimer.unref === "function") this.launcherTimer.unref();
  }

  launcherHeartbeat() {
    if (!this.enabled() || !this.launcherSessionId) return;

    if (!this.isWindowVisible()) return;

    this.push("launcher_heartbeat", {});
    crashMarker.touch(this.userDataDir, this.now());
    this.sender.flush().catch(() => {});
  }

  scheduleGameHeartbeat() {
    if (this.gameTimer) clearTimeout(this.gameTimer);
    if (this.gameSessions.size === 0) return;

    this.gameTimer = setTimeout(() => {
      this.gameHeartbeat();
      this.scheduleGameHeartbeat();
    }, jitter(config.HEARTBEAT.gameMs, config.HEARTBEAT.jitterRatio));

    if (typeof this.gameTimer.unref === "function") this.gameTimer.unref();
  }

  gameHeartbeat() {
    if (!this.enabled()) return;

    for (const [gameId, session] of this.gameSessions) {
      this.push("game_heartbeat", {
        sessionId: session.sessionId,
        gameId,
        gameVersion: session.meta.gameVersion,
        buildId: session.meta.buildId,
      });
    }

    this.sender.flush().catch(() => {});
  }

  beginGameSession(gameId, meta) {
    if (!this.enabled()) return null;

    const session = {
      sessionId: identity.randomUuid(),
      startedMs: this.now(),
      meta: {
        gameVersion: (meta && meta.gameVersion) || "0.0.0",
        buildId: (meta && meta.buildId) || "unknown",
      },
    };

    this.gameSessions.set(gameId, session);

    this.push("game_launch_started", {
      sessionId: session.sessionId,
      gameId,
      gameVersion: session.meta.gameVersion,
      buildId: session.meta.buildId,
    });

    return session.sessionId;
  }

  confirmGameRunning(gameId) {
    const session = this.gameSessions.get(gameId);
    if (!session || !this.enabled()) return;

    this.push("game_launch_succeeded", {
      sessionId: session.sessionId,
      gameId,
      gameVersion: session.meta.gameVersion,
      buildId: session.meta.buildId,
    });

    this.scheduleGameHeartbeat();
    this.sender.flush().catch(() => {});
  }

  failGameLaunch(gameId, errorCode) {
    const session = this.gameSessions.get(gameId);
    if (!session || !this.enabled()) return;

    this.push("game_launch_failed", {
      sessionId: session.sessionId,
      gameId,
      gameVersion: session.meta.gameVersion,
      buildId: session.meta.buildId,
      errorCode: schema.ERROR_CODES.indexOf(errorCode) === -1 ? "UNKNOWN_SAFE" : errorCode,
    });

    this.gameSessions.delete(gameId);
    this.scheduleGameHeartbeat();
  }

  endGameSession(gameId, outcome) {
    const session = this.gameSessions.get(gameId);
    if (!session) return;

    if (this.enabled()) {
      const durationSeconds = this.elapsedSeconds(session.startedMs);
      const info = outcome || {};

      if (info.crashed === true) {
        this.push("game_crashed", {
          sessionId: session.sessionId,
          gameId,
          gameVersion: session.meta.gameVersion,
          buildId: session.meta.buildId,
          durationSeconds,
          errorCode: "GAME_CRASH",
        });
      } else {
        const reason = schema.EXIT_REASONS.indexOf(info.exitReason) === -1 ? "unknown" : info.exitReason;
        this.push("game_session_ended", {
          sessionId: session.sessionId,
          gameId,
          gameVersion: session.meta.gameVersion,
          buildId: session.meta.buildId,
          durationSeconds,
          exitReason: reason,
        });
      }
    }

    this.gameSessions.delete(gameId);
    this.scheduleGameHeartbeat();
    this.sender.flush().catch(() => {});
  }

  gameSessionId(gameId) {
    const session = this.gameSessions.get(gameId);
    return session ? session.sessionId : null;
  }

  gameSessionMeta(gameId) {
    const session = this.gameSessions.get(gameId);
    return session ? session.meta : null;
  }

  snapshot() {
    const id = identity.read(this.userDataDir);

    return {
      backendConfigured: this.backendConfigured,
      enabled: this.enabled(),
      hasIdentity: id !== null,
      installationIdPrefix: id ? id.installationId.slice(0, 8) : null,
      launcherSessionActive: this.launcherSessionId !== null,
      gameSessionsActive: this.gameSessions.size,
      queue: this.sender.snapshot(),
    };
  }
}

module.exports = TelemetryService;
module.exports.windowsMajor = windowsMajor;
module.exports.windowsArch = windowsArch;
module.exports.jitter = jitter;
