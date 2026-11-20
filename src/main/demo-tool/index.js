"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const { DemoError, publicError } = require("./errors");
const planner = require("./planner");
const estimates = require("./estimate");
const ffmpeg = require("./ffmpeg");
const adapters = require("./adapters");
const jobs = require("./jobs");
const library = require("./library");
const media = require("./media");
const { GhostingAdapter, GAME_ID: GHOSTING_ID } = require("./ghosting");
const { Hl1Adapter, GAME_ID: HL1_ID } = require("./goldsrc");
const goldsrcRender = require("./goldsrc-render");
const { scanDemo } = require("./goldsrc-demo");
const sourceDemo = require("./source-demo");
const sourceRender = require("./source-render");
const { createPlayerFullscreen } = require("./player-fullscreen");
const toolRegistry = require("../game-delivery/tools");

// Инструмент «Демо» в главном процессе: список и приём демо, разбор, FFmpeg
// и bxt-rs как скачиваемые зависимости, пробы кодировщиков, план и задания экспорта
// и предпросмотра, протокол hlsr-media для плеера. IPC — bind() ниже.

const FFMPEG_TOOL = toolRegistry.FFMPEG;
const BXTRS_TOOL = toolRegistry.BXTRS;
// Подмена bxt_rs.dll (файл или папка с ним) — только dev и тесты, как HLSR_DEMO_FFMPEG.
const BXTRS_OVERRIDE = "HLSR_DEMO_BXTRS";
const MAX_TIMELINE_BYTES = 5 * 1024 * 1024;
// Вкладка «Отрендеренные»: видео в папке вывода, у которых есть свой timeline.json.
const MAX_RENDERED = 500;
const MAX_TIMELINE_MARKERS = 5000;
const SAVED_KEYS = ["preset", "height", "fps", "audio", "keyhud", "integrity", "encoder", "speed", "hud", "ownConfig", "ownHud"];
const MUTE_RESET_INTERVAL_MS = 60000;
const silentLogger = { info() {}, warn() {}, error() {} };

// Переключатели «Дополнительных настроек», которые умеет адаптер.
function viewCaps(caps) {
  const out = {};
  for (const key of planner.VIEW_KEYS) out[key] = !!(caps && caps[key]);
  return out;
}

// «github.com/GyanD/codexffmpeg» и релиз — у источника на GitHub, иначе только сайт.
function originOf(provider) {
  try {
    const url = new URL(provider.url);
    const release = /^\/([^/]+\/[^/]+)\/releases\/download\/([^/]+)\//.exec(url.pathname);
    if (url.hostname === "github.com" && release) return { site: `github.com/${release[1]}`, release: decodeURIComponent(release[2]) };
    return { site: url.hostname, release: null };
  } catch (error) {
    return null;
  }
}

// timeline.json задания: размер, версия, массив меток. Файл — только настоящий (не ссылка).
function readTimelineFile(file) {
  const stat = fs.lstatSync(file);
  if (!stat.isFile() || stat.size > MAX_TIMELINE_BYTES) throw new DemoError("TIMELINE_INVALID");
  const timeline = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!timeline || timeline.version !== 1 || !Array.isArray(timeline.markers) || !timeline.video || typeof timeline.video !== "object")
    throw new DemoError("TIMELINE_INVALID");
  return timeline;
}

// timeline.json из папки вывода — данные с диска: в окно только известные поля и метки.
function publicTimeline(timeline) {
  const num = (value) => (typeof value === "number" && Number.isFinite(value) ? value : null);
  const text = (value) => (typeof value === "string" ? value.slice(0, 64) : null);
  const video = timeline.video;
  const range = video.range && typeof video.range === "object" ? { from: num(video.range.from), to: num(video.range.to) } : null;
  const source = timeline.source && typeof timeline.source === "object" ? timeline.source : {};
  return {
    video: {
      duration: num(video.duration),
      fps: num(video.fps),
      width: num(video.width),
      height: num(video.height),
      speed: num(video.speed) || 1,
      range: range && range.from !== null && range.to !== null ? range : null,
    },
    source: { game: text(source.game), map: text(source.map), files: Array.isArray(source.files) ? source.files.length : 0 },
    markers: timeline.markers
      .filter((marker) => marker && planner.MARKER_TYPES.has(marker.type) && Number.isFinite(marker.t) && marker.t >= 0)
      .slice(0, MAX_TIMELINE_MARKERS)
      .map((marker) => ({
        t: marker.t,
        type: marker.type,
        label: String(marker.label === undefined || marker.label === null ? "" : marker.label).slice(0, 64),
        sourceTime: Number.isFinite(marker.sourceTime) ? marker.sourceTime : null,
      })),
  };
}

class DemoTool {
  constructor(options) {
    const opts = options || {};
    this.store = opts.store;
    this.games = opts.games || (() => []);
    this.tools = opts.tools || (() => null);
    this.emit = opts.emit || (() => {});
    // Достижения профиля (OnlineProfileService.achieve): «Монтажёр» за готовый экспорт.
    this.achieve = opts.achieve || (() => {});
    this.dialog = opts.dialog || null;
    this.shell = opts.shell || null;
    this.videosDir = opts.videosDir || path.join(os.homedir(), "Videos");
    this.platform = opts.platform || process.platform;
    this.env = opts.env || process.env;
    this.production = opts.production === undefined ? ffmpeg.PRODUCTION_BUILD : opts.production;
    this.logger = opts.logger || silentLogger;
    // Корень заданий в данных пользователя (jobs.js); без него задания недоступны.
    this.jobsRoot = opts.jobsRoot || null;
    this.media = opts.media || new media.MediaRegistry();
    this.adapterList = opts.adapters || null;
    // Только тесты: искусственно маленький лимит Discord.
    this.limitBytes = opts.limitBytes || undefined;
    this.job = null;
    this.lastJob = null;
    this.preview = null;
    this.results = new Map();
    this.install = null;
    this.versions = new Map();
    this.probe = null;
    // Разбор демок HL1 для меток (путь|размер|длительность -> Promise).
    this.scans = new Map();
    // Пробы и проверки версии гасятся при выходе (dispose).
    this.processes = new AbortController();
    this.fullscreen = createPlayerFullscreen((value) => this.emit("demo-player-fullscreen-changed", value));
    // Рендер игр: по одному адаптеру на лаунчер (состояние рендера, маркер возврата реестра).
    const gameOptions = (gameId) => ({
      platform: this.platform,
      env: this.env,
      logger: this.logger,
      jobsRoot: this.jobsRoot,
      store: this.store,
      resourcesPath: opts.resourcesPath,
      installPath: () => {
        const found = this.contexts().find((context) => context.gameId === gameId);
        return found ? found.installPath : null;
      },
      gameRunning: opts.gameRunning,
      gameBusy: opts.gameBusy,
      steamStatus: opts.steamStatus,
      screenSize: opts.screenSize,
    });
    this.ghosting = opts.ghostingAdapter || new GhostingAdapter(gameOptions(GHOSTING_ID));
    this.hl1 = opts.hl1Adapter || new Hl1Adapter(Object.assign(gameOptions(HL1_ID), { bxtrs: () => this.bxtrsLocated() }));
    this.renderers = [this.ghosting, this.hl1];
    this.muteTimer = null;
    this.muteResetting = null;
    this.muteGaveUp = new Set();
    // Маркер прошлого рендера, прерванного сбоем, — раньше чистки старых папок.
    for (const renderer of this.renderers)
      try {
        renderer.recover();
      } catch (error) {
        this.logger.warn(`[demo] восстановление после рендера не удалось: ${error.message}`);
      }
    try {
      // При старте заданий ещё нет: всё, что осталось в корне после сбоя, — мусор (video.mkv — гигабайты).
      const removed = jobs.cleanupStale(this.jobsRoot, Date.now(), 0);
      if (removed) this.logger.info(`[demo] удалены старые папки заданий: ${removed}`);
    } catch (error) {
      this.logger.warn(`[demo] старые папки заданий не убраны: ${error.message}`);
    }
  }

  // ---------- список и приём ----------

  storeGet(key) {
    try {
      return this.store ? this.store.get(key) : undefined;
    } catch (error) {
      return undefined;
    }
  }

  storeSet(key, value) {
    if (this.store) this.store.set(key, value);
  }

  manual() {
    const stored = this.storeGet("manual");
    const read = library.manualEntries(stored);
    // Удалённые файлы отбрасываются и из Store; записи с недоступного диска
    // (USB, сеть) остаются и показываются с DEMO_NOT_FOUND.
    if (Array.isArray(stored) && JSON.stringify(read.cleaned) !== JSON.stringify(stored)) this.storeSet("manual", read.cleaned);
    return read;
  }

  contexts() {
    try {
      return this.games() || [];
    } catch (error) {
      this.logger.warn(`[demo] игры не прочитаны: ${error.message}`);
      return [];
    }
  }

  entries() {
    return library.gameEntries(this.contexts()).concat(this.manual().entries);
  }

  // Идёт рендер этой игры: её запуск из лаунчера отказывает (RENDER_ACTIVE).
  renderActive(gameId) {
    const renderer = { [GHOSTING_ID]: this.ghosting, [HL1_ID]: this.hl1 }[gameId];
    return !!(renderer && renderer.isActive());
  }

  // Mute игры, запомненный Windows после рендера (флаг сброса), не ждёт следующего
  // экспорта: фоновый беззвучный сброс при старте и раз в минуту, пока флаг стоит и
  // ничто не мешает (muteResetBlocker), — по одной игре, без рендера рядом.
  startMuteResets() {
    if (this.muteTimer) return;
    const tick = () => {
      this.resetPendingMutes().catch((error) => this.logger.warn(`[demo] фоновый сброс mute: ${error.message}`));
    };
    tick();
    this.muteTimer = setInterval(tick, MUTE_RESET_INTERVAL_MS);
    if (this.muteTimer.unref) this.muteTimer.unref();
  }

  resetPendingMutes() {
    if (!this.muteResetting)
      this.muteResetting = (async () => {
        for (const renderer of this.renderers) {
          // Неудачный фоновый сброс (нет звуковых сессий и т. п.) до перезапуска лаунчера не повторяется:
          // игра не стартует на скрытом столе каждую минуту. Перед «Играть» — пробуем всё равно.
          if (this.job || this.renderers.some((other) => other.isActive()) || this.muteGaveUp.has(renderer)) continue;
          if (renderer.muteResetBlocker() !== null) continue;
          if (!(await renderer.resetMute())) this.muteGaveUp.add(renderer);
        }
      })().finally(() => {
        this.muteResetting = null;
      });
    return this.muteResetting;
  }

  // Перед запуском игры из лаунчера: флаг стоит — короткий сброс, иначе игрок остался бы без звука.
  async resetMuteBeforeLaunch(gameId) {
    const renderer = { [GHOSTING_ID]: this.ghosting, [HL1_ID]: this.hl1 }[gameId];
    if (!renderer) return false;
    if (this.muteResetting) await this.muteResetting.catch(() => {});
    if (renderer.muteResetBlocker() !== null) return false;
    try {
      return await renderer.resetMute();
    } catch (error) {
      this.logger.warn(`[demo] сброс mute перед запуском: ${error.message}`);
      return false;
    }
  }

  // Старт лаунчера: папки рендера, оставшиеся в играх после сбоя.
  cleanupRenderDirs() {
    for (const renderer of this.renderers)
      try {
        const removed = renderer.cleanupStale();
        if (removed) this.logger.info(`[demo] удалены папки рендера в игре: ${removed}`);
      } catch (error) {
        this.logger.warn(`[demo] папки рендера в игре не убраны: ${error.message}`);
      }
  }

  resolve(id) {
    const found = typeof id === "string" ? this.entries().find((item) => item.id === id) : null;
    if (!found) throw new DemoError("DEMO_NOT_FOUND");
    return library.describe(found);
  }

  list() {
    return { status: "ok", items: this.entries().map((item) => library.summary(library.describe(item))) };
  }

  addPaths(paths) {
    if (!Array.isArray(paths) || paths.length > 512) throw new DemoError("PATH_INVALID");
    const result = library.addPaths(this.manual().cleaned, paths, this.platform);
    if (result.added.length) this.storeSet("manual", result.stored);
    for (const rejected of result.rejected) this.logger.info(`[demo] не добавлен ${rejected.name}: ${rejected.code}`);
    return {
      status: "ok",
      added: library.manualEntries(result.added).entries.map((item) => item.id),
      rejected: result.rejected,
      items: this.list().items,
    };
  }

  async pickFiles() {
    if (!this.dialog) throw new DemoError("DIALOG_UNAVAILABLE");
    const picked = await this.dialog.showOpenDialog({
      properties: ["openFile", "multiSelections"],
      filters: [{ name: "Demo", extensions: ["dem", "ghplaylist"] }],
    });
    if (picked.canceled || !picked.filePaths || !picked.filePaths.length) return { status: "cancelled" };
    return this.addPaths(picked.filePaths);
  }

  manualIndex(id) {
    const { entries, cleaned } = this.manual();
    const index = entries.findIndex((item) => item.id === id);
    if (index < 0) throw new DemoError("DEMO_NOT_FOUND");
    return { index, cleaned, item: entries[index] };
  }

  remove(id) {
    const { index, cleaned } = this.manualIndex(id);
    cleaned.splice(index, 1);
    this.storeSet("manual", cleaned);
    return { status: "ok", items: this.list().items };
  }

  // Порядок сегментов набора HL1: order — перестановка индексов 0..n-1.
  async reorder(id, order) {
    const { index, cleaned, item } = this.manualIndex(id);
    const files = item.files;
    const valid = Array.isArray(order) && order.length === files.length &&
      order.every((value) => Number.isInteger(value) && value >= 0 && value < files.length) &&
      new Set(order).size === files.length;
    if (item.kind !== "segments" || !valid) throw new DemoError("ORDER_INVALID");
    cleaned[index] = { kind: "segments", paths: order.map((value) => files[value]) };
    this.storeSet("manual", cleaned);
    return this.inspect(id);
  }

  async inspect(id) {
    const item = this.resolve(id);
    const verdict = this.verdict(item);
    const segments = item.segments.map((segment) => {
      const out = Object.assign({}, segment);
      delete out.path;
      delete out.sha256;
      return out;
    });
    return {
      status: "ok",
      item: library.summary(item),
      segments,
      markers: await this.sourceMarkers(item),
      // view — что из «Дополнительных настроек» умеет адаптер (keyhud, hud, ownConfig, ownHud).
      render: { ok: verdict.ok, code: verdict.code, view: viewCaps(verdict.adapter ? verdict.adapter.capabilities : null) },
    };
  }

  // Метки до рендера. HL1: те же, что даст рендер (goldsrc-render.timelineOf):
  // стыки сегментов и смены карт внутри демок — из самих демок (разбор кэшируется).
  // Source: метки описания (с главами) и финиш из самих демок (source-demo.js).
  async sourceMarkers(item) {
    if (item.engine === "source") return item.markers.concat(await this.finishMarkers(item)).sort((a, b) => a.t - b.t);
    if (item.engine !== "goldsrc" || !item.supported) return item.markers;
    const plan = goldsrcRender.planSegments(item.segments, null);
    const scans = [];
    for (const entry of plan.picked) scans.push(await this.scanCached(entry.segment));
    return goldsrcRender.timelineOf(plan.picked, scans, null);
  }

  // Финиш рана Source (запись мода в демке) — во времени демо, по тем же сегментам, что метки описания.
  async finishMarkers(item) {
    if (item.engine !== "source" || item.incomplete || item.segments.some((segment) => !segment.ok)) return [];
    const picked = sourceRender.planSegments(item.segments, null, 60).picked;
    const scans = [];
    for (const entry of picked) {
      const key = `source|${entry.segment.path}|${entry.segment.size}|${entry.segment.duration}`;
      if (!this.scans.has(key)) {
        if (this.scans.size >= 256) this.scans.clear();
        this.scans.set(key, sourceDemo.scanFile(entry.segment.path, { signal: this.processes.signal }).catch(() => null));
      }
      scans.push(await this.scans.get(key));
    }
    return sourceDemo.finishMarkers(picked, scans, { incomplete: item.incomplete, runTime: item.runTime });
  }

  // Метки рана, которых нет у адаптера рендера: главы HL2 и финиш (время демо).
  async runMarkers(item) {
    if (item.engine !== "source") return [];
    return sourceDemo.chapterMarkers(sourceRender.planSegments(item.segments, null, 60).picked).concat(await this.finishMarkers(item));
  }

  async scanCached(segment) {
    const key = `${segment.path}|${segment.size}|${segment.duration}`;
    if (!this.scans.has(key)) {
      if (this.scans.size >= 256) this.scans.clear();
      this.scans.set(key, scanDemo(segment.path, { signal: this.processes.signal }).catch(() => null));
    }
    return this.scans.get(key);
  }

  // ---------- FFmpeg ----------

  toolDir() {
    const tools = this.tools();
    if (!tools) return null;
    try {
      return tools.readInstalled(FFMPEG_TOOL) ? tools.installPathOf(FFMPEG_TOOL) : null;
    } catch (error) {
      return null;
    }
  }

  located() {
    return ffmpeg.locate({
      platform: this.platform,
      env: this.env,
      production: this.production,
      toolDir: this.platform === "win32" ? this.toolDir() : null,
    });
  }

  versionOf(exe) {
    if (!this.versions.has(exe)) {
      const pending = ffmpeg.version(exe, { signal: this.processes.signal });
      pending.catch(() => this.versions.delete(exe));
      this.versions.set(exe, pending);
    }
    return this.versions.get(exe);
  }

  async ffmpegStatus() {
    const manifest = toolRegistry.DEPENDENCIES[FFMPEG_TOOL];
    const installing = this.installing(FFMPEG_TOOL);
    const base = {
      state: "not-installed",
      code: null,
      source: null,
      version: null,
      license: null,
      pinnedVersion: manifest.version,
      build: manifest.build,
      pinnedLicense: manifest.license,
      installable: this.platform === "win32",
      sizeBytes: manifest.archive.size,
      // Откуда качает ToolService — источники манифеста по порядку (окно перед входом в «Демо»).
      origins: manifest.archive.providers.map(originOf),
      percent: installing ? installing.percent : 0,
    };
    if (installing) return Object.assign(base, { state: "installing" });
    const where = this.located();
    if (!where) return base;
    let info;
    try {
      info = await this.versionOf(where.ffmpeg);
    } catch (error) {
      return Object.assign(base, where.source === "path"
        ? { code: "FFMPEG_NOT_IN_PATH" }
        : { state: "error", code: "FFMPEG_BROKEN", source: where.source });
    }
    if (where.source === "tool" && !info.pinned)
      return Object.assign(base, { state: "error", code: "FFMPEG_VERSION_MISMATCH", source: where.source, version: info.version });
    return Object.assign(base, { state: "ready", source: where.source, version: info.version, license: info.license });
  }

  async requireFfmpeg() {
    const status = await this.ffmpegStatus();
    if (status.state !== "ready")
      throw new DemoError(status.code || (status.state === "installing" ? "FFMPEG_INSTALLING" : "FFMPEG_MISSING"));
    return this.located();
  }

  // Идущая установка этого инструмента ({ toolId, percent }) или null.
  installing(toolId) {
    return this.install && this.install.toolId === toolId ? this.install : null;
  }

  // Скачать зависимость тем же ToolService (закреплённый sha256); прогресс — событием channel.
  // null — установлено, { status: "cancelled" } — отменено.
  async installTool(toolId, channel, unsupported) {
    if (this.platform !== "win32") throw new DemoError(unsupported);
    const tools = this.tools();
    if (!tools) throw new DemoError("TOOLS_UNAVAILABLE");
    if (this.install) throw new DemoError("ALREADY_RUNNING");
    // Посреди экспорта ffmpeg.exe и bxt_rs.dll заняты: переустановка сломала бы задание.
    if (this.job) throw new DemoError("JOB_BUSY");
    this.install = { toolId, percent: 0 };
    // Новый FFmpeg — пробы кодировщиков и версии заново (после проверок: посреди экспорта не сбрасываем).
    if (toolId === FFMPEG_TOOL) {
      this.versions.clear();
      this.probe = null;
    }
    try {
      await tools.install(toolId, (percent) => {
        if (percent === this.install.percent) return;
        this.install.percent = percent;
        this.emit(channel, { percent });
      });
    } catch (error) {
      if (error.code === "CANCELLED") return { status: "cancelled" };
      throw error;
    } finally {
      this.install = null;
    }
    return null;
  }

  async installFfmpeg() {
    const cancelled = await this.installTool(FFMPEG_TOOL, "demo-ffmpeg-progress", "FFMPEG_INSTALL_UNSUPPORTED");
    if (cancelled) return cancelled;
    const tools = this.tools();
    // Закреплённая сборка: первая строка ffmpeg -version содержит 9.0.2.
    const status = await this.ffmpegStatus();
    if (status.state !== "ready") {
      this.logger.warn(`[demo] FFmpeg после установки не прошёл проверку: ${status.code}`);
      try {
        tools.remove(FFMPEG_TOOL);
      } catch (error) {
        this.logger.warn(`[demo] FFmpeg не удалён: ${error.message}`);
      }
      throw new DemoError(status.code || "FFMPEG_BROKEN");
    }
    this.logger.info(`[demo] FFmpeg ${status.version} установлен`);
    return { status: "ok", ffmpeg: status };
  }

  cancelFfmpegInstall() {
    const tools = this.tools();
    return { status: "ok", cancelled: !!(tools && tools.cancel(FFMPEG_TOOL)) };
  }

  // ---------- bxt-rs (рендер HL1) ----------

  // bxt_rs.dll: подмена HLSR_DEMO_BXTRS (dev и тесты) или установленный
  // инструмент закреплённой версии; sha256 сверяет адаптер перед рендером.
  bxtrsLocated() {
    const override = !this.production && this.env ? this.env[BXTRS_OVERRIDE] : null;
    if (override) {
      try {
        const file = fs.statSync(override).isDirectory() ? path.join(override, "bxt_rs.dll") : override;
        return { path: file, sha256: null, source: "env" };
      } catch (error) {
        return null;
      }
    }
    const tools = this.platform === "win32" ? this.tools() : null;
    if (!tools) return null;
    const manifest = toolRegistry.DEPENDENCIES[BXTRS_TOOL];
    try {
      const installed = tools.readInstalled(BXTRS_TOOL);
      const file = installed && installed.version === manifest.version ? path.join(tools.installPathOf(BXTRS_TOOL), manifest.executable) : null;
      return file && fs.existsSync(file) ? { path: file, sha256: manifest.files[manifest.executable], source: "tool" } : null;
    } catch (error) {
      return null;
    }
  }

  bxtrsStatus() {
    const manifest = toolRegistry.DEPENDENCIES[BXTRS_TOOL];
    const installing = this.installing(BXTRS_TOOL);
    const where = installing ? null : this.bxtrsLocated();
    return {
      state: installing ? "installing" : where ? "ready" : "not-installed",
      source: where ? where.source : null,
      version: manifest.version,
      license: manifest.license,
      installable: this.platform === "win32",
      // DLL и рядом с ней текст лицензии и уведомления крейтов (manifest.extras).
      sizeBytes: (manifest.extras || []).reduce((sum, extra) => sum + extra.size, manifest.archive.size),
      percent: installing ? installing.percent : 0,
      // Ключи ссылок на исходник и лицензию (open-external-link, src/shared/links.js).
      sourceLink: "bxtrsSource",
      licenseLink: "bxtrsLicense",
    };
  }

  async installBxtrs() {
    const cancelled = await this.installTool(BXTRS_TOOL, "demo-bxtrs-progress", "BXTRS_INSTALL_UNSUPPORTED");
    if (cancelled) return cancelled;
    const status = this.bxtrsStatus();
    if (status.state !== "ready") throw new DemoError("BXTRS_BROKEN");
    this.logger.info(`[demo] bxt-rs ${status.version} установлен`);
    return { status: "ok", bxtrs: status };
  }

  cancelBxtrsInstall() {
    const tools = this.tools();
    return { status: "ok", cancelled: !!(tools && tools.cancel(BXTRS_TOOL)) };
  }

  encoderProbe(exe) {
    if (!this.probe || this.probe.exe !== exe)
      this.probe = new ffmpeg.EncoderProbe({ exe, logger: this.logger, signal: this.processes.signal });
    return this.probe;
  }

  async encoders() {
    const where = await this.requireFfmpeg();
    return Object.assign({ status: "ok" }, await this.encoderProbe(where.ffmpeg).summary());
  }

  // FPS, которые кодировщик тянет на этом разрешении: проба на максимальном
  // FPS, при неудаче — на следующем ниже.
  async supportedFps(probe, encoder, resolution, fpsList) {
    for (const fps of fpsList.slice().sort((a, b) => b - a))
      if (await probe.works(encoder, resolution.width, resolution.height, fps)) return fpsList.filter((value) => value <= fps);
    return [];
  }

  async combos(probe, encoder, caps) {
    const fpsList = planner.FPS.filter((fps) => (caps.fps || []).includes(fps));
    const allowed = new Map();
    for (const resolution of planner.RESOLUTIONS)
      if (resolution.height <= caps.maxHeight)
        allowed.set(resolution.height, await this.supportedFps(probe, encoder, resolution, fpsList));
    return planner.allowedCombos(caps, (height, fps) => (allowed.get(height) || []).includes(fps));
  }

  resolveEncoder(mode, summary) {
    const encoder = mode === "gpu" ? summary.gpu : mode === "cpu" ? summary.cpu : summary.auto;
    if (!encoder) throw new DemoError(mode === "gpu" ? "ENCODER_GPU_UNAVAILABLE" : "ENCODER_UNAVAILABLE");
    return encoder;
  }

  // ---------- адаптеры, план ----------

  adapterSet(ffmpegPath) {
    return this.adapterList || adapters.defaultAdapters({ env: this.env, production: this.production, ffmpegPath, ghosting: this.ghosting, goldsrc: this.hl1 });
  }

  verdict(item, ffmpegPath) {
    if (!item.supported) return { adapter: null, ok: false, code: item.reason || "DEMO_UNSUPPORTED" };
    return adapters.pickAdapter(item, this.adapterSet(ffmpegPath || (this.located() || {}).ffmpeg));
  }

  savedSettings() {
    const saved = this.storeGet("export");
    const out = Object.assign({}, planner.DEFAULTS);
    for (const key of SAVED_KEYS) if (saved && saved[key] !== undefined) out[key] = saved[key];
    try {
      planner.normalizeSettings(out, 1);
      return out;
    } catch (error) {
      return Object.assign({}, planner.DEFAULTS);
    }
  }

  async options(id) {
    const item = this.resolve(id);
    const verdict = this.verdict(item);
    const caps = verdict.adapter ? verdict.adapter.capabilities : adapters.PENDING_CAPS;
    const status = await this.ffmpegStatus();
    let modes = null;
    if (status.state === "ready") {
      const probe = this.encoderProbe(this.located().ffmpeg);
      const summary = await probe.summary();
      modes = {};
      for (const mode of planner.ENCODER_MODES) {
        const encoder = mode === "gpu" ? summary.gpu : mode === "cpu" ? summary.cpu : summary.auto;
        modes[mode] = encoder ? { encoder, combos: await this.combos(probe, encoder, caps) } : null;
      }
    }
    return {
      status: "ok",
      id,
      duration: item.duration,
      render: { ok: verdict.ok, code: verdict.code },
      ffmpeg: { state: status.state, code: status.code },
      capabilities: Object.assign({ maxHeight: caps.maxHeight, fps: caps.fps.slice(), audio: !!caps.audio, integrity: !!caps.integrity }, viewCaps(caps)),
      keyhud: caps.keyhud ? { available: true, code: null } : { available: false, code: "KEYHUD_NOT_AVAILABLE" },
      resolutions: planner.RESOLUTIONS.filter((item) => item.height <= caps.maxHeight),
      speed: planner.SPEED,
      presets: ["quality", "discord"],
      discordLimitBytes: this.limitBytes || planner.DISCORD.limitBytes,
      modes,
      defaults: this.savedSettings(),
    };
  }

  // Общая подготовка плана: проверка возможностей адаптера и кодировщика.
  async prepare(item, raw, options) {
    const settings = planner.normalizeSettings(raw, item.duration, options);
    const where = await this.requireFfmpeg();
    const verdict = this.verdict(item, where.ffmpeg);
    const caps = verdict.adapter ? verdict.adapter.capabilities : adapters.PENDING_CAPS;
    const warnings = [];
    if (settings.keyhud && !caps.keyhud) {
      settings.keyhud = false;
      warnings.push("KEYHUD_UNAVAILABLE");
    }
    // Надпись проверки данных — только у адаптеров, которые её умеют (Ghosting); у остальных молча выключена.
    if (settings.integrity && !caps.integrity) settings.integrity = false;
    // Выключить HUD, взять стандартный конфиг или HUD — только где игра это умеет; иначе как у игрока.
    for (const key of ["hud", "ownConfig", "ownHud"]) if (!settings[key] && !caps[key]) settings[key] = true;
    if (settings.audio && !caps.audio) {
      settings.audio = false;
      warnings.push("AUDIO_UNAVAILABLE");
    }
    if (settings.height > caps.maxHeight || !caps.fps.includes(settings.fps)) throw new DemoError("COMBO_UNSUPPORTED");
    const probe = this.encoderProbe(where.ffmpeg);
    const encoder = this.resolveEncoder(settings.encoder, await probe.summary());
    const resolution = planner.RESOLUTIONS.find((value) => value.height === settings.height);
    if (!(await this.supportedFps(probe, encoder, resolution, caps.fps)).includes(settings.fps))
      throw new DemoError("COMBO_UNSUPPORTED", null, { encoder });
    const outputDuration = planner.clipDuration({ from: settings.range.from, to: settings.range.to, speed: settings.speed });
    const plan = planner.planExport(settings, { encoder, outputDuration, limitBytes: this.limitBytes, warnings });
    return { settings, plan, verdict, where };
  }

  async plan(id, raw) {
    const item = this.resolve(id);
    const prepared = await this.prepare(item, raw);
    return { status: "ok", plan: prepared.plan, render: { ok: prepared.verdict.ok, code: prepared.verdict.code } };
  }

  // ---------- оценка времени (estimate.js) ----------

  // Скорости прошлых заданий: { render: { "<профиль>@<высота>": { rate, frames } }, encode: {...} }.
  learnedSpeed() {
    const saved = this.storeGet("speed");
    return saved && typeof saved === "object" ? saved : {};
  }

  // Кодировщик, которым пойдёт задание: из проб FFmpeg; без FFmpeg — процессор (оценка с запасом).
  async encoderGuess(mode) {
    try {
      const status = await this.ffmpegStatus();
      if (status.state !== "ready") return planner.CPU_ENCODER;
      return this.resolveEncoder(mode, await this.encoderProbe(this.located().ffmpeg).summary());
    } catch (error) {
      return planner.CPU_ENCODER;
    }
  }

  // Оценка одного задания: raw — настройки экспорта или { preset: "preview", range } предпросмотра.
  async estimateFor(item, raw) {
    const preview = !!(raw && raw.preset === "preview");
    const input = preview ? Object.assign({}, planner.PREVIEW, { encoder: "auto", range: raw.range || null }) : raw;
    const settings = planner.normalizeSettings(input, item.duration, { allowPreview: preview });
    const verdict = this.verdict(item);
    const adapter = verdict.adapter;
    const profile = adapter && typeof adapter.estimateProfile === "function"
      ? adapter.estimateProfile(item, settings.range, settings.fps)
      : { key: "other", launches: 1, segments: item.segments.length };
    const seconds = settings.range.to - settings.range.from;
    const encoder = await this.encoderGuess(settings.encoder);
    const result = estimates.estimate({
      profile: profile.key,
      launches: profile.launches,
      segments: profile.segments,
      seconds,
      fps: settings.fps,
      width: settings.width,
      height: settings.height,
      outputSeconds: seconds / settings.speed,
      preset: settings.preset,
      gpu: encoder !== planner.CPU_ENCODER,
    }, this.learnedSpeed());
    return Object.assign(result, { range: Math.round(seconds * 10) / 10, segments: profile.segments || 1 });
  }

  // Окно «Демо» спрашивает несколько вариантов сразу (предпросмотр, «Лучшее качество», Discord).
  async estimate(id, list) {
    const item = this.resolve(id);
    if (!Array.isArray(list) || !list.length || list.length > 8) throw new DemoError("SETTINGS_INVALID", null, { field: "estimate" });
    const items = [];
    for (const raw of list) items.push(await this.estimateFor(item, raw));
    return { status: "ok", items, threshold: estimates.PREVIEW_PROMPT_SECONDS };
  }

  // ---------- задания ----------

  outputDir() {
    const saved = this.storeGet("outputDir");
    return typeof saved === "string" && saved && path.isAbsolute(saved) ? saved : path.join(this.videosDir, "HLSR");
  }

  settings() {
    return {
      status: "ok",
      settings: this.savedSettings(),
      outputDir: this.outputDir(),
      defaultOutputDir: path.join(this.videosDir, "HLSR"),
      preview: this.previewView(),
    };
  }

  async pickOutputDir() {
    if (!this.dialog) throw new DemoError("DIALOG_UNAVAILABLE");
    const picked = await this.dialog.showOpenDialog({
      properties: ["openDirectory", "createDirectory"],
      defaultPath: this.outputDir(),
    });
    if (picked.canceled || !picked.filePaths || !picked.filePaths.length) return { status: "cancelled" };
    this.storeSet("outputDir", path.resolve(picked.filePaths[0]));
    return { status: "ok", outputDir: this.outputDir() };
  }

  startExport(id, raw) {
    return this.start("export", id, raw);
  }

  startPreview(id, raw) {
    // Предпросмотр: 720p30, быстрое кодирование, со звуком; из окна — отрезок и
    // KeyHUD (по умолчанию включён, выбор запоминается), из настроек экспорта —
    // надпись проверки данных.
    const range = raw && raw.range ? raw.range : null;
    const view = this.previewView();
    let chosen = false;
    for (const key of planner.VIEW_KEYS)
      if (raw && typeof raw[key] === "boolean") {
        view[key] = raw[key];
        chosen = true;
      }
    if (chosen) this.storeSet("preview", view);
    return this.start("preview", id, Object.assign({}, planner.PREVIEW, view, { encoder: "auto", range, integrity: this.savedSettings().integrity }));
  }

  // Вид предпросмотра (KeyHUD, HUD, конфиг и HUD игрока): по умолчанию всё включено, выбор запоминается.
  previewView() {
    const saved = this.storeGet("preview") || {};
    const view = {};
    for (const key of planner.VIEW_KEYS) view[key] = typeof saved[key] === "boolean" ? saved[key] : true;
    return view;
  }

  async start(kind, id, raw) {
    if (this.job) throw new DemoError("JOB_BUSY");
    if (this.installing(FFMPEG_TOOL)) throw new DemoError("FFMPEG_INSTALLING");
    const controller = new AbortController();
    this.job = { id: null, kind, controller };
    try {
      jobs.prepareRoot(this.jobsRoot);
      const item = this.resolve(id);
      const prepared = await this.prepare(item, raw, { allowPreview: kind === "preview" });
      if (!prepared.verdict.ok) throw new DemoError(prepared.verdict.code);
      if (controller.signal.aborted) throw new DemoError("CANCELLED");
      const outputDir = kind === "export" ? this.outputDir() : null;
      if (kind === "export") {
        try {
          fs.mkdirSync(outputDir, { recursive: true });
        } catch (error) {
          throw new DemoError("OUTPUT_DIR_UNAVAILABLE");
        }
        const remembered = {};
        for (const key of SAVED_KEYS) remembered[key] = prepared.settings[key];
        this.storeSet("export", remembered);
      } else this.dropPreview();

      // Главы и финиш Source — из карт и самих демок, рендер их не знает (метки поверх меток адаптера).
      const extraMarkers = await this.runMarkers(item);
      if (controller.signal.aborted) throw new DemoError("CANCELLED");

      const jobId = crypto.randomBytes(8).toString("hex");
      this.job.id = jobId;
      this.logger.info(`[demo] задание ${jobId} (${kind}): ${item.kind} ${item.name}, ${prepared.plan.width}x${prepared.plan.height}@${prepared.plan.fps}, ${prepared.plan.encoder}, ${prepared.plan.preset}`);
      const emitProgress = (progress) => this.emit("demo-job-progress", Object.assign({ jobId, kind }, progress));
      const pending = jobs.runJob({
        id: jobId,
        kind,
        item,
        adapter: prepared.verdict.adapter,
        settings: prepared.settings,
        plan: prepared.plan,
        root: this.jobsRoot,
        outputDir,
        extraMarkers,
        ffmpegPath: prepared.where.ffmpeg,
        ffprobePath: prepared.where.ffprobe,
        signal: controller.signal,
        onProgress: emitProgress,
        logger: this.logger,
        platform: this.platform,
      }).then(
        (result) => {
          this.job = null;
          return this.finished(jobId, kind, result);
        },
        (error) => {
          this.job = null;
          return this.failed(jobId, kind, error);
        }
      // finished() бросил до demo-job-done (например, media.register): задание всё равно
      // заканчивается ошибкой — интерфейс и Discord («Рендерит демку») не ждут вечно.
      ).catch((error) => this.failed(jobId, kind, error));
      this.lastJob = pending;
      return { status: "ok", jobId, plan: prepared.plan };
    } catch (error) {
      this.job = null;
      throw error;
    }
  }

  finished(jobId, kind, result) {
    if (result.stats && result.stats.profile)
      try {
        this.storeSet("speed", estimates.learn(this.learnedSpeed(), result.stats));
      } catch (error) {
        this.logger.warn(`[demo] скорость задания не сохранена: ${error.message}`);
      }
    const registered = this.media.register(result.file);
    this.results.set(jobId, Object.assign({}, result, { token: registered.token, url: registered.url }));
    if (kind === "preview") this.preview = { jobId, dir: result.dir, token: registered.token };
    const payload = {
      jobId,
      kind,
      result: {
        id: jobId,
        kind,
        file: kind === "export" ? result.file : null,
        fileName: kind === "export" ? path.basename(result.file) : null,
        timelineFile: result.timelineFile ? path.basename(result.timelineFile) : null,
        size: result.size,
        attempts: result.attempts,
        encoder: result.encoder,
        warnings: result.warnings,
        video: result.timeline.video,
        markers: result.timeline.markers,
        mediaUrl: registered.url,
      },
    };
    this.logger.info(`[demo] задание ${jobId} готово: ${result.size} байт, ${result.encoder}, попыток ${result.attempts}`);
    this.emit("demo-job-done", payload);
    if (kind === "export") {
      try {
        this.achieve("video-editor");
      } catch (error) {
        this.logger.warn(`[demo] достижение «Монтажёр»: ${error.message}`);
      }
    }
    return payload;
  }

  failed(jobId, kind, error) {
    const details = publicError(error, "JOB_FAILED");
    if (details.code === "CANCELLED") this.logger.info(`[demo] задание ${jobId} отменено`);
    else this.logger.warn(`[demo] задание ${jobId} не удалось: ${details.code} ${error && error.message}`);
    const payload = { jobId, kind, error: details };
    this.emit("demo-job-error", payload);
    return payload;
  }

  // С jobId — только это задание (поздняя отмена старого не гасит новое,
  // пока у того ещё нет id); без jobId — текущее, в том числе готовящееся.
  cancel(jobId) {
    const job = this.job;
    if (!job || (jobId && job.id !== jobId)) return { status: "ok", cancelled: false };
    job.controller.abort();
    return { status: "ok", cancelled: true };
  }

  // Предпросмотр хранится один: при новом, при уходе со страницы и при выходе удаляется.
  clearPreview() {
    if (this.job && this.job.kind === "preview") this.job.controller.abort();
    return this.dropPreview();
  }

  dropPreview() {
    const preview = this.preview;
    this.preview = null;
    if (!preview) return { status: "ok", cleared: false };
    this.media.unregister(preview.token);
    this.results.delete(preview.jobId);
    if (preview.dir) jobs.removeJobDir(this.jobsRoot, preview.dir);
    return { status: "ok", cleared: true };
  }

  result(id) {
    const found = typeof id === "string" ? this.results.get(id) : null;
    if (!found) throw new DemoError("RESULT_NOT_FOUND");
    return found;
  }

  mediaUrl(id) {
    return { status: "ok", url: this.result(id).url };
  }

  reveal(id) {
    const found = this.result(id);
    if (found.kind !== "export" || !fs.existsSync(found.file)) throw new DemoError("RESULT_NOT_FOUND");
    if (this.shell) this.shell.showItemInFolder(found.file);
    return { status: "ok" };
  }

  readTimeline(id) {
    const found = this.result(id);
    if (!found.timelineFile) return { status: "ok", timeline: found.timeline };
    try {
      return { status: "ok", timeline: readTimelineFile(found.timelineFile) };
    } catch (error) {
      throw error.name === "DemoError" ? error : new DemoError(error.code === "ENOENT" ? "RESULT_NOT_FOUND" : "TIMELINE_INVALID");
    }
  }

  // ---------- отрендеренные: видео в папке вывода ----------

  // Имя из окна — только имя файла .mp4 (без папок, «..», «:» потоков NTFS), а
  // файл — прямо в папке вывода; рядом его <имя>.timeline.json.
  renderedPaths(name) {
    // eslint-disable-next-line no-control-regex
    if (typeof name !== "string" || name.length > 255 || !/\.mp4$/i.test(name) || /[<>:"/\\|?*\x00-\x1f]/.test(name) || path.basename(name) !== name)
      throw new DemoError("PATH_INVALID");
    const dir = path.resolve(this.outputDir());
    const file = path.join(dir, name);
    if (path.dirname(file) !== dir) throw new DemoError("PATH_INVALID");
    return { dir, file, timeline: path.join(dir, `${name.slice(0, -4)}.timeline.json`) };
  }

  // Список вкладки: только видео со своим timeline.json (наши итоги), новые сверху.
  renderedList() {
    const dir = this.outputDir();
    let entries = [];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (error) {
      entries = [];
    }
    const videos = [];
    for (const entry of entries) {
      if (!entry.isFile() || !/\.mp4$/i.test(entry.name)) continue;
      try {
        const paths = this.renderedPaths(entry.name);
        const stat = fs.statSync(paths.file);
        videos.push({ name: entry.name, paths, size: stat.size, mtimeMs: stat.mtimeMs });
      } catch (error) {
        /* имя не годится или файл пропал */
      }
    }
    videos.sort((a, b) => b.mtimeMs - a.mtimeMs || a.name.localeCompare(b.name));
    const items = [];
    for (const video of videos) {
      if (items.length >= MAX_RENDERED) break;
      try {
        const timeline = publicTimeline(readTimelineFile(video.paths.timeline));
        items.push({ name: video.name, size: video.size, mtimeMs: video.mtimeMs, video: timeline.video, source: timeline.source,
          markerCount: timeline.markers.length });
      } catch (error) {
        /* чужое видео или битый timeline.json — не наше */
      }
    }
    return { status: "ok", outputDir: dir, items };
  }

  // Видео для плеера вкладки: адрес hlsr-media (один на вкладку) и метки из timeline.json.
  renderedOpen(name) {
    const paths = this.renderedPaths(name);
    let raw;
    let timeline;
    try {
      if (!fs.lstatSync(paths.file).isFile()) throw new DemoError("RESULT_NOT_FOUND");
      raw = readTimelineFile(paths.timeline);
      timeline = publicTimeline(raw);
    } catch (error) {
      throw error.name === "DemoError" ? error : new DemoError(error.code === "ENOENT" ? "RESULT_NOT_FOUND" : "TIMELINE_INVALID");
    }
    this.dropRendered();
    const registered = this.media.register(paths.file);
    this.rendered = { token: registered.token, file: paths.file };
    return { status: "ok", name, url: registered.url, video: timeline.video, source: timeline.source, markers: timeline.markers,
      sourceId: this.renderedSourceId(raw) };
  }

  // Исходная демка видео (кнопка «Экспорт» во вкладке «Отрендеренные»): по id из timeline.json,
  // а у видео старых сборок — по именам файлов, игре и карте, если такая запись в списке одна.
  renderedSourceId(timeline) {
    const source = timeline && timeline.source && typeof timeline.source === "object" ? timeline.source : {};
    const entries = this.entries();
    if (typeof source.id === "string" && entries.some((entry) => entry.id === source.id)) return source.id;
    const files = Array.isArray(source.files) && source.files.length ? source.files.map(String) : null;
    if (!files) return null;
    const matches = entries
      .filter((entry) => path.basename(entry.files[0]) === files[0])
      .filter((entry) => {
        const item = library.describe(entry);
        return item.game === (source.game || null) && (item.map || null) === (source.map || null) && JSON.stringify(item.sourceFiles) === JSON.stringify(files);
      });
    return matches.length === 1 ? matches[0].id : null;
  }

  // «Открыть папку»: Проводник с выделенным видео — только файл прямо в папке вывода.
  renderedReveal(name) {
    const paths = this.renderedPaths(name);
    let ok = false;
    try {
      ok = fs.lstatSync(paths.file).isFile();
    } catch (error) {
      ok = false;
    }
    if (!ok) throw new DemoError("RESULT_NOT_FOUND");
    if (this.shell) this.shell.showItemInFolder(paths.file);
    return { status: "ok" };
  }

  dropRendered() {
    if (this.rendered) this.media.unregister(this.rendered.token);
    this.rendered = null;
  }

  // В Корзину видео и его timeline.json (подтверждает окно). Ответ — новый список.
  async renderedDelete(name) {
    const paths = this.renderedPaths(name);
    const isFile = (file) => {
      try {
        return fs.lstatSync(file).isFile();
      } catch (error) {
        return false;
      }
    };
    if (!isFile(paths.file)) throw new DemoError("RESULT_NOT_FOUND");
    if (!this.shell || typeof this.shell.trashItem !== "function") throw new DemoError("TRASH_FAILED");
    if (this.rendered && this.rendered.file === paths.file) this.dropRendered();
    try {
      await this.shell.trashItem(paths.file);
      if (isFile(paths.timeline)) await this.shell.trashItem(paths.timeline);
    } catch (error) {
      this.logger.warn(`[demo] ${name} не убран в Корзину: ${error.message}`);
      throw new DemoError("TRASH_FAILED", error.message);
    }
    this.logger.info(`[demo] ${name} и его метки убраны в Корзину`);
    // Итог экспорта с этим файлом больше не отдаётся (иначе плеер «Демок» открывал бы удалённое).
    for (const [jobId, result] of this.results)
      if (result.file && path.resolve(result.file) === paths.file) {
        this.media.unregister(result.token);
        this.results.delete(jobId);
      }
    return Object.assign(this.renderedList(), { deleted: paths.file });
  }

  // Полный экран плеера в окне лаунчера (player-fullscreen.js); ответ — фактическое состояние.
  playerFullscreen(win, on) {
    return this.fullscreen.set(win, on === true);
  }

  attachProtocol(protocol) {
    protocol.handle(media.SCHEME, (request) => this.media.handle(request));
  }

  dispose() {
    if (this.muteTimer) clearInterval(this.muteTimer);
    this.muteTimer = null;
    if (this.job) this.job.controller.abort();
    // Рендер игры: дождаться закрытия её PID и вернуть реестр синхронно —
    // после выхода лаунчера асинхронная отмена не успеет.
    for (const renderer of this.renderers)
      try {
        renderer.disposeSync();
      } catch (error) {
        this.logger.warn(`[demo] рендер не закрыт при выходе: ${error.message}`);
      }
    this.processes.abort();
    this.fullscreen.dispose();
    this.clearPreview();
    this.media.clear();
  }
}

// IPC «Демо». Ответ: { status: "ok", … } или { status: "error", code, …подробности };
// demo-player-fullscreen отвечает просто состоянием (boolean). windowOf — окно
// лаунчера (trusted пропускает только его).
function bind(ipcMain, tool, trusted, windowOf) {
  const call = async (event, fn) => {
    if (trusted && !trusted(event)) return { status: "error", code: "UNTRUSTED" };
    try {
      return await fn();
    } catch (error) {
      const details = publicError(error, "DEMO_FAILED");
      if (!(error && error.name === "DemoError")) tool.logger.warn(`[demo] ${details.code}: ${error && (error.stack || error.message)}`);
      return Object.assign({ status: "error" }, details);
    }
  };
  ipcMain.handle("demo-list", (event) => call(event, () => tool.list()));
  ipcMain.handle("demo-add-paths", (event, paths) => call(event, () => tool.addPaths(paths)));
  ipcMain.handle("demo-pick-files", (event) => call(event, () => tool.pickFiles()));
  ipcMain.handle("demo-remove", (event, id) => call(event, () => tool.remove(id)));
  ipcMain.handle("demo-reorder", (event, id, order) => call(event, () => tool.reorder(id, order)));
  ipcMain.handle("demo-inspect", (event, id) => call(event, () => tool.inspect(id)));
  ipcMain.handle("demo-ffmpeg-status", (event) => call(event, async () => ({ status: "ok", ffmpeg: await tool.ffmpegStatus() })));
  ipcMain.handle("demo-ffmpeg-install", (event) => call(event, () => tool.installFfmpeg()));
  ipcMain.handle("demo-ffmpeg-cancel", (event) => call(event, () => tool.cancelFfmpegInstall()));
  ipcMain.handle("demo-bxtrs-status", (event) => call(event, () => ({ status: "ok", bxtrs: tool.bxtrsStatus() })));
  ipcMain.handle("demo-bxtrs-install", (event) => call(event, () => tool.installBxtrs()));
  ipcMain.handle("demo-bxtrs-cancel", (event) => call(event, () => tool.cancelBxtrsInstall()));
  ipcMain.handle("demo-encoders", (event) => call(event, () => tool.encoders()));
  ipcMain.handle("demo-options", (event, id) => call(event, () => tool.options(id)));
  ipcMain.handle("demo-plan", (event, id, settings) => call(event, () => tool.plan(id, settings)));
  ipcMain.handle("demo-estimate", (event, id, list) => call(event, () => tool.estimate(id, list)));
  ipcMain.handle("demo-export-start", (event, id, settings) => call(event, () => tool.startExport(id, settings)));
  ipcMain.handle("demo-preview-start", (event, id, settings) => call(event, () => tool.startPreview(id, settings)));
  ipcMain.handle("demo-cancel", (event, jobId) => call(event, () => tool.cancel(jobId)));
  ipcMain.handle("demo-preview-clear", (event) => call(event, () => tool.clearPreview()));
  ipcMain.handle("demo-settings-get", (event) => call(event, () => tool.settings()));
  ipcMain.handle("demo-output-pick", (event) => call(event, () => tool.pickOutputDir()));
  ipcMain.handle("demo-reveal", (event, id) => call(event, () => tool.reveal(id)));
  ipcMain.handle("demo-media-url", (event, id) => call(event, () => tool.mediaUrl(id)));
  ipcMain.handle("demo-timeline-read", (event, id) => call(event, () => tool.readTimeline(id)));
  ipcMain.handle("demo-rendered-list", (event) => call(event, () => tool.renderedList()));
  ipcMain.handle("demo-rendered-open", (event, name) => call(event, () => tool.renderedOpen(name)));
  ipcMain.handle("demo-rendered-delete", (event, name) => call(event, () => tool.renderedDelete(name)));
  ipcMain.handle("demo-rendered-reveal", (event, name) => call(event, () => tool.renderedReveal(name)));
  ipcMain.handle("demo-player-fullscreen", (event, on) => call(event, () => tool.playerFullscreen(windowOf ? windowOf() : null, on)));
}

module.exports = DemoTool;
module.exports.DemoTool = DemoTool;
module.exports.bind = bind;
