"use strict";

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");

const FILE_NAME = "telemetry-identity.json";
const SCHEMA_VERSION = 1;

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function identityPath(userDataDir) {
  return path.join(userDataDir, FILE_NAME);
}

function randomUuid() {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();

  const bytes = crypto.randomBytes(16);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString("hex");

  return (
    `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-` +
    `${hex.slice(16, 20)}-${hex.slice(20)}`
  );
}

function read(userDataDir) {
  const data = readJsonSafe(identityPath(userDataDir));

  if (
    !data ||
    typeof data !== "object" ||
    data.schemaVersion !== SCHEMA_VERSION ||
    typeof data.installationId !== "string" ||
    !UUID_RE.test(data.installationId)
  )
    return null;

  return { installationId: data.installationId, createdAt: data.createdAt || null };
}

function ensure(userDataDir) {
  const existing = read(userDataDir);
  if (existing) return existing;

  const identity = {
    schemaVersion: SCHEMA_VERSION,
    installationId: randomUuid(),
    createdAt: new Date().toISOString(),
  };

  writeJsonAtomic(identityPath(userDataDir), identity);

  return { installationId: identity.installationId, createdAt: identity.createdAt };
}

function reset(userDataDir) {
  const file = identityPath(userDataDir);

  try {
    if (!fs.existsSync(file)) return false;
    fs.rmSync(file, { force: true });
    return true;
  } catch (error) {
    return false;
  }
}

module.exports = {
  FILE_NAME,
  SCHEMA_VERSION,
  randomUuid,
  read,
  ensure,
  reset,
  identityPath,
};
