"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const { DemoError } = require("./errors");
const planner = require("./planner");
const jobs = require("./jobs");
const { RenderHost, scriptPath } = require("./render-host");
const { removeWithinZone } = require("../game-delivery/paths");
const defaultInspector = require("../game-delivery/process-inspect");
const { systemRoot } = require("../windows-tools");

// Общее у адаптеров, которые рендерят настоящую игру на скрытом рабочем
// столе через помощник render-host (Ghosting — ghosting.js, HL1 — goldsrc.js):
// - реестр игры: снимок до первого запуска, возврат после; снимок с маркером
//   в корне заданий живёт, пока не возвращён (после сбоя — следующий старт
//   лаунчера или следующий рендер);
// - звук: игра всегда закрывается заглушённой, mute, запомненный Windows,
//   снимает беззвучный перезапуск в конце рендера; не снят — флаг в Store,
//   следующий рендер снимет его со всех сессий, заглушённых до него;
// - предусловия: игра не запущена и не занята, Steam;
// - папка рендера внутри игры и выход лаунчера посреди рендера.
// Что у каждой игры своё — spec: { gameId, image, registryKey, marker,
// regFile, unmutePending, label, required, renderRoot(install), caps }.

const FPS = [30, 60, 120];

// ---------- реестр игрока ----------

// reg.exe. «Ключа нет» — только наверняка: родитель читается, а ключа среди
// его подразделов нет. Таймаут, запрет политикой и прочее — REGISTRY_FAILED
// (тексты reg.exe локализованы, по ним не судим).
function regTool(env, runFn) {
  const exe = path.win32.join(systemRoot(env), "System32", "reg.exe");
  const run = runFn || ((args) => spawnSync(exe, args, { windowsHide: true, encoding: "utf8", timeout: 15000 }));
  const failed = (what, result) => new DemoError("REGISTRY_FAILED", `reg ${what}: ${result.status} ${String(result.stderr || result.error || "").trim()}`);
  const norm = (key) => key.replace(/^HKEY_CURRENT_USER\\/i, "HKCU\\").replace(/\\+$/, "").toLowerCase();
  function exists(key) {
    const own = run(["query", key]);
    if (own.status === 0) return true;
    const parent = key.slice(0, key.lastIndexOf("\\"));
    if (!parent.includes("\\")) throw failed("query", own);
    const listed = run(["query", parent]);
    if (listed.status === 0) {
      const children = String(listed.stdout || "").split(/\r?\n/).map((line) => line.trim()).filter((line) => /^HKEY_|^HKCU\\/i.test(line)).map(norm);
      if (children.includes(norm(key))) throw failed("query", own);
      return false;
    }
    if (exists(parent)) throw failed("query", listed);
    return false;
  }
  return {
    exists,
    exportKey(key, file) {
      const result = run(["export", key, file, "/y"]);
      if (result.status !== 0) throw failed("export", result);
    },
    deleteKey(key) {
      const result = run(["delete", key, "/f"]);
      if (result.status !== 0 && exists(key)) throw failed("delete", result);
    },
    importFile(file) {
      const result = run(["import", file]);
      if (result.status !== 0) throw failed("import", result);
    },
  };
}

function readMarker(root, spec) {
  try {
    const data = JSON.parse(fs.readFileSync(path.join(root, spec.marker), "utf8"));
    return data && data.version === 1 ? data : null;
  } catch (error) {
    return null;
  }
}

function writeMarker(root, spec, data) {
  const file = path.join(root, spec.marker);
  fs.writeFileSync(`${file}.tmp`, JSON.stringify(data, null, 2) + "\n");
  fs.renameSync(`${file}.tmp`, file);
}

// Снимок ключа до первого запуска игры. Маркер пишется до запуска: если
// лаунчер упадёт посреди рендера, следующий старт вернёт реестр по нему.
// Невозвращённый снимок не перезаписывается никогда.
function snapshotRegistry(reg, root, spec, extra) {
  if (readMarker(root, spec)) throw new DemoError("REGISTRY_FAILED", "Прошлый снимок реестра ещё не возвращён");
  const existed = reg.exists(spec.registryKey);
  if (existed) reg.exportKey(spec.registryKey, path.join(root, spec.regFile));
  else fs.rmSync(path.join(root, spec.regFile), { force: true });
  const marker = Object.assign({ version: 1, existed, at: new Date().toISOString(), muted: false }, extra || {});
  writeMarker(root, spec, marker);
  return marker;
}

// Вернуть ключ как был (ключа не было — удалить созданный игрой). Ключ —
// всегда наш, из маркера берётся только «был ли». Маркер и снимок удаляются
// только после удачного возврата.
function restoreRegistry(reg, root, spec) {
  const marker = readMarker(root, spec);
  if (!marker) return null;
  const regFile = path.join(root, spec.regFile);
  if (marker.existed && !fs.existsSync(regFile)) throw new DemoError("REGISTRY_FAILED", "Нет снимка реестра");
  reg.deleteKey(spec.registryKey);
  if (marker.existed) reg.importFile(regFile);
  fs.rmSync(regFile, { force: true });
  fs.rmSync(path.join(root, spec.marker), { force: true });
  return marker;
}

// ---------- папка рендера в игре ----------

// Остатки прошлых рендеров — только внутри root (папка рендера игры), по
// ссылкам не ходим: ссылка снимается сама, папка удаляется rmSync (внутрь ссылок он не идёт).
function cleanupRenderDirs(root, keep) {
  let stat;
  try {
    stat = fs.lstatSync(root);
  } catch (error) {
    return 0;
  }
  if (stat.isSymbolicLink() || !stat.isDirectory()) {
    try {
      fs.unlinkSync(root);
    } catch (error) {
      try { fs.rmdirSync(root); } catch (ignored) { /* не наше — не трогаем */ }
    }
    return 1;
  }
  let removed = 0;
  for (const name of fs.readdirSync(root)) {
    if (keep && name.toLowerCase() === keep.toLowerCase()) continue;
    const full = path.join(root, name);
    try {
      const entry = fs.lstatSync(full);
      if (entry.isSymbolicLink()) {
        try { fs.unlinkSync(full); } catch (error) { fs.rmdirSync(full); }
      } else removeWithinZone(root, full);
      removed++;
    } catch (error) {
      /* занято — уберётся в следующий раз */
    }
  }
  if (!keep) {
    try { fs.rmdirSync(root); } catch (error) { /* не пустая */ }
  }
  return removed;
}

// Своя папка задания внутри папки рендера игры (root — не ссылка).
function makeRenderDir(root, name) {
  cleanupRenderDirs(root, null);
  fs.mkdirSync(root, { recursive: true });
  const stat = fs.lstatSync(root);
  if (stat.isSymbolicLink() || !stat.isDirectory()) throw new DemoError("RENDER_FAILED", `${path.basename(root)} — ссылка`);
  const dir = path.join(root, name);
  fs.mkdirSync(dir);
  return dir;
}

function linkOrCopy(source, target) {
  try {
    fs.linkSync(source, target);
  } catch (error) {
    fs.copyFileSync(source, target, fs.constants.COPYFILE_EXCL);
  }
}

// Синхронное ожидание (выход лаунчера): цикл событий уже не нужен.
function waitDead(pid, timeoutMs) {
  const alive = () => {
    try {
      process.kill(pid, 0);
      return true;
    } catch (error) {
      return error.code === "EPERM";
    }
  };
  const until = Date.now() + timeoutMs;
  const cell = new Int32Array(new SharedArrayBuffer(4));
  while (alive() && Date.now() < until) Atomics.wait(cell, 0, 0, 50);
  return !alive();
}

// Наибольшее разрешение из списка, которое помещается на основной монитор:
// скрытый стол живёт в той же конфигурации дисплеев, окно больше экрана
// обрезается. Игры не DPI-aware — размер экрана в DIP (AppManager).
function maxHeightFor(screen) {
  const fits = planner.RESOLUTIONS.filter((item) => screen && item.width <= screen.width && item.height <= screen.height);
  return fits.length ? fits[fits.length - 1].height : planner.RESOLUTIONS[0].height;
}

class GameRenderAdapter {
  constructor(options, spec) {
    const opts = options || {};
    this.spec = spec;
    this.platform = opts.platform || process.platform;
    this.env = opts.env || process.env;
    this.logger = opts.logger || { info() {}, warn() {} };
    // Путь установки из библиотеки лаунчера (null — не установлена).
    this.installPathFn = opts.installPath || (() => null);
    this.gameRunning = opts.gameRunning || (() => false);
    this.gameBusy = opts.gameBusy || (() => false);
    this.steamStatus = opts.steamStatus || (() => ({ ok: true }));
    this.screenSize = opts.screenSize || (() => ({ width: 1920, height: 1080 }));
    this.jobsRoot = opts.jobsRoot || null;
    this.store = opts.store || null;
    this.inspector = opts.inspector || defaultInspector;
    this.reg = opts.registry || regTool(this.env);
    this.script = opts.script === undefined ? scriptPath(opts.resourcesPath) : opts.script;
    this.hostFactory = opts.hostFactory || ((hostOptions) => new RenderHost(hostOptions));
    this.active = null;
  }

  get capabilities() {
    let screen = null;
    try {
      screen = this.screenSize();
    } catch (error) {
      screen = null;
    }
    return Object.assign({ maxHeight: maxHeightFor(screen), fps: FPS.slice() }, this.spec.caps);
  }

  installPath() {
    try {
      const found = this.installPathFn();
      return found && this.spec.required.every((name) => fs.existsSync(path.join(found, name))) ? found : null;
    } catch (error) {
      return null;
    }
  }

  isActive() {
    return !!this.active;
  }

  // Старт лаунчера: маркер после сбоя — вернуть реестр. Mute после сбоя без
  // процесса игры не снять: флаг, снимет следующий рендер.
  recover() {
    if (!this.jobsRoot) return null;
    const marker = readMarker(this.jobsRoot, this.spec);
    if (!marker) return null;
    if (marker.muted) this.setPending(true);
    try {
      restoreRegistry(this.reg, this.jobsRoot, this.spec);
      this.logger.warn(`[demo] рендер ${this.spec.label} прерван сбоем (${marker.at}): реестр игры возвращён`);
    } catch (error) {
      this.logger.warn(`[demo] реестр ${this.spec.label} после сбоя не возвращён (вернёт следующий рендер): ${error.message}`);
    }
    return marker;
  }

  setPending(value) {
    if (this.store) this.store.set(this.spec.unmutePending, value);
  }

  // Старт лаунчера: устаревшие папки рендера внутри игры (путь из библиотеки).
  cleanupStale() {
    if (this.active) return 0;
    const install = this.installPath();
    return install ? cleanupRenderDirs(this.spec.renderRoot(install), null) : 0;
  }

  steamCode() {
    let steam;
    try {
      steam = this.steamStatus();
    } catch (error) {
      steam = { ok: false, code: error.code };
    }
    if (steam && steam.ok) return null;
    return ["STEAM_NOT_INSTALLED", "STEAM_ELEVATED"].includes(steam && steam.code) ? steam.code : "STEAM_NOT_RUNNING";
  }

  async preflight(exe) {
    if (this.gameRunning(this.spec.gameId)) throw new DemoError("GAME_RUNNING");
    if (this.gameBusy()) throw new DemoError("GAME_BUSY");
    // Игра, запущенная не из лаунчера (Steam, ярлык): тот же exe. spec.otherRunning —
    // код для любого другого процесса с тем же образом (HL1: общий ключ реестра движка).
    let running = [];
    try {
      running = this.inspector.findByImageName(this.spec.image) || [];
    } catch (error) {
      running = [];
    }
    if (running.some((item) => item.executablePath && this.inspector.samePath(item.executablePath, exe))) throw new DemoError("GAME_RUNNING");
    if (this.spec.otherRunning && running.length) throw new DemoError(this.spec.otherRunning);
    const steam = this.steamCode();
    if (steam) throw new DemoError(steam);
  }

  // Снимок прошлого рендера, не возвращённый после сбоя, — сначала вернуть
  // его: новый снимок поверх потерял бы значения игрока.
  restorePrevious() {
    if (this.jobsRoot && readMarker(this.jobsRoot, this.spec)) {
      restoreRegistry(this.reg, this.jobsRoot, this.spec);
      this.logger.warn(`[demo] ${this.spec.label}: возвращён реестр прошлого прерванного рендера`);
    }
  }

  // Ошибка снимка (reg.exe не ответил, запрет политикой) — до любых изменений.
  // extra — что ещё вернуть после сбоя (HL1: журналы игрока).
  snapshot(active, extra) {
    if (!this.jobsRoot) return;
    snapshotRegistry(this.reg, this.jobsRoot, this.spec, Object.assign({ muted: active.muted }, extra));
    active.registry = true;
  }

  async render(request) {
    // Идёт фоновый сброс mute (секунды): экспорт его дожидается, а не отказывает.
    if (this.resetting) await this.resetting;
    if (this.active) throw new DemoError("JOB_BUSY");
    const pendingBefore = !!(this.store && this.store.get(this.spec.unmutePending) === true);
    // muted — Windows, возможно, помнит mute игры из-за нас (до события
    // audio-reset ok). Сначала — как было до рендера (флаг прошлого раза).
    this.active = { host: null, pump: null, modJobDir: null, registry: false, muted: pendingBefore, pendingBefore };
    try {
      return await this.renderJob(request, this.active);
    } catch (error) {
      // Отмена гасит игру и её процессы: их ошибки после неё — та же отмена.
      if (request.signal && request.signal.aborted) throw new DemoError("CANCELLED");
      throw error;
    } finally {
      this.active = null;
    }
  }

  // Можно ли сейчас снять mute, запомненный после рендера (флаг unmutePending):
  // null — можно, иначе причина. Не трогаем игру игрока и чужие игры на том же образе
  // (HL1: Steam HL1, CS 1.6 — у WON рядом с ними мьютекс), без Steam игра не стартует.
  muteResetBlocker() {
    if (!this.store || this.store.get(this.spec.unmutePending) !== true) return "no-flag";
    if (this.active || this.resetting) return "RENDER_ACTIVE";
    if (this.platform !== "win32" || !this.script) return "RENDER_NOT_READY";
    if (!this.installPath()) return "GAME_NOT_INSTALLED";
    if (this.gameRunning(this.spec.gameId) || this.gameBusy()) return "GAME_RUNNING";
    let running;
    try {
      running = this.inspector.findByImageName(this.spec.image) || [];
    } catch (error) {
      running = null;
    }
    if (!running || running.length) return "GAME_RUNNING";
    return this.steamCode();
  }

  // Беззвучный сброс без рендера: помощник (resetOnly) один раз запускает игру на скрытом
  // столе и снимает mute с её сессий. Реестр — снимок и возврат, как у рендера
  // (игра при старте читает и меняет свои настройки). -> true, если mute снят.
  async resetMute() {
    if (this.muteResetBlocker() !== null) return false;
    const active = { host: null, pump: null, modJobDir: null, registry: false, muted: true, pendingBefore: true };
    this.active = active;
    const run = (async () => {
      try {
        // Снимок реестра — в корне заданий; после чистки его может не быть.
        if (this.jobsRoot) jobs.prepareRoot(this.jobsRoot);
        this.restorePrevious();
        this.snapshot(active);
        const launch = this.resetLaunch(this.installPath(), active);
        const host = this.hostFactory({ script: this.script, env: this.env, logger: this.logger });
        active.host = host;
        this.watchAudio(host, active, true);
        await host.start({ resetOnly: true, exe: launch.exe, cwd: launch.cwd, commandLine: launch.commandLine,
          resetExe: launch.exe, resetCommandLine: launch.commandLine, mute: false });
        await this.stopHost(active, true);
      } catch (error) {
        this.logger.warn(`[demo] ${this.spec.label}: сброс mute без рендера не удался: ${error.message}`);
      } finally {
        if (active.host) await this.stopHost(active, true).catch(() => {});
        this.finish(active);
        this.active = null;
      }
      this.logger.info(`[demo] ${this.spec.label}: mute после рендера ${active.muted ? "не снят — попробуем позже" : "снят без нового экспорта"}`);
      return !active.muted;
    })();
    this.resetting = run;
    try {
      return await run;
    } finally {
      this.resetting = null;
    }
  }

  // Закрыть игру. reset — снять запомненный mute (конец рендера); без
  // Steam перезапуск-сброс не стартует — тогда mute остаётся под флагом.
  // Снятым mute считается только после подтверждённого сброса (audio-reset ok):
  // «сессию не видели» (отмена или падение до появления звука, а Windows
  // помнит mute прошлой пачки или прошлого сбоя) ничего не снимает.
  async stopHost(active, reset) {
    const host = active.host;
    await host.stop(reset && this.steamCode() === null);
    if (active.host === host) active.host = null;
    if (host.audioReset === true) this.markMuted(active, false);
    else if (host.pending === true) this.markMuted(active, true);
  }

  // События помощника про звук: глушение своих сессий и сброс mute.
  watchAudio(host, active, unmuteAll) {
    host.on("event", (message) => {
      if (message.event === "audio" && message.action === "muted" && (message.was === false || unmuteAll)) this.markMuted(active, true);
      else if (message.event === "audio-reset") {
        host.audioReset = message.ok === true;
        this.logger.info(`[demo] сброс mute после игры: ${message.ok ? "снят" : "не удался"}`);
      }
    });
  }

  // Конец задания (и выход лаунчера): mute, реестр, папка в игре, маркер.
  // Флаг снимается, только если mute этого рендера (и прошлого) снят подтверждённым сбросом.
  finish(active) {
    if (active.muted) this.setPending(true);
    else if (active.pendingBefore) this.setPending(false);
    if (active.registry && this.jobsRoot) {
      try {
        restoreRegistry(this.reg, this.jobsRoot, this.spec);
        active.registry = false;
      } catch (error) {
        this.logger.warn(`[demo] реестр ${this.spec.label} не возвращён (вернётся при следующем запуске): ${error.message}`);
      }
    }
    if (active.modJobDir) {
      try {
        removeWithinZone(path.dirname(active.modJobDir), active.modJobDir);
        try { fs.rmdirSync(path.dirname(active.modJobDir)); } catch (error) { /* не пустая */ }
        active.modJobDir = null;
      } catch (error) {
        this.logger.warn(`[demo] папка рендера в игре не удалена: ${error.message}`);
      }
    }
  }

  // Маркер повторяет состояние mute: после сбоя лаунчера его прочтёт recover().
  markMuted(active, muted) {
    active.muted = muted;
    if (!this.jobsRoot) return;
    const marker = readMarker(this.jobsRoot, this.spec);
    if (!marker || marker.muted === muted) return;
    try {
      writeMarker(this.jobsRoot, this.spec, Object.assign(marker, { muted }));
    } catch (error) {
      /* маркер — страховка, рендер не останавливаем */
    }
  }

  // Выход лаунчера посреди рендера (синхронно): помощник получает stop и
  // закрытый stdin — закроет игру (через своё задание), снимет mute
  // беззвучным перезапуском и без нас. Ждём его выхода: перезапуск тоже пишет
  // реестр. Игру по PID не трогаем: её закрывает задание помощника. Затем
  // реестр и папка; события помощника уже не прочесть — mute помечается к
  // снятию следующим рендером, если он вообще был.
  disposeSync() {
    const active = this.active;
    if (!active) return;
    const host = active.host;
    if (host) host.detach();
    if (active.pump && active.pump.process) try { active.pump.process.kill(); } catch (error) { /* вышел */ }
    // child.kill работает по дескриптору процесса — чужой процесс с тем же PID не задеть.
    if (host && host.helperPid && !waitDead(host.helperPid, 20000)) {
      host.kill();
      active.muted = true;
    }
    this.finish(active);
  }
}

module.exports = {
  FPS,
  GameRenderAdapter,
  regTool,
  readMarker,
  writeMarker,
  snapshotRegistry,
  restoreRegistry,
  cleanupRenderDirs,
  makeRenderDir,
  linkOrCopy,
  waitDead,
  maxHeightFor,
};
