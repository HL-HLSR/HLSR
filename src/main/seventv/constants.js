"use strict";

const API_HOST = "7tv.io";
const CDN_HOST = "cdn.7tv.app";

function emoteSetUrl(roomId) {
  return `https://${API_HOST}/v3/users/twitch/${encodeURIComponent(roomId)}`;
}

function emoteSetByIdUrl(setId) {
  return `https://${API_HOST}/v3/emote-sets/${encodeURIComponent(setId)}`;
}

function sevenTvUserUrl(userId) {
  return `https://${API_HOST}/v3/users/${encodeURIComponent(userId)}`;
}

const EMOTE_PX = 32;

const SIZE_TIERS = [
  { tier: 1, tall: 32 },
  { tier: 2, tall: 64 },
  { tier: 3, tall: 96 },
  { tier: 4, tall: 128 },
];

const MIN_SIZE_TIER = 2;

function pickSizeTier(targetPx) {
  const want = targetPx > 0 ? targetPx : EMOTE_PX;
  for (const size of SIZE_TIERS) {
    if (size.tall >= want) return Math.max(size.tier, MIN_SIZE_TIER);
  }
  return SIZE_TIERS[SIZE_TIERS.length - 1].tier;
}

function emoteGifUrl(emoteId, targetPx) {
  const tier = pickSizeTier(targetPx);
  return `https://${CDN_HOST}/emote/${encodeURIComponent(emoteId)}/${tier}x.gif`;
}

function emoteImageUrl(emoteId, animated, targetPx) {
  const tier = pickSizeTier(targetPx);
  const file = animated ? `${tier}x_static.png` : `${tier}x.png`;
  return `https://${CDN_HOST}/emote/${encodeURIComponent(emoteId)}/${file}`;
}

module.exports = {
  API_HOST,
  CDN_HOST,
  SIZE_TIERS,
  pickSizeTier,
  emoteSetUrl,
  emoteSetByIdUrl,
  sevenTvUserUrl,
  emoteImageUrl,
  emoteGifUrl,

  MAX_SET_BYTES: 4 * 1024 * 1024,
  MAX_IMAGE_BYTES: 512 * 1024,
  REQUEST_TIMEOUT_MS: 8000,

  EMOTE_PX,

  MAX_CACHED_IMAGES: 256,

  MAX_ANIM_FRAMES: 16,

  MAX_GIF_BYTES: 4 * 1024 * 1024,

  MAX_ASPECT: 4,

  MAX_SHEET_WIDE: 2048,

  LOG_PREFIX: "[seventv]",
};
