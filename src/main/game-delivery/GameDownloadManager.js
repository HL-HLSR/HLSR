"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { EventEmitter } = require("events");
const { Transform } = require("stream");
const { pipeline } = require("stream/promises");

const httpClientDefault = require("./http-client");
const { DeliveryError, toDeliveryError } = require("./errors");
const { assertInside, removeWithinZone } = require("./paths");

const STATE_VERSION = 1;
const DEFAULT_MAX_ATTEMPTS = 5;
const DEFAULT_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 30000;
const PROGRESS_INTERVAL_MS = 300;
const SPEED_SMOOTHING = 0.3;
const FREE_SPACE_RESERVE = 256 * 1024 * 1024;

const RETRYABLE_CODES = [
  "NETWORK_ERROR",
  "NETWORK_TIMEOUT",
  "PROVIDER_LINK_FAILED",
  "PROVIDER_FORBIDDEN",
  "CONTENT_LENGTH_MISMATCH",
  "RANGE_NOT_SUPPORTED",
];

function delay(ms, signal) {
  return new Promise((resolve) => {
    if (signal && signal.aborted) return resolve();
    const done = () => {
      clearTimeout(timer);
      if (signal) signal.removeEventListener("abort", done);
      resolve();
    };
    const timer = setTimeout(done, ms);
    if (signal) signal.addEventListener("abort", done, { once: true });
  });
}

const { freeSpaceBytes } = require("./free-space");

class GameDownloadManager extends EventEmitter {
  constructor(options) {
    super();
    const opts = options || {};

    this.paths = opts.paths;
    this.httpClient = opts.httpClient || httpClientDefault;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.timeout = opts.timeout || httpClientDefault.DEFAULT_TIMEOUT_MS;
    this.maxAttempts = opts.maxAttempts || DEFAULT_MAX_ATTEMPTS;
    this.backoffBaseMs = opts.backoffBaseMs == null ? DEFAULT_BACKOFF_MS : opts.backoffBaseMs;
    this.freeSpaceReserve =
      opts.freeSpaceReserve == null ? FREE_SPACE_RESERVE : opts.freeSpaceReserve;
    this.freeSpaceProvider = opts.freeSpaceProvider || freeSpaceBytes;
    this.rateLimiter = opts.rateLimiter || null;

    this.active = null;
  }

  isBusy() {
    return !!this.active;
  }

  downloadDir(gameId) {
    return assertInside(
      this.paths.downloads,
      path.join(this.paths.downloads, gameId),
      "Папка загрузки"
    );
  }

  partPaths(gameId, filename) {
    const dir = this.downloadDir(gameId);
    const base = path.basename(filename);
    const part = assertInside(dir, path.join(dir, `${base}.part`), "Файл загрузки");

    return { dir, part, state: `${part}.json`, target: path.join(dir, base) };
  }

  readState(statePath) {
    try {
      const data = JSON.parse(fs.readFileSync(statePath, "utf8"));
      return data && data.version === STATE_VERSION ? data : null;
    } catch (e) {
      return null;
    }
  }

  writeState(statePath, state) {
    fs.writeFileSync(
      statePath,
      JSON.stringify(
        {
          version: STATE_VERSION,
          gameId: state.gameId,
          filename: state.filename,
          expectedSize: state.expectedSize,
          expectedSha256: state.expectedSha256,
          providerId: state.providerId,
          bytesDownloaded: state.bytesDownloaded,
          updatedAt: new Date().toISOString(),
        },
        null,
        2
      ),
      "utf8"
    );
  }

  findResumable(gameId, filename) {
    const { part, state } = this.partPaths(gameId, filename);
    const saved = this.readState(state);
    if (!saved) return null;

    let size = 0;
    try {
      size = fs.statSync(part).size;
    } catch (e) {
      return null;
    }

    return {
      gameId: saved.gameId,
      filename: saved.filename,
      expectedSize: saved.expectedSize,
      bytesOnDisk: size,
      bytesDownloaded: Math.min(saved.bytesDownloaded || 0, size),
      updatedAt: saved.updatedAt,
    };
  }

  discard(gameId, filename) {
    const { part, state } = this.partPaths(gameId, filename);
    for (const file of [part, state])
      if (fs.existsSync(file)) removeWithinZone(this.paths.downloads, file);
  }

  // keep(filename) — чужие загрузки в той же папке (эпизоды Ghosting), их отмена игры не трогает.
  discardPaused(gameId, keep) {
    const dir = this.downloadDir(gameId);
    if (!fs.existsSync(dir)) return false;
    let discarded = false;
    for (const file of fs.readdirSync(dir)) {
      if (!file.endsWith(".part.json")) continue;
      const saved = this.readState(path.join(dir, file));
      if (!saved || saved.gameId !== gameId || typeof saved.filename !== "string") continue;
      if (keep && keep(saved.filename)) continue;
      this.discard(gameId, saved.filename);
      discarded = true;
    }
    return discarded;
  }

  pause() {
    if (!this.active) return false;
    this.active.pauseRequested = true;
    this.active.abortController.abort();
    if (this.active.response) this.active.response.destroy();
    return true;
  }

  cancel() {
    if (!this.active) return false;
    this.active.cancelRequested = true;
    this.active.abortController.abort();
    if (this.active.response) this.active.response.destroy();
    return true;
  }

  async start(request) {
    if (this.active)
      throw new DeliveryError("ALREADY_RUNNING", "Уже идёт загрузка другой сборки", {
        gameId: this.active.gameId,
      });

    const { gameId, provider, filename, expectedSize, expectedSha256 } = request;
    const files = this.partPaths(gameId, filename);

    this.checkFreeSpace(path.dirname(files.dir), expectedSize);

    fs.mkdirSync(files.dir, { recursive: true });

    const ctx = {
      gameId,
      provider,
      filename: path.basename(filename),
      expectedSize,
      expectedSha256,
      files,
      bytesDownloaded: 0,
      networkBytes: 0,
      hash: null,
      response: null,
      abortController: new AbortController(),
      pauseRequested: false,
      cancelRequested: false,
      startedAt: Date.now(),
      speedBps: 0,
      lastSample: null,
      lastEmit: 0,
      rangeSupported: true,
      attempt: 0,
    };

    this.active = ctx;

    try {
      this.restore(ctx);
      return await this.run(ctx);
    } catch (error) {
      const delivery = toDeliveryError(error, "NETWORK_ERROR");
      this.emitState(ctx, "error", { error: delivery.toPublic() });
      throw delivery;
    } finally {
      this.active = null;
    }
  }

  checkFreeSpace(dir, needBytes) {
    if (!needBytes) return;

    const free = this.freeSpaceProvider(dir);
    if (free == null) return;

    if (free < needBytes + this.freeSpaceReserve)
      throw new DeliveryError(
        "NO_SPACE",
        `Недостаточно места: нужно ${needBytes + this.freeSpaceReserve}, свободно ${free}`,
        { requiredBytes: needBytes + this.freeSpaceReserve, freeBytes: free }
      );
  }

  restore(ctx) {
    const saved = this.readState(ctx.files.state);
    let onDisk = 0;

    try {
      onDisk = fs.statSync(ctx.files.part).size;
    } catch (e) {
      onDisk = 0;
    }

    if (!saved || saved.expectedSha256 !== ctx.expectedSha256 || saved.expectedSize !== ctx.expectedSize) {
      if (onDisk) this.discard(ctx.gameId, ctx.filename);
      ctx.bytesDownloaded = 0;
      return;
    }

    ctx.bytesDownloaded = Math.min(saved.bytesDownloaded || 0, onDisk);
    if (onDisk > ctx.bytesDownloaded) fs.truncateSync(ctx.files.part, ctx.bytesDownloaded);
  }

  async run(ctx) {
    for (;;) {
      ctx.attempt++;

      try {
        await this.attempt(ctx);

        const digest = ctx.hash.digest("hex");
        this.finalize(ctx, digest);

        return {
          status: "completed",
          filePath: ctx.files.target,
          sha256: digest,
          size: ctx.bytesDownloaded,
          networkBytes: ctx.networkBytes,
          durationMs: Math.max(0, Date.now() - ctx.startedAt),
        };
      } catch (error) {
        if (ctx.cancelRequested) {
          this.discard(ctx.gameId, ctx.filename);
          this.emitState(ctx, "cancelled");
          return { status: "cancelled", networkBytes: ctx.networkBytes };
        }

        if (ctx.pauseRequested) {
          this.persist(ctx);
          this.emitState(ctx, "paused");
          return { status: "paused", bytesDownloaded: ctx.bytesDownloaded };
        }

        const delivery = toDeliveryError(error, "NETWORK_ERROR");

        if (fs.existsSync(ctx.files.part)) this.persist(ctx);

        if (RETRYABLE_CODES.indexOf(delivery.code) === -1 || ctx.attempt >= this.maxAttempts) {
          throw delivery;
        }

        const wait = Math.min(
          MAX_BACKOFF_MS,
          Math.round(this.backoffBaseMs * Math.pow(2, ctx.attempt - 1) * (0.8 + Math.random() * 0.4))
        );

        this.logger.warn(
          `[game-delivery] попытка ${ctx.attempt} не удалась (${delivery.code}), повтор через ${wait} мс`
        );
        this.emitState(ctx, "retrying", {
          attempt: ctx.attempt,
          maxAttempts: this.maxAttempts,
          retryInMs: wait,
          error: delivery.toPublic(),
        });

        await delay(wait, ctx.abortController.signal);
        ctx.hash = null;
      }
    }
  }

  syncWithDisk(ctx) {
    let onDisk = 0;
    try {
      onDisk = fs.statSync(ctx.files.part).size;
    } catch (e) {
      onDisk = 0;
    }

    if (onDisk < ctx.bytesDownloaded) ctx.bytesDownloaded = onDisk;
    else if (onDisk > ctx.bytesDownloaded) fs.truncateSync(ctx.files.part, ctx.bytesDownloaded);
  }

  async attempt(ctx) {
    if (!ctx.hash) this.syncWithDisk(ctx);

    const resolved = await ctx.provider.resolve({ signal: ctx.abortController.signal });
    ctx.abortController.signal.throwIfAborted();
    const headers = Object.assign({}, resolved.headers);

    if (ctx.bytesDownloaded > 0 && ctx.provider.supportsRange())
      headers.range = `bytes=${ctx.bytesDownloaded}-`;

    this.emitState(ctx, ctx.bytesDownloaded > 0 ? "resuming" : "preparing", {
      providerId: ctx.provider.id,
    });

    const response = await this.httpClient.request(resolved.url, {
      headers,
      timeout: this.timeout,
      signal: ctx.abortController.signal,
    });

    ctx.response = response;

    const status = response.statusCode;
    let start = ctx.bytesDownloaded;

    this.logger.info(
      `[game-delivery] HTTP ${status} от ${response.finalUrl ? response.finalUrl.host : "?"}` +
        `, запрошено с байта ${start}` +
        (response.headers["content-range"]
          ? `, content-range: ${response.headers["content-range"]}`
          : "") +
        `, content-length: ${response.headers["content-length"] || "?"}`
    );

    if (status === 416) {
      response.resume();
      this.resetProgress(ctx);
      throw new DeliveryError("RANGE_NOT_SUPPORTED", "Сервер отклонил диапазон, файл начнётся заново");
    }

    if (ctx.provider.shouldRefreshOn(status)) {
      response.resume();
      throw new DeliveryError(
        "PROVIDER_FORBIDDEN",
        `Источник ответил ${status} — нужна новая ссылка`
      );
    }

    if (status !== 200 && status !== 206) {
      response.resume();
      const retryable = status >= 500 || status === 429 || status === 408;
      throw new DeliveryError(
        retryable ? "NETWORK_ERROR" : "HTTP_ERROR",
        `Источник ответил HTTP ${status}`,
        { status, retryAfter: response.headers["retry-after"] || null }
      );
    }

    if (start > 0) {
      if (status === 206) {
        this.checkContentRange(ctx, response.headers["content-range"], start);
      } else {
        this.logger.warn("[game-delivery] сервер не поддержал Range, файл скачивается заново");
        ctx.rangeSupported = false;
        this.resetProgress(ctx);
        start = 0;
      }
    }

    this.checkContentLength(ctx, response.headers["content-length"], start);

    if (!ctx.hash) await this.rebuildHash(ctx, start);

    this.emitState(ctx, "downloading", { providerId: ctx.provider.id });
    await this.pump(ctx, response, start);

    if (ctx.expectedSize && ctx.bytesDownloaded !== ctx.expectedSize)
      throw new DeliveryError(
        "CONTENT_LENGTH_MISMATCH",
        `Получено ${ctx.bytesDownloaded} байт вместо ${ctx.expectedSize}`
      );
  }

  checkContentRange(ctx, header, start) {
    const match = String(header || "").match(/^bytes\s+(\d+)-(\d+)\/(\d+|\*)$/i);

    if (!match)
      throw new DeliveryError("RANGE_NOT_SUPPORTED", `Некорректный Content-Range: ${header}`);

    if (Number(match[1]) !== start)
      throw new DeliveryError(
        "RANGE_NOT_SUPPORTED",
        `Сервер продолжил с ${match[1]} вместо ${start}`
      );

    if (ctx.expectedSize && match[3] !== "*" && Number(match[3]) !== ctx.expectedSize)
      throw new DeliveryError(
        "SIZE_MISMATCH",
        `Размер файла на источнике ${match[3]} не совпадает с манифестом ${ctx.expectedSize}`
      );
  }

  checkContentLength(ctx, header, start) {
    if (header == null || !ctx.expectedSize) return;

    const length = Number(header);
    if (!Number.isFinite(length)) return;

    const expected = ctx.expectedSize - start;
    if (length !== expected)
      throw new DeliveryError(
        "CONTENT_LENGTH_MISMATCH",
        `Content-Length ${length} не совпадает с ожидаемым ${expected}`
      );
  }

  resetProgress(ctx) {
    ctx.bytesDownloaded = 0;
    ctx.hash = null;
    try {
      fs.writeFileSync(ctx.files.part, "");
    } catch (e) {
      /* файла может не быть — это нормально */
    }
  }

  async rebuildHash(ctx, bytes) {
    const hash = crypto.createHash("sha256");

    if (bytes > 0) {
      const handle = await fs.promises.open(ctx.files.part, "r");
      const buffer = Buffer.allocUnsafe(4 * 1024 * 1024);
      let read = 0;

      try {
        while (read < bytes) {
          ctx.abortController.signal.throwIfAborted();
          const size = Math.min(buffer.length, bytes - read);
          const { bytesRead } = await handle.read(buffer, 0, size, read);
          if (bytesRead <= 0) break;
          hash.update(buffer.subarray(0, bytesRead));
          read += bytesRead;
        }
      } finally {
        await handle.close();
      }

      if (read !== bytes)
        throw new DeliveryError("NETWORK_ERROR", "Не удалось перечитать уже скачанную часть");
    }

    ctx.hash = hash;
  }

  async pump(ctx, response, start) {
    const writer = fs.createWriteStream(ctx.files.part, {
      flags: start > 0 ? "r+" : "w",
      start,
    });
    let writeError = null;
    writer.on("error", (error) => {
      if (["open", "write", "writev"].includes(error.syscall)) writeError = error;
    });
    const meter = new Transform({
      transform: (chunk, encoding, callback) => {
        ctx.hash.update(chunk);
        ctx.bytesDownloaded += chunk.length;
        ctx.networkBytes += chunk.length;
        this.sampleProgress(ctx);
        callback(null, chunk);
      },
    });
    const streams = [response];
    if (this.rateLimiter) streams.push(this.rateLimiter.createStream());
    streams.push(meter, writer);

    try {
      await pipeline(...streams, { signal: ctx.abortController.signal });
      if (ctx.cancelRequested || ctx.pauseRequested)
        throw new DeliveryError("CANCELLED", "Загрузка остановлена");
    } catch (error) {
      if (writeError && !ctx.pauseRequested && !ctx.cancelRequested)
        throw new DeliveryError("PATH_NOT_WRITABLE", `Ошибка записи: ${writeError.message}`);
      throw error;
    } finally {
      ctx.response = null;
    }
  }

  sampleProgress(ctx) {
    const now = Date.now();

    if (!ctx.lastSample) {
      ctx.lastSample = { at: now, bytes: ctx.bytesDownloaded };
      return;
    }

    if (now - ctx.lastEmit < PROGRESS_INTERVAL_MS) return;

    const elapsed = (now - ctx.lastSample.at) / 1000;
    if (elapsed > 0) {
      const instant = (ctx.bytesDownloaded - ctx.lastSample.bytes) / elapsed;
      ctx.speedBps = ctx.speedBps
        ? ctx.speedBps * (1 - SPEED_SMOOTHING) + instant * SPEED_SMOOTHING
        : instant;
    }

    ctx.lastSample = { at: now, bytes: ctx.bytesDownloaded };
    ctx.lastEmit = now;

    this.persist(ctx);
    this.emit("progress", this.progressOf(ctx));
  }

  progressOf(ctx) {
    const total = ctx.expectedSize || 0;
    const remaining = Math.max(0, total - ctx.bytesDownloaded);

    return {
      gameId: ctx.gameId,
      bytesDownloaded: ctx.bytesDownloaded,
      totalBytes: total,
      percent: total ? Math.min(100, (ctx.bytesDownloaded / total) * 100) : 0,
      speedBps: Math.max(0, Math.round(ctx.speedBps)),
      etaSeconds: ctx.speedBps > 1 ? Math.round(remaining / ctx.speedBps) : null,
      providerId: ctx.provider ? ctx.provider.id : null,
      rangeSupported: ctx.rangeSupported,
    };
  }

  persist(ctx) {
    try {
      this.writeState(ctx.files.state, {
        gameId: ctx.gameId,
        filename: ctx.filename,
        expectedSize: ctx.expectedSize,
        expectedSha256: ctx.expectedSha256,
        providerId: ctx.provider ? ctx.provider.id : null,
        bytesDownloaded: ctx.bytesDownloaded,
      });
    } catch (e) {
      this.logger.warn(`[game-delivery] не удалось сохранить состояние загрузки: ${e.message}`);
    }
  }

  finalize(ctx, digest) {
    if (ctx.expectedSha256 && digest !== ctx.expectedSha256) {
      this.discard(ctx.gameId, ctx.filename);
      throw new DeliveryError(
        "CHECKSUM_MISMATCH",
        `SHA-256 не совпал: получено ${digest}, ожидалось ${ctx.expectedSha256}`,
        { expected: ctx.expectedSha256, actual: digest }
      );
    }

    fs.renameSync(ctx.files.part, ctx.files.target);
    if (fs.existsSync(ctx.files.state)) removeWithinZone(this.paths.downloads, ctx.files.state);

    this.emitState(ctx, "downloaded", { filePath: ctx.files.target, sha256: digest });
  }

  emitState(ctx, state, extra) {
    this.emit("state", Object.assign({ state }, this.progressOf(ctx), extra || {}));
  }
}

module.exports = GameDownloadManager;
module.exports.STATE_VERSION = STATE_VERSION;
module.exports.RETRYABLE_CODES = RETRYABLE_CODES;
