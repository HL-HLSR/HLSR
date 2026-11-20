"use strict";

const { STATES, CODES, TIMINGS, APPLICATION_ID } = require("./constants");
const presenceModel = require("./presence-model");

class DiscordRichPresenceService {
  constructor(options) {
    const opts = options || {};

    this.createClient = opts.createClient || null;
    this.applicationId = opts.applicationId || APPLICATION_ID;

    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.setTimer = opts.setTimeout || setTimeout;
    this.clearTimer = opts.clearTimeout || clearTimeout;

    this.versionProvider = opts.versionProvider || (() => null);
    this.localeProvider = opts.localeProvider || (() => "ru");

    this.connectionHandler = opts.onConnectionChange || (() => {});
    this.connected = false;

    this.state = STATES.DISABLED;
    this.client = null;
    this.attempt = 0;

    this.generation = 0;

    this.retryTimer = null;
    this.debounceTimer = null;

    this.pendingModel = null;
    this.sentModel = null;
    this.lastConnectedAtMs = null;
    this.lastCode = null;

    this.lastLogged = new Map();
  }

  publicState() {
    return {
      state: this.state,
      code: this.lastCode,
      lastConnectedAt: this.lastConnectedAtMs,
      attempt: this.attempt,
    };
  }

  setConnectedFlag(value) {
    const next = value === true;
    if (next === this.connected) return false;
    this.connected = next;
    try {
      this.connectionHandler(next);
    } catch (e) {
      /* потребитель не должен ронять интеграцию */
    }
    return true;
  }

  note(code, message) {
    this.lastCode = code;
    const at = this.now();
    const seen = this.lastLogged.has(code);
    const previous = this.lastLogged.get(code);
    if (seen && at - previous < TIMINGS.LOG_THROTTLE_MS) return;
    this.lastLogged.set(code, at);
    this.log.info(`[discord-rpc] ${code}${message ? ": " + message : ""}`);
  }

  clearRetry() {
    if (this.retryTimer) {
      this.clearTimer(this.retryTimer);
      this.retryTimer = null;
    }
  }

  clearDebounce() {
    if (this.debounceTimer) {
      this.clearTimer(this.debounceTimer);
      this.debounceTimer = null;
    }
  }

  clearAllTimers() {
    this.clearRetry();
    this.clearDebounce();
  }

  retryDelay() {
    const base = Math.min(
      TIMINGS.RETRY_BASE_MS * Math.pow(2, Math.max(0, this.attempt - 1)),
      TIMINGS.RETRY_MAX_MS
    );
    const spread = base * TIMINGS.RETRY_JITTER;
    return Math.round(base - spread + Math.random() * spread * 2);
  }

  isEnabled() {
    return this.state !== STATES.DISABLED && this.state !== STATES.SHUTTING_DOWN;
  }

  enable() {
    if (this.state === STATES.SHUTTING_DOWN) return false;
    if (this.isEnabled()) return false;

    this.generation++;
    this.attempt = 0;
    this.state = STATES.DISCONNECTED;
    this.note(CODES.CONNECTING, "включено пользователем");
    this.connect();
    return true;
  }

  disable() {
    if (this.state === STATES.DISABLED) return false;

    this.generation++;
    this.clearAllTimers();
    this.destroyClient(true);

    this.state = STATES.DISABLED;
    this.attempt = 0;
    this.pendingModel = null;
    this.sentModel = null;
    this.setConnectedFlag(false);
    this.note(CODES.DISABLED, "выключено пользователем");
    return true;
  }

  shutdown() {
    if (this.state === STATES.SHUTTING_DOWN) return false;
    this.generation++;
    this.clearAllTimers();
    this.destroyClient(true);
    this.state = STATES.SHUTTING_DOWN;
    this.setConnectedFlag(false);
    this.lastCode = CODES.SHUTDOWN;
    this.log.info(`[discord-rpc] ${CODES.SHUTDOWN}`);
    return true;
  }

  destroyClient(clearActivity) {
    const client = this.client;
    this.client = null;
    if (!client) return;

    try {
      if (clearActivity && typeof client.clearActivity === "function") {
        const result = client.clearActivity();
        if (result && typeof result.catch === "function") result.catch(() => {});
      }
    } catch (e) {
      /* Discord уже закрыт — очищать нечего */
    }

    try {
      if (typeof client.destroy === "function") {
        const result = client.destroy();
        if (result && typeof result.catch === "function") result.catch(() => {});
      }
    } catch (e) {
      /* сокет уже закрыт */
    }
  }

  connect() {
    if (!this.isEnabled() || !this.applicationId) return;
    if (this.state === STATES.CONNECTING || this.client) return;
    if (typeof this.createClient !== "function") {
      this.state = STATES.UNAVAILABLE;
      this.note(CODES.DISCORD_UNAVAILABLE, "клиент Discord недоступен в этой сборке");
      return;
    }

    const generation = this.generation;
    this.state = STATES.CONNECTING;
    this.attempt++;
    this.note(CODES.CONNECTING);

    let client = null;
    try {
      client = this.createClient({ applicationId: this.applicationId });
    } catch (error) {
      this.onConnectFailed(generation, error);
      return;
    }
    if (!client) {
      this.onConnectFailed(generation, new Error("фабрика не вернула клиент"));
      return;
    }

    if (typeof client.on === "function") {
      client.on("disconnected", () => this.onDisconnected(generation));
    }

    let login = null;
    try {
      login = client.login({ clientId: this.applicationId });
    } catch (error) {
      this.destroyOrphan(client);
      this.onConnectFailed(generation, error);
      return;
    }

    Promise.resolve(login).then(
      () => {
        if (generation !== this.generation) {
          this.destroyOrphan(client);
          return;
        }
        this.client = client;
        this.state = STATES.CONNECTED;
        this.attempt = 0;
        this.lastConnectedAtMs = this.now();
        this.lastLogged.delete(CODES.DISCORD_UNAVAILABLE);
        this.lastCode = CODES.CONNECTED;
        this.log.info(`[discord-rpc] ${CODES.CONNECTED}`);
        this.setConnectedFlag(true);
        this.flush();
      },
      (error) => {
        this.destroyOrphan(client);
        this.onConnectFailed(generation, error);
      }
    );
  }

  destroyOrphan(client) {
    try {
      if (client && typeof client.destroy === "function") {
        const result = client.destroy();
        if (result && typeof result.catch === "function") result.catch(() => {});
      }
    } catch (e) {
      /* нечего закрывать */
    }
  }

  onConnectFailed(generation, error) {
    if (generation !== this.generation) return;
    this.client = null;
    this.setConnectedFlag(false);
    this.state = STATES.UNAVAILABLE;
    this.note(CODES.DISCORD_UNAVAILABLE, error && error.code ? String(error.code) : null);
    this.scheduleRetry(generation);
  }

  onDisconnected(generation) {
    if (generation !== this.generation) return;
    if (!this.isEnabled()) return;
    this.client = null;
    this.setConnectedFlag(false);
    this.state = STATES.DISCONNECTED;
    this.sentModel = null;
    this.note(CODES.DISCONNECTED);
    this.scheduleRetry(generation);
  }

  scheduleRetry(generation) {
    if (!this.isEnabled()) return;
    this.clearRetry();

    const delay = this.retryDelay();
    this.state = STATES.RETRY_WAIT;
    this.retryTimer = this.setTimer(() => {
      this.retryTimer = null;
      if (generation !== this.generation) return;
      if (!this.isEnabled()) return;
      this.state = STATES.DISCONNECTED;
      this.connect();
    }, delay);

    return delay;
  }

  setPresence(state) {
    const model = presenceModel.normalize(
      Object.assign({}, state, { locale: this.localeProvider() })
    );

    if (presenceModel.same(model, this.pendingModel)) return false;
    this.pendingModel = model;

    if (!this.isEnabled()) return false;

    this.clearDebounce();
    this.debounceTimer = this.setTimer(() => {
      this.debounceTimer = null;
      this.flush();
    }, TIMINGS.DEBOUNCE_MS);
    return true;
  }

  flush() {
    if (this.state !== STATES.CONNECTED || !this.client) return false;
    if (!this.pendingModel) return false;
    if (presenceModel.same(this.pendingModel, this.sentModel)) return false;

    const model = this.pendingModel;
    const activity = presenceModel.toActivity(model, { version: this.versionProvider() });
    const generation = this.generation;

    try {
      const result = this.client.setActivity(activity);
      if (result && typeof result.then === "function") {
        result.then(
          () => {
            if (generation === this.generation) this.sentModel = model;
          },
          (error) => this.onUpdateFailed(generation, error)
        );
      } else {
        this.sentModel = model;
      }
    } catch (error) {
      this.onUpdateFailed(generation, error);
      return false;
    }
    return true;
  }

  onUpdateFailed(generation, error) {
    if (generation !== this.generation) return;
    this.note(CODES.UPDATE_FAILED, error && error.code ? String(error.code) : null);
  }
}

module.exports = { DiscordRichPresenceService };
