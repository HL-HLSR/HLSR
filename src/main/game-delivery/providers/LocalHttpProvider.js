"use strict";

const BaseProvider = require("./BaseProvider");
const httpClient = require("../http-client");
const { DeliveryError } = require("../errors");
const { URL } = require("url");

class LocalHttpProvider extends BaseProvider {
  constructor(descriptor, options) {
    super(descriptor, options);

    let parsed;
    try {
      parsed = new URL(String(descriptor.url));
    } catch (e) {
      throw new DeliveryError("PROVIDER_UNSUPPORTED", `Некорректный URL: ${descriptor.url}`);
    }

    if (parsed.protocol !== "http:" || !httpClient.isLoopback(parsed.hostname))
      throw new DeliveryError(
        "PROVIDER_UNSUPPORTED",
        `local-http допускает только loopback: ${descriptor.url}`
      );

    this.url = parsed.href;
  }

  async resolve() {
    return { url: this.url, headers: {}, temporary: false };
  }

  async resolveSibling(filename) {
    if (/[\\/]/.test(String(filename)))
      throw new DeliveryError("PATH_INVALID", `Недопустимое имя файла: ${filename}`);

    const url = new URL(this.url);
    url.pathname = url.pathname.replace(/[^/]+$/, encodeURIComponent(filename));

    return { url: url.href, headers: {} };
  }
}

module.exports = LocalHttpProvider;
