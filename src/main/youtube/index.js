"use strict";

const https = require("https");

const constants = require("./constants");
const { YouTubeChat } = require("./YouTubeChat");
const { decodePng } = require("../seventv");

function badgeNamesForRoles(roles) {
  if (!roles) return ["", ""];
  const rank = roles.owner ? "yt_owner" : roles.moderator ? "yt_moderator" : "";
  const standing = roles.verified ? "yt_verified" : roles.member ? "yt_member" : "";
  return rank ? [rank, standing] : [standing, ""];
}

const MAX_REDIRECTS = 3;

function allowHost(hostname) {
  const host = String(hostname || "").toLowerCase();
  return host === constants.HOST || constants.IMAGE_HOSTS.indexOf(host) !== -1;
}

function request(url, options) {
  const opts = options || {};
  const limit = opts.maxBytes || constants.MAX_POLL_BYTES;
  const left = opts.redirectsLeft == null ? MAX_REDIRECTS : opts.redirectsLeft;
  const payload = opts.body ? JSON.stringify(opts.body) : null;

  return new Promise((resolve) => {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (error) {
      return resolve(null);
    }

    if (parsed.protocol !== "https:") return resolve(null);
    if (!allowHost(parsed.hostname)) return resolve(null);

    const headers = {
      "user-agent": constants.USER_AGENT,
      "accept-language": "en-US,en;q=0.9",
    };
    if (payload) {
      headers["content-type"] = "application/json";
      headers["content-length"] = Buffer.byteLength(payload);
    }

    const req = https.request(
      {
        hostname: parsed.hostname,
        path: parsed.pathname + parsed.search,
        method: payload ? "POST" : "GET",
        headers,
        timeout: constants.REQUEST_TIMEOUT_MS,
      },
      (response) => {
        const status = response.statusCode;

        if (status >= 301 && status <= 308 && response.headers.location && left > 0) {
          response.resume();
          let next;
          try {
            next = new URL(response.headers.location, url).toString();
          } catch (error) {
            return resolve(null);
          }
          return resolve(request(next, Object.assign({}, opts, { redirectsLeft: left - 1 })));
        }

        if (status !== 200) {
          response.resume();
          return resolve(null);
        }

        const chunks = [];
        let total = 0;
        response.on("data", (chunk) => {
          total += chunk.length;
          if (total > limit) {
            response.destroy();
            return resolve(null);
          }
          chunks.push(chunk);
        });
        response.on("end", () => {
          const buffer = Buffer.concat(chunks);
          resolve(opts.binary ? buffer : buffer.toString("utf8"));
        });
        response.on("error", () => resolve(null));
      }
    );

    req.on("timeout", () => req.destroy());
    req.on("error", () => resolve(null));
    if (payload) req.write(payload);
    req.end();
  });
}

function fetchText(url) {
  return request(url, { maxBytes: constants.MAX_PAGE_BYTES });
}

function fetchBinary(url) {
  return request(url, { maxBytes: constants.MAX_EMOJI_BYTES, binary: true });
}

async function postJson(url, body) {
  const text = await request(url, { body, maxBytes: constants.MAX_POLL_BYTES });
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch (error) {
    return null;
  }
}

class YouTubeChatBridge {
  constructor(options) {
    const opts = options || {};
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.chat = new YouTubeChat({ fetchText, postJson, log: this.log });
    this.bridgeServer = null;

    this.sentEmoji = new Set();
    this.px = constants.DEFAULT_EMOJI_PX;

    this.memberBadge = null;
    this.memberBadgeInFlight = false;
    this.memberBadgeSentForConnection = false;
  }

  attachBridge(bridgeServer) {
    this.bridgeServer = bridgeServer;
  }

  handleMessage(message) {
    if (!message) return;

    if (message.type === "chat.subscribe" && message.platform === "youtube") {
      this.chat
        .start(message.id, (nick, text, emoji, roles) => {
          const badges = badgeNamesForRoles(roles);
          Promise.all([this.pushEmoji(emoji), this.pushMemberBadge(roles && roles.memberUrl)]).then(() => {
            if (this.bridgeServer) {
              this.bridgeServer.sendChatLine("youtube", nick, text, badges[0], badges[1]);
            }
          });
        })
        .catch((error) => {
          this.log.warn(`${constants.LOG_PREFIX} START_FAILED: ${error.message}`);
        });
      return;
    }

    if (message.type === "emote.resolve" && message.px > 0) {
      this.px = message.px;
      return;
    }

    if (message.type === "chat.unsubscribe") {
      this.chat.stop();
      return;
    }
  }

  async pushMemberBadge(url) {
    if (!url || !this.bridgeServer) return;

    if (this.memberBadge) {
      if (this.memberBadge.url !== url) return;
      if (!this.memberBadgeSentForConnection) this.sendMemberBadge();
      return;
    }

    if (this.memberBadgeInFlight) return;
    this.memberBadgeInFlight = true;
    try {
      const png = await this.fetchBinary(url, constants.MAX_EMOJI_BYTES);
      if (!png) return;
      const decoded = decodePng(png, constants.MEMBER_BADGE_PX);
      if (!decoded || !decoded.rgba) return;

      this.memberBadge = {
        url,
        wide: decoded.wide,
        tall: decoded.tall,
        base64: decoded.rgba.toString("base64"),
      };
      this.sendMemberBadge();
    } catch (error) {
      this.log.warn(`${constants.LOG_PREFIX} MEMBER_BADGE_FAILED: ${error.message}`);
    } finally {
      this.memberBadgeInFlight = false;
    }
  }

  sendMemberBadge() {
    const b = this.memberBadge;
    if (!b || !this.bridgeServer || typeof this.bridgeServer.sendBadgeReady !== "function") return;
    if (this.bridgeServer.sendBadgeReady(constants.MEMBER_BADGE_NAME, b.wide, b.tall, b.base64)) {
      this.memberBadgeSentForConnection = true;
    }
  }

  handleReady() {
    this.memberBadgeSentForConnection = false;
    if (this.memberBadge) this.sendMemberBadge();
  }

  async pushEmoji(list) {
    if (!this.bridgeServer || !Array.isArray(list) || !list.length) return;

    for (const item of list) {
      if (!item || !item.shortcut || !item.url) continue;
      if (this.sentEmoji.has(item.shortcut)) continue;

      this.sentEmoji.add(item.shortcut);
      if (this.sentEmoji.size > constants.MAX_EMOJI_CACHED) {
        this.sentEmoji = new Set(Array.from(this.sentEmoji).slice(-constants.MAX_EMOJI_CACHED / 2));
      }

      try {
        const png = await fetchBinary(item.url);
        if (!png) continue;

        const decoded = decodePng(png, this.px);
        if (!decoded) continue;

        this.bridgeServer.sendEmoteReady(
          item.shortcut,
          decoded.wide,
          decoded.tall,
          decoded.rgba.toString("base64")
        );
        this.log.info(`${constants.LOG_PREFIX} EMOJI: ${item.shortcut} ${decoded.wide}x${decoded.tall}`);
      } catch (error) {
        this.log.warn(`${constants.LOG_PREFIX} EMOJI_FAILED: ${item.shortcut}`);
      }
    }
  }

  forget() {
    this.chat.stop();
    this.sentEmoji.clear();
    this.memberBadgeSentForConnection = false;
  }

  diagnostics() {
    return this.chat.diagnostics();
  }
}

module.exports = { YouTubeChatBridge, fetchText, postJson };
