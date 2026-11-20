"use strict";

const BaseProvider = require("./BaseProvider");
const httpClient = require("../http-client");
const { DeliveryError } = require("../errors");

const API_BASE = "https://cloud-api.yandex.net/v1/disk/public/resources/download";
const PUBLIC_URL_RE = /^https:\/\/disk\.yandex\.(ru|com)\/d\/[A-Za-z0-9_-]+$/;

class YandexPublicProvider extends BaseProvider {
  constructor(descriptor, options) {
    super(descriptor, options);

    if (!PUBLIC_URL_RE.test(String(descriptor.publicUrl || "")))
      throw new DeliveryError(
        "PROVIDER_UNSUPPORTED",
        `Некорректная публичная ссылка Яндекс.Диска: ${descriptor.publicUrl}`
      );

    this.publicUrl = descriptor.publicUrl;
    this.apiBase = (options && options.apiBase) || API_BASE;
    this.httpClient = (options && options.httpClient) || httpClient;
  }

  buildApiUrl() {
    return `${this.apiBase}?public_key=${encodeURIComponent(this.publicUrl)}`;
  }

  async resolve(options = {}) {
    const payload = await this.httpClient.getJson(this.buildApiUrl(), {
      timeout: (this.options && this.options.timeout) || undefined,
      headers: { accept: "application/json" },
      signal: options.signal,
    });

    if (!payload || typeof payload.href !== "string" || !payload.href)
      throw new DeliveryError("PROVIDER_LINK_FAILED", "Яндекс.Диск не вернул ссылку на файл");

    if (payload.templated)
      throw new DeliveryError(
        "PROVIDER_LINK_FAILED",
        "Яндекс.Диск вернул шаблонную ссылку — такой ответ не поддерживается"
      );

    if (!/^https:\/\//i.test(payload.href))
      throw new DeliveryError(
        "PROVIDER_LINK_FAILED",
        `Яндекс.Диск вернул ссылку не по https: ${payload.href}`
      );

    return { url: payload.href, headers: {}, temporary: true };
  }
}

module.exports = YandexPublicProvider;
module.exports.API_BASE = API_BASE;
