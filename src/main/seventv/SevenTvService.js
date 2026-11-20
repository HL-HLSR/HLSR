"use strict";

const constants = require("./constants");

class SevenTvService {
  constructor(options) {
    const opts = options || {};

    this.get = opts.get;
    this.decodePng = opts.decodePng;
    this.resizeRgba = opts.resizeRgba || null;
    this.decodeGif = opts.decodeGif || null;
    this.sampleFrames = opts.sampleFrames || null;
    this.log = opts.log || { info() {}, warn() {}, error() {} };

    this.roomId = null;
    this.byName = new Map();
    this.cache = new Map();
    this.setLoaded = false;
    this.setPromise = null;
  }

  setChannel(source) {
    const roomId = source ? String(source) : null;

    if (this.roomId === roomId && (this.setLoaded || this.setPromise)) {
      return this.setPromise || Promise.resolve(true);
    }

    this.roomId = roomId;
    this.byName.clear();
    this.cache.clear();
    this.setLoaded = false;

    if (!roomId) {
      this.setPromise = null;
      return Promise.resolve(false);
    }

    this.setPromise = this.loadSet(roomId);
    return this.setPromise;
  }

  parseSource(source) {
    const raw = String(source || "");
    const colon = raw.indexOf(":");
    const kind = colon > 0 ? raw.slice(0, colon) : "twitch";
    const id = colon > 0 ? raw.slice(colon + 1) : raw;

    if (!id) return null;
    if (kind === "set") return { kind, url: constants.emoteSetByIdUrl(id), id };
    if (kind === "user") return { kind, url: constants.sevenTvUserUrl(id), id };
    if (kind === "twitch") return { kind, url: constants.emoteSetUrl(id), id };
    return null;
  }

  emotesFromJson(json) {
    if (!json) return null;
    if (Array.isArray(json.emotes)) return json.emotes;
    if (json.emote_set && Array.isArray(json.emote_set.emotes)) return json.emote_set.emotes;
    return null;
  }

  async fetchJson(url, what) {
    const body = await this.get(url, {
      maxBytes: constants.MAX_SET_BYTES,
      accept: "application/json",
    });
    if (!body) {
      this.log.warn(`${constants.LOG_PREFIX} SET_FETCH_FAILED: ${what}`);
      return null;
    }
    try {
      return JSON.parse(body.toString("utf8"));
    } catch (error) {
      this.log.warn(`${constants.LOG_PREFIX} SET_NOT_JSON: ${what}`);
      return null;
    }
  }

  async loadSet(roomId) {
    const source = this.parseSource(roomId);
    if (!source) {
      this.forgetChannel();
      return false;
    }

    let json = await this.fetchJson(source.url, roomId);
    if (!json) {
      this.forgetChannel();
      return false;
    }

    if (source.kind === "user") {
      const link = (json.connections || []).find((c) => c && c.emote_set_id);
      if (!link) {
        this.log.warn(`${constants.LOG_PREFIX} SET_NOT_LINKED: ${roomId}`);
        this.forgetChannel();
        return false;
      }
      json = await this.fetchJson(constants.emoteSetByIdUrl(link.emote_set_id), link.emote_set_id);
      if (!json) {
        this.forgetChannel();
        return false;
      }
    }

    const emotes = this.emotesFromJson(json) || [];

    for (const emote of emotes) {
      if (!emote || typeof emote.name !== "string" || typeof emote.id !== "string") continue;
      const animated = Boolean(emote.data && emote.data.animated);
      this.byName.set(emote.name, { id: emote.id, animated });
    }

    this.setLoaded = true;
    this.log.info(`${constants.LOG_PREFIX} SET_LOADED: ${roomId}, эмоутов ${this.byName.size}`);
    return true;
  }

  forget() {
    const had = this.cache.size + this.byName.size;
    this.cache.clear();
    this.byName.clear();
    this.forgetChannel();
    if (had) this.log.info(`${constants.LOG_PREFIX} FORGOTTEN: кэш и набор канала очищены`);
  }

  forgetChannel() {
    this.roomId = null;
    this.setPromise = null;
    this.setLoaded = false;
  }

  cacheKey(name, px) {
    return px > 0 ? `${name}@${px}` : name;
  }

  async resolve(name, px) {
    if (this.setPromise) {
      try {
        await this.setPromise;
      } catch (error) {
        return null;
      }
    }
    if (!this.setLoaded) return null;

    const key = this.cacheKey(name, px);
    if (this.cache.has(key)) return this.cache.get(key);

    const entry = this.byName.get(name);
    if (!entry) {
      this.remember(key, null);
      return null;
    }

    if (entry.animated && this.decodeGif && this.sampleFrames && this.resizeRgba) {
      const animated = await this.resolveAnimated(name, entry, px);
      if (animated) return animated;
    }

    const png = await this.get(constants.emoteImageUrl(entry.id, entry.animated, px), {
      maxBytes: constants.MAX_IMAGE_BYTES,
      accept: "image/png",
    });
    if (!png) {
      this.log.warn(`${constants.LOG_PREFIX} IMAGE_FETCH_FAILED: ${name}`);
      return null;
    }

    const decoded = this.decodePng(png, px > 0 ? px : constants.EMOTE_PX);
    if (!decoded) {
      this.log.warn(`${constants.LOG_PREFIX} IMAGE_DECODE_FAILED: ${name}`);
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

  async resolveAnimated(name, entry, px) {
    const gif = await this.get(constants.emoteGifUrl(entry.id, px), {
      maxBytes: constants.MAX_GIF_BYTES,
      accept: "image/gif",
    });
    if (!gif) return null;

    const decoded = this.decodeGif(gif);
    if (!decoded || !decoded.frames.length) {
      this.log.warn(`${constants.LOG_PREFIX} GIF_DECODE_FAILED: ${name}`);
      return null;
    }

    const target = px > 0 ? px : constants.EMOTE_PX;
    let outTall = target;
    let outWide = Math.max(1, Math.round((decoded.wide * target) / decoded.tall));
    const maxW = target * constants.MAX_ASPECT;
    if (outWide > maxW) {
      outTall = Math.max(1, Math.round((decoded.tall * maxW) / decoded.wide));
      outWide = Math.round(maxW);
    }

    const byWidth = Math.max(2, Math.floor(constants.MAX_SHEET_WIDE / outWide));
    const picked = this.sampleFrames(decoded, Math.min(constants.MAX_ANIM_FRAMES, byWidth));
    if (picked.frames.length < 2) return null;

    const out = [];
    for (const frame of picked.frames) {
      const scaled = this.resizeRgba(frame, decoded.wide, decoded.tall, outWide, outTall);
      if (!scaled) return null;
      out.push(scaled.toString("base64"));
    }

    this.log.info(
      `${constants.LOG_PREFIX} ANIMATED: ${name} ${outWide}x${outTall}, ` +
        `${out.length} из ${decoded.frames.length} кадров, ${picked.intervalMs}мс`
    );

    return { wide: outWide, tall: outTall, frames: out, frameMs: picked.intervalMs };
  }

  remember(name, value) {
    if (this.cache.size >= constants.MAX_CACHED_IMAGES) {
      const oldest = this.cache.keys().next();
      if (!oldest.done) this.cache.delete(oldest.value);
    }
    this.cache.set(name, value);
  }

  diagnostics() {
    return {
      roomId: this.roomId,
      setLoaded: this.setLoaded,
      emotes: this.byName.size,
      cached: this.cache.size,
    };
  }
}

module.exports = { SevenTvService };
