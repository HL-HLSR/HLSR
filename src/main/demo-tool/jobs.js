"use strict";

const fs = require("fs");
const path = require("path");

const { DemoError } = require("./errors");
const planner = require("./planner");
const ffmpeg = require("./ffmpeg");
const { isInside, removeWithinZone } = require("../game-delivery/paths");

// Задание экспорта или предпросмотра: адаптер рендерит сырьё в папку
// задания, FFmpeg кодирует MP4 (H.264 + AAC, +faststart), итог проверяется
// (размер для Discord, ffprobe) и сохраняется рядом с timeline.json.
// Папка задания удаляется всегда, кроме готового предпросмотра.
//
// Корень заданий — в данных пользователя лаунчера (AppManager: Windows —
// %LOCALAPPDATA%\hlsr\demo-jobs, Linux — ~/.config/hlsr/demo-jobs), не в
// общем /tmp: там чужой пользователь мог бы подложить папку или ссылку.

const JOBS_DIR = "demo-jobs";
const STALE_MS = 24 * 60 * 60 * 1000;
const PHASES = { prepare: [0, 2], render: [2, 50], encode: [50, 95], verify: [95, 100], done: [100, 100] };
const round3 = (value) => Math.round(value * 1000) / 1000;

// Корень можно использовать: настоящая папка (не ссылка и не junction), на
// Linux — наша и только для нас (0700). Иначе — JOBS_DIR_UNSAFE.
function checkRoot(root, options) {
  const opts = options || {};
  const platform = opts.platform || process.platform;
  let stat;
  try {
    stat = fs.lstatSync(root);
  } catch (error) {
    throw new DemoError("JOBS_DIR_UNAVAILABLE", error.message);
  }
  if (stat.isSymbolicLink() || !stat.isDirectory()) throw new DemoError("JOBS_DIR_UNSAFE", `${root}: ссылка или не папка`);
  if (platform !== "win32") {
    const uid = opts.uid === undefined ? process.getuid() : opts.uid;
    if (stat.uid !== uid) throw new DemoError("JOBS_DIR_UNSAFE", `${root}: владелец ${stat.uid}, а не ${uid}`);
    if ((stat.mode & 0o077) !== 0) fs.chmodSync(root, 0o700);
  }
  return root;
}

function prepareRoot(root, options) {
  if (!root) throw new DemoError("JOBS_DIR_UNAVAILABLE", "Корень заданий не задан");
  try {
    fs.mkdirSync(root, { recursive: true, mode: 0o700 });
  } catch (error) {
    throw new DemoError(error.code === "ENOSPC" ? "NO_SPACE" : "JOBS_DIR_UNAVAILABLE", error.message);
  }
  return checkRoot(root, options);
}

// Остатки прошлых запусков старше суток — только внутри корня заданий и без
// перехода по ссылкам: ссылка снимается сама, папка удаляется rmSync,
// который по ссылкам внутри не заходит.
function cleanupStale(root, now, maxAgeMs, options) {
  const limit = maxAgeMs === undefined ? STALE_MS : maxAgeMs;
  if (!root || !fs.existsSync(root)) return 0;
  checkRoot(root, options);
  let entries;
  try {
    entries = fs.readdirSync(root);
  } catch (error) {
    return 0;
  }
  let removed = 0;
  for (const name of entries) {
    const full = path.join(root, name);
    try {
      const stat = fs.lstatSync(full);
      // Файлы корня — маркер и снимок реестра рендера Ghosting (ghosting.js): их
      // удаляет только удачный возврат реестра, чистка по возрасту не трогает.
      if (stat.isFile()) continue;
      if ((now || Date.now()) - stat.mtimeMs < limit) continue;
      if (stat.isSymbolicLink()) {
        try { fs.unlinkSync(full); } catch (error) { fs.rmdirSync(full); }
      } else removeWithinZone(root, full);
      removed++;
    } catch (error) {
      /* занято — уберётся в следующий раз */
    }
  }
  return removed;
}

function removeJobDir(root, dir) {
  try {
    removeWithinZone(root, dir);
  } catch (error) {
    /* уберётся чисткой старых папок */
  }
}

function safeBaseName(name) {
  let base = String(name || "")
    .normalize("NFC")
    // eslint-disable-next-line no-control-regex
    .replace(/[<>:"/\\|?*\x00-\x1f\x7f]/g, "_")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[. ]+$/, "");
  if (!base) base = "demo";
  if (/^(con|prn|aux|nul|conin\$|conout\$|com[0-9¹²³]|lpt[0-9¹²³])(\..*)?$/i.test(base)) base = `_${base}`;
  return base.slice(0, 120).replace(/[. ]+$/, "") || "demo";
}

function clock(seconds) {
  const whole = Math.floor(seconds);
  return `${String(Math.floor(whole / 60)).padStart(2, "0")}-${String(whole % 60).padStart(2, "0")}`;
}

function outputBaseName(item, settings) {
  const stem = String(item.name || "demo").replace(/\.(dem|ghplaylist)$/i, "");
  let base = stem;
  const full = item.duration && settings.range.from <= 0.001 && settings.range.to >= item.duration - 0.001;
  if (!full) base += `_${clock(settings.range.from)}_${clock(settings.range.to)}`;
  if (settings.speed !== 1) base += `_x${settings.speed}`;
  if (settings.preset === "discord") base += "_discord";
  return safeBaseName(base);
}

// Имя без перезаписи: «имя», «имя (2)», … — и для MP4, и для timeline.json.
function saveOutput(outputDir, base, file, timeline) {
  try {
    fs.mkdirSync(outputDir, { recursive: true });
  } catch (error) {
    throw new DemoError("OUTPUT_DIR_UNAVAILABLE", error.message);
  }
  const failed = (error) => new DemoError(error.code === "ENOSPC" ? "NO_SPACE" : "OUTPUT_WRITE_FAILED", error.message);
  for (let n = 1; n < 1000; n++) {
    const name = n === 1 ? base : `${base} (${n})`;
    const video = path.join(outputDir, `${name}.mp4`);
    const json = path.join(outputDir, `${name}.timeline.json`);
    if (fs.existsSync(json)) continue;
    try {
      fs.copyFileSync(file, video, fs.constants.COPYFILE_EXCL);
    } catch (error) {
      if (error.code === "EEXIST") continue;
      throw failed(error);
    }
    const data = Object.assign({}, timeline, { video: Object.assign({}, timeline.video, { file: `${name}.mp4` }) });
    try {
      fs.writeFileSync(json, JSON.stringify(data, null, 2) + "\n", { flag: "wx" });
    } catch (error) {
      fs.rmSync(video, { force: true });
      if (error.code === "EEXIST") continue;
      throw failed(error);
    }
    return { file: video, timelineFile: json, timeline: data };
  }
  throw new DemoError("OUTPUT_WRITE_FAILED", "Свободное имя не найдено");
}

function reporter(onProgress) {
  let lastAt = 0;
  let lastKey = "";
  return (phase, fraction, extra) => {
    if (!onProgress) return;
    const [low, high] = PHASES[phase];
    const share = Math.max(0, Math.min(1, Number(fraction) || 0));
    const percent = Math.round(low + (high - low) * share);
    const segment = (extra && extra.segment) || null;
    const key = `${phase}:${segment ? segment.index : ""}`;
    const now = Date.now();
    if (key === lastKey && share < 1 && now - lastAt < 250) return;
    lastAt = now;
    lastKey = key;
    onProgress({ phase, percent, segment, message: (extra && extra.message) || null });
  };
}

// Сырьё адаптера — только файлы внутри папки задания.
function checkMaterial(material, jobDir) {
  const inside = (file) => typeof file === "string" && isInside(jobDir, file) && path.resolve(file) !== path.resolve(jobDir);
  if (!material || typeof material !== "object") throw new DemoError("RENDER_FAILED", "Адаптер не вернул материал");
  if (material.pattern) {
    if (!inside(material.pattern) || !(material.fps > 0)) throw new DemoError("RENDER_FAILED", "Кадры вне папки задания");
  } else if (!inside(material.video) || !fs.existsSync(material.video)) throw new DemoError("RENDER_FAILED", "Нет видео адаптера");
  if (material.audio && (!inside(material.audio) || !fs.existsSync(material.audio)))
    throw new DemoError("RENDER_FAILED", "Нет звука адаптера");
  // Оверлей — имя файла в папке задания (FFmpeg запускается с cwd в ней).
  if (material.overlay && (!/^[\w.-]+\.ass$/i.test(material.overlay) || !fs.existsSync(path.join(jobDir, material.overlay))))
    throw new DemoError("RENDER_FAILED", "Нет оверлея адаптера");
}

// Проверка итога по самому видеопотоку: кодек, длительность, FPS и размер.
// Длительность файла не годится — apad дотягивает звук, и обрезанное видео
// (адаптер отдал меньше кадров) прошло бы с застывшим кадром.
async function verifyOutput({ ffprobePath, file, plan, withAudio, ffprobeFn, signal }) {
  if (!ffprobePath) return null;
  let info;
  try {
    info = await ffprobeFn(ffprobePath, file, { signal });
  } catch (error) {
    if (error.code === "CANCELLED") throw error;
    // Linux: ffprobe может не быть в PATH — тогда проверка только по размеру.
    if (error.code === "FFMPEG_MISSING") return null;
    throw new DemoError("VERIFY_FAILED", error.message);
  }
  const streams = (info && info.streams) || [];
  const video = streams.find((stream) => stream.codec_type === "video");
  const audio = streams.find((stream) => stream.codec_type === "audio");
  const duration = Number(video && video.duration);
  const tolerance = Math.max(0.5, plan.outputDuration * 0.02);
  const problems = [];
  if (!video || video.codec_name !== "h264") problems.push("video-codec");
  if (withAudio && (!audio || audio.codec_name !== "aac")) problems.push("audio-codec");
  if (video && !(Math.abs(duration - plan.outputDuration) <= tolerance)) problems.push(`video-duration ${duration}`);
  if (video && video.avg_frame_rate !== `${plan.fps}/1`) problems.push(`fps ${video.avg_frame_rate}`);
  if (video && (video.width !== plan.width || video.height !== plan.height)) problems.push(`size ${video.width}x${video.height}`);
  if (problems.length) throw new DemoError("VERIFY_FAILED", `ffprobe: ${problems.join(", ")}`);
  return { duration: round3(duration), width: video.width, height: video.height, frameRate: video.avg_frame_rate };
}

async function runJob(options) {
  const opts = options;
  const { id, kind, item, adapter, settings, root, signal } = opts;
  const runFn = opts.runFn || ffmpeg.run;
  const ffprobeFn = opts.ffprobeFn || ffmpeg.ffprobe;
  const logger = opts.logger || { info() {}, warn() {} };
  const platform = opts.platform || process.platform;
  const report = reporter(opts.onProgress);
  const jobDir = path.join(root, id);
  const aborted = () => {
    if (signal && signal.aborted) throw new DemoError("CANCELLED");
  };
  let keep = false;

  prepareRoot(root);
  // Не recursive: уже существующая папка с этим именем — не наша.
  try {
    fs.mkdirSync(jobDir, { mode: 0o700 });
  } catch (error) {
    throw new DemoError(error.code === "ENOSPC" ? "NO_SPACE" : "JOBS_DIR_UNAVAILABLE", error.message);
  }
  try {
    report("prepare", 0);
    const range = settings.range;
    const renderStarted = Date.now();
    const material = await adapter.render({
      item,
      segments: item.segments,
      range,
      fps: settings.fps,
      width: settings.width,
      height: settings.height,
      audio: settings.audio,
      keyhud: settings.keyhud,
      integrity: settings.integrity,
      hud: settings.hud,
      ownConfig: settings.ownConfig,
      ownHud: settings.ownHud,
      jobDir,
      ffmpegPath: opts.ffmpegPath,
      ffprobePath: opts.ffprobePath,
      signal,
      onProgress: (info) => report("render", info && info.fraction, info),
    });
    const renderSeconds = (Date.now() - renderStarted) / 1000;
    aborted();
    checkMaterial(material, jobDir);

    const cuts = planner.normalizeCuts(material.cuts);
    const timing = { offset: Number.isFinite(material.offset) ? material.offset : range.from, cuts };
    const start = planner.materialTime(range.from, timing);
    const length = planner.materialTime(range.to, timing) - start;
    // Вырезанное адаптером накрыло весь отрезок — кодировать нечего.
    if (!(length / settings.speed >= 0.1)) throw new DemoError("RENDER_FAILED", "Материал короче 0,1 с после вырезанного");
    const withAudio = !!(settings.audio && material.audio);
    const warnings = (opts.plan.warnings || []).slice();
    if (settings.audio && !material.audio) warnings.push("AUDIO_UNAVAILABLE");
    // Предупреждения адаптера (HL1: KeyHUD без данных в части демки).
    for (const code of Array.isArray(material.warnings) ? material.warnings : [])
      if (typeof code === "string" && /^[A-Z_]{1,40}$/.test(code) && !warnings.includes(code)) warnings.push(code);
    let plan = planner.planExport(Object.assign({}, settings, { audio: withAudio }), {
      encoder: opts.plan.encoder,
      outputDuration: round3(length / settings.speed),
      limitBytes: opts.plan.limitBytes || undefined,
      warnings,
    });

    const output = path.join(jobDir, "output.mp4");
    const encodeStarted = Date.now();
    const passes = plan.twoPass ? [1, 2] : [0];
    const nullTarget = platform === "win32" ? "NUL" : "/dev/null";
    let attempt = 0;
    let size = 0;
    let encodeSeconds = 0;
    for (;;) {
      for (let i = 0; i < passes.length; i++) {
        const args = planner.encodeArgs({
          material,
          trim: { start, duration: length },
          plan,
          output,
          pass: passes[i],
          passlog: path.join(jobDir, "pass"),
          nullTarget,
        });
        await runFn(opts.ffmpegPath, args, {
          signal,
          cwd: jobDir,
          onSpawn: opts.onSpawn,
          onProgress: (seconds) => report("encode", (i + Math.min(1, seconds / plan.outputDuration)) / passes.length,
            { message: attempt ? `retry-${attempt}` : null }),
        });
      }
      aborted();
      size = fs.statSync(output).size;
      encodeSeconds = (Date.now() - encodeStarted) / 1000;
      if (plan.preset !== "discord" || size <= plan.limitBytes) break;
      attempt++;
      const videoBps = planner.discordRetry(plan, size, attempt);
      logger.info(`[demo] ${id}: ${size} байт больше лимита ${plan.limitBytes}, повтор ${attempt} с ${videoBps} бит/с`);
      plan = Object.assign({}, plan, { videoBps });
    }

    // Итог закодирован окончательно (повторов Discord больше не будет): сырьё адаптера
    // (video.mkv — сотни МБ и гигабайты, WAV) и журналы проходов больше не нужны,
    // а предпросмотр держит папку, пока открыт.
    for (const name of fs.readdirSync(jobDir))
      if (name !== path.basename(output)) fs.rmSync(path.join(jobDir, name), { recursive: true, force: true });

    report("verify", 0);
    const probed = await verifyOutput({ ffprobePath: opts.ffprobePath, file: output, plan, withAudio, ffprobeFn, signal });
    aborted();

    const timeline = {
      version: 1,
      source: {
        // id записи в списке «Демо»: по нему вкладка «Отрендеренные» находит исходную демку.
        id: typeof item.id === "string" ? item.id : null,
        files: item.sourceFiles || (item.segments || []).map((segment) => segment.name),
        engine: item.engine,
        game: item.game,
        map: item.map || null,
      },
      video: {
        file: path.basename(output),
        duration: probed ? probed.duration : plan.outputDuration,
        fps: plan.fps,
        width: plan.width,
        height: plan.height,
        speed: plan.speed,
        range: { from: range.from, to: range.to },
        encoder: plan.encoder,
      },
      // extraMarkers — метки поверх меток адаптера (главы и финиш Source), тоже во времени демо.
      markers: planner.mapTimeline((material.timeline || []).concat(opts.extraMarkers || []),
        { from: range.from, to: range.to, speed: settings.speed, offset: timing.offset, cuts }),
    };
    const result = {
      kind,
      size,
      attempts: attempt + 1,
      encoder: plan.encoder,
      videoBps: plan.videoBps,
      warnings: plan.warnings,
      // Замер для оценки времени следующих заданий (estimate.js learn): профиль и запуски игры — от адаптера.
      stats: {
        profile: typeof material.profile === "string" ? material.profile : null,
        launches: material.launches || 1,
        segments: material.segments || null,
        frames: Number.isFinite(material.frames) ? material.frames : Math.round(length * settings.fps),
        renderSeconds,
        height: settings.height,
        preset: plan.preset,
        gpu: plan.encoder !== planner.CPU_ENCODER,
        outputFrames: Math.round(plan.outputDuration * plan.fps) * (attempt + 1),
        encodeSeconds,
      },
    };
    if (kind === "preview") {
      keep = true;
      Object.assign(result, { file: output, dir: jobDir, timelineFile: null, timeline });
    } else Object.assign(result, saveOutput(opts.outputDir, outputBaseName(item, settings), output, timeline));
    report("done", 1);
    return result;
  } finally {
    if (!keep) removeJobDir(root, jobDir);
  }
}

module.exports = { JOBS_DIR, STALE_MS, checkRoot, prepareRoot, cleanupStale, removeJobDir, safeBaseName, outputBaseName, saveOutput, runJob };
