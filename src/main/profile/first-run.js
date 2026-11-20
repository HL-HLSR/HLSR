"use strict";

const fs = require("fs");
const path = require("path");

const FILE_NAME = "profile.json";
const SCHEMA_VERSION = 1;

const IGNORED = new Set([
  "DevToolsActivePort",
  "lockfile",
  "QuotaManager",
  "QuotaManager-journal",
  "Local State",
  "Preferences",
  FILE_NAME,
]);

function isValidDate(value) {
  if (typeof value !== "string" || !value) return false;
  const time = Date.parse(value);
  return Number.isFinite(time) && time > 0;
}

function oldestEntryAt(dir, deps) {
  const io = (deps && deps.fs) || fs;

  let names;
  try {
    names = io.readdirSync(dir);
  } catch (error) {
    return null;
  }

  let oldest = null;

  for (const name of names) {
    if (IGNORED.has(name)) continue;

    let stats;
    try {
      stats = io.statSync(path.join(dir, name));
    } catch (error) {
      continue;
    }

    const time = Number(stats.birthtimeMs) || Number(stats.mtimeMs);
    if (!Number.isFinite(time) || time <= 0) continue;

    if (oldest === null || time < oldest) oldest = time;
  }

  return oldest === null ? null : new Date(oldest).toISOString();
}

function read(dir, deps) {
  const io = (deps && deps.fs) || fs;

  try {
    const data = JSON.parse(io.readFileSync(path.join(dir, FILE_NAME), "utf8"));
    return isValidDate(data && data.firstRunAt) ? data.firstRunAt : null;
  } catch (error) {
    return null;
  }
}

function ensureFirstRunAt(dir, options) {
  const opts = options || {};
  const io = opts.fs || fs;
  const logger = opts.logger || { info() {}, warn() {} };

  if (!dir) return null;

  const known = read(dir, opts);
  if (known) return known;

  const guessed = (opts.oldestEntryAt || oldestEntryAt)(dir, opts);
  const firstRunAt = guessed || new Date(opts.now || Date.now()).toISOString();

  try {
    io.writeFileSync(
      path.join(dir, FILE_NAME),
      JSON.stringify({ schemaVersion: SCHEMA_VERSION, firstRunAt }, null, 2)
    );

    logger.info(
      `[profile] дата первого запуска сохранена: ${firstRunAt}` +
        (guessed ? " (восстановлена по каталогу настроек)" : "")
    );
  } catch (error) {
    logger.warn(`[profile] дату первого запуска не удалось сохранить: ${error.message}`);
  }

  return firstRunAt;
}

module.exports = { ensureFirstRunAt, oldestEntryAt, read, FILE_NAME, SCHEMA_VERSION };
