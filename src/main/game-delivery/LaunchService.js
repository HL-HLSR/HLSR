"use strict";

const nodeFs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const SteamLocator = require("./SteamLocator");
const manifestRegistry = require("./manifests");
const defaultInspector = require("./process-inspect");
const { powershellPath } = require("../windows-tools");
const launchArgs = require("./launch-args");
const gameLanguage = require("./game-language");
const defaultConfigs = require("./default-configs");
const buildMetadataTrust = require("./build-metadata-trust");
const goldsrc = require("./goldsrc-launch");
const depCompat = require("./dep-compat");
const rinputMenuFix = require("./rinput-menu-fix-win");
const vulkanBackend = require("./vulkan-backend");
const episodes = require("./episodes");
const episodeInstall = require("./episode-install");
const { assertInside } = require("./paths");
const { DeliveryError, toDeliveryError } = require("./errors");

const DEFINITE_STEAM_CODES = [
  "STEAM_APP_NOT_OWNED",
  "STEAM_NOT_RUNNING",
  "STEAM_NOT_INSTALLED",
  "STEAM_PROCESS_MISMATCH",
  "STEAM_ELEVATED",
];

const UNCERTAIN_LAUNCH_CODES = ["LAUNCH_FAILED", "STEAM_NOT_RUNNING"];

const EARLY_EXIT_WINDOW_MS = 1500;

// Инжектор Bunnymod XT запускает hl.exe, внедряет DLL и сам завершается.
const INJECTED_GAME_TIMEOUT_MS = 30000;
const INJECTED_GAME_POLL_MS = 500;
const INJECTOR_EXIT_GRACE_MS = 5000;

// Курсор RInput в меню: сколько ждать, пока RInput поставит перехваты.
const RINPUT_PATCH_WAIT_MS = 30000;
const RINPUT_PATCH_POLL_MS = 500;

const STRIPPED_ENV = ["SteamAppId", "SteamGameId", "SteamOverlayGameId", "SteamAppUser"];

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function normalizeAppIdText(raw) {
  return String(raw == null ? "" : raw)
    .replace(/^\uFEFF/, "")
    // eslint-disable-next-line no-control-regex
    .replace(/\x00+/g, "")
    .trim();
}

class LaunchService {
  constructor(options) {
    const opts = options || {};

    this.delivery = opts.delivery;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.fs = opts.fs || nodeFs;
    this.env = opts.env || process.env;
    this.inspector = opts.inspector || defaultInspector;
    this.spawnFn = opts.spawnFn || spawn;
    this.registry = opts.launchRegistry || manifestRegistry.LAUNCH_CONFIG;
    this.settingsStore = opts.settingsStore || null;
    this.logsDir = opts.logsDir || null;
    this.dependencyUpdater = opts.dependencyUpdater || null;
    this.packApplier = opts.packApplier || null;
    this.extraEnvProvider = opts.extraEnvProvider || null;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.locator = opts.steamLocator || new SteamLocator({ logger: this.logger });
    this.depotService = opts.depotService || null;
    this.earlyExitWindowMs =
      opts.earlyExitWindowMs == null ? EARLY_EXIT_WINDOW_MS : opts.earlyExitWindowMs;
    this.injectedGameTimeoutMs =
      opts.injectedGameTimeoutMs == null ? INJECTED_GAME_TIMEOUT_MS : opts.injectedGameTimeoutMs;
    this.injectedGamePollMs =
      opts.injectedGamePollMs == null ? INJECTED_GAME_POLL_MS : opts.injectedGamePollMs;

    this.rinputMenuSession = opts.rinputMenuSession || ((options) => rinputMenuFix.openSession(options));
    this.rinputPatchWaitMs = opts.rinputPatchWaitMs == null ? RINPUT_PATCH_WAIT_MS : opts.rinputPatchWaitMs;
    this.rinputPatchPollMs = opts.rinputPatchPollMs == null ? RINPUT_PATCH_POLL_MS : opts.rinputPatchPollMs;
    this.rinputPatch = null;
    this.liveSplitOverlay = opts.liveSplitOverlay || null;
    this.depCompat = opts.depCompat || depCompat;
    this.platform = opts.platform || process.platform;
    this.depState = null;

    this.launching = new Set();
  }

  configFor(gameId) {
    const config = this.registry[gameId];

    if (!config)
      throw new DeliveryError(
        "LAUNCH_CONFIG_INVALID",
        `Для игры ${gameId} нет доверенной конфигурации запуска`,
        { gameId }
      );

    if (!Number.isInteger(config.steamAppId) || config.steamAppId <= 0)
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", "Некорректный steamAppId в конфигурации");

    if (typeof config.executable !== "string" || !config.executable || path.isAbsolute(config.executable))
      throw new DeliveryError(
        "LAUNCH_CONFIG_INVALID",
        "Исполняемый файл задаётся относительным путём внутри сборки"
      );

    if (typeof config.gameDirectory !== "string" || !config.gameDirectory)
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", "Не задан каталог мода");

    if (!Array.isArray(config.arguments) || config.arguments.some((a) => typeof a !== "string"))
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", "Аргументы запуска должны быть массивом строк");

    return config;
  }

  resolveGameFiles(installPath, config) {
    const executable = assertInside(
      installPath,
      path.join(installPath, config.executable.replace(/\//g, path.sep)),
      "Исполняемый файл игры"
    );

    const gameDirectory = assertInside(
      installPath,
      path.join(installPath, config.gameDirectory.replace(/\//g, path.sep)),
      "Каталог мода"
    );

    if (!this.fs.existsSync(executable))
      throw new DeliveryError(
        "GAME_EXECUTABLE_MISSING",
        `В установленной сборке нет ${config.executable}`,
        { executable }
      );

    let gameDirStats = null;
    try {
      gameDirStats = this.fs.statSync(gameDirectory);
    } catch (error) {
      gameDirStats = null;
    }

    if (!gameDirStats || !gameDirStats.isDirectory())
      throw new DeliveryError(
        "GAME_DIRECTORY_MISSING",
        `В установленной сборке нет каталога ${config.gameDirectory}`,
        { gameDirectory }
      );

    const gameinfo = path.join(gameDirectory, "gameinfo.txt");

    if (!this.fs.existsSync(gameinfo))
      throw new DeliveryError(
        "GAME_DIRECTORY_MISSING",
        `В каталоге мода нет gameinfo.txt`,
        { gameinfo }
      );

    return { executable, gameDirectory, gameinfo };
  }

  enforceSteamAppIdFile(installPath, config) {
    const file = assertInside(installPath, path.join(installPath, "steam_appid.txt"), "steam_appid.txt");
    const expected = config.steamAppIdFileText || `${config.steamAppId}\n`;

    let current = null;

    try {
      current = this.fs.readFileSync(file, "utf8");
    } catch (error) {
      current = null;
    }

    if (current !== null && normalizeAppIdText(current) === String(config.steamAppId)) return;

    try {
      this.fs.writeFileSync(file, expected);
      this.logger.info(
        `[launch] steam_appid.txt приведён к ${config.steamAppId}` +
        (current !== null ? ` (было "${normalizeAppIdText(current)}")` : "")
      );
    } catch (error) {
      throw new DeliveryError(
        "STEAM_APP_ID_MISMATCH",
        `Не удалось записать steam_appid.txt: ${error.message}`,
        { file }
      );
    }
  }

  assertSteamAppIdFile(installPath, config) {
    const file = assertInside(installPath, path.join(installPath, "steam_appid.txt"), "steam_appid.txt");

    let raw;
    try {
      raw = this.fs.readFileSync(file, "utf8");
    } catch (error) {
      throw new DeliveryError(
        "STEAM_APP_ID_MISMATCH",
        "В установленной сборке нет steam_appid.txt",
        { file }
      );
    }

    const value = normalizeAppIdText(raw);

    if (value !== String(config.steamAppId))
      throw new DeliveryError(
        "STEAM_APP_ID_MISMATCH",
        `steam_appid.txt объявляет "${value}", ожидался ${config.steamAppId}`,
        { file, expected: String(config.steamAppId), actual: value }
      );

    return file;
  }

  assertGameInfoAppId(gameinfo, config) {
    let text;
    try {
      text = this.fs.readFileSync(gameinfo, "utf8");
    } catch (error) {
      throw new DeliveryError("GAME_DIRECTORY_MISSING", `gameinfo.txt не читается: ${error.message}`);
    }

    const match = text.match(/^[ \t]*SteamAppId[ \t]+(\d+)[ \t]*(?:\/\/.*)?$/im);

    if (!match)
      throw new DeliveryError(
        "STEAM_APP_ID_MISMATCH",
        "В gameinfo.txt не найден SteamAppId",
        { gameinfo }
      );

    if (Number(match[1]) !== config.steamAppId)
      throw new DeliveryError(
        "STEAM_APP_ID_MISMATCH",
        `gameinfo.txt использует SteamAppId ${match[1]}, ожидался ${config.steamAppId}`,
        { gameinfo, expected: config.steamAppId, actual: Number(match[1]) }
      );
  }

  buildEnvironment() {
    const env = Object.assign({}, this.env);

    for (const name of STRIPPED_ENV)
      for (const key of Object.keys(env))
        if (key.toLowerCase() === name.toLowerCase()) delete env[key];

    if (typeof this.extraEnvProvider === "function") {
      let extra = null;
      try {
        extra = this.extraEnvProvider();
      } catch (error) {
        this.logger.warn(`[launch] дополнительное окружение недоступно: ${error.message}`);
      }
      if (extra && typeof extra === "object")
        for (const key of Object.keys(extra)) {
          if (!/^HLSR_[A-Z0-9_]+$/.test(key)) continue;
          const value = extra[key];
          if (typeof value !== "string" || value.length === 0) continue;
          env[key] = value;
        }
    }

    return env;
  }

  assertBuildMetadata(installed) {
    if (!installed || !installed.buildMetadata) return;

    const result = buildMetadataTrust.verifyAgainstSnapshot({
      installRoot: installed.installPath,
      snapshot: installed,
      launcherVersion: this.launcherVersion,
    });

    if (result.state === "trusted" || result.state === "legacy") return;

    const code = result.reason || "BUILD_METADATA_MISMATCH";
    throw new DeliveryError(code, `Сборка не подтверждена метаданными: ${code}`, result.detail || null);
  }

  // options.episode ("ep1" | "ep2") — эпизод Ghosting: тот же hl2.exe с -game episodic|ep2.
  async launch(gameId, options = {}) {
    const episode = options && options.episode != null ? options.episode : null;
    const base = this.configFor(gameId);

    if (episode !== null && (gameId !== episodeInstall.GAME_ID || !episodes.isEpisode(episode)))
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", `Эпизод ${episode} не запускается для ${gameId}`, {
        gameId,
        episode: String(episode),
      });

    // Каталог мода, gameinfo.txt и -game — эпизода; конфиг игрока и язык — Ghosting (base).
    const config = episode ? episodes.launchConfig(base, episode) : base;

    if (this.launching.has(gameId))
      throw new DeliveryError("ALREADY_RUNNING", "Запуск уже выполняется", { gameId });

    const processService = this.delivery.processService;

    if (processService.isRunning(gameId))
      return { status: "already-running", process: processService.getProcessState(gameId) };

    if (this.delivery.isBusy())
      throw new DeliveryError("ALREADY_RUNNING", "Дождитесь завершения текущей операции", { gameId });

    const installed = this.delivery.installedOf(gameId);

    if (episode && !episodes.status(installed.installPath, { fs: this.fs })[episode].installed)
      throw new DeliveryError("NOT_INSTALLED", `Эпизод ${episode} не установлен`, { gameId, episode });

    this.launching.add(gameId);
    processService.markStarting(gameId);

    try {
      // config.cfg эпизода, не вернувшийся после краха игры или лаунчера, — в Ghosting до её запуска.
      if (gameId === episodeInstall.GAME_ID)
        episodes.returnPending(installed.installPath, { fs: this.fs, logger: this.logger });

      this.assertBuildMetadata(installed);

      // Все сборки идут с -steam, а engine.dll без клиента Steam не стартует
      // («Steam must be running…»): отказываем заранее, и лаунчер предлагает
      // открыть Steam, как на Linux. sourceSdkRequired: false снимает только
      // проверку Source SDK Base, но не самого Steam.
      const preflight = config.sourceSdkRequired !== false
        ? this.locator.preflight(config.steamAppId) : this.assertSteamRunning();

      await this.assertOwnedApps(gameId, config);

      if (goldsrc.isGoldSrc(config)) return await this.launchGoldSrc(gameId, installed, config);

      const files = this.resolveGameFiles(installed.installPath, config);
      if (config.ownsSteamAppIdFile) this.enforceSteamAppIdFile(installed.installPath, config);
      else this.assertSteamAppIdFile(installed.installPath, config);

      this.assertGameInfoAppId(files.gameinfo, config);

      const settings = this.settingsStore ? this.settingsStore.get(gameId) : null;
      const argv = launchArgs.mergeLaunchArguments(config.arguments, settings);

      // Языковая подмена — в ghostingmod/custom: эпизоды монтируют ../ghostingmod/custom/*.
      this.applyLanguage(gameId, installed.installPath, base, settings);

      // Файл игрока один — в ghostingmod/cfg; в эпизод его копирует syncConfigIn ниже.
      this.prepareConfigs(gameId, installed, base, settings);

      const vulkan = this.prepareVulkan(installed, config, settings);

      this.prepareOverlay(gameId, installed, config, settings, vulkan);

      if (episode)
        episodes.syncConfigIn(installed.installPath, episode, {
          fs: this.fs,
          ghostingStock: this.ghostingStockOf(gameId, installed),
        });

      const child = this.spawnFn(files.executable, argv, {
        cwd: installed.installPath,
        windowsHide: false,
        shell: false,
        detached: true,
        stdio: "ignore",
        env: vulkanBackend.applyEnvironment(this.buildEnvironment(), installed.installPath, vulkan),
      });

      const started = await this.waitForStart(gameId, child, files.executable);

      this.applyAffinity(gameId, started.pid);

      const state = this.delivery.registerStartedProcess(gameId, {
        pid: started.pid,
        executablePath: started.executablePath,
        startedAt: started.startedAt,
        child,
        episode,
      });

      if (typeof child.unref === "function") child.unref();

      this.logger.info(
        `[launch] ${gameId}${episode ? ` (эпизод ${episode})` : ""} запущена: PID ${started.pid}, ${files.executable}, ` +
          `аргументы [${argv.join(" ")}], AppID ${config.steamAppId}` +
          (preflight ? `, Steam PID ${preflight.steam.pid}` : "")
      );

      return { status: "started", process: state };
    } catch (error) {
      const delivery = this.explainWithSteamState(toDeliveryError(error, "LAUNCH_FAILED"));

      processService.cancelStarting(gameId, `${delivery.code}: ${delivery.message}`);

      this.logger.error(`[launch] запуск ${gameId} не удался: ${delivery.code} ${delivery.message}`);
      throw delivery;
    } finally {
      this.launching.delete(gameId);
    }
  }

  // Имена cfg сборки Ghosting (ghostingmod/cfg из files.json установленной версии): их
  // syncConfigIn в эпизод не берёт. null — манифест недоступен, идут только autoexec и config.
  ghostingStockOf(gameId, installed) {
    try {
      const manifest = this.delivery.filesManifestFor(gameId, installed.version);
      return manifest.files
        .map((file) => /^ghostingmod\/cfg\/([^/]+)$/.exec(String(file.path).toLowerCase()))
        .filter(Boolean)
        .map((match) => match[1]);
    } catch (error) {
      this.logger.warn(`[launch] список cfg сборки Ghosting недоступен: ${error.message}`);
      return null;
    }
  }

  // Файл игрока на запуске не переписывается: только переход с 0.4.8 или создание отсутствующего.
  prepareConfigs(gameId, installed, config, settings) {
    try {
      defaultConfigs.prepareForLaunch(gameId, installed.installPath, config, settings, {
        launcherVersion: this.launcherVersion,
        fs: this.fs,
      });
    } catch (error) {
      this.logger.warn(`[launch] конфиг игрока не подготовлен: ${error.message}`);
    }
  }

  // HL1 WON: hl.exe стартует через инжектор Bunnymod XT; отслеживаем сам hl.exe.
  async launchGoldSrc(gameId, installed, config) {
    const dependencies = this.dependencyUpdater
      ? await this.dependencyUpdater(installed.installPath, config) : [];
    const files = goldsrc.resolveFiles(installed.installPath, config, this.fs);
    goldsrc.enforceSteamAppIdFile(installed.installPath, config, this.fs, this.logger);

    const settings = this.settingsStore ? this.settingsStore.get(gameId) : null;
    this.prepareConfigs(gameId, installed, config, settings);
    const packs = this.packApplier ? await this.packApplier(installed.installPath, config, settings) : null;
    const argv = goldsrc.mergeArguments(config, settings);
    const env = Object.assign(this.buildEnvironment(), goldsrc.environmentOf(config));
    const bxtLog = goldsrc.prepareBxtLog(this.logsDir, this.fs, this.logger);
    if (bxtLog) env.BXT_LOGFILE = bxtLog;
    this.prepareDep(files.executable);
    this.prepareOverlay(gameId, installed, config, settings);
    const spawnedAt = Date.now();

    const child = this.spawnFn(files.injector, [files.executable, ...argv], {
      cwd: path.dirname(files.executable),
      windowsHide: false,
      shell: false,
      detached: true,
      stdio: "ignore",
      env,
    });

    const started = await this.waitForInjectedGame(child, files.executable, spawnedAt);

    this.applyAffinity(gameId, started.pid);

    const state = this.delivery.registerStartedProcess(gameId, {
      pid: started.pid,
      executablePath: started.executablePath,
      startedAt: started.startedAt,
    });

    if (typeof child.unref === "function") child.unref();

    this.startRInput(installed, config, settings, started.pid);

    this.logger.info(
      `[launch] ${gameId} запущена через Bunnymod XT: PID ${started.pid}, ${files.executable}, ` +
        `аргументы [${argv.join(" ")}], AppID ${config.steamAppId}`
    );

    return { status: "started", process: state, dependencies, packs };
  }

  // hl.exe из WON падает при DEP в режиме OptOut (dep-compat.js): перед
  // запуском снимаем DEP с него слоем совместимости, а режим системы узнаём
  // в фоне, чтобы при падении подсказать, что делать.
  prepareDep(executable) {
    if (this.platform !== "win32") return null;
    const exemption = this.depCompat.ensureExemption(executable, { logger: this.logger });
    const state = { exemption, policy: this.depState ? this.depState.policy : null };
    this.depState = state;
    if (state.policy == null)
      this.depCompat.queryPolicy().then((policy) => {
        state.policy = policy;
        this.logger.info(
          `[dep] режим DEP: ${policy == null ? "не определён" : this.depCompat.POLICY[policy]}, слой для hl.exe: ${exemption}`
        );
      });
    return state;
  }

  depCrashHint(exitCode) {
    return this.depCompat.crashHint(this.depState, exitCode);
  }

  // Vulkan (vulkan-backend.js): включён в настройках и DXVK есть в папке игры.
  // Нет DXVK — игра идёт на DirectX 9, как без переключателя.
  prepareVulkan(installed, config, settings) {
    if (!settings || settings.vulkan !== true) return false;
    const available = vulkanBackend.availability(installed.installPath, config, this.fs, this.platform);
    if (available === null) return false;
    if (!available) {
      this.logger.warn(`[launch] Vulkan включён, но в папке игры нет ${vulkanBackend.BACKEND}: запуск на DirectX 9`);
      return false;
    }
    for (const dir of [vulkanBackend.LOG_DIR, vulkanBackend.CACHE_DIR]) {
      try {
        this.fs.mkdirSync(path.join(installed.installPath, dir), { recursive: true });
      } catch (error) {
        this.logger.warn(`[launch] Vulkan: папка ${dir} не создана: ${error.message}`);
      }
    }
    this.logger.info(`[launch] Vulkan (DXVK): журнал в ${path.join(installed.installPath, vulkanBackend.LOG_DIR)}`);
    return true;
  }

  // Оверлей LiveSplit (livesplit-overlay.js): прокси d3d9.dll или
  // opengl32.dll кладётся в папку игры до запуска, игра грузит его сама. Ничего
  // не внедряется. Сбой оверлея игру не останавливает. vulkan — прокси нужен
  // и с выключенным оверлеем: через него игра грузит DXVK.
  prepareOverlay(gameId, installed, config, settings, vulkan = false) {
    if (!this.liveSplitOverlay) return null;
    try {
      const prepared = this.liveSplitOverlay.prepare(gameId, settings, { installPath: installed.installPath, config }, { proxy: vulkan });
      if (!prepared) return null;
      if (prepared.placed.length)
        this.logger.info(`[launch] оверлей LiveSplit: ${prepared.proxy} в ${prepared.placed.join(", ")}, журнал: ${prepared.log}`);
      else
        this.logger.warn(`[launch] оверлей LiveSplit: ${prepared.proxy} в папку игры не положен (чужой файл или нет папки)`);
      return prepared;
    } catch (error) {
      this.logger.warn(`[launch] оверлей LiveSplit не подготовлен: ${error.message}`);
      return null;
    }
  }

  // hl.exe уже найден, так что RInput внедрится сразу и завершится сам.
  // Сбой RInput игру не останавливает.
  startRInput(installed, config, settings, gamePid) {
    const rinput = goldsrc.resolveRInput(installed.installPath, config, settings, this.fs);
    if (!rinput) return;
    if (rinput.missing) {
      this.logger.warn(`[launch] RInput не найден: ${rinput.executable}`);
      return;
    }
    try {
      const child = this.spawnFn(rinput.executable, [rinput.processName], {
        cwd: path.dirname(rinput.executable),
        windowsHide: true,
        shell: false,
        detached: true,
        stdio: "ignore",
      });
      child.once("error", (error) => this.logger.warn(`[launch] RInput не запущен: ${error.message}`));
      child.once("exit", (code) => this.logger.info(`[launch] RInput завершился (код ${code})`));
      if (typeof child.unref === "function") child.unref();
    } catch (error) {
      this.logger.warn(`[launch] RInput не запущен: ${error.message}`);
      return;
    }
    if (launchArgs.sanitizeSettings(settings).settings.rinputMenuFix)
      this.rinputPatch = this.patchRInputMenu(rinput.processName, gamePid)
        .catch((error) => this.logger.warn(`[launch] курсор RInput в меню не исправлен: ${error.message}`));
  }

  // Курсор в меню: когда RInput поставит свои перехваты в hl.exe, патчим их
  // в памяти (см. rinput-menu-fix.js). Сбой патча игру не трогает.
  async patchRInputMenu(processName, gamePid) {
    const alive = () => !Number.isInteger(gamePid) || this.inspector.isAlive(gamePid);
    const session = this.rinputMenuSession({ spawnFn: this.spawnFn });
    const deadline = Date.now() + this.rinputPatchWaitMs;
    try {
      while (alive()) {
        let status;
        try {
          status = await session.patch(processName);
        } catch (error) {
          this.logger.warn(`[launch] курсор RInput в меню не исправлен: ${error.message}`);
          return;
        }
        if (status === "patched" || status === "already") {
          this.logger.info("[launch] курсор RInput в меню исправлен");
          return;
        }
        if (status === "unsupported") {
          this.logger.warn("[launch] курсор RInput в меню не исправлен: незнакомая версия RInput");
          return;
        }
        if (Date.now() >= deadline) {
          this.logger.warn(`[launch] курсор RInput в меню не исправлен: RInput не перехватил курсор за ${Math.round(this.rinputPatchWaitMs / 1000)} с`);
          return;
        }
        await delay(this.rinputPatchPollMs);
      }
    } finally {
      session.close();
    }
  }

  async waitForInjectedGame(child, expectedExecutable, spawnedAt) {
    let injectorExit = null;
    let injectorError = null;

    child.once("exit", (code) => {
      injectorExit = { code, at: Date.now() };
    });
    child.once("error", (error) => {
      injectorError = error;
    });

    const imageName = path.basename(expectedExecutable);
    const deadline = Date.now() + this.injectedGameTimeoutMs;

    while (Date.now() < deadline) {
      if (injectorError)
        throw new DeliveryError("LAUNCH_FAILED", `Инжектор Bunnymod XT не запустился: ${injectorError.message}`);

      const found = (this.inspector.findByImageName(imageName) || [])
        .filter((item) => item.executablePath && this.inspector.samePath(item.executablePath, expectedExecutable))
        .filter((item) => !item.startedAt || Date.parse(item.startedAt) >= spawnedAt - 2000)
        .filter((item) => this.inspector.isAlive(item.pid))
        .sort((a, b) => Date.parse(b.startedAt || 0) - Date.parse(a.startedAt || 0));

      if (found.length)
        return {
          pid: found[0].pid,
          executablePath: found[0].executablePath,
          startedAt: found[0].startedAt || new Date().toISOString(),
        };

      if (injectorExit && injectorExit.code !== 0)
        throw new DeliveryError(
          "LAUNCH_FAILED",
          `Инжектор Bunnymod XT завершился с кодом ${injectorExit.code == null ? "?" : injectorExit.code}`,
          { exitCode: injectorExit.code }
        );

      if (injectorExit && Date.now() - injectorExit.at > INJECTOR_EXIT_GRACE_MS)
        throw new DeliveryError("LAUNCH_FAILED", "Инжектор Bunnymod XT завершился, но игра не запустилась");

      await delay(this.injectedGamePollMs);
    }

    throw new DeliveryError("LAUNCH_FAILED", "Игра не запустилась через Bunnymod XT за отведённое время");
  }

  assertSteamRunning() {
    const steam = this.locator.describeSteamOnly();
    if (!steam || !steam.ok)
      throw new DeliveryError(
        (steam && steam.code) || "STEAM_NOT_RUNNING",
        (steam && steam.message) || "Steam не запущен"
      );
    return steam;
  }

  explainWithSteamState(delivery) {
    if (!delivery || !UNCERTAIN_LAUNCH_CODES.includes(delivery.code)) return delivery;
    if (!this.locator || typeof this.locator.describeSteamOnly !== "function") return delivery;

    let state = null;

    try {
      state = this.locator.describeSteamOnly();
    } catch (error) {
      return delivery;
    }

    if (!state || state.code !== "STEAM_ELEVATED") return delivery;

    this.logger.info(
      `[launch] причина уточнена: ${delivery.code} -> STEAM_ELEVATED (${state.message || "права Steam и лаунчера разошлись"})`
    );

    return new DeliveryError("STEAM_ELEVATED", state.message || delivery.message, {
      originalCode: delivery.code,
      originalMessage: delivery.message,
    });
  }

  async assertOwnedApps(gameId, config) {
    const appIds = config.requiresOwnedAppIds || [];
    if (!appIds.length || !this.depotService) return null;

    try {
      await this.depotService.assertOwns(appIds);
      return { owned: true };
    } catch (error) {
      if (DEFINITE_STEAM_CODES.indexOf(error && error.code) !== -1) throw error;

      this.logger.warn(
        `[launch] ${gameId}: лицензию подтвердить не удалось (${(error && error.code) || "?"}: ` +
          `${(error && error.message) || error}) — запуск продолжается, решение за игрой`
      );

      return { owned: null };
    }
  }

  applyLanguage(gameId, installPath, config, settings) {
    if (!config || typeof config.gameDirectory !== "string") return null;

    const choice = launchArgs.languageOf(settings || {});

    let report;
    try {
      report = gameLanguage.apply(installPath, config.gameDirectory, choice);
    } catch (error) {
      this.logger.warn(`[launch] ${gameId}: язык не применён (${error.message})`);
      return null;
    }

    const QUIET = ["AUTO", "SAME_AS_ENGINE"];

    if (report.applied)
      this.logger.info(
        `[launch] ${gameId}: язык ${report.language} подменой ` +
          `${report.files.length} файлов (движок подставит ${report.engineLanguage})`
      );
    else if (!report.ok)
      this.logger.warn(
        `[launch] ${gameId}: язык не применён (${report.reason})` +
          (report.message ? `: ${report.message}` : "")
      );
    else if (QUIET.indexOf(report.reason) === -1)
      this.logger.info(`[launch] ${gameId}: язык остаётся как в Steam (${report.reason})`);

    return report;
  }

  applyAffinity(gameId, pid) {
    const settings = this.settingsStore ? this.settingsStore.get(gameId) : null;
    if (!settings || settings.excludeCpu0 !== true) return null;

    if (process.platform !== "win32") return null;
    if (!Number.isInteger(pid) || pid <= 0) return null;

    const mask = launchArgs.affinityMaskWithoutCpu0(os.cpus().length);

    if (!mask) {
      this.logger.warn(
        `[launch] ${gameId}: процессоров меньше двух, оставить игру без ЦП 0 невозможно`
      );
      return null;
    }

    try {
      const result = this.spawnFn(
        powershellPath(),
        [
          "-NoProfile",
          "-NonInteractive",
          "-Command",
          `(Get-Process -Id ${pid}).ProcessorAffinity = ${mask}`,
        ],
        { windowsHide: true, shell: false, stdio: "ignore", detached: false }
      );

      if (result && typeof result.unref === "function") result.unref();

      this.logger.info(
        `[launch] ${gameId}: маска процессоров 0x${BigInt(mask).toString(16).toUpperCase()} ` +
          `(${os.cpus().length} ЦП, нулевой исключён)`
      );

      return mask;
    } catch (error) {
      this.logger.warn(`[launch] ${gameId}: маску процессоров применить не удалось: ${error.message}`);
      return null;
    }
  }

  waitForStart(gameId, child, expectedExecutable) {
    return new Promise((resolve, reject) => {
      let exited = null;
      let errored = null;

      const onExit = (code, signal) => {
        exited = { code, signal };
      };

      const onError = (error) => {
        errored = error;
      };

      child.once("exit", onExit);
      child.once("error", onError);

      const cleanup = () => {
        child.removeListener("exit", onExit);
        child.removeListener("error", onError);
      };

      const fail = (error) => {
        cleanup();
        reject(error);
      };

      const settle = async () => {
        await delay(this.earlyExitWindowMs);

        if (errored)
          return fail(
            new DeliveryError("LAUNCH_FAILED", `Процесс игры не запустился: ${errored.message}`)
          );

        if (exited)
          return fail(
            new DeliveryError(
              "LAUNCH_FAILED",
              `Игра завершилась сразу после запуска (код ${exited.code == null ? "?" : exited.code}). ` +
                "Обычно это означает, что Steam отклонил запуск.",
              { exitCode: exited.code, signal: exited.signal }
            )
          );

        if (!child.pid || !this.inspector.isAlive(child.pid))
          return fail(new DeliveryError("LAUNCH_FAILED", "Процесс игры не найден после запуска"));

        const actual = this.inspector.inspect(child.pid);
        let startedAt = new Date().toISOString();
        let executablePath = expectedExecutable;

        if (actual) {
          if (
            actual.executablePath &&
            !this.inspector.samePath(actual.executablePath, expectedExecutable)
          )
            return fail(
              new DeliveryError(
                "LAUNCH_FAILED",
                `PID ${child.pid} принадлежит другому файлу: ${actual.executablePath}`
              )
            );

          if (actual.startedAt) startedAt = actual.startedAt;
          if (actual.executablePath) executablePath = actual.executablePath;
        } else {
          this.logger.warn(
            `[launch] система не отдала данные о PID ${child.pid} — использую данные spawn`
          );
        }

        cleanup();
        resolve({ pid: child.pid, executablePath, startedAt });
      };

      settle();
    });
  }

  describeAvailability(gameId) {
    let config = null;

    try {
      config = this.configFor(gameId);
    } catch (error) {
      return { ok: false, code: "LAUNCH_CONFIG_INVALID" };
    }

    if (config.sourceSdkRequired === false) return this.locator.describeSteamOnly();

    return this.locator.describeAvailability(config.steamAppId);
  }
}

module.exports = LaunchService;
module.exports.EARLY_EXIT_WINDOW_MS = EARLY_EXIT_WINDOW_MS;
module.exports.STRIPPED_ENV = STRIPPED_ENV;
module.exports.normalizeAppIdText = normalizeAppIdText;
