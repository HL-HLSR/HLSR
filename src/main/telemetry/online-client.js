"use strict";

const config = require("./config");
const transport = require("./transport");

class OnlineClient {
  constructor(options) {
    const opts = options || {};

    this.endpoint = opts.endpoint || config.ENDPOINTS.online;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.request = opts.request || transport.request;
    this.backendConfigured =
      opts.backendConfigured === undefined ? config.BACKEND_CONFIGURED : opts.backendConfigured;
    this.isWindowVisible = opts.isWindowVisible || (() => true);
    this.onUpdate = opts.onUpdate || (() => {});

    this.timer = null;
    this.inFlight = false;
    this.etag = null;
    this.lastValue = null;
    this.lastValueAt = 0;
    this.lastError = null;
    this.consecutiveFailures = 0;
  }

  snapshot() {
    if (this.lastValue === null)
      return { status: "unknown", launcherOnline: null, games: {}, updatedAt: null };

    const age = this.now() - this.lastValueAt;
    const stale = age > config.ONLINE.staleAfterMs;

    return {
      status: stale ? "stale" : "ok",
      launcherOnline: this.lastValue.launcherOnline,
      games: this.lastValue.games || {},
      updatedAt: new Date(this.lastValueAt).toISOString(),
      ageMs: age,
    };
  }

  start() {
    if (this.timer || !this.backendConfigured) return;

    this.fetchOnce().catch(() => {});

    this.timer = setInterval(() => {
      this.fetchOnce().catch(() => {});
    }, config.ONLINE.pollMs);

    if (typeof this.timer.unref === "function") this.timer.unref();
  }

  stop() {
    if (!this.timer) return;
    clearInterval(this.timer);
    this.timer = null;
  }

  async fetchOnce(options) {
    const force = options && options.force === true;

    if (!this.backendConfigured) return { skipped: "backend-off" };
    if (this.inFlight) return { skipped: "in-flight" };
    if (!force && !this.isWindowVisible()) return { skipped: "hidden" };

    this.inFlight = true;

    try {
      const headers = {};
      if (this.etag) headers["if-none-match"] = this.etag;

      const response = await this.request(this.endpoint, {
        method: "GET",
        headers,
        timeoutMs: config.ONLINE.timeoutMs,
        launcherVersion: this.launcherVersion,
      });

      return this.handle(response);
    } catch (error) {
      this.consecutiveFailures++;
      this.lastError = error && error.message ? error.message : "network";
      this.notify();
      return { ok: false, reason: "network" };
    } finally {
      this.inFlight = false;
    }
  }

  handle(response) {
    if (response.status === 304) {
      this.lastValueAt = this.now();
      this.consecutiveFailures = 0;
      this.lastError = null;
      this.notify();
      return { ok: true, notModified: true };
    }

    if (response.status < 200 || response.status >= 300) {
      this.consecutiveFailures++;
      this.lastError = `HTTP_${response.status}`;
      this.notify();
      return { ok: false, reason: "http", status: response.status };
    }

    const parsed = this.parse(response.body);
    if (!parsed) {
      this.consecutiveFailures++;
      this.lastError = "BAD_PAYLOAD";
      this.notify();
      return { ok: false, reason: "payload" };
    }

    const etag = response.headers && response.headers.etag;
    if (typeof etag === "string") this.etag = etag;

    this.lastValue = parsed;
    this.lastValueAt = this.now();
    this.consecutiveFailures = 0;
    this.lastError = null;
    this.notify();

    return { ok: true, value: parsed };
  }

  parse(body) {
    let data;
    try {
      data = JSON.parse(body);
    } catch (error) {
      return null;
    }

    if (!data || typeof data !== "object" || Array.isArray(data)) return null;
    if (data.schemaVersion !== 1) return null;
    if (!Number.isInteger(data.launcherOnline) || data.launcherOnline < 0) return null;

    const games = {};
    if (data.games && typeof data.games === "object" && !Array.isArray(data.games)) {
      for (const [gameId, value] of Object.entries(data.games)) {
        if (!/^[0-9a-z][0-9a-z._-]{0,63}$/i.test(gameId)) continue;
        if (!value || typeof value !== "object") continue;

        const online =
          value.online === null ? null : Number.isInteger(value.online) && value.online >= 0 ? value.online : undefined;
        if (online === undefined) continue;

        games[gameId] = {
          online,
          activeRuns: Number.isInteger(value.activeRuns) && value.activeRuns >= 0 ? value.activeRuns : null,
        };
      }
    }

    return { launcherOnline: data.launcherOnline, games };
  }

  notify() {
    try {
      this.onUpdate(this.snapshot());
    } catch (ignored) {
    }
  }
}

module.exports = OnlineClient;
