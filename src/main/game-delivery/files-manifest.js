"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { matchesMutable } = require("./mutable-paths");
const { classifyPath } = require("./archive-entries");
const { DeliveryError } = require("./errors");

const SCHEMA_VERSION = 1;
const HASH_CHUNK = 4 * 1024 * 1024;

const MAX_ENTRIES = 200000;

// Стоковые cfg мода лежат в папках игрока (mutable-paths.js): в files они не
// входят, поэтому установка, обновления, патчи и удаления их не трогают, а
// manifestSha256 (canonicalPayload) от них не зависит. «Проверить файлы» и ремонт
// сверяют их по отдельному списку stockConfigs того же файла. config.cfg пишет
// игра, autoexec.cfg и hlsr-*.cfg — лаунчер: это не стоковые.
const STOCK_CONFIG_DIRS = ["ghostingmod/cfg", "hl2/cfg", "lostcoast/cfg", "hl2_russian/cfg"];
const NOT_STOCK = /^(config|autoexec)\.cfg$|^hlsr-.*\.cfg$/i;
const MAX_STOCK_CONFIGS = 1000;

function isStockConfigPath(relative) {
  const text = String(relative || "");
  const slash = text.lastIndexOf("/");
  const dir = text.slice(0, slash).toLowerCase();
  const name = text.slice(slash + 1);
  return slash > 0 && /\.cfg$/i.test(name) && !NOT_STOCK.test(name) && STOCK_CONFIG_DIRS.includes(dir);
}

function toRelative(root, full) {
  return path.relative(root, full).replace(/\\/g, "/");
}

function hashFileSync(file) {
  const hash = crypto.createHash("sha256");
  const fd = fs.openSync(file, "r");
  const buffer = Buffer.allocUnsafe(HASH_CHUNK);

  try {
    for (;;) {
      const read = fs.readSync(fd, buffer, 0, buffer.length, null);
      if (read <= 0) break;
      hash.update(buffer.subarray(0, read));
    }
  } finally {
    fs.closeSync(fd);
  }

  return hash.digest("hex");
}

function collectFiles(root, options) {
  const opts = options || {};
  const isMutable = opts.isMutable || matchesMutable;
  const files = [];
  const skipped = { mutable: [], links: [], unsafe: [] };
  const stack = [root];

  while (stack.length) {
    const dir = stack.pop();
    let items;

    try {
      items = fs.readdirSync(dir, { withFileTypes: true });
    } catch (e) {
      throw new DeliveryError("PATH_NOT_WRITABLE", `Не удалось прочитать ${dir}: ${e.message}`);
    }

    for (const item of items) {
      const full = path.join(dir, item.name);
      const relative = toRelative(root, full);

      if (item.isSymbolicLink()) {
        skipped.links.push(relative);
        continue;
      }

      if (classifyPath(relative).length) {
        skipped.unsafe.push(relative);
        continue;
      }

      if (isMutable(relative)) {
        skipped.mutable.push(relative);
        continue;
      }

      if (item.isDirectory()) {
        stack.push(full);
        continue;
      }

      if (!item.isFile()) continue;

      files.push({ relative, full });
    }
  }

  files.sort((a, b) => (a.relative < b.relative ? -1 : a.relative > b.relative ? 1 : 0));

  return { files, skipped };
}

// Стоковые cfg каталога сборки: файлы (не ссылки) прямо в STOCK_CONFIG_DIRS.
function collectStockConfigs(root) {
  const entries = [];

  for (const dir of STOCK_CONFIG_DIRS) {
    let items;
    try {
      items = fs.readdirSync(path.join(root, ...dir.split("/")), { withFileTypes: true });
    } catch (e) {
      continue;
    }

    for (const item of items) {
      const relative = `${dir}/${item.name}`;
      if (!item.isFile() || !isStockConfigPath(relative)) continue;
      const full = path.join(root, ...relative.split("/"));
      entries.push({ path: relative, size: fs.statSync(full).size, sha256: hashFileSync(full) });
    }
  }

  return entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

function canonicalPayload(manifest) {
  return JSON.stringify({
    schemaVersion: manifest.schemaVersion,
    gameId: manifest.gameId,
    version: manifest.version,
    totalFiles: manifest.totalFiles,
    totalSize: manifest.totalSize,
    files: manifest.files.map((file) => [file.path, file.size, file.sha256]),
  });
}

function hashManifest(manifest) {
  return crypto.createHash("sha256").update(canonicalPayload(manifest), "utf8").digest("hex");
}

function build(options) {
  const opts = options || {};
  const root = path.resolve(opts.root);

  if (!fs.existsSync(root))
    throw new DeliveryError("PATH_INVALID", `Каталог сборки не найден: ${root}`);

  const { files, skipped } = collectFiles(root, opts);
  const entries = [];
  let totalSize = 0;

  for (let index = 0; index < files.length; index++) {
    const file = files[index];
    const size = fs.statSync(file.full).size;
    const sha256 = hashFileSync(file.full);

    entries.push({ path: file.relative, size, sha256 });
    totalSize += size;

    if (opts.onProgress && (index % 50 === 0 || index === files.length - 1))
      opts.onProgress({ processed: index + 1, total: files.length, path: file.relative });
  }

  const manifest = {
    schemaVersion: SCHEMA_VERSION,
    gameId: opts.gameId,
    version: opts.version,
    generatedAt: (opts.now || new Date()).toISOString(),
    totalFiles: entries.length,
    totalSize,
    files: entries,
  };

  const listed = new Set(entries.map((entry) => entry.path.toLowerCase()));
  const stockConfigs = collectStockConfigs(root).filter((entry) => !listed.has(entry.path.toLowerCase()));
  if (stockConfigs.length) manifest.stockConfigs = stockConfigs;

  manifest.manifestSha256 = hashManifest(manifest);

  return { manifest, skipped };
}

function load(file) {
  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (e) {
    throw new DeliveryError("MANIFEST_NOT_FOUND", `Файловый манифест не найден: ${file}`);
  }

  let data;
  try {
    data = JSON.parse(raw);
  } catch (e) {
    throw new DeliveryError("MANIFEST_INVALID", `Файловый манифест не является JSON: ${e.message}`);
  }

  return validate(data, file);
}

function validate(data, source, options) {
  const opts = options || {};

  if (!data || typeof data !== "object" || Array.isArray(data))
    throw new DeliveryError("MANIFEST_INVALID", "Файловый манифест должен быть JSON-объектом");

  if (data.schemaVersion !== SCHEMA_VERSION)
    throw new DeliveryError("MANIFEST_INVALID", "Неподдерживаемая версия файлового манифеста");

  if (!Array.isArray(data.files))
    throw new DeliveryError("MANIFEST_INVALID", "В файловом манифесте нет списка файлов");

  const maxEntries = opts.maxEntries || MAX_ENTRIES;

  if (data.files.length > maxEntries)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `Слишком много записей в файловом манифесте: ${data.files.length} при пределе ${maxEntries}`
    );

  if (opts.gameId && data.gameId !== opts.gameId)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `Файловый манифест описывает игру ${data.gameId}, ожидалась ${opts.gameId}`
    );

  if (opts.version && data.version !== opts.version)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `Файловый манифест описывает версию ${data.version}, ожидалась ${opts.version}`
    );

  const seen = new Set();
  let totalSize = 0;

  for (const file of data.files) {
    if (!file || typeof file.path !== "string" || !/^[a-f0-9]{64}$/.test(file.sha256 || ""))
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Некорректная запись манифеста: ${file && file.path}`
      );

    if (!Number.isInteger(file.size) || file.size < 0)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Некорректный размер файла ${file.path}: ${file.size}`
      );

    const problems = classifyPath(file.path);
    if (problems.length)
      throw new DeliveryError(
        "ARCHIVE_UNSAFE_ENTRY",
        `Опасный путь в файловом манифесте: ${file.path} (${problems.join(", ")})`,
        { source: source || null }
      );

    const key = file.path.toLowerCase();

    if (seen.has(key))
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Путь встречается в файловом манифесте дважды: ${file.path}`
      );

    seen.add(key);
    totalSize += file.size;
  }

  if (data.stockConfigs !== undefined) {
    if (!Array.isArray(data.stockConfigs) || data.stockConfigs.length > MAX_STOCK_CONFIGS)
      throw new DeliveryError("MANIFEST_INVALID", "Некорректный список стоковых cfg в файловом манифесте");

    for (const file of data.stockConfigs) {
      const valid =
        file &&
        isStockConfigPath(file.path) &&
        !classifyPath(file.path).length &&
        /^[a-f0-9]{64}$/.test(file.sha256 || "") &&
        Number.isInteger(file.size) &&
        file.size >= 0 &&
        !seen.has(file.path.toLowerCase());

      if (!valid)
        throw new DeliveryError("MANIFEST_INVALID", `Некорректный стоковый cfg в файловом манифесте: ${file && file.path}`);

      seen.add(file.path.toLowerCase());
    }
  }

  if (data.totalFiles != null && data.totalFiles !== data.files.length)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `totalFiles не сходится: заявлено ${data.totalFiles}, записей ${data.files.length}`
    );

  if (data.totalSize != null && data.totalSize !== totalSize)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `totalSize не сходится: заявлено ${data.totalSize}, посчитано ${totalSize}`
    );

  const expected = hashManifest(data);

  if (data.manifestSha256 && data.manifestSha256 !== expected)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `manifestSha256 не сходится: в файле ${data.manifestSha256}, посчитано ${expected}`
    );

  if (opts.requireManifestSha256 && !data.manifestSha256)
    throw new DeliveryError("MANIFEST_INVALID", "В файловом манифесте нет manifestSha256");

  return data;
}

function hashFile(file) {
  return hashFileSync(file);
}

function serialize(manifest) {
  return JSON.stringify(manifest, null, 2) + "\n";
}

module.exports = {
  SCHEMA_VERSION,
  MAX_ENTRIES,
  build,
  collectStockConfigs,
  isStockConfigPath,
  load,
  validate,
  serialize,
  hashManifest,
  hashFileSync,
  hashFile,
  collectFiles,
  canonicalPayload,
  toRelative,
};
