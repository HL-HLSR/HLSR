"use strict";

const fs = require("fs");
const path = require("path");
const { decodePng } = require("../seventv");

const BADGE_NAMES = ["broadcaster", "staff", "moderator", "vip", "partner", "subscriber"];

const YT_BADGE_NAMES = ["yt_owner", "yt_moderator", "yt_verified"];

const ALL_BADGE_NAMES = BADGE_NAMES.concat(YT_BADGE_NAMES);

const BADGE_PX = 64;

const MAX_BADGE_LINE_HINT = 64 * 1024;

class TwitchBadgeBridge {
  constructor(opts) {
    const options = opts || {};
    this.log = options.log || { info() {}, warn() {}, error() {} };
    this.electron = options.electron || null;

    this.bridgeServer = null;
    this.sentForConnection = false;

    this.cache = null;
    this.dir = null;
  }

  attachBridge(server) {
    this.bridgeServer = server;
  }

  handleMessage() {}

  handleReady() {
    if (this.sentForConnection) return;
    this.sentForConnection = true;
    this.pushAll();
  }

  forget() {
    this.sentForConnection = false;
  }

  resolveDir() {
    if (this.dir) return this.dir;

    const electron = this.electron || require("electron");
    const app = electron && electron.app;
    const root = app && typeof app.getAppPath === "function" ? app.getAppPath() : null;

    const candidates = root
      ? [path.join(root, "icons", "badges"), path.join(root, "..", "..", "icons", "badges")]
      : [path.join(__dirname, "..", "..", "..", "icons", "badges")];

    for (const candidate of candidates) {
      try {
        if (fs.existsSync(candidate)) {
          this.dir = candidate;
          return candidate;
        }
      } catch (error) {
        /* следующий кандидат */
      }
    }

    this.dir = candidates[0];
    return this.dir;
  }

  load() {
    if (this.cache) return this.cache;

    const dir = this.resolveDir();
    const loaded = [];

    for (const name of ALL_BADGE_NAMES) {
      try {
        const buffer = fs.readFileSync(path.join(dir, `${name}.png`));
        const decoded = decodePng(buffer, BADGE_PX);
        if (!decoded || !decoded.rgba || !decoded.wide || !decoded.tall) {
          this.log.warn(`[twitch-badges] ${name}: не удалось раскодировать`);
          continue;
        }

        const base64 = decoded.rgba.toString("base64");
        if (base64.length + 256 > MAX_BADGE_LINE_HINT) {
          this.log.warn(`[twitch-badges] ${name}: строка не влезает в буфер игры`);
          continue;
        }

        loaded.push({ name, wide: decoded.wide, tall: decoded.tall, base64 });
      } catch (error) {
        this.log.warn(`[twitch-badges] ${name}: ${error.message}`);
      }
    }

    this.cache = loaded;
    return loaded;
  }

  pushAll() {
    const server = this.bridgeServer;
    if (!server || typeof server.sendBadgeReady !== "function") return 0;

    const badges = this.load();
    let sent = 0;

    for (const badge of badges) {
      if (server.sendBadgeReady(badge.name, badge.wide, badge.tall, badge.base64)) sent++;
    }

    if (sent) this.log.info(`[twitch-badges] значков отправлено: ${sent}`);
    return sent;
  }
}

module.exports = { TwitchBadgeBridge, BADGE_NAMES, YT_BADGE_NAMES, ALL_BADGE_NAMES, BADGE_PX };
