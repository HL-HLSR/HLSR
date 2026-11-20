"use strict";

const HOST = "www.youtube.com";

const IMAGE_HOSTS = ["yt3.ggpht.com", "fonts.gstatic.com"];

const USER_AGENT =
  (process.platform === "linux"
    ? "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
    : "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) ") +
  "Chrome/131.0.0.0 Safari/537.36";

function liveChatUrl(videoId) {
  return `https://${HOST}/live_chat?is_popout=1&v=${encodeURIComponent(videoId)}`;
}

function pollUrl(apiKey) {
  return `https://${HOST}/youtubei/v1/live_chat/get_live_chat?key=${encodeURIComponent(apiKey)}`;
}

const VIDEO_ID_RE = /^[A-Za-z0-9_-]{5,32}$/;

function parseVideoId(input) {
  const raw = String(input || "").trim();
  if (VIDEO_ID_RE.test(raw)) return raw;

  const byQuery = /[?&]v=([A-Za-z0-9_-]{5,32})/.exec(raw);
  if (byQuery) return byQuery[1];

  const byShort = /youtu\.be\/([A-Za-z0-9_-]{5,32})/.exec(raw);
  if (byShort) return byShort[1];

  const byLive = /youtube\.com\/live\/([A-Za-z0-9_-]{5,32})/.exec(raw);
  if (byLive) return byLive[1];

  return null;
}

module.exports = {
  HOST,
  IMAGE_HOSTS,
  USER_AGENT,
  liveChatUrl,
  pollUrl,
  parseVideoId,
  VIDEO_ID_RE,

  MAX_PAGE_BYTES: 4 * 1024 * 1024,
  MAX_POLL_BYTES: 2 * 1024 * 1024,
  REQUEST_TIMEOUT_MS: 10000,

  DEFAULT_POLL_MS: 1000,
  MIN_POLL_MS: 1000,
  MAX_POLL_MS: 30000,
  POLL_CAP_MS: 1000,

  MAX_LINES_PER_POLL: 8,

  MAX_CONSECUTIVE_FAILURES: 5,

  MAX_EMOJI_BYTES: 256 * 1024,
  DEFAULT_EMOJI_PX: 36,

  MEMBER_BADGE_PX: 32,
  MEMBER_BADGE_NAME: "yt_member",
  MAX_EMOJI_CACHED: 256,

  LOG_PREFIX: "[youtube]",
};
