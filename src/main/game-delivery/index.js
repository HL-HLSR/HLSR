"use strict";

const { ipcMain } = require("electron");
const log = require("electron-log");
const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const paths = require("./paths");
const runsScan = require("./runs-scan");
const demoArchive = require("./demo-archive");
const { powershellPath } = require("../windows-tools");

const GameDeliveryService = require("./GameDeliveryService");
const GameActions = require("./GameActions");
// В Linux игра запускается через Proton, а Steam ищется в домашнем каталоге:
// у этих трёх служб своя реализация в src/main/linux.
const IS_LINUX = process.platform === "linux";
const LaunchService = IS_LINUX ? require("../linux/LaunchService") : require("./LaunchService");
const SteamLocator = IS_LINUX ? require("../linux/SteamLocator") : require("./SteamLocator");
const SteamAccount = IS_LINUX ? require("../linux/SteamAccount") : require("./SteamAccount");
const PresenceRelay = IS_LINUX ? require("../linux/PresenceRelay") : null;
const ToolService = require("./ToolService");
const toolRegistry = require("./tools");
const PracticeSaves = require("./practice-saves");
const TrayController = require("../tray/TrayController");

// Папка логов лаунчера: туда же пишется лог Bunnymod XT при запуске HL1.
function logsDirOf(logger) {
  try {
    return path.dirname(logger.transports.file.getFile().path);
  } catch (error) {
    return null;
  }
}

const TRAY_LABELS = {
  ru: {
    "#L_TRAY_SHOW": "Показать HLSR",
    "#L_TRAY_QUIT": "Выход",
  },
  en: {
    "#L_TRAY_SHOW": "Show HLSR",
    "#L_TRAY_QUIT": "Quit",
  },
};

function trayLabel(key, locale) {
  const table = TRAY_LABELS[locale] || TRAY_LABELS.en;
  return table[key] || TRAY_LABELS.en[key] || key;
}
const SteamDepotService = require("./steam-depot");

function writeRegistryDword(keyPath, valueName, value) {
  if (process.platform !== "win32") return false;

  const result = require("child_process").spawnSync(
    "reg.exe",
    ["add", keyPath, "/f", "/v", valueName, "/t", "REG_DWORD", "/d", String(value)],
    { windowsHide: true, shell: false, encoding: "utf8", timeout: 5000 }
  );

  return !!result && result.status === 0;
}
const GameSettingsStore = require("./GameSettingsStore");
const defaultConfigs = require("./default-configs");
const personalizationCode = require("./personalization-code");
const ghostingAssets = require("./ghosting-assets");
const ghostModel = require("./ghost-model");
const sourceScreen = require("./source-screen");
const { resetExtras } = require("../../shared/ghosting-settings");
const launchArgs = require("./launch-args");
const goldsrcDependencies = require("./goldsrc-dependencies");
const goldsrcPacks = require("./goldsrc-packs");
const customPacks = require("./custom-packs");
const gameBinds = require("../../shared/game-binds");
const cfgImport = require("../../shared/cfg-import");
const bindPresets = require("../../shared/bind-presets");
const hl1Guide = require("../../shared/hl1-guide");
const gameBindsInstall = require("./game-binds-install");
const playerConfig = require("./player-config");
const gameConfigInstall = require("./game-config-install");
const cfgFiles = require("./cfg-files");
const buildTransfer = require("./build-transfer");
const gamePack = require("./game-pack");
const LiveSplitOverlay = require("./livesplit-overlay");
const LiveSplitWindowCapture = require("./livesplit-window-capture");
const liveSplitSplits = require("./livesplit-splits");
const GameStatsStore = require("../game-stats/GameStatsStore");
const crashDumps = require("./crash-dumps");
const libraryMigration = require("./library-migration");
const libraryRecovery = require("./library-recovery");
const achievements = require("../profile/achievements");
const firstRun = require("../profile/first-run");
const steamFriends = require("../profile/steam-friends");
const steamAvatars = require("../profile/steam-avatars");
const notificationCatalog = require("../notifications/catalog");
const errorMap = require("../telemetry/error-map");
const RunLogReader = require("../telemetry/RunLogReader");
const telemetryIdentity = require("../telemetry/identity");

function toByteCount(value, fallback) {
  const candidate = value === undefined || value === null ? fallback : value;

  if (candidate === undefined || candidate === null) return null;
  if (typeof candidate !== "number" || !Number.isFinite(candidate)) return null;
  if (!Number.isInteger(candidate) || candidate < 0) return null;

  return candidate;
}

const links = require("../../shared/links");
const { DEFAULT_LIBRARY_PATH } = require("../../shared/library-path");
const DISCORD_URL = links.urlFor("discord");
const featureFlag = require("./feature-flag");
const qaMode = require("./qa-mode");
const qaLaunch = require("./qa-launch");
const channelConfig = require("./channel-config");
const manifestRegistry = require("./manifests");
const vulkanBackend = require("./vulkan-backend");
const progressStates = require("../../shared/progress-states");
const gameLanguage = require("./game-language");
const { DeliveryError, toDeliveryError } = require("./errors");
const episodes = require("./episodes");

const UPDATE_TICK_MS = 5 * 60 * 1000;

const ELEVATION_WAIT_MS = 3 * 60 * 1000;

const ELEVATION_QUIT_DELAY_MS = 600;

const STEAM_CONNECTION_FIRST_MS = 150;

const STEAM_CONNECTION_TICK_MS = 30 * 1000;

const STEAM_SIGNED_OUT_TICK_MS = 3 * 1000;

const STEAM_SIGNED_OUT_FAST_TICKS = 10;

const CHANNELS = {
  status: "game-delivery-status",
  getStatus: "game-delivery-get-status",
  install: "game-delivery-install",
  pause: "game-delivery-pause",
  cancel: "game-delivery-cancel",
  cleanup: "game-delivery-cleanup",
  info: "game-delivery-info",

  menuState: "game-menu-state",
  openFolder: "game-menu-open-folder",
  openScreenshots: "game-menu-open-screenshots",
  createShortcut: "game-menu-create-shortcut",
  checkUpdates: "game-menu-check-updates",
  applyUpdate: "game-menu-apply-update",
  verifyFiles: "game-menu-verify-files",
  repair: "game-menu-repair",
  uninstall: "game-menu-uninstall",
  selectBuild: "game-select-build",
  selectGame: "select-game",
  episodesStatus: "game-episodes-status",
  episodeInstall: "game-episode-install",
  episodeRemove: "game-episode-remove",

  launch: "game-process-launch",
  stop: "game-process-stop",
  gameSelected: "game-delivery-selected",

  launchStatus: "game-launch-steam-status",
  openSteam: "game-launch-open-steam",
  installSdk: "game-launch-install-sdk",
  restartElevated: "game-launch-restart-elevated",

  settingsGet: "game-settings-get",
  settingsSave: "game-settings-save",
  settingsReset: "game-settings-reset",
  liveSplitCapture: "livesplit-overlay-capture",

  releaseNotes: "game-release-notes",
  releaseNotesAll: "game-release-notes-all",

  processRunning: "game-process-running",
  processExited: "game-process-exited",
  processCrashed: "game-process-crashed",
  crashReport: "game-crash-report",
};

class GameDelivery {
  constructor(options) {
    const opts = options || {};

    this.window = opts.window;

    this.tray = this.window
      ? new TrayController({
          window: this.window,
          logger: log,
          translate: (key) => trayLabel(key, typeof opts.locale === "function" ? opts.locale() : null),
          isGameRunning: () => this.isAnyGameRunning(),
        })
      : null;

    this.libraryPath = opts.libraryPath;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.userDataPath = opts.userDataPath || null;
    this.liveSplitOverlay = this.userDataPath
      ? new LiveSplitOverlay({ baseDir: this.userDataPath, logger: log })
      : null;
    if (this.liveSplitOverlay) this.liveSplitOverlay.removeLegacyCapture();
    this.liveSplitWindowCapture = new LiveSplitWindowCapture({ logger: log });
    this.liveSplitCapture = null;
    this.protonRunner = opts.protonRunner || null;
    this.rateLimiter = opts.rateLimiter || null;
    this.lastSelectedGameId = "ghosting-renewed";
    this.notifications = opts.notifications || null;
    this.versionEvents = opts.versionEvents || null;
    this.telemetry = opts.telemetry || null;
    this.runLogReader = this.telemetry
      ? new RunLogReader({
          telemetry: this.telemetry,
          logger: log,
          generateUuid: telemetryIdentity.randomUuid,
        })
      : null;
    this.releaseNotesStore = null;
    this.service = null;
    this.initError = null;
    this.settingsStore = null;
    this.presenceHandler = null;
    this.gameEnvironmentProvider = null;
    this.statsStore = null;
    this.friendsCache = null;
    this.legacyLayout = [];
    this.dumpSnapshots = new Map();
    // Время старта процесса игры (мс): для «24/7» и «Марафона», когда лаунчер перезапускали посреди игры.
    this.processStarts = new Map();
    this.lastCrash = new Map();
    this.steamHealth = { status: "unknown", code: null, checkedAt: 0 };
    this.selectedBuilds = new Map();
    // Сплиты LiveSplit по играм ({ gameId: id набора }) из окна запуска.
    this.liveSplitChoices = {};
  }

  ensureReleaseNotes() {
    if (this.releaseNotesStore) return this.releaseNotesStore;
    if (!this.userDataPath) return null;

    const ReleaseNotesStore = require("./release-notes");
    this.releaseNotesStore = new ReleaseNotesStore({
      dir: this.userDataPath,
      logger: log,
    });

    return this.releaseNotesStore;
  }

  ensureStatsStore() {
    if (this.statsStore) return this.statsStore;
    if (!this.userDataPath) return null;

    this.statsStore = new GameStatsStore({
      file: require("path").join(this.userDataPath, GameStatsStore.FILE_NAME),
      logger: log,
      notifications: this.notifications,
    });

    return this.statsStore;
  }

  diagnosticsProfile() {
    const out = {
      signedIn: false,
      playSeconds: 0,
      launchCount: 0,
      gamesPlayed: 0,
      topGameId: "",
      achievements: null,
      installedAt: null,
    };

    try {
      const account = this.steamAccountState();
      out.signedIn = !!account.signedIn;

      const stats = this.profileStats();
      if (stats) {
        out.playSeconds = stats.playSeconds;
        out.launchCount = stats.launchCount;
        out.gamesPlayed = stats.gamesPlayed;
        out.topGameId = stats.topGameId;
      }

      const state = this.profileState();
      out.achievements = state.summary || null;
      out.installedAt = state.firstRunAt || null;
    } catch (error) {
      log.warn(`[diagnostics] снимок профиля не собран: ${error.message}`);
    }

    return out;
  }

  isWindowVisible() {
    try {
      if (!this.window || this.window.isDestroyed()) return false;
      return this.window.isVisible() && !this.window.isMinimized();
    } catch (error) {
      return true;
    }
  }

  profileStats() {
    let games = {};
    try {
      const store = this.ensureStatsStore();
      const all = store ? store.readAll() : null;
      games = (all && all.games) || {};
    } catch (error) {
      return null;
    }

    let playSeconds = 0;
    let launchCount = 0;
    let gamesPlayed = 0;

    for (const gameId of Object.keys(games)) {
      const seconds = Number(games[gameId] && games[gameId].totalPlaySeconds);
      const launches = Number(games[gameId] && games[gameId].launchCount);

      if (Number.isFinite(seconds) && seconds > 0) {
        playSeconds += seconds;
        gamesPlayed += 1;
      }
      if (Number.isFinite(launches) && launches > 0) launchCount += launches;
    }

    const best = achievements.bestGame(games);

    return {
      playSeconds: Math.floor(playSeconds),
      launchCount,
      gamesPlayed,
      topGameId: best ? best.gameId : "",
      topGameSeconds: best ? Math.floor(best.seconds) : 0,
    };
  }

  steamFriends() {
    if (!this.steamAccount) return [];

    let steam = null;
    let accountId = null;

    try {
      steam = this.steamAccount.locator ? this.steamAccount.locator.locateSteam() : null;
      accountId = this.steamAccount.activeAccountId();
    } catch (error) {
      return [];
    }

    if (!steam || !steam.installPath || !accountId) return [];

    try {
      this.friendsCache = steamFriends.list({
        installPath: steam.installPath,
        accountId,
        logger: log,
        avatars: false,
      });
    } catch (error) {
      log.warn(`[friends] список не прочитан: ${error.message}`);
      this.friendsCache = [];
    }

    return this.friendsCache;
  }

  avatarCacheDir() {
    if (!this.userDataPath) return null;
    return path.join(this.userDataPath, "avatar-cache");
  }

  steamInstallPath() {
    if (!this.steamAccount || !this.steamAccount.locator) return null;

    try {
      const steam = this.steamAccount.locator.locateSteam();
      return steam && steam.installPath ? steam.installPath : null;
    } catch (error) {
      return null;
    }
  }

  notify(code, detail) {
    if (!this.notifications) return;

    const level = notificationCatalog.levelForCode(code);
    if (!level) return;

    try {
      this.notifications.add({ level, code, detail });
    } catch (e) {
      log.warn(`[game-delivery] уведомление не записано: ${e.message}`);
    }
  }

  ensureSettingsStore() {
    if (this.settingsStore) return this.settingsStore;
    if (!this.userDataPath) return null;

    this.settingsStore = new GameSettingsStore({
      file: GameSettingsStore.defaultFile(this.userDataPath),
      logger: log,
    });

    return this.settingsStore;
  }

  recoverLibraryPath(current) {
    return libraryRecovery.recoverLibraryPath(current, {
      fallback: DEFAULT_LIBRARY_PATH,
      gameIds: Object.keys(manifestRegistry.MANIFESTS),
      logger: log,
    });
  }

  ensureService() {
    if (this.service || this.initError) return this.service;

    const qa = qaMode.isEnabled();

    try {
      const libraryPath = qa ? qaMode.libraryPath() : this.recoverLibraryPath(this.libraryPath);
      const manifests = qa
        ? qaMode.manifestsFor(manifestRegistry.MANIFESTS, null, process.env)
        : undefined;

      const filesManifests = qa
        ? qaMode.filesManifestsFor(manifestRegistry.FILES_MANIFESTS, process.env)
        : undefined;

      if (qa)
        log.warn(
          `[game-delivery] ВКЛЮЧЁН QA-РЕЖИМ (${qaMode.QA_FLAG}). ` +
            `Библиотека: ${libraryPath}, версия сборки: ${qaMode.QA_VERSION}. ` +
            "Это режим для локальной проверки, в production он недоступен."
        );

      const steamLocator = new SteamLocator({ logger: log });
      this.steamLocator = steamLocator;

      this.steamAccount = new SteamAccount({ locator: steamLocator, logger: log });

      // Resolve the connection before the renderer's first account request;
      // the timer below is responsible for subsequent changes.
      this.steamAccount.refreshConnection();

      this.startSteamConnectionWatch();

      this.depotService = new SteamDepotService({ logger: log, locator: steamLocator });

      this.service = new GameDeliveryService({
        libraryPath,
        launcherVersion: this.launcherVersion,
        manifests,
        filesManifests,
        logger: log,
        versionEvents: this.versionEvents,
        releaseNotes: this.ensureReleaseNotes(),
        depotService: this.depotService,
        setRegistryValue: writeRegistryDword,
        rateLimiter: this.rateLimiter,
        channelFreshnessFile: this.userDataPath ? path.join(this.userDataPath, "game-channel-freshness.json") : null,
      });

      this.service.on("status", (status) => this.send(status));
      this.guardRendering(this.service);

      try {
        this.tools = new ToolService({
          paths: this.service.paths,
          registry: Object.assign({}, toolRegistry.REGISTRY, toolRegistry.DEPENDENCIES),
          archive: this.service.archiveService,
          logger: log,
          protonRunner: this.protonRunner,
          rateLimiter: this.rateLimiter,
        });
        this.tools.on("state", (toolId) => {
          this.emitTo("tool-status", this.toolCard(toolId));
          if (toolId === toolRegistry.LIVESPLIT) this.bindLiveSplitOverlay();
        });
      } catch (error) {
        this.tools = null;
        log.error(`[tools] подсистема инструментов не поднялась: ${error.message}`);
      }

      this.practiceSaves = new PracticeSaves({
        paths: this.service.paths,
        archive: this.service.archiveService,
        logger: log,
        rateLimiter: this.rateLimiter,
      });

      this.actions = new GameActions({
        paths: this.service.paths,
        logger: log,
      });

      this.launchService = new LaunchService({
        delivery: this.service,
        logger: log,
        settingsStore: this.ensureSettingsStore(),
        launcherVersion: this.launcherVersion,
        steamLocator,
        depotService: this.depotService,
        extraEnvProvider: () => this.gameEnvironment(),
        protonRunner: this.protonRunner,
        liveSplitOverlay: this.liveSplitOverlay,
        presenceRelay: PresenceRelay && this.userDataPath ? new PresenceRelay({ baseDir: this.userDataPath }) : null,
        logsDir: logsDirOf(log),
        dependencyUpdater: (installPath, config) =>
          goldsrcDependencies.updateDependencies(installPath, config, {
            archive: this.service.archiveService,
            logger: log,
          }),
        packApplier: (installPath, config, settings) => {
          return goldsrcPacks.applyPacks(installPath, config, settings, {
            archive: this.service.archiveService,
            logger: log,
          });
        },
      });

      this.attachProcessEvents();

      try {
        this.legacyLayout = libraryMigration.detectLegacyLayout(
          libraryPath,
          Object.keys(manifestRegistry.MANIFESTS)
        );

        if (this.legacyLayout.length) {
          log.warn(
            `[game-delivery] обнаружена старая раскладка библиотеки: ` +
              this.legacyLayout.map((g) => g.from).join(", ")
          );
          this.notify("LIBRARY_LEGACY_LAYOUT", this.legacyLayout.map((g) => g.gameId).join(", "));
        }
      } catch (e) {
        this.legacyLayout = [];
      }

      this.service.refreshAll();
      this.recoverPending();

      this.service.reconcileProcesses();

      this.scheduleUpdateChecks();
    } catch (error) {
      this.initError = toDeliveryError(error, "PATH_INVALID");
      log.error(`[game-delivery] сервис не инициализирован: ${this.initError.message}`);
    }

    return this.service;
  }

  isStatusVisible(status) {
    if (!status || !status.gameId) return true;

    const launcherId = manifestRegistry.launcherIdOf(status.gameId);
    if (!launcherId) return true;

    const builds = manifestRegistry.BUILDS_BY_LAUNCHER_ID[String(launcherId)];
    if (!builds || builds.length < 2) return true;

    return this.resolveGameId(launcherId) === status.gameId;
  }

  send(status) {
    if (status && progressStates.isSuccess(status.state)) {
      this.checkFullSet();
      this.applyPendingBuildImport(status.gameId);
    }
    if (!this.isStatusVisible(status)) return;

    this.emitTo(CHANNELS.status, status);
    if (status && typeof status === "object")
      this.emitPresence({ type: "status", gameId: status.gameId, state: status.state });
  }

  achieve(id) {
    if (this.onlineProfiles && typeof this.onlineProfiles.achieve === "function") this.onlineProfiles.achieve(id);
  }

  // «Полный комплект»: установлены все игры, у которых есть манифест.
  checkFullSet() {
    if (!this.service || this.fullSetReached) return;
    try {
      const all = Object.keys(manifestRegistry.MANIFESTS);
      if (!all.every((gameId) => this.service.installService.readInstalled(gameId))) return;
      this.fullSetReached = true;
      this.achieve("full-set");
    } catch (error) {
      log.warn(`[profile] не удалось проверить установленные игры: ${error.message}`);
    }
  }

  emitTo(channel, payload) {
    if (this.window && !this.window.isDestroyed())
      this.window.webContents.send(channel, payload);
  }

  onPresenceSignal(handler) {
    if (typeof handler === "function") this.presenceHandler = handler;
  }

  setGameEnvironmentProvider(provider) {
    this.gameEnvironmentProvider = typeof provider === "function" ? provider : null;
  }

  gameEnvironment() {
    if (!this.gameEnvironmentProvider) return null;
    try {
      return this.gameEnvironmentProvider();
    } catch (e) {
      log.warn(`[game-delivery] окружение запуска не получено: ${e.message}`);
      return null;
    }
  }

  emitPresence(signal) {
    if (!this.presenceHandler) return;
    try {
      this.presenceHandler(signal);
    } catch (e) {
      log.warn(`[discord-rpc] сигнал presence не обработан: ${e.message}`);
    }
  }

  attachProcessEvents() {
    const process = this.service.processService;

    process.on("starting", (event) => {
      try {
        const installed = this.service.installService.readInstalled(event.gameId);
        this.dumpSnapshots.set(event.gameId, {
          snapshot: crashDumps.snapshotDumps(installed ? installed.installPath : null, undefined, event.gameId),
          gameId: event.gameId,
          installPath: installed ? installed.installPath : null,
          startedAtMs: Date.now(),
        });
      } catch (e) {
        this.dumpSnapshots.delete(event.gameId);
      }
    });

    process.on("running", (event) => {
      // У подхваченного после перезапуска лаунчера процесса startedAt — из
      // записи, сверенной со временем старта процесса в ОС (matchesRecord).
      const startedAt = Date.parse(event.startedAt);
      if (Number.isFinite(startedAt)) this.processStarts.set(event.gameId, startedAt);
      else this.processStarts.delete(event.gameId);

      const stats = this.ensureStatsStore();
      if (stats)
        try {
          stats.sessionStart(event.gameId, {});
        } catch (e) {
          log.warn(`[game-stats] сессия не открыта: ${e.message}`);
        }

      if (this.onlineProfiles) this.onlineProfiles.gameStart(event.gameId);
      this.telemetryGameRunning(event.gameId);

      this.emitTo(CHANNELS.processRunning, event);

      if (this.tray) this.tray.onGameStarted();

      this.startTimerWithGame(event.gameId);

      this.emitPresence({ type: "running", gameId: event.gameId, episode: event.episode || null });
    });

    process.on("finished", (event) => {
      if (this.tray) this.tray.onGameFinished();

      if (this.steamAccount)
        try {
          this.steamAccount.refreshConnection();
          this.pushSteamAccount();
        } catch (error) {
          log.warn(`[steam] состояние подключения не обновлено: ${error.message}`);
        }

      if (event.lastExit && event.lastExit.outcome !== "crashed" && !event.lastExit.requested &&
          crashDumps.leftNewDump(this.dumpSnapshots.get(event.gameId))) {
        log.warn(`[game-process] ${event.gameId} вышла с кодом ${event.lastExit.exitCode}, но оставила дамп — считаем падением`);
        event.lastExit.outcome = "crashed";
      }

      const crashed =
        event.lastExit && event.lastExit.outcome === "crashed" && !event.lastExit.requested;
      const channel =
        event.lastExit && event.lastExit.outcome === "crashed"
          ? CHANNELS.processCrashed
          : CHANNELS.processExited;

      this.emitPresence({ type: "stopped", gameId: event.gameId });

      const startedAt = this.processStarts.get(event.gameId);
      this.processStarts.delete(event.gameId);
      const stats = this.ensureStatsStore();
      if (stats)
        try {
          const session = stats.sessionEnd(event.gameId);
          // «Марафон» и «24/7»: столько секунд за один запуск игры. Сессия
          // статистики после перезапуска лаунчера начинается заново, поэтому
          // берётся и время от старта процесса.
          const played = Math.max(session ? session.seconds : 0, startedAt ? (Date.now() - startedAt) / 1000 : 0);
          for (const id of achievements.sessionAchievements(played)) this.achieve(id);
        } catch (e) {
          log.warn(`[game-stats] сессия не закрыта: ${e.message}`);
        }

      if (this.runLogReader)
        try {
          this.runLogReader.detach();
        } catch (e) {
          log.warn(`[telemetry] приёмник забегов не отцеплен: ${e.message}`);
        }

      if (this.onlineProfiles) this.onlineProfiles.gameEnd(event.gameId);
      this.telemetryGameFinished(event.gameId, event.lastExit, crashed);

      if (crashed) this.handleCrash(event);

      this.emitTo(channel, {
        gameId: event.gameId,
        outcome: event.lastExit.outcome,
        exitCode: event.lastExit.exitCode,
        signal: event.lastExit.signal,
        requested: event.lastExit.requested,
        at: event.lastExit.at,
      });

      this.service.checkForUpdatesIfDue(event.gameId, "after-exit");
    });
  }

  scheduleUpdateChecks() {
    const run = (reason) => {
      for (const gameId of Object.keys(manifestRegistry.MANIFESTS))
        this.service.checkForUpdatesIfDue(gameId, reason);
    };

    run("launcher-start");

    const TICK_JITTER_MS = 30 * 1000;
    const schedule = () => {
      const delay = UPDATE_TICK_MS + Math.floor(Math.random() * TICK_JITTER_MS);
      this.updateTimer = setTimeout(() => {
        run("scheduled");
        schedule();
      }, delay);
      if (typeof this.updateTimer.unref === "function") this.updateTimer.unref();
    };

    schedule();
  }

  checkOnFocus() {
    this.steamMissWindow = 0;

    if (!this.service) return;
    for (const gameId of Object.keys(manifestRegistry.MANIFESTS))
      this.service.checkForUpdatesOnFocus(gameId);
  }

  // Linux: в префиксе какой игры запустить инструмент. Сначала игра из
  // настроек Proton (id из games.config, у HL2 — выбранная сборка), без
  // выбора — запущенная игра, иначе последняя выбранная.
  toolGameId(gameChoice) {
    const chosen = gameChoice ? this.resolveGameId(gameChoice) : null;
    const running = this.service && this.service.processService && this.service.processService.anyRunning();
    return chosen || running || this.lastSelectedGameId;
  }

  resolveGameId(id) {
    const value = String(id);
    if (manifestRegistry.isKnownGame(value)) return value;

    const selected = this.selectedBuilds.get(value);
    if (selected) return selected;

    return manifestRegistry.BY_LAUNCHER_ID[value] || null;
  }

  selectBuild(launcherId, build) {
    const key = String(launcherId);
    const gameId = manifestRegistry.buildGameIdOf(key, build);

    if (!gameId)
      return { status: "unknown-build", launcherId: key, build: String(build), builds: manifestRegistry.buildsOf(key) };

    this.selectedBuilds.set(key, gameId);
    log.info(`[game-delivery] выбрана сборка ${build} игры ${key} -> ${gameId}`);

    return { status: "ok", launcherId: key, build: String(build), gameId };
  }

  getReleaseNotes(id, version) {
    const gameId = this.resolveGameId(id);
    if (!gameId) return null;
    if (typeof version !== "string" || !/^[0-9A-Za-z.\-+]{1,32}$/.test(version)) return null;

    const store = this.ensureReleaseNotes();
    if (!store) return null;

    try {
      return store.get(gameId, version);
    } catch (error) {
      log.warn(`[release-notes] чтение не удалось: ${error.message}`);
      return null;
    }
  }

  allReleaseNotes() {
    const store = this.ensureReleaseNotes();
    if (!store) return [];

    let games = [];
    try {
      games = store.list();
    } catch (error) {
      log.warn(`[release-notes] список не прочитан: ${error.message}`);
      return [];
    }

    return games
      .map((game) => ({
        gameId: game.gameId,
        launcherId: manifestRegistry.launcherIdOf(game.gameId) || null,
        versions: game.versions.map((entry) => ({
          version: entry.version,
          items: entry.items,
        })),
      }))
      .filter((game) => game.versions.length);
  }

  initialize() {
    const flag = featureFlag.describe();
    log.info(
      `[game-delivery] feature flag ${featureFlag.FLAG}=${flag.remoteEnabled}; ` +
        `разрешённые провайдеры: ${flag.allowedProviderTypes.join(", ") || "нет"}`
    );

    ipcMain.handle(CHANNELS.info, () => this.info());
    ipcMain.handle(CHANNELS.getStatus, (event, id) => this.getStatus(id));
    ipcMain.handle(CHANNELS.install, (event, id) => this.install(id));
    ipcMain.handle(CHANNELS.pause, () => (this.service ? this.service.pause() : false));
    // options — { episode } из карточки эпизода на паузе: убрать архив только его.
    ipcMain.handle(CHANNELS.cancel, (event, id, options) => {
      if (!this.service) return false;
      const before = this.service.statusOf(id);
      const cancelled = this.service.cancel(id, options);
      if (cancelled && before && (before.state === "downloading" || before.state === "paused") && Number(before.percent) >= 99)
        this.achieve("almost");
      return cancelled;
    });
    ipcMain.handle(CHANNELS.cleanup, (event, id) => this.cleanup(id));

    ipcMain.handle(CHANNELS.menuState, (event, id) => this.menuState(id));
    ipcMain.handle(CHANNELS.openFolder, (event, id) =>
      this.run(() => this.actions.openGameFolder(this.requireGameId(id)))
    );
    ipcMain.handle(CHANNELS.openScreenshots, (event, id) =>
      this.run(() => this.actions.openScreenshotsFolder(this.requireGameId(id)))
    );
    ipcMain.handle(CHANNELS.createShortcut, (event, id) =>
      this.run(() => this.actions.createDesktopShortcut(this.requireGameId(id)))
    );
    ipcMain.handle(CHANNELS.releaseNotesAll, () => this.allReleaseNotes());
    ipcMain.handle(CHANNELS.releaseNotes, (event, id, version) =>
      this.getReleaseNotes(id, version)
    );
    ipcMain.handle(CHANNELS.checkUpdates, (event, id) =>
      this.run(() => this.service.checkForUpdates(this.requireGameId(id)))
    );
    ipcMain.handle(CHANNELS.applyUpdate, (event, id) =>
      this.run(() => this.applyUpdateWithTelemetry(this.requireGameId(id)))
    );
    ipcMain.handle(CHANNELS.verifyFiles, (event, id) =>
      this.run(async () => {
        const result = await this.service.verifyFiles(this.requireGameId(id));
        return this.service.summarizeIntegrity(result);
      })
    );
    ipcMain.handle(CHANNELS.repair, (event, id, options) =>
      this.run(() =>
        this.repairWithTelemetry(this.requireGameId(id), {
          confirmed: !!(options && options.confirmed),
          recheck: !(options && options.recheck === false),
          // Сервис берёт из них только то, что сам нашёл при проверке.
          files: options && Array.isArray(options.files)
            ? options.files.filter((item) => typeof item === "string")
            : undefined,
        })
      )
    );
    ipcMain.handle(CHANNELS.uninstall, (event, id, options) =>
      this.run(() => this.uninstallGame(this.requireGameId(id), options))
    );

    ipcMain.handle(CHANNELS.selectBuild, (event, id, build) => {
      if (this.ensureService() && this.service.isBusy())
        return { status: "busy", gameId: this.resolveGameId(id) };

      const result = this.selectBuild(id, build);

      // Discord: статусы новой сборки сравниваются с выбранной (discord-rpc/index.js).
      if (result.status === "ok") this.emitPresence({ type: "selected", gameId: result.gameId });

      if (result.status === "ok" && this.ensureService()) {
        this.service.checkForUpdatesIfDue(result.gameId, "build-selected");
        result.delivery = this.service.refresh(result.gameId);
      }

      return result;
    });

    // Эпизоды Ghosting: id эпизода ("ep1" | "ep2") проверяет сама служба (episodeIdOf).
    ipcMain.handle(CHANNELS.episodesStatus, () => (this.ensureService() ? this.service.episodesStatus() : null));
    ipcMain.handle(CHANNELS.episodeInstall, (event, id) => this.run(() => this.service.installEpisode(id)));
    ipcMain.handle(CHANNELS.episodeRemove, (event, id) => this.run(() => this.service.removeEpisode(id)));

    // Второй аргумент — { episode: "ep1" | "ep2" } для эпизода Ghosting; чужое значение — отказ.
    ipcMain.handle(CHANNELS.launch, (event, id, options) =>
      this.run(() => {
        const episode = options && typeof options === "object" && options.episode != null ? options.episode : null;
        if (episode !== null && !episodes.isEpisode(episode))
          throw new DeliveryError("LAUNCH_CONFIG_INVALID", "Неизвестный эпизод", { episode: String(episode) });
        return this.launch(this.requireGameId(id), episode ? { episode } : {});
      })
    );

    ipcMain.handle(CHANNELS.stop, (event, id, options) =>
      this.run(() =>
        this.service.stopGame(this.requireGameId(id), { force: !!(options && options.force) })
      )
    );

    ipcMain.handle(CHANNELS.launchStatus, (event, id) => {
      if (!this.ensureService()) return { ok: false, code: "LAUNCH_CONFIG_INVALID" };

      const gameId = this.resolveGameId(id) || "ghosting-renewed";
      const status = this.launchService.describeAvailability(gameId);

      return { ok: status.ok, code: status.code || null };
    });

    ipcMain.handle(CHANNELS.openSteam, () => {
      const { shell } = require("electron");
      shell.openExternal("steam://open/main");
      return { status: "ok" };
    });

    ipcMain.handle(CHANNELS.installSdk, () => {
      const { shell } = require("electron");
      shell.openExternal("steam://install/243730");
      return { status: "ok" };
    });

    ipcMain.handle(CHANNELS.restartElevated, async () => {
      const { app } = require("electron");
      const { spawn } = require("child_process");

      if (process.platform !== "win32") return { status: "unsupported" };

      try {
        app.releaseSingleInstanceLock();
      } catch (error) {
        log.warn(`[launch] замок единственного экземпляра не отпущен: ${error.message}`);
      }

      const quitSoon = () => setTimeout(() => app.quit(), ELEVATION_QUIT_DELAY_MS);

      return await new Promise((resolve) => {
        let settled = false;
        const answer = (value) => {
          if (settled) return;
          settled = true;
          resolve(value);
        };

        let child;

        try {
          child = spawn(
            powershellPath(),
            [
              "-NoProfile",
              "-NonInteractive",
              "-ExecutionPolicy",
              "Bypass",
              "-Command",
              "Start-Process -Verb RunAs -FilePath $env:HLSR_EXE",
            ],
            {
              stdio: "ignore",
              windowsHide: true,
              env: Object.assign({}, process.env, { HLSR_EXE: process.execPath }),
            }
          );
        } catch (error) {
          log.warn(`[launch] перезапуск от администратора не удался: ${error.message}`);
          app.requestSingleInstanceLock();
          return answer({ status: "error" });
        }

        const timer = setTimeout(() => answer({ status: "pending" }), ELEVATION_WAIT_MS);

        child.on("error", (error) => {
          clearTimeout(timer);
          log.warn(`[launch] перезапуск от администратора не удался: ${error.message}`);
          app.requestSingleInstanceLock();
          answer({ status: "error" });
        });

        child.on("exit", (code) => {
          clearTimeout(timer);

          if (code !== 0) {
            log.info(`[launch] повышение прав отклонено, код ${code}`);
            app.requestSingleInstanceLock();
            return answer({ status: "declined" });
          }

          quitSoon();
          answer({ status: "ok" });
        });
      });
    });

    ipcMain.handle(CHANNELS.settingsGet, (event, id) => this.gameSettings("get", id));
    ipcMain.handle(CHANNELS.settingsSave, (event, id, settings, scope, options) =>
      this.gameSettings("save", id, settings, scope, options)
    );
    ipcMain.handle(CHANNELS.settingsReset, (event, id, options) => this.gameSettings("reset", id, undefined, undefined, options));
    ipcMain.handle(CHANNELS.liveSplitCapture, (event, id) => this.captureLiveSplit(id));
    ipcMain.handle("game-personalization-pick-asset", (event, id, kind) =>
      this.pickPersonalizationAsset(id, kind)
    );

    ipcMain.handle("player-config-import-pick", (event, id, options) => this.pickBindsConfig(id, options));
    ipcMain.handle("player-config-import-apply", (event, id, payload) => this.importPlayerConfig(id, payload));
    ipcMain.handle("player-config-import-undo", (event, id) => this.undoPlayerConfigImport(id));
    ipcMain.handle("player-config-setup-state", (event, id) => this.playerConfigSetupState(id));
    ipcMain.handle("player-config-setup-done", (event, id, choice) => this.playerConfigSetupDone(id, choice));
    ipcMain.handle("player-config-export",(event, id) => this.exportPlayerConfig(id));
    ipcMain.handle("player-config-check", (event, id, category) => this.playerConfigCheck(id, category));

    ipcMain.handle("game-personalization-preview-asset", (event, id, options) => this.readPersonalizationPreviewAsset(id, options));
    ipcMain.handle("custom-packs-get", (event, id) => this.customPacks(id));
    ipcMain.handle("custom-packs-set", (event, id, packId, enabled) => this.customPacks(id, { packId, enabled }));
    ipcMain.handle("custom-packs-sound", (event, id, packId, file, which) => this.customPackSound(id, packId, file, which));
    ipcMain.handle("ghosting-screen-get", (event, id) => this.ghostingScreen(id));

    ipcMain.handle("game-personalization-code-export", (event, id, look) => this.personalizationCode("export", id, look));
    ipcMain.handle("game-personalization-code-import", (event, id, code) => this.personalizationCode("import", id, code));

    ipcMain.handle("game-library-migrate", () => this.migrateLegacyLayout());

    ipcMain.handle("game-stats-get", (event, id) => {
      const stats = this.ensureStatsStore();
      const gameId = this.resolveGameId(id);
      if (!stats || !gameId) return { available: false };
      if (this.onlineProfiles && this.onlineProfiles.profile) {
        const snapshot = this.onlineProfiles.snapshot();
        const remote = snapshot.games && snapshot.games[gameId];
        return Object.assign({ available: true, gameId, totalPlaySeconds: 0, lastLaunchAt: null,
          launchCount: 0, isRunning: this.onlineProfiles.active.has(gameId) }, remote || {}, { source: "server" });
      }
      try {
        return stats.summary(gameId);
      } catch (e) {
        return { available: false };
      }
    });

    ipcMain.handle("game-crash-info", (event, id) => this.crashInfo(id));
    ipcMain.handle("game-crash-open-dump", (event, id) => this.openCrashDump(id));

    ipcMain.handle("open-discord", () => {
      const { shell } = require("electron");
      shell.openExternal(DISCORD_URL);
      return { status: "ok" };
    });

    ipcMain.handle("open-external-link", (event, key) => {
      const url = links.urlFor(key);

      if (!url) {
        log.warn(`[links] отклонён неизвестный ключ ссылки: ${String(key)}`);
        return { status: "error", code: "UNKNOWN_LINK" };
      }

      const { shell } = require("electron");
      shell.openExternal(url);
      return { status: "ok" };
    });

    // Вкладка «Скрипты» HL1: открываются только ссылки из данных гайда.
    ipcMain.handle("hl1-guide-open-link", (event, url) => {
      if (!hl1Guide.isGuideLink(url)) {
        log.warn("[links] отклонена ссылка не из гайда HL1");
        return { status: "error", code: "UNKNOWN_LINK" };
      }

      const { shell } = require("electron");
      shell.openExternal(url);
      return { status: "ok" };
    });

    ipcMain.handle("game-runs-list", (event, id) => this.listRuns(id));
    ipcMain.handle("game-runs-delete", (event, id, name) => this.deleteRun(id, name));

    ipcMain.handle("game-runs-open-folder", async (event, id) => {
      const location = this.runsLocation(id);
      if (!location) return { status: "error", code: "NOT_INSTALLED" };

      // Первый существующий каталог записей: языковая папка, если игра пишет
      // туда, иначе основная.
      const listed = location.dirs
        .map((relative) => runsScan.scan(location.installPath, relative, location.extension))
        .find((result) => result.status === "ok" || result.status === "no-directory");
      if (!listed || !listed.path) return { status: "error", code: "PATH_INVALID" };
      if (listed.status !== "ok") return { status: "error", code: "NO_DIRECTORY" };

      const { shell } = require("electron");
      const failure = await shell.openPath(listed.path);
      return failure ? { status: "error", code: "OPEN_FAILED" } : { status: "ok" };
    });

    ipcMain.handle("game-runs-reveal", (event, id, name) => {
      const location = this.runsLocation(id);
      if (!location) return { status: "error", code: "NOT_INSTALLED" };

      const target = location.dirs
        .map((relative) => runsScan.resolveItem(location.installPath, relative, name, location.extension))
        .find(Boolean);
      if (!target) {
        log.warn(`[runs] отклонён показ файла вне каталога записей: ${String(name)}`);
        return { status: "error", code: "PATH_INVALID" };
      }

      const { shell } = require("electron");
      shell.showItemInFolder(target);
      return { status: "ok" };
    });

    // Демо игры: список, упаковка выбранных в zip и показ архива. Путь из окна в fs не попадает.
    ipcMain.handle("game-demos-list", (event, id) => this.listGameDemos(id));
    ipcMain.handle("game-demos-pack", (event, id, items) => this.packGameDemos(id, items));
    ipcMain.handle("game-demos-archive-reveal", (event, name) => {
      const target = this.service && demoArchive.isArchiveName(name)
        ? path.join(this.service.paths.root, "Demos", name)
        : null;
      if (!target || !fs.existsSync(target)) return { status: "error", code: "NOT_FOUND" };
      const { shell } = require("electron");
      shell.showItemInFolder(target);
      return { status: "ok" };
    });
    ipcMain.handle("game-config-info", (event, id, build) => this.configInfo(id, build));

    ipcMain.handle("game-config-read", (event, id, build) => this.readGameConfig(id, build));

    ipcMain.handle("game-config-write", (event, id, build, changes, options) => this.writeGameConfig(id, build, changes, options));

    // player — основной файл игрока (HL1 userconfig.cfg, Ghosting и HL2 autoexec.cfg), список
    // помечает его меткой.
    ipcMain.handle("game-cfg-files-list", (event, id, build) =>
      this.cfgFileCall(id, build, (installPath, gameId, prepared) => Object.assign(cfgFiles.list(installPath), {
        player: prepared && prepared.file ? path.relative(installPath, prepared.file).split(path.sep).join("/") : null,
      }))
    );

    ipcMain.handle("game-cfg-file-read", (event, id, build, relative) =>
      this.cfgFileCall(id, build, (installPath) => cfgFiles.read(installPath, relative))
    );

    // Запись не ждёт закрытия игры: изменения подхватит exec или следующий
    // запуск. Только config.cfg игра перепишет при выходе, об этом говорит
    // редактор.
    ipcMain.handle("game-cfg-file-write", (event, id, build, relative, content, meta) =>
      this.cfgFileCall(id, build, (installPath, gameId) => {
        const opts = meta && typeof meta === "object" ? meta : {};
        const result = cfgFiles.write(installPath, relative, content, {
          encoding: opts.encoding,
          eol: opts.eol,
          expectedMtime: opts.mtime,
        });
        log.info(`[cfg-files] ${gameId}: сохранён ${result.path}`);
        return result;
      })
    );

    ipcMain.handle("game-build-export", (event, id) => this.exportBuild(id));
    ipcMain.handle("game-build-export-settings", (event, id) => this.exportSettings(id));
    ipcMain.handle("game-build-export-confirm", (event, token) => this.confirmPackExport(token));
    ipcMain.handle("game-build-import-confirm", (event, token) => this.confirmPackImport(token));

    ipcMain.handle("game-build-import", (event, id) => this.importBuild(id));

    ipcMain.handle("game-config-open-folder", async (event, id, build) => {
      const info = this.configInfo(id, build);
      if (!info || info.status !== "ok") return { status: "error", code: "NOT_INSTALLED" };
      if (!info.exists) return { status: "error", code: "NO_DIRECTORY" };

      const { shell } = require("electron");
      const failure = await shell.openPath(info.path);
      return failure ? { status: "error", code: "OPEN_FAILED" } : { status: "ok" };
    });

    ipcMain.handle("steam-health-get", (event, options) =>
      this.steamHealthCheck(!!(options && options.force))
    );

    ipcMain.handle("steam-account-get", () => this.steamAccountState());

    ipcMain.handle("profile-get", () => {
      return this.profileState();
    });

    ipcMain.handle("friends-list", (event, options) => {
      if (this.onlineProfiles) return this.onlineProfiles.friends({ force: !!(options && options.force === true) });
      const account = this.steamAccountState();
      if (!account.signedIn) return { signedIn: false, friends: [] };

      return { signedIn: true, friends: this.steamFriends() };
    });

    ipcMain.handle("friends-avatars", async (event, ids) => {
      const installPath = this.steamInstallPath();
      if (!Array.isArray(ids) || ids.length > 100) return {};
      ids = ids.filter((id) => typeof id === "string" && /^7656119\d{10}$/.test(id));

      let out = {};

      if (installPath) {
        try {
          out = await steamFriends.avatarsFor(installPath, ids);
        } catch (error) {
          log.warn(`[friends] аватары из кеша Steam не прочитаны: ${error.message}`);
        }
      }

      const missing = ids.filter((id) => !out[String(id)]);
      if (!missing.length) return out;

      const cacheDir = this.avatarCacheDir();

      const CONCURRENCY = 6;
      let next = 0;

      const worker = async () => {
        for (;;) {
          const at = next++;
          if (at >= missing.length) return;

          try {
            const uri = await steamAvatars.fetchAvatar(missing[at], { cacheDir });
            if (uri) out[String(missing[at])] = uri;
          } catch (ignored) {
          }
        }
      };

      await Promise.all(
        Array.from({ length: Math.min(CONCURRENCY, missing.length) }, () => worker())
      );

      return out;
    });

    ipcMain.handle("tool-list", () =>
      toolRegistry.ids().map((toolId) => this.toolCard(toolId))
    );

    // Общие каналы tool-* — только инструменты панели (REGISTRY). Зависимости
    // (FFmpeg «Демо») ставит и проверяет их владелец, запускать их нельзя.
    const unknownTool = (toolId) => (toolRegistry.manifestOf(toolId) ? null : { status: "error", error: { code: "TOOL_UNKNOWN" } });

    ipcMain.handle("tool-status", (event, toolId) => this.toolCard(toolId));
    ipcMain.handle("tool-cancel", (event, toolId) => !unknownTool(toolId) && this.tools.cancel(toolId));

    ipcMain.handle("tool-install", async (event, toolId) => {
      if (!toolRegistry.manifestOf(toolId)) return { status: "error", error: { code: "TOOL_UNKNOWN" } };

      try {
        const pending = this.tools.install(toolId, (percent) =>
          this.emitTo("tool-progress", { toolId, percent })
        );
        this.emitTo("tool-status", this.toolCard(toolId));
        const result = await pending;
        this.emitTo("tool-status", this.toolCard(toolId));
        return Object.assign({ status: "ok" }, result);
      } catch (error) {
        const delivery = toDeliveryError(error, "TOOL_INSTALL_FAILED");
        if (delivery.code === "CANCELLED") {
          this.emitTo("tool-status", this.toolCard(toolId));
          return { status: "cancelled" };
        }
        log.error(`[tools] установка ${toolId} не удалась: ${delivery.code} ${delivery.message}`);
        this.emitTo("tool-status", this.toolCard(toolId));
        return { status: "error", error: { code: delivery.code, message: delivery.message } };
      }
    });

    // options.restart — LiveSplit уже открыт для другой игры или с другими
    // сплитами, и пользователь согласился его перезапустить.
    ipcMain.handle("tool-launch", async (event, toolId, splits, gameChoice, options) => {
      if (unknownTool(toolId)) return unknownTool(toolId);
      try {
        const gameId = this.toolGameId(gameChoice);
        const preset = typeof splits === "string" && splits ? splits : this.liveSplitChoices[gameId];
        const launched = await this.tools.launch(toolId, {
          splits: preset, gameId, restart: !!(options && options.restart === true), serverPort: this.liveSplitServerPort(gameId),
        });
        if (toolId === "livesplit") this.achieve("stopwatch");
        return Object.assign({ status: "ok" }, launched);
      } catch (error) {
        const delivery = toDeliveryError(error, "TOOL_LAUNCH_FAILED");
        return { status: "error", error: { code: delivery.code, message: delivery.message } };
      }
    });

    ipcMain.handle("tool-stop", async (event, toolId, options) => {
      if (unknownTool(toolId)) return unknownTool(toolId);
      try {
        return Object.assign({ status: "ok" }, await this.tools.stop(toolId, options));
      } catch (error) {
        const delivery = toDeliveryError(error, "STOP_FAILED");
        return { status: "error", error: { code: delivery.code, message: delivery.message } };
      }
    });

    ipcMain.handle("tool-remove", (event, toolId) => {
      if (unknownTool(toolId)) return unknownTool(toolId);
      try {
        const result = this.tools.remove(toolId);
        this.emitTo("tool-status", this.toolCard(toolId));
        return Object.assign({ status: "ok" }, result);
      } catch (error) {
        const delivery = toDeliveryError(error, "TOOL_REMOVE_FAILED");
        return { status: "error", error: { code: delivery.code, message: delivery.message } };
      }
    });

    // Выбранные сплиты по играм: с ними LiveSplit стартует и вместе с игрой.
    ipcMain.handle("tool-splits-set", (event, toolId, choices) => {
      if (toolId !== toolRegistry.LIVESPLIT || !choices || typeof choices !== "object")
        return { status: "unknown-splits" };

      const next = {};
      for (const gameId of Object.keys(choices)) {
        const presets = liveSplitSplits.presetsOf(gameId);
        if (presets.some((preset) => preset.id === choices[gameId])) next[gameId] = choices[gameId];
      }
      this.liveSplitChoices = next;
      return { status: "ok" };
    });

    // Наборы сплитов для игр окна запуска LiveSplit. ids — id игр из
    // games.config; у HL2 наборы зависят от выбранной сборки.
    ipcMain.handle("tool-splits-presets", (event, ids) => {
      if (!Array.isArray(ids)) return [];
      return ids.slice(0, 32).map((id) => {
        const gameId = this.resolveGameId(String(id));
        return { id: String(id), gameId, presets: gameId ? liveSplitSplits.describe(gameId) : [] };
      });
    });

    ipcMain.handle("tool-open-folder", async (event, toolId) => {
      const { shell } = require("electron");

      if (unknownTool(toolId)) return unknownTool(toolId);
      if (!this.tools) return { status: "unavailable" };

      try {
        const state = this.tools.status(toolId);

        if (state.state !== "ready" || !state.installPath)
          return { status: "not-installed" };

        const failure = await shell.openPath(state.installPath);
        if (failure) return { status: "error", error: { code: "PATH_INVALID", message: failure } };

        return { status: "ok" };
      } catch (error) {
        const delivery = toDeliveryError(error, "PATH_INVALID");
        return { status: "error", error: { code: delivery.code, message: delivery.message } };
      }
    });

    ipcMain.handle("tool-open-help", (event, toolId) => {
      const { shell } = require("electron");
      const presentation = toolRegistry.presentationOf(toolId);

      if (!presentation || !presentation.help) return { status: "unknown-tool" };

      shell.openExternal(presentation.help);
      return { status: "ok" };
    });

    ipcMain.handle("steam-open-profile", () => {
      const { shell } = require("electron");
      const account = this.steamAccountState();
      if (!account.signedIn || !/^[0-9]{17}$/.test(String(account.steamId64 || "")))
        return { status: "unknown-account" };

      shell.openExternal(`https://steamcommunity.com/profiles/${account.steamId64}`);
      return { status: "ok" };
    });

    ipcMain.handle("tray-hide-while-playing-set", (event, enabled) => {
      if (this.tray) this.tray.setEnabled(enabled !== false);
      return { status: "ok" };
    });

    ipcMain.handle(CHANNELS.gameSelected, (event, id) => {
      const gameId = this.resolveGameId(id);
      // Сборка, а не id лаунчера: статусы приходят с id сборки (hl2-2707), и RPC сравнивает их с выбранной.
      this.emitPresence({ type: "selected", gameId: gameId || id });
      if (!gameId || !this.ensureService()) return null;
      this.lastSelectedGameId = gameId;

      this.service.checkForUpdatesIfDue(gameId, "game-selected");
      return this.service.refresh(gameId);
    });
  }

  // Порт LiveSplit Server, если в Ghosting включено «Время из LiveSplit»
  // (gh_timer_mirror): тогда LiveSplit стартует с TCP-сервером. Иначе null.
  liveSplitServerPort(gameId) {
    try {
      const store = gameId ? this.ensureSettingsStore() : null;
      const settings = store ? store.get(gameId) : null;
      const extra = settings && settings.personalization && settings.personalization.extra;
      if (!extra || extra.gh_timer_mirror !== true || extra.gh_timer === false) return null;
      const port = Number(extra.gh_timer_mirror_port);
      return Number.isInteger(port) && port > 0 && port <= 65535 ? port : 16834;
    } catch (error) {
      return null;
    }
  }

  isAnyGameRunning() {
    if (!this.service) return false;
    return this.service.processService.anyRunning();
  }

  telemetryBuildMeta(gameId) {
    try {
      const installed = this.service.installService.readInstalled(gameId);
      return {
        gameVersion: (installed && installed.version) || "0.0.0",
        buildId: (installed && installed.buildId) || "unknown",
      };
    } catch (error) {
      return { gameVersion: "0.0.0", buildId: "unknown" };
    }
  }

  telemetryGameStarting(gameId) {
    if (!this.telemetry) return;
    try {
      this.telemetry.beginGameSession(gameId, this.telemetryBuildMeta(gameId));
    } catch (error) {
      log.warn(`[telemetry] старт игры не записан: ${error.message}`);
    }
  }

  telemetryGameRunning(gameId) {
    if (!this.telemetry) return;

    try {
      this.telemetry.confirmGameRunning(gameId);

      if (this.runLogReader) {
        const installed = this.service.installService.readInstalled(gameId);
        if (installed && installed.installPath)
          this.runLogReader.attach(gameId, installed.installPath);
      }
    } catch (error) {
      log.warn(`[telemetry] запуск игры не записан: ${error.message}`);
    }
  }

  telemetryGameLaunchFailed(gameId, error) {
    if (!this.telemetry) return;
    try {
      this.telemetry.failGameLaunch(gameId, errorMap.toTelemetryCode(error));
    } catch (e) {
      log.warn(`[telemetry] отказ запуска не записан: ${e.message}`);
    }
  }

  telemetryGameFinished(gameId, lastExit, crashed) {
    if (!this.telemetry) return;

    try {
      if (crashed) {
        this.telemetry.endGameSession(gameId, { crashed: true });
        return;
      }

      const requested = !!(lastExit && lastExit.requested);
      const outcome = lastExit && lastExit.outcome;

      let exitReason = "unknown";
      if (requested) exitReason = "user-closed";
      else if (outcome === "exited") exitReason = "normal";
      else if (outcome === "crashed") exitReason = "crashed";

      this.telemetry.endGameSession(gameId, { exitReason });
    } catch (error) {
      log.warn(`[telemetry] завершение игры не записано: ${error.message}`);
    }
  }

  telemetryEmit(type, fields) {
    if (!this.telemetry) return;
    try {
      this.telemetry.emit(type, fields);
    } catch (error) {
      log.warn(`[telemetry] событие ${type} не записано: ${error.message}`);
    }
  }

  async applyUpdateWithTelemetry(gameId) {
    const before = this.telemetryBuildMeta(gameId);
    let plan = null;

    try {
      plan = this.service.updatePlan ? this.service.updatePlan(gameId) : null;
    } catch (error) {
      plan = null;
    }

    const planKind = plan && plan.kind === "patch" ? "patch" : plan && plan.kind === "patch-chain" ? "patch-chain" : "full";
    const startedMs = Date.now();

    this.telemetryEmit("game_download_started", {
      gameId,
      fromVersion: before.gameVersion,
      toVersion: (plan && plan.toVersion) || before.gameVersion,
      planKind,
    });

    try {
      const result = await this.service.applyUpdate(gameId);
      const after = this.telemetryBuildMeta(gameId);

      const bytes = toByteCount(result && result.networkBytes);
      const durationMs = toByteCount(result && result.durationMs, Math.max(0, Date.now() - startedMs));

      const patchBytes = toByteCount(result && result.patchBytes);
      const fullArchiveBytes = toByteCount(result && result.fullArchiveBytes);

      this.telemetryEmit("game_download_completed", {
        gameId,
        fromVersion: before.gameVersion,
        toVersion: after.gameVersion,
        planKind,
        downloadBytes: bytes,
        downloadDurationMilliseconds: durationMs,
        averageBytesPerSecond:
          bytes !== null && durationMs !== null && durationMs > 0
            ? Math.floor((bytes * 1000) / durationMs)
            : null,
        patchBytes,
        fullArchiveBytes,
        savedBytes:
          patchBytes !== null && fullArchiveBytes !== null
            ? Math.max(0, fullArchiveBytes - patchBytes)
            : null,
      });

      this.telemetryEmit("game_update_installed", {
        gameId,
        fromVersion: before.gameVersion,
        toVersion: after.gameVersion,
        planKind,
      });

      return result;
    } catch (error) {
      const cancelled = error && error.code === "CANCELLED";

      const failure = {
        gameId,
        fromVersion: before.gameVersion,
        toVersion: (plan && plan.toVersion) || before.gameVersion,
        planKind,
      };

      if (cancelled) {
        failure.downloadBytes = toByteCount(error && error.networkBytes);
      } else {
        failure.errorCode = errorMap.toTelemetryCode(error);
        failure.fallbackReason = errorMap.toFallbackReason(error);
      }

      this.telemetryEmit(cancelled ? "game_download_cancelled" : "game_update_failed", failure);

      throw error;
    }
  }

  async repairWithTelemetry(gameId, options) {
    const meta = this.telemetryBuildMeta(gameId);
    const startedMs = Date.now();

    this.telemetryEmit("game_repair_started", { gameId, gameVersion: meta.gameVersion });

    try {
      const result = await this.service.repair(gameId, options);

      this.telemetryEmit("game_repair_completed", {
        gameId,
        gameVersion: meta.gameVersion,
        durationSeconds: Math.max(0, Math.floor((Date.now() - startedMs) / 1000)),
      });

      return result;
    } catch (error) {
      this.telemetryEmit("game_repair_failed", {
        gameId,
        gameVersion: meta.gameVersion,
        errorCode: errorMap.toTelemetryCode(error),
      });

      throw error;
    }
  }

  // Установка, обновление, ремонт, удаление, импорт игры во время её рендера —
  // RENDER_ACTIVE (GameDeliveryService.assertNotRendering). renderActive ставит AppManager.
  guardRendering(service) {
    service.renderActive = (gameId) => typeof this.renderActive === "function" && this.renderActive(gameId);
    return service;
  }

  async launch(gameId, options = {}) {
    if (this.service.isGameRunning(gameId)) return { status: "already-running" };
    // Инструмент «Демо» рендерит эту игру в фоне (AppManager ставит проверку).
    if (typeof this.renderActive === "function" && this.renderActive(gameId))
      throw new DeliveryError("RENDER_ACTIVE", "Идёт рендер демо этой игры");
    // Mute, оставшийся после рендера «Демо», снимается до запуска (AppManager ставит хук);
    // не вышло — игра всё равно запускается.
    if (typeof this.beforeLaunch === "function")
      try {
        await this.beforeLaunch(gameId);
      } catch (error) {
        log.warn(`[game-delivery] подготовка к запуску ${gameId}: ${error.message}`);
      }

    // QA-процесс вместо игры эпизодов не знает: иначе вместо эпизода «запустилась» бы Ghosting.
    if (options && options.episode != null && qaLaunch.isEnabled())
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", "Эпизоды Ghosting в QA-запуске не запускаются", { gameId });

    this.telemetryGameStarting(gameId);

    if (qaLaunch.isEnabled()) {
      const installed = this.service.installedOf(gameId);

      this.service.processService.markStarting(gameId);

      try {
        const delay = qaLaunch.startupDelay();
        if (delay) await new Promise((resolve) => setTimeout(resolve, delay));

        const started = qaLaunch.launch(installed.installPath);

        log.warn(
          `[game-delivery] QA-РЕЖИМ: вместо игры запущен тестовый процесс ` +
            `${started.executablePath} (PID ${started.pid})`
        );

        const state = this.service.registerStartedProcess(gameId, started);
        return { status: "started", process: state };
      } catch (error) {
        this.service.processService.cancelStarting(gameId, error.message);
        this.telemetryGameLaunchFailed(gameId, error);
        throw error;
      }
    }

    this.ensurePracticeSaves(gameId);

    try {
      return await this.launchService.launch(gameId, options);
    } catch (error) {
      this.telemetryGameLaunchFailed(gameId, error);
      throw error;
    }
  }

  // Тренировочные сейвы HL1 докачиваются в фоне: ни установку, ни запуск
  // игры их отсутствие или недоступный источник не задерживает.
  ensurePracticeSaves(gameId) {
    if (!this.practiceSaves || !this.practiceSaves.packFor(gameId)) return;

    let installed;
    try {
      installed = this.service.installedOf(gameId);
    } catch (error) {
      return;
    }

    this.practiceSaves.ensure(gameId, installed.installPath).catch((error) => {
      log.warn(`[saves] тренировочные сейвы ${gameId} не скачались: ${error.message}`);
    });
  }

  requireGameId(id) {
    const gameId = this.resolveGameId(id);
    if (!gameId) throw new Error(`Неизвестная игра: ${id}`);
    return gameId;
  }

  migrateLegacyLayout() {
    const service = this.ensureService();
    if (!service) return { status: "error", error: { code: "PATH_INVALID" } };

    if (!this.legacyLayout.length) return { status: "ok", migrated: [] };

    if (service.isBusy() || service.processService.anyRunning())
      return { status: "error", error: { code: "GAME_RUNNING" } };

    const migrated = [];

    for (const entry of this.legacyLayout.slice()) {
      try {
        const result = libraryMigration.migrateLegacyGame(this.libraryPath, entry.gameId);
        migrated.push(result);
        this.legacyLayout = this.legacyLayout.filter((g) => g.gameId !== entry.gameId);
        log.info(`[game-delivery] миграция ${entry.gameId}: ${result.from} -> ${result.to}`);
      } catch (error) {
        const delivery = toDeliveryError(error, "INSTALL_FAILED");
        log.error(`[game-delivery] миграция ${entry.gameId} не удалась: ${delivery.message}`);
        this.notify(delivery.code, `миграция ${entry.gameId}: ${delivery.message}`);
        return { status: "error", error: delivery.toPublic(), migrated };
      }
    }

    service.refreshAll();
    return { status: "ok", migrated };
  }

  async handleCrash(event) {
    const gameId = event.gameId;
    const exitCode = event.lastExit.exitCode == null ? null : event.lastExit.exitCode;
    const context = this.dumpSnapshots.get(gameId) || {};

    let dumps = [];
    try {
      dumps = await crashDumps.waitForDump(
        context.installPath || null,
        context.snapshot || new Map(),
        context.startedAtMs || null,
        { gameId }
      );
    } catch (e) {
      dumps = [];
    }
    this.dumpSnapshots.delete(gameId);

    const dump = dumps.length ? dumps[0] : null;
    const logPath = dump ? crashDumps.crashLogOf(dump.path) : null;
    const archivePath = crashDumps.crashArchiveOf(context.installPath || null, context.startedAtMs || null, gameId);

    const hint = this.crashHintOf(gameId, exitCode);

    this.lastCrash.set(gameId, {
      at: new Date().toISOString(),
      exitCode,
      dumpPath: dump ? dump.path : null,
      dumpSize: dump ? dump.size : null,
      logPath,
      archivePath,
      installPath: context.installPath || null,
      hint,
    });

    if (hint) log.warn(`[crash] ${gameId}: вероятная причина — ${hint}`);

    if (dump)
      log.warn(`[crash] дамп ${gameId} найден: ${dump.path} (${dump.size} байт)`);
    else log.warn("[crash] crash dump не найден в разрешённых каталогах");
    if (archivePath) log.warn(`[crash] архив сбоя ${gameId}: ${archivePath}`);

    if (this.notifications)
      try {
        this.notifications.add({
          level: "error",
          code: "GAME_CRASHED",
          detail: `код выхода ${exitCode == null ? "?" : exitCode}`,
          meta: { hasDump: !!(dump || archivePath), exitCode, gameId, launcherId: manifestRegistry.launcherIdOf(gameId) || null, hint },
        });
      } catch (e) {
        log.warn(`[crash] уведомление не записано: ${e.message}`);
      }

    // Окно о краше: уведомление в центре остаётся, а окно — чтобы игрок
    // точно увидел его и прислал лог. Спрятанное в трей окно трей покажет
    // сам после выхода игры; свёрнутое или закрытое другими — выводим вперёд.
    this.emitTo(CHANNELS.crashReport, {
      gameId,
      launcherId: manifestRegistry.launcherIdOf(gameId) || null,
      exitCode,
      hasDump: !!(dump || archivePath),
      // Архив сбоя (Ghosting) — один файл с логом и дампом: просим его.
      archive: !!archivePath,
      file: archivePath || logPath || (dump ? dump.path : null),
      hint,
    });
    if (this.window && !this.window.isDestroyed()) {
      if (this.window.isMinimized()) this.window.restore();
      if (this.window.isVisible()) this.window.focus();
    }
  }

  crashHintOf(gameId, exitCode) {
    const config = manifestRegistry.LAUNCH_CONFIG[gameId];
    if (!config || config.engine !== "goldsrc" || !this.launchService) return null;
    if (typeof this.launchService.depCrashHint !== "function") return null;
    try {
      return this.launchService.depCrashHint(exitCode);
    } catch (e) {
      return null;
    }
  }

  runsLocation(id) {
    const gameId = this.resolveGameId(id);
    const config = gameId ? manifestRegistry.localConfigOf(gameId) : null;
    if (!config || !config.runsPath) return null;

    const installPath = this.installedPathOf(gameId);
    if (!installPath) return null;

    // Демо HL1 с языком игры, отличным от английского, ложатся в языковую
    // папку мода (valve_WON_russian): записи ищутся и там.
    const dirs = config.runsLanguageDirs ? runsScan.runDirs(installPath, config.runsPath) : [config.runsPath];
    return { gameId, installPath, relative: config.runsPath, dirs, extension: config.runsExtension };
  }

  // Файл игрока готов к чтению и записи: переход с 0.4.8 или создание отсутствующего.
  // Зовётся первым там, где раньше звали ensureDefaultConfig, — иначе вкладки увидят раскладку 0.4.8.
  preparePlayerConfig(gameId) {
    const installPath = this.installedPathOf(gameId);
    const launchConfig = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    if (!installPath || !launchConfig) return null;
    try {
      const store = this.ensureSettingsStore();
      const result = defaultConfigs.prepareConfig(gameId, installPath, launchConfig, {
        settings: store ? store.get(gameId) : null,
        launcherVersion: this.launcherVersion,
      });
      return result ? { installPath, gameDirectory: launchConfig.gameDirectory, file: result.file } : null;
    } catch (error) {
      log.warn(`[player-config] ${gameId}: файл игрока не подготовлен: ${error.message}`);
      return null;
    }
  }

  // Опрос при первом запуске: спрашивать ли и запись ответа. Ошибки не мешают запуску.
  playerConfigSetupState(id) {
    const gameId = this.resolveGameId(id);
    const installPath = this.installedPathOf(gameId);
    const launchConfig = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    if (!installPath || !launchConfig || !launchConfig.gameDirectory) return { needed: false };
    try {
      return playerConfig.setupState(installPath, gameId, launchConfig.gameDirectory, { launcherVersion: this.launcherVersion });
    } catch (error) {
      log.warn(`[player-config] ${gameId}: состояние опроса не прочитано: ${error.message}`);
      return { needed: false };
    }
  }

  playerConfigSetupDone(id, choice) {
    const gameId = this.resolveGameId(id);
    const installPath = this.installedPathOf(gameId);
    const launchConfig = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    if (!installPath || !launchConfig || !launchConfig.gameDirectory) return { status: "error", code: "NOT_INSTALLED" };
    try {
      const store = this.ensureSettingsStore();
      playerConfig.setupDone(installPath, gameId, launchConfig.gameDirectory, choice, {
        settings: store ? store.get(gameId) : null,
        launcherVersion: this.launcherVersion,
      });
      log.info(`[player-config] ${gameId}: ответ на опрос — ${choice}`);
      return { status: "ok" };
    } catch (error) {
      log.warn(`[player-config] ${gameId}: ответ на опрос не записан: ${error.code || error.message}`);
      return { status: "error", code: error.code === "CONFIG_SETUP_CHOICE" ? "BAD_CHOICE" : "PATH_NOT_WRITABLE" };
    }
  }

  installedPathOf(gameId) {
    if (!gameId) return null;

    try {
      const installed = this.service.installService.readInstalled(gameId);
      return installed && installed.installPath ? installed.installPath : null;
    } catch (error) {
      return null;
    }
  }

  buildLabelOf(launcherId, gameId) {
    const entry = (manifestRegistry.BUILDS_BY_LAUNCHER_ID[String(launcherId)] || []).find(
      (item) => item.gameId === gameId
    );

    return entry ? entry.build : null;
  }

  configBuilds(launcherId) {
    return (manifestRegistry.BUILDS_BY_LAUNCHER_ID[String(launcherId)] || []).map((item) => ({
      build: item.build,
      installed: !!this.installedPathOf(item.gameId),
    }));
  }

  listRuns(id) {
    const location = this.runsLocation(id);
    if (!location) return { status: "not-installed", items: [] };

    return runsScan.scanMany(location.installPath, location.dirs, location.extension);
  }

  // Игра и её папки с демо; null — игра не установлена или у неё нет демо.
  demoContext(id) {
    const gameId = this.resolveGameId(id);
    const config = gameId ? manifestRegistry.localConfigOf(gameId) : null;
    const installPath = gameId ? this.installedPathOf(gameId) : null;
    if (!config || !installPath || !demoArchive.demoSources(config, installPath).dirs.length) return null;
    return { gameId, config, installPath };
  }

  // Установленные игры с папками демо — для инструмента «Демо».
  demoContexts() {
    if (!this.ensureService()) return [];
    return Object.keys(manifestRegistry.MANIFESTS).map((gameId) => this.demoContext(gameId)).filter(Boolean);
  }

  listGameDemos(id) {
    const context = this.demoContext(id);
    if (!context) return { status: "error", code: "NOT_INSTALLED" };
    try {
      return { status: "ok", demos: demoArchive.listDemos(context.config, context.installPath) };
    } catch (error) {
      log.warn(`[demos] ${context.gameId}: список не прочитан: ${error.message}`);
      return { status: "error", code: "LIST_FAILED" };
    }
  }

  async packGameDemos(id, requested) {
    const context = this.demoContext(id);
    if (!context || !this.ensureService()) return { status: "error", code: "NOT_INSTALLED" };
    if (this.demoPackBusy) return { status: "error", code: "PACK_BUSY" };
    const picked = demoArchive.pickItems(requested, demoArchive.listDemos(context.config, context.installPath));
    if (!picked.ok) return { status: "error", code: picked.code };
    this.demoPackBusy = true;
    try {
      const archive = await demoArchive.packDemos({
        installPath: context.installPath,
        items: picked.items,
        archivesDir: path.join(this.service.paths.root, "Demos"),
        slug: context.gameId === "ghosting-renewed" ? "ghosting" : "hl1",
        now: new Date(),
        archive: this.service.archiveService,
        onProgress: (percent) => this.emitTo("game-demos-pack-progress", { gameId: id, percent }),
      });
      return { status: "ok", archive };
    } catch (error) {
      log.warn(`[demos] ${context.gameId}: упаковка не удалась: ${error.message}`);
      return { status: "error", code: error.code === "DEMO_NOT_FOUND" ? "DEMO_NOT_FOUND" : "PACK_FAILED" };
    } finally {
      this.demoPackBusy = false;
    }
  }
  deleteRun(id, name) {
    const location = this.runsLocation(id);
    if (!location) return { status: "error", code: "NOT_INSTALLED" };
    if (this.service.isGameRunning(location.gameId)) return { status: "error", code: "GAME_RUNNING" };
    const relative = (location.dirs || [location.relative]).find((dir) =>
      runsScan.resolveItem(location.installPath, dir, name, location.extension)) || location.relative;
    return runsScan.deleteItem(location.installPath, relative, name, location.extension);
  }

  // Сборка, чьи конфиги показывает вкладка «Конфиги»: выбранная или
  // установленная сборка игры.
  configGameId(id, build) {
    const key = String(id);
    const launcherId = manifestRegistry.launcherIdOf(this.resolveGameId(key) || key) || key;
    if (build) return manifestRegistry.buildGameIdOf(launcherId, build) || null;
    let gameId = this.resolveGameId(key);
    if (gameId && !this.installedPathOf(gameId)) {
      const installed = (manifestRegistry.BUILDS_BY_LAUNCHER_ID[launcherId] || []).find(
        (item) => this.installedPathOf(item.gameId)
      );
      if (installed) gameId = installed.gameId;
    }
    return gameId || null;
  }

  // Экспорт сборки целиком: папка игры с cfg, сейвами и демо и настройки
  // лаунчера в одном архиве .hlsrpack (game-pack.js). Сборку из Steam
  // скачивает Steam — у неё переносятся только настройки и cfg.
  async exportBuild(id) {
    const gameId = this.resolveGameId(id);
    if (!this.ensureService()) return { status: "error", code: "NOT_READY" };
    const installed = gameId ? this.service.installService.readInstalled(gameId) : null;
    if (!installed) return { status: "error", code: "NOT_INSTALLED" };
    if (manifestRegistry.isDepotBuild(gameId)) return this.exportSettings(gameId);
    if (this.service.isGameRunning(gameId)) return { status: "error", code: "GAME_RUNNING" };
    if (this.service.isBusy()) return { status: "error", code: "BUSY" };

    const launcherId = manifestRegistry.launcherIdOf(gameId) || gameId;
    const stamp = new Date().toISOString().slice(0, 10);
    const { dialog } = require("electron");
    const result = await dialog.showSaveDialog(this.window, {
      defaultPath: `${gameId}${installed.version ? "-" + installed.version : ""}-${stamp}.${gamePack.EXTENSION}`,
      filters: [{ name: `HLSR (*.${gamePack.EXTENSION})`, extensions: [gamePack.EXTENSION] }],
    });
    if (result.canceled || !result.filePath) return { status: "canceled" };
    const filePath = path.extname(result.filePath) ? result.filePath : `${result.filePath}.${gamePack.EXTENSION}`;

    return this.runPackExport(gameId, filePath, {
      launcherId,
      build: this.buildLabelOf(launcherId, gameId),
      launcherVersion: this.launcherVersion,
      settings: this.ensureSettingsStore() ? this.ensureSettingsStore().get(gameId) : null,
    });
  }

  // Изменённые файлы сборки (PACK_MODIFIED): окно предупреждает, путь к архиву
  // и сведения остаются в main, окно возвращает одноразовый токен (confirmPackExport).
  async runPackExport(gameId, filePath, extra) {
    try {
      const exported = await this.service.exportPack(gameId, filePath, extra);
      return { status: "ok", kind: "pack", gameId, size: exported.size, path: exported.path };
    } catch (error) {
      if (error && error.code === "PACK_MODIFIED") {
        const token = crypto.randomBytes(16).toString("hex");
        this.pendingPackExport = { token, gameId, filePath, extra };
        const details = error.details || {};
        return { status: "confirmation-required", kind: "pack-modified", token, gameId, modified: details.modified, custom: !!details.custom };
      }
      return this.packError(error, "PACK_FAILED", `${gameId}: экспорт не удался`);
    }
  }

  async confirmPackExport(token) {
    const pending = this.pendingPackExport;
    this.pendingPackExport = null;
    if (!pending || typeof token !== "string" || token !== pending.token) return { status: "error", code: "PACK_FAILED" };
    if (!this.ensureService()) return { status: "error", code: "NOT_READY" };
    if (this.service.isGameRunning(pending.gameId)) return { status: "error", code: "GAME_RUNNING" };
    if (this.service.isBusy()) return { status: "error", code: "BUSY" };
    return this.runPackExport(pending.gameId, pending.filePath, Object.assign({}, pending.extra, { allowModified: true }));
  }

  // Экспорт только настроек игры и cfg игрока в файл .hlsrbuild
  // (build-transfer.js).
  async exportSettings(id) {
    const gameId = this.resolveGameId(id);
    const installed = gameId && this.service ? this.service.installService.readInstalled(gameId) : null;
    if (!installed || !installed.installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      let previous = null;
      try {
        previous = installed.version ? manifestRegistry.filesManifestOf(gameId, installed.version) : null;
      } catch (error) {
        previous = null;
      }
      const files = buildTransfer.collect(installed.installPath, previous);
      const launcherId = manifestRegistry.launcherIdOf(gameId) || gameId;
      const build = this.buildLabelOf(launcherId, gameId);
      const stamp = new Date().toISOString().slice(0, 10);
      const { dialog } = require("electron");
      const result = await dialog.showSaveDialog(this.window, {
        defaultPath: `${gameId}${installed.version ? "-" + installed.version : ""}-${stamp}.${buildTransfer.EXTENSION}`,
        filters: [{ name: `HLSR (*.${buildTransfer.EXTENSION})`, extensions: [buildTransfer.EXTENSION] }],
      });
      if (result.canceled || !result.filePath) return { status: "canceled" };
      const text = buildTransfer.serialize({
        launcherId,
        gameId,
        build,
        version: installed.version || null,
        launcherVersion: this.launcherVersion,
        settings: this.ensureSettingsStore() ? this.ensureSettingsStore().get(gameId) : null,
        files,
      });
      await fs.promises.writeFile(result.filePath, text, "utf8");
      log.info(`[build-transfer] ${gameId}: экспорт в ${result.filePath}, cfg: ${files.length}`);
      return { status: "ok", gameId, files: files.length, path: result.filePath };
    } catch (error) {
      if (error instanceof buildTransfer.TransferError) {
        if (error.file) log.warn(`[build-transfer] ${gameId}: экспорт не удался: ${error.message}`);
        return { status: "error", code: error.code, file: error.file || null };
      }
      log.warn(`[build-transfer] ${gameId}: экспорт не удался: ${error.message}`);
      return { status: "error", code: "IO_FAILED" };
    }
  }

  // Импорт сборки: настройки сохраняются сразу, cfg — в установленную
  // сборку. Не установленная сборка ставится, и cfg ложатся после установки
  // (applyPendingBuildImport). Обновление проверяет окно после импорта.
  async importBuild() {
    if (!this.ensureService()) return { status: "error", code: "NOT_READY" };
    let parsed;
    let text;
    try {
      const { dialog } = require("electron");
      const result = await dialog.showOpenDialog(this.window, {
        properties: ["openFile"],
        filters: [
          { name: `HLSR (*.${gamePack.EXTENSION}, *.${buildTransfer.EXTENSION})`, extensions: [gamePack.EXTENSION, buildTransfer.EXTENSION] },
        ],
      });
      if (result.canceled || !result.filePaths.length) return { status: "canceled" };
      if (path.extname(result.filePaths[0]).toLowerCase() === `.${gamePack.EXTENSION}`)
        return this.importPack(result.filePaths[0]);
      const stat = await fs.promises.stat(result.filePaths[0]);
      if (!stat.isFile() || stat.size > 24 * 1024 * 1024) return { status: "error", code: "TOO_LARGE" };
      text = await fs.promises.readFile(result.filePaths[0], "utf8");
      parsed = buildTransfer.parse(text, { isKnownGame: (gameId) => manifestRegistry.isKnownGame(gameId) });
    } catch (error) {
      if (error instanceof buildTransfer.TransferError) return { status: "error", code: error.code };
      log.warn(`[build-transfer] файл сборки не прочитан: ${error.message}`);
      return { status: "error", code: "IO_FAILED" };
    }

    const gameId = parsed.gameId;
    if (this.service.isGameRunning(gameId)) return { status: "error", code: "GAME_RUNNING" };
    if (this.service.isBusy()) return { status: "error", code: "BUSY" };

    if (parsed.settings && this.ensureSettingsStore()) {
      const saved = this.ensureSettingsStore().saveImported(gameId, parsed.settings);
      if (!saved.ok) log.warn(`[build-transfer] ${gameId}: настройки из файла отклонены (${saved.error && saved.error.code})`);
    }
    const launcherId = manifestRegistry.launcherIdOf(gameId) || gameId;
    if (launcherId !== gameId) this.selectedBuilds.set(launcherId, gameId);

    const installPath = this.installedPathOf(gameId);
    try {
      if (installPath) {
        const written = buildTransfer.applyFiles(installPath, parsed.files);
        log.info(`[build-transfer] ${gameId}: импортировано cfg: ${written.length}`);
        return { status: "ok", gameId, launcherId, installed: true, files: written.length, fromVersion: parsed.version };
      }
      const file = this.pendingBuildImportFile(gameId);
      if (!file) return { status: "error", code: "IO_FAILED" };
      await fs.promises.mkdir(path.dirname(file), { recursive: true });
      await fs.promises.writeFile(file, text, "utf8");
      log.info(`[build-transfer] ${gameId}: cfg (${parsed.files.length}) лягут после установки`);
      return { status: "ok", gameId, launcherId, installed: false, files: parsed.files.length, fromVersion: parsed.version };
    } catch (error) {
      if (error instanceof buildTransfer.TransferError) return { status: "error", code: error.code };
      log.warn(`[build-transfer] ${gameId}: импорт не удался: ${error.message}`);
      return { status: "error", code: "IO_FAILED" };
    }
  }

  // Импорт сборки из архива .hlsrpack. Установленная сборка заменяется
  // целиком, поэтому сперва подтверждение в окне: путь к архиву остаётся в
  // main, окно возвращает только одноразовый токен (confirmPackImport).
  async importPack(file) {
    let meta;
    try {
      meta = await this.service.readPackMeta(file);
    } catch (error) {
      return this.packError(error, "PACK_BAD_FORMAT", `архив ${file} не прочитан`);
    }

    if (this.service.isGameRunning(meta.gameId)) return { status: "error", code: "GAME_RUNNING" };
    if (this.service.isBusy()) return { status: "error", code: "BUSY" };
    if (!meta.installedVersion) return this.runPackImport(file, meta);

    const token = crypto.randomBytes(16).toString("hex");
    this.pendingPackImport = { token, file };
    return {
      status: "confirmation-required",
      kind: "pack",
      token,
      gameId: meta.gameId,
      version: meta.version,
      installedVersion: meta.installedVersion,
    };
  }

  async confirmPackImport(token) {
    const pending = this.pendingPackImport;
    this.pendingPackImport = null;
    if (!pending || typeof token !== "string" || token !== pending.token) return { status: "error", code: "PACK_FAILED" };
    if (!this.ensureService()) return { status: "error", code: "NOT_READY" };
    let meta;
    try {
      meta = await this.service.readPackMeta(pending.file);
    } catch (error) {
      return this.packError(error, "PACK_BAD_FORMAT", `архив ${pending.file} не прочитан`);
    }
    return this.runPackImport(pending.file, meta);
  }

  async runPackImport(file, meta) {
    const gameId = meta.gameId;
    let imported;
    try {
      imported = await this.service.importPack(file, meta);
    } catch (error) {
      return this.packError(error, "PACK_FAILED", `${gameId}: импорт не удался`);
    }

    if (meta.settings && this.ensureSettingsStore()) {
      const saved = this.ensureSettingsStore().saveImported(gameId, meta.settings);
      if (!saved.ok) log.warn(`[game-pack] ${gameId}: настройки из архива отклонены (${saved.error && saved.error.code})`);
    }
    const launcherId = manifestRegistry.launcherIdOf(gameId) || gameId;
    if (launcherId !== gameId) this.selectedBuilds.set(launcherId, gameId);
    return {
      status: "ok",
      kind: "pack",
      gameId,
      launcherId,
      installed: true,
      version: imported.version,
      dropped: imported.dropped,
      modified: imported.modified || null,
    };
  }

  packError(error, fallback, context) {
    const delivery = toDeliveryError(error, fallback);
    if (delivery.code === "CANCELLED") return { status: "canceled" };
    log.warn(`[game-pack] ${context}: ${delivery.code} ${delivery.message}`);
    return { status: "error", code: delivery.code, error: delivery.toPublic() };
  }

  pendingBuildImportFile(gameId) {
    if (!this.userDataPath || !manifestRegistry.isKnownGame(gameId)) return null;
    return path.join(this.userDataPath, "build-imports", `${gameId}.${buildTransfer.EXTENSION}`);
  }

  applyPendingBuildImport(gameId) {
    const file = gameId ? this.pendingBuildImportFile(gameId) : null;
    if (!file || !fs.existsSync(file)) return;
    const installPath = this.installedPathOf(gameId);
    if (!installPath) return;
    try {
      const parsed = buildTransfer.parse(fs.readFileSync(file, "utf8"), { isKnownGame: (id) => id === gameId });
      const written = buildTransfer.applyFiles(installPath, parsed.files);
      log.info(`[build-transfer] ${gameId}: после установки импортировано cfg: ${written.length}`);
      this.emitTo("game-build-import-applied", { gameId, files: written.length });
    } catch (error) {
      log.warn(`[build-transfer] ${gameId}: отложенный импорт не удался: ${error.code || error.message}`);
      this.emitTo("game-build-import-applied", { gameId, error: error.code || "IO_FAILED" });
    }
    try {
      fs.rmSync(file, { force: true });
    } catch (error) {
      /* следующий успех попробует снова */
    }
  }

  // Редактор cfg-файлов: действие над папкой установленной сборки.
  cfgFileCall(id, build, action) {
    const gameId = this.configGameId(id, build);
    if (!gameId || !manifestRegistry.launchConfigOf(gameId)) return { status: "error", code: "UNSUPPORTED" };
    const installPath = this.installedPathOf(gameId);
    if (!installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      const prepared = this.preparePlayerConfig(gameId);
      return Object.assign({ status: "ok", gameId, running: !!(this.service && this.service.isGameRunning(gameId)) }, action(installPath, gameId, prepared));
    } catch (error) {
      if (error instanceof cfgFiles.CfgFileError) return { status: "error", code: error.code };
      log.warn(`[cfg-files] ${gameId}: ${error.message}`);
      return { status: "error", code: "IO_FAILED" };
    }
  }

  // Редактор конфигов: значения настроек сборки из её cfg.
  readGameConfig(id, build) {
    try {
      const gameId = this.configGameId(id, build);
      const launchConfig = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
      const engine = gameId ? bindPresets.engineFor(gameId) : null;
      if (!launchConfig || !engine) return { status: "unsupported" };
      const installPath = this.installedPathOf(gameId);
      if (!installPath) return { status: "not-installed", gameId, engine };
      this.preparePlayerConfig(gameId);
      const result = gameConfigInstall.readConfig(installPath, launchConfig, gameId, engine);
      if (!result) return { status: "unsupported" };
      return {
        status: "ok",
        gameId,
        engine,
        running: !!(this.service && this.service.isGameRunning(gameId)),
        values: result.values,
        sources: result.sources,
        sha256: result.sha256,
        playerFile: result.playerFile,
        shadowed: result.shadowed,
      };
    } catch (error) {
      log.warn(`[game-config] конфиг не прочитан: ${error.message}`);
      return { status: "error", code: "READ_FAILED" };
    }
  }

  // Правки пишутся в файл игрока, а его запущенная игра не переписывает (при выходе она пишет
  // только config.cfg), поэтому запись идёт и при запущенной игре, как у «Клавиатуры».
  // options.expectedSha256 — отпечаток файла игрока из game-config-read.
  writeGameConfig(id, build, changes, options) {
    const gameId = this.configGameId(id, build);
    const launchConfig = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    const engine = gameId ? bindPresets.engineFor(gameId) : null;
    if (!launchConfig || !engine) return { status: "error", code: "UNSUPPORTED" };
    const installPath = this.installedPathOf(gameId);
    if (!installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      this.preparePlayerConfig(gameId);
      const expected = options && typeof options.expectedSha256 === "string" ? { expectedSha256: options.expectedSha256 } : {};
      const result = gameConfigInstall.writeConfig(installPath, launchConfig, gameId, engine, changes, expected);
      if (result.written.length) log.info(`[game-config] ${gameId}: записаны ${result.written.join(", ")}`);
      return { status: "ok", sha256: result.sha256 };
    } catch (error) {
      if (error && error.code === "CONFIG_CHANGED") {
        log.warn(`[game-config] ${gameId}: файл игрока изменён вне лаунчера, правка отклонена`);
        return { status: "error", code: "CONFIG_CHANGED" };
      }
      if (error instanceof gameConfigInstall.ConfigError) {
        log.warn(`[game-config] ${gameId}: правка отклонена (${error.message})`);
        return { status: "error", code: error.code, cvar: error.cvar };
      }
      log.error(`[game-config] ${gameId}: запись не удалась: ${error.message}`);
      return { status: "error", code: "WRITE_FAILED" };
    }
  }

  configInfo(id, build) {
    const key = String(id);
    const launcherId = manifestRegistry.launcherIdOf(this.resolveGameId(key) || key) || key;
    const builds = this.configBuilds(launcherId);

    let gameId;

    if (build) {
      gameId = manifestRegistry.buildGameIdOf(launcherId, build);
      if (!gameId)
        return { status: "unknown-build", exists: false, build: String(build), builds };
    } else {
      gameId = this.resolveGameId(key);

      if (gameId && !this.installedPathOf(gameId)) {
        const installed = (manifestRegistry.BUILDS_BY_LAUNCHER_ID[launcherId] || []).find(
          (item) => this.installedPathOf(item.gameId)
        );

        if (installed) gameId = installed.gameId;
      }
    }

    const config = gameId ? manifestRegistry.localConfigOf(gameId) : null;
    if (!config || !config.configPath)
      return { status: "unsupported", exists: false, build: null, builds };

    const buildLabel = this.buildLabelOf(launcherId, gameId);
    const installPath = this.installedPathOf(gameId);

    if (!installPath)
      return { status: "not-installed", exists: false, build: buildLabel, builds };

    let directory;
    try {
      directory = paths.assertInside(
        installPath,
        path.join(installPath, config.configPath.replace(/\//g, path.sep)),
        "Каталог конфигов"
      );
    } catch (error) {
      return { status: "error", code: "PATH_INVALID", exists: false, build: buildLabel, builds };
    }

    return {
      status: "ok",
      path: directory,
      exists: fs.existsSync(directory),
      build: buildLabel,
      builds,
    };
  }

  crashInfo(id) {
    if (id == null || id === "") return this.latestCrash();

    const gameId = this.resolveGameId(id);
    if (!gameId) return this.latestCrash();

    return this.lastCrash.get(gameId) || null;
  }

  latestCrash() {
    let newest = null;

    for (const info of this.lastCrash.values())
      if (!newest || String(info.at) > String(newest.at)) newest = info;

    return newest;
  }

  openCrashDump(id) {
    const info = this.crashInfo(id);
    if (info && info.archivePath && crashDumps.isAllowedDumpPath(info.archivePath, info.installPath)) {
      require("electron").shell.showItemInFolder(info.archivePath);
      return { status: "ok" };
    }
    if (!info || !info.dumpPath) return { status: "error", error: { code: "NO_DUMP" } };

    if (!crashDumps.isAllowedDumpPath(info.dumpPath, info.installPath))
      return { status: "error", error: { code: "PATH_INVALID" } };

    // Текстовый лог лежит рядом с дампом (crashLogOf) — показываем его.
    const shown = info.logPath && path.dirname(info.logPath) === path.dirname(info.dumpPath) ? info.logPath : info.dumpPath;
    const { shell } = require("electron");
    shell.showItemInFolder(shown);
    return { status: "ok" };
  }

  async startTimerWithGame(gameId) {
    if (!this.tools) return;

    const store = this.ensureSettingsStore();
    if (!store) return;

    try {
      const settings = store.get(gameId);
      if (!settings || settings.startTimer !== true) return;

      const status = this.tools.status(toolRegistry.LIVESPLIT);
      if (status.state !== "ready") {
        log.info("[tools] автозапуск таймера пропущен: он не установлен");
        return;
      }

      const splits = this.liveSplitChoices[gameId];

      await this.tools.launch(toolRegistry.LIVESPLIT, { splits, gameId, serverPort: this.liveSplitServerPort(gameId) });
      log.info(`[tools] таймер запущен вместе с ${gameId} (сплиты: ${splits || "по умолчанию"})`);
    } catch (error) {
      log.warn(`[tools] таймер не запустился вместе с игрой: ${error.message}`);
    }
  }

  toolCard(toolId) {
    const presentation = toolRegistry.presentationOf(toolId) || {};
    const manifest = toolRegistry.manifestOf(toolId);

    let state = { toolId, state: "unknown", version: null, updateAvailable: false };

    try {
      if (this.tools && manifest) state = this.tools.status(toolId);
    } catch (error) {
      log.warn(`[tools] состояние ${toolId} не прочитано: ${error.message}`);
    }

    return Object.assign({}, state, {
      shortName: presentation.shortName || toolId,
      title: presentation.title || toolId,
      subtitle: presentation.subtitle || null,
      description: presentation.description || null,
      icon: presentation.icon || "fas fa-toolbox",
      accent: presentation.accent || null,
      accentSoft: presentation.accentSoft || null,
      hasHelp: !!presentation.help,
      sizeBytes: manifest && manifest.archive ? manifest.archive.size : null,
    });
  }

  startSteamConnectionWatch() {
    const schedule = (delay) => {
      if (this.steamConnectionTimer) clearTimeout(this.steamConnectionTimer);

      this.steamConnectionTimer = setTimeout(tick, delay);
      if (typeof this.steamConnectionTimer.unref === "function")
        this.steamConnectionTimer.unref();
    };

    const tick = () => {
      // В Windows опрос Steam идёт через PowerShell и во время игры
      // откладывается; в Linux он читает /proc и почти ничего не стоит.
      if (!IS_LINUX && this.isAnyGameRunning()) return schedule(STEAM_CONNECTION_TICK_MS);

      if (!this.isWindowVisible()) return schedule(STEAM_CONNECTION_TICK_MS);

      try {
        this.steamAccount.refreshConnection();
      } catch (error) {
        log.warn(`[steam] состояние подключения не обновлено: ${error.message}`);
      }

      const state = this.pushSteamAccount();

      if (state && state.signedIn) {
        this.steamMissWindow = 0;
        return schedule(STEAM_CONNECTION_TICK_MS);
      }

      this.steamMissWindow = (this.steamMissWindow || 0) + 1;

      schedule(
        this.steamMissWindow > STEAM_SIGNED_OUT_FAST_TICKS
          ? STEAM_CONNECTION_TICK_MS
          : STEAM_SIGNED_OUT_TICK_MS
      );
    };

    schedule(STEAM_CONNECTION_FIRST_MS);
  }

  pushSteamAccount() {
    const state = this.steamAccountState();

    try {
      this.emitTo("steam-account", state);
    } catch (ignored) {
    }

    return state;
  }

  profileState() {
    if (this.onlineProfiles) return this.onlineProfiles.snapshot();
    const account = this.steamAccountState();

    let firstRunAt = null;
    try {
      firstRunAt = firstRun.ensureFirstRunAt(this.userDataPath, { logger: log });
    } catch (error) {
      log.warn(`[profile] дата первого запуска не определена: ${error.message}`);
    }

    let games = {};
    try {
      const store = this.ensureStatsStore();
      const all = store ? store.readAll() : null;
      games = (all && all.games) || {};
    } catch (error) {
      log.warn(`[profile] статистика не прочитана: ${error.message}`);
    }

    const list = achievements.evaluate({ games, firstRunAt });
    const best = achievements.bestGame(games);

    let totalSeconds = 0;
    for (const gameId of Object.keys(games)) {
      const value = Number(games[gameId] && games[gameId].totalPlaySeconds);
      if (Number.isFinite(value) && value > 0) totalSeconds += value;
    }

    return {
      account,
      firstRunAt,
      achievements: list,
      summary: achievements.summarize(list),
      stats: {
        totalSeconds,
        gamesPlayed: Object.keys(games).filter(
          (id) => Number(games[id] && games[id].totalPlaySeconds) > 0
        ).length,
        best: best
          ? { launcherId: manifestRegistry.launcherIdOf(best.gameId), gameId: best.gameId, seconds: best.seconds }
          : null,
      },
    };
  }

  steamAccountState() {
    const empty = {
      signedIn: false,
      steamId64: null,
      steamId2: null,
      personaName: null,
      avatarDataUri: null,
      connection: "unknown",
    };

    if (!this.steamAccount) return empty;

    try {
      return this.steamAccount.describe();
    } catch (error) {
      log.warn(`[steam] аккаунт не определён: ${error.message}`);
      return empty;
    }
  }

  steamHealthCheck(force) {
    const now = Date.now();

    if (!force && this.steamHealth.checkedAt && now - this.steamHealth.checkedAt < 60 * 1000)
      return this.steamHealth;

    if (!this.ensureService() || !this.launchService) {
      this.steamHealth = {
        status: "unknown",
        code: "STEAM_CHECK_FAILED",
        checkedAt: now,
      };
      return this.steamHealth;
    }

    try {
      // В Linux LaunchService проверяет Proton, а здоровье Steam — только локатор.
      const result = IS_LINUX
        ? this.steamLocator.describeSteamOnly()
        : this.launchService.describeAvailability("ghosting-renewed");

      const steam = result.steam || null;
      const detail = steam
        ? [
            steam.installPath || "",
            steam.pid ? `PID ${steam.pid}` : "",
            steam.source ? `опознан: ${steam.source}` : "",
          ]
            .filter(Boolean)
            .join(", ")
        : result.message || "";

      this.steamHealth = {
        status: result.ok ? "available" : "unavailable",
        code: result.code || null,
        detail,
        checkedAt: now,
      };
    } catch (error) {
      this.steamHealth = {
        status: "unknown",
        code: "STEAM_CHECK_FAILED",
        detail: error && error.message ? error.message : "",
        checkedAt: now,
      };
    }

    this.emitTo("steam-health", this.steamHealth);
    return this.steamHealth;
  }

  requiredSteamApps() {
    return manifestRegistry.requiredOwnedApps();
  }

  async steamOwnership(appId, options) {
    this.ensureService();
    if (!this.depotService) return { appId, owned: null, code: "STEAM_CHECK_FAILED" };

    try {
      return await this.depotService.ownershipOf(appId, options);
    } catch (error) {
      return {
        appId,
        owned: null,
        code: (error && error.code) || "STEAM_CHECK_FAILED",
        message: (error && error.message) || String(error),
      };
    }
  }

  // Каталог паков HUD и прицелов: только у сборок, где они объявлены (HL1).
  gamePacks(gameId) {
    try {
      const config = manifestRegistry.launchConfigOf(gameId);
      return config && config.packs ? { packs: goldsrcPacks.catalog() } : {};
    } catch (error) {
      log.warn(`[game-settings] каталог паков не прочитан: ${error.message}`);
      return {};
    }
  }

  // Бинды из cfg игры для вкладки «Клавиатура». bindsGameId — сборка, чьи
  // заготовки показывать: у HL2 окно настроек открывается по id лаунчера.
  gameBinds(gameId) {
    const info = { bindsGameId: gameId };
    try {
      const config = manifestRegistry.launchConfigOf(gameId);
      const engine = bindPresets.engineFor(gameId);
      if (!config || !engine || !this.service) return info;
      const installed = this.service.installService.readInstalled(gameId);
      if (!installed || !installed.installPath) return info;
      this.preparePlayerConfig(gameId);
      const keyBinds = gameBindsInstall.readInstallBinds(installed.installPath, config.gameDirectory, engine);
      // Файлы подключённого импорта: по ним окно знает, что конфиг подключён
      // и какие бинды пришли из него.
      const record = engine === "goldsrc" ? gameBindsInstall.readImportRecord(installed.installPath) : null;
      if (keyBinds && record) keyBinds.imported = record.files.map((item) => item.path.toLowerCase());
      return keyBinds ? Object.assign(info, { keyBinds }) : info;
    } catch (error) {
      log.warn(`[game-settings] бинды не прочитаны: ${error.message}`);
      return info;
    }
  }

  gameLanguages(gameId) {
    try {
      if (!this.service) return [];

      const installed = this.service.installService.readInstalled(gameId);
      if (!installed || !installed.installPath) return [];

      const config = manifestRegistry.launchConfigOf(gameId);
      if (!config || !config.gameDirectory) return [];

      return gameLanguage.availableLanguages(installed.installPath, config.gameDirectory);
    } catch (error) {
      log.warn(`[game-settings] языки сборки не прочитаны: ${error.message}`);
      return [];
    }
  }

  // Переключатель Vulkan (vulkan-backend.js): null — у игры его нет, иначе
  // есть ли DXVK в установленной сборке.
  gameVulkan(gameId) {
    try {
      const config = manifestRegistry.launchConfigOf(gameId);
      if (!vulkanBackend.supported(config, process.platform)) return null;
      const installed = this.service ? this.service.installService.readInstalled(gameId) : null;
      return vulkanBackend.availability(installed && installed.installPath, config, fs, process.platform);
    } catch (error) {
      log.warn(`[game-settings] DXVK в сборке не проверен: ${error.message}`);
      return null;
    }
  }

  readPersonalizationPreviewAsset(id, options) {
    const gameId = this.resolveGameId(id);
    if (gameId !== "ghosting-renewed" || !options || !["font", "spray", "model"].includes(options.kind))
      return { status: "error", code: "INVALID_ASSET" };
    try {
      const installed = this.service && this.service.installService.readInstalled(gameId);
      if (!installed || !installed.installPath) return { status: "error", code: "NOT_INSTALLED" };
      // Модель призрака: геометрия для предпросмотра (ghost-model.js), ищется там же, где игра.
      if (options.kind === "model") return { status: "ok", mesh: ghostModel.readGhostMesh(installed.installPath, options.value) };
      // Fonts installed in Windows are resolved by the renderer itself; only
      // assets imported into the mod need to be read here. In Linux the game
      // sees fonts of its Proton prefix, so the prefix is passed along.
      const prefix = this.protonRunner ? this.protonRunner.prefixFor(gameId) : null;
      return Object.assign({ status: "ok" }, ghostingAssets.readPreviewAsset(installed.installPath, options.kind, options.value, prefix));
    } catch (error) { return { status: "error", code: error.code || "ASSET_READ_FAILED" }; }
  }

  // Разрешение Ghosting для предпросмотра HUD: из настроек лаунчера или
  // последнее сохранённое игрой (source-screen.js). null — неизвестно.
  async ghostingScreen(id) {
    const gameId = this.resolveGameId(id);
    if (gameId !== "ghosting-renewed") return null;
    try {
      const store = this.ensureSettingsStore();
      const prefix = this.protonRunner ? this.protonRunner.prefixFor(gameId) : null;
      return await sourceScreen.screenOf({ settings: store ? store.get(gameId) : null, mod: "ghostingmod", prefix });
    } catch (error) {
      return null;
    }
  }

  // Выбор cfg для импорта биндов. Файл только читается: разбирает его и
  // показывает итог вкладка «Клавиатура», сохраняется он обычным путём.
  // Выбор cfg для импорта (клавиатура и вкладка «Конфиги»). options.want — файл цепочки exec ({ name, from }):
  // окно подписано его именем и открывается сразу на нём в папке
  // предыдущего выбранного файла (options.dir).
  async pickBindsConfig(id, options = {}) {
    const gameId = this.resolveGameId(id);
    if (!gameId) return { status: "error", code: "UNKNOWN_GAME" };
    try {
      const { dialog } = require("electron");
      const installed = this.service && this.service.installService.readInstalled(gameId);
      const want = options && options.want && typeof options.want.name === "string" ? options.want : null;
      const dir = options && typeof options.dir === "string" && path.isAbsolute(options.dir) ? options.dir : null;
      const base = dir || (installed && installed.installPath) || undefined;
      const multiple = !want && !!(options && options.multiple);
      const result = await dialog.showOpenDialog(this.window, {
        title: want
          ? `${String(want.from || "").slice(0, 120)} → exec ${want.name.slice(0, 120)}`
          : undefined,
        properties: multiple ? ["openFile", "multiSelections"] : ["openFile"],
        defaultPath: want && base ? path.join(base, path.basename(want.name)) : base,
        filters: [{ name: "Config (*.cfg)", extensions: ["cfg", "rc"] }, { name: "*", extensions: ["*"] }],
      });
      if (result.canceled || !result.filePaths.length) return { status: "canceled" };
      if (result.filePaths.length > cfgImport.MAX_FILES) return { status: "error", code: "BINDS_FILE_TOO_LARGE" };
      const files = [];
      for (const file of result.filePaths) {
        const stat = await fs.promises.stat(file);
        if (!stat.isFile() || stat.size > 1024 * 1024) return { status: "error", code: "BINDS_FILE_TOO_LARGE" };
        // Движки читают cfg побайтно: latin1 не теряет байты и не падает на
        // кириллице в комментариях. BOM и UTF-16 Блокнота снимает cfgText.
        files.push({ name: path.basename(file), dir: path.dirname(file), text: gameBinds.cfgText(await fs.promises.readFile(file, "latin1")) });
      }
      return Object.assign({ status: "ok", files }, files[0]);
    } catch (error) {
      log.warn(`[binds] импорт cfg не прочитан: ${error.code || error.message}`);
      return { status: "error", code: "BINDS_FILE_UNREADABLE" };
    }
  }

  // Импорт готового конфига одним файлом (src/shared/cfg-import.js flattenImport):
  // цепочка exec сливается в файл игрока, файлы, которые зовут alias и bind, ложатся
  // в папку мода, прежнее уходит в .hlsr/backup/<время>/. payload — { root, files,
  // missing }, options.expectedSha256 — отпечаток, который видело окно.
  importPlayerConfig(id, payload, options = {}) {
    const gameId = this.resolveGameId(id);
    const config = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    if (!config || !bindPresets.engineFor(gameId)) return { status: "error", code: "UNKNOWN_GAME" };
    const request = typeof payload === "string" ? { root: { name: "player.cfg", text: payload } } : payload;
    const files = request && request.files && typeof request.files === "object" ? request.files : {};
    const valid = request && request.root && typeof request.root.text === "string" &&
      typeof request.root.name === "string" && Object.keys(files).length <= cfgImport.MAX_FILES &&
      Object.values(files).every((file) => file && typeof file.text === "string") &&
      (request.missing === undefined || Array.isArray(request.missing));
    if (!valid) return { status: "error", code: "BINDS_FILE_UNREADABLE" };
    const installPath = this.installedPathOf(gameId);
    if (!installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      const built = cfgImport.flattenImport({
        engine: bindPresets.engineFor(gameId), root: request.root, files, missing: request.missing || [],
      });
      if (built.bytes > gameBinds.MAX_IMPORT_BYTES) return { status: "error", code: "BINDS_FILE_TOO_LARGE" };
      // Запущенная игра не мешает: при выходе она переписывает только config.cfg.
      this.preparePlayerConfig(gameId);
      const result = gameBindsInstall.importPlayerConfig(installPath, gameId, config.gameDirectory, built,
        { expectedSha256: options.expectedSha256, backupsFile: this.playerConfigBackupsFile() });
      if (result.status === "changed") return { status: "error", code: "CONFIG_CHANGED" };
      if (result.status !== "ok") return { status: "error", code: "BINDS_FILE_UNREADABLE" };
      this.dropImportedOverrides(gameId);
      log.info(`[player-config] ${gameId}: конфиг импортирован, копия ${result.backup}, файлы ${result.files.join(", ") || "-"}`);
      return Object.assign({ status: "ok", summary: built.summary, backup: result.backup, sha256: result.sha256, report: built.report },
        this.gameBinds(gameId));
    } catch (error) {
      if (error && error.code === "CONFIG_CHANGED") return { status: "error", code: "CONFIG_CHANGED" };
      log.warn(`[player-config] импорт не записан: ${error.code || error.message}`);
      return { status: "error", code: "PATH_NOT_WRITABLE" };
    }
  }

  // Импорт заменяет файл целиком, и главный теперь он: все переназначения и назначения скриптов
  // игры снимаются из хранилища, «Клавиатура» и «Скрипты» показывают то, что в файле. «Вернуть
  // прежний» их не возвращает: восстановленный файл уже содержит прежние бинды лаунчера.
  dropImportedOverrides(gameId) {
    // Файл заменён целиком: «прежние бинды» правленых вручную блоков к нему уже не относятся.
    if (this.editedPrevious) delete this.editedPrevious[gameId];
    try {
      const store = this.ensureSettingsStore();
      const saved = store && store.get(gameId);
      if (saved && saved.binds && Object.keys(saved.binds).length) store.save(gameId, Object.assign({}, saved, { binds: {} }));
    } catch (error) {
      log.warn(`[player-config] ${gameId}: переназначения после импорта не сняты: ${error.message}`);
    }
  }

  // Какую копию импорта записал сам лаунчер (game-binds-install.js): в папке лаунчера, не игры.
  playerConfigBackupsFile() {
    return this.userDataPath ? path.join(this.userDataPath, "player-config-backups.json") : null;
  }

  // «Вернуть прежний»: файл игрока и файлы скриптов из последней копии импорта.
  undoPlayerConfigImport(id) {
    const gameId = this.resolveGameId(id);
    const config = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    const installPath = gameId ? this.installedPathOf(gameId) : null;
    if (!config || !installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      this.preparePlayerConfig(gameId);
      const result = gameBindsInstall.undoImport(installPath, gameId, config.gameDirectory,
        { backupsFile: this.playerConfigBackupsFile() });
      if (result.status === "nothing") return { status: "error", code: "NO_BACKUP" };
      log.info(`[player-config] ${gameId}: возвращено из ${result.backup}`);
      return Object.assign({ status: "ok", sha256: result.sha256, backup: result.backup }, this.gameBinds(gameId));
    } catch (error) {
      log.warn(`[player-config] возврат не удался: ${error.code || error.message}`);
      return { status: "error", code: "PATH_NOT_WRITABLE" };
    }
  }

  // Проверка конфига HL1 по правилам speedrun.com (src/shared/hl1-config-check.js): только чтение.
  // Ответ { status: "ok" | "nothing" | "unsupported" | "error", findings, stats, file }.
  // category — "scriptless" (по умолчанию, если не передана) или "scripted"; другое значение — BAD_REQUEST.
  playerConfigCheck(id, category = "scriptless") {
    if (category !== "scriptless" && category !== "scripted") return { status: "error", code: "BAD_REQUEST" };
    const gameId = this.resolveGameId(id);
    const config = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    if (gameId !== "hl1-won" || !config) return { status: "unsupported" };
    const installPath = this.installedPathOf(gameId);
    if (!installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      return gameBindsInstall.checkPlayerConfig(installPath, gameId, config.gameDirectory, { category });
    } catch (error) {
      log.warn(`[player-config] ${gameId}: проверка по правилам не выполнена: ${error.code || error.message}`);
      return { status: "error", code: "CHECK_FAILED" };
    }
  }

  // Экспорт файла игрока (с файлами скриптов, если он их зовёт) туда, куда укажет игрок.
  async exportPlayerConfig(id) {
    const gameId = this.resolveGameId(id);
    const config = gameId ? manifestRegistry.launchConfigOf(gameId) : null;
    const installPath = gameId ? this.installedPathOf(gameId) : null;
    if (!config || !installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      this.preparePlayerConfig(gameId);
      const { dialog } = require("electron");
      const result = await gameBindsInstall.exportPlayerConfig(installPath, gameId, config.gameDirectory, {
        chooseDirectory: async () => {
          const picked = await dialog.showOpenDialog(this.window, { properties: ["openDirectory", "createDirectory"] });
          return picked.canceled || !picked.filePaths.length ? null : picked.filePaths[0];
        },
        chooseFile: async (name) => {
          const picked = await dialog.showSaveDialog(this.window, {
            defaultPath: name,
            filters: [{ name: "Config (*.cfg)", extensions: ["cfg"] }],
          });
          return picked.canceled || !picked.filePath ? null : picked.filePath;
        },
      });
      if (result.status === "canceled") return { status: "canceled" };
      if (result.status !== "ok") return { status: "error", code: "NOT_INSTALLED" };
      log.info(`[player-config] ${gameId}: экспорт в ${result.path}, файлов ${result.files}`);
      return { status: "ok", path: result.path };
    } catch (error) {
      log.warn(`[player-config] экспорт не удался: ${error.code || error.message}`);
      return { status: "error", code: "PATH_NOT_WRITABLE" };
    }
  }

  // Моды игры в персонализации (custom-packs.js: HL1 и Ghosting): список, включение и
  // выключение. Файлы меняются только при закрытой игре и без другой работы
  // с играми (установка, проверка, починка).
  customPacks(id, change) {
    const gameId = this.resolveGameId(id);
    if (!customPacks.hasPacks(gameId)) return { status: "error", code: "UNKNOWN_GAME" };
    const installPath = this.installedPathOf(gameId);
    const blocked = !this.ensureService() ? "NOT_READY"
      : this.service.isGameRunning(gameId) ? "GAME_RUNNING"
      : this.service.isBusy() ? "BUSY" : null;
    try {
      if (change && installPath && !blocked) {
        if (!customPacks.PACKS.some((pack) => pack.id === change.packId && pack.game === gameId))
          return { status: "error", code: "CUSTOM_PACK_UNKNOWN" };
        const packs = customPacks.setEnabled(installPath, change.packId, !!change.enabled, { logger: log });
        return { status: "ok", packs };
      }
      const packs = customPacks.list(installPath, gameId, { recover: !blocked, logger: log });
      if (!change) return { status: "ok", installed: !!installPath, packs };
      return { status: "error", code: installPath ? blocked : "NOT_INSTALLED", packs };
    } catch (error) {
      log.warn(`[custom-packs] ${change ? `пак ${change.packId}` : "список"}: ${error.code || ""} ${error.message}`);
      let packs = null;
      try { packs = customPacks.list(installPath, gameId); } catch (ignored) { /* ответ без списка */ }
      return { status: "error", code: error.code || "CUSTOM_PACK_FAILED", packs };
    }
  }

  customPackSound(id, packId, file, which) {
    const gameId = this.resolveGameId(id);
    const installPath = customPacks.hasPacks(gameId) ? this.installedPathOf(gameId) : null;
    if (!installPath) return { status: "error", code: "NOT_INSTALLED" };
    try {
      if (!customPacks.PACKS.some((pack) => pack.id === packId && pack.game === gameId)) return { status: "error", code: "CUSTOM_PACK_UNKNOWN" };
      return Object.assign({ status: "ok" }, customPacks.readSound(installPath, packId, file, which));
    } catch (error) {
      return { status: "error", code: error.code === "ENOENT" ? "CUSTOM_PACK_NOT_AVAILABLE" : error.code || "CUSTOM_PACK_FAILED" };
    }
  }

  async pickPersonalizationAsset(id, kind) {
    const gameId = this.resolveGameId(id);
    if (gameId !== "ghosting-renewed" || !["font", "spray", "model"].includes(kind)) {
      return { status: "error", code: "INVALID_ASSET" };
    }
    try {
      const installed = this.service && this.service.installService.readInstalled(gameId);
      if (!installed || !installed.installPath) return { status: "error", code: "NOT_INSTALLED" };
      const { dialog } = require("electron");
      // Модель призрака не копируется: игра видит только модели внутри мода, поэтому
      // выбирается файл там (ghostingmod/models, ghostingmod/custom/<пак>/models).
      if (kind === "model") {
        // Своих моделей в ghostingmod/models обычно нет (папки нет) — тогда окно открывается в custom.
        const mod = path.join(installed.installPath, "ghostingmod");
        const picked = await dialog.showOpenDialog(this.window, {
          properties: ["openFile"],
          defaultPath: [path.join(mod, "models"), path.join(mod, "custom")].find((dir) => fs.existsSync(dir)) || mod,
          filters: [{ name: "Source model (*.mdl)", extensions: ["mdl"] }],
        });
        if (picked.canceled || !picked.filePaths.length) return { status: "canceled" };
        return { status: "ok", value: ghostModel.valueOfPicked(installed.installPath, picked.filePaths[0]) };
      }
      const result = await dialog.showOpenDialog(this.window, {
        properties: ["openFile"],
        filters: kind === "font"
          ? [{ name: "TrueType / OpenType", extensions: ["ttf", "otf"] }]
          : [{ name: "PNG / BMP / TGA", extensions: ["png", "bmp", "tga"] }],
      });
      if (result.canceled || !result.filePaths.length) return { status: "canceled" };
      const current = this.service.installService.readInstalled(gameId);
      if (!current || current.installPath !== installed.installPath) {
        return { status: "error", code: "NOT_INSTALLED" };
      }
      const imported = await ghostingAssets.importAsset(installed.installPath, result.filePaths[0], kind);
      return Object.assign({ status: "ok" }, imported);
    } catch (error) {
      log.warn(`[personalization] asset import failed: ${error.code || "ASSET_IMPORT_FAILED"}`);
      return { status: "error", code: error.code || "ASSET_IMPORT_FAILED" };
    }
  }

  // Экспорт и проверка импорта не записывают ни настройки, ни игровые файлы.
  // Редактор показывает импортированный снимок и сохраняет его обычным путём.
  // Настройки внешнего вида с поправкой на то, что игрок написал в своём файле.
  // Нет игры на диске или файл не читается - настройки как есть.
  // configSha256 - отпечаток файла, по которому построены настройки: окно вернёт его при сохранении.
  inspectPlayerPersonalization(gameId, stored) {
    try {
      const installed = this.service ? this.service.installService.readInstalled(gameId) : null;
      const config = manifestRegistry.launchConfigOf(gameId);
      if (!installed || !installed.installPath || !config) return { settings: stored };
      const { settings, sha256 } = playerConfig.inspectPersonalization(installed.installPath, gameId, config.gameDirectory, stored);
      return sha256 === undefined ? { settings } : { settings, configSha256: sha256 };
    } catch (error) {
      log.warn(`[personalization] файл игрока не прочитан для ${gameId}: ${error.message}`);
      return { settings: stored };
    }
  }

  readPlayerPersonalization(gameId, stored) {
    return this.inspectPlayerPersonalization(gameId, stored).settings;
  }

  personalizationCode(action, id, value) {
    try {
      const gameId = this.resolveGameId(id);
      if (action === "export") {
        // Окно присылает черновик. Пока он не отличается от сохранённого, код берёт
        // то, что сейчас в файле игрока (его могли поправить руками после открытия окна).
        let look = value;
        const store = this.ensureSettingsStore();
        if (store && gameId) {
          const saved = store.get(gameId);
          const draft = Object.assign({}, saved, value);
          const lookOf = (settings) => JSON.stringify([settings.personalization, settings.packs, settings.bxt]);
          const same = lookOf(draft) === lookOf(saved);
          if (same) look = this.readPlayerPersonalization(gameId, saved);
        }
        // Моды с вкладки модов включаются сразу, без черновика: их состояние — то, что на диске.
        const installPath = customPacks.hasPacks(gameId) ? this.installedPathOf(gameId) : null;
        if (installPath) {
          try {
            look = Object.assign({}, look, { mods: Object.fromEntries(customPacks.list(installPath, gameId).map((pack) => [pack.id, pack.enabled])) });
          } catch (error) { log.warn(`[personalization-code] моды не вошли в код: ${error.code || ""} ${error.message}`); }
        }
        return { status: "ok", code: personalizationCode.encode(gameId, look) };
      }
      // Моды отдельно от вида: вид идёт в черновик до «Сохранить», моды окно переключает сразу.
      const { mods, ...look } = personalizationCode.decode(gameId, value);
      return { status: "ok", look, mods: mods || null };
    } catch (error) {
      return { status: "error", error: { code: error instanceof personalizationCode.PersonalizationCodeError ? error.code : "PERSONALIZATION_CODE_INVALID" } };
    }
  }

  gameSettings(action, id, settings, scope, options) {
    const store = this.ensureSettingsStore();
    if (!store) return { status: "error", error: { code: "SETTINGS_UNAVAILABLE" } };

    const gameId = this.resolveGameId(id);
    if (!gameId) return { status: "error", error: { code: "UNKNOWN_GAME" } };

    try {
      this.preparePlayerConfig(gameId);
      if (action === "get") {
        const bindsInfo = this.gameBinds(gameId);
        const fromStore = store.get(gameId);
        const stored = bindsInfo.keyBinds
          ? Object.assign({}, fromStore, {
              binds: gameBinds.pruneOverrides(fromStore.binds, bindsInfo.keyBinds.binds),
            })
          : fromStore;
        const inspected = this.inspectPlayerPersonalization(gameId, stored);
        return Object.assign(
          {
            status: "ok",
            settings: inspected.settings,
            languages: this.gameLanguages(gameId),
            vulkan: this.gameVulkan(gameId),
          },
          "configSha256" in inspected ? { configSha256: inspected.configSha256 } : {},
          this.gamePacks(gameId),
          bindsInfo
        );
      }

      if (action === "save") {
        if (scope !== undefined && scope !== "personalization")
          return { status: "error", error: { code: "SETTINGS_SCOPE_INVALID" } };
        let next = settings;
        if (scope === "personalization") {
          // Панель могла загрузиться до смены биндов или параметров запуска.
          // Обновляем только внешний вид поверх актуальных настроек в main.
          const patch = {
            personalization: settings && settings.personalization,
            liveSplitOverlay: settings && settings.liveSplitOverlay,
          };
          if (gameId === "hl1-won") {
            patch.packs = settings && settings.packs;
            patch.bxt = settings && settings.bxt;
          }
          next = Object.assign({}, store.get(gameId), patch);
        }
        const previous = store.get(gameId);
        // Оверлей LiveSplit меняет только панель персонализации. Окно
        // настроек игры могло открыться раньше и прислать устаревшую копию:
        // раньше сохранение биндов или запуска сбрасывало оверлей.
        if (scope === undefined)
          next = Object.assign({}, previous, settings, { liveSplitOverlay: previous.liveSplitOverlay });
        const normalized = launchArgs.normalizeSettings(next);
        if (!normalized.ok) return { status: "error", error: normalized.error };
        // Файл игрока пишется до хранилища: не записался (CONFIG_CHANGED, права) - бинды в
        // хранилище не попадают, и следующее сохранение возьмёт прежние как previous.
        const removeEdited = !!(options && options.removeEdited);
        this.editedPrevious = this.editedPrevious || {};
        const previousBinds = (removeEdited && this.editedPrevious[gameId]) || previous.binds;
        // options.reset — «Сбросить» в «Персонализации»: умолчания пишутся явными строками (§5).
        const { keptEdited, configSha256 } = this.applyPersonalizationIfInstalled(gameId, normalized.settings, {
          previousBinds, removeEdited, previousSettings: previous, expectedSha256: options && options.configSha256,
          explicitPersonalization: !!(options && options.reset),
        });
        const result = store.save(gameId, next);
        if (!result.ok) return { status: "error", error: result.error };
        // Блоки, оставленные из-за правок: ответ «Удалить» придёт уже после записи в хранилище.
        if (keptEdited.length) this.editedPrevious[gameId] = previousBinds;
        else delete this.editedPrevious[gameId];
        if (JSON.stringify(previous.binds) !== JSON.stringify(result.settings.binds)) this.achieve("own-binds");
        const look = (value) => JSON.stringify([value.personalization, value.packs, value.bxt]);
        if (look(previous) !== look(result.settings)) this.achieve("own-style");
        return Object.assign({ status: "ok", settings: result.settings, keptEdited }, configSha256 === undefined ? {} : { configSha256 });
      }

      // Как в save: файл игрока пишется до хранилища, ошибка записи видна в окне.
      const previous = store.get(gameId);
      const defaults = launchArgs.defaultSettings();
      if (gameId === "ghosting-renewed") defaults.personalization.extra = resetExtras(previous.personalization.extra);
      const normalized = launchArgs.normalizeSettings(defaults);
      if (!normalized.ok) return { status: "error", error: normalized.error };
      this.editedPrevious = this.editedPrevious || {};
      const { keptEdited, configSha256 } = this.applyPersonalizationIfInstalled(gameId, normalized.settings, {
        previousBinds: previous.binds, previousSettings: previous, expectedSha256: options && options.configSha256,
      });
      const result = gameId === "ghosting-renewed" ? store.save(gameId, defaults) : store.reset(gameId);
      if (!result.ok) return { status: "error", error: result.error };
      if (keptEdited.length) this.editedPrevious[gameId] = previous.binds;
      else delete this.editedPrevious[gameId];
      return Object.assign({ status: "ok", settings: result.settings, keptEdited }, configSha256 === undefined ? {} : { configSha256 });
    } catch (error) {
      log.warn(`[game-settings] действие ${action} не выполнено: ${error.message}`);
      return { status: "error", error: { code: error && error.code === "CONFIG_CHANGED" ? "CONFIG_CHANGED" : "SETTINGS_IO_ERROR" } };
    }
  }

  // Снимок окна LiveSplit для предпросмотра оверлея. На Linux снимок
  // делается в префиксе, где запущен LiveSplit: окна других префиксов не
  // видны. prefixGameId говорит окну персонализации, для какой игры он
  // запущен (в другой игре оверлей его не увидит).
  captureLiveSplit(id) {
    const gameId = this.resolveGameId(id);
    if (!gameId) return Promise.resolve({ status: "error", error: { code: "UNKNOWN_GAME" } });
    if (!this.liveSplitCapture)
      this.liveSplitCapture = this.runLiveSplitCapture(gameId).finally(() => { this.liveSplitCapture = null; });
    return this.liveSplitCapture;
  }

  async runLiveSplitCapture(gameId) {
    // Windows: снимок находит любой LiveSplit.exe, как и оверлей в игре, —
    // запущенный и не из лаунчера. Linux: только свой, его префикс известен.
    const launched = this.tools && this.tools.runningInfo(ToolService.LIVESPLIT_TOOL_ID);
    const running = launched || (IS_LINUX ? null : {});
    if (!running) return { status: "not-running" };
    const tool = this.liveSplitOverlay && this.liveSplitOverlay.captureTool();
    if (!tool) return { status: "error", error: { code: "LIVESPLIT_OVERLAY_UNAVAILABLE" } };
    const prefixGameId = IS_LINUX ? running.gameId || gameId : null;
    // Окно персонализации знает игру по id лаунчера, а префикс назван по
    // gameId, поэтому сравниваем здесь, где оба приведены к gameId. На
    // Windows LiveSplit тоже открыт для одной игры: оверлей других игр
    // выключен (livesplit-overlay.js, bindTo).
    const boundGameId = running.gameId || prefixGameId;
    const otherGame = !!boundGameId && boundGameId !== gameId;
    for (const file of [tool.snapshot, tool.status]) {
      try { fs.rmSync(file, { force: true }); } catch (ignored) { /* файла не было */ }
    }
    let code;
    try {
      if (IS_LINUX) {
        if (!this.protonRunner) return { status: "error", error: { code: "PROTON_NOT_INSTALLED" } };
        code = await this.runLinuxLiveSplitCapture(prefixGameId, tool);
      } else {
        code = await this.liveSplitWindowCapture.capture(tool.snapshot);
      }
    } catch (error) {
      log.warn(`[livesplit-overlay] снимок LiveSplit не сделан: ${error.message}`);
      return { status: "error", error: { code: "LIVESPLIT_CAPTURE_FAILED" }, prefixGameId, otherGame };
    }
    // 2 — окно LiveSplit не найдено (свёрнуто или ещё открывается).
    // Не из лаунчера и окна нет — значит, LiveSplit просто не запущен.
    if (code === 2) return { status: launched ? "no-window" : "not-running", prefixGameId, otherGame };
    if (code !== 0) {
      log.warn(`[livesplit-overlay] снимок LiveSplit не сделан: ${code === null ? "нет ответа" : `код ${code}`}`);
      return { status: "error", error: { code: "LIVESPLIT_CAPTURE_FAILED" }, prefixGameId, otherGame };
    }
    const snapshot = this.liveSplitOverlay.readSnapshot(tool.snapshot);
    if (!snapshot) return { status: "error", error: { code: "LIVESPLIT_CAPTURE_FAILED" }, prefixGameId, otherGame };
    return {
      status: "ok",
      width: snapshot.width,
      height: snapshot.height,
      pixels: new Uint8Array(snapshot.pixels),
      prefixGameId,
      otherGame,
    };
  }

  // Под Wine окно LiveSplit снимается только изнутри LiveSplit: 64-битный
  // помощник внедряет в него hlsr-share64.dll и берёт кадр из общей памяти.
  // Сессия umu не завершается, пока в префиксе работает LiveSplit: ждём файл
  // статуса и закрываем сессию сами.
  async runLinuxLiveSplitCapture(prefixGameId, tool) {
    const { windowsPathOf } = require("../linux/ProtonRunner");
    const { child } = await this.protonRunner.launch(prefixGameId, tool.injector64,
      ["capture", "--out", windowsPathOf(tool.snapshot), "--status", windowsPathOf(tool.status),
        "--dll", windowsPathOf(tool.shareDll)],
      { cwd: tool.dir, skipFonts: true, env: { PROTON_HIDE_PROCESS_WINDOW: "1" } });
    if (typeof child.unref === "function") child.unref();
    let exited = false;
    child.once("exit", () => { exited = true; });
    try {
      return await this.liveSplitOverlay.waitForStatus(tool.status, 20000, { cancelled: () => exited });
    } finally {
      if (!exited && child.pid) {
        try { process.kill(-child.pid, "SIGTERM"); } catch (ignored) { /* уже завершился */ }
      }
    }
  }

  // Оверлей LiveSplit показывается только в игре, для которой LiveSplit
  // запущен из лаунчера (livesplit-overlay.js, bindTo).
  bindLiveSplitOverlay() {
    if (!this.liveSplitOverlay || !this.tools) return;
    const running = this.tools.runningInfo(toolRegistry.LIVESPLIT);
    const store = this.ensureSettingsStore();
    const changed = this.liveSplitOverlay.bindTo(running && running.gameId, (gameId) => {
      try {
        return store ? store.get(gameId) : null;
      } catch (error) {
        return null;
      }
    });
    if (changed) log.info(`[livesplit-overlay] оверлей привязан к ${(running && running.gameId) || "любой игре"}`);
  }

  // previousBinds — переназначения до сохранения (что пропало, уходит из файла игрока).
  // Возвращает { keptEdited }: блоки скриптов, правленные руками и оставленные в файле.
  // previousSettings - настройки до сохранения: из них и settings берутся изменённые строки персонализации.
  applyPersonalizationIfInstalled(gameId, settings, { previousBinds, removeEdited = false, previousSettings, expectedSha256, explicitPersonalization = false } = {}) {
    let keptEdited = [];
    let configSha256;
    let writing = false;
    // Оверлей LiveSplit в запущенной игре перечитывает overlay.ini сам.
    try {
      if (settings && this.liveSplitOverlay) this.liveSplitOverlay.writeConfig(gameId, settings);
    } catch (error) {
      log.warn(`[livesplit-overlay] настройки не записаны для ${gameId}: ${error.message}`);
    }
    try {
      if (!settings || !this.service) return { keptEdited };
      const installed = this.service.installService.readInstalled(gameId);
      if (!installed || !installed.installPath) return { keptEdited };
      // Бинды, скрипты и персонализация правятся в файле игрока сразу, одной записью,
      // в том числе при запущенной игре.
      const config = manifestRegistry.launchConfigOf(gameId);
      if (config && bindPresets.engineFor(gameId)) {
        writing = true;
        const applied = gameBindsInstall.applyBinds(installed.installPath, gameId, config.gameDirectory, settings.binds, {
          previous: previousBinds,
          removeEdited,
          settings,
          previousSettings,
          expectedSha256,
          explicitPersonalization,
        });
        keptEdited = applied.keptEdited || [];
        configSha256 = applied.sha256;
      }
    } catch (error) {
      // Файл игрока не записался: вызывающий отвечает ошибкой, а не «сохранено».
      if (writing) throw error;
      log.warn(`[personalization] не применена для ${gameId}: ${error.message}`);
    }
    return { keptEdited, configSha256 };
  }

  async run(action) {
    if (!this.ensureService())
      return { status: "error", error: this.initError ? this.initError.toPublic() : null };

    try {
      const result = await action();
      return Object.assign({ status: "ok" }, result || {});
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");
      log.warn(`[game-delivery] действие меню не выполнено: ${delivery.code} ${delivery.message}`);

      this.notify(delivery.code, delivery.message);

      return { status: "error", error: delivery.toPublic() };
    }
  }

  async uninstallGame(gameId, options) {
    const result = await this.service.uninstallGame(gameId, {
      confirmed: !!(options && options.confirmed === true),
    });

    if (result.status === "confirmation-required") return result;

    let shortcut = { removed: false, reason: "skipped" };
    try {
      shortcut = this.actions.removeDesktopShortcut(gameId);
    } catch (error) {
      log.warn(`[game-delivery] ярлык ${gameId} не удалён: ${error.message}`);
    }

    return Object.assign({}, result, { shortcut });
  }

  menuState(id) {
    const service = this.ensureService();
    const gameId = this.resolveGameId(id);

    if (!service || !gameId)
      return { available: false, installed: false, busy: false, running: false, items: {} };

    const installed = !!service.installService.readInstalled(gameId);
    const busy = service.isBusy();
    const running = service.isGameRunning(gameId);

    const fileAction = (needsInstall) => {
      if (running) return { enabled: false, reason: "game-running" };
      if (needsInstall && !installed) return { enabled: false, reason: "not-installed" };
      if (busy) return { enabled: false, reason: "busy" };
      return { enabled: true, reason: null };
    };

    const steamManaged = manifestRegistry.isDepotBuild(gameId);
    const manifestAction = (needsInstall) =>
      steamManaged ? { enabled: false, reason: "steam-managed" } : fileAction(needsInstall);

    const inDevelopment = { enabled: false, reason: "in-development" };
    // Проверка и ремонт по файлам включены по списку: у остальных игр пока нет зеркала.
    const fileRepair = steamManaged
      ? manifestAction(true)
      : manifestRegistry.isFileRepairGame(gameId) ? fileAction(true) : inDevelopment;
    // Настройки открываются и при запущенной игре: окно само блокирует то,
    // что трогает файлы игры (импорт конфига, удаление забегов), а
    // остальное применяется при следующем запуске. Без установки — выключены: «Персонализация»
    // показывает только установленные игры и открыла бы вместо этой прошлую.
    const settings = !installed ? { enabled: false, reason: "not-installed" } : busy ? { enabled: false, reason: "busy" } : { enabled: true, reason: null };

    return {
      available: true,
      gameId,
      installed,
      busy,
      running,
      items: {
        settings,
        shortcut: { enabled: installed, reason: installed ? null : "not-installed" },
        screenshots: { enabled: installed, reason: installed ? null : "not-installed" },
        folder: { enabled: installed, reason: installed ? null : "not-installed" },
        updates: manifestAction(false),
        verify: fileRepair,
        repair: fileRepair,
        exportBuild: steamManaged ? manifestAction(true) : fileAction(true),
        exportSettings: installed ? { enabled: true, reason: null } : { enabled: false, reason: "not-installed" },
        importBuild: busy ? { enabled: false, reason: "busy" } : running ? { enabled: false, reason: "game-running" } : { enabled: true, reason: null },
        uninstall: fileAction(true),
      },
    };
  }

  recoverPending() {
    for (const gameId of Object.keys(manifestRegistry.MANIFESTS)) {
      const fullUpdate = this.service.pendingFullUpdate(gameId);

      if (fullUpdate) {
        log.warn(
          `[game-delivery] найдена незавершённая транзакция обновления ${gameId} ` +
            `(${fullUpdate.fromVersion} -> ${fullUpdate.toVersion}, фаза ${fullUpdate.phase})`
        );

        try {
          const result = this.service.recoverFullUpdate(gameId);
          log.info(`[game-delivery] восстановление обновления: ${JSON.stringify(result)}`);
        } catch (error) {
          log.error(`[game-delivery] восстановление обновления не удалось: ${error.message}`);
        }
      }

      const patch = this.service.pendingPatch(gameId);

      if (patch) {
        log.warn(
          `[game-delivery] найдена незавершённая транзакция патча ${gameId} ` +
            (patch.corrupt
              ? "с повреждённым журналом"
              : `(${patch.fromVersion} -> ${patch.toVersion}, фаза ${patch.phase})`)
        );

        try {
          const result = this.service.recoverPatch(gameId);
          log.info(`[game-delivery] восстановление патча: ${JSON.stringify(result)}`);
        } catch (error) {
          log.error(`[game-delivery] восстановление патча не удалось: ${error.message}`);
        }
      }

      const repair = this.service.pendingRepair(gameId);

      if (repair) {
        log.warn(
          `[game-delivery] найден незавершённый ремонт ${gameId} (фаза ${repair.phase})`
        );

        try {
          const result = this.service.recoverRepair(gameId);
          log.info(`[game-delivery] восстановление после ремонта: ${JSON.stringify(result)}`);
        } catch (error) {
          log.error(`[game-delivery] откат ремонта не удался: ${error.message}`);
        }
      }

      // Эпизоды, отложенные подменой сборки, которую прервал крах (InstallService.parkEpisodes).
      try {
        this.service.installService.unparkEpisodes(gameId);
      } catch (error) {
        log.error(`[game-delivery] эпизоды ${gameId} не вернулись: ${error.message}`);
      }

      if (fullUpdate || patch || repair) this.service.refresh(gameId);
    }

    try {
      this.service.cleanupOrphanStaging();
    } catch (error) {
      log.warn(`[game-delivery] уборка забытых каталогов не удалась: ${error.message}`);
    }
  }

  selectGameFromArgs(argv) {
    const gameId = GameActions.parseSelectGameArg(argv);
    if (!gameId) return null;

    const launcherId = manifestRegistry.launcherIdOf(gameId);
    if (!launcherId) return null;

    if (this.window && !this.window.isDestroyed())
      this.window.webContents.send(CHANNELS.selectGame, { gameId, launcherId });

    log.info(`[game-delivery] выбрана игра из аргумента запуска: ${gameId}`);
    return { gameId, launcherId };
  }

  info() {
    this.ensureService();

    return {
      flag: featureFlag.describe(),
      qa: qaMode.describe(),
      qaLaunch: qaLaunch.describe(),
      libraryPath: this.service ? this.service.paths.root : this.libraryPath,
      games: Object.keys(manifestRegistry.MANIFESTS).map((gameId) => ({
        gameId,
        launcherId: manifestRegistry.launcherIdOf(gameId),
        channel: channelConfig.describe(gameId),
      })),
      pending: this.service ? this.service.listPendingOperations() : [],
      legacyLayout: this.legacyLayout.map((g) => ({ gameId: g.gameId, version: g.version })),
      error: this.initError ? this.initError.toPublic() : null,
    };
  }

  getStatus(id) {
    const service = this.ensureService();
    if (!service) return this.initError ? { error: this.initError.toPublic() } : null;

    if (id == null) return service.refreshAll();

    const gameId = this.resolveGameId(id);
    return gameId ? service.refresh(gameId) : null;
  }

  async install(id) {
    const service = this.ensureService();

    if (!service) {
      const delivery = toDeliveryError(this.initError, "PATH_INVALID");

      log.error(`[game-delivery] установка невозможна: ${delivery.code} ${delivery.message}`);
      this.notify(delivery.code, delivery.message);

      return { status: "error", error: delivery.toPublic() };
    }

    const gameId = this.resolveGameId(id);
    if (!gameId) return { status: "unknown-game" };

    try {
      const result = await service.installToLatest(gameId);
      if (result && result.status === "installed") this.ensurePracticeSaves(gameId);
      return result;
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");

      log.error(
        `[game-delivery] установка ${gameId} не удалась: ${delivery.code} ${delivery.message}` +
          (error && error.stack ? `\n${error.stack}` : "")
      );

      this.notify(delivery.code, delivery.message);
      return { status: "error", error: delivery.toPublic() };
    }
  }

  cleanup(id) {
    const service = this.ensureService();
    if (!service) return { removed: false };

    const gameId = this.resolveGameId(id);
    return gameId ? service.cleanupPending(gameId) : { removed: false };
  }

  setLibraryPath(libraryPath) {
    if (qaMode.isEnabled()) {
      this.libraryPath = libraryPath;
      return;
    }

    if (libraryPath === this.libraryPath) return;

    // Новая служба не видит работы старой: загрузка писала бы в прежнюю папку,
    // «Пауза» и «Отмена» уходили бы в пустую службу, игра пропала бы из виду.
    // silentVersionEvents — установка «база + патчи»: между ними слот свободен.
    const service = this.service;
    if (
      (service && (service.isBusy() || service.processService.anyRunning() || service.silentVersionEvents.size)) ||
      (this.tools && (this.tools.busyWith || this.tools.running.size)) ||
      (this.launchService && this.launchService.launching.size)
    )
      return { ok: false, error: "busy" };

    if (this.updateTimer) {
      clearTimeout(this.updateTimer);
      this.updateTimer = null;
    }

    if (this.service) {
      try {
        this.service.dispose();
      } catch (error) {
        log.warn(`[game-delivery] прежняя служба не остановлена: ${error.message}`);
      }
    }

    this.libraryPath = libraryPath;
    this.service = null;
    this.initError = null;
    this.ensureService();
  }

  dispose() {
    if (this.updateTimer) {
      clearTimeout(this.updateTimer);
      this.updateTimer = null;
    }

    if (this.steamConnectionTimer) {
      clearTimeout(this.steamConnectionTimer);
      this.steamConnectionTimer = null;
    }

    if (this.tray) this.tray.dispose();
    if (this.statsStore) this.statsStore.dispose();
    if (this.service) this.service.dispose();
    this.liveSplitWindowCapture.close();
  }
}

module.exports = GameDelivery;
module.exports.CHANNELS = CHANNELS;
