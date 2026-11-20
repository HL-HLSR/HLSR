"use strict";

const path = require("path");
const fs = require("fs");

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");

const SCHEMA_VERSION = 2;
const FILE = "discord-rpc.json";

const DEFAULT_ENABLED = true;

const SOURCES = {
  DEFAULT: "default",
  USER: "user",
  MIGRATED: "migrated",
};

const LEGACY_FILE = "settings.json";
const LEGACY_KEY = "rpc";

function filePath(userDataDir) {
  return path.join(userDataDir, FILE);
}

function read(userDataDir) {
  const data = readJsonSafe(filePath(userDataDir));

  if (!data || typeof data !== "object" || typeof data.enabled !== "boolean")
    return { enabled: DEFAULT_ENABLED, source: SOURCES.DEFAULT, migrated: false, schemaVersion: 0 };

  return {
    enabled: data.enabled === true,
    source: typeof data.source === "string" ? data.source : SOURCES.USER,
    migrated: data.migrated === true,
    legacyValue: typeof data.legacyValue === "boolean" ? data.legacyValue : null,
    schemaVersion: Number.isInteger(data.schemaVersion) ? data.schemaVersion : 1,
  };
}

function write(userDataDir, state) {
  const next = {
    schemaVersion: SCHEMA_VERSION,
    enabled: state.enabled === true,
    source: state.source || SOURCES.USER,
    migrated: state.migrated === true,
    updatedAt: new Date().toISOString(),
  };
  if (typeof state.legacyValue === "boolean") next.legacyValue = state.legacyValue;

  writeJsonAtomic(filePath(userDataDir), next);
  return read(userDataDir);
}

function set(userDataDir, enabled) {
  const current = read(userDataDir);
  return write(userDataDir, {
    enabled: enabled === true,
    source: SOURCES.USER,
    migrated: current.migrated,
    legacyValue: current.legacyValue,
  });
}

function reset(userDataDir) {
  return write(userDataDir, { enabled: DEFAULT_ENABLED, source: SOURCES.DEFAULT, migrated: true });
}

function adoptDefault(userDataDir, known) {
  const current = known || read(userDataDir);
  if (current.schemaVersion >= SCHEMA_VERSION) return current;

  const chosen = current.source === SOURCES.USER || current.legacyValue === false;

  return write(userDataDir, {
    enabled: chosen ? current.enabled : DEFAULT_ENABLED,
    source: current.source,
    migrated: current.migrated,
    legacyValue: typeof current.legacyValue === "boolean" ? current.legacyValue : undefined,
  });
}

function migrate(userDataDir) {
  const current = read(userDataDir);
  if (current.migrated) return adoptDefault(userDataDir, current);

  let legacyValue = null;
  try {
    const raw = fs.readFileSync(path.join(userDataDir, LEGACY_FILE), "utf8");
    const parsed = JSON.parse(raw);
    const config = parsed && typeof parsed === "object" ? parsed.config : null;
    if (config && typeof config[LEGACY_KEY] === "boolean") legacyValue = config[LEGACY_KEY];
  } catch (e) {
    legacyValue = null;
  }

  return write(userDataDir, {
    enabled: legacyValue === false ? false : DEFAULT_ENABLED,
    source: legacyValue === null ? SOURCES.DEFAULT : SOURCES.MIGRATED,
    migrated: true,
    legacyValue: typeof legacyValue === "boolean" ? legacyValue : undefined,
  });
}

module.exports = {
  SCHEMA_VERSION,
  FILE,
  LEGACY_FILE,
  LEGACY_KEY,
  DEFAULT_ENABLED,
  SOURCES,
  filePath,
  read,
  set,
  reset,
  adoptDefault,
  migrate,
};
