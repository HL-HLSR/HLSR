"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { EventEmitter } = require("events");

const defaultInspector = require("./process-inspect");
const defaultStopper = require("./process-stop");
const { writeJsonAtomic, readJsonSafe } = require("./atomic");
const { assertInside, gameInstallPath } = require("./paths");
const { DeliveryError } = require("./errors");
const episodes = require("./episodes");

const STATE_VERSION = 1;

const STATES = ["not-running", "starting", "running", "stopping", "exited", "crashed", "unknown"];

const ACTIVE_STATES = ["starting", "running", "stopping"];

const STOP_TIMEOUT_MS = 10000;
const POLL_INTERVAL_MS = 1500;

function nowIso() {
  return new Date().toISOString();
}

function processKeyOf(pid, executablePath, startedAt) {
  return crypto
    .createHash("sha256")
    .update(`${pid}|${String(executablePath || "").toLowerCase()}|${startedAt || ""}`)
    .digest("hex")
    .slice(0, 32);
}

class GameProcessService extends EventEmitter {
  constructor(options) {
    const opts = options || {};
    super();

    this.paths = opts.paths;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.inspector = opts.inspector || defaultInspector;
    this.stopper = opts.stopper || defaultStopper;
    this.stopTimeoutMs = opts.stopTimeoutMs == null ? STOP_TIMEOUT_MS : opts.stopTimeoutMs;
    this.pollIntervalMs = opts.pollIntervalMs == null ? POLL_INTERVAL_MS : opts.pollIntervalMs;

    this.tracked = new Map();

    this.lastExits = new Map();
  }

  statePath(gameId) {
    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(String(gameId || "")))
      throw new DeliveryError("PATH_INVALID", `Недопустимый gameId: ${gameId}`);

    return assertInside(
      this.paths.state,
      path.join(this.paths.state, `${gameId}.process.json`),
      "Файл состояния процесса"
    );
  }

  persist(record) {
    try {
      writeJsonAtomic(this.statePath(record.gameId), Object.assign({ version: STATE_VERSION }, record));
    } catch (error) {
      this.logger.warn(`[game-process] состояние не сохранено: ${error.message}`);
    }
  }

  readPersisted(gameId) {
    const data = readJsonSafe(this.statePath(gameId));
    // gameId — из имени файла: по нему matchesRecord берёт папку игры. Файл в общей библиотеке:
    // эпизод из него — только известный (по нему на выходе пишется config.cfg Ghosting).
    return data && data.version === STATE_VERSION
      ? Object.assign(data, { gameId, episode: episodes.isEpisode(data.episode) ? data.episode : null })
      : null;
  }

  clearPersisted(gameId) {
    try {
      const file = this.statePath(gameId);
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch (error) {
      this.logger.warn(`[game-process] состояние не удалено: ${error.message}`);
    }
  }

  defaultState(gameId) {
    return {
      gameId,
      state: "not-running",
      pid: null,
      processKey: null,
      executablePath: null,
      startedAt: null,
      since: null,
      exitCode: null,
      signal: null,
      lastExit: null,
    };
  }

  publicState(gameId) {
    const entry = this.tracked.get(gameId);
    const state = entry ? entry.record : this.readPersisted(gameId) || this.defaultState(gameId);

    return {
      gameId,
      state: state.state,
      running: ACTIVE_STATES.indexOf(state.state) !== -1,
      since: state.since || null,
      startedAt: state.startedAt || null,
      exitCode: state.exitCode == null ? null : state.exitCode,
      lastExit: state.lastExit || this.lastExits.get(gameId) || null,
      episode: state.episode || null,
    };
  }

  getProcessState(gameId) {
    return this.publicState(gameId);
  }

  isRunning(gameId) {
    const entry = this.tracked.get(gameId);
    if (!entry) return false;
    return ACTIVE_STATES.indexOf(entry.record.state) !== -1;
  }

  anyRunning() {
    for (const gameId of this.tracked.keys()) if (this.isRunning(gameId)) return gameId;
    return null;
  }

  setState(gameId, state, extra) {
    const entry = this.tracked.get(gameId);
    const record = entry ? entry.record : this.defaultState(gameId);

    Object.assign(record, extra || {}, { state, since: nowIso() });

    if (entry) entry.record = record;

    if (ACTIVE_STATES.indexOf(state) !== -1) this.persist(record);
    else this.clearPersisted(gameId);

    const payload = this.publicState(gameId);
    this.emit("state", payload);
    this.emit(state, payload);

    return payload;
  }

  registerStartedProcess(gameId, processInfo) {
    const info = processInfo || {};
    const pid = Number(info.pid);

    this.lastExits.delete(gameId);

    if (!Number.isInteger(pid) || pid <= 0)
      throw new DeliveryError("PROCESS_NOT_FOUND", `Некорректный PID: ${info.pid}`);

    if (!info.executablePath || !path.isAbsolute(info.executablePath))
      throw new DeliveryError(
        "PROCESS_MISMATCH",
        "Нужен абсолютный путь исполняемого файла игры"
      );

    if (!info.installPath)
      throw new DeliveryError("PROCESS_MISMATCH", "Не задана папка установленной игры");

    const executablePath = assertInside(
      info.installPath,
      info.executablePath,
      "Исполняемый файл игры"
    );

    const current = this.tracked.get(gameId);

    if (current && ["running", "stopping"].indexOf(current.record.state) !== -1)
      throw new DeliveryError("ALREADY_RUNNING", `Игра ${gameId} уже отслеживается`);

    if (current) this.stopPolling(gameId);

    const startedAt = info.startedAt || nowIso();

    const record = Object.assign(this.defaultState(gameId), {
      state: "running",
      pid,
      executablePath,
      installPath: path.resolve(info.installPath),
      startedAt,
      processKey: info.processKey || processKeyOf(pid, executablePath, startedAt),
      launcherPid: process.pid,
      registeredAt: nowIso(),
      hostProcess: info.hostProcess === true,
      // Эпизод Ghosting: на выходе (и после перезапуска лаунчера) его config.cfg вернётся в Ghosting.
      episode: episodes.isEpisode(info.episode) ? info.episode : null,
    });

    const entry = { record, child: info.child || null, timer: null, stopRequested: false };
    this.tracked.set(gameId, entry);

    this.logger.info(
      `[game-process] игра ${gameId} зарегистрирована: PID ${pid}, ${executablePath}`
    );

    if (entry.child) this.attachChild(gameId, entry.child);
    else this.startPolling(gameId);

    this.setState(gameId, "running", {});
    return this.publicState(gameId);
  }

  markStarting(gameId) {
    const entry = this.tracked.get(gameId);
    if (entry && ACTIVE_STATES.indexOf(entry.record.state) !== -1) return this.publicState(gameId);

    this.tracked.set(gameId, {
      record: Object.assign(this.defaultState(gameId), { state: "starting" }),
      child: null,
      timer: null,
      stopRequested: false,
    });

    return this.setState(gameId, "starting", {});
  }

  cancelStarting(gameId, reason) {
    const entry = this.tracked.get(gameId);
    if (!entry || entry.record.state !== "starting") return this.publicState(gameId);

    this.logger.warn(`[game-process] запуск ${gameId} не состоялся: ${reason || "нет причины"}`);
    return this.unregisterProcess(gameId, { kind: "not-started", reason: reason || null });
  }

  attachChild(gameId, child) {
    const onExit = (code, signal) => this.handleExit(gameId, code, signal);

    child.once("exit", onExit);
    child.once("error", (error) => {
      this.logger.error(`[game-process] ошибка процесса ${gameId}: ${error.message}`);
      this.handleExit(gameId, null, null, error);
    });
  }

  startPolling(gameId) {
    const entry = this.tracked.get(gameId);
    if (!entry || entry.timer) return;

    entry.timer = setInterval(() => {
      const current = this.tracked.get(gameId);
      if (!current) return;

      if (!this.inspector.isAlive(current.record.pid)) {
        this.handleExit(gameId, current.stopRequested ? 0 : null, null, null, true);
      }
    }, this.pollIntervalMs);

    if (typeof entry.timer.unref === "function") entry.timer.unref();
  }

  stopPolling(gameId) {
    const entry = this.tracked.get(gameId);
    if (entry && entry.timer) {
      clearInterval(entry.timer);
      entry.timer = null;
    }
  }

  classifyExit(entry, exitCode, signal, error) {
    if (error) return "crashed";
    if (entry.stopRequested) return "exited";
    if (signal) return "crashed";
    if (exitCode === 0) return "exited";
    if (exitCode == null) return "exited";
    return "crashed";
  }

  // Процесс игры пропал, но тот же exe из той же папки уже запущен заново:
  // так GoldSrc перезапускает себя (смена видеорежима, _restart). Это та же
  // игра — лаунчер следит за новым процессом и не выходит из трея.
  findSuccessor(entry) {
    const record = entry.record;
    if (entry.stopRequested || entry.child || record.hostProcess) return null;
    if (typeof this.inspector.findByImageName !== "function") return null;
    try {
      const found = (this.inspector.findByImageName(path.basename(record.executablePath)) || [])
        .filter((item) => item.pid !== record.pid && item.executablePath &&
          this.inspector.samePath(item.executablePath, record.executablePath) &&
          this.inspector.isAlive(item.pid));
      return found[0] || null;
    } catch (error) {
      this.logger.warn(`[game-process] поиск перезапущенной игры не удался: ${error.message}`);
      return null;
    }
  }

  handleExit(gameId, exitCode, signal, error, detected) {
    const entry = this.tracked.get(gameId);
    if (!entry) return;

    if (["exited", "crashed"].indexOf(entry.record.state) !== -1) return;

    const successor = detected && !error ? this.findSuccessor(entry) : null;
    if (successor) {
      this.logger.info(`[game-process] игра ${gameId} перезапустилась: PID ${entry.record.pid} → ${successor.pid}`);
      const startedAt = successor.startedAt || nowIso();
      Object.assign(entry.record, {
        pid: successor.pid,
        startedAt,
        processKey: processKeyOf(successor.pid, entry.record.executablePath, startedAt),
      });
      this.persist(entry.record);
      return;
    }

    this.stopPolling(gameId);

    const outcome = this.classifyExit(entry, exitCode, signal, error);
    const lastExit = {
      outcome,
      exitCode: exitCode == null ? null : exitCode,
      signal: signal || null,
      requested: !!entry.stopRequested,
      detected: !!detected,
      error: error ? error.message : null,
      at: nowIso(),
      pid: entry.record.pid,
      startedAt: entry.record.startedAt,
    };

    if (outcome === "crashed")
      this.logger.error(
        `[game-process] игра ${gameId} завершилась с ошибкой: код ${lastExit.exitCode}, ` +
          `сигнал ${lastExit.signal || "нет"}, PID ${lastExit.pid}` +
          (lastExit.error ? `, ${lastExit.error}` : "")
      );
    else
      this.logger.info(
        `[game-process] игра ${gameId} закрыта штатно (код ${lastExit.exitCode}, PID ${lastExit.pid})`
      );

    this.lastExits.set(gameId, lastExit);

    this.setState(gameId, outcome, { exitCode: lastExit.exitCode, signal: lastExit.signal, lastExit });

    this.tracked.delete(gameId);
    this.clearPersisted(gameId);

    this.emit("finished", { gameId, lastExit, episode: entry.record.episode || null });
    this.emit("state", Object.assign(this.publicState(gameId), { lastExit }));
  }

  // Игра закрылась, хотя процесс-хозяин (сессия umu на Linux) ещё жив:
  // его держат другие программы префикса. pid — проверка, что это тот же
  // запуск, а не следующий.
  markExited(gameId, pid) {
    const entry = this.tracked.get(gameId);
    if (!entry || entry.record.pid !== pid) return false;
    this.handleExit(gameId, entry.stopRequested ? 0 : null, null, null, true);
    return true;
  }

  unregisterProcess(gameId, result) {
    const entry = this.tracked.get(gameId);
    this.stopPolling(gameId);
    this.tracked.delete(gameId);
    this.clearPersisted(gameId);

    const payload = Object.assign(this.publicState(gameId), {
      lastExit: (result && result.lastExit) || (entry && entry.record.lastExit) || null,
    });

    this.emit("state", payload);
    return payload;
  }

  async requestStop(gameId, options) {
    const opts = options || {};
    const entry = this.tracked.get(gameId);

    if (!entry || ACTIVE_STATES.indexOf(entry.record.state) === -1)
      return { status: "not-running" };

    const record = entry.record;

    if (!this.matchesRecord(record))
      throw new DeliveryError(
        "PROCESS_MISMATCH",
        "PID больше не принадлежит запущенной игре — завершение отменено",
        { gameId }
      );

    entry.stopRequested = true;
    this.setState(gameId, "stopping", {});

    const stop = record.hostProcess
      ? (opts.force ? this.stopper.forceGroup : this.stopper.gracefulGroup)
      : (opts.force ? this.stopper.force : this.stopper.graceful);
    const result = await stop.call(this.stopper, record.pid);

    if (!result.ok && !opts.force)
      this.logger.warn(
        `[game-process] запрос на закрытие ${gameId} вернул ошибку: ${result.output.trim()}`
      );

    const deadline = Date.now() + (opts.timeoutMs == null ? this.stopTimeoutMs : opts.timeoutMs);

    while (Date.now() < deadline) {
      if (!this.tracked.has(gameId)) return { status: "stopped" };

      if (!this.inspector.isAlive(record.pid)) {
        this.handleExit(gameId, 0, null, null, true);
        return { status: "stopped" };
      }

      await new Promise((resolve) => setTimeout(resolve, 250));
    }

    if (!this.tracked.has(gameId)) return { status: "stopped" };

    if (opts.force)
      throw new DeliveryError("STOP_FAILED", "Процесс не удалось завершить даже принудительно", {
        gameId,
      });

    entry.stopRequested = false;
    this.setState(gameId, "running", {});

    return { status: "timeout", timeoutMs: this.stopTimeoutMs };
  }

  matchesRecord(record) {
    if (!record || !record.pid) return false;
    if (!this.inspector.isAlive(record.pid)) return false;

    if (record.hostProcess) {
      const live = this.tracked.get(record.gameId);
      return !!(live && live.child && live.child.pid === record.pid);
    }

    const actual = this.inspector.inspect(record.pid);
    if (!actual) return false;

    if (!this.inspector.samePath(actual.executablePath, record.executablePath)) return false;

    // Файл состояния лежит в общей библиотеке, его может написать другой
    // пользователь ПК: папка игры берётся своя, а не из записи.
    try {
      assertInside(gameInstallPath(this.paths, record.gameId), actual.executablePath, "Исполняемый файл игры");
    } catch (error) {
      return false;
    }

    if (!this.inspector.sameStart(record.startedAt, actual.startedAt)) return false;

    if (record.processKey && actual.startedAt) {
      const expected = processKeyOf(record.pid, record.executablePath, record.startedAt);
      if (record.processKey !== expected) return false;
    }

    return true;
  }

  reconcileAfterLauncherRestart(gameIds) {
    const ids = gameIds && gameIds.length ? gameIds : this.persistedGameIds();
    const results = [];

    for (const gameId of ids) {
      const record = this.readPersisted(gameId);
      if (!record) continue;

      if (!this.matchesRecord(record)) {
        this.logger.info(
          `[game-process] сохранённый процесс ${gameId} (PID ${record.pid}) больше не наш — ` +
            "состояние очищено"
        );

        this.clearPersisted(gameId);
        results.push({ gameId, adopted: false, reason: "stale" });
        this.emit("state", this.publicState(gameId));
        continue;
      }

      this.logger.warn(
        `[game-process] игра ${gameId} всё ещё запущена (PID ${record.pid}) — ` +
          "восстанавливаю состояние после перезапуска лаунчера"
      );

      this.tracked.set(gameId, {
        record: Object.assign({}, record, { state: "running", adopted: true }),
        child: null,
        timer: null,
        stopRequested: false,
      });

      this.startPolling(gameId);
      this.setState(gameId, "running", { adopted: true });

      results.push({ gameId, adopted: true, pid: record.pid });
    }

    return results;
  }

  persistedGameIds() {
    let items;

    try {
      items = fs.readdirSync(this.paths.state);
    } catch (error) {
      return [];
    }

    return items
      .filter((name) => name.endsWith(".process.json"))
      .map((name) => name.slice(0, -".process.json".length));
  }

  dispose() {
    for (const gameId of Array.from(this.tracked.keys())) this.stopPolling(gameId);
    this.tracked.clear();
  }
}

module.exports = GameProcessService;
module.exports.STATES = STATES;
module.exports.ACTIVE_STATES = ACTIVE_STATES;
module.exports.processKeyOf = processKeyOf;
