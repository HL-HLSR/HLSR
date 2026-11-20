"use strict";

const CDN_HOST = "static-cdn.jtvnw.net";

const SIZE_TIERS = [
  { scale: "1.0", tall: 28 },
  { scale: "2.0", tall: 56 },
  { scale: "3.0", tall: 84 },
];

const MIN_SCALE = "2.0";

function pickScale(px) {
  const want = px > 0 ? px : EMOTE_PX;
  for (const size of SIZE_TIERS) {
    if (size.tall >= want) return size.scale === "1.0" ? MIN_SCALE : size.scale;
  }
  return SIZE_TIERS[SIZE_TIERS.length - 1].scale;
}

function emoteImageUrl(id, px) {
  return `https://${CDN_HOST}/emoticons/v2/${encodeURIComponent(id)}/default/dark/${pickScale(px)}`;
}

const EMOTE_PX = 32;

module.exports = {
  CDN_HOST,
  SIZE_TIERS,
  MIN_SCALE,
  pickScale,
  emoteImageUrl,
  EMOTE_PX,

  MAX_IMAGE_BYTES: 4 * 1024 * 1024,
  REQUEST_TIMEOUT_MS: 8000,
  MAX_REDIRECTS: 3,

  MAX_CACHED_IMAGES: 256,

  MAX_ANIM_FRAMES: 16,
  MAX_SHEET_WIDE: 2048,
  MAX_ASPECT: 4,

  LOG_PREFIX: "[twitch-emotes]",
};
