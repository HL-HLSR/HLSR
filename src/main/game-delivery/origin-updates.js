"use strict";

const httpClient = require("./http-client");
const { DeliveryError } = require("./errors");

// У сборок без нашего подписанного канала (HL1 с bxt.rs) ставится только
// архив, чей SHA-256 закреплён во встроенном манифесте. Лаунчер сверяет
// Last-Modified и ETag архива в источнике с отпечатком закреплённой сборки:
// если там уже другая, обновление не предлагается, в журнал пишется
// предупреждение — новая сборка ждёт выпуска HLSR с её SHA-256.

const PROBE_TIMEOUT_MS = 10000;

// Отпечаток архива, из которого собран встроенный манифест.
const ORIGINS = {
  "hl1-won": {
    providerId: "bxt-rs",
    baseline: {
      version: "1.0.0",
      lastModified: "Mon, 17 Jul 2023 11:23:57 GMT",
      etag: '"64b524cd-9aee5c0"',
      size: 162457024,
    },
  },
};

function sameStamp(a, b) {
  if (!a || !b || a.size !== b.size) return false;
  if (a.etag && b.etag) return a.etag === b.etag;
  return !!a.lastModified && a.lastModified === b.lastModified;
}

class OriginUpdates {
  constructor(options) {
    const opts = options || {};
    this.client = opts.client || httpClient;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.origins = opts.origins || ORIGINS;
    this.warned = null;
  }

  supports(gameId) {
    return !!this.origins[gameId];
  }

  providerOf(gameId, manifest) {
    const origin = this.origins[gameId];
    const providers = (manifest.baseArchive && manifest.baseArchive.providers) || [];
    const provider = providers.find((item) => item.id === origin.providerId);
    if (!provider)
      throw new DeliveryError("MANIFEST_INVALID", `В манифесте ${gameId} нет источника ${origin.providerId}`);
    return provider;
  }

  async probe(gameId, manifest) {
    const provider = this.providerOf(gameId, manifest);
    const response = await this.client.request(provider.url, { method: "HEAD", timeout: PROBE_TIMEOUT_MS });
    response.resume();

    if (response.statusCode !== 200)
      throw new DeliveryError("HTTP_ERROR", `HTTP ${response.statusCode} при проверке ${provider.url}`);

    const size = Number(response.headers["content-length"]);
    if (!Number.isInteger(size) || size <= 0)
      throw new DeliveryError("PROVIDER_LINK_FAILED", "Источник не сообщил размер архива");

    return {
      size,
      etag: response.headers.etag || null,
      lastModified: response.headers["last-modified"] || null,
    };
  }

  // Цель обновления в том же виде, что у канала — только закреплённая сборка.
  // null: в источнике другой архив, UpdateChecker берёт встроенный манифест.
  async target(gameId, manifest) {
    const origin = this.origins[gameId];
    const stamp = await this.probe(gameId, manifest);

    if (!sameStamp(stamp, origin.baseline)) {
      const mark = `${stamp.etag || stamp.lastModified || "?"}, ${stamp.size} Б`;
      // Проверка идёт каждые несколько минут — одной записи на отпечаток хватит.
      if (this.warned !== mark)
        this.logger.warn(
          `[game-delivery] ${gameId}: в источнике другая сборка (${mark}), её SHA-256 не закреплён — ` +
            "обновление не предлагается до выпуска HLSR с этой сборкой"
        );
      this.warned = mark;
      return null;
    }

    const version = origin.baseline.version;
    const sha256 = manifest.baseArchive.sha256;

    return {
      source: "origin",
      version,
      updateRequired: false,
      minimumLauncherVersion: null,
      publishedAt: stamp.lastModified ? new Date(stamp.lastModified).toISOString() : null,
      expectedInstalledSize: null,
      patches: [],
      fullArchive: {
        version,
        filename: `${gameId}-${version}.7z`,
        size: stamp.size,
        sha256,
        rootPrefix: manifest.baseArchive.rootPrefix || null,
        providers: [this.providerOf(gameId, manifest)],
      },
      repairMirror: null,
      filesManifest: null,
      buildMetadata: null,
      notes: null,
      stamp,
    };
  }
}

module.exports = OriginUpdates;
module.exports.sameStamp = sameStamp;
module.exports.ORIGINS = ORIGINS;
