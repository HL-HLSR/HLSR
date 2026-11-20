"use strict";

const SCHEMA_VERSION = 1;

const ENVELOPE = [
  "schemaVersion",
  "eventId",
  "installationId",
  "sessionId",
  "type",
  "occurredAt",
  "launcherVersion",
  "locale",
  "windowsMajor",
  "windowsArch",
];

const ERROR_CODES = [
  "STEAM_NOT_RUNNING",
  "STEAM_NOT_INSTALLED",
  "SDK_2013_NOT_INSTALLED",
  "LICENSE_NOT_AVAILABLE",
  "HTTP_404",
  "HTTP_429",
  "HTTP_5XX",
  "NETWORK_TIMEOUT",
  "NETWORK_UNREACHABLE",
  "POSSIBLE_RESOURCE_BLOCK",
  "CHECKSUM_MISMATCH",
  "SIGNATURE_INVALID",
  "MANIFEST_INVALID",
  "EXTRACT_FAILED",
  "NO_SPACE",
  "REPAIR_FAILED",
  "PROFILE_IMPORT_FAILED",
  "LAUNCH_FAILED",
  "GAME_CRASH",
  "LAUNCHER_CRASH",
  "UNKNOWN_SAFE",
];

const EXIT_REASONS = ["normal", "user-closed", "crashed", "killed", "unknown"];
const PLAN_KINDS = ["full", "patch", "patch-chain"];
const FALLBACK_REASONS = [
  "PATCH_UNAVAILABLE",
  "PATCH_APPLY_FAILED",
  "CHECKSUM_MISMATCH",
  "CHAIN_TOO_LONG",
  "SOURCE_MISSING",
  "UNKNOWN_SAFE",
];

const LOCALES = ["ru", "en"];
const WINDOWS_MAJORS = ["10", "11", "unknown"];
const WINDOWS_ARCHS = ["x64"];

const NULLABLE = [
  "downloadBytes",
  "downloadDurationMilliseconds",
  "averageBytesPerSecond",
  "patchBytes",
  "fullArchiveBytes",
  "savedBytes",
];

const LIMITS = {
  durationSeconds: 2592000,
  elapsedMilliseconds: 360000000,
  segmentMilliseconds: 360000000,
  restartCount: 100000,
  bytes: 68719476736,
  downloadDurationMilliseconds: 604800000,
};

const TYPES = {
  launcher_session_started: { kind: "launcher", fields: [] },
  launcher_heartbeat: { kind: "launcher", fields: [] },
  launcher_session_ended: { kind: "launcher", fields: ["durationSeconds"] },
  launcher_crash_detected: { kind: "launcher", fields: [] },

  launcher_install_completed: { kind: "none", fields: [] },
  launcher_uninstall_initiated: { kind: "none", fields: [] },
  launcher_uninstall_completed: { kind: "none", fields: [] },

  game_launch_started: { kind: "game", fields: ["gameId", "gameVersion", "buildId"] },
  game_launch_succeeded: { kind: "game", fields: ["gameId", "gameVersion", "buildId"] },
  game_launch_failed: { kind: "game", fields: ["gameId", "gameVersion", "buildId", "errorCode"] },
  game_heartbeat: { kind: "game", fields: ["gameId", "gameVersion", "buildId"] },
  game_session_ended: {
    kind: "game",
    fields: ["gameId", "gameVersion", "buildId", "durationSeconds", "exitReason"],
  },
  game_crashed: {
    kind: "game",
    fields: ["gameId", "gameVersion", "buildId", "durationSeconds", "errorCode"],
  },

  run_started: {
    kind: "game",
    fields: ["gameId", "gameVersion", "buildId", "runId", "categoryId", "mapId", "chapterId"],
  },
  run_restarted: {
    kind: "game",
    fields: [
      "gameId", "gameVersion", "buildId", "runId", "categoryId",
      "mapId", "chapterId", "elapsedMilliseconds", "restartCount",
    ],
  },
  run_completed: {
    kind: "game",
    fields: [
      "gameId", "gameVersion", "buildId", "runId", "categoryId",
      "elapsedMilliseconds", "restartCount", "isPersonalBest",
    ],
  },
  run_abandoned: {
    kind: "game",
    fields: [
      "gameId", "gameVersion", "buildId", "runId", "categoryId",
      "mapId", "chapterId", "elapsedMilliseconds", "restartCount",
    ],
  },
  run_personal_best: {
    kind: "game",
    fields: ["gameId", "gameVersion", "buildId", "runId", "categoryId", "elapsedMilliseconds"],
  },
  run_heartbeat: {
    kind: "game",
    fields: [
      "gameId", "gameVersion", "buildId", "runId", "categoryId",
      "mapId", "chapterId", "elapsedMilliseconds",
    ],
  },
  map_entered: {
    kind: "game",
    fields: ["gameId", "gameVersion", "buildId", "runId", "mapId", "chapterId", "elapsedMilliseconds"],
  },
  map_left: {
    kind: "game",
    fields: [
      "gameId", "gameVersion", "buildId", "runId", "mapId", "chapterId",
      "elapsedMilliseconds", "segmentMilliseconds",
    ],
  },

  game_update_available: { kind: "launcher", fields: ["gameId", "fromVersion", "toVersion"] },
  game_download_started: { kind: "launcher", fields: ["gameId", "fromVersion", "toVersion", "planKind"] },
  game_download_completed: {
    kind: "launcher",
    fields: [
      "gameId", "fromVersion", "toVersion", "planKind", "downloadBytes",
      "downloadDurationMilliseconds", "averageBytesPerSecond",
      "patchBytes", "fullArchiveBytes", "savedBytes",
    ],
  },
  game_download_cancelled: {
    kind: "launcher",
    fields: ["gameId", "fromVersion", "toVersion", "planKind", "downloadBytes"],
  },
  game_update_installed: { kind: "launcher", fields: ["gameId", "fromVersion", "toVersion", "planKind"] },
  game_update_failed: {
    kind: "launcher",
    fields: ["gameId", "fromVersion", "toVersion", "planKind", "errorCode", "fallbackReason"],
  },
  game_repair_started: { kind: "launcher", fields: ["gameId", "gameVersion"] },
  game_repair_completed: { kind: "launcher", fields: ["gameId", "gameVersion", "durationSeconds"] },
  game_repair_failed: { kind: "launcher", fields: ["gameId", "gameVersion", "errorCode"] },

  launcher_update_available: { kind: "launcher", fields: ["fromVersion", "toVersion"] },
  launcher_download_started: { kind: "launcher", fields: ["fromVersion", "toVersion"] },
  launcher_download_completed: {
    kind: "launcher",
    fields: ["fromVersion", "toVersion", "downloadBytes", "downloadDurationMilliseconds", "averageBytesPerSecond"],
  },
  launcher_download_cancelled: { kind: "launcher", fields: ["fromVersion", "toVersion"] },
  launcher_update_installed: { kind: "launcher", fields: ["fromVersion", "toVersion"] },
  launcher_update_failed: { kind: "launcher", fields: ["fromVersion", "toVersion", "errorCode"] },
};

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/;

const BUILD_RE = /^[a-z0-9]+([._-][a-z0-9]+)*$/;
const BUILD_MAX = 128;

const ID_RE = /^[0-9a-z][0-9a-z._-]{0,63}$/i;
const TIMESTAMP_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/;

function isBoundedInt(value, max) {
  return Number.isInteger(value) && value >= 0 && value <= max;
}

function checkField(name, value) {
  switch (name) {
    case "gameId":
    case "mapId":
    case "chapterId":
    case "categoryId":
      return typeof value === "string" && ID_RE.test(value);

    case "gameVersion":
    case "fromVersion":
    case "toVersion":
      return typeof value === "string" && VERSION_RE.test(value);

    case "buildId":
      return (
        typeof value === "string" &&
        value.length >= 3 &&
        value.length <= BUILD_MAX &&
        BUILD_RE.test(value)
      );

    case "runId":
      return typeof value === "string" && UUID_RE.test(value);

    case "errorCode":
      return ERROR_CODES.indexOf(value) !== -1;

    case "exitReason":
      return EXIT_REASONS.indexOf(value) !== -1;

    case "planKind":
      return PLAN_KINDS.indexOf(value) !== -1;

    case "fallbackReason":
      return FALLBACK_REASONS.indexOf(value) !== -1;

    case "durationSeconds":
      return isBoundedInt(value, LIMITS.durationSeconds);

    case "elapsedMilliseconds":
      return isBoundedInt(value, LIMITS.elapsedMilliseconds);

    case "segmentMilliseconds":
      return isBoundedInt(value, LIMITS.segmentMilliseconds);

    case "restartCount":
      return isBoundedInt(value, LIMITS.restartCount);

    case "downloadBytes":
    case "patchBytes":
    case "fullArchiveBytes":
    case "savedBytes":
    case "averageBytesPerSecond":
      return isBoundedInt(value, LIMITS.bytes);

    case "downloadDurationMilliseconds":
      return isBoundedInt(value, LIMITS.downloadDurationMilliseconds);

    case "isPersonalBest":
      return typeof value === "boolean";

    default:
      return false;
  }
}

function validate(event) {
  if (!event || typeof event !== "object" || Array.isArray(event))
    return { ok: false, reason: "not-object" };

  const spec = TYPES[event.type];
  if (!spec) return { ok: false, reason: "unknown-type" };

  const allowed = ENVELOPE.concat(spec.fields);
  for (const key of Object.keys(event))
    if (allowed.indexOf(key) === -1) return { ok: false, reason: `unknown-field:${key}` };

  if (event.schemaVersion !== SCHEMA_VERSION) return { ok: false, reason: "schema-version" };
  if (typeof event.eventId !== "string" || !UUID_RE.test(event.eventId))
    return { ok: false, reason: "event-id" };
  if (typeof event.installationId !== "string" || !UUID_RE.test(event.installationId))
    return { ok: false, reason: "installation-id" };

  if (spec.kind === "none") {
    if (event.sessionId !== null) return { ok: false, reason: "session-forbidden" };
  } else if (typeof event.sessionId !== "string" || !UUID_RE.test(event.sessionId)) {
    return { ok: false, reason: "session-id" };
  }

  if (typeof event.occurredAt !== "string" || !TIMESTAMP_RE.test(event.occurredAt))
    return { ok: false, reason: "occurred-at" };
  if (typeof event.launcherVersion !== "string" || !VERSION_RE.test(event.launcherVersion))
    return { ok: false, reason: "launcher-version" };
  if (LOCALES.indexOf(event.locale) === -1) return { ok: false, reason: "locale" };
  if (WINDOWS_MAJORS.indexOf(event.windowsMajor) === -1) return { ok: false, reason: "windows-major" };
  if (WINDOWS_ARCHS.indexOf(event.windowsArch) === -1) return { ok: false, reason: "windows-arch" };

  for (const field of spec.fields) {
    if (!(field in event)) return { ok: false, reason: `missing:${field}` };

    if (event[field] === null) {
      if (NULLABLE.indexOf(field) === -1) return { ok: false, reason: `null:${field}` };
      continue;
    }

    if (!checkField(field, event[field])) return { ok: false, reason: `bad:${field}` };
  }

  return { ok: true };
}

const DISABLED_TYPES = [
  "run_started",
  "run_restarted",
  "run_completed",
  "run_abandoned",
  "run_personal_best",
  "run_heartbeat",
  "map_entered",
  "map_left",
];

function isEnabledType(type) {
  return Object.prototype.hasOwnProperty.call(TYPES, type) && DISABLED_TYPES.indexOf(type) === -1;
}

function gameTelemetryConnected() {
  return isEnabledType("game_heartbeat");
}

function emittedTypes() {
  return Object.keys(TYPES).filter(
    (type) => type !== "launcher_uninstall_completed" && isEnabledType(type)
  );
}

module.exports = {
  SCHEMA_VERSION,
  ENVELOPE,
  ERROR_CODES,
  EXIT_REASONS,
  PLAN_KINDS,
  FALLBACK_REASONS,
  LOCALES,
  WINDOWS_MAJORS,
  WINDOWS_ARCHS,
  LIMITS,
  NULLABLE,
  TYPES,
  DISABLED_TYPES,
  validate,
  checkField,
  emittedTypes,
  isEnabledType,
  gameTelemetryConnected,
};
