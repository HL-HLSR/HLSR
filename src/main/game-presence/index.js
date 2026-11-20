"use strict";

const { PresenceBridgeServer } = require("./PresenceBridgeServer");
const gameState = require("./game-state");

class GamePresenceBridge {
  constructor(options) {
    const opts = options || {};
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());

    this.presenceHandler = opts.onPresence || (() => {});

    this.state = gameState.emptyState();
    this.lastFpsPublishAtMs = null;

    this.server = new PresenceBridgeServer({
      log: this.log,
      now: this.now,
      token: opts.token,
      pipeName: opts.pipeName,
      port: opts.port,
      limits: opts.limits,
      onState: (message) => this.onMessage(message),
      onGameGone: () => this.onGameGone(),
      onChat: (message) => this.onChatMessage(message),
      onGameReady: () => this.onGameReady(),
    });

    this.chatConsumers = [];
  }

  attachChatConsumer(consumer) {
    if (!consumer) return;
    this.chatConsumers.push(consumer);
    if (typeof consumer.attachBridge === "function") consumer.attachBridge(this.server);
  }

  onChatMessage(message) {
    for (const consumer of this.chatConsumers) {
      if (typeof consumer.handleMessage !== "function") continue;
      try {
        consumer.handleMessage(message);
      } catch (error) {
        this.log.warn(`[game-presence] потребитель чата упал: ${error.message}`);
      }
    }
  }

  onGameReady() {
    for (const consumer of this.chatConsumers) {
      if (typeof consumer.handleReady !== "function") continue;
      try {
        consumer.handleReady();
      } catch (error) {
        this.log.warn(`[game-presence] потребитель чата не смог поздороваться: ${error.message}`);
      }
    }
  }

  start() {
    return this.server.start();
  }

  shutdown() {
    this.forgetConsumers();
    this.server.stop();
    this.state = gameState.emptyState();
    this.lastFpsPublishAtMs = null;
  }

  environment() {
    return this.server.environment();
  }

  endpoint() {
    return this.server.endpoint();
  }

  onMessage(message) {
    const at = this.now();
    const result = gameState.apply(this.state, message, at);
    if (result.ignored) return false;

    const previous = this.state;
    this.state = result.state;
    if (!result.changed) return false;

    if (!gameState.shouldPublish(previous, this.state, this.lastFpsPublishAtMs, at)) return false;

    if (gameState.sameExceptFps(previous, this.state)) this.lastFpsPublishAtMs = at;
    else this.lastFpsPublishAtMs = null;

    this.emit();
    return true;
  }

  forgetConsumers() {
    for (const consumer of this.chatConsumers) {
      if (typeof consumer.forget !== "function") continue;
      try {
        consumer.forget();
      } catch (error) {
        this.log.warn(`[game-presence] потребитель чата не смог очиститься: ${error.message}`);
      }
    }
  }

  onGameGone() {
    this.forgetConsumers();

    if (!this.state.active) return false;
    this.state = gameState.emptyState();
    this.lastFpsPublishAtMs = null;
    this.emit();
    return true;
  }

  emit() {
    try {
      this.presenceHandler(this.snapshot());
    } catch (error) {
      this.log.warn(`[game-presence] потребитель состояния упал: ${error.message}`);
    }
  }

  snapshot() {
    const s = this.state;
    return {
      active: s.active,
      lifecycle: s.lifecycle,
      map: s.map,
      fps: s.fps,
      running: s.running,
      runStartedAtMs: s.runStartedAtMs,
      finalTime: s.finalTime,
    };
  }

  setDiscordConnected(connected) {
    return this.server.setDiscordConnected(connected);
  }

  diagnostics() {
    const bridge = this.server.diagnostics();
    return {
      listening: bridge.listening,
      connected: bridge.connected,
      rejected: bridge.rejected,
      lifecycle: this.state.lifecycle,
      hasMap: Boolean(this.state.map),
    };
  }
}

module.exports = { GamePresenceBridge };
