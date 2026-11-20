"use strict";

const fs = require("fs");
const path = require("path");
const { execFile } = require("child_process");

const { DeliveryError } = require("./errors");
const { assertInside } = require("./paths");

function defaultRun(file, args, options) {
  return new Promise((resolve) => {
    execFile(file, args, options || {}, (error, stdout, stderr) => {
      let status = 0;
      if (error) status = typeof error.code === "number" ? error.code : 1;

      resolve({ error: error || null, status, stdout: stdout || "", stderr: stderr || "" });
    });
  });
}

const DEFAULT_TIMEOUT_MS = 10 * 60 * 1000;
const COMMAND_TIMEOUT_MS = 15000;
const POLL_INTERVAL_MS = 400;
const MAX_CONTENT_SLOT = 256;

const NO_MATCH_FILTER = "?!";

const RESPONSES = {
  downloadOk: "Depot download complete",
  downloadFailed: "Depot download failed",
  noLicense: "No active license found for appID",
  purchased: "]  - Purchased : ",
};

const CATALOG = {
  "hl2-5288": {
    requiresAppIds: [220],
    depots: [
      {
        appId: 220,
        depotId: 222,
        manifestId: "3151477805868332059",
        fileFilter: "bin*;hl2*",
        expectedFiles: 103,
      },
      {
        appId: 220,
        depotId: 221,
        manifestId: "2694190745568322895",
        fileFilter: "platform*;hl2*;bin*",
        expectedFiles: 1058,
      },
      {
        appId: 243750,
        depotId: 243752,
        manifestId: "8806556782944295588",
        fileFilter: "*FileSystem_Stdio.dll",
        expectedFiles: 1,
      },
    ],
  },
};

const MANIFEST_ID_RE = /^[0-9]{1,20}$/;
const FILE_FILTER_RE = /^[A-Za-z0-9_.*;-]{1,120}$/;

function assertDepot(depot) {
  if (!Number.isInteger(depot.appId) || depot.appId <= 0)
    throw new DeliveryError("MANIFEST_INVALID", `Некорректный appId депота: ${depot.appId}`);

  if (!Number.isInteger(depot.depotId) || depot.depotId <= 0)
    throw new DeliveryError("MANIFEST_INVALID", `Некорректный depotId: ${depot.depotId}`);

  if (!MANIFEST_ID_RE.test(String(depot.manifestId)))
    throw new DeliveryError("MANIFEST_INVALID", `Некорректный manifestId: ${depot.manifestId}`);

  if (depot.fileFilter != null && !FILE_FILTER_RE.test(String(depot.fileFilter)))
    throw new DeliveryError("MANIFEST_INVALID", `Некорректный фильтр файлов: ${depot.fileFilter}`);

  return depot;
}

function catalogFor(buildId) {
  const entry = CATALOG[String(buildId || "")];

  if (!entry)
    throw new DeliveryError("MANIFEST_INVALID", `Сборка ${buildId} не объявлена в каталоге депотов`, {
      buildId,
    });

  entry.depots.forEach(assertDepot);
  return entry;
}

class ConsoleTail {
  constructor(file, options) {
    const opts = options || {};

    this.file = file;
    this.fsImpl = opts.fs || fs;
    this.position = 0;
    this.buffer = "";

    try {
      this.position = this.fsImpl.statSync(file).size;
    } catch (error) {
      this.position = 0;
    }
  }

  read() {
    let stat;
    try {
      stat = this.fsImpl.statSync(this.file);
    } catch (error) {
      return [];
    }

    if (stat.size < this.position) this.position = 0;
    if (stat.size === this.position) return [];

    let handle;
    let chunk = "";

    try {
      handle = this.fsImpl.openSync(this.file, "r");
      const length = stat.size - this.position;
      const buf = Buffer.alloc(length);
      const read = this.fsImpl.readSync(handle, buf, 0, length, this.position);
      chunk = buf.slice(0, read).toString("utf8");
      this.position += read;
    } catch (error) {
      return [];
    } finally {
      if (handle != null) {
        try {
          this.fsImpl.closeSync(handle);
        } catch (e) {
          /* закрыть не удалось — читать всё равно больше нечего */
        }
      }
    }

    this.buffer += chunk;

    const lines = this.buffer.split(/\r?\n/);
    this.buffer = lines.pop() || "";

    return lines;
  }
}

class SteamDepotService {
  constructor(options) {
    const opts = options || {};

    this.locator = opts.locator || null;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.run = opts.run || defaultRun;
    this.commandTimeoutMs = opts.commandTimeoutMs || COMMAND_TIMEOUT_MS;
    this.fsImpl = opts.fs || fs;
    this.sleep = opts.sleep || ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
    this.now = opts.now || (() => Date.now());
    this.timeoutMs = opts.timeoutMs || DEFAULT_TIMEOUT_MS;
    this.pollMs = opts.pollMs || POLL_INTERVAL_MS;
    this.makeTail = opts.makeTail || ((file) => new ConsoleTail(file, { fs: this.fsImpl }));

    this.ownedCache = new Set();

    this.ownedSessionPid = null;
  }

  requireSteamSession() {
    const running = this.locator.assertSteamRunning(this.locator.locateSteam());
    const pid = running && running.pid ? running.pid : null;

    if (this.ownedSessionPid !== pid) {
      if (this.ownedCache.size)
        this.logger.info(`[steam-depot] сеанс Steam сменился — лицензии проверяются заново`);

      this.ownedCache.clear();
      this.ownedSessionPid = pid;
    }

    return running;
  }

  steamPaths() {
    if (!this.locator)
      throw new DeliveryError("STEAM_NOT_FOUND", "Локатор Steam не задан");

    const steam = this.locator.locateSteam();
    const root = steam && steam.installPath;
    const exe = steam && steam.steamExe;

    if (!root || !exe)
      throw new DeliveryError(
        "STEAM_NOT_FOUND",
        `Локатор Steam вернул неполные данные: installPath=${root}, steamExe=${exe}`
      );

    return {
      root,
      exe,
      // Flatpak-Steam в Linux запускается как `flatpak run com.valvesoftware.Steam`.
      baseArgs: steam.steamArgs || [],
      contentDir: path.join(root, "steamapps", "content"),
      consoleLog: path.join(root, "logs", "console_log.txt"),
    };
  }

  async sendCommand(paths, parts) {
    const tokens = parts.map((part) => String(part));

    const argv = (paths.baseArgs || []).concat([`steam://+${tokens[0]}`], tokens.slice(1));

    this.logger.info(`[steam-depot] команда: ${tokens.join(" ")}`);

    const result = await this.run(paths.exe, argv, {
      windowsHide: true,
      shell: false,
      encoding: "utf8",
      timeout: this.commandTimeoutMs,
    });

    if (result && result.error) {
      const error = result.error;
      const timedOut = !!error.killed || error.signal === "SIGTERM";
      const notStarted = typeof error.code === "string";

      if (timedOut)
        throw new DeliveryError(
          "STEAM_TIMEOUT",
          `Steam не принял команду за ${Math.round(this.commandTimeoutMs / 1000)} с — вероятно, он ещё запускается`
        );

      if (notStarted)
        throw new DeliveryError("STEAM_NOT_RUNNING", `Не удалось обратиться к Steam: ${error.message}`);

      this.logger.info(
        `[steam-depot] Steam вышел с кодом ${error.code} на команде ${tokens[0]} — ответ ждём в логе`
      );
    }

    return result;
  }

  async waitFor(tail, needles, options) {
    const opts = options || {};
    const deadline = this.now() + (opts.timeoutMs || this.timeoutMs);

    for (;;) {
      for (const line of tail.read()) {
        for (const needle of needles) if (line.indexOf(needle) !== -1) return { needle, line };
      }

      if (typeof opts.onPoll === "function") opts.onPoll();

      if (this.now() >= deadline)
        throw new DeliveryError(
          "STEAM_TIMEOUT",
          `Steam не ответил за ${Math.round((opts.timeoutMs || this.timeoutMs) / 1000)} с`
        );

      await this.sleep(this.pollMs);
    }
  }

  async assertOwns(appIds) {
    const wanted = appIds || [];
    if (!wanted.length) return true;

    this.requireSteamSession();

    const unchecked = wanted.filter((appId) => !this.ownedCache.has(appId));
    if (!unchecked.length) return true;

    for (const appId of unchecked) {
      const verdict = await this.ownershipOf(appId, { timeoutMs: 60000 });

      if (verdict.owned === true) continue;

      throw new DeliveryError(
        verdict.code || "STEAM_APP_NOT_OWNED",
        verdict.message || `Игры нет на аккаунте Steam (AppID ${appId})`,
        { appId }
      );
    }

    return true;
  }

  async ownershipOf(appId, options) {
    const opts = options || {};

    if (!Number.isInteger(appId) || appId <= 0)
      return { appId, owned: null, code: "MANIFEST_INVALID", message: `Некорректный appId: ${appId}` };

    let paths;
    try {
      paths = this.steamPaths();
      this.requireSteamSession();
    } catch (error) {
      return {
        appId,
        owned: null,
        code: (error && error.code) || "STEAM_NOT_FOUND",
        message: (error && error.message) || String(error),
      };
    }

    if (this.ownedCache.has(appId)) return { appId, owned: true, code: null, cached: true };

    let found;
    try {
      const tail = this.makeTail(paths.consoleLog);
      await this.sendCommand(paths, ["licenses_for_app", appId]);
      found = await this.waitFor(tail, [RESPONSES.noLicense, RESPONSES.purchased], {
        timeoutMs: opts.timeoutMs || 60000,
      });
    } catch (error) {
      return {
        appId,
        owned: null,
        code: (error && error.code) || "STEAM_TIMEOUT",
        message: (error && error.message) || String(error),
      };
    }

    if (found.needle === RESPONSES.noLicense)
      return {
        appId,
        owned: false,
        code: "STEAM_APP_NOT_OWNED",
        message: `Игры нет на аккаунте Steam (AppID ${appId})`,
      };

    this.ownedCache.add(appId);
    this.logger.info(`[steam-depot] лицензия на ${appId} подтверждена`);

    return { appId, owned: true, code: null };
  }

  sweepContentSlots(contentDir, libraryRoot) {
    if (!libraryRoot) return 0;

    let names;
    try {
      names = this.fsImpl.readdirSync(contentDir);
    } catch (error) {
      return 0;
    }

    let removed = 0;

    for (const name of names) {
      if (!/^app_\d+$/.test(name)) continue;

      const full = path.join(contentDir, name);

      let stat;
      try {
        stat = this.fsImpl.lstatSync(full);
      } catch (error) {
        continue;
      }

      if (!stat.isSymbolicLink()) continue;

      let target;
      try {
        target = this.fsImpl.readlinkSync(full);
      } catch (error) {
        continue;
      }

      try {
        assertInside(libraryRoot, target, "Цель ссылки в каталоге Steam");
      } catch (error) {
        continue;
      }

      if (this.unlinkContentSlot(full)) {
        removed++;
        this.logger.info(`[steam-depot] снята ссылка ${name} → ${target}`);
      }
    }

    if (removed) this.logger.info(`[steam-depot] убрано брошенных ссылок: ${removed}`);

    return removed;
  }

  freeContentSlot(contentDir) {
    for (let slot = 2; slot <= MAX_CONTENT_SLOT; slot++) {
      const candidate = path.join(contentDir, `app_${slot}`);

      let taken = true;
      try {
        this.fsImpl.lstatSync(candidate);
      } catch (error) {
        taken = false;
      }

      if (!taken) return { slot, path: candidate };
    }

    throw new DeliveryError(
      "STEAM_CONTENT_BUSY",
      `В каталоге загрузок Steam не нашлось свободного слота (проверено ${MAX_CONTENT_SLOT})`
    );
  }

  async linkContentSlot(slotPath, targetDir, libraryRoot) {
    assertInside(libraryRoot, targetDir, "Каталог загрузки депота");

    this.fsImpl.mkdirSync(targetDir, { recursive: true });

    // Без cmd.exe /c mklink: путь библиотеки с «&» не должен разбираться как команда.
    try {
      this.fsImpl.symlinkSync(targetDir, slotPath, process.platform === "win32" ? "junction" : "dir");
    } catch (error) {
      throw new DeliveryError(
        "STEAM_LINK_FAILED",
        `Не удалось связать каталог загрузок Steam с ${targetDir}: ${error.message}`
      );
    }

    return slotPath;
  }

  unlinkContentSlot(slotPath) {
    let stat;
    try {
      stat = this.fsImpl.lstatSync(slotPath);
    } catch (error) {
      return false;
    }

    if (!stat.isSymbolicLink()) {
      this.logger.warn(`[steam-depot] ${slotPath} не является ссылкой — не трогаю`);
      return false;
    }

    try {
      this.fsImpl.rmdirSync(slotPath);
      return true;
    } catch (error) {
      try {
        this.fsImpl.unlinkSync(slotPath);
        return true;
      } catch (inner) {
        this.logger.warn(`[steam-depot] ссылку ${slotPath} снять не удалось: ${inner.message}`);
        return false;
      }
    }
  }

  async download(buildId, targetDir, options) {
    const opts = options || {};
    const entry = catalogFor(buildId);
    const paths = this.steamPaths();
    const libraryRoot = opts.libraryRoot;

    if (!libraryRoot)
      throw new DeliveryError("PATH_INVALID", "Не задан корень библиотеки для загрузки депота");

    await this.assertOwns(entry.requiresAppIds);

    this.sweepContentSlots(paths.contentDir, libraryRoot);

    const slot = this.freeContentSlot(paths.contentDir);
    this.fsImpl.mkdirSync(paths.contentDir, { recursive: true });
    await this.linkContentSlot(slot.path, targetDir, libraryRoot);

    try {
      const timeoutTail = this.makeTail(paths.consoleLog);
      await this.sendCommand(paths, ["DepotDownloadProgressTimeout", 30]);
      await this.waitFor(timeoutTail, ["DepotDownloadProgressTimeout"], { timeoutMs: 30000 });

      for (let index = 0; index < entry.depots.length; index++) {
        const depot = entry.depots[index];

        const primeFilterTail = this.makeTail(paths.consoleLog);
        await this.sendCommand(paths, ["sDepotDownloadFileFilter", NO_MATCH_FILTER]);
        await this.waitFor(primeFilterTail, ["sDepotDownloadFileFilter"], { timeoutMs: 30000 });

        const primeTail = this.makeTail(paths.consoleLog);
        await this.sendCommand(paths, ["download_depot", depot.appId, depot.depotId, depot.manifestId]);
        await this.waitFor(primeTail, [RESPONSES.downloadOk, RESPONSES.downloadFailed], {
          timeoutMs: 120000,
        });

        const filterTail = this.makeTail(paths.consoleLog);
        await this.sendCommand(paths, ["sDepotDownloadFileFilter", depot.fileFilter || NO_MATCH_FILTER]);
        await this.waitFor(filterTail, ["sDepotDownloadFileFilter"], { timeoutMs: 30000 });

        const tail = this.makeTail(paths.consoleLog);

        await this.sendCommand(paths, ["download_depot", slot.slot, depot.depotId, depot.manifestId]);

        const depotDir = path.join(targetDir, `depot_${depot.depotId}`);

        const found = await this.waitFor(tail, [RESPONSES.downloadOk, RESPONSES.downloadFailed], {
          onPoll: () => {
            if (typeof opts.onProgress !== "function") return;

            let files = 0;
            try {
              files = this.countFiles(depotDir);
            } catch (error) {
              files = 0;
            }

            opts.onProgress({
              depotId: depot.depotId,
              index,
              total: entry.depots.length,
              files,
              expectedFiles: depot.expectedFiles || null,
            });
          },
        });

        if (found.needle === RESPONSES.downloadFailed) {
          const foreign = (entry.requiresAppIds || []).indexOf(depot.appId) === -1;

          throw new DeliveryError(
            "STEAM_DEPOT_FAILED",
            `Steam не смог скачать депот ${depot.depotId}` +
              (foreign
                ? `. Он принадлежит приложению ${depot.appId} — проверьте, что оно есть ` +
                  "в библиотеке Steam (Source SDK Base 2013 Multiplayer ставится бесплатно)"
                : ""),
            { depotId: depot.depotId, manifestId: depot.manifestId, appId: depot.appId }
          );
        }

        const files = this.countFiles(depotDir);
        const floor = depot.expectedFiles ? Math.max(1, Math.floor(depot.expectedFiles / 2)) : 1;

        if (files < floor)
          throw new DeliveryError(
            "STEAM_DEPOT_FAILED",
            `Депот ${depot.depotId} скачан не полностью: файлов ${files}, ожидалось около ` +
              `${depot.expectedFiles || "нескольких"}`,
            { depotId: depot.depotId, manifestId: depot.manifestId, files }
          );

        this.logger.info(
          `[steam-depot] депот ${depot.depotId} скачан в ${depotDir}: файлов ${files}`
        );
      }

      return {
        status: "downloaded",
        buildId,
        targetDir,
        depots: entry.depots.map((depot) => ({
          depotId: depot.depotId,
          dir: path.join(targetDir, `depot_${depot.depotId}`),
        })),
      };
    } finally {
      this.unlinkContentSlot(slot.path);
    }
  }

  countFiles(dir) {
    let entries = [];
    try {
      entries = this.fsImpl.readdirSync(dir, { withFileTypes: true });
    } catch (error) {
      return 0;
    }

    let total = 0;

    for (const item of entries) {
      if (item.isSymbolicLink()) continue;
      if (item.isDirectory()) total += this.countFiles(path.join(dir, item.name));
      else total += 1;
    }

    return total;
  }
}

module.exports = SteamDepotService;
module.exports.CATALOG = CATALOG;
module.exports.RESPONSES = RESPONSES;
module.exports.ConsoleTail = ConsoleTail;
module.exports.catalogFor = catalogFor;
module.exports.assertDepot = assertDepot;
module.exports.MAX_CONTENT_SLOT = MAX_CONTENT_SLOT;
module.exports.defaultRun = defaultRun;
