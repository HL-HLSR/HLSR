"use strict";

const { ipcMain } = require("electron");

const { DiscordRichPresenceService } = require("./DiscordRichPresenceService");
const preference = require("./preference");
const presenceModel = require("./presence-model");
const diagnosticsModel = require("./diagnostics-model");
const { APPLICATION_ID, STATES, CHANNELS } = require("./constants");

function defaultCreateClient({ applicationId }) {
  const DiscordRPC = require("discord-rpc");
  try {
    DiscordRPC.register(applicationId);
  } catch (e) {
    /* реестр недоступен — presence всё равно работает */
  }
  return new DiscordRPC.Client({ transport: "ipc" });
}

class DiscordRichPresenceBridge {
  constructor(options) {
    const opts = options || {};
    this.userDataPath = opts.userDataPath;
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.window = null;
    this.now = opts.now || (() => Date.now());

    this.sessionStartedAtMs = this.now();

    this.service = new DiscordRichPresenceService({
      createClient: opts.createClient || defaultCreateClient,
      applicationId: APPLICATION_ID,
      log: this.log,
      versionProvider: opts.versionProvider || (() => null),
      localeProvider: opts.localeProvider || (() => "ru"),
      onConnectionChange: (connected) => this.onDiscordConnection(connected),
    });

    this.selectedGameId = null;
    this.deliveryKind = presenceModel.KINDS.LAUNCHER;
    // Раздел лаунчера (ACTIVITY_KINDS) и идёт ли рендер демки (DemoTool).
    this.activity = null;
    this.demoRendering = false;

    this.gamePresence = null;
    this.gameState = null;
    this.episode = null;
  }

  setWindow(window) {
    this.window = window;
  }

  start() {
    try {
      preference.migrate(this.userDataPath);
    } catch (e) {
      this.log.warn(`[discord-rpc] миграция настройки не удалась: ${e.message}`);
    }

    if (preference.read(this.userDataPath).enabled) {
      this.service.enable();
      this.publish();
    }

    this.registerIpc();
  }

  registerIpc() {
    ipcMain.handle(CHANNELS.get, () => this.snapshot());

    ipcMain.on(CHANNELS.activity, (event, activity) => this.setActivity(activity));

    ipcMain.handle(CHANNELS.set, (event, enabled) => {
      const next = enabled === true;
      preference.set(this.userDataPath, next);

      if (next) {
        this.service.enable();
        this.publish();
      } else {
        this.service.disable();
      }

      const snapshot = this.snapshot();
      this.emitState(snapshot);
      return snapshot;
    });
  }

  snapshot() {
    const pref = preference.read(this.userDataPath);
    const state = this.service.publicState();
    return {
      enabled: pref.enabled,
      source: pref.source,
      state: state.state,
      code: state.code,
      lastConnectedAt: state.lastConnectedAt,
    };
  }

  emitState(snapshot) {
    if (this.window && !this.window.isDestroyed())
      this.window.webContents.send(CHANNELS.state, snapshot || this.snapshot());
  }

  attachDelivery(bridge) {
    if (!bridge || typeof bridge.onPresenceSignal !== "function") return false;
    bridge.onPresenceSignal((signal) => this.onSignal(signal));
    return true;
  }

  onSignal(signal) {
    if (!signal || typeof signal !== "object") return;
    const KINDS = presenceModel.KINDS;

    switch (signal.type) {
      case "selected":
        this.selectedGameId = signal.gameId != null ? String(signal.gameId) : null;
        if (this.deliveryKind !== KINDS.PLAYING) this.deliveryKind = KINDS.BROWSING;
        break;

      case "status":
        if (this.deliveryKind === KINDS.PLAYING) return;
        // Статус чужой игры (проверка обновлений в фоне) не меняет выбранную:
        // иначе при выбранной HL2 Ghosting в Discord была «Half-Life 2 (4260)».
        if (signal.gameId != null && this.selectedGameId && !presenceModel.sameGame(signal.gameId, this.selectedGameId)) return;
        this.selectedGameId = signal.gameId != null ? String(signal.gameId) : this.selectedGameId;
        this.deliveryKind = presenceModel.kindFromDelivery(signal);
        break;

      case "running":
        this.selectedGameId = signal.gameId != null ? String(signal.gameId) : this.selectedGameId;
        this.deliveryKind = KINDS.PLAYING;
        // Эпизод Ghosting (presence-model.EPISODE_TITLES проверяет значение).
        this.episode = signal.episode || null;
        break;

      case "stopped":
        this.deliveryKind = this.selectedGameId ? KINDS.READY : KINDS.LAUNCHER;
        this.gameState = null;
        this.episode = null;
        break;

      default:
        return;
    }

    this.publish();
  }

  attachGamePresence(bridge) {
    if (!bridge) return false;
    this.gamePresence = bridge;
    bridge.setDiscordConnected(this.service.connected === true);
    return true;
  }

  setGameState(state) {
    this.gameState = state && state.active ? state : null;
    this.publish();
  }

  // Раздел лаунчера; неизвестное значение — «Игры» (обычные состояния выбора и запуска).
  setActivity(activity) {
    const kind = Object.prototype.hasOwnProperty.call(presenceModel.ACTIVITY_KINDS, activity)
      ? presenceModel.ACTIVITY_KINDS[activity]
      : null;
    if (kind === this.activity) return;
    this.activity = kind;
    this.publish();
  }

  setDemoRendering(on) {
    if ((on === true) === this.demoRendering) return;
    this.demoRendering = on === true;
    this.publish();
  }

  onDiscordConnection(connected) {
    if (this.gamePresence) this.gamePresence.setDiscordConnected(connected);
  }

  publish() {
    const game = this.gameState;

    if (game && this.selectedGameId) {
      const kind =
        game.lifecycle === "playing" ? presenceModel.KINDS.GAME_MAP : presenceModel.KINDS.GAME_MENU;
      this.service.setPresence({
        kind,
        gameId: this.selectedGameId,
        episode: this.episode,
        sessionStartedAtMs: this.sessionStartedAtMs,
        map: game.map,
        fps: game.fps,
        running: game.running,
        runStartedAtMs: game.runStartedAtMs,
        finalTime: game.finalTime,
      });
      return;
    }

    // Игра, установка, обновление и проверка файлов важнее всего; рендер демки — важнее раздела.
    const KINDS = presenceModel.KINDS;
    const busy = [KINDS.PLAYING, KINDS.INSTALLING, KINDS.UPDATING, KINDS.REPAIRING].indexOf(this.deliveryKind) !== -1;
    let kind = this.deliveryKind;
    if (!busy && this.demoRendering) kind = KINDS.DEMO_RENDER;
    else if (!busy && this.activity) kind = this.activity;

    this.service.setPresence({
      kind,
      gameId: this.selectedGameId,
      episode: this.episode,
      sessionStartedAtMs: this.sessionStartedAtMs,
    });
  }

  diagnostics() {
    const snapshot = this.snapshot();
    return diagnosticsModel.normalize({
      enabled: snapshot.enabled,
      state: snapshot.state,
      lastConnectedAt: snapshot.lastConnectedAt,
      lastCode: snapshot.code,
      gameBridge: this.gamePresence ? this.gamePresence.diagnostics() : null,
    });
  }

  shutdown() {
    this.service.shutdown();
  }
}

module.exports = {
  CHANNELS,
  DiscordRichPresenceBridge,
  defaultCreateClient,
  STATES,
};
