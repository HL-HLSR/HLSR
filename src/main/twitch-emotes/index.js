"use strict";

const https = require("https");

const constants = require("./constants");
const protocol = require("../game-presence/protocol");
const { TwitchEmoteService } = require("./TwitchEmoteService");
const { decodePng, resizeRgba } = require("../seventv");
const { decodeGif, sampleFrames } = require("../seventv/gif");

function allowHost(hostname) {
  return String(hostname || "").toLowerCase() === constants.CDN_HOST;
}

function get(url, options) {
  const opts = options || {};
  const limit = opts.maxBytes || constants.MAX_IMAGE_BYTES;
  const left = opts.redirectsLeft == null ? constants.MAX_REDIRECTS : opts.redirectsLeft;

  return new Promise((resolve) => {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (error) {
      return resolve(null);
    }

    if (parsed.protocol !== "https:") return resolve(null);
    if (!allowHost(parsed.hostname)) return resolve(null);

    const request = https.get(
      url,
      { headers: { "user-agent": "HLSR", accept: opts.accept || "*/*" }, timeout: constants.REQUEST_TIMEOUT_MS },
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
          return resolve(get(next, Object.assign({}, opts, { redirectsLeft: left - 1 })));
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
        response.on("end", () => resolve(Buffer.concat(chunks)));
        response.on("error", () => resolve(null));
      }
    );

    request.on("timeout", () => request.destroy());
    request.on("error", () => resolve(null));
  });
}

class TwitchEmoteBridge {
  constructor(options) {
    const opts = options || {};
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.bridgeServer = null;
    this.service =
      opts.service ||
      new TwitchEmoteService({
        get: opts.get || get,
        decodePng,
        decodeGif,
        sampleFrames,
        resizeRgba,
        log: this.log,
      });
  }

  attachBridge(bridgeServer) {
    this.bridgeServer = bridgeServer;
  }

  handleMessage(message) {
    if (!message || message.type !== "emote.resolve") return;

    const name = String(message.name || "");
    if (!name.startsWith(protocol.EMOTE_NS_TWITCH)) return;

    const id = name.slice(protocol.EMOTE_NS_TWITCH.length);
    if (!protocol.TWITCH_EMOTE_ID_RE.test(id)) {
      this.log.warn(`${constants.LOG_PREFIX} BAD_ID: ${name}`);
      return;
    }

    this.service
      .resolve(id, message.px)
      .then((image) => {
        if (!this.bridgeServer) return;

        if (!image) {
          this.bridgeServer.sendEmoteMiss(name);
        } else if (image.frames) {
          this.bridgeServer.sendEmoteBegin(name, image.wide, image.tall, image.frames.length, image.frameMs);
          for (let i = 0; i < image.frames.length; i++) {
            this.bridgeServer.sendEmoteFrame(name, i, image.frames[i]);
          }
        } else {
          this.bridgeServer.sendEmoteReady(name, image.wide, image.tall, image.rgbaBase64);
        }
      })
      .catch((error) => {
        this.log.warn(`${constants.LOG_PREFIX} RESOLVE_FAILED: ${name}: ${error.message}`);
      });
  }

  forget() {}

  diagnostics() {
    return this.service.diagnostics();
  }
}

module.exports = { TwitchEmoteBridge, TwitchEmoteService, get };
