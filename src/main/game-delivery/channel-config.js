"use strict";

const featureFlag = require("./feature-flag");
const endpoints = require("../../shared/delivery-endpoints");

const QA_URL_ENV = "HLSR_GAME_DELIVERY_CHANNEL_URL";
const QA_KEY_ENV = "HLSR_GAME_DELIVERY_CHANNEL_KEY";
const QA_CHANNEL_ENV = "HLSR_GAME_DELIVERY_CHANNEL_NAME";

const QA_ALLOWED_HTTPS_HOSTS = ["hlsr.pro"];
const QA_LOCAL_HOSTS = ["127.0.0.1", "localhost"];
const QA_CHANNEL_NAMES = ["stable", "beta", "test"];

function qaUrlKind(url, env) {
  let parsed;

  try {
    parsed = new URL(url);
  } catch (e) {
    return null;
  }

  if (parsed.protocol === "https:")
    return QA_ALLOWED_HTTPS_HOSTS.indexOf(parsed.hostname) !== -1 ? "https" : null;

  if (parsed.protocol === "http:")
    return featureFlag.isLocalHttpAllowed(env) && QA_LOCAL_HOSTS.indexOf(parsed.hostname) !== -1
      ? "local-http"
      : null;

  return null;
}

const GHOSTING_STABLE_PUBLIC_KEY =
  "-----BEGIN PUBLIC KEY-----\n" +
  "MCowBQYDK2VwAyEAhx8U4Ih2sh7K2JgtHVQiqEzYaoRTsL/KEBEYiPYX854=\n" +
  "-----END PUBLIC KEY-----\n";

const CHANNELS = {
  "ghosting-renewed": {
    url: endpoints.game.channelUrl("ghosting-renewed"),
    channel: "stable",
    requireSignature: true,
    publicKeys: [GHOSTING_STABLE_PUBLIC_KEY],
    keyIds: ["7ccb676761a3ed1c"],
    releaseProfile: { enabled: true },
  },

  "hl2-4260": {
    url: endpoints.game.channelUrl("hl2-4260"),
    channel: "stable",
    requireSignature: true,
    publicKeys: [GHOSTING_STABLE_PUBLIC_KEY],
    keyIds: ["7ccb676761a3ed1c"],
    releaseProfile: { enabled: true },
  },
  "hl2-2707": {
    url: endpoints.game.channelUrl("hl2-2707"),
    channel: "stable",
    requireSignature: true,
    publicKeys: [GHOSTING_STABLE_PUBLIC_KEY],
    keyIds: ["7ccb676761a3ed1c"],
    releaseProfile: { enabled: true },
  },
};

function isProductionBuild() {
  return process.env.NODE_ENV === "production";
}

function hasActiveReleaseProfile() {
  if (!isProductionBuild()) return false;

  return Object.keys(CHANNELS).some((gameId) => {
    const base = CHANNELS[gameId];
    return !!(
      base.releaseProfile &&
      base.releaseProfile.enabled &&
      base.url &&
      base.publicKeys.length
    );
  });
}

function resolve(gameId, env) {
  const environment = env || process.env;
  const base = CHANNELS[gameId] || null;

  if (!base) return null;

  const builtinActive = !!(
    base.url &&
    base.releaseProfile &&
    base.releaseProfile.enabled &&
    isProductionBuild() &&
    base.publicKeys.length
  );

  const config = {
    gameId,
    url: builtinActive ? base.url : null,
    channel: base.channel,
    requireSignature: base.requireSignature,
    publicKeys: base.publicKeys.slice(),
    source: builtinActive ? "builtin" : null,
  };

  const qaUrl = environment[QA_URL_ENV];
  const qaKind = qaUrl && !isProductionBuild() ? qaUrlKind(qaUrl, environment) : null;

  if (qaKind) {
    config.url = qaUrl;
    config.source = "qa";

    const qaChannel = environment[QA_CHANNEL_ENV];
    if (QA_CHANNEL_NAMES.indexOf(qaChannel) !== -1) config.channel = qaChannel;

    const qaKey = environment[QA_KEY_ENV];
    if (qaKey) config.publicKeys = [qaKey.replace(/\\n/g, "\n")];
  }

  return config;
}

function describe(gameId, env) {
  const config = resolve(gameId, env);

  return {
    gameId,
    configured: !!(config && config.url),
    source: config ? config.source : null,
    requireSignature: config ? config.requireSignature : true,
    trustedKeys: config ? config.publicKeys.length : 0,
  };
}

module.exports = {
  CHANNELS,
  QA_URL_ENV,
  QA_KEY_ENV,
  QA_CHANNEL_ENV,
  QA_ALLOWED_HTTPS_HOSTS,
  qaUrlKind,
  resolve,
  describe,
  isProductionBuild,
  hasActiveReleaseProfile,
};
