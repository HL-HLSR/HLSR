"use strict";

const fs = require("fs");
const path = require("path");

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");

const SCHEMA_VERSION = 1;
const FILE_NAME = "game-stats.json";
const HEARTBEAT_MS = 30 * 1000;

function monotonicSeconds() {
  return process.hrtime()[0];
}

class GameStatsStore {
  constructor(options) {
    const opts = options || {};
    if (!opts.file) throw new Error("GameStatsStore: не задан путь к файлу");

    this.file = opts.file;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.notifications = opts.notifications || null;
    this.now = opts.now || (() => Date.now());
    this.monotonic = opts.monotonic || monotonicSeconds;
    this.heartbeatMs = opts.heartbeatMs == null ? HEARTBEAT_MS : opts.heartbeatMs;

    this.active = new Map();
    this.heartbeatTimer = null;
    this.corrupt = false;

    this.recoverInterrupted();
  }

  readAll() {
    if (!fs.existsSync(this.file)) return { schemaVersion: SCHEMA_VERSION, games: {} };

    const raw = readJsonSafe(this.file);

    if (!raw || typeof raw !== "object" || raw.schemaVersion !== SCHEMA_VERSION || typeof raw.games !== "object" || !raw.games) {
      this.quarantine();
      return { schemaVersion: SCHEMA_VERSION, games: {} };
    }

    return raw;
  }

  quarantine() {
    if (this.corrupt) return;
    this.corrupt = true;

    try {
      const target = `${this.file}.corrupt-${this.now().toString(36)}`;
      fs.renameSync(this.file, target);
      this.logger.warn(`[game-stats] повреждённый файл карантинирован: ${path.basename(target)}`);
    } catch (e) {
      this.logger.warn(`[game-stats] карантин не удался: ${e.message}`);
    }

    if (this.notifications)
      try {
        this.notifications.add({
          level: "warning",
          code: "GAME_STATS_CORRUPT",
          detail: "статистика игры сброшена из-за повреждения файла",
        });
      } catch (e) {
        /* уведомление вторично */
      }
  }

  write(all) {
    all.updatedAt = new Date(this.now()).toISOString();
    writeJsonAtomic(this.file, all);
  }

  gameOf(all, gameId) {
    if (!all.games[gameId])
      all.games[gameId] = {
        gameId,
        totalPlaySeconds: 0,
        lastLaunchAt: null,
        lastExitAt: null,
        lastSessionSeconds: null,
        launchCount: 0,
        currentSession: null,
      };
    return all.games[gameId];
  }

  recoverInterrupted() {
    const all = this.readAll();
    let changed = false;

    for (const gameId of Object.keys(all.games)) {
      const game = all.games[gameId];
      const session = game.currentSession;
      if (!session) continue;

      const credited = Math.max(0, Math.floor(session.heartbeatSeconds || 0));

      game.totalPlaySeconds += credited;
      game.lastSessionSeconds = credited;
      game.lastExitAt = session.lastHeartbeatAt || session.startedAt || null;
      game.currentSession = null;
      changed = true;

      this.logger.warn(
        `[game-stats] незакрытая сессия ${gameId} восстановлена по heartbeat: +${credited} c`
      );
    }

    if (changed) this.write(all);
  }

  sessionStart(gameId, info) {
    const all = this.readAll();
    const game = this.gameOf(all, gameId);
    const startedAt = new Date(this.now()).toISOString();

    game.launchCount += 1;
    game.lastLaunchAt = startedAt;
    game.currentSession = {
      startedAt,
      lastHeartbeatAt: startedAt,
      heartbeatSeconds: 0,
      pid: info && info.pid ? info.pid : null,
    };

    this.active.set(gameId, { startedMonotonic: this.monotonic() });
    this.write(all);
    this.ensureHeartbeat();
  }

  ensureHeartbeat() {
    if (this.heartbeatTimer || !this.active.size) return;

    this.heartbeatTimer = setInterval(() => this.heartbeat(), this.heartbeatMs);
    if (typeof this.heartbeatTimer.unref === "function") this.heartbeatTimer.unref();
  }

  stopHeartbeatIfIdle() {
    if (this.active.size || !this.heartbeatTimer) return;
    clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = null;
  }

  heartbeat() {
    if (!this.active.size) return this.stopHeartbeatIfIdle();

    const all = this.readAll();
    let changed = false;

    for (const [gameId, session] of this.active) {
      const game = this.gameOf(all, gameId);
      if (!game.currentSession) continue;

      game.currentSession.heartbeatSeconds = Math.max(
        0,
        Math.floor(this.monotonic() - session.startedMonotonic)
      );
      game.currentSession.lastHeartbeatAt = new Date(this.now()).toISOString();
      changed = true;
    }

    if (changed) this.write(all);
  }

  sessionEnd(gameId) {
    const session = this.active.get(gameId);
    const all = this.readAll();
    const game = this.gameOf(all, gameId);

    let seconds = 0;

    if (session) {
      seconds = Math.max(0, Math.floor(this.monotonic() - session.startedMonotonic));
      this.active.delete(gameId);
    } else if (game.currentSession) {
      seconds = Math.max(0, Math.floor(game.currentSession.heartbeatSeconds || 0));
    } else {
      this.stopHeartbeatIfIdle();
      return null;
    }

    game.totalPlaySeconds += seconds;
    game.lastSessionSeconds = seconds;
    game.lastExitAt = new Date(this.now()).toISOString();
    game.currentSession = null;

    this.write(all);
    this.stopHeartbeatIfIdle();

    return { gameId, seconds, totalPlaySeconds: game.totalPlaySeconds };
  }

  summary(gameId) {
    const all = this.readAll();
    const game = all.games[gameId];

    if (!game || !game.launchCount)
      return { available: false, gameId };

    return {
      available: true,
      gameId,
      totalPlaySeconds: game.totalPlaySeconds,
      lastLaunchAt: game.lastLaunchAt,
      lastExitAt: game.lastExitAt,
      lastSessionSeconds: game.lastSessionSeconds,
      launchCount: game.launchCount,
      sessionActive: this.active.has(gameId) || !!game.currentSession,
    };
  }

  dispose() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }
}

module.exports = GameStatsStore;
module.exports.FILE_NAME = FILE_NAME;
module.exports.SCHEMA_VERSION = SCHEMA_VERSION;
module.exports.HEARTBEAT_MS = HEARTBEAT_MS;
