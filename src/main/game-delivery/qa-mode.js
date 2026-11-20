"use strict";

const path = require("path");
const featureFlag = require("./feature-flag");
const { DeliveryError } = require("./errors");

const QA_FLAG = "HLSR_GAME_DELIVERY_QA";
const QA_LIBRARY_FLAG = "HLSR_GAME_DELIVERY_QA_LIBRARY";
const QA_PATCHES_FLAG = "HLSR_GAME_DELIVERY_QA_PATCHES";
const QA_MANIFEST_FLAG = "HLSR_GAME_DELIVERY_QA_MANIFEST";
const QA_FILES_FLAG = "HLSR_GAME_DELIVERY_QA_FILES";
const DEFAULT_QA_LIBRARY =
  process.platform === "win32"
    ? "C:\\HLSR-Yandex-QA"
    : path.join(require("os").homedir(), "Games", "HLSR-Yandex-QA");
const QA_VERSION = "1.0.0";
const QA_PROVIDER_ID = "yandex-public-temp";

const PRODUCTION_BUILD = process.env.NODE_ENV === "production";

function evaluate(options) {
  const opts = options || {};
  const env = opts.env || {};

  if (opts.productionBuild) return { enabled: false, reason: "production-build" };
  if (env[QA_FLAG] !== "true") return { enabled: false, reason: "qa-flag-off" };
  if (!featureFlag.isRemoteDeliveryEnabled(env))
    return { enabled: false, reason: "remote-disabled" };

  return { enabled: true, reason: null };
}

function describe(env) {
  const source = env || process.env;
  const result = evaluate({ productionBuild: PRODUCTION_BUILD, env: source });

  return {
    flag: QA_FLAG,
    productionBuild: PRODUCTION_BUILD,
    enabled: result.enabled,
    reason: result.reason,
    libraryPath: result.enabled ? libraryPath(source) : null,
    version: QA_VERSION,
  };
}

function isEnabled(env) {
  return evaluate({ productionBuild: PRODUCTION_BUILD, env: env || process.env }).enabled;
}

function libraryPath(env) {
  const source = env || process.env;
  const raw = source[QA_LIBRARY_FLAG] || DEFAULT_QA_LIBRARY;
  const resolved = path.resolve(raw);

  const absolute =
    process.platform === "win32" ? /^[A-Za-z]:[\\/]/.test(resolved) : path.isAbsolute(raw);

  if (!absolute || path.dirname(resolved) === resolved)
    throw new DeliveryError("PATH_INVALID", `Недопустимый путь QA-библиотеки: ${raw}`);

  return resolved;
}

function loadQaPatches(env) {
  const file = (env || process.env)[QA_PATCHES_FLAG];
  if (!file) return null;

  const fs = require("fs");

  try {
    const data = JSON.parse(fs.readFileSync(file, "utf8"));
    return Array.isArray(data) ? data : data.patches || null;
  } catch (e) {
    throw new DeliveryError("MANIFEST_INVALID", `Не удалось прочитать QA-патчи из ${file}: ${e.message}`);
  }
}

function manifestFor(base, now, env) {
  const copy = JSON.parse(JSON.stringify(base));

  copy.channel = "test";
  copy.version = base.version || QA_VERSION;
  copy.versionStatus = "assigned";
  copy.publishedAt = (now || new Date()).toISOString();
  copy.enabled = true;
  copy.notes =
    "RUNTIME-КОПИЯ ДЛЯ ЛОКАЛЬНОГО QA. Не публиковать. Создаётся в памяти при " +
    `${QA_FLAG}=true и не сохраняется на диск; файл манифеста в репозитории ` +
    "остаётся выключенным черновиком.";

  const temporary = copy.baseArchive.providers.filter(
    (provider) => provider.id === QA_PROVIDER_ID
  );

  if (temporary.length) copy.baseArchive.providers = temporary;

  if (!copy.baseArchive.providers.length)
    throw new DeliveryError(
      "PROVIDER_UNSUPPORTED",
      "В манифесте нет ни одного источника для QA-режима"
    );

  const patches = loadQaPatches(env);
  if (patches) {
    copy.patches = patches;
    const last = patches[patches.length - 1];
    if (last && last.toVersion) copy.version = last.toVersion;
  }

  return copy;
}

function manifestOverrides(env) {
  const raw = (env || process.env)[QA_MANIFEST_FLAG];
  if (!raw) return null;

  const fs = require("fs");
  let map;

  try {
    map = JSON.parse(raw);
  } catch (e) {
    throw new DeliveryError("MANIFEST_INVALID", `${QA_MANIFEST_FLAG} не является JSON: ${e.message}`);
  }

  const result = {};

  for (const [gameId, file] of Object.entries(map)) {
    try {
      result[gameId] = JSON.parse(fs.readFileSync(file, "utf8"));
    } catch (e) {
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Не удалось прочитать QA-манифест ${gameId} из ${file}: ${e.message}`
      );
    }
  }

  return result;
}

function manifestsFor(registry, now, env) {
  const overrides = manifestOverrides(env) || {};
  const result = {};

  for (const gameId of Object.keys(registry))
    result[gameId] = manifestFor(overrides[gameId] || registry[gameId], now, env);

  return result;
}

function filesManifestsFor(registry, env) {
  const raw = (env || process.env)[QA_FILES_FLAG];

  const result = {};
  for (const gameId of Object.keys(registry))
    result[gameId] = Object.assign({}, registry[gameId]);

  if (!raw) return result;

  const fs = require("fs");
  let extra;

  try {
    extra = JSON.parse(raw);
  } catch (e) {
    throw new DeliveryError("MANIFEST_INVALID", `${QA_FILES_FLAG} не является JSON: ${e.message}`);
  }

  for (const [gameId, byVersion] of Object.entries(extra)) {
    if (!result[gameId]) result[gameId] = {};

    for (const [version, file] of Object.entries(byVersion)) {
      try {
        result[gameId][version] = JSON.parse(fs.readFileSync(file, "utf8"));
      } catch (e) {
        throw new DeliveryError(
          "MANIFEST_INVALID",
          `Не удалось прочитать файловый манифест ${gameId} ${version} из ${file}: ${e.message}`
        );
      }
    }
  }

  return result;
}

module.exports = {
  QA_FLAG,
  QA_LIBRARY_FLAG,
  QA_PATCHES_FLAG,
  QA_FILES_FLAG,
  QA_MANIFEST_FLAG,
  manifestOverrides,
  loadQaPatches,
  filesManifestsFor,
  QA_VERSION,
  DEFAULT_QA_LIBRARY,
  PRODUCTION_BUILD,
  evaluate,
  describe,
  isEnabled,
  libraryPath,
  manifestFor,
  manifestsFor,
};
