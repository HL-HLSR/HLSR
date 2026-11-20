"use strict";

const fs = require("fs");
const path = require("path");
const https = require("https");

const SteamAccount = require("../game-delivery/SteamAccount");

const PROFILE_HOST = "steamcommunity.com";

const IMAGE_HOSTS = [
  "steamcdn-a.akamaihd.net",
  "media.steampowered.com",
  "steamcommunity-a.akamaihd.net",
];
const IMAGE_HOST_SUFFIX = ".steamstatic.com";

const REQUEST_TIMEOUT_MS = 8000;
const MAX_REDIRECTS = 3;

function isAllowedImageHost(hostname) {
  const host = String(hostname || "").toLowerCase();
  if (IMAGE_HOSTS.indexOf(host) !== -1) return true;
  return host.endsWith(IMAGE_HOST_SUFFIX);
}

function get(url, options) {
  const opts = options || {};
  const limit = opts.maxBytes || SteamAccount.MAX_AVATAR_BYTES;
  const left = opts.redirectsLeft == null ? MAX_REDIRECTS : opts.redirectsLeft;

  return new Promise((resolve) => {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (error) {
      return resolve(null);
    }

    if (parsed.protocol !== "https:") return resolve(null);
    if (typeof opts.allowHost === "function" && !opts.allowHost(parsed.hostname))
      return resolve(null);

    const request = https.get(
      url,
      {
        headers: { "user-agent": "HLSR", accept: opts.accept || "*/*" },
        timeout: REQUEST_TIMEOUT_MS,
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

function avatarUrlFrom(xml) {
  const text = String(xml || "");
  const match =
    /<avatarMedium>\s*(?:<!\[CDATA\[)?\s*(https:\/\/[^\]\s<]+)\s*(?:\]\]>)?\s*<\/avatarMedium>/i.exec(
      text
    );

  return match ? match[1] : null;
}

function imageTypeOf(data) {
  if (!Buffer.isBuffer(data) || data.length < 8) return null;
  if (data.readUInt32BE(0) === 0x89504e47) return "png";
  if (data[0] === 0xff && data[1] === 0xd8 && data[2] === 0xff) return "jpeg";
  return null;
}

function cacheFileFor(cacheDir, steamId64) {
  return path.join(cacheDir, `${steamId64}.bin`);
}

/**
 * Аватар одного человека: сначала наш кеш на диске, потом сеть.
 *
 * Возвращает data-URI или null. Не бросает никогда.
 */
async function fetchAvatar(steamId64, options) {
  const opts = options || {};
  const io = opts.fs || fs.promises;
  const cacheDir = opts.cacheDir;
  const fetch = opts.get || get;

  if (!/^\d{1,20}$/.test(String(steamId64))) return null;

  if (cacheDir) {
    try {
      const data = await io.readFile(cacheFileFor(cacheDir, steamId64));
      const type = imageTypeOf(data);
      if (type) return `data:image/${type};base64,${data.toString("base64")}`;
    } catch (error) {
      /* в кеше нет — пойдём в сеть */
    }
  }

  const xml = await fetch(`https://${PROFILE_HOST}/profiles/${steamId64}?xml=1`, {
    allowHost: (host) => String(host).toLowerCase() === PROFILE_HOST,
    accept: "text/xml",
    maxBytes: 256 * 1024,
  });

  if (!xml) return null;

  const url = avatarUrlFrom(xml.toString("utf8"));
  if (!url) return null;

  const image = await fetch(url, { allowHost: isAllowedImageHost });
  const type = imageTypeOf(image);
  if (!type) return null;

  if (cacheDir) {
    try {
      await io.mkdir(cacheDir, { recursive: true });
      await io.writeFile(cacheFileFor(cacheDir, steamId64), image);
    } catch (ignored) {
    }
  }

  return `data:image/${type};base64,${image.toString("base64")}`;
}

module.exports = {
  fetchAvatar,
  avatarUrlFrom,
  imageTypeOf,
  isAllowedImageHost,
  PROFILE_HOST,
  IMAGE_HOSTS,
  IMAGE_HOST_SUFFIX,
};
