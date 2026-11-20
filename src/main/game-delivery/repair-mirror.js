"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { URL } = require("url");
const { Transform } = require("stream");
const { pipeline } = require("stream/promises");

const httpClient = require("./http-client");
const fileOps = require("./file-ops");
const { DeliveryError } = require("./errors");

const SHA256_PATTERN = /^[a-f0-9]{64}$/;
const DEFAULT_TIMEOUT_MS = 30000;
const DEFAULT_ATTEMPTS = 2;
const PROGRESS_EVERY_MS = 200;

const MAX_FILES = 2000;

const DISK_ERRORS = {
  ENOSPC: "NO_SPACE",
  EDQUOT: "NO_SPACE",
  EACCES: "PATH_NOT_WRITABLE",
  EPERM: "PATH_NOT_WRITABLE",
  EBUSY: "PATH_NOT_WRITABLE",
  EROFS: "PATH_NOT_WRITABLE",
};

const LOCK_RETRY_MS = 500;

// Антивирус ненадолго держит только что записанный файл (EPERM/EBUSY на Windows):
// одна повторная попытка после паузы, только потом «нет прав на запись».
async function renameSettled(from, to, signal) {
  try {
    fs.renameSync(from, to);
  } catch (error) {
    if (error.code !== "EPERM" && error.code !== "EBUSY") throw error;
    await new Promise((resolve) => setTimeout(resolve, LOCK_RETRY_MS));
    if (signal && signal.aborted) throw cancelled();
    fs.renameSync(from, to);
  }
}

const LAYOUTS = ["sha256-2"];

function assertDigest(sha256) {
  if (!SHA256_PATTERN.test(String(sha256 || "")))
    throw new DeliveryError("PATH_INVALID", `Недопустимый SHA-256 для зеркала: ${sha256}`);
}

function fileUrl(baseUrl, sha256, layout) {
  assertDigest(sha256);

  if (LAYOUTS.indexOf(layout) === -1)
    throw new DeliveryError("MANIFEST_INVALID", `Неизвестная раскладка зеркала: ${layout}`);

  const base = String(baseUrl).replace(/\/+$/, "");
  return `${base}/${sha256.slice(0, 2)}/${sha256}`;
}

function usableProviders(mirror, opts) {
  return (mirror.providers || []).filter(
    (provider) => (opts.allowedProviderTypes || []).indexOf(provider.type) !== -1
  );
}

// Почему зеркалом этой версии пользоваться нельзя вообще (null — можно).
function unavailableReason(mirror, options) {
  const opts = options || {};

  if (!mirror) return "MIRROR_NOT_DECLARED";
  if (LAYOUTS.indexOf(mirror.layout) === -1) return "MIRROR_LAYOUT_UNKNOWN";
  if (opts.version && mirror.version !== opts.version) return "MIRROR_VERSION_MISMATCH";
  if (!usableProviders(mirror, opts).length) return "MIRROR_PROVIDER_DISABLED";
  return null;
}

function plan(mirror, entries, options) {
  const opts = options || {};
  const reason = unavailableReason(mirror, opts);

  if (reason) return { usable: false, reason };
  if (!entries.length) return { usable: false, reason: "MIRROR_NOTHING_TO_FETCH" };
  if (entries.length > MAX_FILES) return { usable: false, reason: "MIRROR_TOO_MANY_FILES" };

  const maxFileSize = Number(mirror.maxFileSize) || 0;
  const oversized = entries.filter((entry) => entry.size > maxFileSize);

  if (oversized.length)
    return {
      usable: false,
      reason: "MIRROR_FILE_TOO_LARGE",
      oversized: oversized.slice(0, 5).map((entry) => entry.path),
    };

  return {
    usable: true,
    providers: usableProviders(mirror, opts),
    totalBytes: entries.reduce((sum, entry) => sum + entry.size, 0),
    files: entries.length,
  };
}

function cancelled() {
  return new DeliveryError("CANCELLED", "Восстановление отменено");
}

class RepairMirrorClient {
  constructor(options) {
    const opts = options || {};

    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.timeout = opts.timeout || DEFAULT_TIMEOUT_MS;
    this.maxAttempts = opts.maxAttempts || DEFAULT_ATTEMPTS;
    this.request = opts.request || httpClient.request;
    this.rateLimiter = opts.rateLimiter || null;
  }

  // Файл идёт потоком в <target>.part с подсчётом SHA-256 и только после
  // сверки с манифестом становится target: в памяти целиком он не бывает.
  async fetchOne(baseUrl, entry, layout, target, options) {
    const opts = options || {};
    const signal = opts.signal;
    const url = new URL(fileUrl(baseUrl, entry.sha256, layout));

    if (url.protocol !== "https:" && !(url.protocol === "http:" && httpClient.isLoopback(url.hostname)))
      throw new DeliveryError(
        "INSECURE_REDIRECT",
        `Зеркало ремонта допускается только по HTTPS: ${url.protocol}`
      );

    const part = `${target}.part`;
    const hash = crypto.createHash("sha256");
    let received = 0;

    try {
      const response = await this.request(url.toString(), { timeout: this.timeout, signal });

      if (response.statusCode !== 200) {
        response.resume();
        throw new DeliveryError(
          "REPAIR_SOURCE_MISSING",
          `Файл ${entry.path} недоступен в зеркале ремонта: HTTP ${response.statusCode}`,
          { statusCode: response.statusCode, path: entry.path }
        );
      }

      const meter = new Transform({
        transform(chunk, encoding, callback) {
          received += chunk.length;
          if (received > entry.size)
            return callback(
              new DeliveryError("SIZE_MISMATCH", `Файл ${entry.path} из зеркала длиннее манифеста`, { path: entry.path })
            );
          hash.update(chunk);
          if (opts.onBytes) opts.onBytes(chunk.length);
          callback(null, chunk);
        },
      });

      const streams = [response];
      if (this.rateLimiter) streams.push(this.rateLimiter.createStream());
      streams.push(meter, fs.createWriteStream(part));
      await pipeline(...streams, { signal });

      if (received !== entry.size)
        throw new DeliveryError(
          "SIZE_MISMATCH",
          `Размер файла ${entry.path} из зеркала ${received}, ожидался ${entry.size}`,
          { path: entry.path }
        );

      const digest = hash.digest("hex");

      if (digest !== entry.sha256)
        throw new DeliveryError(
          "CHECKSUM_MISMATCH",
          `Файл ${entry.path} из зеркала не совпал с манифестом`,
          { path: entry.path, expected: entry.sha256, actual: digest }
        );

      await renameSettled(part, target, signal);
      return received;
    } catch (error) {
      fs.rmSync(part, { force: true });
      if (signal && signal.aborted) throw cancelled();
      if (error instanceof DeliveryError) throw error;
      // Ошибка записи на диск — не сеть: повтор с другого зеркала не поможет.
      if (DISK_ERRORS[error.code])
        throw new DeliveryError(DISK_ERRORS[error.code], `Не удалось записать ${entry.path}: ${error.message}`, { path: entry.path });
      throw new DeliveryError("NETWORK_ERROR", `Обрыв загрузки ${entry.path}: ${error.message}`, { path: entry.path });
    }
  }

  async fetchInto(targetRoot, entries, options) {
    const opts = options || {};
    const providers = opts.providers || [];
    const layout = opts.layout;
    const signal = opts.signal;
    const total = entries.reduce((sum, entry) => sum + entry.size, 0);

    fs.mkdirSync(targetRoot, { recursive: true });

    const fetched = [];
    let bytes = 0;
    let reportedAt = 0;

    const report = (force) => {
      const now = Date.now();
      if (!opts.onProgress || (!force && now - reportedAt < PROGRESS_EVERY_MS)) return;
      reportedAt = now;
      opts.onProgress({
        fetched: fetched.length,
        total: entries.length,
        bytes,
        totalBytes: total,
        percent: total ? (bytes / total) * 100 : (fetched.length / entries.length) * 100,
      });
    };

    for (const entry of entries) {
      const target = fileOps.resolveInside(targetRoot, entry.path, `Эталон "${entry.path}"`);
      fs.mkdirSync(path.dirname(target), { recursive: true });

      let done = false;
      let lastError = null;

      for (const provider of providers) {
        for (let attempt = 0; attempt < this.maxAttempts && !done; attempt++) {
          if (signal && signal.aborted) throw cancelled();
          let partial = 0;
          try {
            await this.fetchOne(provider.url, entry, layout, target, {
              signal,
              onBytes: (count) => {
                partial += count;
                bytes += count;
                report(false);
              },
            });
            done = true;
          } catch (error) {
            bytes -= partial;
            lastError = error;
            if (["CANCELLED", "NO_SPACE", "PATH_NOT_WRITABLE"].includes(error.code)) throw error;
            if (error.code === "REPAIR_SOURCE_MISSING") break;
          }
        }

        if (done) break;
      }

      if (!done)
        throw lastError || new DeliveryError("REPAIR_SOURCE_MISSING", `Файл ${entry.path} недоступен в зеркале`, { path: entry.path });

      fetched.push(entry.path);
      report(true);
    }

    return { files: fetched, bytes };
  }
}

module.exports = {
  RepairMirrorClient,
  fileUrl,
  plan,
  unavailableReason,
  LAYOUTS,
  MAX_FILES,
  SHA256_PATTERN,
};
