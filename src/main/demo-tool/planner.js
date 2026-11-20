"use strict";

const { DemoError } = require("./errors");

// Планировщик экспорта: только чистые функции (сочетания, битрейт, фильтры,
// аргументы FFmpeg, пересчёт таймлайна). Процессы запускает jobs.js.

const RESOLUTIONS = [
  { height: 720, width: 1280, label: "720p" },
  { height: 1080, width: 1920, label: "1080p" },
  { height: 1440, width: 2560, label: "1440p" },
  { height: 2160, width: 3840, label: "4K" },
];
const FPS = [30, 60, 120];
// Пресеты окна экспорта; "preview" подставляет только startPreview.
const PRESETS = ["quality", "discord"];
const ENCODER_MODES = ["auto", "gpu", "cpu"];
// Порядок «Авто»: первый рабочий GPU, иначе процессор.
const GPU_ENCODERS = ["h264_nvenc", "h264_amf", "h264_qsv"];
const CPU_ENCODER = "libx264";
const SPEED = { min: 0.25, max: 4 };
// integrity — надпись проверки данных демки поверх кадра («GH DEMO DATA: INTACT» у Ghosting).
// hud — HUD игры в кадре; ownConfig — конфиг игрока (иначе стандартный); ownHud — HUD из
// «Персонализации» (иначе стандартный). Что из этого умеет игра — capabilities адаптера.
const DEFAULTS = Object.freeze({ preset: "quality", height: 1080, fps: 60, audio: true, keyhud: false, integrity: false, encoder: "auto", speed: 1,
  hud: true, ownConfig: true, ownHud: true });
const PREVIEW = Object.freeze({ preset: "preview", height: 720, fps: 30, audio: true, keyhud: false, integrity: false, speed: 1,
  hud: true, ownConfig: true, ownHud: true });
// Настройки вида в кадре: переключатели «Дополнительных настроек» предпросмотра и экспорта.
const VIEW_KEYS = ["keyhud", "hud", "ownConfig", "ownHud"];

const DISCORD = Object.freeze({
  limitBytes: 10000000,
  // Запас под контейнер MP4 и неточность битрейта кодировщика.
  containerReserve: 0.04,
  audioBps: 96000,
  maxRetries: 2,
  retrySafety: 0.97,
});
const AUDIO_BPS = { quality: 192000, preview: 128000 };
// Ниже этого видео перестаёт быть смотрибельным: бит на пиксель кадра.
const MIN_BITS_PER_PIXEL = 0.005;
const MIN_VIDEO_BPS = 250000;
// Грубая оценка размера для качества по CRF/CQ (точного размера там нет).
const ROUGH_BITS_PER_PIXEL = { quality: 0.08, preview: 0.03 };
// chapter и finish — главы HL2 и финиш рана Source (source-demo.js).
const MARKER_TYPES = new Set(["map", "segment", "saveload", "timer", "chapter", "finish"]);
const PIX_FMT = { libx264: "yuv420p", h264_nvenc: "yuv420p", h264_qsv: "nv12", h264_amf: "nv12" };

const round3 = (value) => Math.round(value * 1000) / 1000;
const num = (value) => String(round3(value));
const resolutionOf = (height) => RESOLUTIONS.find((item) => item.height === height) || null;

function normalizeSettings(raw, duration, options) {
  const input = raw && typeof raw === "object" ? raw : {};
  const presets = options && options.allowPreview ? PRESETS.concat(["preview"]) : PRESETS;
  const choose = (key, allowed) => {
    if (input[key] === undefined || input[key] === null) return DEFAULTS[key];
    if (!allowed.includes(input[key])) throw new DemoError("SETTINGS_INVALID", null, { field: key });
    return input[key];
  };
  const flag = (key) => (input[key] === undefined || input[key] === null ? DEFAULTS[key] : input[key] === true);

  const settings = {
    preset: choose("preset", presets),
    height: choose("height", RESOLUTIONS.map((item) => item.height)),
    fps: choose("fps", FPS),
    audio: flag("audio"),
    keyhud: flag("keyhud"),
    integrity: flag("integrity"),
    hud: flag("hud"),
    ownConfig: flag("ownConfig"),
    ownHud: flag("ownHud"),
    encoder: choose("encoder", ENCODER_MODES),
  };
  settings.width = resolutionOf(settings.height).width;

  const speed = input.speed === undefined || input.speed === null ? 1 : Number(input.speed);
  if (!Number.isFinite(speed) || speed < SPEED.min || speed > SPEED.max)
    throw new DemoError("SETTINGS_INVALID", null, { field: "speed" });
  settings.speed = Math.round(speed * 100) / 100;

  if (!(duration > 0)) throw new DemoError("DURATION_UNKNOWN");
  let from = 0;
  let to = duration;
  if (input.range) {
    from = Number(input.range.from);
    to = Number(input.range.to);
    // Ползунок может отдать конец чуть дальше длительности — прижимаем.
    if (Number.isFinite(to) && to > duration && to <= duration + 0.05) to = duration;
    if (!Number.isFinite(from) || !Number.isFinite(to) || from < 0 || to > duration || to <= from)
      throw new DemoError("RANGE_INVALID");
  }
  if ((to - from) / settings.speed < 0.1) throw new DemoError("RANGE_INVALID");
  settings.range = { from: round3(from), to: round3(to) };
  return settings;
}

// Цепочка atempo с множителями в [0.5, 2]: звук ускоряется без смены высоты.
function atempoChain(speed) {
  const chain = [];
  let rest = speed;
  while (rest > 2) {
    chain.push(2);
    rest /= 2;
  }
  while (rest < 0.5) {
    chain.push(0.5);
    rest /= 0.5;
  }
  chain.push(Math.round(rest * 1e6) / 1e6);
  return chain;
}

function normalizeCuts(cuts) {
  const sorted = (Array.isArray(cuts) ? cuts : [])
    .map((cut) => ({ from: Number(cut && cut.from), to: Number(cut && cut.to) }))
    .filter((cut) => Number.isFinite(cut.from) && Number.isFinite(cut.to) && cut.to > cut.from && cut.from >= 0)
    .sort((a, b) => a.from - b.from);
  const merged = [];
  for (const cut of sorted) {
    const last = merged[merged.length - 1];
    if (last && cut.from <= last.to) last.to = Math.max(last.to, cut.to);
    else merged.push(cut);
  }
  return merged;
}

// Время в материале адаптера: материал начинается с offset (время демо) и
// не содержит вырезанных участков cuts (загрузки).
function materialTime(t, timing) {
  const offset = (timing && timing.offset) || 0;
  let removed = 0;
  for (const cut of (timing && timing.cuts) || []) {
    const a = Math.max(cut.from, offset);
    const b = Math.min(cut.to, t);
    if (b > a) removed += b - a;
  }
  return Math.max(0, t - offset - removed);
}

// Метки источника -> время видео: t = (t_демо − от − вырезанное) / скорость.
function mapTimeline(markers, { from, to, speed, offset, cuts }) {
  const timing = { offset: offset === undefined ? from : offset, cuts: normalizeCuts(cuts) };
  const start = materialTime(from, timing);
  return (Array.isArray(markers) ? markers : [])
    .filter((marker) => marker && MARKER_TYPES.has(marker.type) && Number.isFinite(marker.t) &&
      marker.t >= from - 1e-6 && marker.t <= to + 1e-6)
    .map((marker) => ({
      t: round3((materialTime(marker.t, timing) - start) / speed),
      type: marker.type,
      label: String(marker.label === undefined || marker.label === null ? "" : marker.label).slice(0, 64),
      sourceTime: round3(marker.t),
    }))
    .sort((a, b) => a.t - b.t);
}

function clipDuration({ from, to, speed, offset, cuts }) {
  const timing = { offset: offset === undefined ? from : offset, cuts: normalizeCuts(cuts) };
  return round3((materialTime(to, timing) - materialTime(from, timing)) / speed);
}

// Сочетания разрешение × FPS: пересечение возможностей адаптера и
// кодировщика (supports(height, fps) — прошла ли проба).
function allowedCombos(capabilities, supports) {
  const caps = capabilities || {};
  const fpsList = FPS.filter((fps) => (caps.fps || []).includes(fps));
  const combos = [];
  for (const resolution of RESOLUTIONS) {
    if (resolution.height > (caps.maxHeight || 0)) continue;
    for (const fps of fpsList)
      if (!supports || supports(resolution.height, fps)) combos.push({ height: resolution.height, width: resolution.width, fps });
  }
  return combos;
}

function minVideoBps(width, height, fps) {
  return Math.max(MIN_VIDEO_BPS, Math.round(width * height * fps * MIN_BITS_PER_PIXEL));
}

function discordBudget(duration, audio, limitBytes) {
  const audioBps = audio ? DISCORD.audioBps : 0;
  const budgetBits = (limitBytes || DISCORD.limitBytes) * (1 - DISCORD.containerReserve) * 8;
  return { audioBps, budgetBits, videoBps: Math.floor(budgetBits / duration - audioBps) };
}

function tooLong(details, budgetBits, audioBps, settings, duration, limitBytes) {
  const floor = minVideoBps(settings.width, settings.height, settings.fps);
  const maxSeconds = Math.floor((budgetBits / (floor + audioBps)) * 10) / 10;
  // Разрешение пониже, в которое этот клип ещё влезает.
  const lower = RESOLUTIONS.filter((item) => item.height < settings.height)
    .reverse()
    .find((item) => budgetBits / duration - audioBps >= minVideoBps(item.width, item.height, settings.fps));
  return new DemoError("DISCORD_TOO_LONG", null, Object.assign({
    recommendedMaxSeconds: details.recommendedMaxSeconds === undefined ? maxSeconds : details.recommendedMaxSeconds,
    suggestedHeight: lower ? lower.height : null,
    limitBytes: limitBytes || DISCORD.limitBytes,
  }, { reason: details.reason }));
}

// План для окна экспорта и для задания. outputDuration — длительность
// итогового видео (с учётом скорости и вырезанного).
function planExport(settings, options) {
  const opts = options || {};
  const encoder = opts.encoder || CPU_ENCODER;
  const duration = opts.outputDuration;
  const plan = {
    preset: settings.preset,
    width: settings.width,
    height: settings.height,
    fps: settings.fps,
    speed: settings.speed,
    range: settings.range,
    audio: settings.audio,
    keyhud: settings.keyhud,
    encoder,
    outputDuration: duration,
    warnings: (opts.warnings || []).slice(),
  };

  if (settings.preset === "discord") {
    const budget = discordBudget(duration, settings.audio, opts.limitBytes);
    const floor = minVideoBps(settings.width, settings.height, settings.fps);
    if (budget.videoBps < floor)
      throw tooLong({ reason: "bitrate-floor" }, budget.budgetBits, budget.audioBps, settings, duration, opts.limitBytes);
    Object.assign(plan, {
      mode: "bitrate",
      videoBps: budget.videoBps,
      audioBps: budget.audioBps,
      twoPass: encoder === CPU_ENCODER,
      limitBytes: opts.limitBytes || DISCORD.limitBytes,
      estimatedBytes: Math.round(((budget.videoBps + budget.audioBps) * duration) / 8),
    });
    if (budget.videoBps < floor * 2) plan.warnings.push("DISCORD_LOW_QUALITY");
  } else {
    const audioBps = settings.audio ? AUDIO_BPS[settings.preset] || AUDIO_BPS.quality : 0;
    const bpp = ROUGH_BITS_PER_PIXEL[settings.preset] || ROUGH_BITS_PER_PIXEL.quality;
    Object.assign(plan, {
      mode: "quality",
      videoBps: null,
      audioBps,
      twoPass: false,
      limitBytes: null,
      estimatedBytes: Math.round(((settings.width * settings.height * settings.fps * bpp + audioBps) * duration) / 8),
    });
  }
  if (encoder === CPU_ENCODER && (settings.height >= 2160 || settings.fps >= 120)) plan.warnings.push("CPU_SLOW");
  return plan;
}

// Discord: итог больше лимита — битрейт с поправкой по факту. attempt —
// номер повтора (1, 2); дальше или ниже порога — ошибка с советом.
function discordRetry(plan, actualBytes, attempt) {
  const limit = plan.limitBytes || DISCORD.limitBytes;
  const budgetBits = limit * (1 - DISCORD.containerReserve) * 8;
  const settings = { width: plan.width, height: plan.height, fps: plan.fps };
  if (attempt > DISCORD.maxRetries) {
    const recommendedMaxSeconds = Math.floor(((plan.outputDuration * limit) / actualBytes) * 0.9 * 10) / 10;
    throw tooLong({ reason: "retries", recommendedMaxSeconds }, budgetBits, plan.audioBps, settings, plan.outputDuration, limit);
  }
  const audioBytes = (plan.audioBps * plan.outputDuration) / 8;
  const videoBytes = actualBytes - audioBytes;
  const wanted = (limit * (1 - DISCORD.containerReserve)) - audioBytes;
  const factor = (wanted > 0 && videoBytes > 0 ? wanted / videoBytes : (limit * (1 - DISCORD.containerReserve)) / actualBytes) *
    DISCORD.retrySafety;
  const next = Math.floor(plan.videoBps * Math.min(factor, 1));
  if (next < minVideoBps(plan.width, plan.height, plan.fps))
    throw tooLong({ reason: "bitrate-floor" }, budgetBits, plan.audioBps, settings, plan.outputDuration, limit);
  return next;
}

// overlay: { file, start } — субтитры ASS адаптера во времени материала (KeyHUD
// HL1); кадр после -ss сначала получает своё время материала (start + PTS).
function videoFilter(plan, overlay) {
  const pix = PIX_FMT[plan.encoder] || "yuv420p";
  return [
    ...(overlay ? [`setpts=PTS-STARTPTS+${num(overlay.start)}/TB`, `ass=${overlay.file}`] : []),
    `setpts=(PTS-STARTPTS)/${num(plan.speed)}`,
    `fps=${plan.fps}`,
    `scale=${plan.width}:${plan.height}:force_original_aspect_ratio=decrease:flags=lanczos`,
    `pad=${plan.width}:${plan.height}:(ow-iw)/2:(oh-ih)/2`,
    "setsar=1",
    `format=${pix}`,
  ].join(",");
}

function audioFilter(speed) {
  const chain = ["asetpts=PTS-STARTPTS"];
  if (speed !== 1) for (const factor of atempoChain(speed)) chain.push(`atempo=${factor}`);
  // apad + -t на выходе: звук не короче и не длиннее видео.
  chain.push("aresample=48000", "apad");
  return chain.join(",");
}

function videoCodecArgs(plan, pass, passlog) {
  const { encoder, preset, fps } = plan;
  const bps = String(plan.videoBps);
  const rate = ["-b:v", bps, "-maxrate", bps, "-bufsize", String(plan.videoBps * 2)];
  const common = ["-c:v", encoder, "-pix_fmt", PIX_FMT[encoder] || "yuv420p", "-r", String(fps), "-g", String(fps * 2)];
  switch (encoder) {
    case "libx264":
      if (preset === "discord")
        return common.concat(["-profile:v", "high", "-preset", "medium", "-b:v", bps, "-pass", String(pass), "-passlogfile", passlog]);
      return common.concat(["-profile:v", "high"], preset === "preview" ? ["-preset", "veryfast", "-crf", "26"] : ["-preset", "slow", "-crf", "18"]);
    case "h264_nvenc":
      if (preset === "discord") return common.concat(["-preset", "p5", "-tune", "hq", "-rc", "vbr"], rate);
      return common.concat(preset === "preview"
        ? ["-preset", "p2", "-rc", "vbr", "-cq", "28", "-b:v", "0"]
        : ["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19", "-b:v", "0"]);
    case "h264_qsv":
      if (preset === "discord") return common.concat(["-preset", "slower"], rate);
      return common.concat(preset === "preview" ? ["-preset", "veryfast", "-global_quality", "28"] : ["-preset", "slower", "-global_quality", "20"]);
    case "h264_amf":
      if (preset === "discord") return common.concat(["-quality", "quality", "-rc", "vbr_peak"], rate);
      return common.concat(preset === "preview"
        ? ["-quality", "speed", "-rc", "cqp", "-qp_i", "26", "-qp_p", "28"]
        : ["-quality", "quality", "-rc", "cqp", "-qp_i", "18", "-qp_p", "20"]);
    default:
      throw new DemoError("ENCODER_UNAVAILABLE");
  }
}

// material: { video } (файл) или { pattern, fps, startNumber } (кадры),
// audio — файл со звуком или null, overlay — ASS в папке задания (cwd) или null.
// trim — отрезок материала { start, duration }.
// pass: 0 — один проход, 1/2 — двухпроходное кодирование libx264.
function encodeArgs({ material, trim, plan, output, pass, passlog, nullTarget }) {
  const seek = ["-ss", num(trim.start), "-t", num(trim.duration)];
  const args = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"];
  if (material.pattern) {
    args.push("-framerate", String(material.fps));
    if (Number.isInteger(material.startNumber)) args.push("-start_number", String(material.startNumber));
    args.push(...seek, "-i", material.pattern);
  } else args.push(...seek, "-i", material.video);
  const withAudio = !!(plan.audio && material.audio && pass !== 1);
  if (withAudio) args.push(...seek, "-i", material.audio);
  args.push("-map", "0:v:0");
  if (withAudio) args.push("-map", "1:a:0");
  const overlay = material.overlay ? { file: material.overlay, start: trim.start } : null;
  args.push("-filter:v", videoFilter(plan, overlay), ...videoCodecArgs(plan, pass || 1, passlog));
  if (withAudio) args.push("-filter:a", audioFilter(plan.speed), "-c:a", "aac", "-b:a", String(plan.audioBps), "-ac", "2");
  else args.push("-an");
  args.push("-t", num(plan.outputDuration));
  if (pass === 1) args.push("-f", "null", nullTarget);
  else args.push("-movflags", "+faststart", output);
  return args;
}

module.exports = {
  RESOLUTIONS,
  FPS,
  PRESETS,
  ENCODER_MODES,
  GPU_ENCODERS,
  CPU_ENCODER,
  SPEED,
  DEFAULTS,
  PREVIEW,
  VIEW_KEYS,
  DISCORD,
  MARKER_TYPES,
  normalizeSettings,
  atempoChain,
  normalizeCuts,
  materialTime,
  mapTimeline,
  clipDuration,
  allowedCombos,
  minVideoBps,
  discordBudget,
  planExport,
  discordRetry,
  videoFilter,
  audioFilter,
  videoCodecArgs,
  encodeArgs,
};
