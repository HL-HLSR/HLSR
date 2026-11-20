"use strict";

const https = require("https");
const { nativeImage } = require("electron");

const constants = require("./constants");
const protocol = require("../game-presence/protocol");
const { SevenTvService } = require("./SevenTvService");
const { decodeGif, sampleFrames } = require("./gif");

const MAX_REDIRECTS = 3;

const MAX_ASPECT = 4;

function allowHost(hostname) {
  const host = String(hostname || "").toLowerCase();
  return host === constants.API_HOST || host === constants.CDN_HOST;
}

function get(url, options) {
  const opts = options || {};
  const limit = opts.maxBytes || constants.MAX_IMAGE_BYTES;
  const left = opts.redirectsLeft == null ? MAX_REDIRECTS : opts.redirectsLeft;

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

function swapRB(src) {
  const out = Buffer.allocUnsafe(src.length);
  for (let i = 0; i < src.length; i += 4) {
    out[i] = src[i + 2];
    out[i + 1] = src[i + 1];
    out[i + 2] = src[i];
    out[i + 3] = src[i + 3];
  }
  return out;
}

function resizeRgba(rgba, wide, tall, outWide, outTall) {
  try {
    if (wide === outWide && tall === outTall) return rgba;
    const image = nativeImage.createFromBitmap(swapRB(rgba), { width: wide, height: tall });
    if (image.isEmpty()) return null;
    const resized = image.resize({ width: outWide, height: outTall, quality: "good" });
    const out = typeof resized.toBitmap === "function" ? resized.toBitmap() : resized.getBitmap();
    if (!out || out.length < outWide * outTall * 4) return null;
    return swapRB(out);
  } catch (error) {
    return null;
  }
}

function decodePng(buffer, px) {
  try {
    let image = nativeImage.createFromBuffer(buffer);
    if (image.isEmpty()) return null;

    const size = image.getSize();
    if (!size.width || !size.height) return null;

    let targetH = px;
    let targetW = Math.max(1, Math.round((size.width * px) / size.height));

    const maxW = px * MAX_ASPECT;
    if (targetW > maxW) {
      targetH = Math.max(1, Math.round((size.height * maxW) / size.width));
      targetW = Math.round(maxW);
    }

    if (targetW !== size.width || targetH !== size.height) {
      image = image.resize({ width: targetW, height: targetH, quality: "good" });
    }

    const out = image.getSize();
    const bgra = typeof image.toBitmap === "function" ? image.toBitmap() : image.getBitmap();
    if (!bgra || bgra.length < out.width * out.height * 4) return null;

    const rgba = Buffer.allocUnsafe(bgra.length);
    for (let i = 0; i < bgra.length; i += 4) {
      rgba[i] = bgra[i + 2];
      rgba[i + 1] = bgra[i + 1];
      rgba[i + 2] = bgra[i];
      rgba[i + 3] = bgra[i + 3];
    }

    return { wide: out.width, tall: out.height, rgba };
  } catch (error) {
    return null;
  }
}

class SevenTvBridge {
  constructor(options) {
    const opts = options || {};
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.service = new SevenTvService({ get, decodePng, resizeRgba, decodeGif, sampleFrames, log: this.log });
    this.bridgeServer = null;
  }

  attachBridge(bridgeServer) {
    this.bridgeServer = bridgeServer;
  }

  handleMessage(message) {
    if (!message) return;

    if (message.type === "emote.set") {
      this.service.setChannel(message.roomId).catch((error) => {
        this.log.warn(`${constants.LOG_PREFIX} SET_FAILED: ${error.message}`);
      });
      return;
    }

    if (message.type === "emote.resolve") {
      const name = message.name;

      if (String(name).startsWith(protocol.EMOTE_NS_TWITCH)) return;

      this.service
        .resolve(name, message.px)
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
          this.log.warn(`${constants.LOG_PREFIX} RESOLVE_FAILED: ${error.message}`);
        });
      return;
    }
  }

  forget() {
    this.service.forget();
  }

  diagnostics() {
    return this.service.diagnostics();
  }
}

module.exports = { SevenTvBridge, get, decodePng, resizeRgba };
