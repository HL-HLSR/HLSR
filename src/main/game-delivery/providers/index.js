"use strict";

const YandexPublicProvider = require("./YandexPublicProvider");
const HttpsDirectProvider = require("./HttpsDirectProvider");
const LocalHttpProvider = require("./LocalHttpProvider");
const featureFlag = require("../feature-flag");
const { DeliveryError } = require("../errors");

const REGISTRY = {
  "yandex-public": YandexPublicProvider,
  "https-direct": HttpsDirectProvider,
  "local-http": LocalHttpProvider,
};

function create(descriptor, options) {
  const opts = options || {};
  const allowed = opts.allowedProviderTypes || featureFlag.allowedProviderTypes(opts.env);

  if (allowed.indexOf(descriptor.type) === -1)
    throw new DeliveryError(
      "REMOTE_DISABLED",
      `Провайдер ${descriptor.type} выключен feature flag'ом ${featureFlag.FLAG}`,
      { type: descriptor.type, allowed }
    );

  const Provider = REGISTRY[descriptor.type];
  if (!Provider)
    throw new DeliveryError("PROVIDER_UNSUPPORTED", `Неизвестный тип провайдера: ${descriptor.type}`);

  return new Provider(descriptor, opts);
}

module.exports = { create, REGISTRY };
