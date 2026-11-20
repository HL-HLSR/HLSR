"use strict";

const https = require("https");
const crypto = require("crypto");
const { URL } = require("url");

const channel = require("./launcher-channel");
const endpoints = require("../../shared/delivery-endpoints");

const DEFAULT_TIMEOUT_MS = 20000;
const MAX_BODY = 256 * 1024;
const MAX_REDIRECTS = 5;

function httpsGet(url, options) {
  const opts = options || {};
  const redirectsLeft = opts.redirectsLeft == null ? MAX_REDIRECTS : opts.redirectsLeft;

  return new Promise((resolve, reject) => {
    if (!channel.isAllowedUrl(url))
      return reject(new Error(`URL вне allowlist: ${url}`));

    const parsed = new URL(url);
    const request = https.get(
      {
        hostname: parsed.hostname,
        port: parsed.port || 443,
        path: parsed.pathname + parsed.search,
        headers: { "user-agent": "HLSR-launcher-update" },
        timeout: opts.timeout || DEFAULT_TIMEOUT_MS,
        signal: opts.signal,
      },
      (response) => {
        const status = response.statusCode;

        if (status >= 301 && status <= 308 && response.headers.location) {
          response.resume();
          if (!redirectsLeft) return reject(new Error("слишком много редиректов"));

          const next = new URL(response.headers.location, url).toString();
          return resolve(httpsGet(next, Object.assign({}, opts, { redirectsLeft: redirectsLeft - 1 })));
        }

        if (status !== 200) {
          response.resume();
          return reject(new Error(`HTTP ${status}`));
        }

        const chunks = [];
        let total = 0;

        response.on("data", (chunk) => {
          total += chunk.length;
          if (total > (opts.maxBody || MAX_BODY)) {
            response.destroy();
            return reject(new Error("ответ слишком большой"));
          }
          chunks.push(chunk);
        });

        response.on("end", () => resolve(Buffer.concat(chunks)));
        response.on("error", reject);
      }
    );

    request.on("timeout", () => request.destroy(new Error("таймаут")));
    request.on("error", reject);
  });
}

class LauncherUpdateGate {
  constructor(options) {
    const opts = options || {};

    this.currentVersion = opts.currentVersion;
    this.signal = opts.signal;
    this.channelUrls = opts.channelUrls || endpoints.launcher.channelUrls.slice();
    this.latestYmlUrls = opts.latestYmlUrls || endpoints.launcher.latestYmlUrls.slice();
    this.publicKeys = opts.publicKeys || channel.LAUNCHER_PUBLIC_KEYS;
    this.httpGet = opts.httpGet || httpsGet;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };

    this.lastResult = null;
  }

  async fetchChannelFrom(url) {
    const body = await this.httpGet(url, { signal: this.signal });

    try {
      return JSON.parse(body.toString("utf8"));
    } catch (e) {
      throw new Error(`не JSON: ${e.message}`, { cause: e });
    }
  }

  async fetchChannel() {
    let lastError = null;

    for (const url of this.channelUrls) {
      try {
        return { manifest: await this.fetchChannelFrom(url), sourceUrl: url };
      } catch (error) {
        lastError = error;
        this.logger.warn(`[launcher-update] канал недоступен (${url}): ${error.message}`);
      }
    }

    throw lastError || new Error("каналы обновления лаунчера недоступны");
  }

  async probeLatest() {
    const url = this.channelUrls[0];
    if (!url) return null;

    let manifest;
    try {
      manifest = await this.fetchChannelFrom(url);
    } catch (error) {
      this.logger.warn(`[launcher-update] канал не ответил на справку (${url}): ${error.message}`);
      return null;
    }

    try {
      return await this.evaluateManifest(manifest, url, null);
    } catch (error) {
      this.logger.warn(`[launcher-update] справка по каналу не удалась: ${error.message}`);
      return null;
    }
  }

  async evaluate(expectedVersion) {
    let firstOutcome = null;
    let lastError = null;

    for (const url of this.channelUrls) {
      let manifest;
      try {
        manifest = await this.fetchChannelFrom(url);
      } catch (error) {
        lastError = error;
        this.logger.warn(`[launcher-update] канал недоступен (${url}): ${error.message}`);
        continue;
      }

      const outcome = await this.evaluateManifest(manifest, url, expectedVersion);

      if (outcome.ok) {
        if (firstOutcome)
          this.logger.info(
            `[launcher-update] основной канал не подошёл, взят резервный: ${url}`
          );
        this.lastResult = outcome;
        return outcome;
      }

      if (!firstOutcome) firstOutcome = outcome;
    }

    if (firstOutcome) {
      this.lastResult = firstOutcome;
      return firstOutcome;
    }

    throw lastError || new Error("каналы обновления лаунчера недоступны");
  }

  async evaluateManifest(manifest, sourceUrl, expectedVersion) {
    const result = channel.check(manifest, {
      currentVersion: this.currentVersion,
      publicKeys: this.publicKeys,
    });

    const outcome = {
      ok: result.ok,
      problems: result.problems.slice(),
      updateAvailable: result.updateAvailable,
      downgrade: result.downgrade,
      unsupported: !!result.unsupported,
      code: result.code || null,
      // Обязательно только обновление на версию строго новее: подписанный канал той же
      // или старой версии (повтор с зеркала) не должен блокировать запуск.
      updateRequired: result.ok && result.updateAvailable && manifest.updateRequired === true,
      manifest: result.ok ? manifest : null,
      sourceUrl,
    };

    if (result.ok && expectedVersion && manifest.version !== expectedVersion) {
      outcome.ok = false;
      outcome.problems.push(
        `feed предлагает ${expectedVersion}, подписанные метаданные — ${manifest.version}`
      );
    }

    if (result.ok && result.downgrade) {
      outcome.ok = false;
      outcome.problems.push(
        `версия ${manifest.version} старше установленной ${this.currentVersion} (downgrade отклонён)`
      );
    }

    if (outcome.ok && manifest.latestYmlSha256) {
      let ymlOk = false;
      let lastError = "недоступен";

      for (const url of this.latestYmlUrls) {
        try {
          const yml = await this.httpGet(url);
          const sha = crypto.createHash("sha256").update(yml).digest("hex");

          if (sha === manifest.latestYmlSha256) {
            ymlOk = true;
            break;
          }
          lastError = `SHA-256 latest.yml не совпал (${url})`;
        } catch (error) {
          lastError = error.message;
        }
      }

      if (!ymlOk) {
        outcome.ok = false;
        outcome.problems.push(`latest.yml не подтверждён: ${lastError}`);
      }
    }

    return outcome;
  }

  async verifyInstaller(filePath, options) {
    const opts = options || {};
    const source = opts.manifest || (this.lastResult && this.lastResult.manifest);

    if (!source)
      return {
        ok: false,
        problems: ["нет проверенных метаданных"],
        code: channel.CODES.NO_METADATA,
      };

    if (opts.expectedVersion && source.version !== opts.expectedVersion)
      return {
        ok: false,
        problems: [
          `проверенные метаданные описывают ${source.version}, ` +
            `а скачана ${opts.expectedVersion}`,
        ],
        code: channel.CODES.VERSION_MISMATCH,
      };

    return channel.verifyInstallerStream(filePath, source.installer, opts);
  }
}

function qaHttpGet(url, options) {
  if (/^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?\//.test(url)) {
    const http = require("http");

    return new Promise((resolve, reject) => {
      const request = http.get(url, (response) => {
        if (response.statusCode !== 200) {
          response.resume();
          return reject(new Error(`HTTP ${response.statusCode}`));
        }

        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => resolve(Buffer.concat(chunks)));
        response.on("error", reject);
      });

      request.on("error", reject);
    });
  }

  return httpsGet(url, options);
}

module.exports = LauncherUpdateGate;
module.exports.httpsGet = httpsGet;
module.exports.qaHttpGet = qaHttpGet;
