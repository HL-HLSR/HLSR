"use strict";

const path = require("path");

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");

const MARKER_NAME = ".hlsr-install-owner.json";
const STATE_FILE = "telemetry-install-state.json";
const SCHEMA_VERSION = 1;

const INSTALL_ID_RE = /^\{[0-9A-Fa-f-]{36}\}$/;

function statePath(userDataDir) {
  return path.join(userDataDir, STATE_FILE);
}

function readMarker(installDir) {
  if (typeof installDir !== "string" || !installDir) return null;

  const data = readJsonSafe(path.join(installDir, MARKER_NAME));

  if (
    !data ||
    typeof data !== "object" ||
    typeof data.installId !== "string" ||
    !INSTALL_ID_RE.test(data.installId)
  )
    return null;

  return data.installId.toUpperCase();
}

function readState(userDataDir) {
  const data = readJsonSafe(statePath(userDataDir));

  if (!data || typeof data !== "object" || data.schemaVersion !== SCHEMA_VERSION) return null;

  return {
    installId: typeof data.installId === "string" ? data.installId : null,
    reportedAt: data.reportedAt || null,
  };
}

function writeState(userDataDir, installId) {
  writeJsonAtomic(statePath(userDataDir), {
    schemaVersion: SCHEMA_VERSION,
    installId,
    reportedAt: new Date().toISOString(),
  });
}

/**
 * Нужно ли отправить launcher_install_completed.
 *
 * Возвращает { report: boolean, installId: string|null, reinstall: boolean }.
 *
 * Дедупликация обязательна: без неё событие уходило бы при КАЖДОМ
 * запуске лаунчера, и «установок» стало бы столько же, сколько сессий.
 */
function detectInstall(userDataDir, installDir) {
  const installId = readMarker(installDir);
  if (!installId) return { report: false, installId: null, reinstall: false };

  const state = readState(userDataDir);

  if (state && state.installId === installId)
    return { report: false, installId, reinstall: false };

  return { report: true, installId, reinstall: state !== null && state.installId !== null };
}

function confirmInstall(userDataDir, installId) {
  try {
    writeState(userDataDir, installId);
    return true;
  } catch (error) {
    return false;
  }
}

module.exports = {
  MARKER_NAME,
  STATE_FILE,
  SCHEMA_VERSION,
  readMarker,
  readState,
  detectInstall,
  confirmInstall,
  statePath,
};
