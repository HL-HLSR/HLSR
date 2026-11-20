"use strict";

const fs = require("fs");
const path = require("path");

const semver = require("../game-delivery/semver");

const SCHEMA_VERSION = 1;
const MAX_SEEN = 200;

const CODES = {
  LAUNCHER_UPDATE_AVAILABLE: "LAUNCHER_UPDATE_AVAILABLE",
  LAUNCHER_UPDATED: "LAUNCHER_UPDATED",
  GAME_UPDATE_AVAILABLE: "GAME_UPDATE_AVAILABLE",
  GAME_UPDATED: "GAME_UPDATED",
  GAME_INSTALLED: "GAME_INSTALLED",
};

const VERSION_RE = /^[0-9A-Za-z.\-+]{1,32}$/;
const GAME_ID_RE = /^[a-z0-9]+([._-][a-z0-9]+)*$/;

function isVersion(value) {
  return typeof value === "string" && VERSION_RE.test(value);
}

function isGameId(value) {
  return typeof value === "string" && value.length <= 64 && GAME_ID_RE.test(value);
}

class VersionEvents {
  constructor(options) {
    const opts = options || {};
    this.file = opts.file;
    this.store = opts.store;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
  }

  emptyState() {
    return { schemaVersion: SCHEMA_VERSION, seen: {}, receipt: null };
  }

  readState() {
    let raw;
    try {
      raw = fs.readFileSync(this.file, "utf8");
    } catch (e) {
      return this.emptyState();
    }

    let data;
    try {
      data = JSON.parse(raw);
    } catch (e) {
      this.quarantine("не разбирается как JSON");
      return this.emptyState();
    }

    if (!data || typeof data !== "object" || data.schemaVersion !== SCHEMA_VERSION) {
      this.quarantine(`неподдерживаемая схема: ${data && data.schemaVersion}`);
      return this.emptyState();
    }

    if (!data.seen || typeof data.seen !== "object") data.seen = {};
    if (data.receipt && typeof data.receipt !== "object") data.receipt = null;

    return data;
  }

  quarantine(reason) {
    try {
      if (!fs.existsSync(this.file)) return;
      const target = `${this.file}.corrupt-${this.now()}`;
      fs.renameSync(this.file, target);
      this.logger.warn(`[versions] состояние повреждено (${reason}), карантин: ${path.basename(target)}`);
    } catch (error) {
      this.logger.warn(`[versions] карантин не выполнен: ${error.message}`);
    }
  }

  writeState(state) {
    const temp = `${this.file}.tmp`;
    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true });
      fs.writeFileSync(temp, JSON.stringify(state), "utf8");
      fs.renameSync(temp, this.file);
    } catch (error) {
      this.logger.warn(`[versions] состояние не сохранено: ${error.message}`);
      try {
        if (fs.existsSync(temp)) fs.unlinkSync(temp);
      } catch (e) {
        /* временный файл */
      }
    }
  }

  key(code, version, gameId) {
    return isGameId(gameId) ? `${code}:${gameId}:${version}` : `${code}:${version}`;
  }

  alreadyAnnounced(code, version, state, gameId) {
    const data = state || this.readState();
    if (Object.prototype.hasOwnProperty.call(data.seen, this.key(code, version, gameId)))
      return true;

    if (isGameId(gameId))
      return Object.prototype.hasOwnProperty.call(data.seen, this.key(code, version));

    return false;
  }

  markAnnounced(code, version, state, gameId) {
    const data = state || this.readState();
    data.seen[this.key(code, version, gameId)] = this.now();

    const keys = Object.keys(data.seen);
    if (keys.length > MAX_SEEN) {
      keys
        .sort((a, b) => data.seen[a] - data.seen[b])
        .slice(0, keys.length - MAX_SEEN)
        .forEach((k) => delete data.seen[k]);
    }

    this.writeState(data);
    return data;
  }

  publishOnce(code, version, level, detail, meta) {
    if (!isVersion(version)) {
      this.logger.warn(`[versions] отклонена некорректная версия для ${code}`);
      return null;
    }

    const gameId = meta && isGameId(meta.gameId) ? meta.gameId : null;

    const state = this.readState();
    if (this.alreadyAnnounced(code, version, state, gameId)) return null;

    const item = this.store.add({ code, level, detail, meta });
    this.markAnnounced(code, version, state, gameId);
    return item;
  }

  resolveUpdateOffers(target, installedVersion, gameId) {
    if (!this.store || typeof this.store.removeWhere !== "function") return 0;
    if (!isVersion(installedVersion)) return 0;

    const code =
      target === "launcher" ? CODES.LAUNCHER_UPDATE_AVAILABLE : CODES.GAME_UPDATE_AVAILABLE;
    const id = isGameId(gameId) ? gameId : null;

    let removed = 0;
    try {
      removed = this.store.removeWhere((item) => {
        if (!item || item.code !== code) return false;

        const meta = item.meta || {};
        if (target === "game" && id && meta.gameId && meta.gameId !== id) return false;
        if (!isVersion(meta.newVersion)) return false;

        return semver.compare(meta.newVersion, installedVersion) <= 0;
      });
    } catch (error) {
      this.logger.warn(`[versions] устаревшие приглашения не сняты: ${error.message}`);
      return 0;
    }

    if (removed)
      this.logger.info(
        `[versions] снято приглашений обновиться: ${removed} (установлено ${installedVersion})`
      );

    return removed;
  }

  launcherUpdateAvailable(version, currentVersion, options) {
    const opts = options || {};
    return this.publishOnce(CODES.LAUNCHER_UPDATE_AVAILABLE, version, "info", version, {
      target: "launcher",
      oldVersion: isVersion(currentVersion) ? currentVersion : undefined,
      newVersion: version,
      hasReleaseNotes: opts.hasReleaseNotes === true,
    });
  }

  launcherUpdated(fromVersion, toVersion) {
    return this.publishOnce(CODES.LAUNCHER_UPDATED, toVersion, "info", toVersion, {
      target: "launcher",
      oldVersion: isVersion(fromVersion) ? fromVersion : undefined,
      newVersion: toVersion,
      hasReleaseNotes: true,
    });
  }

  gameUpdateAvailable(version, currentVersion, gameId, options) {
    const opts = options || {};
    return this.publishOnce(CODES.GAME_UPDATE_AVAILABLE, version, "info", version, {
      target: "game",
      gameId: isGameId(gameId) ? gameId : undefined,
      launcherId: opts.launcherId,
      oldVersion: isVersion(currentVersion) ? currentVersion : undefined,
      newVersion: version,
      hasReleaseNotes: opts.hasReleaseNotes === true,
    });
  }

  gameUpdated(fromVersion, toVersion, gameId, options) {
    const opts = options || {};
    return this.publishOnce(CODES.GAME_UPDATED, toVersion, "info", toVersion, {
      target: "game",
      gameId: isGameId(gameId) ? gameId : undefined,
      launcherId: opts.launcherId,
      oldVersion: isVersion(fromVersion) ? fromVersion : undefined,
      newVersion: toVersion,
      hasReleaseNotes: opts.hasReleaseNotes === true,
    });
  }

  gameInstalled(version, gameId, options) {
    const opts = options || {};
    return this.publishOnce(CODES.GAME_INSTALLED, version, "info", version, {
      target: "game",
      gameId: isGameId(gameId) ? gameId : undefined,
      launcherId: opts.launcherId,
      newVersion: version,
      hasReleaseNotes: opts.hasReleaseNotes === true,
    });
  }

  writePendingReceipt(fromVersion, toVersion) {
    if (!isVersion(toVersion)) return null;

    const state = this.readState();
    state.receipt = {
      from: isVersion(fromVersion) ? fromVersion : null,
      to: toVersion,
      ts: this.now(),
    };
    this.writeState(state);
    return state.receipt;
  }

  peekReceipt() {
    const receipt = this.readState().receipt;
    if (!receipt || !isVersion(receipt.to)) return null;

    return { from: receipt.from || null, to: receipt.to };
  }

  consumeReceipt(currentVersion) {
    const state = this.readState();
    const receipt = state.receipt;
    if (!receipt || !isVersion(receipt.to)) return null;

    if (receipt.to !== currentVersion) return null;

    state.receipt = null;
    this.writeState(state);

    return this.launcherUpdated(receipt.from, receipt.to);
  }

  clearReceipt() {
    const state = this.readState();
    if (!state.receipt) return false;
    state.receipt = null;
    this.writeState(state);
    return true;
  }
}

module.exports = VersionEvents;
module.exports.CODES = CODES;
module.exports.SCHEMA_VERSION = SCHEMA_VERSION;
