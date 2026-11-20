"use strict";

const constants = require("./constants");

class TwitchEmoteService {
  constructor(deps) {
    const d = deps || {};
    this.get = d.get;
    this.decodePng = d.decodePng;
    this.decodeGif = d.decodeGif;
    this.sampleFrames = d.sampleFrames;
    this.resizeRgba = d.resizeRgba;
    this.log = d.log || { info() {}, warn() {}, error() {} };

    this.cache = new Map();
  }

  remember(key, value) {
    if (this.cache.size >= constants.MAX_CACHED_IMAGES) {
      const oldest = this.cache.keys().next();
      if (!oldest.done) this.cache.delete(oldest.value);
    }
    this.cache.set(key, value);
  }

  async resolve(id, px) {
    const key = `${id}@${px > 0 ? px : constants.EMOTE_PX}`;
    if (this.cache.has(key)) return this.cache.get(key);

    const buf = await this.get(constants.emoteImageUrl(id, px), {
      maxBytes: constants.MAX_IMAGE_BYTES,
      accept: "image/*",
    });
    if (!buf) {
      this.log.warn(`${constants.LOG_PREFIX} FETCH_FAILED: ${id}`);
      return null;
    }

    const target = px > 0 ? px : constants.EMOTE_PX;

    const gif = this.decodeGif ? this.decodeGif(buf) : null;
    if (gif && gif.frames && gif.frames.length) {
      const animated = this.buildAnimated(id, gif, target);
      if (animated) {
        this.remember(key, animated);
        return animated;
      }

      const still = this.buildStillFromGif(gif, target);
      if (still) {
        this.remember(key, still);
        return still;
      }
      return null;
    }

    const decoded = this.decodePng(buf, target);
    if (!decoded) {
      this.log.warn(`${constants.LOG_PREFIX} DECODE_FAILED: ${id}`);
      return null;
    }

    const image = {
      wide: decoded.wide,
      tall: decoded.tall,
      rgbaBase64: decoded.rgba.toString("base64"),
    };
    this.remember(key, image);
    return image;
  }

  targetSize(wide, tall, target) {
    let outTall = target;
    let outWide = Math.max(1, Math.round((wide * target) / tall));
    const maxW = target * constants.MAX_ASPECT;
    if (outWide > maxW) {
      outTall = Math.max(1, Math.round((tall * maxW) / wide));
      outWide = Math.round(maxW);
    }
    return { outWide, outTall };
  }

  buildAnimated(id, decoded, target) {
    if (!this.sampleFrames || !this.resizeRgba) return null;

    const { outWide, outTall } = this.targetSize(decoded.wide, decoded.tall, target);

    const byWidth = Math.max(2, Math.floor(constants.MAX_SHEET_WIDE / outWide));
    const picked = this.sampleFrames(decoded, Math.min(constants.MAX_ANIM_FRAMES, byWidth));
    if (!picked || picked.frames.length < 2) return null;

    const out = [];
    for (const frame of picked.frames) {
      const scaled = this.resizeRgba(frame, decoded.wide, decoded.tall, outWide, outTall);
      if (!scaled) return null;
      out.push(scaled.toString("base64"));
    }

    this.log.info(
      `${constants.LOG_PREFIX} ANIMATED: ${id} ${outWide}x${outTall}, ` +
        `${out.length} из ${decoded.frames.length} кадров, ${picked.intervalMs}мс`
    );

    return { wide: outWide, tall: outTall, frames: out, frameMs: picked.intervalMs };
  }

  buildStillFromGif(decoded, target) {
    if (!this.resizeRgba || !decoded.frames.length) return null;

    const { outWide, outTall } = this.targetSize(decoded.wide, decoded.tall, target);
    const scaled = this.resizeRgba(decoded.frames[0], decoded.wide, decoded.tall, outWide, outTall);
    if (!scaled) return null;

    return { wide: outWide, tall: outTall, rgbaBase64: scaled.toString("base64") };
  }

  diagnostics() {
    return { cached: this.cache.size };
  }
}

module.exports = { TwitchEmoteService };
