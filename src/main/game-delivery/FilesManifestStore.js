"use strict";

const fs = require("fs");
const path = require("path");

const httpClient = require("./http-client");
const filesManifestLib = require("./files-manifest");
const providers = require("./providers");
const { MirrorRunner } = require("./mirror-runner");
const featureFlag = require("./feature-flag");
const ManifestService = require("./ManifestService");
const { writeFileAtomic, writeJsonAtomic, readJsonSafe } = require("./atomic");
const { assertInside } = require("./paths");
const { DeliveryError, toDeliveryError } = require("./errors");

const META_VERSION = 1;

const MAX_MANIFEST_BYTES = 64 * 1024 * 1024;
const MAX_ENTRIES = filesManifestLib.MAX_ENTRIES;

const DEFAULT_TIMEOUT_MS = 30000;
const DEFAULT_ATTEMPTS = 3;
const DEFAULT_BACKOFF_MS = 500;

const VERSION_PATTERN = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;
const GAME_ID_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/;

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

class FilesManifestStore {
  constructor(options) {
    const opts = options || {};

    this.paths = opts.paths;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.env = opts.env || process.env;
    this.timeout = opts.timeout || DEFAULT_TIMEOUT_MS;
    this.maxAttempts = opts.maxAttempts || DEFAULT_ATTEMPTS;
    this.backoffBaseMs = opts.backoffBaseMs == null ? DEFAULT_BACKOFF_MS : opts.backoffBaseMs;
    this.mirrors = opts.mirrors || new MirrorRunner({ logger: this.logger });
    this.lastMirrorAttempts = [];
  }

  rootDir() {
    return path.join(this.paths.internal, "manifests");
  }

  gameDir(gameId) {
    if (!GAME_ID_PATTERN.test(String(gameId || "")))
      throw new DeliveryError("PATH_INVALID", `Недопустимый gameId: ${gameId}`);

    return assertInside(this.rootDir(), path.join(this.rootDir(), gameId), "Папка манифестов");
  }

  manifestPath(gameId, version) {
    if (!VERSION_PATTERN.test(String(version || "")))
      throw new DeliveryError("PATH_INVALID", `Недопустимая версия: ${version}`);

    const dir = this.gameDir(gameId);
    return assertInside(dir, path.join(dir, `${version}.files.json`), "Файловый манифест");
  }

  metaPath(gameId, version) {
    const dir = this.gameDir(gameId);
    return assertInside(dir, path.join(dir, `${version}.trust.json`), "Metadata манифеста");
  }

  has(gameId, version) {
    try {
      return fs.existsSync(this.manifestPath(gameId, version));
    } catch (error) {
      return false;
    }
  }

  list(gameId) {
    let items;

    try {
      items = fs.readdirSync(this.gameDir(gameId));
    } catch (error) {
      return [];
    }

    return items
      .filter((name) => name.endsWith(".files.json"))
      .map((name) => name.slice(0, -".files.json".length))
      .sort();
  }

  load(gameId, version) {
    let file;
    let metaFile;

    try {
      file = this.manifestPath(gameId, version);
      metaFile = this.metaPath(gameId, version);
    } catch (error) {
      return null;
    }

    if (!fs.existsSync(file)) return null;

    const meta = readJsonSafe(metaFile);

    if (!meta || meta.metaVersion !== META_VERSION) {
      this.reject(gameId, version, "рядом нет metadata доверия");
      return null;
    }

    if (meta.gameId !== gameId || meta.version !== version) {
      this.reject(gameId, version, `metadata описывает ${meta.gameId} ${meta.version}`);
      return null;
    }

    let raw;
    try {
      raw = fs.readFileSync(file);
    } catch (error) {
      this.reject(gameId, version, `файл не читается: ${error.message}`);
      return null;
    }

    if (raw.length > MAX_MANIFEST_BYTES) {
      this.reject(gameId, version, `размер ${raw.length} превышает предел`);
      return null;
    }

    if (meta.size != null && raw.length !== meta.size) {
      this.reject(gameId, version, `размер ${raw.length} вместо ${meta.size}`);
      return null;
    }

    const digest = require("crypto").createHash("sha256").update(raw).digest("hex");

    if (digest !== meta.sha256) {
      this.reject(gameId, version, `SHA-256 ${digest} вместо ${meta.sha256}`);
      return null;
    }

    let data;
    try {
      data = JSON.parse(raw.toString("utf8"));
    } catch (error) {
      this.reject(gameId, version, `не разбирается как JSON: ${error.message}`);
      return null;
    }

    try {
      return filesManifestLib.validate(data, `cache:${gameId}-${version}`, {
        gameId,
        version,
        maxEntries: MAX_ENTRIES,
        requireManifestSha256: true,
      });
    } catch (error) {
      this.reject(gameId, version, toDeliveryError(error, "MANIFEST_INVALID").message);
      return null;
    }
  }

  reject(gameId, version, reason) {
    this.logger.warn(
      `[game-delivery] кешированный файловый манифест ${gameId} ${version} отклонён: ${reason}`
    );

    for (const file of [this.manifestPath(gameId, version), this.metaPath(gameId, version)]) {
      try {
        if (fs.existsSync(file)) fs.rmSync(file, { force: true });
      } catch (error) {
        this.logger.warn(`[game-delivery] не удалось убрать ${file}: ${error.message}`);
      }
    }
  }

  store(gameId, version, raw, meta) {
    const dir = this.gameDir(gameId);
    fs.mkdirSync(dir, { recursive: true });

    const file = this.manifestPath(gameId, version);
    writeFileAtomic(file, raw);

    writeJsonAtomic(
      this.metaPath(gameId, version),
      Object.assign(
        {
          metaVersion: META_VERSION,
          gameId,
          version,
          size: raw.length,
          storedAt: new Date().toISOString(),
        },
        meta || {}
      )
    );

    return file;
  }

  async fetch(gameId, descriptor, options) {
    const opts = options || {};

    this.assertDescriptor(gameId, descriptor);

    const allowedProviderTypes = featureFlag.allowedProviderTypes(this.env);
    const usable = (descriptor.providers || []).filter(
      (provider) => allowedProviderTypes.indexOf(provider.type) !== -1
    );

    if (!usable.length)
      throw new DeliveryError(
        "REMOTE_DISABLED",
        "Источник файлового манифеста выключен feature flag'ом",
        { gameId, version: descriptor.version }
      );

    const outcome = await this.mirrors.run({
      artifact: `${gameId}:files-manifest:${descriptor.version}`,
      descriptors: ManifestService.orderProviders(usable),
      operation: async ({ descriptor: entry }) => {
        const provider = providers.create(entry, {
          env: this.env,
          allowedProviderTypes,
        });

        const source = { id: entry.id, type: entry.type };
        let lastError = null;

        for (let attempt = 1; attempt <= this.maxAttempts; attempt++) {
          try {
            const raw = await this.download(provider, descriptor);
            return this.accept(gameId, descriptor, raw, Object.assign({ source }, opts));
          } catch (error) {
            lastError = toDeliveryError(error, "MANIFEST_NOT_FOUND");

            if (
              ["MANIFEST_INVALID", "CHECKSUM_MISMATCH", "SIZE_MISMATCH", "ARCHIVE_UNSAFE_ENTRY"].indexOf(
                lastError.code
              ) !== -1
            )
              throw lastError;

            if (attempt === this.maxAttempts) break;

            const delay = this.backoffBaseMs * Math.pow(2, attempt - 1);
            const jitter = Math.floor(Math.random() * (this.backoffBaseMs || 1));

            this.logger.warn(
              `[game-delivery] файловый манифест ${gameId} ${descriptor.version}: попытка ` +
                `${attempt} не удалась (${lastError.code}), повтор через ${delay + jitter} мс`
            );

            await sleep(delay + jitter);
          }
        }

        throw lastError || new DeliveryError("MANIFEST_NOT_FOUND", "Файловый манифест недоступен");
      },
    });

    this.lastMirrorAttempts = outcome.attempts;
    return outcome.value;
  }

  assertDescriptor(gameId, descriptor) {
    if (!descriptor)
      throw new DeliveryError(
        "MANIFEST_NOT_FOUND",
        `Канал не объявляет файловый манифест для ${gameId}`
      );

    if (!VERSION_PATTERN.test(String(descriptor.version || "")))
      throw new DeliveryError("MANIFEST_INVALID", `Недопустимая версия: ${descriptor.version}`);

    if (!/^[a-f0-9]{64}$/.test(String(descriptor.sha256 || "")))
      throw new DeliveryError("MANIFEST_INVALID", "В описании манифеста нет корректного SHA-256");

    if (!Number.isInteger(descriptor.size) || descriptor.size <= 0)
      throw new DeliveryError("MANIFEST_INVALID", "В описании манифеста нет корректного размера");

    if (descriptor.size > MAX_MANIFEST_BYTES)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Заявленный размер манифеста ${descriptor.size} превышает предел ${MAX_MANIFEST_BYTES}`
      );

    const filename = String(descriptor.filename || "");

    if (!filename || /[\\/]/.test(filename) || filename.indexOf("..") !== -1)
      throw new DeliveryError("PATH_INVALID", `Недопустимое имя файла манифеста: ${filename}`);

    if (!/^[^:*?"<>|]+\.json$/.test(filename))
      throw new DeliveryError("PATH_INVALID", `Недопустимое имя файла манифеста: ${filename}`);
  }

  async download(provider, descriptor) {
    const sibling = await provider.resolveSibling(descriptor.filename);

    const url = new (require("url").URL)(sibling.url);

    if (url.protocol !== "https:" && !(url.protocol === "http:" && httpClient.isLoopback(url.hostname)))
      throw new DeliveryError(
        "INSECURE_REDIRECT",
        `Файловый манифест допускается только по HTTPS (или локальному HTTP): ${url.protocol}`
      );

    const response = await httpClient.request(sibling.url, {
      headers: sibling.headers,
      timeout: this.timeout,
    });

    if (response.statusCode !== 200) {
      response.resume();
      throw new DeliveryError(
        "MANIFEST_NOT_FOUND",
        `Файловый манифест недоступен: HTTP ${response.statusCode}`,
        { statusCode: response.statusCode }
      );
    }

    return httpClient.readBody(response, descriptor.size);
  }

  accept(gameId, descriptor, raw, options) {
    const opts = options || {};

    if (raw.length !== descriptor.size)
      throw new DeliveryError(
        "SIZE_MISMATCH",
        `Размер файлового манифеста ${raw.length}, ожидался ${descriptor.size}`
      );

    const digest = require("crypto").createHash("sha256").update(raw).digest("hex");

    if (digest !== descriptor.sha256)
      throw new DeliveryError("CHECKSUM_MISMATCH", "SHA-256 файлового манифеста не совпал", {
        expected: descriptor.sha256,
        actual: digest,
      });

    let data;
    try {
      data = JSON.parse(raw.toString("utf8"));
    } catch (error) {
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Файловый манифест не является JSON: ${error.message}`
      );
    }

    const manifest = filesManifestLib.validate(data, `channel:${gameId}`, {
      gameId,
      version: descriptor.version,
      maxEntries: MAX_ENTRIES,
      requireManifestSha256: true,
    });

    const file = this.store(gameId, descriptor.version, raw, {
      sha256: digest,
      source: opts.source || "channel",
      channelSha256: opts.channelSha256 || null,
      totalFiles: manifest.totalFiles,
      manifestSha256: manifest.manifestSha256,
    });

    this.logger.info(
      `[game-delivery] файловый манифест ${gameId} ${descriptor.version} проверен и сохранён: ` +
        `${manifest.totalFiles} файлов, ${raw.length} Б, ${file}`
    );

    return manifest;
  }
}

module.exports = FilesManifestStore;
module.exports.MAX_MANIFEST_BYTES = MAX_MANIFEST_BYTES;
module.exports.META_VERSION = META_VERSION;
