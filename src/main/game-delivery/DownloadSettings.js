"use strict";

const DownloadRateLimiter = require("./DownloadRateLimiter");
const { readJsonState, writeJsonAtomic } = require("./atomic");
const { validSpeedLimit, normalizeSpeedLimit } = require("../../shared/download-settings");

class DownloadSettings {
  constructor(options) {
    this.file = options.file;
    this.state = readJsonState(this.file);
    this.initialized = this.state.valid && validSpeedLimit(this.state.data && this.state.data.speedLimit);
    this.speedLimit = normalizeSpeedLimit(this.state.data && this.state.data.speedLimit);
    this.rateLimiter = new DownloadRateLimiter(this.speedLimit * 1024 * 1024);
  }

  snapshot() {
    return { speedLimit: this.speedLimit, initialized: this.initialized };
  }

  save(speedLimit) {
    if (!validSpeedLimit(speedLimit)) return { ok: false, code: "INVALID_SPEED_LIMIT" };
    // Не перезаписываем файл, который не удалось прочитать.
    const current = readJsonState(this.file);
    if (current.found && !current.valid)
      return { ok: false, code: "SETTINGS_SAVE_FAILED" };
    try {
      writeJsonAtomic(this.file, { speedLimit });
    } catch (error) {
      return { ok: false, code: "SETTINGS_SAVE_FAILED" };
    }
    this.state = { found: true, valid: true, data: { speedLimit } };
    this.initialized = true;
    this.speedLimit = speedLimit;
    this.rateLimiter.setLimit(speedLimit * 1024 * 1024);
    return Object.assign({ ok: true }, this.snapshot());
  }
}

module.exports = DownloadSettings;
