"use strict";

const fs = require("fs");
const http = require("http");
const https = require("https");
const crypto = require("crypto");
const { URL } = require("url");

const USER_AGENT = "HLSR-release-verify";

const LOCAL_HOSTS = ["127.0.0.1", "localhost", "::1"];

const FRESH_SOCKET_NO_KEEPALIVE_HANG_UP = false;

function transportFor(parsed, url) {
  if (parsed.protocol === "https:") return https;

  if (parsed.protocol === "http:" && LOCAL_HOSTS.indexOf(parsed.hostname) !== -1) return http;

  throw new Error(`Только https (или http на localhost): ${url}`);
}

function request(url, options) {
  const opts = options || {};

  return new Promise((resolve, reject) => {
    const parsed = new URL(url);

    let transport;
    try {
      transport = transportFor(parsed, url);
    } catch (error) {
      return reject(error);
    }

    const req = transport.request(
      {
        hostname: parsed.hostname,
        port: parsed.port || 443,
        path: parsed.pathname + parsed.search,
        method: opts.method || "GET",
        headers: Object.assign({ "user-agent": USER_AGENT }, opts.headers || {}),
        timeout: opts.timeout || 30000,
        agent: FRESH_SOCKET_NO_KEEPALIVE_HANG_UP,
      },
      resolve
    );

    req.on("timeout", () => req.destroy(new Error("таймаут")));
    req.on("error", reject);
    req.end();
  });
}

function readBody(response, maxBytes) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let total = 0;

    response.on("data", (chunk) => {
      total += chunk.length;
      if (maxBytes && total > maxBytes) {
        response.destroy();
        return reject(new Error(`Ответ больше ${maxBytes} байт`));
      }
      chunks.push(chunk);
    });

    response.on("end", () => resolve(Buffer.concat(chunks)));
    response.on("error", reject);
  });
}

async function head(url) {
  const response = await request(url, { method: "HEAD" });
  response.resume();

  return {
    statusCode: response.statusCode,
    contentLength: response.headers["content-length"] ? Number(response.headers["content-length"]) : null,
    contentType: response.headers["content-type"] || null,
    etag: response.headers.etag || null,
    lastModified: response.headers["last-modified"] || null,
    acceptRanges: response.headers["accept-ranges"] || null,
    cacheControl: response.headers["cache-control"] || null,
    nosniff: response.headers["x-content-type-options"] || null,
  };
}

async function getRange(url, start, end) {
  const response = await request(url, {
    headers: { range: `bytes=${start}-${end}` },
  });

  const body = await readBody(response, end - start + 1 + 1024);

  return {
    statusCode: response.statusCode,
    contentRange: response.headers["content-range"] || null,
    body,
  };
}

function cacheExpectationFor(url) {
  const pathname = (() => {
    try {
      return new URL(url).pathname;
    } catch (e) {
      return String(url);
    }
  })();

  if (/\/(stable|qa|beta|test)\.json$/.test(pathname)) return "no-cache";
  if (/\.(7z|zip|exe|blockmap)$/.test(pathname)) return "immutable";
  if (/\.files\.json$/.test(pathname)) return "immutable";

  return null;
}

function localSlice(file, start, length) {
  const fd = fs.openSync(file, "r");
  try {
    const buffer = Buffer.allocUnsafe(length);
    const read = fs.readSync(fd, buffer, 0, length, start);
    return buffer.subarray(0, read);
  } finally {
    fs.closeSync(fd);
  }
}

async function verifyArtifact(item, options) {
  const opts = options || {};
  const report = { url: item.url, ok: true, checks: [] };

  const push = (name, ok, detail, blocking) => {
    report.checks.push({ name, ok, detail, blocking: blocking !== false });
    if (!ok && blocking !== false) report.ok = false;
  };

  let headInfo;
  try {
    headInfo = await head(item.url);
  } catch (error) {
    push("HEAD", false, error.message);
    return report;
  }

  push("HEAD 200", headInfo.statusCode === 200, `HTTP ${headInfo.statusCode}`);
  if (item.size != null)
    push(
      "Content-Length",
      headInfo.contentLength === item.size,
      `${headInfo.contentLength} (ожидалось ${item.size})`
    );
  push("Content-Type", !!headInfo.contentType, headInfo.contentType || "нет");
  push("ETag/Last-Modified", !!(headInfo.etag || headInfo.lastModified),
    `etag=${headInfo.etag || "—"} last-modified=${headInfo.lastModified || "—"}`);
  push("Accept-Ranges", headInfo.acceptRanges === "bytes", headInfo.acceptRanges || "нет");

  const cacheExpectation =
    item.cacheControl === undefined ? cacheExpectationFor(item.url) : item.cacheControl;
  const cacheValue = headInfo.cacheControl || "";

  if (cacheExpectation === "immutable")
    push(
      "Cache-Control: долгий кеш",
      /immutable/i.test(cacheValue) || /max-age=\d{5,}/.test(cacheValue),
      cacheValue || "не задан",
      false
    );
  else if (cacheExpectation === "no-cache")
    push(
      "Cache-Control: без кеша",
      /no-cache|no-store|must-revalidate|max-age=0/i.test(cacheValue),
      cacheValue || "не задан"
    );
  else push("Cache-Control", true, cacheValue || "не задан", false);

  push("X-Content-Type-Options: nosniff", headInfo.nosniff === "nosniff", headInfo.nosniff || "не задан", false);
  report.headers = headInfo;

  const end = Math.min(1048575, (item.size || 1048576) - 1);
  try {
    const range = await getRange(item.url, 0, end);
    push("Range 0-1MiB -> 206", range.statusCode === 206, `HTTP ${range.statusCode}`);
    push("Content-Range", /^bytes 0-/.test(range.contentRange || ""), range.contentRange || "нет");

    if (item.localPath && range.statusCode === 206) {
      const local = localSlice(item.localPath, 0, range.body.length);
      push("байты 0-1MiB совпали", local.equals(range.body), `${range.body.length} байт`);
    }
  } catch (error) {
    push("Range", false, error.message);
  }

  if (item.localPath && item.size > 2 * 1048576) {
    const start = item.size - 1048576;
    try {
      const range = await getRange(item.url, start, item.size - 1);
      push("Range хвоста -> 206", range.statusCode === 206, `HTTP ${range.statusCode}`);
      if (range.statusCode === 206) {
        const local = localSlice(item.localPath, start, range.body.length);
        push("байты хвоста совпали", local.equals(range.body), `${range.body.length} байт`);
      }
    } catch (error) {
      push("Range хвоста", false, error.message);
    }
  }

  if (item.sha256) {
    if (opts.onProgress)
      opts.onProgress(`скачиваю целиком для SHA-256 (${item.size || "?"} байт)...`);

    try {
      const started = Date.now();
      const downloaded = await downloadFullWithRetry(item.url, opts.onProgress);
      const seconds = (Date.now() - started) / 1000;

      push(
        "SHA-256 всего файла",
        downloaded.sha256 === item.sha256,
        `${downloaded.sha256} (${downloaded.size} байт за ${seconds.toFixed(1)} с)`
      );

      if (item.size != null)
        push(
          "размер скачанного тела",
          downloaded.size === item.size,
          `${downloaded.size} (ожидалось ${item.size})`
        );
    } catch (error) {
      push("SHA-256 всего файла", false, error.message);
    }
  } else {
    push("SHA-256 всего файла", false, "ожидаемый SHA-256 не задан — проверить нечем");
  }

  return report;
}

const INTERRUPTED_RE = /aborted|ECONNRESET|socket hang up|таймаут|ETIMEDOUT|EPIPE/i;
const FULL_DOWNLOAD_TIMEOUT_MS = 120000;
const FULL_DOWNLOAD_ATTEMPTS = 3;

async function downloadFullWithRetry(url, onProgress) {
  let lastError = null;

  for (let attempt = 1; attempt <= FULL_DOWNLOAD_ATTEMPTS; attempt++) {
    try {
      return await downloadSha256(url, null, { timeout: FULL_DOWNLOAD_TIMEOUT_MS });
    } catch (error) {
      lastError = error;

      if (!INTERRUPTED_RE.test(String(error && error.message))) throw error;

      if (attempt < FULL_DOWNLOAD_ATTEMPTS && onProgress)
        onProgress(`соединение оборвалось (${error.message}), попытка ${attempt + 1}...`);
    }
  }

  throw lastError;
}

async function downloadSha256(url, maxBytes, options) {
  const opts = options || {};
  const response = await request(url, { timeout: opts.timeout });
  if (response.statusCode !== 200) {
    response.resume();
    throw new Error(`HTTP ${response.statusCode}`);
  }

  return new Promise((resolve, reject) => {
    const hash = crypto.createHash("sha256");
    let total = 0;

    response.on("data", (chunk) => {
      total += chunk.length;
      if (maxBytes && total > maxBytes) {
        response.destroy();
        return reject(new Error(`Файл больше ${maxBytes}`));
      }
      hash.update(chunk);
    });

    response.on("end", () => resolve({ sha256: hash.digest("hex"), size: total }));
    response.on("error", reject);
  });
}

async function parallelRangeTest(url, n) {
  const count = Math.min(n, 10);
  const started = Date.now();

  const jobs = [];
  for (let i = 0; i < count; i++)
    jobs.push(getRange(url, i * 1048576, (i + 1) * 1048576 - 1));

  const results = await Promise.all(jobs);
  const elapsed = Date.now() - started;
  const bytes = results.reduce((sum, r) => sum + r.body.length, 0);

  return {
    parallel: count,
    ok: results.every((r) => r.statusCode === 206),
    statusCodes: results.map((r) => r.statusCode),
    bytes,
    elapsedMs: elapsed,
    mbps: bytes / 1048576 / (elapsed / 1000),
  };
}

module.exports = {
  request,
  readBody,
  head,
  getRange,
  cacheExpectationFor,
  verifyArtifact,
  downloadSha256,
  parallelRangeTest,
};
