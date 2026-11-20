"use strict";

const ERROR_CODES = new Set([
  // Сервер не узнал сборку лаунчера (security/build-identity.js).
  "CLIENT_MODIFIED",
  "STEAM_NOT_INSTALLED",
  "STEAM_NOT_RUNNING",
  "STEAM_PROCESS_MISMATCH",
  "SOURCE_SDK_NOT_INSTALLED",
  "SOURCE_SDK_INCOMPLETE",
  "STEAM_APP_ID_MISMATCH",
  "GAME_EXECUTABLE_MISSING",
  "GAME_DIRECTORY_MISSING",
  "LIBRARY_PATH_CASE_CONFLICT",
  "LAUNCH_FAILED",
  "LAUNCH_CONFIG_INVALID",
  "GAME_CRASHED",

  "SIGNATURE_INVALID",
  "CHECKSUM_MISMATCH",
  "ARCHIVE_INVALID",
  "ARCHIVE_UNSUPPORTED",
  "PATCH_INVALID",
  "PATCH_FAILED",
  "PATCH_CHAIN_MISSING",
  "NO_SPACE",
  "PATH_NOT_WRITABLE",
  "PATH_INVALID",
  "REPAIR_FAILED",
  "REPAIR_SOURCE_MISSING",
  "INSTALL_FAILED",
  // Не скопировался файл игрока перед подменой сборки (game-delivery/errors.js).
  "PRESERVE_FAILED",
  // Windows: файл игры заблокировал антивирус (game-delivery/errors.js).
  "ANTIVIRUS_BLOCKED",
  "UPDATE_FAILED",
  "FILES_MANIFEST_INVALID",

  "LU_SIGNATURE_INVALID",
  "LU_SCHEMA_INVALID",
  "LU_INSTALLER_SHA256",
  "LU_INSTALLER_SHA512",
  "LU_INSTALLER_SIZE",
  "LU_VERSION_MISMATCH",
  "LU_LATEST_YML_MISMATCH",
  "LU_VERSION_UNSUPPORTED",
  "LAUNCHER_UPDATE_ERROR",

  "STARTUP_ERROR",
]);

const WARNING_CODES = new Set([
  "MANIFEST_UNAVAILABLE",
  "NETWORK_ERROR",
  "FEED_UNAVAILABLE",
  "LU_FEED_UNAVAILABLE",
  "UPDATE_CHECK_TIMEOUT",
  "FILE_LOCKED",
  "GAME_RUNNING",
  "LIBRARY_LEGACY_LAYOUT",

  // Модерация из админки (online-profile/moderation.js).
  "MOD_BAN",
  "MOD_MUTE",
  "MOD_RESET_STATUS",
  "MOD_RESET_NICK",
  "MOD_RESET_AVATAR",
]);

const INFO_CODES = new Set([
  "SUPPORT_THANKS",
  "LAUNCHER_UPDATE_AVAILABLE",
  "LAUNCHER_UPDATED",
  "GAME_UPDATE_AVAILABLE",
  "GAME_UPDATED",
  "GAME_INSTALLED",
  "MOD_UNBAN",
  "MOD_UNMUTE",
  "MOD_RESTORE_NICK",
  "MOD_RESTORE_AVATAR",
]);

const IGNORED_CODES = new Set([
  "REMOTE_DISABLED",
  "MANIFEST_DISABLED",
  "MANIFEST_VERSION_DRAFT",
  "NOT_INSTALLED",
  "ALREADY_RUNNING",
  "UP_TO_DATE",
]);

function levelForCode(code) {
  const value = String(code || "");
  if (IGNORED_CODES.has(value)) return null;
  if (ERROR_CODES.has(value)) return "error";
  if (WARNING_CODES.has(value)) return "warning";
  if (INFO_CODES.has(value)) return "info";
  return null;
}

module.exports = { ERROR_CODES, WARNING_CODES, INFO_CODES, IGNORED_CODES, levelForCode };
