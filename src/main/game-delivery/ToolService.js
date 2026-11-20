"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { spawn } = require("child_process");
const { EventEmitter } = require("events");
const { Transform } = require("stream");
const { pipeline } = require("stream/promises");

const { DeliveryError } = require("./errors");
const { assertInside, removeWithinZone } = require("./paths");
const http = require("./http-client");
const LiveSplitSplits = require("./livesplit-splits");

const STATE_FILE = "installed.json";
const STATE_VERSION = 1;

const MAX_ARCHIVE_BYTES = 512 * 1024 * 1024;

const DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1000;

const TOOL_ID = /^[a-z0-9]+([._-][a-z0-9]+)*$/;
const LIVESPLIT_TOOL_ID = "livesplit";

// Режим дисплея (PROTON_ENABLE_WAYLAND) задаёт ProtonRunner по игре префикса:
// LiveSplit не должен выбирать его сам, иначе, запущенный первым, он навязал
// бы свой драйвер игре.
function toolLaunchEnvironment(toolId, hostEnvironment = process.env) {
  if (toolId !== LIVESPLIT_TOOL_ID || !hostEnvironment.DISPLAY) return null;
  return { PROTON_USE_X11_EXCLUSIVE: "LiveSplit.exe" };
}

// Процесс Wine с именем exe есть в /proc (Wine называет процесс по exe).
function wineProcessRunning(fsModule, exeName) {
  const wanted = String(exeName).toLowerCase().slice(0, 15);
  let entries;
  try { entries = fsModule.readdirSync("/proc"); } catch (error) { return null; }
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      if (fsModule.readFileSync(`/proc/${entry}/comm`, "utf8").trim().toLowerCase() === wanted) return true;
    } catch (error) { /* процесс уже завершился или чужой */ }
  }
  return false;
}

class ToolService extends EventEmitter {
  constructor(options) {
    const opts = options || {};
    super();

    this.paths = opts.paths;
    this.registry = opts.registry || {};
    this.archive = opts.archive || null;
    this.channel = opts.channel || null;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.spawnFn = opts.spawnFn || spawn;
    this.fs = opts.fs || fs;
    this.protonRunner = opts.protonRunner || null;
    this.rateLimiter = opts.rateLimiter || null;
    this.httpClient = opts.httpClient || http;
    this.signalFn = opts.signalFn || process.kill.bind(process);
    // Windows закрывает инструмент через taskkill (process-stop): сигнала
    // группе процессов там нет.
    this.platform = opts.platform || process.platform;
    this.stopper = opts.stopper || require("./process-stop");
    this.hostEnvironment = opts.hostEnvironment || process.env;
    this.resourcesPath = opts.resourcesPath || process.resourcesPath;
    this.liveSplitFixes = opts.liveSplitFixes || require("../linux/LiveSplitFixes").applyLiveSplitFixes;
    this.liveSplitSplits =
      opts.liveSplitSplits || new LiveSplitSplits({ fs: this.fs, resourcesPath: this.resourcesPath, logger: this.logger });

    this.processRunning = opts.processRunning || ((name) => wineProcessRunning(this.fs, name));
    this.watchIntervalMs = opts.watchIntervalMs || 2000;

    this.busyWith = null;
    this.installJob = null;
    this.running = new Map();
  }

  toolsRoot() {
    if (!this.paths || !this.paths.tools)
      throw new DeliveryError("PATH_INVALID", "Каталог инструментов не задан");
    return this.paths.tools;
  }

  installPathOf(toolId) {
    if (!TOOL_ID.test(String(toolId || "")))
      throw new DeliveryError("TOOL_UNKNOWN", `Недопустимый идентификатор инструмента: ${toolId}`);

    const target = path.join(this.toolsRoot(), toolId);
    assertInside(this.toolsRoot(), target);
    return target;
  }

  manifestOf(toolId) {
    const manifest = this.registry[toolId];
    if (!manifest) throw new DeliveryError("TOOL_UNKNOWN", `Нет манифеста инструмента ${toolId}`);
    return manifest;
  }

  status(toolId) {
    const base = {
      toolId,
      state: this.busyWith === toolId ? "installing" : "not-installed",
      version: null,
      availableVersion: null,
      updateAvailable: false,
      installPath: null,
      percent: 0,
      error: null,
      running: this.running.has(toolId),
      stopping: !!(this.running.get(toolId) && this.running.get(toolId).stopping),
    };

    let manifest = null;

    try {
      manifest = this.manifestOf(toolId);
    } catch (error) {
      return Object.assign(base, { state: "unknown", error: { code: error.code } });
    }

    base.availableVersion = manifest.version;

    const installed = this.readInstalled(toolId);
    if (!installed) return base;

    base.state = this.busyWith === toolId ? "installing" : "ready";
    base.version = installed.version;
    base.installPath = installed.installPath;
    base.updateAvailable = !!(manifest.version && installed.version !== manifest.version);

    return base;
  }

  readInstalled(toolId) {
    let file;

    try {
      file = path.join(this.installPathOf(toolId), STATE_FILE);
    } catch (error) {
      return null;
    }

    try {
      const data = JSON.parse(this.fs.readFileSync(file, "utf8"));
      if (!data || data.toolId !== toolId || !data.version) return null;
      return data;
    } catch (error) {
      return null;
    }
  }

  async install(toolId, onProgress) {
    if (this.running.has(toolId))
      throw new DeliveryError("ALREADY_RUNNING", "Закройте инструмент перед обновлением");
    if (this.busyWith)
      throw new DeliveryError("ALREADY_RUNNING", "Установка уже идёт");

    const manifest = this.manifestOf(toolId);
    const target = this.installPathOf(toolId);

    if (manifest.enabled === false)
      throw new DeliveryError("REMOTE_DISABLED", "Инструмент временно недоступен");

    const staging = path.join(this.paths.staging, `tool-${toolId}`);
    const archivePath = path.join(this.paths.downloads, manifest.archive.filename);

    this.busyWith = toolId;
    const job = { toolId, controller: new AbortController(), extract: null };
    this.installJob = job;
    const signal = job.controller.signal;

    try {
      this.fs.mkdirSync(this.toolsRoot(), { recursive: true });
      this.fs.mkdirSync(this.paths.downloads, { recursive: true });
      this.fs.mkdirSync(this.paths.staging, { recursive: true });

      await this.download(manifest, archivePath, onProgress, signal);
      signal.throwIfAborted();

      removeWithinZone(this.paths.root, staging);
      this.fs.mkdirSync(staging, { recursive: true });

      // manifest.raw — скачивается сам файл инструмента (bxt_rs.dll), не архив.
      if (manifest.raw) this.fs.renameSync(archivePath, path.join(staging, manifest.executable));
      else {
        if (!this.archive) throw new DeliveryError("EXTRACT_FAILED", "Распаковщик не подключён");

        // manifest.extract — распаковать только эти файлы архива, плоско в корень.
        job.extract = this.archive.extract(archivePath, staging, manifest.extract ? { files: manifest.extract } : undefined);
        await job.extract.promise;
        signal.throwIfAborted();
      }

      // manifest.extras — файлы рядом с инструментом (лицензия, уведомления), каждый с закреплёнными размером и sha256.
      for (const extra of manifest.extras || []) {
        if (!extra || !extra.name || !extra.url || !extra.size || !extra.sha256)
          throw new DeliveryError("MANIFEST_INVALID", "У файла инструмента нет размера или sha256");
        const file = path.join(staging, extra.name);
        assertInside(staging, file);
        await this.downloadFrom(extra.url, extra, file, null, signal);
      }

      const exe = path.join(staging, manifest.executable);
      assertInside(staging, exe);

      if (!this.fs.existsSync(exe))
        throw new DeliveryError(
          "EXTRACT_FAILED",
          `В архиве нет ${manifest.executable} — распаковано не то, что ожидалось`
        );

      await this.verifyFiles(staging, manifest.files);

      removeWithinZone(this.paths.root, target);
      this.fs.renameSync(staging, target);

      this.writeInstalled(toolId, manifest, target);

      if (!manifest.raw)
        try {
          this.fs.unlinkSync(archivePath);
        } catch (error) {
          this.logger.warn(`[tools] архив не удалён: ${error.message}`);
        }

      this.logger.info(`[tools] ${toolId} ${manifest.version} установлен: ${target}`);
      return { status: "installed", version: manifest.version, installPath: target };
    } catch (error) {
      if (signal.aborted) throw new DeliveryError("CANCELLED", "Установка инструмента отменена");
      throw error;
    } finally {
      this.busyWith = null;
      this.installJob = null;
      try { this.fs.unlinkSync(archivePath); } catch (ignored) {}
      try {
        removeWithinZone(this.paths.root, staging);
      } catch (ignored) {
      }
    }
  }

  // manifest.files — закреплённые SHA-256 распакованных файлов ({ имя: sha256 }).
  async verifyFiles(root, files) {
    for (const name of Object.keys(files || {})) {
      const file = path.join(root, name);
      assertInside(root, file);
      const hash = crypto.createHash("sha256");
      try {
        for await (const chunk of this.fs.createReadStream(file)) hash.update(chunk);
      } catch (error) {
        throw new DeliveryError("EXTRACT_FAILED", `В архиве нет ${name}`);
      }
      if (hash.digest("hex") !== String(files[name]).toLowerCase())
        throw new DeliveryError("CHECKSUM_MISMATCH", `SHA-256 ${name} не совпал с закреплённым`);
    }
  }

  cancel(toolId) {
    const job = this.installJob;
    if (!job || job.toolId !== toolId) return false;
    job.controller.abort();
    if (job.extract) job.extract.cancel();
    return true;
  }

  writeInstalled(toolId, manifest, installPath) {
    const state = {
      schemaVersion: STATE_VERSION,
      toolId,
      version: manifest.version,
      executable: manifest.executable,
      installPath,
      installedAt: new Date().toISOString(),
    };

    this.fs.writeFileSync(path.join(installPath, STATE_FILE), JSON.stringify(state, null, 2));
  }

  async download(manifest, target, onProgress, signal) {
    const descriptor = manifest.archive;
    const providers = descriptor.providers || [];

    if (!providers.length)
      throw new DeliveryError("MANIFEST_INVALID", "У архива инструмента нет ни одного источника");

    let lastError = null;

    for (const provider of providers) {
      try {
        if (signal) signal.throwIfAborted();
        await this.downloadFrom(provider.url, descriptor, target, onProgress, signal);
        return;
      } catch (error) {
        if (signal && signal.aborted) throw new DeliveryError("CANCELLED", "Загрузка инструмента отменена");
        lastError = error;
        this.logger.warn(`[tools] источник ${provider.id} не сработал: ${error.message}`);
      }
    }

    throw lastError || new DeliveryError("NETWORK_ERROR", "Скачать архив инструмента не удалось");
  }

  async downloadFrom(url, descriptor, target, onProgress, signal) {
    if (descriptor.size > MAX_ARCHIVE_BYTES)
      throw new DeliveryError("MANIFEST_INVALID", "Архив инструмента подозрительно велик");

    const part = `${target}.part`;
    let writeError = null;
    try {
      const response = await this.httpClient.request(url, { timeout: DOWNLOAD_TIMEOUT_MS, signal });
      if (response.statusCode !== 200) {
        response.resume();
        throw new DeliveryError("NETWORK_ERROR", `Источник ответил ${response.statusCode}`);
      }
      const hash = crypto.createHash("sha256");
      let received = 0;
      const meter = new Transform({
        transform(chunk, encoding, callback) {
          received += chunk.length;
          if (received > MAX_ARCHIVE_BYTES)
            return callback(new DeliveryError("NETWORK_ERROR", "Ответ длиннее допустимого"));
          hash.update(chunk);
          if (onProgress && descriptor.size)
            onProgress(Math.min(100, Math.round((received / descriptor.size) * 100)), received);
          callback(null, chunk);
        },
      });
      const writer = fs.createWriteStream(part);
      writer.on("error", (error) => {
        if (["open", "write", "writev"].includes(error.syscall)) writeError = error;
      });
      const streams = [response];
      if (this.rateLimiter) streams.push(this.rateLimiter.createStream());
      streams.push(meter, writer);
      await pipeline(...streams, { signal });

      if (descriptor.size && received !== descriptor.size)
        throw new DeliveryError("SIZE_MISMATCH", `Размер архива не совпал: ${received} вместо ${descriptor.size}`);
      if (descriptor.sha256 && hash.digest("hex") !== descriptor.sha256)
        throw new DeliveryError("CHECKSUM_MISMATCH", "SHA-256 скачанного архива не совпал с объявленным");
      try {
        this.fs.renameSync(part, target);
      } catch (error) {
        throw new DeliveryError("PATH_NOT_WRITABLE", error.message);
      }
    } catch (error) {
      try {
        this.fs.unlinkSync(part);
      } catch (ignored) {
      }
      if (signal && signal.aborted) throw new DeliveryError("CANCELLED", "Загрузка инструмента отменена");
      if (writeError) throw new DeliveryError("PATH_NOT_WRITABLE", writeError.message);
      throw error instanceof DeliveryError ? error : new DeliveryError("NETWORK_ERROR", error.message);
    }
  }

  async launch(toolId, options) {
    const opts = options || {};
    const current = this.running.get(toolId);
    if (current) {
      // Уже открытый LiveSplit не перечитывает аргументы, поэтому другие
      // сплиты или другая игра — только через перезапуск (opts.restart).
      const same =
        toolId !== LIVESPLIT_TOOL_ID ||
        ((!opts.gameId || current.gameId === opts.gameId) && (!opts.splits || current.splits === opts.splits));
      if (same || !opts.restart)
        return { status: "already-running", gameId: current.gameId, splits: current.splits, same };
      const stopped = await this.stop(toolId);
      if (!stopped || (stopped.status !== "stopped" && stopped.status !== "not-running")) return { status: "stop-timeout" };
    }
    const installed = this.readInstalled(toolId);

    if (!installed) throw new DeliveryError("NOT_INSTALLED", "Инструмент не установлен");

    const manifest = this.manifestOf(toolId);
    const root = this.installPathOf(toolId);
    const exe = path.join(root, installed.executable || manifest.executable);

    assertInside(root, exe);

    if (!this.fs.existsSync(exe))
      throw new DeliveryError(
        "TOOL_EXECUTABLE_MISSING",
        `Файл ${installed.executable || manifest.executable} не найден — переустановите инструмент`
      );

    // На Windows LiveSplit общий, но запоминается, для какой игры он
    // открыт: оверлей LiveSplit показывается только в этой игре.
    const gameId = opts.gameId || (this.platform === "linux" ? "ghosting-renewed" : null);
    const splits = toolId === LIVESPLIT_TOOL_ID ? this.splitArguments(root, gameId, opts.splits) : { args: [], preset: null };
    const args = splits.args;
    const launchInfo = { gameId, splits: splits.preset };
    if (toolId === LIVESPLIT_TOOL_ID && Number.isInteger(opts.serverPort))
      try {
        this.liveSplitSplits.enableServer(root, opts.serverPort);
      } catch (error) {
        this.logger.warn(`[tools] livesplit: TCP-сервер не включён: ${error.message}`);
      }

    if (this.platform !== "linux") return this.launchDirect(toolId, exe, root, args, launchInfo);

    if (!this.protonRunner) throw new DeliveryError("PROTON_NOT_INSTALLED", "Proton не настроен");
    const launchOptions = { cwd: root };
    if (toolId === LIVESPLIT_TOOL_ID) {
      launchOptions.fontProfile = "livesplit";
      // Исправления компонентов под Wine (LiveSplitFixes). Без них LiveSplit
      // всё равно запускается, поэтому ошибка только пишется в журнал.
      try {
        for (const file of this.liveSplitFixes(root, { resourcesPath: this.resourcesPath, fs: this.fs }))
          this.logger.info(`[tools] livesplit: заменён ${file} исправленным под Wine`);
      } catch (error) {
        this.logger.warn(`[tools] livesplit: исправления не применены: ${error.message}`);
      }
    }
    const toolEnvironment = toolLaunchEnvironment(toolId, this.hostEnvironment);
    if (toolEnvironment) {
      // Proton Wineland defaults to winewayland.drv. LiveSplit is a movable,
      // resizable WinForms utility window, which behaves correctly through
      // Wine's X11 driver.
      launchOptions.env = toolEnvironment;
    }
    const launched = await this.protonRunner.launch(gameId, exe, args, launchOptions);
    const child = launched.child;
    this.track(toolId, child, launchInfo);
    this.watchWineProcess(toolId, path.win32.basename(installed.executable || manifest.executable));

    this.logger.info(`[tools] ${toolId} запущен в префиксе ${launched.prefix}: ${exe}`);
    return { status: "started", pid: child ? child.pid : null };
  }

  // Запущенный инструмент запоминается до выхода процесса: по этому
  // интерфейс показывает «Закрыть» вместо «Запустить».
  // Запущен ли инструмент и в префиксе какой игры (null на Windows).
  runningInfo(toolId) {
    const record = this.running.get(toolId);
    return record && !record.stopping ? { gameId: record.gameId, splits: record.splits } : null;
  }

  // info.gameId — для какой игры открыт инструмент (на Linux — чей префикс
  // Proton: окно инструмента видно только там), info.splits — набор сплитов.
  track(toolId, child, info) {
    if (!child) return;
    const launched = info || {};
    const record = { child, stopping: false, gameId: launched.gameId || null, splits: launched.splits || null };
    this.running.set(toolId, record);
    const finished = () => {
      if (this.running.get(toolId) !== record) return;
      this.running.delete(toolId);
      this.emit("state", toolId);
    };
    child.once("exit", finished);
    child.once("error", finished);
    this.emit("state", toolId);
    if (typeof child.exitCode === "number" || typeof child.signalCode === "string")
      finished();

    if (typeof child.unref === "function") child.unref();
  }

  // Linux: сессия umu не завершается, пока в префиксе работает игра, поэтому
  // LiveSplit, закрытый своим крестиком или «Закрыть» в меню, лаунчер считал
  // открытым: оверлей оставался привязан, а новый запуск отвечал
  // «уже запущен». Следим за самим процессом exe: пропал два опроса подряд —
  // инструмент закрыт, сессия umu закрывается вместе с ним.
  watchWineProcess(toolId, exeName) {
    const record = this.running.get(toolId);
    if (!record || !exeName) return;
    let seen = false;
    let missing = 0;
    const timer = setInterval(() => {
      if (this.running.get(toolId) !== record) return clearInterval(timer);
      const alive = this.processRunning(exeName);
      if (alive === null) return;
      if (alive) {
        seen = true;
        missing = 0;
        return;
      }
      // Пока Wine поднимает префикс, exe ещё нет: ждём его появления.
      if (!seen || record.stopping || ++missing < 2) return;
      clearInterval(timer);
      this.logger.info(`[tools] ${toolId}: ${exeName} закрыт вне лаунчера`);
      this.running.delete(toolId);
      const pid = Number(record.child && record.child.pid);
      if (Number.isSafeInteger(pid) && pid > 0) {
        try { this.signalFn(-pid, "SIGTERM"); } catch (ignored) { /* сессия уже завершилась */ }
      }
      this.emit("state", toolId);
    }, this.watchIntervalMs);
    if (typeof timer.unref === "function") timer.unref();
  }

  // Windows: инструмент запускается напрямую. Он отделён от лаунчера и
  // переживает его закрытие, но пока лаунчер открыт, процесс отслеживается
  // и его можно закрыть из лаунчера, как на Linux.
  launchDirect(toolId, exe, root, args, info) {
    const child = this.spawnFn(exe, args, {
      cwd: root,
      detached: true,
      stdio: "ignore",
      windowsHide: false,
      shell: false,
    });
    this.track(toolId, child, info);

    this.logger.info(`[tools] ${toolId} запущен: ${exe} [${args.join(" ")}]`);
    return { status: "started", pid: child ? child.pid : null };
  }

  async stop(toolId, options) {
    const record = this.running.get(toolId);
    if (!record) return { status: "not-running" };
    if (record.stopping) return { status: "stopping" };
    const pid = Number(record.child && record.child.pid);
    if (!Number.isSafeInteger(pid) || pid <= 0)
      throw new DeliveryError("PROCESS_NOT_FOUND", "Процесс инструмента не найден");

    record.stopping = true;
    this.emit("state", toolId);
    const force = !!(options && options.force);
    const failed = (message) => {
      record.stopping = false;
      this.emit("state", toolId);
      return new DeliveryError("STOP_FAILED", `Не удалось закрыть инструмент: ${message}`);
    };
    if (this.platform === "win32") {
      // Без /F taskkill просит окно закрыться (LiveSplit успеет спросить про
      // сплиты), с /F — снимает процесс. /T — вместе с дочерними.
      const result = await (force ? this.stopper.force(pid) : this.stopper.graceful(pid));
      const exited = typeof record.child.exitCode === "number" || !this.running.has(toolId);
      if (!result.ok && !exited) throw failed(result.output || `taskkill ${result.code}`);
    } else {
      try {
        this.signalFn(-pid, force ? "SIGKILL" : "SIGTERM");
      } catch (error) {
        throw failed(error.message);
      }
    }

    const stopped = await new Promise((resolve) => {
      if (!this.running.has(toolId)) return resolve(true);
      const done = () => {
        clearTimeout(timer);
        record.child.removeListener("exit", done);
        record.child.removeListener("error", done);
        resolve(true);
      };
      const timer = setTimeout(() => {
        record.child.removeListener("exit", done);
        record.child.removeListener("error", done);
        resolve(false);
      }, 10000);
      record.child.once("exit", done);
      record.child.once("error", done);
    });
    if (stopped || !this.running.has(toolId)) return { status: "stopped" };
    record.stopping = false;
    this.emit("state", toolId);
    return { status: "timeout" };
  }

  // Сплиты и раскладка игры (livesplit-splits.js). Без них LiveSplit всё
  // равно запускается — с последними открытыми сплитами.
  splitArguments(root, gameId, presetId) {
    if (!gameId) return { args: [], preset: null };
    try {
      const prepared = this.liveSplitSplits.prepare(root, gameId, presetId);
      if (prepared.preset) this.logger.info(`[tools] livesplit: сплиты ${gameId}/${prepared.preset.id}`);
      return { args: prepared.args, preset: prepared.preset ? prepared.preset.id : null };
    } catch (error) {
      this.logger.warn(`[tools] livesplit: сплиты ${gameId} не подготовлены: ${error.message}`);
      return { args: [], preset: null };
    }
  }

  remove(toolId) {
    if (this.busyWith === toolId) throw new DeliveryError("ALREADY_RUNNING", "Установка ещё выполняется");
    if (this.running.has(toolId))
      throw new DeliveryError("ALREADY_RUNNING", "Закройте инструмент перед удалением");
    const target = this.installPathOf(toolId);

    if (!this.fs.existsSync(target)) return { status: "not-installed" };

    removeWithinZone(this.paths.root, target);

    this.logger.info(`[tools] ${toolId} удалён: ${target}`);
    return { status: "removed" };
  }
}

module.exports = ToolService;
module.exports.LIVESPLIT_TOOL_ID = LIVESPLIT_TOOL_ID;
module.exports.toolLaunchEnvironment = toolLaunchEnvironment;
module.exports.MAX_ARCHIVE_BYTES = MAX_ARCHIVE_BYTES;
module.exports.STATE_FILE = STATE_FILE;
