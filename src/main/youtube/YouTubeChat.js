"use strict";

const constants = require("./constants");

function pick(html, re) {
  const m = re.exec(html);
  return m ? m[1] : null;
}

function readContinuation(node) {
  if (!node) return null;
  const list = node.continuations;
  if (!Array.isArray(list) || !list.length) return null;

  const c = list[0];
  const data =
    c.invalidationContinuationData ||
    c.timedContinuationData ||
    c.reloadContinuationData ||
    c.liveChatReplayContinuationData;
  if (!data || typeof data.continuation !== "string") return null;

  return { token: data.continuation, timeoutMs: data.timeoutMs };
}

const seenUnknownBadge = new Set();

function readAuthorRoles(m, log) {
  const roles = { owner: false, moderator: false, verified: false, member: false, memberUrl: null };
  const badges = Array.isArray(m && m.authorBadges) ? m.authorBadges : [];

  for (const badge of badges) {
    const r = badge && badge.liveChatAuthorBadgeRenderer;
    if (!r) continue;

    const iconType = r.icon && typeof r.icon.iconType === "string" ? r.icon.iconType : null;
    if (iconType === "OWNER") {
      roles.owner = true;
    } else if (iconType === "MODERATOR") {
      roles.moderator = true;
    } else if (iconType === "VERIFIED") {
      roles.verified = true;
    } else if (r.customThumbnail) {
      roles.member = true;
      const thumbs = Array.isArray(r.customThumbnail.thumbnails) ? r.customThumbnail.thumbnails : [];
      const best = thumbs.length ? thumbs[thumbs.length - 1] : null;
      if (best && typeof best.url === "string") roles.memberUrl = best.url;
    } else {
      const key = `${iconType || "?"}:${Object.keys(r).join(",")}`;
      if (!seenUnknownBadge.has(key)) {
        seenUnknownBadge.add(key);
        if (log && typeof log.warn === "function") {
          log.warn(`[youtube] незнакомый значок: iconType=${iconType || "нет"} tooltip=${r.tooltip || "нет"} поля=${key}`);
        }
      }
    }
  }

  return roles;
}

function readMessage(action, log) {
  const item = action && action.addChatItemAction && action.addChatItemAction.item;
  const m = item && item.liveChatTextMessageRenderer;
  if (!m) return null;

  const id = typeof m.id === "string" ? m.id : null;
  const nick = m.authorName && typeof m.authorName.simpleText === "string" ? m.authorName.simpleText : "";
  const runs = m.message && Array.isArray(m.message.runs) ? m.message.runs : [];

  const emoji = [];
  const text = runs
    .map((r) => {
      if (typeof r.text === "string") return r.text;
      if (!r.emoji) return "";

      const shortcut = Array.isArray(r.emoji.shortcuts) && r.emoji.shortcuts.length ? r.emoji.shortcuts[0] : "";
      if (!shortcut) return "";

      const thumbs = r.emoji.image && Array.isArray(r.emoji.image.thumbnails) ? r.emoji.image.thumbnails : [];
      const best = thumbs.length ? thumbs[thumbs.length - 1] : null;
      if (best && typeof best.url === "string") emoji.push({ shortcut, url: best.url });

      return ` ${shortcut} `;
    })
    .join("")
    .replace(/ {2,}/g, " ")
    .trim();

  if (!nick || !text) return null;
  return { id, nick, text, emoji, roles: readAuthorRoles(m, log) };
}

class YouTubeChat {
  constructor(options) {
    const opts = options || {};

    this.fetchText = opts.fetchText;
    this.postJson = opts.postJson;
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.setTimer = opts.setTimeout || setTimeout;
    this.clearTimer = opts.clearTimeout || clearTimeout;

    this.videoId = null;
    this.apiKey = null;
    this.clientVersion = null;
    this.token = null;
    this.timer = null;
    this.running = false;
    this.failures = 0;

    this.generation = 0;

    this.seen = new Set();

    this.onLine = null;
  }

  async start(videoIdOrUrl, onLine) {
    const videoId = constants.parseVideoId(videoIdOrUrl);
    if (!videoId) {
      this.log.warn(`${constants.LOG_PREFIX} BAD_VIDEO_ID: ${String(videoIdOrUrl).slice(0, 60)}`);
      return false;
    }

    this.stop();
    const generation = ++this.generation;

    this.videoId = videoId;
    this.onLine = typeof onLine === "function" ? onLine : null;
    this.running = true;
    this.failures = 0;
    this.seen.clear();

    const ok = await this.openChat();

    if (generation !== this.generation) return false;

    if (!ok) {
      this.stop();
      return false;
    }

    this.log.info(`${constants.LOG_PREFIX} WATCHING: ${videoId}`);
    this.schedule(constants.MIN_POLL_MS);
    return true;
  }

  stop() {
    if (this.timer) {
      this.clearTimer(this.timer);
      this.timer = null;
    }
    if (this.running) this.log.info(`${constants.LOG_PREFIX} STOPPED: ${this.videoId}`);
    this.running = false;
    this.videoId = null;
    this.apiKey = null;
    this.clientVersion = null;
    this.token = null;
    this.onLine = null;
    this.seen.clear();
  }

  isRunning() {
    return this.running;
  }

  async openChat() {
    const html = await this.fetchText(constants.liveChatUrl(this.videoId));
    if (!html) {
      this.log.warn(`${constants.LOG_PREFIX} PAGE_FAILED: ${this.videoId}`);
      return false;
    }

    if (html.length < 4096 && !/INNERTUBE_API_KEY/.test(html)) {
      this.log.warn(`${constants.LOG_PREFIX} PAGE_STUB: YouTube отдал заглушку вместо чата`);
      return false;
    }

    this.apiKey = pick(html, /"INNERTUBE_API_KEY":"([^"]+)"/);
    this.clientVersion = pick(html, /"clientVersion":"([\d.]+)"/);
    const token = pick(html, /"continuation":"([^"]+)"/);

    if (!this.apiKey || !token) {
      this.log.warn(`${constants.LOG_PREFIX} NO_LIVE_CHAT: ${this.videoId}`);
      return false;
    }

    this.token = token;
    return true;
  }

  schedule(ms) {
    if (!this.running) return;
    const wait = Math.min(constants.MAX_POLL_MS, Math.max(constants.MIN_POLL_MS, ms || constants.DEFAULT_POLL_MS));
    this.timer = this.setTimer(() => {
      this.timer = null;
      this.poll();
    }, wait);
  }

  async poll() {
    if (!this.running || !this.token) return;

    const body = {
      context: { client: { clientName: "WEB", clientVersion: this.clientVersion || "2.20240101.00.00" } },
      continuation: this.token,
    };

    const generation = this.generation;
    const json = await this.postJson(constants.pollUrl(this.apiKey), body);

    if (!this.running || generation !== this.generation) return;

    if (!json) {
      this.failures++;
      if (this.failures >= constants.MAX_CONSECUTIVE_FAILURES) {
        this.log.warn(`${constants.LOG_PREFIX} GIVING_UP: ${this.videoId} (${this.failures} неудач подряд)`);
        this.stop();
        return;
      }
      this.schedule(constants.DEFAULT_POLL_MS);
      return;
    }

    this.failures = 0;

    const chat = json.continuationContents && json.continuationContents.liveChatContinuation;
    const actions = chat && Array.isArray(chat.actions) ? chat.actions : [];

    let delivered = 0;
    for (const action of actions) {
      if (delivered >= constants.MAX_LINES_PER_POLL) break;

      const message = readMessage(action, this.log);
      if (!message) continue;

      if (message.id) {
        if (this.seen.has(message.id)) continue;
        this.seen.add(message.id);
        if (this.seen.size > 512) this.seen = new Set(Array.from(this.seen).slice(-256));
      }

      delivered++;
      if (this.onLine) {
        try {
          this.onLine(message.nick, message.text, message.emoji || [], message.roles || null);
        } catch (error) {
          this.log.warn(`${constants.LOG_PREFIX} SINK_FAILED: ${error.message}`);
        }
      }
    }

    const next = readContinuation(chat);
    if (!next) {
      this.log.info(`${constants.LOG_PREFIX} ENDED: ${this.videoId}`);
      this.stop();
      return;
    }

    this.token = next.token;
    this.schedule(Math.min(next.timeoutMs || constants.DEFAULT_POLL_MS, constants.POLL_CAP_MS));
  }

  diagnostics() {
    return {
      videoId: this.videoId,
      running: this.running,
      failures: this.failures,
      seen: this.seen.size,
    };
  }
}

module.exports = { YouTubeChat, readMessage, readContinuation };
