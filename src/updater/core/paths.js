"use strict";

const path = require("path");

const { CODES, UpdaterError } = require("./errors");

const PROTECTED_SEGMENTS = [
  ["games"],
  ["downloads"],
  ["game-stats.json"],
  ["game-settings.json"],
  ["settings.json"],
  ["library.json"],
];

function normalize(value) {
  return path.resolve(String(value || "")).toLowerCase().replace(/[\\/]+$/, "");
}

function isInside(parent, child) {
  const p = normalize(parent);
  const c = normalize(child);
  if (c === p) return true;
  return c.startsWith(p + path.sep.toLowerCase()) || c.startsWith(p + "/");
}

function assertWritable(target, options) {
  const opts = options || {};
  const allowedRoots = (opts.allowedRoots || []).filter(Boolean);
  const allowedFiles = (opts.allowedFiles || []).filter(Boolean);

  const normalized = normalize(target);

  for (const file of allowedFiles) if (normalize(file) === normalized) return true;

  const base = path.basename(normalized);
  for (const segments of PROTECTED_SEGMENTS) {
    if (segments.length === 1 && segments[0] === base)
      throw new UpdaterError(
        CODES.BAD_PLAN,
        `запись в пользовательский файл запрещена: ${base}`
      );
  }

  const parts = normalized.split(/[\\/]/);
  for (const guard of ["games", "downloads"])
    if (parts.indexOf(guard) !== -1 && !allowedRoots.some((root) => isInside(root, target)))
      throw new UpdaterError(CODES.BAD_PLAN, `запись в каталог ${guard} запрещена`);

  if (!allowedRoots.length) return true;

  for (const root of allowedRoots) if (isInside(root, target)) return true;

  throw new UpdaterError(CODES.BAD_PLAN, "запись вне разрешённых каталогов");
}

function assertSafeWorkDir(workDir, options) {
  const opts = options || {};
  const tempRoot = opts.tempRoot || require("os").tmpdir();
  const installDir = opts.installDir;

  if (installDir && isInside(installDir, workDir))
    throw new UpdaterError(
      CODES.STAGING_FAILED,
      "рабочий каталог Updater не может лежать внутри каталога установки"
    );

  if (!isInside(tempRoot, workDir))
    throw new UpdaterError(CODES.STAGING_FAILED, "рабочий каталог Updater вне временной зоны");

  return true;
}

module.exports = { assertWritable, assertSafeWorkDir, isInside, normalize, PROTECTED_SEGMENTS };
