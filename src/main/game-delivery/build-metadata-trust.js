"use strict";

const reader = require("./build-metadata-reader");
const buildMetadata = require("../../shared/build-metadata");
const semver = require("./semver");

const RELATIVE_PATH = buildMetadata.RELATIVE_PATH;

const REASONS = {
  MISSING: "BUILD_METADATA_MISSING",
  INVALID: "BUILD_METADATA_INVALID",
  MISMATCH: "BUILD_METADATA_MISMATCH",
  UNSUPPORTED: "BUILD_METADATA_UNSUPPORTED",
  REQUIRES_NEWER_LAUNCHER: "BUILD_REQUIRES_NEWER_LAUNCHER",
};

const ALL_REASONS = Object.keys(REASONS).map((k) => REASONS[k]);

const { REQUIRED_FIELDS: SNAPSHOT_FIELDS } = require("../../shared/build-metadata");

class BuildMetadataTrustError extends Error {
  constructor(code, message, detail) {
    super(message || code);
    this.name = "BuildMetadataTrustError";
    this.code = ALL_REASONS.indexOf(code) !== -1 ? code : REASONS.INVALID;
    this.detail = detail || null;
  }

  toPublic() {
    return { code: this.code, message: this.message, detail: this.detail };
  }
}

function trustError(code, message, detail) {
  return new BuildMetadataTrustError(code, message, detail);
}

function mapReaderError(error, detail) {
  const code =
    error && error.code === "BUILD_METADATA_UNSUPPORTED"
      ? REASONS.UNSUPPORTED
      : REASONS.INVALID;
  return trustError(code, (error && error.message) || "Метаданные сборки повреждены", detail);
}

function validateDescriptor(descriptor) {
  if (!descriptor || typeof descriptor !== "object" || Array.isArray(descriptor))
    throw trustError(REASONS.INVALID, "Некорректный descriptor buildMetadata");

  if (descriptor.path !== RELATIVE_PATH)
    throw trustError(
      REASONS.INVALID,
      `descriptor.path должен быть строго "${RELATIVE_PATH}", получено "${descriptor.path}"`,
      { expected: RELATIVE_PATH, actual: descriptor.path }
    );

  if (descriptor.schemaVersion !== buildMetadata.SCHEMA_VERSION)
    throw trustError(
      REASONS.UNSUPPORTED,
      `descriptor.schemaVersion должен быть ${buildMetadata.SCHEMA_VERSION}`
    );

  return descriptor;
}

function verifyForInstall(options) {
  const opts = options || {};
  const descriptor = validateDescriptor(opts.descriptor);
  const manifest = opts.manifest || {};

  let result;
  try {
    result = reader.read(opts.installRoot, { now: opts.now, expected: opts.expected });
  } catch (error) {
    throw mapReaderError(error, { phase: "install-read" });
  }

  if (!result.present)
    throw trustError(REASONS.MISSING, `Обязательный файл метаданных ${RELATIVE_PATH} отсутствует`, {
      relativePath: RELATIVE_PATH,
    });

  const dto = result.dto;

  let filesEntry = null;
  if (opts.filesManifest && Array.isArray(opts.filesManifest.files)) {
    filesEntry = opts.filesManifest.files.find((f) => f.path === RELATIVE_PATH) || null;

    if (!filesEntry)
      throw trustError(
        REASONS.MISMATCH,
        `Файл ${RELATIVE_PATH} не входит в проверяемый files manifest`,
        { relativePath: RELATIVE_PATH }
      );

    if (filesEntry.size !== result.sizeBytes || filesEntry.sha256 !== result.sha256)
      throw trustError(
        REASONS.MISMATCH,
        "Метаданные на диске не совпали с записью в files manifest (size/sha256)",
        {
          expected: { size: filesEntry.size, sha256: filesEntry.sha256 },
          actual: { size: result.sizeBytes, sha256: result.sha256 },
        }
      );
  }

  assertConsistentWithManifest(dto, descriptor, manifest);

  assertLauncherCompatible(dto, opts.launcherVersion);

  return {
    state: "trusted",
    reason: null,
    metadata: dto,
    filesEntry,
    fileSha256: result.sha256,
    fileSize: result.sizeBytes,
    verifiedAt: new Date(typeof opts.now === "number" ? opts.now : Date.now()).toISOString(),
    publishedAt: manifest.publishedAt || null,
  };
}

function assertConsistentWithManifest(dto, descriptor, manifest) {
  const pairs = [
    ["buildId", dto.buildId, descriptor.buildId],
    ["buildName", dto.buildName, descriptor.buildName],
    ["gitCommit", dto.gitCommit, descriptor.gitCommit],
    ["architecture", dto.architecture, descriptor.architecture],
  ];

  if (descriptor.builtAt != null) {
    let normalizedDescriptorBuiltAt;
    try {
      normalizedDescriptorBuiltAt = buildMetadata.normalizeBuiltAt(descriptor.builtAt);
    } catch (e) {
      throw trustError(REASONS.INVALID, `descriptor.builtAt некорректна: ${descriptor.builtAt}`);
    }
    pairs.push(["builtAt", dto.builtAt, normalizedDescriptorBuiltAt]);
  }

  for (const [field, actual, expected] of pairs)
    if (expected != null && actual !== expected)
      throw trustError(
        REASONS.MISMATCH,
        `Поле ${field} в метаданных (${actual}) не совпало с подписанным манифестом (${expected})`,
        { field, actual, expected }
      );

  if (manifest.gameId != null && dto.gameId !== manifest.gameId)
    throw trustError(
      REASONS.MISMATCH,
      `gameId метаданных (${dto.gameId}) не совпал с манифестом (${manifest.gameId})`,
      { field: "gameId", actual: dto.gameId, expected: manifest.gameId }
    );

  if (manifest.version != null && dto.version !== manifest.version)
    throw trustError(
      REASONS.MISMATCH,
      `version метаданных (${dto.version}) не совпала с манифестом (${manifest.version})`,
      { field: "version", actual: dto.version, expected: manifest.version }
    );
}

function assertLauncherCompatible(dto, launcherVersion) {
  if (!launcherVersion) return;
  if (semver.lt(launcherVersion, dto.minimumLauncherVersion))
    throw trustError(
      REASONS.REQUIRES_NEWER_LAUNCHER,
      `Сборке нужен лаунчер не ниже ${dto.minimumLauncherVersion}, установлен ${launcherVersion}`,
      { required: dto.minimumLauncherVersion, current: launcherVersion }
    );
}

function verifyAgainstSnapshot(options) {
  const opts = options || {};
  const snapshot = opts.snapshot || null;
  const hasMetadata = !!(snapshot && snapshot.buildMetadata);

  if (!hasMetadata) return { state: "legacy", reason: null, metadata: null };

  const expected = snapshot.buildMetadata;

  let result;
  try {
    result = reader.read(opts.installRoot, { now: opts.now, expected: opts.expected });
  } catch (error) {
    const mapped = mapReaderError(error, { phase: "runtime-read" });
    return { state: "repair-required", reason: mapped.code, repairable: true, metadata: null, detail: mapped.detail };
  }

  if (!result.present)
    return {
      state: "repair-required",
      reason: REASONS.MISSING,
      repairable: true,
      metadata: null,
      detail: { relativePath: RELATIVE_PATH },
    };

  const dto = result.dto;

  for (const field of SNAPSHOT_FIELDS)
    if (dto[field] !== expected[field])
      return {
        state: "repair-required",
        reason: REASONS.MISMATCH,
        repairable: true,
        metadata: null,
        detail: { field, actual: dto[field], expected: expected[field] },
      };

  if (opts.launcherVersion && semver.lt(opts.launcherVersion, dto.minimumLauncherVersion))
    return {
      state: "launcher-update-required",
      reason: REASONS.REQUIRES_NEWER_LAUNCHER,
      repairable: false,
      metadata: dto,
      detail: { required: dto.minimumLauncherVersion, current: opts.launcherVersion },
    };

  return { state: "trusted", reason: null, metadata: dto };
}

function snapshotFromVerified(verified) {
  const dto = verified.metadata;
  return {
    buildMetadata: {
      schemaVersion: dto.schemaVersion,
      gameId: dto.gameId,
      version: dto.version,
      buildId: dto.buildId,
      buildName: dto.buildName,
      gitCommit: dto.gitCommit,
      builtAt: dto.builtAt,
      architecture: dto.architecture,
      requiredSteamAppId: dto.requiredSteamAppId,
      minimumLauncherVersion: dto.minimumLauncherVersion,
    },
    buildMetadataVerifiedAt: verified.verifiedAt,
    buildMetadataSha256: verified.fileSha256,
    publishedAt: verified.publishedAt || null,
  };
}

module.exports = {
  REASONS,
  ALL_REASONS,
  RELATIVE_PATH,
  BuildMetadataTrustError,
  validateDescriptor,
  verifyForInstall,
  verifyAgainstSnapshot,
  snapshotFromVerified,
  assertConsistentWithManifest,
  assertLauncherCompatible,
};
