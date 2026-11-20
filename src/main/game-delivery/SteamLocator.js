"use strict";

const nodeFs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const vdf = require("./vdf");
const defaultInspector = require("./process-inspect");
const { DeliveryError } = require("./errors");

const STEAM_KEY = "HKCU\\Software\\Valve\\Steam";
const ACTIVE_PROCESS_KEY = "HKCU\\Software\\Valve\\Steam\\ActiveProcess";

const SOURCE_SDK_APP_ID = 243730;

const SDK_REQUIRED_DIRS = ["hl2", "bin", "platform"];
const SDK_REQUIRED_FILES = ["hl2.exe"];

function readRegistryValue(key, name) {
  let output;

  try {
    output = execFileSync("reg.exe", ["query", key, "/v", name], {
      encoding: "utf8",
      windowsHide: true,
      timeout: 5000,
    });
  } catch (error) {
    return null;
  }

  for (const line of String(output).split(/\r?\n/)) {
    const match = line.match(/^\s*(\S+)\s+(REG_[A-Z_]+)\s+(.*)$/);
    if (!match) continue;
    if (match[1].toLowerCase() !== String(name).toLowerCase()) continue;

    const type = match[2];
    const raw = match[3].trim();

    if (type === "REG_DWORD") {
      const value = Number.parseInt(raw, 16);
      return Number.isFinite(value) ? value : null;
    }

    return raw;
  }

  return null;
}

const defaultRegistry = { read: readRegistryValue };

class SteamLocator {
  constructor(options) {
    const opts = options || {};

    this.fs = opts.fs || nodeFs;
    this.registry = opts.registry || defaultRegistry;
    this.inspector = opts.inspector || defaultInspector;
    this.platform = opts.platform || process.platform;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };

    this.runningCache = null;

    this.missCache = null;
    this.missTtlMs = opts.missTtlMs || 8000;
    this.now = opts.now || (() => Date.now());

    this.steamCache = null;
  }

  locateSteam() {
    if (this.platform !== "win32")
      throw new DeliveryError("STEAM_NOT_INSTALLED", "Запуск через Steam поддерживается только в Windows");

    if (this.steamCache) return this.steamCache;

    const steamPath = this.registry.read(STEAM_KEY, "SteamPath");
    const steamExeValue = this.registry.read(STEAM_KEY, "SteamExe");

    if (!steamPath && !steamExeValue)
      throw new DeliveryError("STEAM_NOT_INSTALLED", "Steam не найден в реестре текущего пользователя");

    const root = steamPath
      ? path.normalize(String(steamPath))
      : path.dirname(path.normalize(String(steamExeValue)));

    const steamExe = steamExeValue
      ? path.normalize(String(steamExeValue))
      : path.join(root, "steam.exe");

    if (!this.fs.existsSync(steamExe))
      throw new DeliveryError("STEAM_NOT_INSTALLED", `Файл Steam не найден: ${steamExe}`);

    this.steamCache = { installPath: root, steamExe };
    return this.steamCache;
  }

  assertSteamRunning(steam) {
    const pid = this.registry.read(ACTIVE_PROCESS_KEY, "pid");
    let registryProblem = null;

    if (
      this.missCache &&
      this.missCache.pid === pid &&
      this.missCache.steamExe === steam.steamExe &&
      this.now() - this.missCache.at < this.missTtlMs
    )
      throw new DeliveryError(this.missCache.code, this.missCache.message);

    if (!Number.isInteger(pid) || pid <= 0) registryProblem = "в реестре нет активного процесса";
    else if (!this.inspector.isAlive(pid)) registryProblem = `процесс ${pid} из реестра мёртв`;
    else if (
      this.runningCache &&
      this.runningCache.pid === pid &&
      this.runningCache.steamExe === steam.steamExe
    ) {
      return this.runningCache.result;
    } else {
      const actual = this.inspector.inspect(pid);

      if (this.sameImageName(actual, steam)) {
        // Путь узнать нечем (WMI и PowerShell не отвечают, ответил только
        // tasklist), но живой процесс из реестра Steam называется steam.exe.
        this.logger.warn(
          `[steam] путь процесса ${pid} недоступен (${this.inspectorFailure() || "нет данных"}); принимаю Steam по имени образа`
        );
        const result = { pid, executablePath: steam.steamExe, startedAt: null, source: "registry-name" };
        this.runningCache = { pid, steamExe: steam.steamExe, result };
        return result;
      }

      if (!actual || !actual.executablePath) {
        const failure = this.inspectorFailure();
        registryProblem = `процесс ${pid} из реестра не удалось опознать${failure ? `: ${failure}` : ""}`;
      } else if (!this.inspector.samePath(actual.executablePath, steam.steamExe))
        registryProblem = `процесс ${pid} из реестра — не Steam: ${actual.executablePath}`;
      else {
        const result = {
          pid,
          executablePath: actual.executablePath,
          startedAt: actual.startedAt || null,
          source: "registry",
        };

        this.runningCache = { pid, steamExe: steam.steamExe, result };

        return result;
      }
    }

    const scan = this.findSteamProcess(steam);
    const found = scan.match;

    if (found) {
      this.logger.warn(
        `[steam] ${registryProblem}; нашёл работающий Steam по пути: PID ${found.pid}`
      );

      return {
        pid: found.pid,
        executablePath: found.executablePath,
        startedAt: found.startedAt || null,
        source: "scan",
      };
    }

    const code = scan.blind ? "STEAM_ELEVATED" : "STEAM_NOT_RUNNING";
    const message = scan.blind
      ? `Steam запущен с другими правами: ${registryProblem}, ` +
        `процессов steam.exe без доступа к пути: ${scan.blind}`
      : `Steam не запущен (${registryProblem})`;

    this.missCache = { pid, steamExe: steam.steamExe, at: this.now(), code, message };

    throw new DeliveryError(code, message);
  }

  sameImageName(actual, steam) {
    if (!actual || actual.executablePath || actual.source !== "tasklist" || !actual.name) return false;
    return String(actual.name).toLowerCase() === path.win32.basename(steam.steamExe).toLowerCase();
  }

  inspectorFailure() {
    return typeof this.inspector.lastFailure === "function" ? this.inspector.lastFailure() : null;
  }

  findSteamProcess(steam) {
    if (typeof this.inspector.findByImageName !== "function") return { match: null, blind: 0 };

    let candidates = [];
    try {
      candidates = this.inspector.findByImageName(path.basename(steam.steamExe));
    } catch (error) {
      this.logger.warn(`[steam] поиск процесса Steam не удался: ${error.message}`);
      return { match: null, blind: 0 };
    }

    const match =
      candidates.find(
        (item) =>
          item &&
          item.executablePath &&
          this.inspector.samePath(item.executablePath, steam.steamExe)
      ) || null;

    // Без пути от tasklist — это не «другие права», а просто нечем узнать путь.
    const blind = candidates.filter((item) => item && !item.executablePath && item.source !== "tasklist").length;

    return { match, blind };
  }

  steamLibraries(steam) {
    const libraries = [];
    const seen = new Set();

    const add = (root) => {
      if (!root) return;
      const resolved = path.resolve(String(root));
      const key = resolved.toLowerCase();
      if (seen.has(key)) return;
      seen.add(key);
      libraries.push(resolved);
    };

    add(steam.installPath);

    const file = path.join(steam.installPath, "steamapps", "libraryfolders.vdf");

    if (!this.fs.existsSync(file)) return libraries;

    let data;
    try {
      data = vdf.parse(this.fs.readFileSync(file, "utf8"));
    } catch (error) {
      this.logger.warn(`[steam-locator] libraryfolders.vdf не разобран: ${error.message}`);
      return libraries;
    }

    const folders = vdf.getInsensitive(data, "libraryfolders");
    if (!folders || typeof folders !== "object") return libraries;

    for (const key of Object.keys(folders)) {
      if (!/^\d+$/.test(key)) continue;

      const entry = folders[key];

      if (typeof entry === "string") add(entry);
      else if (entry && typeof entry === "object") add(vdf.getInsensitive(entry, "path"));
    }

    return libraries;
  }

  findApp(steam, appId) {
    const libraries = this.steamLibraries(steam);
    let corrupted = null;

    for (const library of libraries) {
      const manifestPath = path.join(library, "steamapps", `appmanifest_${appId}.acf`);

      if (!this.fs.existsSync(manifestPath)) continue;

      let data;
      try {
        data = vdf.parse(this.fs.readFileSync(manifestPath, "utf8"));
      } catch (error) {
        corrupted = { manifestPath, error };
        continue;
      }

      const appState = vdf.getInsensitive(data, "AppState");

      if (!appState) {
        corrupted = { manifestPath, error: new Error("нет секции AppState") };
        continue;
      }

      const declaredId = Number(vdf.getInsensitive(appState, "appid"));

      if (declaredId !== Number(appId))
        throw new DeliveryError(
          "STEAM_APP_ID_MISMATCH",
          `appmanifest_${appId}.acf объявляет AppID ${declaredId}`,
          { manifestPath, expected: Number(appId), actual: declaredId }
        );

      const installdir = vdf.getInsensitive(appState, "installdir");

      if (!installdir || /[\\/]|\.\./.test(String(installdir)))
        throw new DeliveryError(
          "SOURCE_SDK_INCOMPLETE",
          `Недопустимый installdir в appmanifest: ${installdir}`,
          { manifestPath }
        );

      const appPath = path.join(library, "steamapps", "common", String(installdir));

      if (!this.fs.existsSync(appPath))
        throw new DeliveryError(
          "SOURCE_SDK_INCOMPLETE",
          `Каталог приложения из appmanifest не найден: ${appPath}`,
          { manifestPath, appPath }
        );

      return {
        appId: Number(appId),
        library,
        manifestPath,
        installdir: String(installdir),
        appPath,
        stateFlags: Number(vdf.getInsensitive(appState, "StateFlags")) || null,
        name: vdf.getInsensitive(appState, "name") || null,
      };
    }

    if (corrupted)
      throw new DeliveryError(
        "SOURCE_SDK_INCOMPLETE",
        `appmanifest повреждён: ${corrupted.error.message}`,
        { manifestPath: corrupted.manifestPath }
      );

    throw new DeliveryError(
      "SOURCE_SDK_NOT_INSTALLED",
      `Приложение Steam ${appId} не установлено ни в одной библиотеке`,
      { appId: Number(appId), libraries }
    );
  }

  assertSourceSdkComplete(app) {
    const missing = [];

    for (const name of SDK_REQUIRED_FILES) {
      const file = path.join(app.appPath, name);
      if (!this.fs.existsSync(file)) missing.push(name);
    }

    for (const name of SDK_REQUIRED_DIRS) {
      const dir = path.join(app.appPath, name);

      let stats = null;
      try {
        stats = this.fs.statSync(dir);
      } catch (error) {
        stats = null;
      }

      if (!stats || !stats.isDirectory()) missing.push(`${name}${path.sep}`);
    }

    if (missing.length)
      throw new DeliveryError(
        "SOURCE_SDK_INCOMPLETE",
        `Установка Source SDK Base неполная: нет ${missing.join(", ")}`,
        { appPath: app.appPath, missing }
      );

    return true;
  }

  preflight(appId) {
    const id = appId == null ? SOURCE_SDK_APP_ID : Number(appId);

    const steam = this.locateSteam();
    const running = this.assertSteamRunning(steam);
    const app = this.findApp(steam, id);
    this.assertSourceSdkComplete(app);

    return {
      steam: {
        installPath: steam.installPath,
        steamExe: steam.steamExe,
        pid: running.pid,
        source: running.source || null,
      },
      app: {
        appId: app.appId,
        library: app.library,
        manifestPath: app.manifestPath,
        installdir: app.installdir,
        appPath: app.appPath,
      },
    };
  }

  describeSteamOnly() {
    try {
      const steam = this.locateSteam();
      const running = this.assertSteamRunning(steam);

      return {
        ok: true,
        code: null,
        steam: {
          installPath: steam.installPath,
          steamExe: steam.steamExe,
          pid: running.pid,
          source: running.source || null,
        },
        app: null,
      };
    } catch (error) {
      return {
        ok: false,
        code: (error && error.code) || "LAUNCH_FAILED",
        message: error && error.message,
      };
    }
  }

  describeAvailability(appId) {
    try {
      const result = this.preflight(appId);
      return { ok: true, code: null, steam: result.steam, app: result.app };
    } catch (error) {
      const code = error && error.code ? error.code : "LAUNCH_FAILED";
      return { ok: false, code, message: error.message };
    }
  }
}

module.exports = SteamLocator;
module.exports.SOURCE_SDK_APP_ID = SOURCE_SDK_APP_ID;
module.exports.STEAM_KEY = STEAM_KEY;
module.exports.ACTIVE_PROCESS_KEY = ACTIVE_PROCESS_KEY;
module.exports.readRegistryValue = readRegistryValue;
