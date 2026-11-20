"use strict";

const { DeliveryError, toDeliveryError } = require("./errors");

const SWITCH_CODES = [
  "NETWORK_ERROR",
  "NETWORK_TIMEOUT",
  "HTTP_ERROR",
  "TOO_MANY_REDIRECTS",
  "INSECURE_REDIRECT",
  "CONTENT_LENGTH_MISMATCH",
  "SIZE_MISMATCH",
  "CHECKSUM_MISMATCH",
  "PROVIDER_LINK_FAILED",
  "PROVIDER_FORBIDDEN",
  "CHANNEL_UNAVAILABLE",
  "SIGNATURE_INVALID",
  "MANIFEST_INVALID",
  "FILES_MANIFEST_MISSING",
];

const STOP_CODES = [
  "CANCELLED",
  "ALREADY_RUNNING",
  "GAME_RUNNING",
  "FILE_LOCKED",
  "NO_SPACE",
  "PATH_NOT_WRITABLE",
  "PATH_INVALID",
  "ARCHIVE_LIST_FAILED",
  "ARCHIVE_UNSAFE_ENTRY",
  "ARCHIVE_LIMIT_EXCEEDED",
  "EXTRACT_FAILED",
  "REQUIRED_FILE_MISSING",
  "INSTALL_FAILED",
  "ROLLBACK_FAILED",
  "REPAIR_FAILED",
  "VERIFY_FAILED",
  "PATCH_FAILED",
  "PATCH_CHAIN_MISSING",
  "REMOTE_DISABLED",
  "PROVIDER_UNSUPPORTED",
  "MANIFEST_DISABLED",
  "MANIFEST_NOT_FOUND",
  "MANIFEST_VERSION_DRAFT",
  "LAUNCHER_TOO_OLD",
  "CHANNEL_DISABLED",
  "UPDATE_REQUIRED",
  "NOT_INSTALLED",
  "VERSION_MISMATCH",
];

const DISCARD_PARTIAL_CODES = ["SIZE_MISMATCH", "CHECKSUM_MISMATCH", "CONTENT_LENGTH_MISMATCH"];

const DEFAULT_COOLDOWN_MS = 15 * 60 * 1000;
const MAX_COOLDOWN_MS = 60 * 60 * 1000;

function hostOf(descriptor) {
  const raw = descriptor && descriptor.url;
  if (!raw) return null;

  try {
    return new URL(String(raw)).hostname;
  } catch (e) {
    return null;
  }
}

function authorityOf(descriptor) {
  const raw = descriptor && descriptor.url;
  if (!raw) return null;

  try {
    return new URL(String(raw)).host;
  } catch (e) {
    return null;
  }
}

function parseRetryAfter(value, now) {
  if (value == null) return null;

  const raw = String(value).trim();
  if (!raw) return null;

  if (/^\d+$/.test(raw)) {
    const seconds = Number.parseInt(raw, 10);
    if (!Number.isFinite(seconds) || seconds < 0) return null;
    return Math.min(seconds * 1000, MAX_COOLDOWN_MS);
  }

  const at = Date.parse(raw);
  if (!Number.isFinite(at)) return null;

  const delta = at - now;
  if (delta <= 0) return null;
  return Math.min(delta, MAX_COOLDOWN_MS);
}

function classify(error) {
  const delivery = toDeliveryError(error, "INSTALL_FAILED");
  if (STOP_CODES.indexOf(delivery.code) !== -1) return { action: "stop", code: delivery.code };
  if (SWITCH_CODES.indexOf(delivery.code) !== -1) return { action: "switch", code: delivery.code };
  return { action: "stop", code: delivery.code };
}

function httpStatusOf(error) {
  const details = error && error.details;
  if (!details) return null;
  const status = details.status || details.statusCode;
  return Number.isFinite(status) ? status : null;
}

function attemptRecord(descriptor, index, outcome) {
  return {
    providerId: descriptor.id || null,
    host: hostOf(descriptor),
    attempt: index + 1,
    resultCode: outcome.resultCode,
    httpStatus: outcome.httpStatus == null ? null : outcome.httpStatus,
    retryAfterMs: outcome.retryAfterMs == null ? null : outcome.retryAfterMs,
  };
}

class HostCooldown {
  constructor(options) {
    const opts = options || {};
    this.now = opts.now || (() => Date.now());
    this.until = new Map();
  }

  blockedFor(host) {
    if (!host) return 0;
    const until = this.until.get(host);
    if (!until) return 0;

    const left = until - this.now();
    if (left <= 0) {
      this.until.delete(host);
      return 0;
    }
    return left;
  }

  pause(host, ms) {
    if (!host || !ms) return;
    const until = this.now() + Math.min(ms, MAX_COOLDOWN_MS);
    const current = this.until.get(host) || 0;
    if (until > current) this.until.set(host, until);
  }

  snapshot() {
    const out = {};
    for (const [host] of this.until) {
      const left = this.blockedFor(host);
      if (left > 0) out[host] = left;
    }
    return out;
  }
}

class MirrorRunner {
  constructor(options) {
    const opts = options || {};
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.cooldown = opts.cooldown || new HostCooldown({ now: this.now });
    this.inFlight = new Set();
  }

  async run(request) {
    const {
      artifact,
      descriptors,
      operation,
      onDiscardPartial,
      isTerminalResult,
    } = request;

    if (!Array.isArray(descriptors) || !descriptors.length)
      throw new DeliveryError("REMOTE_DISABLED", "Нет ни одного доступного источника");

    const key = String(artifact || "");
    if (key && this.inFlight.has(key))
      throw new DeliveryError("ALREADY_RUNNING", `Артефакт ${key} уже загружается`);
    if (key) this.inFlight.add(key);

    const attempts = [];
    let lastError = null;

    try {
      for (let index = 0; index < descriptors.length; index += 1) {
        const descriptor = descriptors[index];
        const authority = authorityOf(descriptor);

        const blocked = this.cooldown.blockedFor(authority);
        if (blocked > 0) {
          attempts.push(
            attemptRecord(descriptor, index, {
              resultCode: "HOST_COOLDOWN",
              httpStatus: null,
              retryAfterMs: blocked,
            })
          );
          this.logger.warn(
            `[game-delivery] источник ${descriptor.id} пропущен: cooldown ещё ${Math.round(blocked / 1000)} с`
          );
          continue;
        }

        try {
          const value = await operation({ descriptor, index, isFallback: index > 0 });

          if (typeof isTerminalResult === "function" && isTerminalResult(value)) {
            attempts.push(
              attemptRecord(descriptor, index, { resultCode: "TERMINAL", httpStatus: null, retryAfterMs: null })
            );
            return { value, providerId: descriptor.id || null, usedFallback: index > 0, attempts };
          }

          attempts.push(
            attemptRecord(descriptor, index, { resultCode: "OK", httpStatus: null, retryAfterMs: null })
          );

          if (index > 0)
            this.logger.warn(
              `[game-delivery] артефакт ${key || "?"} получен с резервного источника ${descriptor.id}`
            );

          return { value, providerId: descriptor.id || null, usedFallback: index > 0, attempts };
        } catch (error) {
          const verdict = classify(error);
          const delivery = toDeliveryError(error, verdict.code);
          const status = httpStatusOf(delivery);

          let retryAfterMs = null;
          if (status === 429) {
            const details = delivery.details || {};
            retryAfterMs =
              parseRetryAfter(details.retryAfter, this.now()) || DEFAULT_COOLDOWN_MS;
            this.cooldown.pause(authority, retryAfterMs);
          }

          attempts.push(
            attemptRecord(descriptor, index, {
              resultCode: verdict.code,
              httpStatus: status,
              retryAfterMs,
            })
          );

          lastError = delivery;

          if (verdict.action === "stop") {
            this.logger.warn(
              `[game-delivery] источник ${descriptor.id} отказал с ${verdict.code}: ` +
                "смена зеркала не поможет, операция остановлена"
            );
            throw delivery;
          }

          if (DISCARD_PARTIAL_CODES.indexOf(verdict.code) !== -1 && typeof onDiscardPartial === "function") {
            try {
              await onDiscardPartial({ descriptor, code: verdict.code });
            } catch (e) {
              this.logger.warn(`[game-delivery] не удалось очистить partial: ${e.message}`);
            }
          }

          this.logger.warn(
            `[game-delivery] источник ${descriptor.id} отказал с ${verdict.code}` +
              (index + 1 < descriptors.length ? ", пробуем следующее зеркало" : "")
          );
        }
      }

      throw lastError || new DeliveryError("NETWORK_ERROR", "Все источники недоступны");
    } finally {
      if (key) this.inFlight.delete(key);
    }
  }
}

module.exports = {
  MirrorRunner,
  HostCooldown,
  classify,
  parseRetryAfter,
  hostOf,
  authorityOf,
  SWITCH_CODES,
  STOP_CODES,
  DISCARD_PARTIAL_CODES,
  DEFAULT_COOLDOWN_MS,
  MAX_COOLDOWN_MS,
};
