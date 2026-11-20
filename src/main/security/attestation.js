"use strict";

const crypto = require("crypto");
const config = require("../telemetry/config");
const transport = require("../telemetry/transport");

// Подтверждение сборки перед онлайн-функциями.
//
// Хеш app.asar в X-HLSR-Build подделывается одной строкой, поэтому к нему
// добавлен ответ на вызов сервера. Лаунчер просит вызов, сервер называет
// несколько кусков app.asar (из манифеста релиза, который он знает по
// каналу обновлений) и одноразовое число. Лаунчер читает эти куски с диска,
// считает их SHA-256 и подписывает вместе с числом ключом своей сборки
// (build-key.js). Публичную половину ключа сервер берёт из того же манифеста.
// Верная подпись меняется на билет — подписанную сервером запись «эта сборка
// проверена», которая живёт 15 минут и уходит заголовком X-HLSR-Ticket.
//
// Защита не абсолютна: ключ лежит в самой сборке, и тот, кто его достал,
// подпишет что угодно. Она отсекает правку app.asar без разбора байткода и
// заставляет заново вскрывать каждый релиз (у каждого свой ключ).
const CHALLENGE_PATH = "/v1/attest/challenge";
const PROVE_PATH = "/v1/attest/prove";
const LABEL = "hlsr-attest/1";
const MAX_RANGES = 8;
const MAX_RANGE_BYTES = 256 * 1024;
const BUILD_HEADER = /^(win32|linux)\/[0-9A-Za-z.-]+\/[a-f0-9]{64}$/;
const REFRESH_BEFORE_MS = 120 * 1000;
const FAILURE_BACKOFF_MS = 30 * 1000;
const FORCE_GAP_MS = 10 * 1000;
const UNSUPPORTED_MS = 60 * 60 * 1000;

function coded(code) {
  return Object.assign(new Error(code), { code });
}

function parse(response) {
  try { return JSON.parse(response.body); } catch (error) { return null; }
}

class BuildAttestation {
  constructor(options) {
    const opts = options || {};
    this.identity = opts.identity;
    this.key = opts.key;
    this.request = opts.request || transport.request;
    this.base = opts.apiBase || config.API_BASE;
    this.version = opts.launcherVersion || "0.0.0";
    this.now = opts.now || (() => Date.now());
    this.log = opts.log || { warn() {} };
    this.cached = null;
    this.pending = null;
    this.retryAt = 0;
    this.lastAttempt = 0;
    this.unsupportedUntil = 0;
  }

  // Подписать есть чем и это упакованная сборка: у dev-запуска нет app.asar.
  enabled() {
    return Boolean(this.identity && this.identity.packaged && this.key && this.key.available());
  }

  // Билет для X-HLSR-Ticket или null, если подтвердить сборку нечем или
  // сервер этого не требует. Бросает CLIENT_MODIFIED, когда сервер отверг
  // подпись и признал сборку изменённой. force — получить новый билет,
  // потому что сервер отказал по старому.
  async ticket(force) {
    if (!this.enabled()) return null;
    const now = this.now();
    if (now < this.unsupportedUntil) return null;
    const fresh = this.cached && this.cached.expiresAt > now ? this.cached.ticket : null;
    if (!force && fresh && this.cached.expiresAt - now > REFRESH_BEFORE_MS) return fresh;
    if (this.pending) return this.pending;
    const gapOk = force === true && now - this.lastAttempt >= FORCE_GAP_MS;
    if (now < this.retryAt && !gapOk) return fresh;
    this.lastAttempt = now;
    this.pending = this.handshake().then((result) => {
      if (result) this.cached = { ticket: result.ticket, expiresAt: this.now() + result.ttl * 1000 };
      else this.retryAt = this.now() + FAILURE_BACKOFF_MS;
      return result ? result.ticket : fresh;
    }, (error) => {
      if (error && error.code === "CLIENT_MODIFIED") throw error;
      this.retryAt = this.now() + FAILURE_BACKOFF_MS;
      this.log.warn(`[attest] подтвердить сборку не удалось: ${error && error.message}`);
      return fresh;
    }).finally(() => { this.pending = null; });
    return this.pending;
  }

  async post(endpoint, body, build) {
    const response = await this.request(this.base + endpoint, { method: "POST", body: JSON.stringify(body),
      headers: { "x-hlsr-build": build }, timeoutMs: 10000, maxResponseBytes: 64 * 1024, launcherVersion: this.version });
    if (response.status === 200) {
      const data = parse(response);
      if (!data || typeof data !== "object") throw coded("ATTEST_BAD_RESPONSE");
      return data;
    }
    const error = (parse(response) || {}).error;
    if (response.status === 404) {
      // Сервер не знает аттестации или у этой версии нет ключа.
      this.unsupportedUntil = this.now() + UNSUPPORTED_MS;
      return null;
    }
    if (response.status === 403 && error === "CLIENT_MODIFIED") throw coded("CLIENT_MODIFIED");
    throw coded(`ATTEST_HTTP_${response.status}${error ? "_" + error : ""}`);
  }

  async handshake() {
    const build = await this.identity.header();
    if (!BUILD_HEADER.test(build)) return null;
    const challenge = await this.post(CHALLENGE_PATH, {}, build);
    if (!challenge) return null;
    const ranges = challenge.ranges;
    if (typeof challenge.challenge !== "string" || challenge.challenge.length > 1024 || !Array.isArray(ranges) ||
      ranges.length < 1 || ranges.length > MAX_RANGES) throw coded("ATTEST_BAD_CHALLENGE");
    const hashes = [];
    for (const range of ranges) {
      if (!Array.isArray(range) || range.length !== 2 || !Number.isSafeInteger(range[0]) || !Number.isSafeInteger(range[1]) ||
        range[0] < 0 || range[1] <= 0 || range[1] > MAX_RANGE_BYTES) throw coded("ATTEST_BAD_CHALLENGE");
      hashes.push(crypto.createHash("sha256").update(await this.identity.readRange(range[0], range[1])).digest("hex"));
    }
    const message = [LABEL, build, challenge.challenge, hashes.join(",")].join("\n");
    const signature = this.key.sign(message).toString("base64url");
    const proved = await this.post(PROVE_PATH, { challenge: challenge.challenge, signature }, build);
    if (!proved) return null;
    if (typeof proved.ticket !== "string" || proved.ticket.length === 0 || proved.ticket.length > 1024 ||
      !Number.isInteger(proved.ttl) || proved.ttl < 60 || proved.ttl > 3600) throw coded("ATTEST_BAD_RESPONSE");
    return { ticket: proved.ticket, ttl: proved.ttl };
  }
}

BuildAttestation.LABEL = LABEL;

module.exports = BuildAttestation;
