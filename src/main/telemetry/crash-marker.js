"use strict";

const fs = require("fs");
const path = require("path");

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");

const FILE_NAME = "telemetry-session-marker.json";
const SCHEMA_VERSION = 1;

const MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

function markerPath(userDataDir) {
  return path.join(userDataDir, FILE_NAME);
}

function open(userDataDir, sessionId, nowMs) {
  writeJsonAtomic(markerPath(userDataDir), {
    schemaVersion: SCHEMA_VERSION,
    sessionId,
    openedAt: new Date(nowMs || Date.now()).toISOString(),
    touchedAt: new Date(nowMs || Date.now()).toISOString(),
  });
}

function touch(userDataDir, nowMs) {
  const current = readJsonSafe(markerPath(userDataDir));
  if (!current || typeof current !== "object") return;

  current.touchedAt = new Date(nowMs || Date.now()).toISOString();

  try {
    writeJsonAtomic(markerPath(userDataDir), current);
  } catch (ignored) {
  }
}

function close(userDataDir) {
  try {
    const file = markerPath(userDataDir);
    if (fs.existsSync(file)) fs.rmSync(file, { force: true });
    return true;
  } catch (error) {
    return false;
  }
}

function detect(userDataDir, nowMs) {
  const data = readJsonSafe(markerPath(userDataDir));

  if (
    !data ||
    typeof data !== "object" ||
    data.schemaVersion !== SCHEMA_VERSION ||
    typeof data.sessionId !== "string"
  ) {
    close(userDataDir);
    return null;
  }

  const touchedAt = Date.parse(data.touchedAt || data.openedAt || "");
  const now = nowMs || Date.now();

  if (!Number.isFinite(touchedAt)) {
    close(userDataDir);
    return null;
  }

  if (now - touchedAt > MAX_AGE_MS) {
    close(userDataDir);
    return null;
  }

  return {
    sessionId: data.sessionId,
    lastAliveAt: new Date(touchedAt).toISOString(),
  };
}

function consume(userDataDir) {
  return close(userDataDir);
}

module.exports = {
  FILE_NAME,
  SCHEMA_VERSION,
  MAX_AGE_MS,
  markerPath,
  open,
  touch,
  close,
  detect,
  consume,
};
