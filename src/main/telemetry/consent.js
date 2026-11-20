"use strict";

const fs = require("fs");
const path = require("path");

const config = require("./config");

const SCHEMA_VERSION = 1;

const CONSENT_FILE = "telemetry.json";

const BACKEND_CONFIGURED = config.BACKEND_CONFIGURED;

const DEFAULT_ENABLED = false;

const SOURCES = { DEFAULT: "default", USER: "user" };

function consentPath(userDataDir) {
  return path.join(userDataDir, CONSENT_FILE);
}

function decode(buffer) {
  if (buffer.length >= 2 && buffer[0] === 0xff && buffer[1] === 0xfe)
    return buffer.slice(2).toString("utf16le");

  if (buffer.length >= 2 && buffer[1] === 0x00) return buffer.toString("utf16le");

  return buffer.toString("utf8").replace(/^\uFEFF/, "");
}

function readJson(file) {
  try {
    return JSON.parse(decode(fs.readFileSync(file)));
  } catch (e) {
    return null;
  }
}

function writeJson(file, data) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(data, null, 2), "utf8");
}

function read(userDataDir) {
  const data = readJson(consentPath(userDataDir));

  if (!data || typeof data !== "object" || typeof data.enabled !== "boolean")
    return { enabled: DEFAULT_ENABLED, source: SOURCES.DEFAULT };

  return { enabled: data.enabled, source: SOURCES.USER };
}

function write(userDataDir, enabled, source) {
  const state = {
    schemaVersion: SCHEMA_VERSION,
    enabled: enabled === true,
    source: source || SOURCES.USER,
    updatedAt: new Date().toISOString(),
  };

  writeJson(consentPath(userDataDir), state);
  return { enabled: state.enabled, source: state.source };
}

function set(userDataDir, enabled) {
  return write(userDataDir, enabled === true, SOURCES.USER);
}

function canSend(userDataDir) {
  return BACKEND_CONFIGURED && read(userDataDir).enabled === true;
}

module.exports = {
  SCHEMA_VERSION,
  CONSENT_FILE,
  BACKEND_CONFIGURED,
  DEFAULT_ENABLED,
  SOURCES,
  decode,
  read,
  set,
  canSend,
};
