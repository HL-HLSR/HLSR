"use strict";

const fs = require("fs");
const crypto = require("crypto");
const semver = require("./semver");
const { DeliveryError } = require("./errors");
const { compileSchema, describeErrors } = require("./schema-validator");

const SCHEMA = require("./schemas/game-manifest.schema.json");
const endpoints = require("../../shared/delivery-endpoints");
const SUPPORTED_SCHEMA_VERSION = 1;

class ManifestService {
  constructor(options) {
    const opts = options || {};
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.validator = null;
  }

  getValidator() {
    if (this.validator) return this.validator;

    this.validator = compileSchema(SCHEMA);
    return this.validator;
  }

  loadFromFile(filePath) {
    let raw;
    try {
      raw = fs.readFileSync(filePath, "utf8");
    } catch (e) {
      throw new DeliveryError("MANIFEST_NOT_FOUND", `Манифест не найден: ${filePath}`);
    }

    return this.parse(raw, filePath);
  }

  parse(raw, source) {
    let data;
    try {
      data = JSON.parse(raw);
    } catch (e) {
      throw new DeliveryError("MANIFEST_INVALID", `Манифест не является JSON: ${e.message}`, {
        source: source || null,
      });
    }

    return this.validate(data, source);
  }

  validate(data, source) {
    if (!data || typeof data !== "object" || Array.isArray(data))
      throw new DeliveryError("MANIFEST_INVALID", "Манифест должен быть JSON-объектом");

    if (data.schemaVersion !== SUPPORTED_SCHEMA_VERSION)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Неподдерживаемая версия схемы манифеста: ${data.schemaVersion}`,
        { expected: SUPPORTED_SCHEMA_VERSION }
      );

    const validate = this.getValidator();
    if (!validate(data)) {
      const errors = describeErrors(validate.errors);

      throw new DeliveryError("MANIFEST_INVALID", `Манифест не прошёл валидацию: ${errors[0]}`, {
        source: source || null,
        errors: errors.slice(0, 20),
      });
    }

    const badUrls = [];
    const inspect = (providerList) => {
      for (const provider of providerList || []) {
        if (!provider || !provider.url) continue;
        if (provider.type !== "https-direct") continue;
        if (!endpoints.isAllowedDownloadUrl(provider.url)) badUrls.push(provider.url);
      }
    };

    inspect(data.baseArchive && data.baseArchive.providers);
    inspect(data.filesManifest && data.filesManifest.providers);
    for (const patch of data.patches || []) {
      inspect(patch.archive && patch.archive.providers);
      inspect(patch.providers);
    }

    if (badUrls.length)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Источник вне allowlist: ${badUrls[0]}`,
        { source: source || null, hosts: badUrls.slice(0, 5) }
      );

    return {
      manifest: data,
      sha256: ManifestService.hash(data),
      source: source || null,
    };
  }

  static hash(manifest) {
    return crypto.createHash("sha256").update(JSON.stringify(manifest), "utf8").digest("hex");
  }

  checkInstallable(manifest, options) {
    const opts = options || {};
    const allowedProviderTypes = opts.allowedProviderTypes || [];
    const reasons = [];

    if (!manifest.enabled) reasons.push({ code: "MANIFEST_DISABLED" });

    if (!manifest.version || manifest.versionStatus === "draft")
      reasons.push({ code: "MANIFEST_VERSION_DRAFT" });

    if (!manifest.expectedInstalledSize) reasons.push({ code: "MANIFEST_INVALID" });

    const launcherVersion = opts.launcherVersion || this.launcherVersion;
    if (semver.lt(launcherVersion, manifest.minimumLauncherVersion))
      reasons.push({
        code: "LAUNCHER_TOO_OLD",
        details: { required: manifest.minimumLauncherVersion, current: launcherVersion },
      });

    const usable = manifest.baseArchive.providers.filter(
      (provider) => allowedProviderTypes.indexOf(provider.type) !== -1
    );

    if (!usable.length) reasons.push({ code: "REMOTE_DISABLED" });

    return { ok: reasons.length === 0, reasons, providers: usable };
  }

  assertInstallable(manifest, options) {
    const check = this.checkInstallable(manifest, options);
    if (check.ok) return check;

    const first = check.reasons[0];
    throw new DeliveryError(first.code, `Установка сборки недоступна: ${first.code}`, {
      reasons: check.reasons,
    });
  }

  static orderProviders(providers) {
    return providers
      .slice()
      .sort((a, b) => (a.priority == null ? 50 : a.priority) - (b.priority == null ? 50 : b.priority));
  }
}

module.exports = ManifestService;
module.exports.SUPPORTED_SCHEMA_VERSION = SUPPORTED_SCHEMA_VERSION;
module.exports.SCHEMA = SCHEMA;
