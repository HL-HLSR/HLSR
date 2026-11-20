"use strict";

const config = require("./config");
const transport = require("./transport");

function jitter(valueMs, ratio) {
  const spread = valueMs * ratio;
  return Math.max(0, Math.round(valueMs - spread + Math.random() * spread * 2));
}

class Sender {
  constructor(options) {
    const opts = options || {};

    this.queue = opts.queue;
    this.endpoint = opts.endpoint || config.ENDPOINTS.batch;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.request = opts.request || transport.request;
    this.authorizationProvider = opts.authorizationProvider || null;

    this.failures = 0;
    this.nextAttemptAt = 0;
    this.inFlight = false;
    this.timer = null;
    this.stats = { sent: 0, accepted: 0, dropped: 0, lastError: null, lastSentAt: null };
  }

  start() {
    if (this.timer) return;

    this.timer = setInterval(() => {
      this.flush().catch(() => {});
    }, config.TRANSPORT.flushIntervalMs);

    if (typeof this.timer.unref === "function") this.timer.unref();
  }

  stop() {
    if (!this.timer) return;
    clearInterval(this.timer);
    this.timer = null;
  }

  async flush() {
    if (this.inFlight) return { skipped: "in-flight" };
    if (this.authorizationProvider && !this.authorizationProvider()) return { skipped: "signed-out" };
    if (this.now() < this.nextAttemptAt) return { skipped: "backoff" };

    const batch = this.queue.peek(config.QUEUE.batchSize, config.QUEUE.maxBatchBytes);
    if (batch.length === 0) return { skipped: "empty" };

    this.inFlight = true;

    const generation = this.queue.generation;

    try {
      const body = JSON.stringify({ schemaVersion: config.SCHEMA_VERSION, events: batch });

      const response = await this.request(this.endpoint, {
        method: "POST",
        body,
        headers: this.authorizationProvider ? { authorization: "Bearer " + this.authorizationProvider() } : {},
        timeoutMs: config.TRANSPORT.timeoutMs,
        launcherVersion: this.launcherVersion,
      });

      return this.handleResponse(response, batch.length, generation);
    } catch (error) {
      this.onFailure(error && error.message ? error.message : "network");
      return { ok: false, reason: "network" };
    } finally {
      this.inFlight = false;
    }
  }

  queueSurvived(generation) {
    return generation === undefined || generation === this.queue.generation;
  }

  handleResponse(response, count, generation) {
    const status = response.status;

    if (status >= 200 && status < 300) {
      if (this.queueSurvived(generation)) this.queue.remove(count);
      this.failures = 0;
      this.nextAttemptAt = 0;
      this.stats.sent += count;
      this.stats.lastSentAt = new Date(this.now()).toISOString();
      this.stats.lastError = null;

      let accepted = count;
      try {
        const parsed = JSON.parse(response.body);
        if (parsed && Number.isInteger(parsed.accepted)) accepted = parsed.accepted;
      } catch (ignored) {
      }
      this.stats.accepted += accepted;

      return { ok: true, sent: count, accepted };
    }

    if (status === 429) {
      const retryMs = transport.retryAfterMs(response.headers, this.now());
      this.failures++;
      this.nextAttemptAt = this.now() + (retryMs === null ? this.backoffMs() : retryMs);
      this.stats.lastError = "HTTP_429";
      return { ok: false, reason: "rate-limited", retryAfterMs: retryMs };
    }

    if (status >= 400 && status < 500) {
      if (this.queueSurvived(generation)) this.queue.remove(count);
      this.stats.dropped += count;
      this.stats.lastError = `HTTP_${status}`;
      this.logger.warn(`[telemetry] сервер отклонил ${count} событий (HTTP ${status}), очередь очищена`);

      this.onFailure(`HTTP_${status}`, true);
      return { ok: false, reason: "rejected", status };
    }

    this.onFailure(`HTTP_${status}`);
    return { ok: false, reason: "server", status };
  }

  onFailure(reason, keepQueueDrained) {
    this.failures++;
    this.stats.lastError = reason;
    this.nextAttemptAt = this.now() + this.backoffMs();

    if (this.failures === config.TRANSPORT.maxConsecutiveFailures)
      this.logger.warn(
        `[telemetry] ${this.failures} неудачных попыток подряд (${reason}), переход к максимальной задержке`
      );

    if (keepQueueDrained) this.failures = Math.min(this.failures, 3);
  }

  backoffMs() {
    const exponent = Math.min(this.failures, 8);
    const base = Math.min(
      config.TRANSPORT.backoffBaseMs * Math.pow(2, Math.max(0, exponent - 1)),
      config.TRANSPORT.backoffMaxMs
    );
    return jitter(base, config.TRANSPORT.jitterRatio);
  }

  snapshot() {
    return {
      queued: this.queue.size(),
      failures: this.failures,
      nextAttemptInMs: Math.max(0, this.nextAttemptAt - this.now()),
      sent: this.stats.sent,
      accepted: this.stats.accepted,
      dropped: this.stats.dropped,
      lastError: this.stats.lastError,
      lastSentAt: this.stats.lastSentAt,
    };
  }
}

module.exports = Sender;
module.exports.jitter = jitter;
