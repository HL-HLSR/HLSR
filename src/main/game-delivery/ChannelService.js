"use strict";

const httpClient = require("./http-client");
const channelConfig = require("./channel-config");
const signing = require("./manifest-signing");
const { readJsonSafe, writeJsonAtomic } = require("./atomic");
const { DeliveryError, toDeliveryError } = require("./errors");
const { compileSchema, describeErrors } = require("./schema-validator");

const SCHEMA = require("./schemas/channel-manifest.schema.json");
const SUPPORTED_SCHEMA_VERSION = 1;
const MAX_BODY_BYTES = 512 * 1024;

const DEFAULT_TIMEOUT_MS = 15000;
const DEFAULT_ATTEMPTS = 3;
const DEFAULT_BACKOFF_MS = 500;
const FUTURE_SLACK_MS = 24 * 60 * 60 * 1000;

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

class ChannelService {
  constructor(options) {
    const opts = options || {};

    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.env = opts.env || process.env;
    this.timeout = opts.timeout || DEFAULT_TIMEOUT_MS;
    this.maxAttempts = opts.maxAttempts || DEFAULT_ATTEMPTS;
    this.backoffBaseMs = opts.backoffBaseMs == null ? DEFAULT_BACKOFF_MS : opts.backoffBaseMs;
    this.validator = null;

    this.cache = new Map();

    // publishedAt последнего принятого подписанного манифеста по каналу
    // (файл в userData, не в библиотеке игр).
    // Испорченный файл считается пустым и перезаписывается при следующем приёме.
    this.freshnessFile = opts.freshnessFile || null;
    const saved = this.freshnessFile ? readJsonSafe(this.freshnessFile) : null;
    this.freshness = saved && typeof saved === "object" && !Array.isArray(saved) ? saved : {};
  }

  getValidator() {
    if (this.validator) return this.validator;

    this.validator = compileSchema(SCHEMA);
    return this.validator;
  }

  configFor(gameId) {
    return channelConfig.resolve(gameId, this.env);
  }

  cachedFor(gameId) {
    return this.cache.get(gameId) || null;
  }

  async fetch(gameId, options) {
    const opts = options || {};
    const config = this.configFor(gameId);

    if (!config || !config.url)
      throw new DeliveryError(
        "CHANNEL_DISABLED",
        "Удалённый канал обновлений сборки пока не настроен",
        { gameId }
      );

    const cached = this.cachedFor(gameId);
    const headers = {};

    if (cached && !opts.ignoreCache) {
      if (cached.etag) headers["if-none-match"] = cached.etag;
      if (cached.lastModified) headers["if-modified-since"] = cached.lastModified;
    }

    let lastError = null;

    for (let attempt = 1; attempt <= this.maxAttempts; attempt++) {
      try {
        return await this.fetchOnce(gameId, config, headers, cached);
      } catch (error) {
        lastError = toDeliveryError(error, "CHANNEL_UNAVAILABLE");

        if (["MANIFEST_INVALID", "SIGNATURE_INVALID", "CHANNEL_DISABLED"].indexOf(lastError.code) !== -1)
          throw lastError;

        if (attempt === this.maxAttempts) break;

        const delay = this.backoffBaseMs * Math.pow(2, attempt - 1);
        const jitter = Math.floor(Math.random() * (this.backoffBaseMs || 1));

        this.logger.warn(
          `[game-delivery] канал ${gameId}: попытка ${attempt} не удалась (${lastError.code}), ` +
            `повтор через ${delay + jitter} мс`
        );

        await sleep(delay + jitter);
      }
    }

    throw lastError || new DeliveryError("CHANNEL_UNAVAILABLE", "Канал обновлений недоступен");
  }

  async fetchOnce(gameId, config, headers, cached) {
    const response = await httpClient.request(config.url, {
      headers,
      timeout: this.timeout,
    });

    if (response.statusCode === 304) {
      response.resume();

      if (!cached)
        throw new DeliveryError("CHANNEL_UNAVAILABLE", "HTTP 304 без сохранённого ответа");

      this.logger.info(`[game-delivery] канал ${gameId}: HTTP 304, беру ответ из кеша`);
      cached.checkedAt = new Date().toISOString();

      return { status: "not-modified", manifest: cached.manifest, config };
    }

    if (response.statusCode !== 200) {
      response.resume();
      throw new DeliveryError(
        "CHANNEL_UNAVAILABLE",
        `Канал обновлений ответил HTTP ${response.statusCode}`,
        { statusCode: response.statusCode }
      );
    }

    const body = await httpClient.readBody(response, MAX_BODY_BYTES);

    let data;
    try {
      data = JSON.parse(body.toString("utf8"));
    } catch (error) {
      throw new DeliveryError("MANIFEST_INVALID", `Channel manifest не является JSON: ${error.message}`);
    }

    const manifest = this.validate(data, gameId, config);
    this.acceptFresh(gameId, config, manifest);

    const entry = {
      manifest,
      etag: response.headers.etag || null,
      lastModified: response.headers["last-modified"] || null,
      fetchedAt: new Date().toISOString(),
      checkedAt: new Date().toISOString(),
    };

    this.cache.set(gameId, entry);

    this.logger.info(
      `[game-delivery] канал ${gameId}: версия ${manifest.version}, ` +
        `подпись ${config.requireSignature ? "проверена" : "не требуется"}`
    );

    return { status: "fresh", manifest, config };
  }

  validate(data, gameId, config) {
    if (!data || typeof data !== "object" || Array.isArray(data))
      throw new DeliveryError("MANIFEST_INVALID", "Channel manifest должен быть JSON-объектом");

    if (data.schemaVersion !== SUPPORTED_SCHEMA_VERSION)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Неподдерживаемая версия схемы channel manifest: ${data.schemaVersion}`
      );

    const validate = this.getValidator();

    if (!validate(data)) {
      const errors = describeErrors(validate.errors);
      throw new DeliveryError("MANIFEST_INVALID", `Channel manifest не прошёл валидацию: ${errors[0]}`, {
        errors: errors.slice(0, 20),
      });
    }

    if (data.gameId !== gameId)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Channel manifest описывает ${data.gameId}, запрошен ${gameId}`
      );

    if (config.channel && data.channel !== config.channel)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Channel manifest относится к каналу ${data.channel}, ожидался ${config.channel}`
      );

    this.verifySignature(data, config);

    return data;
  }

  // Повтор старого подписанного манифеста не должен молча замораживать или
  // откатывать обновления. publishedAt подписан и ставится в момент подписи
  // (game-channel-sign.js), поэтому принимается только не старее уже
  // принятого. Откат выпуска владельцем — новой подписью прежней версии.
  acceptFresh(gameId, config, manifest) {
    if (!config.requireSignature) return;

    const key = `${gameId}/${config.channel} ${config.url}`;
    const seen = this.freshness[key];

    if (seen && Date.parse(manifest.publishedAt) < Date.parse(seen))
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Канал ${gameId} отдал манифест от ${manifest.publishedAt}, а уже принят от ${seen}: ` +
          "старый манифест не принимаю",
        { publishedAt: manifest.publishedAt, accepted: seen }
      );

    if (seen === manifest.publishedAt) return;

    // Дата больше чем на сутки вперёд — ошибка часов при подписи. Запомнить её —
    // значит не принимать честные выпуски до этой даты: такой манифест
    // принимается, но свежесть не сдвигает (и его повтор тоже).
    if (Date.parse(manifest.publishedAt) > Date.now() + FUTURE_SLACK_MS) {
      // Проверка идёт каждые несколько минут — одной записи на дату хватит.
      if (this.futureWarned !== manifest.publishedAt)
        this.logger.warn(
          `[game-delivery] канал ${gameId}: publishedAt ${manifest.publishedAt} из будущего — свежесть не запоминаю`
        );
      this.futureWarned = manifest.publishedAt;
      return;
    }

    this.freshness[key] = manifest.publishedAt;

    if (!this.freshnessFile) return;
    try {
      writeJsonAtomic(this.freshnessFile, this.freshness);
    } catch (error) {
      this.logger.warn(`[game-delivery] свежесть канала ${gameId} не сохранена: ${error.message}`);
    }
  }

  verifySignature(data, config) {
    if (!config.requireSignature) return { signed: !!data.signature, valid: false, reason: "not-required" };

    try {
      return signing.check(data, { required: true, publicKeys: config.publicKeys });
    } catch (error) {
      throw new DeliveryError(
        "SIGNATURE_INVALID",
        `Подпись channel manifest не принята: ${error.message}`
      );
    }
  }
}

module.exports = ChannelService;
module.exports.SUPPORTED_SCHEMA_VERSION = SUPPORTED_SCHEMA_VERSION;
module.exports.SCHEMA = SCHEMA;
