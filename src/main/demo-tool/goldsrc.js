"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { DemoError } = require("./errors");
const gr = require("./game-render");
const gsr = require("./goldsrc-render");
const { commandLine } = require("./source-render");
const { scanDemo } = require("./goldsrc-demo");
const headers = require("./headers");
const ffmpeg = require("./ffmpeg");
const { freeSpaceBytes } = require("../game-delivery/free-space");
const { hashFile } = require("../game-delivery/file-ops");

// Адаптер рендера Half-Life (WON-сборка HLSR, BUILD 3248) через bxt-rs
// (скачиваемый инструмент, своя сборка с поддержкой WON). Игра на скрытом
// рабочем столе (помощник render-host, общий с Ghosting — game-render.js)
// проигрывает нужные сегменты по порядку (bxt_play_folder), bxt-rs сам
// отдаёт кадры и звук FFmpeg: видео на каждый сегмент (bxt_cap_separate_start)
// без кадров загрузки (bxt_cap_skip_non_gameplay_frames). Сегменты
// склеиваются в один материал, итог кодирует общий слой (jobs.js).
//
// bxt_rs.dll в обычную игру игрока не попадает: Injector внедряет все DLL
// своей папки, поэтому для рендера Injector, BunnymodXT.dll и bxt_rs.dll
// копируются в папку задания лаунчера, а папка «Bunnymod XT» игрока не меняется.
// Вернуть игроку как было: реестр HKCU\Software\Valve\Half-Life\Settings,
// mute hl.exe (общее), qconsole.log и bxt-rs.log в папке Half-Life.

const GAME_ID = "hl1-won";
const HL_DIR = "Half-Life";
const EXE = "hl.exe";
const BXT_DIR = "Bunnymod XT";
const DLL = "bxt_rs.dll";
const LOGS = ["qconsole.log", "bxt-rs.log"];
const SPEC = Object.freeze({
  gameId: GAME_ID,
  image: EXE,
  registryKey: "HKCU\\Software\\Valve\\Half-Life\\Settings",
  marker: "hl1-render.restore.json",
  regFile: "hl1-render.settings.reg",
  unmutePending: "hl1UnmutePending",
  label: "HL1",
  // Любой hl.exe (Steam HL1, CS 1.6…): общий с WON ключ реестра и мьютекс движка.
  otherRunning: "GOLDSRC_RUNNING",
  required: [path.join(HL_DIR, EXE), path.join(BXT_DIR, "Injector.exe")],
  renderRoot: (install) => path.join(install, HL_DIR, "valve_WON", gsr.RENDER_DIR),
  // hud, ownHud: HUD игры выключается, «Персонализация» — стандартная (goldsrc-render viewLines).
  caps: Object.freeze({ keyhud: true, audio: true, hud: true, ownHud: true }),
});
// До первой строки «Playing demo from»: запуск с bxt-rs ~5 с, загрузка карты.
const FIRST_TIMEOUT_MS = 90000;
// Ни строк журнала, ни прогресса FFmpeg.
const STALL_TIMEOUT_MS = 60000;
const POLL_MS = 100;
// Промежуточное H.264 (ultrafast, CRF 14) для проверки места: в игре и копия в задании.
const INTERMEDIATE_BITS_PER_PIXEL = 0.35;
const RESERVE_BYTES = 512 * 1024 * 1024;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const segmentName = (k) => `s${String(k + 1).padStart(3, "0")}`;

function readTail(file, bytes) {
  let fd;
  try {
    fd = fs.openSync(file, "r");
  } catch (error) {
    return null;
  }
  try {
    const size = fs.fstatSync(fd).size;
    const length = Math.min(size, bytes);
    const buffer = Buffer.alloc(length);
    fs.readSync(fd, buffer, 0, length, size - length);
    return buffer.toString("latin1");
  } finally {
    fs.closeSync(fd);
  }
}

// Окружение игры: FFmpeg инструмента первым в PATH (bxt-rs запускает
// «ffmpeg» сам, а в системном PATH может быть другой), без Discord RPC и
// vsync BXT. Остальную чистку (HLSR_*, SLINT_*, Electron, Steam) делает помощник.
function gameEnv(env, ffmpegDir) {
  const out = {};
  let systemPath = "";
  for (const [key, value] of Object.entries(env || {})) {
    if (/^path$/i.test(key)) systemPath = value;
    else if (!/^bxt_disable_(discord_rpc|vsync)$/i.test(key)) out[key] = value;
  }
  out.PATH = systemPath ? `${ffmpegDir}${path.delimiter}${systemPath}` : ffmpegDir;
  out.BXT_DISABLE_DISCORD_RPC = "1";
  out.BXT_DISABLE_VSYNC = "1";
  return out;
}

// Список для concat FFmpeg: длительность каждого файла — длительность его
// видео, чтобы следующий сегмент начинался ровно на своём кадре; звук после
// конца видео сегмента отрезается (outpoint), расхождения не копятся по стыкам.
function concatList(files, durations) {
  return files.map((file, k) => {
    const duration = durations[k].toFixed(6);
    return `file '${file.replace(/\\/g, "/").replace(/'/g, "'\\''")}'\nduration ${duration}\noutpoint ${duration}\n`;
  }).join("");
}

class Hl1Adapter extends gr.GameRenderAdapter {
  constructor(options) {
    const opts = options || {};
    super(opts, SPEC);
    this.id = "goldsrc";
    this.engine = "goldsrc";
    // bxt-rs: () => { path, sha256 | null } (null — не установлен).
    this.bxtrs = opts.bxtrs || (() => null);
    this.runFn = opts.runFn || ffmpeg.run;
    this.ffprobeFn = opts.ffprobeFn || ffmpeg.ffprobe;
    this.freeSpace = opts.freeSpace || freeSpaceBytes;
    this.scanFn = opts.scanFn || scanDemo;
    // Подмены для тестов.
    this.firstTimeoutMs = opts.firstTimeoutMs || FIRST_TIMEOUT_MS;
    this.stallTimeoutMs = opts.stallTimeoutMs || STALL_TIMEOUT_MS;
  }

  bxtrsPath() {
    try {
      const found = this.bxtrs();
      return found && found.path && fs.existsSync(found.path) ? found : null;
    } catch (error) {
      return null;
    }
  }

  // Для оценки времени (estimate.js): один запуск игры, демки набора подряд.
  estimateProfile(item, range) {
    return { key: "hl1", launches: 1, segments: Math.max(1, gsr.planSegments(item.segments, range).picked.length) };
  }

  canRender(item) {
    if (!item || item.engine !== "goldsrc" || item.game !== "hl1") return { ok: false, code: "DEMO_UNSUPPORTED" };
    if (this.platform !== "win32") return { ok: false, code: "RENDER_NOT_READY" };
    if (!this.installPath()) return { ok: false, code: "GAME_NOT_INSTALLED" };
    const segments = item.segments || [];
    if (segments.some((segment) => segment.netProtocol !== gsr.NET_PROTOCOL)) return { ok: false, code: "DEMO_PROTOCOL_UNSUPPORTED" };
    if (!segments.some((segment) => segment.duration > 0)) return { ok: false, code: "DEMO_EMPTY" };
    if (!this.bxtrsPath()) return { ok: false, code: "BXTRS_MISSING" };
    return { ok: true };
  }

  checkSpace(install, jobDir, seconds, request) {
    const videoBytes = Math.ceil((request.width * request.height * request.fps * INTERMEDIATE_BITS_PER_PIXEL * seconds) / 8);
    const gameNeed = videoBytes + RESERVE_BYTES;
    const jobNeed = videoBytes * 2 + RESERVE_BYTES;
    const gameFree = this.freeSpace(install);
    const jobFree = this.freeSpace(jobDir);
    const sameDisk = path.parse(path.resolve(install)).root.toLowerCase() === path.parse(path.resolve(jobDir)).root.toLowerCase();
    if (sameDisk ? gameFree !== null && gameFree < gameNeed + jobNeed : (gameFree !== null && gameFree < gameNeed) || (jobFree !== null && jobFree < jobNeed))
      throw new DemoError("NO_SPACE");
  }

  // Файлы игрока, которые игра с bxt-rs дописывает: копия до запуска, после — как было.
  // Копии — рядом с маркером реестра (файлы корня заданий переживают чистку
  // папок), список — в маркере: после сбоя лаунчера их вернёт recover().
  saveLogs(hlDir, jobDir) {
    const dir = this.jobsRoot || jobDir;
    return LOGS.map((name) => {
      const file = path.join(hlDir, name);
      const backup = path.join(dir, `hl1-render.${name}.orig`);
      try {
        fs.copyFileSync(file, backup);
        return { file, backup, start: fs.statSync(file).size };
      } catch (error) {
        if (error.code !== "ENOENT") throw new DemoError("RENDER_FAILED", `${name}: ${error.message}`);
        fs.rmSync(backup, { force: true });
        return { file, backup: null, start: 0 };
      }
    });
  }

  restoreLogs(logs) {
    for (const log of logs || []) {
      // Маркер — наш файл, но пути из него проверяются: только журналы в папке
      // Half-Life и копии рядом с маркером.
      const file = log && String(log.file);
      const backup = log && log.backup ? String(log.backup) : null;
      if (!file || !LOGS.includes(path.basename(file)) || path.basename(path.dirname(file)) !== HL_DIR) continue;
      if (backup && (!/^hl1-render\.[\w.-]+\.orig$/.test(path.basename(backup)) ||
        (this.jobsRoot && path.resolve(path.dirname(backup)) !== path.resolve(this.jobsRoot)))) continue;
      try {
        if (log.backup) {
          fs.copyFileSync(log.backup, log.file);
          fs.rmSync(log.backup, { force: true });
        } else fs.rmSync(log.file, { force: true });
      } catch (error) {
        this.logger.warn(`[demo] HL1: ${path.basename(log.file)} не возвращён: ${error.message}`);
      }
    }
  }

  // Журналы прошлого рендера, прерванного сбоем, — вместе с реестром.
  recover() {
    const marker = this.jobsRoot ? gr.readMarker(this.jobsRoot, SPEC) : null;
    const result = super.recover();
    this.restoreMarkerLogs(marker);
    return result;
  }

  restorePrevious() {
    const marker = this.jobsRoot ? gr.readMarker(this.jobsRoot, SPEC) : null;
    super.restorePrevious();
    this.restoreMarkerLogs(marker);
  }

  restoreMarkerLogs(marker) {
    if (!marker || !marker.logs) return;
    this.restoreLogs(marker.logs);
    this.forgetMarkerLogs();
  }

  // Журналы возвращаются один раз: маркер, оставшийся из-за невозвращённого реестра,
  // переписывается без них — иначе следующий старт или рендер удалил бы журнал,
  // который игрок создал уже после рендера.
  forgetMarkerLogs() {
    const left = this.jobsRoot ? gr.readMarker(this.jobsRoot, SPEC) : null;
    if (!left || !left.logs) return;
    delete left.logs;
    try {
      gr.writeMarker(this.jobsRoot, SPEC, left);
    } catch (error) {
      this.logger.warn(`[demo] HL1: маркер рендера не переписан: ${error.message}`);
    }
  }

  finish(active) {
    super.finish(active);
    if (active.logs) {
      this.restoreLogs(active.logs);
      active.logs = null;
      this.forgetMarkerLogs();
    }
  }

  // Беззвучный сброс mute без рендера (game-render.js resetMute): hl.exe без внедрения.
  resetLaunch(install) {
    const hlDir = path.join(install, HL_DIR);
    const exe = path.join(hlDir, EXE);
    return { exe, cwd: hlDir, commandLine: commandLine(exe, gsr.resetArguments()) };
  }

  async renderJob(request, active) {
    const { item, range, fps, width, height, audio, keyhud, jobDir, signal, onProgress, ffmpegPath } = request;
    const aborted = () => {
      if (signal && signal.aborted) throw new DemoError("CANCELLED");
    };
    if (this.platform !== "win32") throw new DemoError("RENDER_NOT_READY");
    if (!this.script) throw new DemoError("RENDER_HOST_MISSING");
    if (!ffmpegPath) throw new DemoError("FFMPEG_MISSING");
    const install = this.installPath();
    if (!install) throw new DemoError("GAME_NOT_INSTALLED");
    if (!gr.FPS.includes(fps)) throw new DemoError("COMBO_UNSUPPORTED");
    const bxtrs = this.bxtrsPath();
    if (!bxtrs) throw new DemoError("BXTRS_MISSING");
    const hlDir = path.join(install, HL_DIR);
    const exe = path.join(hlDir, EXE);

    await this.preflight(exe);
    // bxt-rs ищет ffmpeg.exe сначала рядом с hl.exe: чужой там перебил бы наш из PATH.
    if (fs.existsSync(path.join(hlDir, "ffmpeg.exe"))) throw new DemoError("FFMPEG_IN_GAME_DIR");
    const plan = gsr.planSegments(item.segments, range);
    if (!plan.picked.length) throw new DemoError("DEMO_EMPTY");
    const replaced = (entry) => new DemoError("SEGMENT_REPLACED", null, { segment: entry.index + 1, name: entry.segment.name });
    for (const entry of plan.picked) {
      let stat;
      try {
        stat = fs.lstatSync(entry.segment.path);
      } catch (error) {
        stat = null;
      }
      if (!stat || !stat.isFile()) throw new DemoError("SEGMENT_MISSING", null, { segment: entry.index + 1, name: entry.segment.name });
      // Файл заменили после того, как набор добавили в список (другой размер или заголовок).
      const header = headers.parseDemoFile(entry.segment.path);
      if (stat.size !== entry.segment.size || !header.ok || header.engine !== "goldsrc" || header.duration !== entry.segment.duration) throw replaced(entry);
    }
    const demoSeconds = plan.picked.reduce((sum, entry) => sum + entry.duration, 0);
    this.checkSpace(install, jobDir, demoSeconds, request);
    // Смены карт, кнопки и паузы загрузок — из самих демок; не читается — без них.
    const scans = [];
    for (const entry of plan.picked) {
      let scan = null;
      try {
        scan = await this.scanFn(entry.segment.path, { signal });
      } catch (error) {
        if (error.code === "CANCELLED") throw new DemoError("CANCELLED");
        this.logger.warn(`[demo] HL1: ${entry.segment.name} не разобрана: ${error.message}`);
      }
      scans.push(scan && scan.ok ? scan : null);
    }
    aborted();
    this.restorePrevious();

    // Папка задания в игре: bxt_play_folder и exec видят только папку мода.
    const id8 = path.basename(jobDir).replace(/[^0-9a-z]/gi, "").slice(0, 8).toLowerCase() || crypto.randomBytes(4).toString("hex");
    const dir = `${gsr.RENDER_DIR}/${id8}`;
    let host = null;
    const onAbort = () => {
      if (active.host) active.host.stop(this.steamCode() === null);
    };
    if (signal) signal.addEventListener("abort", onAbort, { once: true });

    try {
      active.logs = this.saveLogs(hlDir, jobDir);
      this.snapshot(active, { logs: active.logs.map(({ file, backup }) => ({ file, backup })) });
      const modJobDir = gr.makeRenderDir(SPEC.renderRoot(install), id8);
      active.modJobDir = modJobDir;
      plan.picked.forEach((entry, k) => gr.linkOrCopy(entry.segment.path, path.join(modJobDir, `${segmentName(k)}.dem`)));
      fs.writeFileSync(path.join(modJobDir, "job.cfg"), gsr.jobConfig({ fps, dir, hud: request.hud, ownHud: request.ownHud }));
      // Injector внедряет все DLL своей папки: своя папка только с нужными.
      const injectDir = path.join(jobDir, "inject");
      fs.mkdirSync(injectDir);
      const injector = path.join(injectDir, "Injector.exe");
      fs.copyFileSync(path.join(install, BXT_DIR, "Injector.exe"), injector);
      const bxt = path.join(install, BXT_DIR, "BunnymodXT.dll");
      if (fs.existsSync(bxt)) fs.copyFileSync(bxt, path.join(injectDir, "BunnymodXT.dll"));
      fs.copyFileSync(bxtrs.path, path.join(injectDir, DLL));
      // Закреплённая сборка: сверяется та копия, которая пойдёт в игру; подменённая или битая — не пойдёт.
      if (bxtrs.sha256 && (await hashFile(path.join(injectDir, DLL))) !== bxtrs.sha256) throw new DemoError("BXTRS_BROKEN");

      const unmuteAll = active.muted;
      host = this.hostFactory({ script: this.script, env: gameEnv(this.env, path.dirname(ffmpegPath)), logger: this.logger });
      active.host = host;
      this.watchAudio(host, active, unmuteAll);
      // Чужой hl.exe (Steam HL1, CS 1.6…) появился во время рендера: у него с WON общий
      // ключ реестра — рендер останавливается, реестр возвращается сразу, пока та игра
      // ещё ничего не сохранила (настройки движок пишет при выходе).
      host.on("event", (message) => {
        if (message.event === "foreign" && !active.foreign) active.foreign = message.pid || true;
      });
      const startedAt = Date.now();
      const pid = await host.start({
        exe: injector,
        cwd: hlDir,
        commandLine: commandLine(injector, [exe].concat(gsr.gameArguments({ width, height, dir }))),
        mute: true,
        unmuteAll,
        followJob: true,
        watchImage: "hl",
        resetExe: exe,
        resetCommandLine: commandLine(exe, gsr.resetArguments()),
      });
      this.logger.info(`[demo] HL1: Injector PID ${pid}, сегментов ${plan.picked.length}, ${width}x${height}@${fps}`);
      await this.watch({ host, plan, active, modJobDir, hlDir, signal, onProgress, startedAt });
      // Конец всех демок: игра в меню, закрывается заглушённой, mute снимает перезапуск.
      await this.stopHost(active, true);
      this.logger.info(`[demo] HL1: рендер ${Math.round((Date.now() - startedAt) / 1000)} с`);
      aborted();

      const files = plan.picked.map((entry, k) => path.join(modJobDir, "out", `${segmentName(k)}.mp4`));
      const videos = [];
      for (const file of files) {
        if (!fs.existsSync(file)) throw new DemoError("RENDER_FAILED", `Нет видео ${path.basename(file)}`);
        const info = await this.ffprobeFn(request.ffprobePath || path.join(path.dirname(ffmpegPath), "ffprobe.exe"), file, { signal });
        const stream = ((info && info.streams) || []).find((entry) => entry.codec_type === "video");
        const duration = Number(stream && stream.duration);
        if (!(duration > 0)) throw new DemoError("RENDER_FAILED", `Видео ${path.basename(file)} пустое`);
        // Загрузки съедают ~0,1 с в начале и ~0,15–0,3 с на смене карты; много больше —
        // демка оборвалась (bxt-rs и тогда пишет «Recording stopped.»), клип вышел бы без конца сегмента.
        const entry = plan.picked[videos.length];
        const scan = scans[videos.length];
        const changes = scan ? (scan.mapChanges || []).filter((change) => change.t > 0).length : 0;
        if (entry.duration - duration > 1 + changes)
          throw new DemoError("RENDER_FAILED", `Сегмент ${entry.index + 1}: видео ${duration} с при демке ${entry.duration} с`);
        videos.push(duration);
      }
      const material = path.join(jobDir, "material.mkv");
      const list = path.join(jobDir, "segments.txt");
      fs.writeFileSync(list, concatList(files, videos));
      await this.runFn(ffmpegPath, ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", list,
        "-map", "0:v:0", "-map", "0:a:0?", "-c", "copy", material], { signal, cwd: jobDir });

      const cuts = gsr.lossCuts(plan.picked, videos, scans);
      const timing = { offset: plan.offset, cuts };
      const total = videos.reduce((sum, value) => sum + value, 0);
      this.logger.info(`[demo] HL1: видео ${total.toFixed(3)} с при демке ${demoSeconds.toFixed(3)} с, вырезано загрузок ${cuts.length}`);
      let overlay = null;
      const warnings = [];
      if (keyhud) {
        overlay = "keyhud.ass";
        fs.writeFileSync(path.join(jobDir, overlay), gsr.keyhudAss(gsr.keyEvents(plan.picked, scans, timing), { width, height, end: total }));
        // Демка не разобралась или разбор оборвался: там клавиши не загорятся — сказать в итоге.
        if (scans.some((scan) => !scan || scan.broken)) warnings.push("KEYHUD_PARTIAL");
      }
      return {
        video: material,
        audio: audio ? material : null,
        offset: plan.offset,
        cuts,
        timeline: gsr.timelineOf(plan.picked, scans, range),
        overlay,
        warnings,
        // Для оценки времени следующих заданий (estimate.js).
        profile: "hl1",
        launches: 1,
        segments: plan.picked.length,
        frames: Math.round(total * fps),
      };
    } finally {
      if (signal) signal.removeEventListener("abort", onAbort);
      // Рядом чужой hl.exe: без сброса mute (WON hl.exe рядом с ним ждал бы мьютекс до 30 с) —
      // реестр сразу, mute снимет следующий рендер.
      if (active.host) await this.stopHost(active, !active.foreign);
      this.finish(active);
    }
  }

  // Ход рендера: новые строки qconsole.log (сегменты, конец по «Recording
  // stopped.» каждого, ошибки) и прогресс FFmpeg текущего сегмента.
  async watch({ host, plan, active, modJobDir, hlDir, signal, onProgress, startedAt }) {
    const logFile = path.join(hlDir, "qconsole.log");
    const progressFile = path.join(modJobDir, "progress.txt");
    const total = plan.picked.reduce((sum, entry) => sum + entry.duration, 0);
    const log = active.logs.find((entry) => entry.file === logFile);
    let logPos = log ? log.start : 0;
    let rest = "";
    let stopped = 0;
    let playing = null;
    let failure = null;
    let lastActivity = startedAt;
    let lastProgress = null;
    let lastReport = 0;
    let fraction = 0;
    const readLog = () => {
      let fd;
      try {
        fd = fs.openSync(logFile, "r");
      } catch (error) {
        return;
      }
      try {
        const size = fs.fstatSync(fd).size;
        // Движок мог начать журнал заново.
        if (size < logPos) logPos = 0;
        if (size === logPos) return;
        const chunk = Buffer.alloc(Math.min(size - logPos, 1024 * 1024));
        const read = fs.readSync(fd, chunk, 0, chunk.length, logPos);
        logPos += read;
        const lines = (rest + chunk.subarray(0, read).toString("latin1")).split(/\n/);
        rest = lines.pop();
        lastActivity = Date.now();
        for (const line of lines) {
          const parsed = gsr.parseLogLine(line);
          if (!parsed) continue;
          if (parsed.kind === "error" && !failure) failure = new DemoError(parsed.code, parsed.text);
          else if (parsed.kind === "playing") playing = parsed.index;
          else if (parsed.kind === "stopped") stopped++;
        }
      } finally {
        fs.closeSync(fd);
      }
    };
    for (;;) {
      if (signal && signal.aborted) throw new DemoError("CANCELLED");
      readLog();
      if (failure) throw failure;
      if (stopped >= plan.picked.length) return;
      if (active.foreign) throw new DemoError("GOLDSRC_RUNNING", `Запущен другой hl.exe (PID ${active.foreign})`);
      // Код выхода здесь — у Injector, не у игры: в текст ошибки его не отдаём.
      if (host.exitCode !== null) throw new DemoError("GAME_EXITED", `Задание опустело, код Injector ${host.exitCode}`);
      if (host.closed) throw new DemoError("RENDER_HOST_FAILED", "Помощник вышел");
      const out = gsr.progressSeconds(readTail(progressFile, 2048));
      if (out !== null && out !== lastProgress) {
        lastProgress = out;
        lastActivity = Date.now();
      }
      const now = Date.now();
      if (playing === null && now - startedAt > this.firstTimeoutMs) {
        // bxt-rs не встал: в bxt-rs.log нет строки о найденном движке.
        const found = /found the WON engine/.test(readTail(path.join(hlDir, "bxt-rs.log"), 64 * 1024) || "");
        throw new DemoError(found ? "RENDER_TIMEOUT" : "BXTRS_FAILED");
      }
      if (now - lastActivity > this.stallTimeoutMs) throw new DemoError("RENDER_TIMEOUT");
      if (onProgress && now - lastReport >= 250) {
        lastReport = now;
        const done = plan.picked.slice(0, stopped).reduce((sum, entry) => sum + entry.duration, 0);
        const entry = playing !== null && stopped < playing ? plan.picked[playing - 1] || null : null;
        // progress.txt прошлого сегмента живёт, пока FFmpeg нового его не перепишет: доля не убывает.
        fraction = Math.max(fraction, Math.min(0.999, (done + (entry ? Math.min(lastProgress || 0, entry.duration) : 0)) / Math.max(total, 0.001)));
        onProgress({
          fraction,
          segment: playing !== null ? { index: playing, count: plan.picked.length } : null,
          message: null,
        });
      }
      await sleep(POLL_MS);
    }
  }
}

module.exports = { GAME_ID, SPEC, Hl1Adapter, gameEnv, concatList };
