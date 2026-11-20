import { app, clipboard, dialog, ipcMain, protocol, screen, shell } from "electron";
import fs from "fs";
import path from "path";
import log from "electron-log";
import WindowsUpdateManager from "./UpdateManager";
import { applyUiScale } from "./CreateWindow";
import LinuxUpdateManager from "./linux/UpdateManager";
import Store, {
  quarantineState,
  readState,
  writeState,
} from "../renderer/utils/Store.js";
import StoreDefaults from "../renderer/utils/StoreDefaults.js";
import { DEFAULT_LIBRARY_PATH } from "../shared/library-path";

const GameDelivery = require("./game-delivery");
const DemoTool = require("./demo-tool");
const { createPlayerFullscreen } = require("./demo-tool/player-fullscreen");
const { fillZoom } = require("./window-size");
const { DiscordRichPresenceBridge } = require("./discord-rpc");
const { GamePresenceBridge } = require("./game-presence");
const presenceEndpoint = require("./game-presence/endpoint-store");
const { SevenTvBridge } = require("./seventv");
const { YouTubeChatBridge } = require("./youtube");
const { TwitchBadgeBridge } = require("./twitch-badges");
const { TwitchEmoteBridge } = require("./twitch-emotes");
const diagnosticsExport = require("./diagnostics/DiagnosticsExport");
const DiagnosticsRunner = require("./diagnostics/DiagnosticsRunner");
const diagnosticsZip = require("./diagnostics/DiagnosticsZip");
const NotificationStore = require("./notifications/NotificationStore");
const VersionEvents = require("./notifications/version-events");
const { createPendingEvent } = require("./pending-event");
const { levelForCode } = require("./notifications/catalog");
const onlineModeration = require("./online-profile/moderation");
const telemetryConsent = require("./telemetry/consent");
const TelemetryService = require("./telemetry/TelemetryService");
const OnlineClient = require("./telemetry/online-client");
const NewsClient = require("./telemetry/news-client");
const SponsorsClient = require("./telemetry/sponsors-client");
const StylePresetsClient = require("./telemetry/style-presets-client");
const BuildIdentity = require("./security/build-identity");
const BuildAttestation = require("./security/attestation");
const BuildKey = require("./security/build-key");
const newsIpc = require("./telemetry/news-ipc");
const official = require("./security/official");
const sponsorsIpc = require("./telemetry/sponsors-ipc");
const newsLocales = require("../shared/news-locales");
const LauncherUpdateEvents = require("./telemetry/launcher-update-events");
const OnlineProfileService = require("./online-profile/OnlineProfileService");
const profileIpc = require("./online-profile/ipc");
const profileQa = require("./online-profile/local-qa");
const initialLanguage = require("./initial-language");
const ProtonManager = require("./linux/ProtonManager");
const ProtonRunner = require("./linux/ProtonRunner");
const DownloadSettings = require("./game-delivery/DownloadSettings");
const { freeSpaceBytes } = require("./game-delivery/free-space");
const gpuPreference = require("./gpu-preference");

const IS_LINUX = process.platform === "linux";

// Windows обновляется отдельным приложением-обновлятором и установщиком,
// Linux — AppImage-установщиком, из которого ставится вшитый архив.
const UpdateManager = IS_LINUX ? LinuxUpdateManager : WindowsUpdateManager;

const DEFAULT_INSTALL_PATH = DEFAULT_LIBRARY_PATH;

const LIBRARY_FOLDER_NAME = "HLSR";
const RENDERER_STORE_NAMES = new Set(["library", "settings"]);
const MAX_RENDERER_STORE_BYTES = 1024 * 1024;

function rendererStorePath(configName) {
  if (typeof configName !== "string" || !RENDERER_STORE_NAMES.has(configName)) return null;
  return path.join(app.getPath("userData"), `${configName}.json`);
}

function serializableStoreData(data) {
  if (!data || typeof data !== "object" || Array.isArray(data)) return null;

  try {
    const text = JSON.stringify(data);
    if (Buffer.byteLength(text, "utf8") > MAX_RENDERER_STORE_BYTES) return null;
    return JSON.parse(text);
  } catch (error) {
    return null;
  }
}

function withHlsrFolder(selected) {
  const value = String(selected || "").trim();
  if (!value) return value;

  const normalized = path.resolve(value);
  if (path.basename(normalized).toLowerCase() === LIBRARY_FOLDER_NAME.toLowerCase())
    return normalized;

  return path.join(normalized, LIBRARY_FOLDER_NAME);
}

function loadHlsrNative() {
  try {
    return { module: require("hlsr-native"), error: null };
  } catch (error) {
    return { module: null, error };
  }
}

function logNativeFailure(error) {
  log.error(
    "hlsr-native is not available, running in limited mode " +
      "(Steam integration is disabled): " +
      ((error && (error.stack || error.message)) || error)
  );
}

// Без оболочки (statfs, затем fsutil массивом аргументов): корень пути библиотеки
// не должен разбираться как команда cmd.exe. Форма ответа — как у hlsr-native.
function getDiskFreeSpaceMbytesFallback(dirPath) {
  const bytes = freeSpaceBytes(dirPath || "C:\\");
  return bytes === null ? null : { Result: 1, FreeMBytesAvailable: Math.floor(bytes / (1024 * 1024)) };
}

const fallbackNative = {
  steamworks: {
    SteamAPI_Init: () => false,
    GetFriends: () => [],
    BIsSubscribedApp: () => false,
    GetPersonaName: () => "",
    SetRichPresence: () => false,
  },
  LiveSplit: {
    ReadSplitsFile: () => null,
  },
  WinApi: {
    GetDiskFreeSpaceMbytes: getDiskFreeSpaceMbytesFallback,
  },
};

class AppManager {
  constructor(window) {
    this.window = window;
    const qa = profileQa.settings(app);
    // Официальная сборка (ключ аттестации и адрес онлайна вшиты при сборке выпуска) или
    // локальный QA-стенд. Самосборка онлайн не запускает, интерфейс показывает заглушки.
    this.official = Boolean(qa) || official.isOfficial();
    this.downloadSettings = new DownloadSettings({
      file: path.join(app.getPath("userData"), "download-settings.json"),
    });

    // Proton нужен только в Linux: игры и LiveSplit запускаются через umu.
    this.protonManager = IS_LINUX
      ? new ProtonManager({ dataDir: app.getPath("userData"), logger: log })
      : null;
    this.protonRunner = IS_LINUX
      ? new ProtonRunner({
          manager: this.protonManager,
          dataDir: app.getPath("userData"),
          logger: log,
        })
      : null;

    this.notifications = new NotificationStore({
      file: path.join(app.getPath("userData"), "notifications.json"),
      logger: log,
    });
    this.notifications.onChange = (summary) => {
      if (this.window && !this.window.isDestroyed())
        this.window.webContents.send("notifications-changed", summary);
    };

    this.versionEvents = new VersionEvents({
      file: path.join(app.getPath("userData"), "version-state.json"),
      store: this.notifications,
      logger: log,
    });

    this.telemetry = new TelemetryService({
      endpoint: qa ? qa.apiBase + "/v1/telemetry/batch" : undefined,
      authorizationProvider: () => this.onlineProfiles && this.onlineProfiles.getAuthToken(),
      userDataDir: app.getPath("userData"),
      launcherVersion: app.getVersion(),
      logger: log,
      localeProvider: () => this.currentLocale(),
      isWindowVisible: () => this.windowVisible(),
      installDir: path.dirname(app.getPath("exe")),
    });

    this.onlineClient = new OnlineClient({
      endpoint: qa ? qa.apiBase + "/v1/public/online" : undefined,
      launcherVersion: app.getVersion(),
      logger: log,
      isWindowVisible: () => this.windowVisible(),
      onUpdate: (snapshot) => this.sendOnline(snapshot),
    });

    this.newsClient = new NewsClient({
      endpoint: qa ? qa.apiBase + "/v1/public/news" : undefined,
      launcherVersion: app.getVersion(),
    });

    this.sponsorsClient = new SponsorsClient({
      endpoint: qa ? qa.apiBase + "/v2/public/sponsors" : undefined,
      legacyEndpoint: qa ? qa.apiBase + "/v1/public/sponsors" : undefined,
      launcherVersion: app.getVersion(),
    });

    // Заготовки «подсветка ника + рамка» для настроек профиля.
    this.stylePresetsClient = new StylePresetsClient({
      endpoint: qa ? qa.apiBase + "/v2/public/style-presets" : undefined,
      launcherVersion: app.getVersion(),
    });

    this.launcherUpdateEvents = new LauncherUpdateEvents({
      telemetry: this.telemetry,
      versionEvents: this.versionEvents,
      logger: log,
    });

    this.updateManager = new UpdateManager(window);
    this.updateManager.setNotifications(this.notifications);
    this.updateManager.setVersionEvents(this.versionEvents);
    this.updateManager.setUpdateTelemetry(this.launcherUpdateEvents);
    this.gameDelivery = new GameDelivery({
      window,
      libraryPath: DEFAULT_INSTALL_PATH,
      launcherVersion: app.getVersion(),
      userDataPath: app.getPath("userData"),
      notifications: this.notifications,
      versionEvents: this.versionEvents,
      telemetry: this.telemetry,
      locale: () => this.currentLocale(),
      protonRunner: this.protonRunner,
      rateLimiter: this.downloadSettings.rateLimiter,
    });
    // Инструмент «Демо»: демо из папок игр, FFmpeg — инструмент той же библиотеки.
    this.demoTool = new DemoTool({
      store: new Store({ configName: "demo-tool", defaults: StoreDefaults.demoTool }),
      games: () => this.gameDelivery.demoContexts(),
      tools: () => (this.gameDelivery.ensureService() ? this.gameDelivery.tools : null),
      emit: (channel, payload) => {
        if (this.window && !this.window.isDestroyed()) this.window.webContents.send(channel, payload);
        // Discord: «Рендерит демку», пока идёт предпросмотр или экспорт.
        if (this.discordRpc && /^demo-job-(progress|done|error)$/.test(channel))
          this.discordRpc.setDemoRendering(channel === "demo-job-progress");
      },
      dialog: { showOpenDialog: (options) => dialog.showOpenDialog(this.window, options) },
      shell,
      // onlineProfiles создаётся позже — берём его в момент события.
      achieve: (id) => {
        if (this.onlineProfiles && typeof this.onlineProfiles.achieve === "function") this.onlineProfiles.achieve(id);
      },
      // getPath бросает, если папки «Видео» нет (сломанное перенаправление,
      // OneDrive): необязательный инструмент не должен ронять запуск.
      videosDir: (() => {
        try {
          return app.getPath("videos");
        } catch (error) {
          log.warn(`[demo] папка «Видео» не получена: ${error.message}`);
          return null;
        }
      })(),
      // Временные файлы заданий — в данных пользователя, как логи: Windows —
      // %LOCALAPPDATA%\hlsr (не перемещаемый профиль), Linux — userData (0700).
      jobsRoot: path.join(
        !IS_LINUX && process.env.LOCALAPPDATA ? path.join(process.env.LOCALAPPDATA, app.getName()) : app.getPath("userData"),
        "demo-jobs"
      ),
      logger: log,
      // Рендер Ghosting: помощник из resources/demo-render, предусловия —
      // отслеживание игр и Steam лаунчера, предел разрешения — основной монитор (DIP).
      resourcesPath: process.resourcesPath,
      // Игра запущена или как раз запускается из лаунчера.
      gameRunning: (gameId) => {
        const service = this.gameDelivery.ensureService();
        const launching = this.gameDelivery.launchService && this.gameDelivery.launchService.launching;
        return !!(service && service.isGameRunning(gameId)) || !!(launching && launching.has(gameId));
      },
      // Идёт установка, обновление или ремонт: файлы игры меняются.
      gameBusy: () => {
        const service = this.gameDelivery.ensureService();
        return !!(service && service.isBusy());
      },
      steamStatus: () =>
        this.gameDelivery.ensureService() && this.gameDelivery.steamLocator
          ? this.gameDelivery.steamLocator.describeSteamOnly()
          : { ok: false, code: "STEAM_NOT_RUNNING" },
      // hl2.exe не DPI-aware: игра видит рабочий стол в DIP (2560×1440 при 150 % — это
      // 1707×960), окно больше него обрезается. Поэтому предел — размер в DIP.
      screenSize: () => {
        const display = screen.getPrimaryDisplay();
        return { width: display.size.width, height: display.size.height };
      },
    });
    // Пока идёт рендер Ghosting, его запуск из лаунчера отказывает (RENDER_ACTIVE).
    this.gameDelivery.renderActive = (gameId) => this.demoTool.renderActive(gameId);
    // Mute игры после рендера снимается перед её запуском из лаунчера и в фоне.
    this.gameDelivery.beforeLaunch = (gameId) => this.demoTool.resetMuteBeforeLaunch(gameId);
    // Папки рендера, оставшиеся в моде после сбоя: путь к моду — из библиотеки,
    // поэтому не в конструкторе, а чуть позже старта.
    setTimeout(() => {
      this.demoTool.cleanupRenderDirs();
      this.demoTool.startMuteResets();
    }, 15000).unref();
    this.buildIdentity = new BuildIdentity({ version: app.getVersion(), packaged: app.isPackaged });
    // Хеш app.asar считается сразу: лаунчер может обновиться, пока открыт, и
    // тогда файл на диске уже не тот, что запущен.
    this.buildIdentity.warm();
    this.attestation = new BuildAttestation({
      identity: this.buildIdentity,
      key: BuildKey.embedded(),
      apiBase: qa ? qa.apiBase : undefined,
      launcherVersion: app.getVersion(),
      log,
    });
    this.thanksEvent = createPendingEvent((notice) => {
      if (this.window && !this.window.isDestroyed()) this.window.webContents.send("support-thanks", notice);
    });
    this.onlineProfiles = new OnlineProfileService({
      apiBase: qa ? qa.apiBase : undefined,
      buildHeader: () => this.buildIdentity.header(),
      ticketProvider: (force) => this.attestation.ticket(force),
      onClientModified: () => {
        log.warn("[online] сервер не узнал сборку лаунчера: app.asar изменён, вход отозван");
        this.notifications.add({ level: levelForCode("CLIENT_MODIFIED"), code: "CLIENT_MODIFIED" });
      },
      userDataDir: app.getPath("userData"),
      safeStorage: require("electron").safeStorage,
      launcherVersion: app.getVersion(),
      accountProvider: () => qa ? profileQa.account(qa) : this.gameDelivery.steamAccountState(),
      friendProvider: () => qa ? profileQa.friends(qa) : this.gameDelivery.steamFriends(),
      openExternal: (url) => shell.openExternal(url),
      onChange: (snapshot) => {
        if (this.window && !this.window.isDestroyed()) this.window.webContents.send("runner-profile-state", snapshot);
        if (!snapshot.account.signedIn && this.telemetry) this.telemetry.stopTimers();
      },
      onModeration: (notice) => {
        const code = onlineModeration.CODES[notice.action];
        const timed = notice.action === "ban" || notice.action === "mute";
        this.notifications.add({
          level: levelForCode(code),
          code,
          detail: notice.reason,
          meta: timed ? (notice.expiresAt ? { until: notice.expiresAt } : { forever: true }) : null,
        });
      },
      onConsent: (enabled) => {
        this.telemetry.stopTimers();
        this.telemetry.started = false;
        this.telemetry.setEnabled(enabled);
        this.telemetry.start();
      },
      onSupportThanks: (notice) => {
        this.notifications.add({ level: "info", code: "SUPPORT_THANKS",
          meta: notice.until ? { until: new Date(notice.until * 1000).toISOString().slice(0, 19) + "Z" } : null });
        this.thanksEvent.push(notice);
      },
      onUnlock: (item) => {
        if (this.window && !this.window.isDestroyed()) this.window.webContents.send("achievement-unlocked", item);
      },
      onAuthenticated: () => {
        if (this.window && !this.window.isDestroyed()) {
          if (this.window.isMinimized()) this.window.restore();
          this.window.show();
          this.window.focus();
          this.window.webContents.send("runner-login-success");
        }
      },
    });
    this.gameDelivery.onlineProfiles = this.onlineProfiles;
    // День отмечается, когда интерфейс уже слушает: иначе поздравление
    // с «Неделей в Мезе» ушло бы в ещё не загруженное окно.
    const noteDay = () => { if (this.official) this.onlineProfiles.noteActiveDay(); };
    if (this.window && !this.window.isDestroyed() && this.window.webContents.isLoading()) this.window.webContents.once("did-finish-load", noteDay);
    else noteDay();
    profileIpc.bind(ipcMain, this.onlineProfiles, (url) => shell.openExternal(url));
    // Один диалог «Открыть ссылку?» на ссылки новостей и спонсоров.
    const confirmLink = async (host, url) => {
      const t = newsLocales[this.currentLocale()];
      const result = await dialog.showMessageBox(this.window, {
        type: "question", title: t["#L_HOME_LINK_TITLE"], message: t["#L_HOME_LINK_TEXT"] + "\n" + host,
        detail: url, buttons: [t["#L_HOME_LINK_CANCEL"], t["#L_HOME_LINK_OPEN"]], defaultId: 0, cancelId: 0, noLink: true,
      });
      return result.response === 1;
    };
    newsIpc.bind(ipcMain, {
      client: this.newsClient, profile: this.onlineProfiles, openExternal: (url) => shell.openExternal(url), confirmLink,
    });
    sponsorsIpc.bind(ipcMain, this.sponsorsClient, { openExternal: (url) => shell.openExternal(url), confirmLink });

    this.discordRpc = new DiscordRichPresenceBridge({
      userDataPath: app.getPath("userData"),
      log,
      versionProvider: () => app.getVersion(),
      localeProvider: () => this.currentLocale(),
    });
    this.discordRpc.setWindow(window);
    this.discordRpc.attachDelivery(this.gameDelivery);

    // Канал прошлого запуска: если игра пережила перезапуск лаунчера, она
    // подключается только к нему (endpoint-store.js).
    this.presenceEndpointFile = path.join(app.getPath("userData"), "presence-endpoint.json");
    const savedEndpoint = presenceEndpoint.load(this.presenceEndpointFile) || {};
    this.gamePresence = new GamePresenceBridge({
      log,
      token: savedEndpoint.token,
      pipeName: savedEndpoint.pipeName,
      port: savedEndpoint.port,
      onPresence: (state) => this.discordRpc.setGameState(state),
    });
    this.sevenTv = new SevenTvBridge({ log });
    this.youTube = new YouTubeChatBridge({ log });
    this.twitchBadges = new TwitchBadgeBridge({ log });
    this.twitchEmotes = new TwitchEmoteBridge({ log });

    try {
      this.gamePresence.start();
      this.gameDelivery.setGameEnvironmentProvider(() => {
        const environment = this.gamePresence.environment();
        try {
          presenceEndpoint.save(this.presenceEndpointFile, this.gamePresence.endpoint());
        } catch (error) {
          log.warn(`[game-presence] канал не сохранён: ${error.message}`);
        }
        return environment;
      });
      this.discordRpc.attachGamePresence(this.gamePresence);
      this.gamePresence.attachChatConsumer(this.sevenTv);
      this.gamePresence.attachChatConsumer(this.youTube);
      this.gamePresence.attachChatConsumer(this.twitchBadges);
      this.gamePresence.attachChatConsumer(this.twitchEmotes);
    } catch (error) {
      log.error(`[game-presence] локальный мост не поднялся: ${error.message}`);
    }

    try {
      this.discordRpc.start();
    } catch (error) {
      log.error(`[discord-rpc] не удалось запустить интеграцию: ${error.message}`);
    }

    this.updateManager.setGameRunningCheck(() => this.gameDelivery.isAnyGameRunning());

    // hlsr-native (Steamworks и WinAPI) собирается только под Windows.
    let { module: nativeModule, error: nativeError } = IS_LINUX
      ? { module: null, error: null }
      : loadHlsrNative();
    this.nativeAvailable = !!nativeModule;
    this.nativeLoadError = nativeError ? nativeError.message : null;
    this.native = nativeModule || fallbackNative;
    if (!this.nativeAvailable && !IS_LINUX) logNativeFailure(nativeError);
  }

  umuAvailable() {
    return !!(this.protonRunner && this.protonRunner.umuPath());
  }

  validateInstallPath(rawPath) {
    if (typeof rawPath !== "string" || !rawPath.trim())
      return { ok: false, error: "invalid" };

    let target = path.normalize(rawPath.trim());

    if (IS_LINUX) {
      if (!path.isAbsolute(rawPath.trim())) return { ok: false, error: "invalid" };
      if (target === path.parse(target).root) return { ok: false, error: "invalid" };
    } else {
      if (!/^[A-Za-z]:[\\/]/.test(target)) return { ok: false, error: "invalid" };

      const lower = target.toLowerCase();
      const forbidden = ["\\windows", "\\program files", "\\programdata", "\\system32"];
      if (forbidden.some((part) => lower.includes(part))) return { ok: false, error: "invalid" };

      if (/^[A-Za-z]:[\\/]?$/.test(target)) return { ok: false, error: "invalid" };
    }

    try {
      // Созданную здесь папку запоминает paths.js: при сохранении выбора она получит отметку «создал лаунчер».
      require("./game-delivery/paths").prepareLibraryRoot(target);
    } catch (e) {
      log.warn(`Library path is not creatable: ${e.message}`);
      return { ok: false, error: "no-access" };
    }

    try {
      let probe = path.join(target, `.hlsr-write-test-${Date.now()}`);
      fs.writeFileSync(probe, "");
      fs.unlinkSync(probe);
    } catch (e) {
      log.warn(`Library path is not writable: ${e.message}`);
      return { ok: false, error: "no-access" };
    }

    return {
      ok: true,
      path: target,
      freeSpaceMb: this.getFreeSpaceMb(target),
      hasCyrillic: /[Ѐ-ӿ]/.test(target),
    };
  }

  currentLibraryPath() {
    const LibraryStore = new Store({
      configName: "library",
      defaults: StoreDefaults.library,
    });

    return LibraryStore.get("libraryPath") || DEFAULT_INSTALL_PATH;
  }

  currentLocale() {
    try {
      const SettingsStore = new Store({
        configName: "settings",
        defaults: StoreDefaults.settings,
      });

      return SettingsStore.get("config").language === 1 ? "ru" : "en";
    } catch (error) {
      return "ru";
    }
  }

  windowVisible() {
    try {
      if (!this.window || this.window.isDestroyed()) return false;
      return this.window.isVisible() && !this.window.isMinimized();
    } catch (error) {
      return false;
    }
  }

  sendOnline(snapshot) {
    if (!this.window || this.window.isDestroyed()) return;

    try {
      this.window.webContents.send("public-online", snapshot);
    } catch (error) {
      /* окно закрывается — не повод падать */
    }
  }

  getFreeSpaceMb(target) {
    if (IS_LINUX) {
      try {
        const stat = fs.statfsSync(this.spaceProbePath(target));
        return Math.floor((stat.bavail * stat.bsize) / (1024 * 1024));
      } catch (e) {
        return null;
      }
    }

    try {
      const info = this.native.WinApi.GetDiskFreeSpaceMbytes(this.spaceProbePath(target));
      if (!info || !info.Result) return null;

      const value = Number(
        info.FreeMBytesAvailable != null ? info.FreeMBytesAvailable : info.TotalNumberOfFreeMBytes
      );

      return isFinite(value) && value >= 0 ? Math.floor(value) : null;
    } catch (e) {
      return null;
    }
  }

  spaceProbePath(target) {
    let current = path.resolve(String(target || DEFAULT_INSTALL_PATH));

    for (;;) {
      if (fs.existsSync(current)) return current;

      const parent = path.dirname(current);
      if (parent === current) return current;
      current = parent;
    }
  }

  ensureDefaultInstallPath() {
    const LibraryStore = new Store({
      configName: "library",
      defaults: StoreDefaults.library,
    });

    if (LibraryStore.get("libraryPath")) return;

    let legacyPath = path.join(app.getPath("userData"), "library");
    let installed = LibraryStore.get("installed") || {};
    let hasLegacyGames =
      Object.keys(installed).length > 0 && fs.existsSync(legacyPath);

    if (hasLegacyGames) {
      LibraryStore.set("libraryPath", legacyPath);
      LibraryStore.set("libraryConfirmed", true);
      log.info(`Existing game library detected: ${legacyPath}`);
      return;
    }

    LibraryStore.set("libraryPath", DEFAULT_INSTALL_PATH);
    log.info(`Default game library path proposed: ${DEFAULT_INSTALL_PATH}`);
  }

  revealWindow() {
    const window = this.window;
    if (!window || window.isDestroyed()) return;

    try {
      if (window.isMinimized()) window.restore();
      if (!window.isVisible()) window.show();
      window.focus();
    } catch (error) {
      log.warn(`[window] не удалось показать окно: ${error.message}`);
    }
  }

  initialize() {
    const trustedRenderer = (event) =>
      !!this.window && !this.window.isDestroyed() && event.sender === this.window.webContents;

    // В изолированном preload модуля clipboard нет: копирует main.
    ipcMain.handle("clipboard-write-text", (event, value) => {
      if (!trustedRenderer(event)) return { status: "error", code: "UNTRUSTED" };
      const text = String(value || "");
      if (text.length > 1024 * 1024) return { status: "error", code: "TOO_LARGE" };
      clipboard.writeText(text);
      return { status: "ok" };
    });

    // Масштаб интерфейса (настройки, «Интерфейс»): сохранён renderer'ом в
    // settings.json, здесь только применяется к окну.
    ipcMain.handle("ui-scale-apply", (event, scale) => {
      if (!trustedRenderer(event)) return { status: "error", code: "UNTRUSTED" };
      try {
        // Новый размер окна — из обычного окна: полный экран вернул бы старый.
        // Полный экран плеера «Демо» вернёт окну старый размер — масштаб применится при следующем запуске.
        if (this.window && this.window.__hlsrFullscreen && !(this.windowFullscreen && this.windowFullscreen.active()))
          return { status: "error", code: "FULLSCREEN" };
        if (this.windowFullscreen && this.windowFullscreen.active()) {
          this.windowFullscreen.set(this.window, false);
          this.window.webContents.send("launcher-fullscreen-changed", false);
        }
        return { status: "ok", scale: applyUiScale(this.window, scale) };
      } catch (error) {
        log.warn(`[ui-scale] не применён: ${error.message}`);
        return { status: "error", code: "APPLY_FAILED" };
      }
    });

    ipcMain.on("renderer-store-read", (event, configName) => {
      const file = trustedRenderer(event) ? rendererStorePath(configName) : null;
      event.returnValue = file
        ? readState(file)
        : { found: true, valid: false, reason: "io", code: "EACCES" };
    });

    ipcMain.on("renderer-store-write", (event, configName, data) => {
      const file = trustedRenderer(event) ? rendererStorePath(configName) : null;
      const clean = serializableStoreData(data);
      event.returnValue = !!file && !!clean && writeState(file, clean);
    });

    ipcMain.on("renderer-store-quarantine", (event, configName) => {
      const file = trustedRenderer(event) ? rendererStorePath(configName) : null;
      event.returnValue = file ? quarantineState(file) : { ok: false, error: "EACCES" };
    });

    this.ensureDefaultInstallPath();
    this.gameDelivery.setLibraryPath(this.currentLibraryPath());

    this.window.on("focus", () => {
      try {
        this.updateManager.checkOnFocus();
        this.gameDelivery.checkOnFocus();
        if (this.onlineProfiles) this.onlineProfiles.noteNewDay();
      } catch (error) {
        log.warn(`[focus-check] ${error.message}`);
      }
    });

    app.on("second-instance", (e, cmd, wdir) => {
      if (!this.window || this.window.isDestroyed()) return;

      this.revealWindow();

      cmd.splice(0, 1);
      this.gameDelivery.selectGameFromArgs(cmd);
    });

    // Кнопка «Полный экран» в шапке (WindowControls.vue): механизм плеера «Демо»
    // без выхода по Esc. Окно — на рабочую область экрана (панель задач Windows видна,
    // просьба владельца 14.11), раскладка 1600×900 CSS растягивается на неё.
    const windowFullscreen = createPlayerFullscreen(
      (value) => {
        if (this.window && !this.window.isDestroyed()) this.window.webContents.send("launcher-fullscreen-changed", value);
      },
      {
        escape: false,
        // __hlsrFullscreenZoom — для did-finish-load (CreateWindow.js): перезагрузка страницы в полном экране.
        bounds: (win) => screen.getDisplayMatching(win.getBounds()).workArea,
        onEnter: (win) => {
          win.__hlsrFullscreenZoom = fillZoom(screen.getDisplayMatching(win.getBounds()).workArea);
          win.webContents.setZoomFactor(win.__hlsrFullscreenZoom);
        },
        onLeave: (win) => {
          win.__hlsrFullscreenZoom = null;
          win.webContents.setZoomFactor(win.__hlsrLayout.zoomFactor);
        },
      }
    );
    this.windowFullscreen = windowFullscreen;

    ipcMain.handle("window-command", (e, command) => {
      if (!this.window || this.window.isDestroyed()) return { ok: false, code: "NO_WINDOW" };

      switch (command) {
        // Ответ — состояние после команды (true — окно на весь экран).
        case "fullscreen":
          // Окно на весь экран у плеера «Демо»: кнопка шапки им не владеет и ничего не меняет.
          if (this.window.__hlsrFullscreen && !windowFullscreen.active()) return { ok: true, fullscreen: false };
          return { ok: true, fullscreen: windowFullscreen.set(this.window, !windowFullscreen.active()) };
        case "fullscreen-exit":
          return { ok: true, fullscreen: windowFullscreen.set(this.window, false) };
        case "fullscreen-state":
          return { ok: true, fullscreen: windowFullscreen.active() };
        case "minimize":
          this.window.webContents.send("window-visibility", false);
          this.window.minimize();
          return { ok: true };
        case "close":
          this.window.close();
          return { ok: true };
        case "restore":
          this.revealWindow();
          return { ok: true };
        default:
          log.warn(`[window] отклонена неизвестная команда окна: ${String(command)}`);
          return { ok: false, code: "UNKNOWN_COMMAND" };
      }
    });

    ipcMain.handle("select-install-folder", async (e, currentPath) => {
      let result = await dialog.showOpenDialog(this.window, {
        properties: ["openDirectory", "createDirectory"],
        defaultPath:
          typeof currentPath === "string" && currentPath ? currentPath : undefined,
      });

      if (result.canceled || !result.filePaths.length) return null;
      return withHlsrFolder(result.filePaths[0]);
    });

    ipcMain.handle("download-settings-get", () => this.downloadSettings.snapshot());
    ipcMain.handle("download-settings-set", (event, speedLimit) => {
      if (!trustedRenderer(event)) return { ok: false, code: "UNAUTHORIZED" };
      return this.downloadSettings.save(speedLimit);
    });

    ipcMain.handle("validate-install-path", (e, rawPath) => {
      return this.validateInstallPath(rawPath);
    });

    ipcMain.on("native-available", (e) => {
      e.returnValue = this.nativeAvailable;
    });

    ipcMain.on("umu-available", (e) => {
      e.returnValue = this.umuAvailable();
    });

    if (this.protonManager) {
      ipcMain.handle("proton-cancel", () => this.protonManager.cancel());
      ipcMain.handle("proton-status", () => this.protonManager.status());
      ipcMain.handle("proton-select", (event, selectedPath) => this.protonManager.select(selectedPath));
      ipcMain.handle("proton-add", async () => {
        const chosen = await dialog.showOpenDialog(this.window, {
          title: "Выберите папку Proton",
          properties: ["openDirectory"],
        });
        if (chosen.canceled || !chosen.filePaths.length) return null;
        return this.protonManager.addCustom(chosen.filePaths[0]);
      });
      ipcMain.handle("proton-download", () =>
        this.protonManager.ensureReference().then(() => this.protonManager.status())
      );
      ipcMain.handle("proton-update", () =>
        this.protonManager.applyUpdate().then(() => this.protonManager.status())
      );
      this.protonManager.on("install-started", (event) => {
        this.notifications.add({ level: "info", code: "PROTON_INSTALLING",
          detail: event.version.replace(/^wineland-/, "") });
      });
      this.protonManager.on("installed", (event) => {
        this.notifications.add({ level: "info", code: "PROTON_INSTALLED",
          detail: event.version.replace(/^wineland-/, "") });
      });
      this.protonManager.on("update-available", (event) => {
        if (this.notifications.list().items.some((item) =>
          item.code === "PROTON_UPDATE_AVAILABLE" && item.meta && item.meta.newVersion === event.to)) return;
        this.notifications.removeWhere((item) => item.code === "PROTON_UPDATE_AVAILABLE");
        const short = (tag) => String(tag).replace(/^wineland-/, "");
        this.notifications.add({ level: "info", code: "PROTON_UPDATE_AVAILABLE",
          detail: `${short(event.from)} → ${short(event.to)}`,
          meta: { target: "proton", newVersion: event.to } });
      });
      this.protonManager.on("updated", (event) => {
        this.notifications.removeWhere((item) => item.code === "PROTON_UPDATE_AVAILABLE");
        const short = (tag) => String(tag).replace(/^wineland-/, "");
        this.notifications.add({
          level: "info",
          code: "PROTON_UPDATED",
          detail: `${short(event.from)} → ${short(event.to)}`,
        });
      });
      let protonFailed = false;
      this.protonManager.on("state", (state) => {
        if (state.phase === "error" && !protonFailed)
          this.notifications.add({ level: "error", code: "PROTON_INSTALL_FAILED", detail: state.error });
        protonFailed = state.phase === "error";
        if (this.window && !this.window.isDestroyed()) this.window.webContents.send("proton-state", state);
      });
    }

    ipcMain.on("app-version", (e) => {
      e.returnValue = app.getVersion();
    });

    ipcMain.on("app-official", (e) => {
      e.returnValue = this.official === true;
    });

    ipcMain.on("app-initial-language", (e) => {
      if (this.initialLanguage === undefined) {
        try {
          this.initialLanguage = initialLanguage.pick({ locale: app.getLocale() });
        } catch (error) {
          log.warn(`[language] язык первого запуска не определён: ${error.message}`);
          this.initialLanguage = initialLanguage.EN;
        }
      }

      e.returnValue = this.initialLanguage;
    });

    ipcMain.handle("open-games-folder", async () => {
      const deliveryPaths = require("./game-delivery/paths");
      const libraryPath = this.currentLibraryPath();

      let paths;
      try {
        paths = deliveryPaths.resolveLibrary(libraryPath);
      } catch (error) {
        log.warn(`[games-folder] недопустимый путь библиотеки: ${error.message}`);
        return { status: "error", code: "LIBRARY_UNAVAILABLE" };
      }

      const games = paths.games;

      if (!deliveryPaths.isInside(paths.root, games) || games === paths.root)
        return { status: "error", code: "PATH_INVALID" };

      const lower = games.toLowerCase();
      const forbidden = ["\\windows", "\\program files", "\\programdata"];
      if (!IS_LINUX && forbidden.some((part) => lower.includes(part)))
        return { status: "error", code: "PATH_INVALID" };

      if (!fs.existsSync(games)) {
        try {
          fs.mkdirSync(games, { recursive: true });
        } catch (error) {
          log.warn(`[games-folder] не удалось создать ${games}: ${error.message}`);
          return { status: "error", code: "PATH_NOT_WRITABLE" };
        }
      }

      const failure = await shell.openPath(games);
      if (failure) {
        log.warn(`[games-folder] не удалось открыть ${games}: ${failure}`);
        return { status: "error", code: "OPEN_FAILED" };
      }

      return { status: "ok", path: games };
    });

    ipcMain.handle("get-library-path", () => {
      const LibraryStore = new Store({
        configName: "library",
        defaults: StoreDefaults.library,
      });

      let libraryPath = LibraryStore.get("libraryPath") || DEFAULT_INSTALL_PATH;

      return {
        path: libraryPath,
        exists: fs.existsSync(libraryPath),
        confirmed: !!LibraryStore.get("libraryConfirmed"),
        freeSpaceMb: this.getFreeSpaceMb(libraryPath),
        isDefault: libraryPath === DEFAULT_INSTALL_PATH,
      };
    });

    ipcMain.handle("set-library-path", (e, rawPath) => {
      let check = this.validateInstallPath(rawPath);
      if (!check.ok) return check;

      // Во время загрузки, установки или игры папка не меняется и не запоминается
      const refused = this.gameDelivery.setLibraryPath(check.path);
      if (refused) return refused;

      const LibraryStore = new Store({
        configName: "library",
        defaults: StoreDefaults.library,
      });

      LibraryStore.set("libraryPath", check.path);
      LibraryStore.set("libraryConfirmed", true);
      // Папку создал сам лаунчер (её не было до проверки): полное удаление унесёт её целиком.
      require("./game-delivery/paths").markCreatedLibrary(check.path);
      log.info(`Game library path set to: ${check.path}`);

      return check;
    });

    ipcMain.handle("open-logs-folder", () => {
      let logFile = log.transports.file.getFile().path;
      shell.showItemInFolder(logFile);
      return path.dirname(logFile);
    });

    ipcMain.handle("telemetry-reset-identity", () => {
      const result = this.telemetry.resetIdentity();
      log.info("[telemetry] идентичность сброшена по запросу пользователя");
      return { ok: true, hadIdentity: result.hadIdentity };
    });

    ipcMain.handle("telemetry-status", () => this.telemetry.snapshot());

    ipcMain.handle("public-online-get", () => this.onlineClient.snapshot());
    ipcMain.handle("public-news-get", (event, options) =>
      this.newsClient.get({ force: Boolean(options && options.force === true) }));
    ipcMain.handle("public-sponsors-get", (event, options) =>
      this.sponsorsClient.get({ force: Boolean(options && options.force === true) }));
    ipcMain.handle("public-style-presets-get", (event, options) =>
      this.stylePresetsClient.get({ force: Boolean(options && options.force === true) }));

    ipcMain.handle("export-diagnostics", () => {
      try {
        const logsDir = path.dirname(log.transports.file.getFile().path);
        const libraryPath = this.currentLibraryPath();

        const result = diagnosticsExport.exportDiagnostics({
          logsDir,
          now: new Date().toISOString(),
          date: new Date(),
          appName: app.getName(),
          appVersion: app.getVersion(),
          buildChannel: process.env.HLSR_BUILD_CHANNEL || "production",
          electronVersion: process.versions.electron,
          chromeVersion: process.versions.chrome,
          nodeVersion: process.versions.node,
          arch: process.arch,
          platform: process.platform,
          osRelease: require("os").release(),
          libraryPath,
          libraryExists: fs.existsSync(libraryPath),
          updateState: this.updateManager ? this.updateManager.state : "idle",
          userName: path.basename(app.getPath("home")),
          env: process.env,
        });

        if (result.ok) {
          log.info(`[launcher] diagnostics export created: ${path.basename(result.path)}`);
          shell.showItemInFolder(result.path);
        } else {
          log.warn(`[launcher] diagnostics export failed: ${result.error}`);
        }

        return result;
      } catch (error) {
        log.warn(`[launcher] diagnostics export failed: ${error.message}`);
        return { ok: false, error: error.message };
      }
    });

    this.diagnosticsRunner = new DiagnosticsRunner({
      logger: log,
      context: () => ({
        appVersion: app.getVersion(),
        packaged: app.isPackaged,
        platform: process.platform,
        nativeAvailable: this.nativeAvailable,
        nativeLoadError: this.nativeLoadError,
        gpuStatus: () => gpuPreference.snapshot(app),
        downloadLimitBps: () => this.downloadSettings.rateLimiter.bytesPerSecond,
        umuAvailable: this.umuAvailable(),
        protonSelected:
          this.protonManager && this.protonManager.selected() ? this.protonManager.selected().name : null,
        userDataPath: app.getPath("userData"),
        updateState: this.updateManager ? this.updateManager.state : "idle",
        updateEnabled: this.updateManager ? this.updateManager.enabled !== false : false,
        libraryPath: this.currentLibraryPath(),
        freeSpaceMb: this.getFreeSpaceMb(this.currentLibraryPath()),
        steamHealth: this.gameDelivery.steamHealthCheck(true),
        launchAvailability: () => {
          if (!this.gameDelivery.ensureService() || !this.gameDelivery.launchService)
            return { ok: false, code: "LAUNCH_CONFIG_INVALID" };
          return this.gameDelivery.launchService.describeAvailability("ghosting-renewed");
        },
        requiredSteamApps: () => this.gameDelivery.requiredSteamApps(),
        steamOwnership: (appId, options) => this.gameDelivery.steamOwnership(appId, options),
        profile: () => this.gameDelivery.diagnosticsProfile(),
        telemetryEnabled: () => this.telemetry.enabled(),
        telemetryStatus: () => this.telemetry.snapshot(),
      }),
    });

    this.lastDiagnostics = null;

    ipcMain.handle("diagnostics-run", async () => {
      if (this.diagnosticsRun) {
        log.info("[diagnostics] прогон уже идёт — переиспользую активный runId");
        return this.diagnosticsRun;
      }

      this.diagnosticsRunId = (this.diagnosticsRunId || 0) + 1;
      const runId = this.diagnosticsRunId;

      this.diagnosticsRun = (async () => {
        const result = await this.diagnosticsRunner.run((item) => {
          if (this.window && !this.window.isDestroyed())
            this.window.webContents.send("diagnostics-progress", Object.assign({ runId }, item));
        });

        this.lastDiagnostics = result.checks;

        const failedNetwork = result.checks.filter(
          (c) => c.group === "network" && c.status === "fail"
        );
        if (failedNetwork.length)
          try {
            this.notifications.add({
              level: "warning",
              code: "DIAGNOSTICS_NETWORK",
              detail: failedNetwork.map((c) => c.code).filter(Boolean).join(", "),
            });
          } catch (e) {
            /* уведомление вторично */
          }

        return Object.assign({ runId }, result);
      })();

      try {
        return await this.diagnosticsRun;
      } finally {
        this.diagnosticsRun = null;
      }
    });

    ipcMain.handle("diagnostics-cancel", () => {
      this.diagnosticsRunner.cancel();
      return true;
    });

    ipcMain.handle("export-diagnostics-zip", async (e, options) => {
      const includeDump = !!(options && options.includeDump === true);

      const defaultName = `HLSR-Diagnostics-${diagnosticsZip.timestampName()}.zip`;
      const picked = await dialog.showSaveDialog(this.window, {
        defaultPath: path.join(app.getPath("documents"), defaultName),
        filters: [{ name: "ZIP", extensions: ["zip"] }],
      });

      if (picked.canceled || !picked.filePath) return { ok: false, cancelled: true };

      try {
        const logsDir = path.dirname(log.transports.file.getFile().path);
        const stats = this.gameDelivery.ensureStatsStore();
        const crash = this.gameDelivery.crashInfo(null);

        const result = diagnosticsZip.buildZip({
          outFile: picked.filePath,
          logsDir,
          checks: this.lastDiagnostics,
          statsSummary: stats ? stats.summary("ghosting-renewed") : null,
          notificationsFile: path.join(app.getPath("userData"), "notifications.json"),
          installedFile: (() => {
            try {
              const { resolveLibrary } = require("./game-delivery/paths");
              return path.join(
                resolveLibrary(this.currentLibraryPath()).games,
                "ghosting-renewed",
                "installed.json"
              );
            } catch (err) {
              return null;
            }
          })(),
          crashInfo: crash,
          discordRpc: this.discordRpc ? this.discordRpc.diagnostics() : null,
          includeDump,
          context: {
            appName: app.getName(),
            appVersion: app.getVersion(),
            buildChannel: process.env.HLSR_BUILD_CHANNEL || "production",
            electronVersion: process.versions.electron,
            chromeVersion: process.versions.chrome,
            nodeVersion: process.versions.node,
            arch: process.arch,
            platform: process.platform,
            osRelease: require("os").release(),
            userName: path.basename(app.getPath("home")),
            libraryPath: this.currentLibraryPath(),
            libraryExists: fs.existsSync(this.currentLibraryPath()),
            updateState: this.updateManager ? this.updateManager.state : "idle",
            env: process.env,
          },
        });

        log.info(`[diagnostics] ZIP экспортирован: ${path.basename(result.path)}`);
        return { ok: true };
      } catch (error) {
        log.warn(`[diagnostics] экспорт ZIP не удался: ${error.message}`);
        return { ok: false, error: "EXPORT_FAILED" };
      }
    });

    ipcMain.handle("notifications-list", () => {
      try {
        return this.notifications.list();
      } catch (e) {
        return { items: [], unread: 0 };
      }
    });
    ipcMain.handle("notifications-mark-read", (e, id) => {
      try {
        return this.notifications.markRead(id);
      } catch (err) {
        return false;
      }
    });
    ipcMain.handle("notifications-mark-all-read", () => {
      try {
        return this.notifications.markAllRead();
      } catch (e) {
        return false;
      }
    });
    ipcMain.handle("notifications-clear", () => {
      try {
        return this.notifications.clear();
      } catch (e) {
        return false;
      }
    });

    ipcMain.on("support-thanks-ready", () => this.thanksEvent.markReady());

    ipcMain.on("ready", (e) => {
      let argv = require("process").argv.slice(1);

      this.gameDelivery.selectGameFromArgs(argv);

      if (!this.nativeAvailable && !IS_LINUX)
        log.warn("Steam integration unavailable (hlsr-native not loaded)");

      try {
        this.versionEvents.resolveUpdateOffers("launcher", app.getVersion());
      } catch (error) {
        log.warn(`[versions] приглашения лаунчера не пересмотрены: ${error.message}`);
      }

      try {
        const receipt = this.versionEvents.peekReceipt();
        const consumed = this.versionEvents.consumeReceipt(app.getVersion());

        if (receipt && receipt.to === app.getVersion())
          this.launcherUpdateEvents.installed(receipt.from || "unknown", receipt.to);

        if (consumed) log.info(`[versions] подтверждена установка ${receipt.to}`);
      } catch (error) {
        log.warn(`[versions] receipt не обработан: ${error.message}`);
      }

      this.updateManager.startCheckingInterval();
    });

    this.updateManager.initialize();
    this.gameDelivery.initialize();
    DemoTool.bind(ipcMain, this.demoTool, trustedRenderer, () => this.window);
    try {
      this.demoTool.attachProtocol(protocol);
    } catch (error) {
      log.error(`[demo] протокол hlsr-media не подключён: ${error.message}`);
    }
    if (this.protonManager)
      this.protonManager
        .ensureReference()
        .catch((error) => log.warn(`[proton] первая загрузка отложена: ${error.message}`))
        .then(() => this.protonManager.checkForUpdate());

    try {
      // Согласие на статистику дают при входе, выключателя больше нет. Кто
      // вошёл раньше и выключил её в настройках, получает его обратно.
      const userData = app.getPath("userData");
      if (this.onlineProfiles && this.onlineProfiles.getAuthToken() && !telemetryConsent.read(userData).enabled) {
        telemetryConsent.set(userData, true);
        log.info("[telemetry] согласие восстановлено: вход в аккаунт выполнен");
      }
      this.telemetry.start();
    } catch (error) {
      log.warn(`[telemetry] не запущена: ${error.message}`);
    }

    try {
      this.onlineClient.start();
      if (this.official) this.onlineProfiles.start();
    } catch (error) {
      log.warn(`[online] опрос не запущен: ${error.message}`);
    }

    this.window.on("show", () => this.onlineClient.fetchOnce().catch(() => {}));
    this.window.on("restore", () => this.onlineClient.fetchOnce().catch(() => {}));
  }

  dispose() {
    try {
      if (this.discordRpc) this.discordRpc.shutdown();
    } catch (e) {
      log.warn("DiscordRPC shutdown failed:", e.message);
    }

    try {
      // Игр нет — канал больше никому не нужен, следующий запуск сменит токен.
      if (this.presenceEndpointFile && !(this.gameDelivery && this.gameDelivery.isAnyGameRunning()))
        presenceEndpoint.clear(this.presenceEndpointFile);
    } catch (e) {
      log.warn("GamePresence endpoint cleanup failed:", e.message);
    }

    try {
      if (this.gamePresence) this.gamePresence.shutdown();
    } catch (e) {
      log.warn("GamePresence shutdown failed:", e.message);
    }

    try {
      this.demoTool.dispose();
    } catch (e) {
      log.warn("DemoTool dispose failed:", e.message);
    }

    try {
      this.gameDelivery.dispose();
    } catch (e) {
      log.warn("GameDelivery dispose failed:", e.message);
    }

    try {
      this.updateManager.dispose();
    } catch (e) {
      log.warn("UpdateManager dispose failed:", e.message);
    }

    try {
      this.telemetry.shutdown();
    } catch (e) {
      log.warn("Telemetry shutdown failed:", e.message);
    }

    try {
      this.onlineClient.stop();
      this.onlineProfiles.stop();
    } catch (e) {
      log.warn("Online client stop failed:", e.message);
    }
  }
}

export default AppManager;

export { withHlsrFolder, LIBRARY_FOLDER_NAME };
