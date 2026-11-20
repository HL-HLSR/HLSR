"use strict";
const fs = require("fs");
const path = require("path");
const https = require("https");
const { pipeline } = require("stream/promises");
const { Transform } = require("stream");
const channel = require("./launcher-channel");

function responseFor(url, options = {}, redirects = 5) {
  return new Promise((resolve, reject) => {
    if (!(options.isAllowedUrl || channel.isAllowedUrl)(url)) return reject(new Error("Недоверенный адрес установщика"));
    const request = (options.transport || https).get(url, { headers: { "User-Agent": "HLSR-Linux-Updater" }, timeout: 30000, signal: options.signal }, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode) && response.headers.location) {
        response.resume();
        if (!redirects) return reject(new Error("Слишком много перенаправлений"));
        return resolve(responseFor(new URL(response.headers.location, url).href, options, redirects - 1));
      }
      if (response.statusCode !== 200) { response.resume(); return reject(new Error(`HTTP ${response.statusCode}`)); }
      resolve(response);
    });
    request.on("timeout", () => request.destroy(new Error("Таймаут скачивания")));
    request.on("error", reject);
  });
}

async function download(manifest, directory, options = {}) {
  fs.mkdirSync(directory, { recursive: true });
  let lastError;
  for (const url of manifest.installer.urls) {
    if (options.signal && options.signal.aborted) throw new Error("Скачивание отменено");
    const target = path.join(directory, manifest.installer.filename);
    try {
      const response = await responseFor(url, options);
      let transferred = 0;
      const started = Date.now();
      const meter = new Transform({ transform(chunk, encoding, done) {
        transferred += chunk.length;
        if (transferred > manifest.installer.size) return done(new Error("Установщик больше подписанного размера"));
        if (options.onProgress) options.onProgress({ transferred, total: manifest.installer.size,
          percent: transferred / manifest.installer.size * 100, bytesPerSecond: transferred * 1000 / Math.max(1, Date.now() - started) });
        done(null, chunk);
      } });
      await pipeline(response, meter, fs.createWriteStream(target, { flags: "wx", mode: 0o600 }), { signal: options.signal });
      const result = await channel.verifyInstallerStream(target, manifest.installer, { signal: options.signal });
      if (options.signal) options.signal.throwIfAborted();
      if (!result.ok) throw new Error(result.problems.join("; "));
      return target;
    } catch (error) {
      lastError = error;
      try { fs.unlinkSync(target); } catch (_) { /* partial download absent */ }
    }
  }
  throw lastError || new Error("Нет доступных зеркал установщика");
}
module.exports = { download, responseFor };
