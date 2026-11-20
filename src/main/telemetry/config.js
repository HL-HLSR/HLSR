"use strict";

// Адрес онлайна подставляет webpack в сборку выпуска (HLSR_API_HOST, только вместе с ключом
// аттестации: .electron-vue/webpack.main.config.js). В исходниках его нет: самосборка к серверу
// HLSR не обращается, все адреса онлайна ниже у неё пустые.
const API_HOST = process.env.HLSR_API_HOST || "";
const BACKEND_CONFIGURED = API_HOST !== "";
const API_BASE = API_HOST ? `https://${API_HOST}` : "";

const ENDPOINTS = {
  batch: `${API_BASE}/v1/telemetry/batch`,
  online: `${API_BASE}/v1/public/online`,
};

const SCHEMA_VERSION = 1;

const QUEUE = {
  fileName: "telemetry-queue.jsonl",
  maxEvents: 1000,
  maxBytes: 1024 * 1024,
  batchSize: 50,
  maxBatchBytes: 64 * 1024,
};

const TRANSPORT = {
  timeoutMs: 15000,
  backoffBaseMs: 30 * 1000,
  backoffMaxMs: 30 * 60 * 1000,
  jitterRatio: 0.25,
  maxConsecutiveFailures: 10,
  flushIntervalMs: 60 * 1000,
};

const HEARTBEAT = {
  launcherMs: 120 * 1000,
  gameMs: 60 * 1000,
  jitterRatio: 0.2,
};

const ONLINE = {
  pollMs: 60 * 1000,
  timeoutMs: 8000,
  staleAfterMs: 5 * 60 * 1000,
};

module.exports = {
  BACKEND_CONFIGURED,
  API_HOST,
  API_BASE,
  ENDPOINTS,
  SCHEMA_VERSION,
  QUEUE,
  TRANSPORT,
  HEARTBEAT,
  ONLINE,
};
