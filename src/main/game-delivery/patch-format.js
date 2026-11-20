"use strict";

const crypto = require("crypto");
const { classifyPath } = require("./archive-entries");
const { createMatcher } = require("./path-match");
const { DeliveryError } = require("./errors");

const SCHEMA_VERSION = 1;
const PAYLOAD_ROOT = "payload";
const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;

const FORBIDDEN_FIELDS = [
  "command",
  "commands",
  "script",
  "scripts",
  "exec",
  "executable",
  "shell",
  "shellArgs",
  "args",
  "arguments",
  "env",
  "installPath",
  "absolutePath",
  "steamAppId",
  "preInstall",
  "postInstall",
];

function assertSafeRelativePath(value, what) {
  const problems = classifyPath(value);

  if (problems.length)
    throw new DeliveryError(
      "ARCHIVE_UNSAFE_ENTRY",
      `${what || "Путь"} недопустим: ${value} (${problems.join(", ")})`
    );

  if (/^[A-Za-z]:/.test(value) || value.startsWith("/") || value.startsWith("\\"))
    throw new DeliveryError("ARCHIVE_UNSAFE_ENTRY", `${what || "Путь"} должен быть относительным: ${value}`);

  return value.replace(/\\/g, "/");
}

function diff(oldManifest, newManifest, options) {
  const opts = options || {};
  const preserved = createMatcher(opts.preservePaths || []);
  const stillShipped = typeof opts.stillShipped === "function" ? opts.stillShipped : null;

  const oldFiles = new Map(oldManifest.files.map((file) => [file.path, file]));
  const newFiles = new Map(newManifest.files.map((file) => [file.path, file]));

  const added = [];
  const changed = [];
  const deletedFiles = [];
  const unchanged = [];

  for (const [relativePath, file] of newFiles) {
    assertSafeRelativePath(relativePath, "Файл новой версии");

    if (preserved(relativePath)) continue;

    const previous = oldFiles.get(relativePath);
    if (!previous) added.push(file);
    else if (previous.sha256 !== file.sha256) changed.push(file);
    else unchanged.push(file);
  }

  for (const [relativePath] of oldFiles) {
    if (newFiles.has(relativePath)) continue;
    if (preserved(relativePath)) continue;
    if (stillShipped && stillShipped(relativePath)) continue;
    deletedFiles.push(assertSafeRelativePath(relativePath, "Удаляемый файл"));
  }

  const payload = added.concat(changed).sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  deletedFiles.sort();

  return {
    added,
    changed,
    unchanged,
    deletedFiles,
    payload,
    payloadSize: payload.reduce((sum, file) => sum + file.size, 0),
  };
}

function canonicalPayload(manifest) {
  return JSON.stringify({
    schemaVersion: manifest.schemaVersion,
    gameId: manifest.gameId,
    fromVersion: manifest.fromVersion,
    toVersion: manifest.toVersion,
    payloadRoot: manifest.payloadRoot,
    files: manifest.files.map((file) => [file.path, file.size, file.sha256]),
    deletedFiles: manifest.deletedFiles,
    archive: [manifest.archive.filename, manifest.archive.size, manifest.archive.sha256],
    resultingFilesManifestSha256: manifest.resultingFilesManifestSha256,
    resultingTotalFiles: manifest.resultingTotalFiles,
    resultingTotalSize: manifest.resultingTotalSize,
  });
}

function hashPatchManifest(manifest) {
  return crypto.createHash("sha256").update(canonicalPayload(manifest), "utf8").digest("hex");
}

function validatePatchManifest(data, options) {
  const opts = options || {};

  if (!data || typeof data !== "object" || Array.isArray(data))
    throw new DeliveryError("MANIFEST_INVALID", "Манифест патча должен быть объектом");

  if (data.schemaVersion !== SCHEMA_VERSION)
    throw new DeliveryError("MANIFEST_INVALID", `Неподдерживаемая версия манифеста патча: ${data.schemaVersion}`);

  for (const field of FORBIDDEN_FIELDS)
    if (Object.prototype.hasOwnProperty.call(data, field))
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Манифест патча не может содержать поле "${field}": удалённые данные не задают выполнение`
      );

  for (const field of ["gameId", "fromVersion", "toVersion", "payloadRoot"])
    if (typeof data[field] !== "string" || !data[field])
      throw new DeliveryError("MANIFEST_INVALID", `В манифесте патча нет поля ${field}`);

  if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(data.gameId))
    throw new DeliveryError("MANIFEST_INVALID", `Некорректный gameId: ${data.gameId}`);

  if (!SEMVER.test(data.fromVersion) || !SEMVER.test(data.toVersion))
    throw new DeliveryError("MANIFEST_INVALID", "Версии патча должны быть SemVer");

  if (data.fromVersion === data.toVersion)
    throw new DeliveryError("MANIFEST_INVALID", "fromVersion и toVersion совпадают");

  if (data.payloadRoot !== PAYLOAD_ROOT)
    throw new DeliveryError("MANIFEST_INVALID", `payloadRoot должен быть "${PAYLOAD_ROOT}"`);

  if (!Array.isArray(data.files) || !Array.isArray(data.deletedFiles))
    throw new DeliveryError("MANIFEST_INVALID", "В манифесте патча нет files/deletedFiles");

  const seen = new Set();

  for (const file of data.files) {
    if (!file || typeof file.path !== "string")
      throw new DeliveryError("MANIFEST_INVALID", "Некорректная запись files");

    assertSafeRelativePath(file.path, "Файл патча");

    if (!Number.isInteger(file.size) || file.size < 0)
      throw new DeliveryError("MANIFEST_INVALID", `Некорректный размер ${file.path}`);

    if (!/^[a-f0-9]{64}$/.test(file.sha256 || ""))
      throw new DeliveryError("MANIFEST_INVALID", `Некорректный sha256 ${file.path}`);

    if (seen.has(file.path))
      throw new DeliveryError("MANIFEST_INVALID", `Файл встречается дважды: ${file.path}`);

    seen.add(file.path);
  }

  for (const relativePath of data.deletedFiles) {
    assertSafeRelativePath(relativePath, "Удаляемый файл");

    if (seen.has(relativePath))
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Файл одновременно обновляется и удаляется: ${relativePath}`
      );
  }

  if (!data.archive || typeof data.archive.filename !== "string")
    throw new DeliveryError("MANIFEST_INVALID", "В манифесте патча нет описания архива");

  assertSafeRelativePath(data.archive.filename, "Имя архива патча");

  if (data.archive.filename.indexOf("/") !== -1)
    throw new DeliveryError("MANIFEST_INVALID", "Имя архива патча не может содержать путь");

  if (!Number.isInteger(data.archive.size) || data.archive.size <= 0)
    throw new DeliveryError("MANIFEST_INVALID", "Некорректный размер архива патча");

  if (!/^[a-f0-9]{64}$/.test(data.archive.sha256 || ""))
    throw new DeliveryError("MANIFEST_INVALID", "Некорректный sha256 архива патча");

  if (!/^[a-f0-9]{64}$/.test(data.resultingFilesManifestSha256 || ""))
    throw new DeliveryError("MANIFEST_INVALID", "Нет resultingFilesManifestSha256");

  if (data.patchManifestSha256) {
    const expected = hashPatchManifest(data);
    if (data.patchManifestSha256 !== expected)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `patchManifestSha256 не сходится: ${data.patchManifestSha256} != ${expected}`
      );
  }

  if (opts.expectedFrom && data.fromVersion !== opts.expectedFrom)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `Патч рассчитан на версию ${data.fromVersion}, установлена ${opts.expectedFrom}`,
      { fromVersion: data.fromVersion, installed: opts.expectedFrom }
    );

  return data;
}

function validatePayloadEntries(manifest, entries) {
  const expected = new Set(manifest.files.map((file) => `${PAYLOAD_ROOT}/${file.path}`.toLowerCase()));
  const seen = new Set();

  for (const entry of entries) {
    const normalized = String(entry.path).replace(/\\/g, "/");

    if (entry.isSymlink)
      throw new DeliveryError("ARCHIVE_UNSAFE_ENTRY", `Патч содержит symlink: ${normalized}`);

    assertSafeRelativePath(normalized, "Элемент архива патча");

    if (entry.isDirectory) continue;

    const lower = normalized.toLowerCase();

    if (!lower.startsWith(PAYLOAD_ROOT + "/"))
      throw new DeliveryError(
        "ARCHIVE_UNSAFE_ENTRY",
        `Элемент патча вне каталога ${PAYLOAD_ROOT}: ${normalized}`
      );

    if (!expected.has(lower))
      throw new DeliveryError(
        "ARCHIVE_UNSAFE_ENTRY",
        `Файл есть в архиве, но не объявлен в манифесте патча: ${normalized}`
      );

    seen.add(lower);
  }

  const missing = Array.from(expected).filter((item) => !seen.has(item));
  if (missing.length)
    throw new DeliveryError(
      "MANIFEST_INVALID",
      `В архиве патча не хватает файлов: ${missing.slice(0, 5).join(", ")}`,
      { missing: missing.slice(0, 20) }
    );

  return true;
}

function defaultPatchName(gameId, fromVersion, toVersion) {
  return `${gameId}-${fromVersion}-to-${toVersion}`;
}

module.exports = {
  SCHEMA_VERSION,
  PAYLOAD_ROOT,
  FORBIDDEN_FIELDS,
  assertSafeRelativePath,
  diff,
  canonicalPayload,
  hashPatchManifest,
  validatePatchManifest,
  validatePayloadEntries,
  defaultPatchName,
};
