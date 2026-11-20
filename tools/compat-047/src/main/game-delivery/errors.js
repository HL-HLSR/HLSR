"use strict";

const path = require("path");

const CODES = {
  MANIFEST_NOT_FOUND: "MANIFEST_NOT_FOUND",
  MANIFEST_INVALID: "MANIFEST_INVALID",
  MANIFEST_DISABLED: "MANIFEST_DISABLED",
  MANIFEST_VERSION_DRAFT: "MANIFEST_VERSION_DRAFT",
  LAUNCHER_TOO_OLD: "LAUNCHER_TOO_OLD",

  REMOTE_DISABLED: "REMOTE_DISABLED",
  PROVIDER_UNSUPPORTED: "PROVIDER_UNSUPPORTED",
  PROVIDER_LINK_FAILED: "PROVIDER_LINK_FAILED",
  PROVIDER_FORBIDDEN: "PROVIDER_FORBIDDEN",

  NETWORK_ERROR: "NETWORK_ERROR",
  NETWORK_TIMEOUT: "NETWORK_TIMEOUT",
  HTTP_ERROR: "HTTP_ERROR",
  TOO_MANY_REDIRECTS: "TOO_MANY_REDIRECTS",
  INSECURE_REDIRECT: "INSECURE_REDIRECT",
  RANGE_NOT_SUPPORTED: "RANGE_NOT_SUPPORTED",
  CONTENT_LENGTH_MISMATCH: "CONTENT_LENGTH_MISMATCH",

  SIZE_MISMATCH: "SIZE_MISMATCH",
  CHECKSUM_MISMATCH: "CHECKSUM_MISMATCH",

  NO_SPACE: "NO_SPACE",
  PATH_NOT_WRITABLE: "PATH_NOT_WRITABLE",
  PATH_INVALID: "PATH_INVALID",

  ARCHIVE_LIST_FAILED: "ARCHIVE_LIST_FAILED",
  ARCHIVE_UNSAFE_ENTRY: "ARCHIVE_UNSAFE_ENTRY",
  ARCHIVE_LIMIT_EXCEEDED: "ARCHIVE_LIMIT_EXCEEDED",
  EXTRACT_FAILED: "EXTRACT_FAILED",
  REQUIRED_FILE_MISSING: "REQUIRED_FILE_MISSING",

  TOOL_UNKNOWN: "TOOL_UNKNOWN",
  TOOL_EXECUTABLE_MISSING: "TOOL_EXECUTABLE_MISSING",
  TOOL_INSTALL_FAILED: "TOOL_INSTALL_FAILED",
  TOOL_LAUNCH_FAILED: "TOOL_LAUNCH_FAILED",
  TOOL_REMOVE_FAILED: "TOOL_REMOVE_FAILED",

  ALREADY_RUNNING: "ALREADY_RUNNING",
  CANCELLED: "CANCELLED",
  INSTALL_FAILED: "INSTALL_FAILED",
  ANTIVIRUS_BLOCKED: "ANTIVIRUS_BLOCKED",

  // Полная сборка в архиве .hlsrpack (game-pack.js).
  PACK_FAILED: "PACK_FAILED",
  PACK_BAD_FORMAT: "PACK_BAD_FORMAT",
  PACK_NEWER_FORMAT: "PACK_NEWER_FORMAT",
  PACK_UNKNOWN_GAME: "PACK_UNKNOWN_GAME",
  PACK_TOO_LARGE: "PACK_TOO_LARGE",
  PACK_DEPOT_BUILD: "PACK_DEPOT_BUILD",
  PACK_VERSION_UNKNOWN: "PACK_VERSION_UNKNOWN",
  PACK_CORRUPT: "PACK_CORRUPT",

  NOT_INSTALLED: "NOT_INSTALLED",
  VERSION_MISMATCH: "VERSION_MISMATCH",
  PATCH_CHAIN_MISSING: "PATCH_CHAIN_MISSING",
  PATCH_FAILED: "PATCH_FAILED",
  FILE_LOCKED: "FILE_LOCKED",
  GAME_RUNNING: "GAME_RUNNING",
  VERIFY_FAILED: "VERIFY_FAILED",
  REPAIR_SOURCE_MISSING: "REPAIR_SOURCE_MISSING",
  REPAIR_FAILED: "REPAIR_FAILED",
  ROLLBACK_FAILED: "ROLLBACK_FAILED",
  FILES_MANIFEST_MISSING: "FILES_MANIFEST_MISSING",

  PROCESS_NOT_FOUND: "PROCESS_NOT_FOUND",
  PROCESS_MISMATCH: "PROCESS_MISMATCH",
  STOP_TIMEOUT: "STOP_TIMEOUT",
  STOP_FAILED: "STOP_FAILED",

  CHANNEL_DISABLED: "CHANNEL_DISABLED",
  CHANNEL_UNAVAILABLE: "CHANNEL_UNAVAILABLE",
  SIGNATURE_INVALID: "SIGNATURE_INVALID",
  UPDATE_REQUIRED: "UPDATE_REQUIRED",

  BUILD_METADATA_MISSING: "BUILD_METADATA_MISSING",
  BUILD_METADATA_INVALID: "BUILD_METADATA_INVALID",
  BUILD_METADATA_MISMATCH: "BUILD_METADATA_MISMATCH",
  BUILD_METADATA_UNSUPPORTED: "BUILD_METADATA_UNSUPPORTED",
  BUILD_REQUIRES_NEWER_LAUNCHER: "BUILD_REQUIRES_NEWER_LAUNCHER",

  STEAM_NOT_INSTALLED: "STEAM_NOT_INSTALLED",
  STEAM_NOT_RUNNING: "STEAM_NOT_RUNNING",
  STEAM_ELEVATED: "STEAM_ELEVATED",
  STEAM_PROCESS_MISMATCH: "STEAM_PROCESS_MISMATCH",
  SOURCE_SDK_NOT_INSTALLED: "SOURCE_SDK_NOT_INSTALLED",
  SOURCE_SDK_INCOMPLETE: "SOURCE_SDK_INCOMPLETE",
  GAME_EXECUTABLE_MISSING: "GAME_EXECUTABLE_MISSING",
  GAME_DIRECTORY_MISSING: "GAME_DIRECTORY_MISSING",
  LIBRARY_PATH_CASE_CONFLICT: "LIBRARY_PATH_CASE_CONFLICT",
  STEAM_APP_ID_MISMATCH: "STEAM_APP_ID_MISMATCH",
  LAUNCH_CONFIG_INVALID: "LAUNCH_CONFIG_INVALID",
  LAUNCH_FAILED: "LAUNCH_FAILED",
  PROTON_WRAPPER_MISSING: "PROTON_WRAPPER_MISSING",

  STEAM_NOT_FOUND: "STEAM_NOT_FOUND",
  STEAM_APP_NOT_OWNED: "STEAM_APP_NOT_OWNED",
  STEAM_CONTENT_BUSY: "STEAM_CONTENT_BUSY",
  STEAM_LINK_FAILED: "STEAM_LINK_FAILED",
  STEAM_DEPOT_FAILED: "STEAM_DEPOT_FAILED",
  STEAM_TIMEOUT: "STEAM_TIMEOUT",
  LICENSE_CHECK_UNAVAILABLE: "LICENSE_CHECK_UNAVAILABLE",
  STEAM_MANAGED_BUILD: "STEAM_MANAGED_BUILD",
};

class DeliveryError extends Error {
  constructor(code, message, details) {
    super(message || code);
    this.name = "DeliveryError";
    this.code = CODES[code] ? code : "INSTALL_FAILED";
    this.details = details || null;
  }

  toPublic() {
    return { code: this.code, message: this.message, details: this.details };
  }
}

function isDeliveryError(error) {
  return !!error && error.name === "DeliveryError";
}

// Windows: файл, который антивирус признал вирусом (ERROR_VIRUS_INFECTED,
// ERROR_VIRUS_DELETED), libuv не знает и отдаёт как «UNKNOWN: unknown
// error, open …». 7-Zip пишет системный текст ошибки («…contains a virus…»,
// «…содержит вирус…»). Защитник Windows так помечает инструменты спидрана,
// например инжектор Bunnymod XT в сборке HL1.
const FS_SYSCALLS = new Set(["open", "read", "rename", "copyfile", "unlink", "write", "close"]);

function isAntivirusBlock(error, platform) {
  if ((platform || process.platform) !== "win32" || !error) return false;
  if (error.code === "UNKNOWN" && FS_SYSCALLS.has(error.syscall)) return true;
  return /\bvirus\b|potentially unwanted|вирус/i.test(String(error.stderr || "") + " " + String(error.message || ""));
}

function antivirusError(filePath, error) {
  const name = filePath ? `«${path.win32.basename(filePath)}»` : "файл игры";
  return new DeliveryError(
    "ANTIVIRUS_BLOCKED",
    `Windows не дал открыть ${name}: скорее всего, его заблокировал антивирус. ` +
      "Защитник Windows бывает так реагирует на инструменты спидрана (Bunnymod XT). " +
      "Добавьте папку библиотеки HLSR в исключения антивируса и повторите установку.",
    { path: filePath || null, reason: (error && error.message ? String(error.message) : "").slice(0, 300) }
  );
}

function toDeliveryError(error, fallbackCode) {
  if (isDeliveryError(error)) return error;
  if (isAntivirusBlock(error)) return antivirusError(error.path, error);
  const code = fallbackCode || "INSTALL_FAILED";
  return new DeliveryError(code, (error && error.message) || String(error));
}

module.exports = { CODES, DeliveryError, isDeliveryError, toDeliveryError, isAntivirusBlock, antivirusError };
