"use strict";

const FLAG = "GAME_DELIVERY_REMOTE_ENABLED";
const LOCAL_HTTP_FLAG = "GAME_DELIVERY_ALLOW_LOCAL_HTTP";

function isTruthy(value) {
  return value === "true" || value === "1";
}

function isReleaseProfileActive() {
  const channelConfig = require("./channel-config");
  return channelConfig.hasActiveReleaseProfile();
}

function isRemoteDeliveryEnabled(env) {
  return isTruthy((env || process.env)[FLAG]) || isReleaseProfileActive();
}

function isLocalHttpAllowed(env) {
  const channelConfig = require("./channel-config");
  if (channelConfig.isProductionBuild()) return false;

  return isTruthy((env || process.env)[LOCAL_HTTP_FLAG]);
}

function allowedProviderTypes(env) {
  const types = [];
  if (isRemoteDeliveryEnabled(env)) types.push("yandex-public", "https-direct");
  if (isLocalHttpAllowed(env)) types.push("local-http");
  return types;
}

function describe(env) {
  const source = env || process.env;
  return {
    flag: FLAG,
    remoteEnabled: isRemoteDeliveryEnabled(source),
    releaseProfileActive: isReleaseProfileActive(),
    localHttpAllowed: isLocalHttpAllowed(source),
    allowedProviderTypes: allowedProviderTypes(source),
  };
}

module.exports = {
  FLAG,
  LOCAL_HTTP_FLAG,
  isRemoteDeliveryEnabled,
  isReleaseProfileActive,
  isLocalHttpAllowed,
  allowedProviderTypes,
  describe,
};
