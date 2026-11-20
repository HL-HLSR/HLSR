"use strict";

const fs = require("fs");
const path = require("path");

const { writeJsonAtomic, readJsonSafe } = require("../shared/game-delivery/atomic");

const SCHEMA_VERSION = 1;
const VERSION_RE = /^[0-9A-Za-z.\-+]{1,32}$/;

function isVersion(value) {
  return typeof value === "string" && VERSION_RE.test(value);
}

function writePendingReceipt(file, fromVersion, toVersion, options) {
  const opts = options || {};
  const now = opts.now || (() => Date.now());

  if (!isVersion(toVersion)) return false;

  let state = readJsonSafe(file);

  if (!state || typeof state !== "object" || state.schemaVersion !== SCHEMA_VERSION)
    state = { schemaVersion: SCHEMA_VERSION, seen: {}, receipt: null };

  if (!state.seen || typeof state.seen !== "object") state.seen = {};

  state.receipt = {
    from: isVersion(fromVersion) ? fromVersion : null,
    to: toVersion,
    ts: now(),
  };

  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    writeJsonAtomic(file, state);
    return true;
  } catch (error) {
    return false;
  }
}

module.exports = { writePendingReceipt, SCHEMA_VERSION };
