"use strict";

const schema = require("./event-schema");

const STAGE = {
  AVAILABLE: "TM_LU_AVAILABLE",
  DOWNLOAD_STARTED: "TM_LU_DL_STARTED",
  DOWNLOAD_COMPLETED: "TM_LU_DL_DONE",
  INSTALLED: "TM_LU_INSTALLED",
  FAILED: "TM_LU_FAILED",
};

const FAILURE_CODES = {
  LU_NO_FILE: "MANIFEST_INVALID",
  LU_VERIFY_FAILED: "CHECKSUM_MISMATCH",
  LU_SIZE_MISMATCH: "CHECKSUM_MISMATCH",
  LU_SHA256_MISMATCH: "CHECKSUM_MISMATCH",
  LU_SHA512_MISMATCH: "CHECKSUM_MISMATCH",
  LU_SIGNATURE_INVALID: "SIGNATURE_INVALID",
  LU_SCHEMA_INVALID: "MANIFEST_INVALID",
  LU_DOWNGRADE: "MANIFEST_INVALID",
  LU_VERSION_MISMATCH: "MANIFEST_INVALID",
  LAUNCHER_UPDATE_ERROR: "UNKNOWN_SAFE",
};

class LauncherUpdateEvents {
  constructor(options) {
    const opts = options || {};

    this.telemetry = opts.telemetry || null;
    this.versionEvents = opts.versionEvents || null;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };

    this.startedVersions = new Set();
  }

  once(stage, version, type, fields) {
    if (!this.telemetry) return false;

    if (!this.telemetry.enabled()) return false;
    if (!isVersion(version)) return false;

    if (this.versionEvents) {
      try {
        if (this.versionEvents.alreadyAnnounced(stage, version)) return false;
      } catch (error) {
        return false;
      }
    }

    const queued = this.telemetry.emit(type, fields);

    if (queued && this.versionEvents) {
      try {
        this.versionEvents.markAnnounced(stage, version);
      } catch (error) {
        this.logger.warn(`[telemetry] стадия ${stage} не отмечена: ${error.message}`);
      }
    }

    return queued;
  }

  available(currentVersion, nextVersion) {
    return this.once(STAGE.AVAILABLE, nextVersion, "launcher_update_available", {
      fromVersion: currentVersion,
      toVersion: nextVersion,
    });
  }

  downloadStarted(currentVersion, nextVersion) {
    if (this.startedVersions.has(nextVersion)) return false;
    this.startedVersions.add(nextVersion);

    return this.once(STAGE.DOWNLOAD_STARTED, nextVersion, "launcher_download_started", {
      fromVersion: currentVersion,
      toVersion: nextVersion,
    });
  }

  /**
   * Полная загрузка + пройденная проверка установщика.
   *
   * Байтовые метрики честные: неизвестный размер уходит как null, а не
   * как 0. downloadBytes берётся из фактически переданных байт
   * (progress.transferred у electron-updater), а не из размера файла:
   * при докачке это разные числа.
   */
  downloadCompleted(currentVersion, nextVersion, transfer) {
    const bytes = byteCount(transfer && transfer.bytes);
    const durationMs = byteCount(transfer && transfer.durationMs);

    return this.once(STAGE.DOWNLOAD_COMPLETED, nextVersion, "launcher_download_completed", {
      fromVersion: currentVersion,
      toVersion: nextVersion,
      downloadBytes: bytes,
      downloadDurationMilliseconds: durationMs,
      averageBytesPerSecond:
        bytes !== null && durationMs !== null && durationMs > 0
          ? Math.floor((bytes * 1000) / durationMs)
          : null,
    });
  }

  installed(fromVersion, toVersion) {
    return this.once(STAGE.INSTALLED, toVersion, "launcher_update_installed", {
      fromVersion,
      toVersion,
    });
  }

  failed(currentVersion, nextVersion, code) {
    const mapped = FAILURE_CODES[code] || "UNKNOWN_SAFE";
    const errorCode = schema.ERROR_CODES.indexOf(mapped) === -1 ? "UNKNOWN_SAFE" : mapped;

    const version = isVersion(nextVersion) ? nextVersion : currentVersion;

    return this.once(STAGE.FAILED, version, "launcher_update_failed", {
      fromVersion: currentVersion,
      toVersion: isVersion(nextVersion) ? nextVersion : currentVersion,
      errorCode,
    });
  }
}

function isVersion(value) {
  return typeof value === "string" && /^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/.test(value);
}

function byteCount(value) {
  if (value === undefined || value === null) return null;
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  if (!Number.isInteger(value) || value < 0) return null;
  return value;
}

module.exports = LauncherUpdateEvents;
module.exports.STAGE = STAGE;
module.exports.FAILURE_CODES = FAILURE_CODES;
module.exports.byteCount = byteCount;
