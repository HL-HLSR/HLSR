"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { spawn } = require("child_process");

const { DemoError } = require("./errors");
const sr = require("./source-render");
const gr = require("./game-render");
const gh = require("./ghrender");
const { freeSpaceBytes } = require("../game-delivery/free-space");
const { hashFile } = require("../game-delivery/file-ops");

// Адаптер рендера Ghosting Mod: Renewed (Source 2013, мод ghostingmod) по
// схеме спайка: игра на скрытом рабочем столе (помощник render-host)
// проигрывает непустые сегменты через startdemos, startmovie пишет кадры
// TGA и WAV в папку задания внутри мода, насос кадров отдаёт их FFmpeg
// (промежуточный H.264 без звука) и сразу удаляет. Пачками до 18 сегментов
// (строка startdemos ≤ 512 символов), по запуску игры на пачку; видео и звук непрерывны.
// Мод с режимом -ghrender (ghrender.js, признак — строки в client.dll): нужные записи
// плейлиста одним запуском, без копий демок, только кадры рана, звук без сдвига, события в журнале.
//
// Что адаптер обязан вернуть игроку как было (общее с HL1 — game-render.js):
// - реестр HKCU\Software\Valve\Source\ghostingmod\Settings: снимок до, возврат после;
// - звук hl2.exe: mute снимает беззвучный перезапуск, иначе флаг ghostingUnmutePending;
// - cfg/config.cfg и console.log игрока: игра закрывается только
//   TerminateJobObject своего задания, журнал консоли (+con_logfile) — в папке задания.

const GAME_ID = "ghosting-renewed";
const MOD = "ghostingmod";
const EXE = "hl2.exe";
const RENDER_DIR = "hlsr-render";
const REGISTRY_KEY = "HKCU\\Software\\Valve\\Source\\ghostingmod\\Settings";
const MARKER = "ghosting-render.restore.json";
const REG_FILE = "ghosting-render.settings.reg";
const UNMUTE_PENDING = "ghostingUnmutePending";
const FPS = gr.FPS;
const SPEC = Object.freeze({
  gameId: GAME_ID,
  image: EXE,
  registryKey: REGISTRY_KEY,
  marker: MARKER,
  regFile: REG_FILE,
  unmutePending: UNMUTE_PENDING,
  label: "Ghosting",
  required: [EXE, MOD],
  renderRoot: (install) => path.join(install, MOD, RENDER_DIR),
  // integrity: «GH DEMO DATA: INTACT» Ghosting (gh_demo_overlay) по выбору игрока.
  // ownConfig, ownHud: стандартный конфиг или HUD (source-render.configArguments); HUD целиком — нет.
  caps: Object.freeze({ keyhud: true, audio: true, integrity: true, ownConfig: true, ownHud: true }),
});
const FIRST_FRAME_TIMEOUT_MS = 120000;
const FRAME_TIMEOUT_MS = 60000;
// Первая строка журнала (gh.PROTOCOL) приходит через ~2,5 с после запуска игры; мод без
// режима -ghrender молча игнорирует и висит в меню — тогда тот же экспорт идёт старым путём.
const GHRENDER_FIRST_LINE_MS = 45000;
// Застой: дольше собственного таймаута мода на открытие демки (60 с, FAIL DEMO_FAILED).
const GHRENDER_STALL_MS = 75000;
const MAX_JOURNAL_LINE = 4096;
// После DONE игра сама выходит через 1 с; дольше — закрывает задание.
const GHRENDER_EXIT_MS = 5000;
// Загрузка сегмента (0,3–0,7 с) между кадрами: следующий кадр — первый
// кадр нового сегмента, переходный (без HUD, ракурс с прошлого сегмента).
const JOIN_GAP_MS = 250;
const POLL_MS = 10;
// Оценка промежуточного H.264 (ultrafast, CRF 14) для проверки места.
const INTERMEDIATE_BITS_PER_PIXEL = 0.35;
const RESERVE_BYTES = 512 * 1024 * 1024;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const frameName = (dir, index) => path.join(dir, `f_${String(index).padStart(4, "0")}.tga`);
// Папка задания в моде — первые 8 символов jobId (startdemos хранит не больше 31 символа на демо).
const jobId8 = (jobDir) => path.basename(jobDir).replace(/[^0-9a-z]/gi, "").slice(0, 8).toLowerCase() || crypto.randomBytes(4).toString("hex");
// Сброс mute после закрытия заглушённой игры (помощник коротко перезапустит
// её и снимет глушение): в самой игре звука нет (volume 0).
const resetArguments = (logFile) => ["-steam", "-game", MOD, "-windowed", "-noborder", "-w", "640", "-h", "480", "-novid", "-multirun",
  "+con_logfile", logFile, "+developer", "1", "+volume", "0", "+snd_mute_losefocus", "0"];

// Стандартный конфиг или HUD: cfg в папке задания в моде (std), аргументы игры для них.
function configFor(request, install, modJobDir) {
  const std = path.join(modJobDir, "std");
  const plan = sr.configArguments({ ownConfig: request.ownConfig, ownHud: request.ownHud, dir: path.relative(install, std).split(path.sep).join("/") });
  for (const [name, text] of Object.entries(plan.files)) {
    const file = path.join(std, ...name.split("/"));
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
  return plan.args;
}

async function moveFile(from, to) {
  try {
    await fs.promises.rename(from, to);
  } catch (error) {
    // Игра и папка заданий на разных дисках.
    await fs.promises.copyFile(from, to, fs.constants.COPYFILE_EXCL);
    await fs.promises.rm(from, { force: true });
  }
}

// Реестр, маркер и папка рендера — общие (game-render.js), со своими именами Ghosting.
const readMarker = (root) => gr.readMarker(root, SPEC);
const snapshotRegistry = (reg, root, extra) => gr.snapshotRegistry(reg, root, SPEC, extra);
const restoreRegistry = (reg, root) => gr.restoreRegistry(reg, root, SPEC);
const cleanupRenderDirs = (modDir, keep) => gr.cleanupRenderDirs(path.join(modDir, RENDER_DIR), keep);

class GhostingAdapter extends gr.GameRenderAdapter {
  constructor(options) {
    const opts = options || {};
    super(opts, SPEC);
    this.id = "ghosting";
    this.engine = "source";
    this.spawnFn = opts.spawnFn || spawn;
    this.freeSpace = opts.freeSpace || freeSpaceBytes;
    // Подмены для тестов: размер пачки и таймауты кадров.
    this.batchSize = opts.batchSize || sr.MAX_BATCH;
    this.firstFrameTimeoutMs = opts.firstFrameTimeoutMs || FIRST_FRAME_TIMEOUT_MS;
    this.frameTimeoutMs = opts.frameTimeoutMs || FRAME_TIMEOUT_MS;
    this.ghrenderFirstLineMs = opts.ghrenderFirstLineMs || GHRENDER_FIRST_LINE_MS;
    this.ghrenderStallMs = opts.ghrenderStallMs || GHRENDER_STALL_MS;
  }

  // Беззвучный сброс mute без рендера (game-render.js resetMute): консоль — в своей папке в моде.
  resetLaunch(install, active) {
    const id = crypto.randomBytes(4).toString("hex");
    active.modJobDir = gr.makeRenderDir(SPEC.renderRoot(install), id);
    const exe = path.join(install, EXE);
    return { exe, cwd: install, commandLine: sr.commandLine(exe, resetArguments(`${RENDER_DIR}/${id}/reset.log`)) };
  }

  // Для оценки времени (estimate.js): каким путём пойдёт рендер, сколько запусков игры.
  estimateProfile(item, range, fps) {
    const picked = sr.planSegments(item.segments, range, fps).picked;
    const install = this.installPath();
    if (install && this.ghrenderPlaylist(install, item, true)) return { key: "ghosting-ghrender", launches: 1, segments: picked.length };
    // Имена s001… в папке задания: «hlsr-render/<8 знаков>/sNNN».
    const size = Math.min(this.batchSize, sr.batchLimit(`${RENDER_DIR}/abcdefgh/s${String(picked.length).padStart(3, "0")}`.length));
    return { key: "ghosting-startdemos", launches: Math.max(1, Math.ceil(picked.length / size)), segments: picked.length };
  }

  canRender(item) {
    if (!item || item.engine !== "source" || item.game !== "ghosting") return { ok: false, code: "DEMO_UNSUPPORTED" };
    if (this.platform !== "win32") return { ok: false, code: "RENDER_NOT_READY" };
    if (!this.installPath()) return { ok: false, code: "GAME_NOT_INSTALLED" };
    if (!(item.segments || []).some((segment) => (segment.ticks || 0) > 1)) return { ok: false, code: "DEMO_EMPTY" };
    return { ok: true };
  }

  async checkSegments(picked) {
    for (const item of picked) {
      const segment = item.segment;
      let stat;
      try {
        stat = fs.lstatSync(segment.path);
      } catch (error) {
        stat = null;
      }
      if (!stat || !stat.isFile()) throw new DemoError("SEGMENT_MISSING", null, { segment: item.index + 1 });
      // Плейлист хранит размер и SHA-256 каждого сегмента: изменённый
      // сегмент сырой playdemo проиграл бы молча.
      if (segment.sizeMatches === false || (segment.sha256 && (await hashFile(segment.path)) !== segment.sha256))
        throw new DemoError("SEGMENT_CHANGED", null, { segment: item.index + 1 });
    }
  }

  checkSpace(install, jobDir, plan, request, ghrender) {
    const seconds = plan.framesTotal / request.fps;
    const frameBytes = request.width * request.height * 3;
    const wavBytes = request.audio ? Math.ceil(seconds * 176400) : 0;
    // -ghrender пишет звук всегда и к концу держит два WAV на весь ран (raw_ и audio.wav).
    const gameWav = ghrender ? Math.ceil(seconds * 176400) * 2 : wavBytes;
    const videoBytes = Math.ceil((request.width * request.height * request.fps * INTERMEDIATE_BITS_PER_PIXEL * seconds) / 8);
    const gameNeed = frameBytes * 90 + gameWav + RESERVE_BYTES;
    const jobNeed = videoBytes * 2 + wavBytes + RESERVE_BYTES;
    const gameFree = this.freeSpace(install);
    const jobFree = this.freeSpace(jobDir);
    const sameDisk = path.parse(path.resolve(install)).root.toLowerCase() === path.parse(path.resolve(jobDir)).root.toLowerCase();
    if (sameDisk ? gameFree !== null && gameFree < gameNeed + jobNeed : (gameFree !== null && gameFree < gameNeed) || (jobFree !== null && jobFree < jobNeed))
      throw new DemoError("NO_SPACE");
  }

  async renderJob(request, active) {
    const { item, range, fps, audio, jobDir, signal, onProgress, ffmpegPath } = request;
    const aborted = () => {
      if (signal && signal.aborted) throw new DemoError("CANCELLED");
    };
    if (this.platform !== "win32") throw new DemoError("RENDER_NOT_READY");
    if (!this.script) throw new DemoError("RENDER_HOST_MISSING");
    if (!ffmpegPath) throw new DemoError("FFMPEG_MISSING");
    const install = this.installPath();
    if (!install) throw new DemoError("GAME_NOT_INSTALLED");
    const exe = path.join(install, EXE);
    if (!FPS.includes(fps)) throw new DemoError("COMBO_UNSUPPORTED");

    await this.preflight(exe);
    // Оба пути рендерят сегменты, задевающие интервал; точнее режет общий слой.
    const playlist = this.ghrenderPlaylist(install, item);
    const plan = sr.planSegments(item.segments, range, fps);
    if (!plan.picked.length) throw new DemoError("DEMO_EMPTY");
    await this.checkSegments(plan.picked);
    this.checkSpace(install, jobDir, plan, request, !!playlist);
    aborted();
    this.restorePrevious();
    if (playlist) {
      try {
        return await this.renderGhrender({ request, active, install, exe, playlist, plan });
      } catch (error) {
        if (error.code !== "GHRENDER_UNSUPPORTED") throw error;
        // Мод не ответил журналом -ghrender: игра уже закрыта, всё возвращено — один раз старым путём.
        this.logger.warn(`[demo] Ghosting -ghrender не ответил (${error.message}) — тот же экспорт старым путём`);
        // Время ушло и на неудачный запуск: скорость старого пути по этому заданию не замеряется.
        active.noTiming = true;
      }
      aborted();
      // Флаг mute первой попытки уже записан: подтверждённый сброс старого пути его снимет.
      if (active.muted) active.pendingBefore = true;
    }

    // Папка задания в моде: startdemos и startmovie видят только папку мода.
    const id8 = jobId8(jobDir);
    const rel = (name) => `${RENDER_DIR}/${id8}/${name}`;
    const video = path.join(jobDir, "video.mkv");
    const pump = { process: null, width: 0, height: 0, written: 0, lastPixels: null, heldFirst: null, exit: null, stderr: "",
      expected: { width: request.width, height: request.height } };
    active.pump = pump;
    const sound = { joiner: audio ? new sr.WavJoiner(path.join(jobDir, "audio.wav")) : null, lost: false };
    let completed = false;
    const onAbort = () => {
      if (active.host) active.host.stop(this.steamCode() === null);
      if (pump.process) try { pump.process.kill(); } catch (error) { /* уже вышел */ }
    };
    if (signal) signal.addEventListener("abort", onAbort, { once: true });

    try {
      this.snapshot(active);
      const modJobDir = gr.makeRenderDir(SPEC.renderRoot(install), id8);
      active.modJobDir = modJobDir;
      plan.picked.forEach((entry, k) => gr.linkOrCopy(entry.segment.path, path.join(modJobDir, `s${String(k + 1).padStart(3, "0")}.dem`)));
      const config = configFor(request, install, modJobDir);

      const nameLength = rel(`s${String(plan.picked.length).padStart(3, "0")}`).length;
      const groups = sr.batches(plan.picked.map((entry, k) => ({ entry, k })), Math.min(this.batchSize, sr.batchLimit(nameLength)));
      let launches = 0;
      for (let b = 0; b < groups.length; b++) {
        aborted();
        if (plan.framesNeeded !== null && pump.written >= plan.framesNeeded) break;
        launches++;
        // Steam могли закрыть посреди рендера: без него игра не стартует.
        const steam = b > 0 ? this.steamCode() : null;
        if (steam) throw new DemoError(steam);
        await this.runBatch({ b, last: b === groups.length - 1, group: groups[b], plan, request, install, exe, modJobDir, rel, video, sound, pump, active, ffmpegPath, onProgress, signal, config });
      }
      aborted();
      if (!pump.process) throw new DemoError("RENDER_FAILED", "Игра не дала ни одного кадра");
      // Весь материал — один кадр: придержанный первый кадр всё же пишется.
      if (pump.heldFirst === true) await this.writePixels(pump, pump.lastPixels);
      pump.process.stdin.end();
      const code = await pump.exit;
      if (code !== 0) throw new DemoError("FFMPEG_FAILED", pump.stderr.trim().slice(-500), { exitCode: code });
      let audioFile = null;
      if (sound.joiner && !sound.lost) audioFile = sound.joiner.close();
      else if (sound.joiner) {
        // Без звука (нет устройства вывода и т. п.): видео без звука, jobs.js предупредит.
        sound.joiner.close();
        fs.rmSync(sound.joiner.file, { force: true });
        this.logger.warn("[demo] Ghosting: игра не записала звук — экспорт без звука");
      }
      const expected = plan.framesNeeded !== null ? plan.framesNeeded : plan.framesTotal;
      if (plan.framesNeeded !== null ? pump.written < expected : Math.abs(pump.written - expected) > plan.picked.length + 2)
        this.logger.warn(`[demo] Ghosting: кадров ${pump.written}, ожидалось ${expected}`);
      this.logger.info(`[demo] Ghosting: ${pump.written} кадров ${pump.width}x${pump.height}@${fps}, сегментов ${plan.picked.length}, пачек ${groups.length}`);
      completed = true;
      return {
        video,
        audio: audioFile,
        offset: plan.offset,
        cuts: plan.cuts,
        timeline: sr.timelineOf(plan.picked, range),
        // Для оценки времени следующих заданий (estimate.js).
        profile: active.noTiming ? null : "ghosting-startdemos",
        launches,
        segments: plan.picked.length,
        frames: pump.written,
      };
    } finally {
      if (signal) signal.removeEventListener("abort", onAbort);
      if (active.host) await this.stopHost(active, true);
      if (pump.process && pump.process.exitCode === null) try { pump.process.kill(); } catch (error) { /* вышел */ }
      if (!completed && sound.joiner) try { sound.joiner.close(); } catch (error) { /* не важно: папку задания удалят */ }
      this.finish(active);
    }
  }

  // Режим -ghrender: client.dll мода знает и его, и диапазон записей (протокол 2),
  // ран — плейлист из demos/ мода в своей папке. Иначе — старый путь (startdemos, пачки).
  // quiet — без записи в журнал (оценка времени зовёт это при каждом выборе демки).
  ghrenderPlaylist(install, item, quiet) {
    const modDir = path.join(install, MOD);
    const caps = gh.clientCaps(modDir);
    if (!caps.ghrender) return null;
    const old = (why) => {
      if (!quiet) this.logger.info(`[demo] Ghosting: client.dll знает -ghrender, но ${why} — старый путь`);
      return null;
    };
    if (!caps.segments) return old("без -ghrender_segments (протокол 1)");
    if (item.kind !== "playlist" || !item.files) return old("это не плейлист рана");
    const playlist = gh.playlistArg(modDir, item.files[0]);
    if (!playlist) return old("плейлист не из demos/ мода");
    if (!gh.entriesInPlace(playlist, item.segments || [])) return old("папку рана переименовали или скопировали (пути в плейлисте другие)");
    if (!quiet) this.logger.info(`[demo] Ghosting: режим -ghrender, ${playlist}`);
    return playlist;
  }

  // Одним запуском: мод проигрывает плейлист игрока сам — весь или записи, которые
  // задевает интервал (-ghrender_segments); кадры f_NNNN.tga (появляются атомарно)
  // идут в насос, audio.wav — после DONE, без сдвига; таймлайн и вырезанное — из
  // журнала (entry= — номер записи всего плейлиста). Интервал и скорость режет jobs.js.
  async renderGhrender({ request, active, install, exe, playlist, plan }) {
    const { item, fps, width, height, audio, keyhud, integrity, jobDir, signal, onProgress, ffmpegPath } = request;
    const out = `${RENDER_DIR}/${jobId8(jobDir)}`;
    const segments = gh.segmentsArg(plan.picked, sr.planSegments(item.segments, null, fps).picked.length);
    const video = path.join(jobDir, "video.mkv");
    const pump = { process: null, width: 0, height: 0, written: 0, lastPixels: null, heldFirst: null, exit: null, stderr: "",
      expected: { width, height } };
    active.pump = pump;
    const onAbort = () => {
      if (active.host) active.host.stop(this.steamCode() === null);
      if (pump.process) try { pump.process.kill(); } catch (error) { /* уже вышел */ }
    };
    if (signal) signal.addEventListener("abort", onAbort, { once: true });
    try {
      this.snapshot(active);
      const modJobDir = gr.makeRenderDir(SPEC.renderRoot(install), jobId8(jobDir));
      active.modJobDir = modJobDir;
      const config = configFor(request, install, modJobDir);
      const unmuteAll = active.muted;
      const host = this.hostFactory({ script: this.script, env: this.env, logger: this.logger });
      active.host = host;
      this.watchAudio(host, active, unmuteAll);
      const pid = await host.start({
        exe,
        cwd: install,
        commandLine: sr.commandLine(exe, gh.gameArguments({ width, height, fps, keyhud, integrity, playlist, out, segments, config })),
        mute: true,
        unmuteAll,
        resetCommandLine: sr.commandLine(exe, resetArguments(`${out}/reset.log`)),
      });
      // Таймауты — от запуска игры, без старта помощника (PowerShell, Add-Type).
      const startedAt = Date.now();
      this.logger.info(`[demo] Ghosting -ghrender: PID ${pid}, ${width}x${height}@${fps}, записи ${segments || "все"}`);
      const journal = await this.watchGhrender({ host, modJobDir, pump, plan, fps, video, ffmpegPath, signal, onProgress, startedAt });
      // Игра после DONE выходит сама; не вышла — её закроет задание. Mute снимает перезапуск.
      for (const until = Date.now() + GHRENDER_EXIT_MS; host.exitCode === null && !host.closed && Date.now() < until;) await sleep(50);
      await this.stopHost(active, true);
      if (!pump.process) throw new DemoError("RENDER_FAILED", "Игра не дала ни одного кадра");
      pump.process.stdin.end();
      const code = await pump.exit;
      if (code !== 0) throw new DemoError("FFMPEG_FAILED", pump.stderr.trim().slice(-500), { exitCode: code });
      let audioFile = null;
      const wav = path.join(modJobDir, "audio.wav");
      if (audio && journal.done.audio === "audio.wav" && fs.existsSync(wav)) {
        audioFile = path.join(jobDir, "audio.wav");
        await moveFile(wav, audioFile);
      } else if (audio) this.logger.warn(`[demo] Ghosting -ghrender: звука нет (audio=${journal.done.audio}) — экспорт без звука`);
      if (Number(journal.done.frames) !== pump.written) this.logger.warn(`[demo] Ghosting -ghrender: кадров ${pump.written}, в журнале ${journal.done.frames}`);
      this.logger.info(`[demo] Ghosting -ghrender: ${pump.written} кадров, сегментов ${journal.done.segments}, data=${journal.done.data}, ${Math.round((Date.now() - startedAt) / 1000)} с`);
      let t = 0;
      const starts = item.segments.map((segment) => {
        const start = t;
        t += Number(segment.duration) > 0 ? Number(segment.duration) : 0;
        return start;
      });
      // Хвост после последнего кадра — до конца последней записи диапазона.
      const total = plan.picked[plan.picked.length - 1].end;
      const timeline = gh.timelineOf({ segments: journal.segments, timers: journal.timers, frames: pump.written, fps, starts, total });
      // Без якорей SEGMENT материал диапазона считался бы с начала рана — клип не из того места.
      if (segments && !timeline.markers.length) throw new DemoError("RENDER_FAILED", `В журнале -ghrender нет строк SEGMENT для записей ${segments}`);
      return { video, audio: audioFile, offset: timeline.offset, cuts: timeline.cuts, timeline: gh.withRangeStart(timeline.markers, request.range),
        profile: "ghosting-ghrender", launches: 1, segments: plan.picked.length, frames: pump.written };
    } finally {
      if (signal) signal.removeEventListener("abort", onAbort);
      if (active.host) await this.stopHost(active, true);
      if (pump.process && pump.process.exitCode === null) try { pump.process.kill(); } catch (error) { /* вышел */ }
      this.finish(active);
    }
  }

  // Журнал ghrender.log и кадры: первая строка — gh.PROTOCOL, конец — DONE и
  // все его кадры; FAIL — код лаунчера. -> { first, done, segments, timers }.
  async watchGhrender({ host, modJobDir, pump, plan, fps, video, ffmpegPath, signal, onProgress, startedAt }) {
    const logFile = path.join(modJobDir, "ghrender.log");
    const journal = { first: null, done: null, segments: [], timers: [] };
    let logPos = 0;
    let rest = "";
    let failure = null;
    let frame = 0;
    let segment = null;
    let lastActivity = startedAt;
    let lastReport = 0;
    const readLog = () => {
      let fd;
      try {
        fd = fs.openSync(logFile, "r");
      } catch (error) {
        return;
      }
      try {
        const size = fs.fstatSync(fd).size;
        if (size <= logPos) return;
        const chunk = Buffer.alloc(Math.min(size - logPos, 1024 * 1024));
        const read = fs.readSync(fd, chunk, 0, chunk.length, logPos);
        logPos += read;
        const lines = (rest + chunk.subarray(0, read).toString("latin1")).split("\n");
        rest = lines.pop();
        // Строка без конца не копится: журнал короткими строками пишет мод, длинная — мусор.
        if (rest.length > MAX_JOURNAL_LINE) rest = "";
        lastActivity = Date.now();
        const count = plan.picked.length;
        for (const line of lines) {
          const parsed = line.length <= MAX_JOURNAL_LINE ? gh.parseLine(line) : null;
          if (!parsed) continue;
          if (journal.first === null) {
            journal.first = parsed.text;
            // Другой протокол — не наш формат журнала: renderJob закроет игру и пойдёт старым путём.
            if (parsed.text !== gh.PROTOCOL && !failure) failure = new DemoError("GHRENDER_UNSUPPORTED", parsed.text);
            continue;
          }
          if (parsed.key === "SEGMENT") {
            // Записей не больше, чем сегментов в плане: мод показывает каждый один раз.
            if (journal.segments.length >= count) continue;
            journal.segments.push(parsed.fields);
            const n = Number(parsed.fields.n);
            segment = { index: Number.isInteger(n) && n >= 1 && n <= count ? n : journal.segments.length, count };
          } else if (parsed.key === "TIMER" && journal.timers.length < count * 4 + 16) journal.timers.push(parsed.fields);
          else if (parsed.key === "WARN") this.logger.warn(`[demo] Ghosting -ghrender: ${parsed.text}`);
          else if (parsed.key === "DONE") journal.done = parsed.fields;
          else if (parsed.key === "FAIL" && !failure) failure = gh.failError(parsed);
        }
      } finally {
        fs.closeSync(fd);
      }
    };
    const report = (force) => {
      const now = Date.now();
      if (!onProgress || (!force && now - lastReport < 200)) return;
      lastReport = now;
      onProgress({ fraction: Math.min(0.999, pump.written / Math.max(1, plan.framesTotal)), segment, message: null });
    };
    for (;;) {
      if (signal && signal.aborted) throw new DemoError("CANCELLED");
      readLog();
      if (failure) throw failure;
      let consumed = false;
      while (fs.existsSync(frameName(modJobDir, frame))) {
        await this.consumeReady(pump, frameName(modJobDir, frame), fps, video, ffmpegPath);
        frame++;
        consumed = true;
        lastActivity = Date.now();
        report(false);
        if (signal && signal.aborted) throw new DemoError("CANCELLED");
      }
      const now = Date.now();
      if (journal.done) {
        if (!(frame < Number(journal.done.frames))) break;
        // Кадры появляются атомарно; DONE раньше последнего кадра — подождать недолго.
        if (now - lastActivity > GHRENDER_EXIT_MS) {
          this.logger.warn(`[demo] Ghosting -ghrender: DONE frames=${journal.done.frames}, а кадров ${frame}`);
          break;
        }
      } else {
        if (journal.first === null && now - startedAt > this.ghrenderFirstLineMs) throw new DemoError("GHRENDER_UNSUPPORTED", "Нет журнала ghrender.log");
        if (host.exitCode !== null) {
          readLog();
          if (failure) throw failure;
          // Вышла, не написав ни строки (у мода код 2: журнал не создать): как молчание — откат.
          if (journal.first === null) throw new DemoError("GHRENDER_UNSUPPORTED", `Игра вышла без журнала (код ${host.exitCode})`);
          if (!journal.done) throw new DemoError("GAME_EXITED", null, { exitCode: host.exitCode });
          continue;
        }
        if (host.closed) throw new DemoError("RENDER_HOST_FAILED", "Помощник вышел");
        if (now - lastActivity > this.ghrenderStallMs)
          throw new DemoError("RENDER_TIMEOUT", null, { framesDone: pump.written, framesTotal: plan.framesTotal });
      }
      if (!consumed) await sleep(POLL_MS);
    }
    report(true);
    return journal;
  }

  // Готовый кадр -ghrender (мод кладёт его переименованием): в насос и удалить.
  async consumeReady(pump, file, fps, video, ffmpegPath) {
    let buffer = null;
    // Кадр может быть ненадолго занят (антивирус проверяет новый файл).
    for (let attempt = 0; !buffer; attempt++) {
      try {
        buffer = await fs.promises.readFile(file);
      } catch (error) {
        if (attempt >= 40) throw new DemoError("RENDER_FAILED", `Кадр ${path.basename(file)} не читается: ${error.code}`);
        await sleep(25);
      }
    }
    const info = sr.tgaInfo(buffer);
    if (!info || !info.complete) throw new DemoError("RENDER_FAILED", `Кадр ${path.basename(file)} не читается`);
    await this.pumpFrame(pump, file, buffer, info, false, fps, video, ffmpegPath);
  }

  startPump(pump, info, fps, video, ffmpegPath) {
    const args = ["-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", info.pixFmt, "-s", `${info.width}x${info.height}`,
      "-framerate", String(fps), "-i", "pipe:0"];
    // TGA игры — снизу вверх.
    if (!info.topDown) args.push("-vf", "vflip");
    args.push("-c:v", "libx264", "-preset", "ultrafast", "-crf", "14", "-pix_fmt", "yuv420p", "-y", video);
    const child = this.spawnFn(ffmpegPath, args, { windowsHide: true, shell: false, stdio: ["pipe", "ignore", "pipe"] });
    pump.process = child;
    pump.width = info.width;
    pump.height = info.height;
    pump.pixFmt = info.pixFmt;
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      pump.stderr = (pump.stderr + chunk).slice(-4000);
    });
    child.stdin.on("error", () => {});
    pump.exit = new Promise((resolve) => {
      child.on("error", (error) => {
        pump.stderr += error.message;
        resolve(-1);
      });
      child.on("close", (code) => resolve(code));
    });
  }

  async writePixels(pump, pixels) {
    if (pump.process.exitCode !== null || pump.process.stdin.destroyed)
      throw new DemoError("FFMPEG_FAILED", pump.stderr.trim().slice(-500));
    if (!pump.process.stdin.write(pixels))
      await new Promise((resolve) => {
        const done = () => {
          pump.process.stdin.off("drain", done);
          pump.process.off("close", done);
          resolve();
        };
        pump.process.stdin.once("drain", done);
        pump.process.once("close", done);
      });
    pump.written++;
  }

  // Кадр игры -> FFmpeg, файл кадра удаляется сразу. join: кадр — первый
  // после загрузки сегмента (переходный): вместо него ещё раз последний
  // кадр прошлого сегмента, как gh_demo_seamless; самый первый кадр видео
  // заменяется следующим. Число кадров не меняется — звук и таймлайн целы.
  async pumpFrame(pump, file, buffer, info, join, fps, video, ffmpegPath) {
    if (!pump.process) {
      // Окно больше рабочего стола игра обрезает (масштаб экрана, 4K на
      // меньшем мониторе): растягивать молча не будем — понятная ошибка.
      if (info.width !== pump.expected.width || info.height !== pump.expected.height)
        throw new DemoError("RESOLUTION_LIMITED", `Игра дала кадр ${info.width}x${info.height}`, { width: info.width, height: info.height });
      this.startPump(pump, info, fps, video, ffmpegPath);
    } else if (info.width !== pump.width || info.height !== pump.height || info.pixFmt !== pump.pixFmt)
      throw new DemoError("RENDER_FAILED", `Кадр ${info.width}x${info.height} вместо ${pump.width}x${pump.height}`);
    const pixels = buffer.subarray(info.offset, info.offset + info.bytes);
    if (join && !pump.lastPixels && pump.written === 0 && !pump.heldFirst) pump.heldFirst = true;
    else if (pump.heldFirst === true) {
      await this.writePixels(pump, pixels);
      pump.heldFirst = false;
      await this.writePixels(pump, pixels);
    } else await this.writePixels(pump, join && pump.lastPixels ? pump.lastPixels : pixels);
    pump.lastPixels = pixels;
    for (let attempt = 0; attempt < 20; attempt++) {
      try {
        await fs.promises.unlink(file);
        break;
      } catch (error) {
        if (error.code === "ENOENT") break;
        await sleep(25);
      }
    }
  }

  async runBatch(ctx) {
    const { b, last, group, plan, request, install, exe, modJobDir, rel, video, sound, pump, active, ffmpegPath, onProgress, signal, config } = ctx;
    const { fps, width, height, audio, keyhud, integrity } = request;
    const demos = group.map(({ k }) => rel(`s${String(k + 1).padStart(3, "0")}`));
    const keys = demos.map(sr.demoKey);
    const logName = `console-${b + 1}.log`;
    const logFile = path.join(modJobDir, logName);
    const args = sr.gameArguments({ width, height, fps, keyhud, integrity, audio, demos, movie: rel("f_"), logFile: rel(logName), config });
    const resetArgs = resetArguments(rel("reset.log"));
    // Mute прошлой пачки или прошлого сбоя: заглушённые до нас сессии — тоже наши.
    const unmuteAll = active.muted;
    const host = this.hostFactory({ script: this.script, env: this.env, logger: this.logger });
    active.host = host;
    this.watchAudio(host, active, unmuteAll);
    const batchStart = pump.written;
    const batchFrames = group.reduce((sum, { entry }) => sum + entry.frames, 0);
    const startedAt = Date.now();
    const pid = await host.start({
      exe,
      cwd: install,
      commandLine: sr.commandLine(exe, args),
      mute: true,
      unmuteAll,
      resetCommandLine: sr.commandLine(exe, resetArgs),
    });
    this.logger.info(`[demo] Ghosting: пачка ${b + 1}, сегментов ${group.length}, PID ${pid}, ${width}x${height}@${fps}`);

    const frameDir = modJobDir;
    let frame = 0;
    let firstSeen = 0;
    let endAt = null;
    let failure = null;
    let lastFrameAt = startedAt;
    let pendingJoin = true;
    let prevMtime = null;
    let prevInterval = 0;
    let logPos = 0;
    let logRest = "";
    let segment = null;
    let lastReport = 0;
    const report = (force) => {
      const now = Date.now();
      if (!onProgress || (!force && now - lastReport < 200)) return;
      lastReport = now;
      onProgress({ fraction: Math.min(0.999, pump.written / Math.max(1, plan.framesNeeded || plan.framesTotal)), segment, message: null });
    };
    const readLog = () => {
      let fd;
      try {
        fd = fs.openSync(logFile, "r");
      } catch (error) {
        return;
      }
      try {
        const size = fs.fstatSync(fd).size;
        if (size <= logPos) return;
        const chunk = Buffer.alloc(Math.min(size - logPos, 1024 * 1024));
        const read = fs.readSync(fd, chunk, 0, chunk.length, logPos);
        logPos += read;
        const text = logRest + chunk.subarray(0, read).toString("utf8");
        const lines = text.split(/\n/);
        logRest = lines.pop();
        for (const line of lines) {
          const parsed = sr.parseLogLine(line);
          if (!parsed) continue;
          if (parsed.kind === "error") {
            if (!failure) failure = new DemoError(parsed.code, parsed.text);
            continue;
          }
          const index = keys.indexOf(parsed.demo);
          if (index < 0) continue;
          if (index === 0 && ++firstSeen === 2) {
            // startdemos пошёл на второй круг: идёт загрузка, кадров нет.
            if (endAt === null) endAt = Date.now();
            continue;
          }
          if (endAt !== null) continue;
          if (index > 0) pendingJoin = true;
          segment = { index: group[index].k + 1, count: plan.picked.length };
          report(true);
        }
      } finally {
        fs.closeSync(fd);
      }
    };

    const consume = async (index, final) => {
      const file = frameName(frameDir, index);
      // Асинхронно: ~6 МБ на кадр 1080p, 40+ кадров в секунду — главный процесс не блокируем.
      let buffer;
      let mtime;
      // Кадр может быть ненадолго занят (антивирус проверяет новый файл).
      for (let attempt = 0; !buffer; attempt++) {
        try {
          const handle = await fs.promises.open(file, "r");
          try {
            mtime = (await handle.stat()).mtimeMs;
            buffer = await handle.readFile();
          } finally {
            await handle.close();
          }
        } catch (error) {
          if (final) return false;
          if (attempt >= 40) throw new DemoError("RENDER_FAILED", `Кадр ${path.basename(file)} не читается: ${error.code}`);
          await sleep(25);
        }
      }
      const info = sr.tgaInfo(buffer);
      if (!info || !info.complete) {
        if (final) return false;
        throw new DemoError("RENDER_FAILED", `Кадр ${path.basename(file)} не читается`);
      }
      // Кадр записан уже после конца — это повтор первого сегмента или хвост.
      if (final && endAt !== null && mtime > endAt) return false;
      const interval = prevMtime === null ? 0 : mtime - prevMtime;
      const gap = prevMtime === null || interval > Math.max(JOIN_GAP_MS, prevInterval * 4);
      const join = pendingJoin && gap;
      if (join) pendingJoin = false;
      if (prevMtime !== null) prevInterval = interval;
      prevMtime = mtime;
      await this.pumpFrame(pump, file, buffer, info, join, fps, video, ffmpegPath);
      return true;
    };

    let enough = false;
    try {
      for (;;) {
        if (signal && signal.aborted) throw new DemoError("CANCELLED");
        readLog();
        if (failure) throw failure;
        // Интервал кончился раньше рендера: кадров хватает, дальше не ждём.
        if (endAt === null && plan.framesNeeded !== null && pump.written >= plan.framesNeeded) {
          endAt = Date.now();
          enough = true;
        }
        if (endAt !== null) break;
        if (host.exitCode !== null) throw new DemoError("GAME_EXITED", null, { exitCode: host.exitCode });
        if (host.closed) throw new DemoError("RENDER_HOST_FAILED", "Помощник вышел");
        if (fs.existsSync(frameName(frameDir, frame + 1))) {
          await consume(frame, false);
          frame++;
          lastFrameAt = Date.now();
          report(false);
          continue;
        }
        const now = Date.now();
        if (now - lastFrameAt > (frame === 0 ? this.firstFrameTimeoutMs : this.frameTimeoutMs))
          throw new DemoError("RENDER_TIMEOUT", null, { framesDone: pump.written, framesTotal: plan.framesTotal });
        // Журнал молчит, а кадров уже с запасом больше ожидаемого: конец по кадрам.
        if (frame >= batchFrames + group.length) endAt = now;
        await sleep(POLL_MS);
      }
      // Игра закрывается заглушённой; mute снимается только в конце рендера
      // (последняя пачка или кадров уже хватает), между пачками — не нужно.
      await this.stopHost(active, last || enough);
      // Остаток пачки: дописанные до конца кадры.
      while (fs.existsSync(frameName(frameDir, frame)) && (await consume(frame, true))) frame++;
    } finally {
      if (active.host === host) await this.stopHost(active, true);
      for (const name of fs.existsSync(frameDir) ? fs.readdirSync(frameDir) : [])
        if (/^f_\d+\.tga$/i.test(name)) fs.rmSync(path.join(frameDir, name), { force: true });
    }
    const written = pump.written - batchStart;
    if (sound.joiner && !sound.lost) {
      const wav = path.join(modJobDir, "f_.wav");
      if (!fs.existsSync(wav)) sound.lost = true;
      else {
        try {
          // Опережение микшера — по формуле (snd_mixahead зафиксирован): длина
          // WAV в момент закрытия зависит от того, как быстро игру закрыли.
          const offset = await sound.joiner.append(wav, written, fps, sr.mixerLead(fps));
          this.logger.info(`[demo] Ghosting: пачка ${b + 1}: кадров ${written} (ожидалось ${batchFrames}), сдвиг звука ${offset} с, ${Math.round((Date.now() - startedAt) / 1000)} с`);
        } catch (error) {
          throw error.name === "DemoError" ? error : new DemoError(error.code || "AUDIO_INVALID", error.message);
        }
        fs.rmSync(wav, { force: true });
      }
    }
  }
}

module.exports = {
  GAME_ID,
  REGISTRY_KEY,
  MARKER,
  REG_FILE,
  UNMUTE_PENDING,
  RENDER_DIR,
  GhostingAdapter,
  regTool: gr.regTool,
  readMarker,
  snapshotRegistry,
  restoreRegistry,
  cleanupRenderDirs,
  maxHeightFor: gr.maxHeightFor,
  waitDead: gr.waitDead,
};
