"use strict";

const { DeliveryError } = require("../errors");

class BaseProvider {
  constructor(descriptor, options) {
    this.descriptor = descriptor;
    this.options = options || {};
    this.id = descriptor.id;
    this.type = descriptor.type;
    this.status = descriptor.status;
  }

  async resolve() {
    throw new Error("resolve() не реализован");
  }

  shouldRefreshOn(statusCode) {
    return statusCode === 401 || statusCode === 403 || statusCode === 410;
  }

  supportsRange() {
    return true;
  }

  async resolveSibling() {
    throw new DeliveryError(
      "PROVIDER_UNSUPPORTED",
      `Провайдер ${this.type} не умеет отдавать файлы рядом с архивом`
    );
  }

  describe() {
    return { id: this.id, type: this.type, status: this.status };
  }
}

module.exports = BaseProvider;
