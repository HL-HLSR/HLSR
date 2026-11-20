"use strict";

const CODES = {
  ALREADY_RUNNING: "UPD_ALREADY_RUNNING",
  BAD_PLAN: "UPD_BAD_PLAN",
  PLAN_UNREADABLE: "UPD_PLAN_UNREADABLE",
  STAGING_FAILED: "UPD_STAGING_FAILED",

  DOWNLOAD_FAILED: "UPD_DOWNLOAD_FAILED",
  DOWNLOAD_HTTP: "UPD_DOWNLOAD_HTTP",
  DOWNLOAD_TIMEOUT: "UPD_DOWNLOAD_TIMEOUT",
  DOWNLOAD_SIZE: "UPD_DOWNLOAD_SIZE",
  NO_SPACE: "UPD_NO_SPACE",
  CANCELLED: "UPD_CANCELLED",

  URL_NOT_ALLOWED: "UPD_URL_NOT_ALLOWED",
  FILENAME_MISMATCH: "UPD_FILENAME_MISMATCH",
  VERSION_MISMATCH: "UPD_VERSION_MISMATCH",
  SIGNATURE_INVALID: "UPD_SIGNATURE_INVALID",

  HLSR_STILL_RUNNING: "UPD_HLSR_STILL_RUNNING",
  INSTALL_FAILED: "UPD_INSTALL_FAILED",
  INSTALL_EXIT_CODE: "UPD_INSTALL_EXIT_CODE",
  INSTALL_NOT_APPLIED: "UPD_INSTALL_NOT_APPLIED",
  RELAUNCH_FAILED: "UPD_RELAUNCH_FAILED",
};

class UpdaterError extends Error {
  constructor(code, message, details) {
    super(message || code);
    this.name = "UpdaterError";
    this.code = code;
    this.details = details || null;
  }

  toPublic() {
    return { code: this.code, message: String(this.message || "").slice(0, 400) };
  }
}

function toUpdaterError(error, fallbackCode) {
  if (error instanceof UpdaterError) return error;

  const code = fallbackCode || CODES.INSTALL_FAILED;
  return new UpdaterError(code, error ? String(error.message || error) : "unknown");
}

module.exports = { CODES, UpdaterError, toUpdaterError };
