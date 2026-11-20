"use strict";

const SCHEMA_VERSION = 1;

const MAX_BYTES = 64 * 1024;

const RELATIVE_PATH = "ghostingmod/hlsr-build.json";

const CODES = {
  BUILD_METADATA_INVALID: "BUILD_METADATA_INVALID",
  BUILD_METADATA_UNSUPPORTED: "BUILD_METADATA_UNSUPPORTED",
};

class BuildMetadataError extends Error {
  constructor(code, message, detail) {
    super(message || code);
    this.name = "BuildMetadataError";
    this.code = CODES[code] ? code : "BUILD_METADATA_INVALID";
    this.detail = detail || null;
  }

  toPublic() {
    return { code: this.code, message: this.message, detail: this.detail };
  }
}

const PRODUCT_PROFILE = {
  gameId: "ghosting-renewed",
  buildName: "HL1 Movement",
  architecture: "x86",
  requiredSteamAppId: 243730,
};

const REQUIRED_FIELDS = [
  "schemaVersion",
  "gameId",
  "version",
  "buildId",
  "buildName",
  "gitCommit",
  "builtAt",
  "architecture",
  "requiredSteamAppId",
  "minimumLauncherVersion",
];

const ALLOWED_FIELDS = new Set(REQUIRED_FIELDS);

const SEMVER_FULL = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;
const SEMVER_STRICT = /^\d+\.\d+\.\d+$/;

const BUILD_ID_PATTERN = /^[a-z0-9]+([._-][a-z0-9]+)*$/;
const BUILD_ID_MAX = 128;

const GIT_COMMIT_PATTERN = /^[0-9a-f]{7,40}$/;

const DATE_FLOOR_MS = Date.UTC(2020, 0, 1);
const FUTURE_SKEW_MS = 48 * 60 * 60 * 1000;

function invalid(message, detail) {
  return new BuildMetadataError("BUILD_METADATA_INVALID", message, detail);
}

function isPlainObject(value) {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function normalizeBuiltAt(value, nowMs) {
  if (typeof value !== "string")
    throw invalid("builtAt должен быть строкой ISO-8601", { field: "builtAt" });

  const shape = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d{1,3})?Z$/;
  const m = value.match(shape);
  if (!m) throw invalid(`builtAt не является UTC ISO-8601: ${value}`, { field: "builtAt" });

  const ms = Date.parse(value);
  if (Number.isNaN(ms)) throw invalid(`builtAt не разбирается как дата: ${value}`, { field: "builtAt" });

  const back = new Date(ms);
  const rebuilt =
    `${String(back.getUTCFullYear()).padStart(4, "0")}-` +
    `${String(back.getUTCMonth() + 1).padStart(2, "0")}-` +
    `${String(back.getUTCDate()).padStart(2, "0")}T` +
    `${String(back.getUTCHours()).padStart(2, "0")}:` +
    `${String(back.getUTCMinutes()).padStart(2, "0")}:` +
    `${String(back.getUTCSeconds()).padStart(2, "0")}Z`;
  const shapeNoMs = value.replace(/\.\d{1,3}Z$/, "Z");
  if (rebuilt !== shapeNoMs)
    throw invalid(`builtAt содержит несуществующую дату: ${value}`, { field: "builtAt" });

  if (ms < DATE_FLOOR_MS)
    throw invalid(`builtAt слишком старая, дата сборки не может быть раньше 2020: ${value}`, {
      field: "builtAt",
    });

  const ceiling = (typeof nowMs === "number" ? nowMs : Date.now()) + FUTURE_SKEW_MS;
  if (ms > ceiling)
    throw invalid(`builtAt в будущем — дата сборки недостоверна: ${value}`, { field: "builtAt" });

  return back.toISOString();
}

function validate(data, options) {
  const opts = options || {};
  const expected = opts.expected || PRODUCT_PROFILE;
  const source = opts.source || null;

  if (!isPlainObject(data))
    throw invalid("Метаданные сборки должны быть JSON-объектом", { source });

  if (data.schemaVersion !== SCHEMA_VERSION)
    throw new BuildMetadataError(
      "BUILD_METADATA_UNSUPPORTED",
      `Неподдерживаемая версия схемы метаданных: ${data.schemaVersion}`,
      { expected: SCHEMA_VERSION, actual: data.schemaVersion, source }
    );

  for (const field of REQUIRED_FIELDS)
    if (!(field in data))
      throw invalid(`В метаданных сборки нет поля ${field}`, { field, source });

  for (const key of Object.keys(data))
    if (!ALLOWED_FIELDS.has(key))
      throw invalid(`Неизвестное поле в метаданных сборки: ${key}`, { field: key, source });

  if (typeof data.gameId !== "string" || data.gameId !== expected.gameId)
    throw invalid(
      `Метаданные описывают игру ${data.gameId}, ожидалась ${expected.gameId}`,
      { field: "gameId", expected: expected.gameId, actual: data.gameId, source }
    );

  if (typeof data.version !== "string" || !SEMVER_FULL.test(data.version) || data.version.length > 64)
    throw invalid(`Некорректная версия сборки: ${data.version}`, { field: "version", source });

  if (
    typeof data.buildId !== "string" ||
    data.buildId.length < 3 ||
    data.buildId.length > BUILD_ID_MAX ||
    !BUILD_ID_PATTERN.test(data.buildId)
  )
    throw invalid(`Недопустимый buildId: ${data.buildId}`, { field: "buildId", source });

  if (typeof data.buildName !== "string" || data.buildName !== expected.buildName)
    throw invalid(
      `Метаданные объявляют сборку "${data.buildName}", ожидалась "${expected.buildName}"`,
      { field: "buildName", expected: expected.buildName, actual: data.buildName, source }
    );

  if (typeof data.gitCommit !== "string" || !GIT_COMMIT_PATTERN.test(data.gitCommit))
    throw invalid(`Некорректный gitCommit: ${data.gitCommit}`, { field: "gitCommit", source });

  const builtAt = normalizeBuiltAt(data.builtAt, opts.now);

  if (typeof data.architecture !== "string" || data.architecture !== expected.architecture)
    throw invalid(
      `Архитектура ${data.architecture} не поддерживается, ожидалась ${expected.architecture}`,
      { field: "architecture", expected: expected.architecture, actual: data.architecture, source }
    );

  if (
    typeof data.requiredSteamAppId !== "number" ||
    !Number.isInteger(data.requiredSteamAppId) ||
    data.requiredSteamAppId !== expected.requiredSteamAppId
  )
    throw invalid(
      `requiredSteamAppId ${data.requiredSteamAppId} не совпадает с ожидаемым ${expected.requiredSteamAppId}`,
      { field: "requiredSteamAppId", expected: expected.requiredSteamAppId, actual: data.requiredSteamAppId, source }
    );

  if (typeof data.minimumLauncherVersion !== "string" || !SEMVER_STRICT.test(data.minimumLauncherVersion))
    throw invalid(
      `Некорректная minimumLauncherVersion: ${data.minimumLauncherVersion}`,
      { field: "minimumLauncherVersion", source }
    );

  return {
    schemaVersion: SCHEMA_VERSION,
    gameId: data.gameId,
    version: data.version,
    buildId: data.buildId,
    buildName: data.buildName,
    gitCommit: data.gitCommit,
    builtAt,
    architecture: data.architecture,
    requiredSteamAppId: data.requiredSteamAppId,
    minimumLauncherVersion: data.minimumLauncherVersion,
  };
}

function parse(raw, options) {
  let data;
  try {
    data = JSON.parse(raw);
  } catch (e) {
    throw invalid(`Метаданные сборки не являются JSON: ${e.message}`, {
      source: (options && options.source) || null,
    });
  }
  return validate(data, options);
}

module.exports = {
  SCHEMA_VERSION,
  MAX_BYTES,
  RELATIVE_PATH,
  CODES,
  PRODUCT_PROFILE,
  REQUIRED_FIELDS,
  BuildMetadataError,
  validate,
  parse,
  normalizeBuiltAt,
};
