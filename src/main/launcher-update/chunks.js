"use strict";

// Обновление лаунчера чанками.
//
// При выпуске каждый файл сборки режется на куски по содержимому
// (content-defined chunking: граница ставится там, где скользящий
// gear-хеш даёт нужные нули, поэтому правка в середине файла меняет только
// соседние куски, а не всё, что после неё). Кусок называется своим SHA-256,
// сжимается brotli и кладётся в общее хранилище chunks/<aa>/<sha256>:
// одинаковые куски разных версий хранятся один раз.
//
// Список файлов версии и их кусков — манифест чанков. Его SHA-256 и адрес
// лежат в подписанном launcher-channel.json (блок delta), так что манифест
// и через него каждый кусок проверяются той же подписью, что и установщик.
//
// Клиент собирает новую версию рядом со старой: совпавшие целиком файлы
// копирует, куски изменённых файлов, которые уже есть в старой версии,
// берёт с диска, а скачивает только недостающие. Каждый кусок и каждый
// собранный файл сверяются по SHA-256.
//
// Модуль общий для лаунчера (Linux), апдейтера (Windows) и скриптов
// выпуска, поэтому без Electron и только на CommonJS.

const crypto = require("crypto");
const path = require("path");
const zlib = require("zlib");
const fs = require("./fs-compat");

const SCHEMA = 1;
const KIND = "hlsr-launcher-chunks";
// Параметры нарезки — часть формата: клиент режет старые файлы так же, как
// скрипт выпуска резал новые. Менять только вместе с ALGORITHM.
const ALGORITHM = "gear-cdc-v1";
const MIN_CHUNK = 16 * 1024;
const AVG_CHUNK = 64 * 1024;
const MAX_CHUNK = 256 * 1024;
const MASK_STRICT = (1 << 17) - 1;
const MASK_LOOSE = (1 << 15) - 1;
const COMPRESSION = "br";

const MAX_MANIFEST_BYTES = 16 * 1024 * 1024;
const MAX_FILES = 20000;
const MAX_FILE_BYTES = 1024 * 1024 * 1024;
const MAX_COMPRESSED_CHUNK = MAX_CHUNK + 4096;
const SHA256_RE = /^[a-f0-9]{64}$/;
const VERSION_RE = /^\d+\.\d+\.\d+$/;
const PLATFORMS = ["linux", "win32"];

const DOWNLOAD_CONCURRENCY = 8;
const DOWNLOAD_ATTEMPTS = 3;
const REQUEST_TIMEOUT_MS = 30000;
const MAX_REDIRECTS = 5;

const GEAR = (() => {
  const table = new Uint32Array(256);
  let state = 0x9e3779b9;
  for (let i = 0; i < 256; i++) {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    table[i] = state >>> 0;
  }
  return table;
})();

// Границы кусков буфера: [[начало, конец), ...].
function cut(buffer) {
  const out = [];
  const length = buffer.length;
  let start = 0;
  while (start < length) {
    const end = Math.min(start + MAX_CHUNK, length);
    let i = start + MIN_CHUNK;
    if (i >= end) {
      out.push([start, end]);
      start = end;
      continue;
    }
    let boundary = end;
    let hash = 0;
    const middle = Math.min(start + AVG_CHUNK, end);
    for (; i < middle; i++) {
      hash = ((hash << 1) + GEAR[buffer[i]]) >>> 0;
      if (!(hash & MASK_STRICT)) { boundary = i + 1; break; }
    }
    if (boundary === end)
      for (; i < end; i++) {
        hash = ((hash << 1) + GEAR[buffer[i]]) >>> 0;
        if (!(hash & MASK_LOOSE)) { boundary = i + 1; break; }
      }
    out.push([start, boundary]);
    start = boundary;
  }
  return out;
}

function sha256(data) {
  return crypto.createHash("sha256").update(data).digest("hex");
}

function sha256File(file) {
  const hash = crypto.createHash("sha256");
  const fd = fs.openSync(file, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    let read;
    while ((read = fs.readSync(fd, buffer, 0, buffer.length, null)) > 0) hash.update(buffer.subarray(0, read));
  } finally {
    fs.closeSync(fd);
  }
  return hash.digest("hex");
}

function chunkPath(hash) {
  return `${hash.slice(0, 2)}/${hash}`;
}

function chunkUrl(base, hash) {
  return `${String(base).replace(/\/+$/, "")}/${chunkPath(hash)}`;
}

function compress(data) {
  return zlib.brotliCompressSync(data, {
    params: {
      [zlib.constants.BROTLI_PARAM_QUALITY]: 9,
      [zlib.constants.BROTLI_PARAM_SIZE_HINT]: data.length,
    },
  });
}

function decompress(data, expectedSize) {
  return zlib.brotliDecompressSync(data, { maxOutputLength: Math.max(expectedSize, 1) });
}

function listTree(root, relative = "", out = []) {
  for (const entry of fs.readdirSync(path.join(root, relative), { withFileTypes: true })) {
    const child = relative ? `${relative}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`Ссылка в сборке: ${child}`);
    if (entry.isDirectory()) listTree(root, child, out);
    else if (entry.isFile()) out.push(child);
    else throw new Error(`Неизвестный объект в сборке: ${child}`);
  }
  return out;
}

// Манифест чанков каталога сборки. onChunk(hash, data) вызывается один раз
// на каждый новый кусок: скрипт выпуска пишет его в хранилище.
function describeTree(root, options = {}) {
  const { version, platform } = options;
  const onChunk = options.onChunk || (() => {});
  const exclude = new Set(options.exclude || []);
  const seen = new Set();
  const files = [];
  for (const relative of listTree(root).sort()) {
    if (exclude.has(relative)) continue;
    const full = path.join(root, ...relative.split("/"));
    const stat = fs.statSync(full);
    const data = fs.readFileSync(full);
    const chunks = [];
    for (const [start, end] of cut(data)) {
      const piece = data.subarray(start, end);
      const hash = sha256(piece);
      if (!seen.has(hash)) {
        seen.add(hash);
        onChunk(hash, piece);
      }
      chunks.push([hash, end - start]);
    }
    const entry = { path: relative, size: data.length, sha256: sha256(data), chunks };
    if (platform === "linux" && stat.mode & 0o111) entry.exec = true;
    files.push(entry);
  }
  return {
    schema: SCHEMA, kind: KIND, version, platform,
    chunking: { algorithm: ALGORITHM, min: MIN_CHUNK, avg: AVG_CHUNK, max: MAX_CHUNK },
    compression: COMPRESSION, files,
  };
}

// Нарезка сборки в хранилище: новые куски сжимаются и пишутся в
// store/<aa>/<sha256>, уже лежащие там не трогаются. Возвращает манифест и
// счётчики для отчёта.
function buildStore(root, store, options = {}) {
  let written = 0, writtenBytes = 0, reused = 0;
  const sizes = new Map();
  const manifest = describeTree(root, Object.assign({}, options, {
    onChunk(hash, data) {
      const target = path.join(store, hash.slice(0, 2), hash);
      if (fs.existsSync(target)) {
        reused++;
        sizes.set(hash, fs.statSync(target).size);
        return;
      }
      const packed = compress(data);
      if (packed.length > MAX_COMPRESSED_CHUNK) throw new Error(`Кусок ${hash} после сжатия слишком велик`);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      const temp = `${target}.partial`;
      fs.writeFileSync(temp, packed);
      fs.renameSync(temp, target);
      sizes.set(hash, packed.length);
      written++;
      writtenBytes += packed.length;
    },
  }));
  for (const file of manifest.files)
    file.chunks = file.chunks.map(([hash, size]) => [hash, size, sizes.get(hash)]);
  return { manifest, written, writtenBytes, reused, chunks: sizes.size };
}

function safePath(value) {
  return typeof value === "string" && value.length > 0 && value.length < 512 &&
    !value.startsWith("/") && !/(^|\/)\.\.?(\/|$)/.test(value) && !/[\\:\0]/.test(value) && !/\/$/.test(value);
}

function problemsOf(manifest, expected = {}) {
  const problems = [];
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) return ["манифест чанков не объект"];
  if (manifest.schema !== SCHEMA || manifest.kind !== KIND) problems.push("неизвестная схема манифеста чанков");
  if (!VERSION_RE.test(manifest.version || "")) problems.push(`некорректная версия: ${manifest.version}`);
  if (expected.version && manifest.version !== expected.version) problems.push(`версия ${manifest.version}, ожидалась ${expected.version}`);
  if (!PLATFORMS.includes(manifest.platform)) problems.push(`неизвестная платформа: ${manifest.platform}`);
  if (expected.platform && manifest.platform !== expected.platform) problems.push(`платформа ${manifest.platform}, ожидалась ${expected.platform}`);
  const c = manifest.chunking || {};
  if (c.algorithm !== ALGORITHM || c.min !== MIN_CHUNK || c.avg !== AVG_CHUNK || c.max !== MAX_CHUNK)
    problems.push("неподдерживаемые параметры нарезки");
  if (manifest.compression !== COMPRESSION) problems.push(`неподдерживаемое сжатие: ${manifest.compression}`);
  if (!Array.isArray(manifest.files) || !manifest.files.length || manifest.files.length > MAX_FILES) {
    problems.push("некорректный список файлов");
    return problems;
  }
  const seen = new Set();
  for (const file of manifest.files) {
    const name = file && file.path;
    if (!safePath(name)) { problems.push(`недопустимый путь: ${name}`); continue; }
    const key = name.toLowerCase();
    if (seen.has(key)) problems.push(`повтор: ${name}`);
    seen.add(key);
    if (!Number.isSafeInteger(file.size) || file.size < 0 || file.size > MAX_FILE_BYTES) problems.push(`размер ${name}`);
    if (!SHA256_RE.test(file.sha256 || "")) problems.push(`SHA-256 ${name}`);
    if (file.exec !== undefined && file.exec !== true) problems.push(`exec ${name}`);
    if (!Array.isArray(file.chunks)) { problems.push(`куски ${name}`); continue; }
    let total = 0;
    for (const chunk of file.chunks) {
      if (!Array.isArray(chunk) || chunk.length !== 3 || !SHA256_RE.test(chunk[0] || "") ||
        !Number.isInteger(chunk[1]) || chunk[1] <= 0 || chunk[1] > MAX_CHUNK ||
        !Number.isInteger(chunk[2]) || chunk[2] <= 0 || chunk[2] > MAX_COMPRESSED_CHUNK) {
        problems.push(`некорректный кусок в ${name}`);
        break;
      }
      total += chunk[1];
    }
    if (total !== file.size) problems.push(`куски ${name} не складываются в размер файла`);
    if (problems.length > 20) break;
  }
  return problems;
}

// Блок delta подписанного канала: адрес и хеш манифеста чанков и базы
// хранилища. Старые клиенты блок не знают и не смотрят на него.
function deltaProblems(delta, isAllowedUrl) {
  const problems = [];
  if (!delta || typeof delta !== "object" || Array.isArray(delta)) return ["нет блока delta"];
  if (delta.schema !== SCHEMA) problems.push(`delta.schema ${delta.schema}`);
  const manifest = delta.manifest || {};
  if (!/^HLSR-\d+\.\d+\.\d+-(linux|win32)\.chunks\.json$/.test(manifest.filename || "")) problems.push("delta.manifest.filename");
  if (!Number.isInteger(manifest.size) || manifest.size <= 0 || manifest.size > MAX_MANIFEST_BYTES) problems.push("delta.manifest.size");
  if (!SHA256_RE.test(manifest.sha256 || "")) problems.push("delta.manifest.sha256");
  for (const [label, list] of [["delta.manifest.urls", manifest.urls], ["delta.chunkBaseUrls", delta.chunkBaseUrls]]) {
    if (!Array.isArray(list) || !list.length) problems.push(`${label} пуст`);
    else for (const url of list) if (!isAllowedUrl(url)) problems.push(`${label} вне allowlist: ${url}`);
  }
  return problems;
}

function abortError() {
  const error = new Error("Обновление отменено");
  error.name = "AbortError";
  error.code = "ABORTED";
  return error;
}

function throwIfAborted(signal) {
  if (signal && signal.aborted) throw abortError();
}

// GET с редиректами по allowlist и пределом размера ответа.
function fetchBuffer(url, options = {}) {
  const { isAllowedUrl, maxBytes, signal } = options;
  const redirects = options.redirects == null ? MAX_REDIRECTS : options.redirects;
  return new Promise((resolve, reject) => {
    if (!isAllowedUrl(url)) return reject(new Error(`Адрес вне allowlist: ${url}`));
    if (signal && signal.aborted) return reject(abortError());
    const request = require("https").get(url, {
      headers: { "user-agent": "HLSR-Updater", "accept-encoding": "identity" },
      timeout: REQUEST_TIMEOUT_MS, signal,
    }, (response) => {
      const status = response.statusCode;
      if (status >= 300 && status < 400 && response.headers.location) {
        response.resume();
        if (!redirects) return reject(new Error("Слишком много перенаправлений"));
        const next = new URL(response.headers.location, url).toString();
        return fetchBuffer(next, Object.assign({}, options, { redirects: redirects - 1 })).then(resolve, reject);
      }
      if (status !== 200) {
        response.resume();
        return reject(new Error(`HTTP ${status}: ${url}`));
      }
      const parts = [];
      let size = 0;
      response.on("data", (part) => {
        size += part.length;
        if (size > maxBytes) {
          request.destroy(new Error(`Ответ больше ${maxBytes} байт: ${url}`));
          return;
        }
        parts.push(part);
      });
      response.on("end", () => resolve(Buffer.concat(parts)));
      response.on("error", reject);
    });
    request.on("timeout", () => request.destroy(new Error(`Таймаут: ${url}`)));
    request.on("error", (error) => reject(signal && signal.aborted ? abortError() : error));
  });
}

// Манифест чанков версии: скачать по адресам из канала, сверить с хешем из
// подписанного канала и проверить устройство.
async function fetchManifest(delta, options) {
  const fetch = options.fetch || fetchBuffer;
  let lastError = null;
  for (const url of delta.manifest.urls) {
    throwIfAborted(options.signal);
    try {
      const body = await fetch(url, { isAllowedUrl: options.isAllowedUrl, maxBytes: delta.manifest.size, signal: options.signal });
      if (body.length !== delta.manifest.size || sha256(body) !== delta.manifest.sha256)
        throw new Error("Манифест чанков не совпал с подписанным каналом");
      const manifest = JSON.parse(body.toString("utf8"));
      const problems = problemsOf(manifest, { version: options.version, platform: options.platform });
      if (problems.length) throw new Error(`Манифест чанков: ${problems.slice(0, 5).join("; ")}`);
      return manifest;
    } catch (error) {
      if (error.name === "AbortError") throw error;
      lastError = error;
    }
  }
  throw lastError || new Error("Манифест чанков недоступен");
}

// Что из старой версии годится для новой: целые совпавшие файлы и куски
// изменённых файлов с тем же путём.
function planReuse(manifest, sourceRoot, options = {}) {
  const whole = new Map();
  const local = new Map();
  if (!sourceRoot) return { whole, local };
  for (const file of manifest.files) {
    throwIfAborted(options.signal);
    const from = path.join(sourceRoot, ...file.path.split("/"));
    let stat;
    try { stat = fs.statSync(from); } catch (_) { continue; }
    if (!stat.isFile()) continue;
    if (stat.size === file.size && sha256File(from) === file.sha256) {
      whole.set(file.path, from);
      continue;
    }
    if (!file.chunks.length || stat.size > MAX_FILE_BYTES) continue;
    const data = fs.readFileSync(from);
    for (const [start, end] of cut(data)) {
      const hash = sha256(data.subarray(start, end));
      if (!local.has(hash)) local.set(hash, { file: from, offset: start, size: end - start });
    }
  }
  return { whole, local };
}

function readRange(file, offset, size) {
  const buffer = Buffer.allocUnsafe(size);
  const fd = fs.openSync(file, "r");
  try {
    let done = 0;
    while (done < size) {
      const read = fs.readSync(fd, buffer, done, size - done, offset + done);
      if (!read) throw new Error(`Файл ${file} короче ожидаемого`);
      done += read;
    }
  } finally {
    fs.closeSync(fd);
  }
  return buffer;
}

function progressMeter(total, onProgress) {
  const started = Date.now();
  let done = 0;
  let last = 0;
  return {
    add(bytes) {
      done += bytes;
      const now = Date.now();
      if (now - last < 200 && done < total) return;
      last = now;
      const seconds = Math.max((now - started) / 1000, 0.001);
      onProgress({ done, total, bytesPerSecond: Math.round(done / seconds) });
    },
  };
}

async function downloadChunks(needed, options) {
  const { bases, cacheDir, signal, isAllowedUrl } = options;
  const fetch = options.fetch || fetchBuffer;
  const queue = Array.from(needed.values());
  const total = queue.reduce((sum, chunk) => sum + chunk.csize, 0);
  const meter = progressMeter(total, options.onProgress || (() => {}));
  meter.add(0);

  async function one(chunk) {
    const target = path.join(cacheDir, chunk.hash);
    if (fs.existsSync(target) && fs.statSync(target).size === chunk.size && sha256(fs.readFileSync(target)) === chunk.hash) {
      meter.add(chunk.csize);
      return;
    }
    let lastError = null;
    for (let attempt = 0; attempt < DOWNLOAD_ATTEMPTS * bases.length; attempt++) {
      throwIfAborted(signal);
      const base = bases[attempt % bases.length];
      try {
        const packed = await fetch(chunkUrl(base, chunk.hash), { isAllowedUrl, maxBytes: chunk.csize, signal });
        if (packed.length !== chunk.csize) throw new Error(`Кусок ${chunk.hash}: размер ${packed.length} != ${chunk.csize}`);
        const data = decompress(packed, chunk.size);
        if (data.length !== chunk.size || sha256(data) !== chunk.hash) throw new Error(`Кусок ${chunk.hash} повреждён`);
        fs.writeFileSync(`${target}.partial`, data);
        fs.renameSync(`${target}.partial`, target);
        meter.add(chunk.csize);
        return;
      } catch (error) {
        if (error.name === "AbortError" || (signal && signal.aborted)) throw abortError();
        lastError = error;
      }
    }
    throw lastError;
  }

  let failure = null;
  async function worker() {
    while (queue.length && !failure) {
      const chunk = queue.shift();
      try { await one(chunk); } catch (error) { failure = failure || error; }
    }
  }
  await Promise.all(Array.from({ length: Math.min(DOWNLOAD_CONCURRENCY, queue.length || 1) }, worker));
  if (failure) throw failure;
  return total;
}

function copyWhole(from, to) {
  try { fs.copyFileSync(from, to, fs.constants.COPYFILE_EXCL | fs.constants.COPYFILE_FICLONE); }
  catch (error) {
    if (error.code === "EEXIST") throw error;
    fs.copyFileSync(from, to, fs.constants.COPYFILE_EXCL);
  }
}

// Собирает версию из манифеста в пустой каталог targetDir. sourceRoot —
// установленная версия, из которой берётся всё совпавшее. Возвращает,
// сколько байт скачано и сколько взято с диска.
async function assemble(options) {
  const { manifest, sourceRoot, targetDir, cacheDir, signal } = options;
  const bases = options.chunkBaseUrls;
  const onProgress = options.onProgress || (() => {});
  const log = options.log || { info() {}, warn() {} };

  const reuse = planReuse(manifest, sourceRoot, { signal });
  const needed = new Map();
  for (const file of manifest.files) {
    if (reuse.whole.has(file.path)) continue;
    for (const [hash, size, csize] of file.chunks)
      if (!reuse.local.has(hash) && !needed.has(hash)) needed.set(hash, { hash, size, csize });
  }
  log.info(`[chunks] ${manifest.version}: файлов целиком ${reuse.whole.size}, кусков с диска ${reuse.local.size}, скачать ${needed.size}`);

  fs.mkdirSync(cacheDir, { recursive: true });
  const downloaded = await downloadChunks(needed, {
    bases, cacheDir, signal, isAllowedUrl: options.isAllowedUrl, fetch: options.fetch, onProgress,
  });

  let reusedBytes = 0;
  for (const file of manifest.files) {
    throwIfAborted(signal);
    const target = path.join(targetDir, ...file.path.split("/"));
    fs.mkdirSync(path.dirname(target), { recursive: true });
    const whole = reuse.whole.get(file.path);
    if (whole) {
      copyWhole(whole, target);
      reusedBytes += file.size;
    } else {
      const hash = crypto.createHash("sha256");
      const fd = fs.openSync(target, "wx", 0o644);
      try {
        for (const [chunkHash, size] of file.chunks) {
          const source = reuse.local.get(chunkHash);
          const data = source
            ? readRange(source.file, source.offset, source.size)
            : fs.readFileSync(path.join(cacheDir, chunkHash));
          if (data.length !== size || sha256(data) !== chunkHash) throw new Error(`Кусок ${chunkHash} изменился на диске`);
          if (source) reusedBytes += size;
          hash.update(data);
          fs.writeSync(fd, data);
        }
      } finally {
        fs.closeSync(fd);
      }
      if (hash.digest("hex") !== file.sha256) throw new Error(`Собранный файл не совпал: ${file.path}`);
    }
    if (file.exec) fs.chmodSync(target, 0o755);
  }
  return { downloadedBytes: downloaded, reusedBytes, downloadedChunks: needed.size };
}

module.exports = {
  SCHEMA, KIND, ALGORITHM, MIN_CHUNK, AVG_CHUNK, MAX_CHUNK, COMPRESSION, MAX_MANIFEST_BYTES,
  cut, sha256, sha256File, chunkPath, chunkUrl, compress, decompress,
  describeTree, buildStore, problemsOf, deltaProblems,
  fetchBuffer, fetchManifest, planReuse, assemble,
};
