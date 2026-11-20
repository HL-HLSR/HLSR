"use strict";

const fs = require("fs");
const path = require("path");
const http = require("http");
const https = require("https");
const { URL } = require("url");

const { CODES, UpdaterError } = require("./errors");

const DEFAULT_TIMEOUT_MS = 30000;
const MAX_REDIRECTS = 5;
const SPEED_WINDOW_MS = 3000;
const FREE_SPACE_RESERVE = 400 * 1024 * 1024;

const { freeSpaceBytes } = require("../shared/game-delivery/free-space");

class SpeedMeter {
  constructor(now) {
    this.now = now || (() => Date.now());
    this.points = [];
  }

  push(totalBytes) {
    const ts = this.now();
    this.points.push({ ts, bytes: totalBytes });

    while (this.points.length > 2 && ts - this.points[0].ts > SPEED_WINDOW_MS)
      this.points.shift();
  }

  bytesPerSecond() {
    if (this.points.length < 2) return 0;

    const first = this.points[0];
    const last = this.points[this.points.length - 1];
    const seconds = (last.ts - first.ts) / 1000;
    if (seconds <= 0) return 0;

    return Math.max(0, (last.bytes - first.bytes) / seconds);
  }
}

function request(url, options) {
  const opts = options || {};
  const redirectsLeft = opts.redirectsLeft == null ? MAX_REDIRECTS : opts.redirectsLeft;

  return new Promise((resolve, reject) => {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (e) {
      return reject(new UpdaterError(CODES.URL_NOT_ALLOWED, "неразбираемый URL"));
    }

    const transport = parsed.protocol === "http:" ? http : https;
    const headers = Object.assign({ "user-agent": "HLSR-Updater" }, opts.headers || {});

    const req = transport.get(
      {
        hostname: parsed.hostname,
        port: parsed.port || (parsed.protocol === "http:" ? 80 : 443),
        path: parsed.pathname + parsed.search,
        headers,
        timeout: opts.timeout || DEFAULT_TIMEOUT_MS,
      },
      (response) => {
        const status = response.statusCode;

        if (status >= 301 && status <= 308 && response.headers.location) {
          response.resume();
          if (!redirectsLeft)
            return reject(new UpdaterError(CODES.DOWNLOAD_HTTP, "слишком много редиректов"));

          const next = new URL(response.headers.location, url).toString();
          if (opts.isAllowedUrl && !opts.isAllowedUrl(next))
            return reject(new UpdaterError(CODES.URL_NOT_ALLOWED, "редирект вне allowlist"));

          return resolve(
            request(next, Object.assign({}, opts, { redirectsLeft: redirectsLeft - 1 }))
          );
        }

        if (status !== 200 && status !== 206) {
          response.resume();
          return reject(new UpdaterError(CODES.DOWNLOAD_HTTP, `HTTP ${status}`));
        }

        resolve(response);
      }
    );

    req.on("timeout", () => req.destroy(new UpdaterError(CODES.DOWNLOAD_TIMEOUT, "таймаут")));
    req.on("error", (error) =>
      reject(new UpdaterError(CODES.DOWNLOAD_FAILED, error ? error.message : "network"))
    );

    if (opts.signal)
      opts.signal.addEventListener("abort", () => req.destroy(), { once: true });
  });
}

async function download(options) {
  const opts = options || {};
  const dir = opts.dir;
  const target = path.join(dir, opts.filename);
  const partial = `${target}.part`;
  const total = opts.size;
  const attempts = opts.attempts == null ? 4 : opts.attempts;
  const onProgress = opts.onProgress || (() => {});
  const isCancelled = opts.isCancelled || (() => false);
  const requestFn = opts.request || request;

  fs.mkdirSync(dir, { recursive: true });

  const free = (opts.freeSpaceBytes || freeSpaceBytes)(dir);
  if (free != null && free < total + (opts.reserve == null ? FREE_SPACE_RESERVE : opts.reserve))
    throw new UpdaterError(
      CODES.NO_SPACE,
      `свободно ${free} байт, нужно ${total} + запас`
    );

  if (fs.existsSync(target) && fs.statSync(target).size === total) return target;

  const meter = new SpeedMeter(opts.now);
  let lastError = null;

  for (let attempt = 0; attempt <= attempts; attempt++) {
    if (isCancelled()) throw new UpdaterError(CODES.CANCELLED, "отменено пользователем");

    let done = fs.existsSync(partial) ? fs.statSync(partial).size : 0;

    if (done > total) {
      fs.unlinkSync(partial);
      done = 0;
    }

    if (done === total) break;

    const url = opts.urls[Math.min(attempt, opts.urls.length - 1)];

    try {
      const headers = done > 0 ? { range: `bytes=${done}-` } : {};
      const response = await requestFn(url, {
        headers,
        timeout: opts.timeout,
        isAllowedUrl: opts.isAllowedUrl,
      });

      const resuming = done > 0 && response.statusCode === 206;
      if (done > 0 && !resuming) {
        try {
          fs.unlinkSync(partial);
        } catch (e) {
          /* уже нет */
        }
        done = 0;
      }

      await new Promise((resolve, reject) => {
        const out = fs.createWriteStream(partial, { flags: resuming ? "a" : "w" });
        let settled = false;

        const fail = (error) => {
          if (settled) return;
          settled = true;
          response.destroy();
          out.end(() => reject(error));
        };

        response.on("data", (chunk) => {
          if (isCancelled()) return fail(new UpdaterError(CODES.CANCELLED, "отменено"));

          done += chunk.length;

          if (done > total) return fail(new UpdaterError(CODES.DOWNLOAD_SIZE, "ответ длиннее размера"));

          meter.push(done);
          onProgress({ done, total, bytesPerSecond: meter.bytesPerSecond() });
        });

        response.on("error", (error) =>
          fail(new UpdaterError(CODES.DOWNLOAD_FAILED, error ? error.message : "network"))
        );
        out.on("error", (error) =>
          fail(new UpdaterError(CODES.DOWNLOAD_FAILED, error ? error.message : "write"))
        );

        out.on("close", () => {
          if (settled) return;
          settled = true;
          resolve();
        });

        response.pipe(out);
      });

      if (done === total) break;

      lastError = new UpdaterError(CODES.DOWNLOAD_FAILED, `получено ${done} из ${total}`);
    } catch (error) {
      if (error instanceof UpdaterError && error.code === CODES.CANCELLED) throw error;
      lastError = error;
    }
  }

  const finalSize = fs.existsSync(partial) ? fs.statSync(partial).size : 0;
  if (finalSize !== total)
    throw lastError || new UpdaterError(CODES.DOWNLOAD_SIZE, `получено ${finalSize} из ${total}`);

  fs.renameSync(partial, target);
  return target;
}

module.exports = { download, SpeedMeter, freeSpaceBytes, request, FREE_SPACE_RESERVE };
