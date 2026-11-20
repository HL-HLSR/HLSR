"use strict";

const fs = require("fs");
const path = require("path");
const manifestRegistry = require("../game-delivery/manifests");
const launchArgs = require("../game-delivery/launch-args");
const gameLanguage = require("../game-delivery/game-language");
const defaultConfigs = require("../game-delivery/default-configs");
const buildMetadataTrust = require("../game-delivery/build-metadata-trust");
const goldsrc = require("../game-delivery/goldsrc-launch");
const SteamLocator = require("./SteamLocator");
const rinputMenuFix = require("./rinput-menu-fix");
const LiveSplitOverlay = require("../game-delivery/livesplit-overlay");
const PresenceRelay = require("./PresenceRelay");
const { findCaseConflict } = require("./path-case");
const { windowsPathOf } = require("./ProtonRunner");
const { assertInside } = require("../game-delivery/paths");
const { DeliveryError, toDeliveryError } = require("../game-delivery/errors");

function delay(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }

// Bunnymod XT уже внутри hl.exe: Wine называет процесс по имени exe,
// а внедрённая DLL видна в его карте памяти.
function bxtInjected() {
  let entries;
  try { entries = fs.readdirSync("/proc"); } catch (error) { return false; }
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      if (fs.readFileSync(`/proc/${entry}/comm`, "utf8").trim().toLowerCase() !== "hl.exe") continue;
      if (/bunnymodxt\.dll/i.test(fs.readFileSync(`/proc/${entry}/maps`, "utf8"))) return true;
    } catch (error) { /* процесс уже завершился или чужой */ }
  }
  return false;
}

// Процесс игры уже есть: Wine называет процесс по имени exe.
function processRunning(name) {
  let entries;
  try { entries = fs.readdirSync("/proc"); } catch (error) { return false; }
  const wanted = String(name).toLowerCase().slice(0, 15);
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      if (fs.readFileSync(`/proc/${entry}/comm`, "utf8").trim().toLowerCase() === wanted) return true;
    } catch (error) { /* процесс уже завершился или чужой */ }
  }
  return false;
}

// Процесс игры в префиксе gameId: имя exe и префикс в окружении процесса
// (STEAM_COMPAT_DATA_PATH или WINEPREFIX ведут в compatdata/<gameId>).
// Окружение без доступа не мешает: тогда хватает имени.
function gameProcessRunning(name, gameId) {
  let entries;
  try { entries = fs.readdirSync("/proc"); } catch (error) { return false; }
  const wanted = String(name).toLowerCase().slice(0, 15);
  const prefix = `/compatdata/${gameId}/`;
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      if (fs.readFileSync(`/proc/${entry}/comm`, "utf8").trim().toLowerCase() !== wanted) continue;
    } catch (error) { continue; /* процесс уже завершился или чужой */ }
    let environ = null;
    try { environ = fs.readFileSync(`/proc/${entry}/environ`, "utf8"); } catch (error) { /* нет доступа */ }
    if (environ === null || environ.includes(prefix)) return true;
  }
  return false;
}

// Параметры Proton игрока (см. parseProtonOptions): переменные идут поверх
// окружения запуска, обёртка — вокруг umu-run, аргументы — в конец игры.
function protonOptionsOf(settings) {
  const value = launchArgs.sanitizeSettings(settings).settings.protonOptions;
  const parsed = launchArgs.parseProtonOptions(value);
  return parsed.ok ? parsed : { env: {}, wrapper: [], args: [] };
}

class LaunchService {
  constructor(options = {}) {
    this.delivery = options.delivery;
    this.runner = options.protonRunner;
    this.logger = options.logger || { info() {}, warn() {}, error() {} };
    this.registry = options.launchRegistry || manifestRegistry.LAUNCH_CONFIG;
    this.settingsStore = options.settingsStore || null;
    this.logsDir = options.logsDir || null;
    this.dependencyUpdater = options.dependencyUpdater || null;
    this.packApplier = options.packApplier || null;
    this.launcherVersion = options.launcherVersion || "0.0.0";
    this.steamLocator = options.steamLocator || new SteamLocator({ logger: this.logger });
    this.killProcessGroup = options.killProcessGroup || ((pid) => process.kill(-pid, "SIGTERM"));
    this.bxtInjected = options.bxtInjected || bxtInjected;
    this.bxtWaitMs = options.bxtWaitMs == null ? 30000 : options.bxtWaitMs;
    this.bxtPollMs = options.bxtPollMs == null ? 500 : options.bxtPollMs;
    this.rinputMenuPatcher = options.rinputMenuPatcher || rinputMenuFix.patchRunningGame;
    this.rinputPatchWaitMs = options.rinputPatchWaitMs == null ? 30000 : options.rinputPatchWaitMs;
    this.liveSplitOverlay = options.liveSplitOverlay || null;
    this.presenceRelay = options.presenceRelay || null;
    this.extraEnvProvider = options.extraEnvProvider || null;
    this.presenceWaitMs = options.presenceWaitMs == null ? 20000 : options.presenceWaitMs;
    this.processRunning = options.processRunning || processRunning;
    this.gameProcessRunning = options.gameProcessRunning || gameProcessRunning;
    this.gameWatchMs = options.gameWatchMs == null ? 2000 : options.gameWatchMs;
    this.presenceStartWaitMs = options.presenceStartWaitMs == null ? 60000 : options.presenceStartWaitMs;
    this.presenceSettleMs = options.presenceSettleMs == null ? 5000 : options.presenceSettleMs;
    this.launching = new Set();
  }

  configFor(gameId) {
    const config = this.registry[gameId];
    if (!config || !config.executable || path.isAbsolute(config.executable) ||
        !Array.isArray(config.arguments) || !config.gameDirectory)
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", `Некорректная конфигурация запуска ${gameId}`);
    return config;
  }

  filesFor(installPath, config) {
    const executable = assertInside(installPath, path.join(installPath, config.executable));
    const gameDirectory = assertInside(installPath, path.join(installPath, config.gameDirectory));
    if (!fs.existsSync(executable))
      throw new DeliveryError("GAME_EXECUTABLE_MISSING", `Игра не найдена: ${config.executable}`);
    if (!fs.existsSync(path.join(gameDirectory, "gameinfo.txt")))
      throw new DeliveryError("GAME_DIRECTORY_MISSING", `Не найден gameinfo.txt в ${config.gameDirectory}`);
    return { executable, gameDirectory };
  }

  assertBuildMetadata(installed) {
    if (!installed || !installed.buildMetadata) return;
    const result = buildMetadataTrust.verifyAgainstSnapshot({
      installRoot: installed.installPath,
      snapshot: installed,
      launcherVersion: this.launcherVersion,
    });
    if (result.state === "trusted" || result.state === "legacy") return;
    throw new DeliveryError(result.reason || "BUILD_METADATA_MISMATCH", "Метаданные сборки не подтверждены");
  }

  // Мост к клиенту Steam (ProtonRunner.steamBridgeCommand): игра видна в
  // Steam как запущенная под своим AppID, как на Windows.
  steamBridgeOf(installed, config) {
    if (!config.steamBridge) return null;
    const steam = this.steamLocator.locateSteam();
    this.steamLocator.assertSteamRunning(steam);
    return { appId: config.steamAppId, steamPath: steam.installPath, installPath: installed.installPath };
  }

  // HL1 WON: hl.exe стартует через инжектор Bunnymod XT, а старому steam.dll
  // нужен запущенный клиент Steam, поэтому Proton идёт в режиме моста к Steam.
  goldSrcLaunch(installed, config, settings) {
    const files = goldsrc.resolveFiles(installed.installPath, config);
    goldsrc.enforceSteamAppIdFile(installed.installPath, config, fs, this.logger);
    const proton = protonOptionsOf(settings);
    const argv = goldsrc.mergeArguments(config, settings).concat(proton.args);
    const env = Object.assign(goldsrc.environmentOf(config), proton.env);
    const bxtLog = goldsrc.prepareBxtLog(this.logsDir, fs, this.logger);
    if (bxtLog) env.BXT_LOGFILE = windowsPathOf(bxtLog);
    const options = {
      cwd: path.dirname(files.executable),
      env,
      wrapper: proton.wrapper,
      excludeCpu0: settings && settings.excludeCpu0 === true,
    };
    const bridge = this.steamBridgeOf(installed, config);
    if (bridge) options.steamBridge = bridge;
    return {
      files,
      executable: files.injector,
      argv: [windowsPathOf(files.executable), ...argv],
      options,
    };
  }

  async launch(gameId, options = {}) {
    // Без этого отказа выбранный эпизод молча запускал бы саму Ghosting.
    if (options && options.episode != null)
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", "Эпизоды Ghosting на Linux пока не запускаются", { gameId });
    const config = this.configFor(gameId);
    const processService = this.delivery.processService;
    if (this.launching.has(gameId))
      throw new DeliveryError("ALREADY_RUNNING", "Запуск игры уже выполняется");
    if (processService.isRunning(gameId))
      return { status: "already-running", process: processService.getProcessState(gameId) };
    if (this.delivery.isBusy())
      throw new DeliveryError("ALREADY_RUNNING", "Дождитесь завершения текущей операции");

    this.launching.add(gameId);
    processService.markStarting(gameId);
    try {
      // Все сборки идут с -steam: без клиента Steam игра падает на старте,
      // поэтому отказываем заранее, и лаунчер предлагает открыть Steam.
      this.steamLocator.assertSteamRunning(this.steamLocator.locateSteam());
      const installed = this.delivery.installedOf(gameId);
      const conflict = findCaseConflict(installed.installPath);
      if (conflict)
        throw new DeliveryError("LIBRARY_PATH_CASE_CONFLICT",
          `Рядом с ${conflict.folder} лежит ${conflict.twin}: Wine путает папки, имена которых различаются только регистром`, conflict);
      this.assertBuildMetadata(installed);
      const settings = this.settingsStore ? this.settingsStore.get(gameId) : null;
      await this.prepareTimerFonts(gameId, settings);
      const launched = goldsrc.isGoldSrc(config)
        ? await this.startGoldSrc(gameId, installed, config, settings)
        : await this.startSource(gameId, installed, config, settings);
      const child = launched.child;
      let earlyError = null;
      let earlyExit = null;
      child.once("error", (error) => { earlyError = error; });
      child.once("exit", (code) => { earlyExit = code; });
      await delay(1500);
      if (earlyError || earlyExit !== null || !child.pid)
        throw new DeliveryError("LAUNCH_FAILED", earlyError ? earlyError.message :
          `Proton завершился сразу после запуска (код ${earlyExit})`);

      const state = this.delivery.registerStartedProcess(gameId, {
        pid: child.pid,
        executablePath: launched.executablePath,
        hostProcess: true,
        startedAt: new Date().toISOString(),
        child,
      });
      if (typeof child.unref === "function") child.unref();
      this.watchGameProcess(gameId, config, child);
      this.logger.info(`[launch] ${gameId}: Proton ${launched.proton.name}, префикс ${launched.prefix}`);
      const result = { status: "started", process: state };
      if (launched.dependencies) result.dependencies = launched.dependencies;
      if (launched.packs) result.packs = launched.packs;
      return result;
    } catch (error) {
      const deliveryError = toDeliveryError(error, "LAUNCH_FAILED");
      processService.cancelStarting(gameId, `${deliveryError.code}: ${deliveryError.message}`);
      throw deliveryError;
    } finally {
      this.launching.delete(gameId);
    }
  }

  // Сессия umu игры живёт, пока в префиксе есть программы: explorer.exe,
  // services и winedevice поднимаются в контейнере первой программы
  // префикса и ждут последнюю. Открытый LiveSplit в том же префиксе держал
  // их, и лаунчер считал игру запущенной. Поэтому за игрой следим по её
  // процессу (hl2.exe, hl.exe): он появился, а потом пропал на три опроса
  // подряд — игра закрыта, даже если сессия ещё жива. LiveSplit и сессию
  // не трогаем. Если процесс так и не появился, остаётся выход сессии.
  // Три опроса, а не один: GoldSrc перезапускает себя (смена видеорежима),
  // и новый hl.exe появляется не сразу.
  watchGameProcess(gameId, config, gameChild) {
    const processService = this.delivery.processService;
    const processName = LiveSplitOverlay.processNameOf(config);
    let seen = false;
    let missing = 0;
    const timer = setInterval(() => {
      // Игра уже закрыта; следующий запуск отсекает pid в markExited.
      if (!processService.isRunning(gameId)) {
        clearInterval(timer);
        return;
      }
      if (this.gameProcessRunning(processName, gameId)) {
        seen = true;
        missing = 0;
        return;
      }
      if (!seen || ++missing < 3) return;
      clearInterval(timer);
      this.logger.info(`[launch] ${gameId}: ${processName} закрылся, сессия Proton ещё жива (в префиксе остались программы, например LiveSplit)`);
      processService.markExited(gameId, gameChild.pid);
    }, this.gameWatchMs);
    if (typeof timer.unref === "function") timer.unref();
    gameChild.once("exit", () => clearInterval(timer));
    return timer;
  }

  // Таймер со «startTimer» стартует, когда игра уже запущена, а первый запуск
  // LiveSplit ставит свои шрифты через regedit в тот же префикс. Параллельно
  // со стартом игры это роняло её (код 3), поэтому шрифты ставятся заранее.
  // Сбой шрифтов игру не останавливает: LiveSplit поставит их сам.
  async prepareTimerFonts(gameId, settings) {
    if (!settings || settings.startTimer !== true) return;
    if (typeof this.runner.ensureLiveSplitFonts !== "function") return;
    try {
      await this.runner.ensureLiveSplitFonts(gameId);
    } catch (error) {
      this.logger.warn(`[launch] шрифты LiveSplit не подготовлены до запуска: ${error.message}`);
    }
  }

  // Вывод umu и Proton последнего запуска игры: попадает в ZIP диагностики.
  gameLogFile(gameId) {
    return this.logsDir ? path.join(this.logsDir, `proton-${gameId}.log`) : null;
  }

  async startGoldSrc(gameId, installed, config, settings) {
    const dependencies = this.dependencyUpdater
      ? await this.dependencyUpdater(installed.installPath, config) : [];
    this.prepareConfigs(gameId, installed, config, settings);
    const packs = this.packApplier ? await this.packApplier(installed.installPath, config, settings) : null;
    const plan = this.goldSrcLaunch(installed, config, settings);
    plan.options.logFile = this.gameLogFile(gameId);
    this.applyOverlay(plan.options, this.prepareOverlay(gameId, installed, config, settings));
    const launched = await this.runner.launch(gameId, plan.executable, plan.argv, plan.options);
    // RInput стартует в фоне, когда BXT уже внедрён: запуск игры его не ждёт.
    this.rinputStart = this.startRInput(gameId, installed, config, settings, plan, launched.child)
      .catch((error) => this.logger.warn(`[launch] RInput не запущен: ${error.message}`));
    return Object.assign({ executablePath: plan.files.executable, dependencies, packs }, launched);
  }

  // Ждём, пока инжектор BXT загрузит свою DLL в hl.exe. Инжектор держит
  // игру приостановленной, пока внедряется; RInput, влезший в этот момент,
  // ломает ему внедрение (access denied). null — игра уже закрылась.
  async waitForBxt(gameChild) {
    let gone = gameChild.exitCode != null || gameChild.signalCode != null;
    gameChild.once("exit", () => { gone = true; });
    const deadline = Date.now() + this.bxtWaitMs;
    while (!gone) {
      if (this.bxtInjected()) return true;
      if (Date.now() >= deadline) return false;
      await delay(this.bxtPollMs);
    }
    return null;
  }

  // RInput идёт отдельной сессией Proton в тот же префикс и сам ждёт hl.exe.
  // Если игра так и не появилась, он ждал бы вечно, поэтому вместе с игрой
  // гасим и его группу процессов. Сбой RInput игру не останавливает.
  async startRInput(gameId, installed, config, settings, plan, gameChild) {
    const rinput = goldsrc.resolveRInput(installed.installPath, config, settings, fs);
    if (!rinput) return;
    if (rinput.missing) {
      this.logger.warn(`[launch] RInput не найден: ${rinput.executable}`);
      return;
    }
    const ready = await this.waitForBxt(gameChild);
    if (ready === null) return;
    if (!ready) this.logger.warn(`[launch] Bunnymod XT не появился в ${rinput.processName} за ${Math.round(this.bxtWaitMs / 1000)} с, RInput запускается без него`);
    let child;
    try {
      ({ child } = await this.runner.launch(gameId, rinput.executable, [rinput.processName], {
        cwd: path.dirname(rinput.executable),
        skipFonts: true,
        steamBridge: plan.options.steamBridge,
        // RInput — консольная программа: мост steam.exe открыл бы ей окно
        // консоли, и оно отбирало бы фокус у полноэкранной игры.
        env: { PROTON_HIDE_PROCESS_WINDOW: "1" },
      }));
    } catch (error) {
      this.logger.warn(`[launch] RInput не запущен: ${error.message}`);
      return;
    }
    let exited = false;
    child.once("exit", (code) => {
      exited = true;
      this.logger.info(`[launch] RInput завершился (код ${code})`);
    });
    gameChild.once("exit", () => {
      if (exited || !child.pid) return;
      try { this.killProcessGroup(child.pid); } catch (error) { /* уже завершился */ }
    });
    if (typeof child.unref === "function") child.unref();
    this.logger.info(`[launch] RInput ждёт ${rinput.processName}`);
    if (launchArgs.sanitizeSettings(settings).settings.rinputMenuFix)
      await this.patchRInputMenu(rinput.processName, gameChild);
  }

  // Оверлей LiveSplit (livesplit-overlay.js): прокси d3d9.dll или
  // opengl32.dll кладётся в папку игры до запуска, игра грузит его сама, а
  // WINEDLLOVERRIDES (applyOverlay) велит Wine взять его из папки игры, а не
  // свою DLL. Ничего не внедряется. Окно LiveSplit прокси ищет в своём
  // префиксе, поэтому LiveSplit должен быть запущен для этой же игры. Сбой
  // оверлея игру не останавливает.
  prepareOverlay(gameId, installed, config, settings) {
    if (!this.liveSplitOverlay) return null;
    try {
      const prepared = this.liveSplitOverlay.prepare(gameId, settings, { installPath: installed.installPath, config });
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

  // Замена DLL для Wine в окружение игры; пользовательские замены остаются.
  applyOverlay(options, prepared) {
    if (!prepared || !prepared.dllOverride) return;
    const env = Object.assign({}, options.env);
    env.WINEDLLOVERRIDES = LiveSplitOverlay.mergeOverrides(env.WINEDLLOVERRIDES, prepared.dllOverride);
    options.env = env;
  }

  // Статус игры для Discord (PresenceRelay.js): канал и токен для игры.
  // null — мост этой игре не нужен или на хосте не поднят.
  presenceTarget(config) {
    if (!this.presenceRelay || !config.presenceBridge || typeof this.extraEnvProvider !== "function") return null;
    try { return PresenceRelay.targetOf(this.extraEnvProvider()); } catch (error) { return null; }
  }

  // Помощник стартует только после игры. Первая программа в префиксе
  // поднимает его общие процессы (explorer.exe с рабочим столом, services,
  // winedevice) в своём контейнере, и с помощником впереди игры звук при
  // сворачивании повторялся. client.dll сам повторяет подключение к каналу
  // каждые 5 секунд, так что статус в Discord появится чуть позже, а игру
  // мост не задерживает. Сбой моста игру не останавливает: Discord тогда
  // покажет просто «В игре».
  async startPresenceRelay(gameId, config, target, gameChild) {
    let gameGone = false;
    gameChild.once("exit", () => { gameGone = true; });
    const processName = LiveSplitOverlay.processNameOf(config);
    const deadline = Date.now() + this.presenceStartWaitMs;
    while (!gameGone && !this.processRunning(processName)) {
      if (Date.now() >= deadline) {
        this.logger.warn(`[launch] мост статуса для Discord не запущен: ${processName} не появился за ${Math.round(this.presenceStartWaitMs / 1000)} с`);
        return false;
      }
      await delay(this.bxtPollMs);
    }
    if (!gameGone && this.presenceSettleMs > 0) await delay(this.presenceSettleMs);
    if (gameGone) return false;
    let prepared;
    try {
      prepared = this.presenceRelay.prepare();
    } catch (error) {
      this.logger.warn(`[launch] мост статуса для Discord не подготовлен: ${error.message}`);
      return false;
    }
    const args = PresenceRelay.relayArguments(target, processName, windowsPathOf(prepared.status));
    let child;
    try {
      ({ child } = await this.runner.launch(gameId, prepared.executable, args, {
        cwd: prepared.dir,
        skipFonts: true,
        env: { PROTON_HIDE_PROCESS_WINDOW: "1" },
      }));
    } catch (error) {
      this.logger.warn(`[launch] мост статуса для Discord не запущен: ${error.message}`);
      return false;
    }
    if (typeof child.unref === "function") child.unref();
    let sessionDone = false;
    child.once("exit", () => { sessionDone = true; });
    const stop = () => {
      if (sessionDone || !child.pid) return;
      try { this.killProcessGroup(child.pid); } catch (error) { /* уже завершился */ }
    };
    if (gameGone) stop();
    else gameChild.once("exit", stop);
    const code = await this.presenceRelay.waitForStatus(prepared.status, this.presenceWaitMs, {
      cancelled: () => sessionDone,
    });
    if (code === 0) {
      this.logger.info("[launch] мост статуса для Discord готов");
      return true;
    }
    this.logger.warn(`[launch] мост статуса для Discord не готов (${code === null ? "нет ответа" : `код ${code}`}), игра идёт без него`);
    stop();
    return false;
  }

  // Курсор в меню: когда RInput поставит свои перехваты в hl.exe, патчим их
  // в памяти (см. rinput-menu-fix.js). Сбой патча игру не трогает.
  async patchRInputMenu(processName, gameChild) {
    let gone = gameChild.exitCode != null || gameChild.signalCode != null;
    gameChild.once("exit", () => { gone = true; });
    const deadline = Date.now() + this.rinputPatchWaitMs;
    while (!gone) {
      let status;
      try {
        status = await this.rinputMenuPatcher(processName);
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
      await delay(this.bxtPollMs);
    }
  }

  // Файл игрока на запуске не переписывается: только переход с 0.4.8 или создание отсутствующего.
  prepareConfigs(gameId, installed, config, settings) {
    try {
      defaultConfigs.prepareForLaunch(gameId, installed.installPath, config, settings, {
        launcherVersion: this.launcherVersion,
      });
    } catch (error) {
      this.logger.warn(`[launch] конфиг игрока не подготовлен: ${error.message}`);
    }
  }

  async startSource(gameId, installed, config, settings) {
    const files = this.filesFor(installed.installPath, config);
    const proton = protonOptionsOf(settings);
    const argv = launchArgs.mergeLaunchArguments(config.arguments, settings).concat(proton.args);
    try {
      gameLanguage.apply(installed.installPath, config.gameDirectory, launchArgs.languageOf(settings || {}));
    } catch (error) {
      this.logger.warn(`[launch] язык не применён: ${error.message}`);
    }
    this.prepareConfigs(gameId, installed, config, settings);

    const presence = this.presenceTarget(config);
    const options = {
      cwd: installed.installPath,
      env: Object.assign({}, proton.env, presence ? presence.gameEnv : {}),
      wrapper: proton.wrapper,
      excludeCpu0: settings && settings.excludeCpu0 === true,
      logFile: this.gameLogFile(gameId),
    };
    const bridge = this.steamBridgeOf(installed, config);
    if (bridge) options.steamBridge = bridge;
    this.applyOverlay(options, this.prepareOverlay(gameId, installed, config, settings));
    const launched = await this.runner.launch(gameId, files.executable, argv, options);
    if (presence) {
      this.presenceReady = this.startPresenceRelay(gameId, config, presence, launched.child)
        .catch((error) => {
          this.logger.warn(`[launch] мост статуса для Discord не запущен: ${error.message}`);
          return false;
        });
    }
    return Object.assign({ executablePath: files.executable }, launched);
  }

  describeAvailability(gameId) {
    try {
      this.configFor(gameId);
      if (!this.runner || !this.runner.manager.selected())
        return { ok: false, code: "PROTON_NOT_INSTALLED" };
      if (!this.runner.umuPath()) return { ok: false, code: "UMU_NOT_FOUND" };
      return { ok: true, code: null };
    } catch (error) {
      return { ok: false, code: "LAUNCH_CONFIG_INVALID" };
    }
  }
}

module.exports = LaunchService;
